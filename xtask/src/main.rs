use anyhow::{Context, Result, bail, ensure};
use openapi_to_rust::{CodeGenerator, ConfigFile, SchemaAnalyzer, TypeMapper};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs,
    path::{Path, PathBuf},
};

mod split;

fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(bytes).iter() {
        write!(&mut hex, "{byte:02x}").expect("Writing to a String cannot fail");
    }
    hex
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_owned()
}

fn load_schema(root: &Path) -> Result<Value> {
    let source: Value = serde_json::from_slice(&fs::read(root.join("openapi-source.json"))?)?;
    let digest = source
        .get("sha256")
        .and_then(Value::as_str)
        .context("openapi-source.json requires a lowercase SHA-256")?;
    ensure!(
        digest.len() == 64
            && digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "openapi-source.json requires a lowercase SHA-256"
    );
    let bytes = fs::read(root.join("OpenAPI.yaml"))?;
    ensure!(!bytes.is_empty(), "Package OpenAPI schema is empty");
    ensure!(
        sha256_hex(&bytes) == digest,
        "Package OpenAPI SHA-256 mismatch; review the snapshot and update its digest"
    );
    let document: Value = serde_yaml::from_slice(&bytes)?;
    Ok(document)
}

fn sdk_document(mut document: Value) -> Result<Value> {
    let paths = document
        .get_mut("paths")
        .and_then(Value::as_object_mut)
        .context("OpenAPI document requires paths")?;
    paths.retain(|path, _| path != "/api/v1" && !path.starts_with("/api/v1/"));
    Ok(document)
}

fn multipart_branches(document: &Value, schema: &Value, depth: usize) -> Result<Vec<Value>> {
    ensure!(
        depth < 32,
        "Cyclic or excessively nested multipart request schema"
    );
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let resolved = document
            .pointer(
                reference
                    .strip_prefix('#')
                    .context("Remote multipart reference")?,
            )
            .context("Missing multipart reference")?;
        return multipart_branches(document, resolved, depth + 1);
    }
    if let Some(branches) = schema
        .get("anyOf")
        .or_else(|| schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        let mut output = Vec::new();
        for branch in branches {
            output.extend(multipart_branches(document, branch, depth + 1)?);
        }
        return Ok(output);
    }
    ensure!(
        schema.get("type").and_then(Value::as_str) == Some("object")
            || schema.get("properties").is_some(),
        "Multipart request must resolve to object branches"
    );
    Ok(vec![schema.clone()])
}

// Form null is a wire-level empty value only where the producer explicitly
// declares it. Represent that clear as a typed empty string, rather than a
// nested Option that serde_urlencoded would silently omit.
fn without_form_null(mut schema: Value) -> Option<Value> {
    let Value::Object(object) = &mut schema else {
        return Some(schema);
    };
    if object.get("type").and_then(Value::as_str) == Some("null") {
        return None;
    }
    object.remove("nullable");
    object.remove("x-drf-null-as-empty");
    if let Some(values) = object.get_mut("enum").and_then(Value::as_array_mut) {
        values.retain(|value| !value.is_null());
        if values.is_empty() {
            return None;
        }
    }
    if let Some(types) = object.get_mut("type").and_then(Value::as_array_mut) {
        types.retain(|value| value.as_str() != Some("null"));
        if types.is_empty() {
            return None;
        }
        if types.len() == 1 {
            let sole = types[0].clone();
            object.insert("type".into(), sole);
        }
    }
    for keyword in ["anyOf", "oneOf", "allOf"] {
        let Some(Value::Array(branches)) = object.remove(keyword) else {
            continue;
        };
        let mut alternatives = Vec::new();
        for branch in branches {
            let Some(branch) = without_form_null(branch) else {
                if keyword == "allOf" {
                    return None;
                }
                continue;
            };
            // Flatten only unconstrained anyOf wrappers. Validation keywords on
            // a wrapper remain attached to it; binary/format metadata stays on
            // its original concrete branch.
            let flatten = keyword == "anyOf"
                && branch.as_object().is_some_and(|wrapper| {
                    wrapper.contains_key("anyOf")
                        && wrapper.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "anyOf"
                                    | "title"
                                    | "description"
                                    | "deprecated"
                                    | "example"
                                    | "examples"
                            )
                        })
                });
            if flatten {
                alternatives.extend(branch["anyOf"].as_array()?.iter().cloned());
            } else {
                alternatives.push(branch);
            }
        }
        if alternatives.is_empty() {
            return None;
        }
        object.insert(keyword.into(), Value::Array(alternatives));
    }
    Some(schema)
}

fn is_empty_form_string(schema: &Value) -> bool {
    schema["type"].as_str() == Some("string")
        && (schema["maxLength"].as_u64() == Some(0)
            || schema["enum"]
                .as_array()
                .is_some_and(|values| values.as_slice() == [Value::String(String::new())]))
}

fn form_clear_values(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if object.get("x-drf-null-as-empty").and_then(Value::as_bool) == Some(true) {
                let base = without_form_null(Value::Object(std::mem::take(object)));
                let clear = serde_json::json!({"type":"string", "enum":[""]});
                *schema = match base {
                    Some(mut base) if base.get("anyOf").or_else(|| base.get("oneOf")).is_some() => {
                        let keyword = if base.get("anyOf").is_some() {
                            "anyOf"
                        } else {
                            "oneOf"
                        };
                        let alternatives = base[keyword]
                            .as_array_mut()
                            .expect("Normalized union has branches");
                        if !alternatives.iter().any(is_empty_form_string) {
                            alternatives.push(clear);
                        }
                        base
                    }
                    Some(base) if is_empty_form_string(&base) => base,
                    Some(base) => serde_json::json!({"anyOf":[base, clear]}),
                    None => clear,
                };
                form_clear_values(schema);
                return;
            }
            for value in object.values_mut() {
                form_clear_values(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                form_clear_values(value);
            }
        }
        _ => {}
    }
}

// Reusing exact response schemas avoids thousands of identical error DTOs,
// especially when one operation has several request-media entry points.
fn share_response_schemas(document: &mut Value) -> Result<()> {
    let mut counts: BTreeMap<String, (usize, Value)> = BTreeMap::new();
    fn schemas(document: &mut Value, mut visit: impl FnMut(&mut Value)) {
        if let Some(paths) = document["paths"].as_object_mut() {
            for item in paths.values_mut().filter_map(Value::as_object_mut) {
                for operation in item
                    .values_mut()
                    .filter(|value| value.get("responses").is_some())
                {
                    for response in operation["responses"]
                        .as_object_mut()
                        .into_iter()
                        .flat_map(|responses| responses.values_mut())
                    {
                        for media in response
                            .get_mut("content")
                            .and_then(Value::as_object_mut)
                            .into_iter()
                            .flat_map(|media| media.values_mut())
                        {
                            if let Some(schema) = media.get_mut("schema")
                                && (schema.get("properties").is_some()
                                    || schema.get("anyOf").is_some()
                                    || schema.get("oneOf").is_some()
                                    || schema.get("allOf").is_some())
                            {
                                visit(schema);
                            }
                        }
                    }
                }
            }
        }
    }
    schemas(document, |schema| {
        let hash = sha256_hex(serde_json::to_vec(schema).expect("JSON value serializes"));
        let entry = counts.entry(hash).or_insert((0, schema.clone()));
        entry.0 += 1;
    });
    let shared: BTreeMap<_, _> = counts
        .into_iter()
        .filter(|(_, (count, _))| *count > 1)
        .map(|(hash, (_, schema))| (hash, schema))
        .collect();
    schemas(document, |schema| {
        let hash = sha256_hex(serde_json::to_vec(schema).expect("JSON value serializes"));
        if shared.contains_key(&hash) {
            *schema = serde_json::json!({"$ref": format!("#/components/schemas/SharedResponse{}", &hash[..16])});
        }
    });
    if document.get("components").is_none() {
        document["components"] = serde_json::json!({});
    }
    if document["components"].get("schemas").is_none() {
        document["components"]["schemas"] = serde_json::json!({});
    }
    for (hash, schema) in shared {
        let name = format!("SharedResponse{}", &hash[..16]);
        ensure!(
            document["components"]["schemas"].get(&name).is_none(),
            "Shared response schema name collision"
        );
        document["components"]["schemas"][&name] = schema;
    }
    Ok(())
}

fn form_scalar(document: &Value, schema: &Value, depth: usize) -> bool {
    if depth > 32 || schema["format"].as_str() == Some("binary") {
        return false;
    }
    if let Some(reference) = schema["$ref"].as_str() {
        return reference
            .strip_prefix('#')
            .and_then(|pointer| document.pointer(pointer))
            .is_some_and(|schema| form_scalar(document, schema, depth + 1));
    }
    for key in ["oneOf", "anyOf", "allOf"] {
        if let Some(branches) = schema[key].as_array() {
            return !branches.is_empty()
                && branches
                    .iter()
                    .all(|schema| form_scalar(document, schema, depth + 1));
        }
    }
    if schema["type"].as_str() == Some("array") {
        return form_scalar(document, &schema["items"], depth + 1);
    }
    matches!(
        schema["type"].as_str(),
        Some("string" | "number" | "integer" | "boolean" | "null")
    )
}

// Upstream chooses one request media type per operation. Analyze one operation
// per declared representation, then restore its original route before emission.
// This changes generation input only, never the reviewed snapshot or Rust output.
fn request_variants(document: &mut Value) -> Result<BTreeMap<String, (String, String)>> {
    let original = document["paths"]
        .as_object()
        .context("Missing paths")?
        .clone();
    let mut routes = BTreeMap::new();
    let mut identities = std::collections::BTreeSet::new();
    let methods = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];
    for (path, item) in original {
        for method in methods {
            let Some(operation) = item.get(method) else {
                continue;
            };
            let id = operation["operationId"]
                .as_str()
                .context("Every SDK operation requires operationId")?;
            ensure!(
                identities.insert(id.to_owned()),
                "Duplicate operationId: {id}"
            );
            let Some(body) = operation.get("requestBody") else {
                continue;
            };
            let body = if let Some(reference) = body.get("$ref").and_then(Value::as_str) {
                document
                    .pointer(
                        reference
                            .strip_prefix('#')
                            .context("Remote requestBody references are unsupported")?,
                    )
                    .context("Missing local requestBody reference")?
                    .clone()
            } else {
                body.clone()
            };
            let Some(content) = body.get("content").and_then(Value::as_object) else {
                continue;
            };
            let multipart_union = content
                .get("multipart/form-data")
                .and_then(|value| value.get("schema"))
                .map(|schema| multipart_branches(document, schema, 0))
                .transpose()?;
            if content.len() <= 1
                && multipart_union
                    .as_ref()
                    .is_none_or(|branches| branches.len() <= 1)
            {
                let mut adapted = body.clone();
                for (media, value) in adapted["content"].as_object_mut().unwrap() {
                    if media == "multipart/form-data"
                        || media == "application/x-www-form-urlencoded"
                    {
                        form_clear_values(value);
                    }
                }
                document["paths"][&path][method]["requestBody"] = adapted;
                continue;
            }
            document["paths"][&path]
                .as_object_mut()
                .unwrap()
                .remove(method);
            for (media, schema) in content {
                let suffix = match media.as_str() {
                    "application/json" => "json".to_owned(),
                    "multipart/form-data" => "form_data".to_owned(),
                    "application/x-www-form-urlencoded" => "url_encoded".to_owned(),
                    "*/*" => "content".to_owned(),
                    _ => media
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect(),
                };
                let branches = if media == "multipart/form-data" {
                    multipart_union
                        .as_ref()
                        .context("Missing multipart schemas")?
                        .iter()
                        .map(|branch| {
                            let mut media_schema = schema.clone();
                            media_schema["schema"] = branch.clone();
                            media_schema
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![schema.clone()]
                };
                for (index, mut schema) in branches.iter().cloned().enumerate() {
                    if media == "multipart/form-data"
                        || media == "application/x-www-form-urlencoded"
                    {
                        form_clear_values(&mut schema);
                    }
                    let branch_suffix = if branches.len() > 1 {
                        format!("_variant_{}", index + 1)
                    } else {
                        String::new()
                    };
                    let variant_id = format!("{id}_with_{suffix}{branch_suffix}");
                    ensure!(
                        identities.insert(variant_id.clone()),
                        "Request variant operationId collision: {variant_id}"
                    );
                    let synthetic = format!("/__speleodb_sdk_request/{variant_id}");
                    ensure!(
                        document["paths"].get(&synthetic).is_none(),
                        "Request variant route collision"
                    );
                    let mut variant = operation.clone();
                    variant["operationId"] = Value::String(variant_id);
                    let mut variant_body = body.clone();
                    variant_body["content"] = serde_json::json!({media: schema});
                    variant["requestBody"] = variant_body;
                    let mut variant_item = item.clone();
                    for http_method in methods {
                        variant_item.as_object_mut().unwrap().remove(http_method);
                    }
                    variant_item[method] = variant;
                    document["paths"][&synthetic] = variant_item;
                    routes.insert(synthetic, (path.clone(), id.to_owned()));
                }
            }
        }
    }
    Ok(routes)
}

fn render(root: &Path) -> Result<BTreeMap<PathBuf, String>> {
    let mut document = sdk_document(load_schema(root)?)?;
    let routes = request_variants(&mut document)?;
    share_response_schemas(&mut document)?;
    let schema_names = document["components"]["schemas"]
        .as_object()
        .map(|schemas| schemas.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let temporary = tempfile::tempdir()?;
    fs::write(
        temporary.path().join("OpenAPI.yaml"),
        serde_json::to_vec(&document)?,
    )?;
    fs::copy(
        root.join("openapi-to-rust.toml"),
        temporary.path().join("openapi-to-rust.toml"),
    )?;
    let config =
        ConfigFile::load(&temporary.path().join("openapi-to-rust.toml"))?.into_generator_config();
    // No remote source, schema extension, overlay, or sibling discovery.
    ensure!(
        config.schema_extensions.is_empty() && config.overlays.is_empty(),
        "Only the verified local snapshot may be used"
    );
    let effective_document = document.clone();
    let mut analysis =
        SchemaAnalyzer::with_type_mapper(document, TypeMapper::new(config.types.clone()))?
            .analyze()?;
    for (id, operation) in &mut analysis.operations {
        if let Some((path, source_id)) = routes.get(&operation.path) {
            operation.path.clone_from(path);
            let source = analysis
                .operation_sources
                .get_mut(id)
                .context("Missing operation source")?;
            source.path.clone_from(path);
            source.operation_id = Some(source_id.clone());
            source.json_pointer = format!(
                "/paths/{}/{}",
                path.replace('~', "~0").replace('/', "~1"),
                operation.method.to_lowercase()
            );
        }
    }
    let default_timeout_seconds = config
        .http_client_config
        .as_ref()
        .and_then(|http| http.timeout_seconds);
    let generator = CodeGenerator::new(config).with_source_provenance("OpenAPI.yaml");
    let mut text_forms = std::collections::BTreeSet::new();
    let mut scalar_forms = BTreeMap::new();
    for operation in analysis.operations.values() {
        if let Some(openapi_to_rust::analysis::RequestBodyContent::Multipart {
            validation_schema,
            encoding,
            ..
        }) = &operation.request_body
        {
            let fields: std::collections::BTreeSet<_> = validation_schema["properties"]
                .as_object()
                .into_iter()
                .flat_map(|properties| properties.iter())
                .filter(|(name, schema)| {
                    !encoding.contains_key(*name) && form_scalar(&effective_document, schema, 0)
                })
                .map(|(name, _)| name.clone())
                .collect();
            if !fields.is_empty() {
                scalar_forms.insert(operation.operation_id.clone(), fields);
            }
        }
        if let Some(openapi_to_rust::analysis::RequestBodyContent::Multipart {
            schema_name, ..
        }) = &operation.request_body
            && let openapi_to_rust::analysis::SchemaType::Object {
                properties,
                additional_properties:
                    openapi_to_rust::analysis::ObjectAdditionalProperties::Typed { value_type },
                ..
            } = &analysis.schemas[schema_name].schema_type
        {
            let is_string = |schema: &openapi_to_rust::analysis::SchemaType| matches!(schema, openapi_to_rust::analysis::SchemaType::Primitive { rust_type, .. } if rust_type == "String");
            ensure!(
                is_string(value_type)
                    && properties
                        .values()
                        .all(|property| is_string(&property.schema_type))
                    && operation.request_body_required,
                "Typed additional multipart fields require a required text-only object"
            );
            text_forms.insert(operation.operation_id.clone());
        }
    }
    let result = generator.generate_all(&mut analysis)?;
    let error_media: BTreeMap<_, std::collections::BTreeSet<_>> = analysis
        .operation_responses
        .iter()
        .map(|(id, responses)| {
            let media = responses
                .iter()
                .filter(|(status, _)| !status.starts_with('2'))
                .flat_map(|(_, response)| response.representations.keys().cloned())
                .collect();
            (id.clone(), media)
        })
        .collect();
    let mut artifacts = generator.output_artifacts(&result);
    split::partition(
        &mut artifacts,
        &schema_names,
        &text_forms,
        &scalar_forms,
        default_timeout_seconds,
        &error_media,
    )?;
    Ok(artifacts)
}

fn files(root: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    fn visit(base: &Path, dir: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(base, &entry.path(), output)?;
            } else if kind.is_file() {
                output.insert(
                    entry.path().strip_prefix(base)?.to_owned(),
                    fs::read(entry.path())?,
                );
            } else {
                bail!("Generated output must contain only regular files and directories");
            }
        }
        Ok(())
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output)?;
    Ok(output)
}

fn generate(root: &Path, check: bool) -> Result<()> {
    // Render completely before touching existing output; failures preserve it.
    let expected = render(root)?;
    let destination = root.join("src/generated");
    if check {
        let actual = files(&destination)?;
        ensure!(
            actual.keys().eq(expected.keys()),
            "Generated SDK filenames differ; run cargo xtask generate"
        );
        for (name, content) in expected {
            ensure!(
                actual[&name] == content.as_bytes(),
                "Generated SDK drift in {}; run cargo xtask generate",
                name.display()
            );
        }
    } else {
        if destination.exists() {
            fs::remove_dir_all(&destination)?;
        }
        for (name, content) in &expected {
            let path = destination.join(name);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, content)?;
        }
    }
    Ok(())
}

fn release_check(manifest: &str, reference: Option<&str>) -> Result<()> {
    let manifest: toml::Value = toml::from_str(manifest)?;
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .context("Missing package metadata")?;
    let version = package
        .get("version")
        .and_then(toml::Value::as_str)
        .context("Missing crate version")?;
    let version = semver::Version::parse(version).context("Invalid crate version")?;
    for field in ["name", "description", "license", "repository", "readme"] {
        ensure!(
            package
                .get(field)
                .and_then(toml::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty()),
            "Missing release metadata: package.{field}"
        );
    }
    if let Some(tag) = reference.and_then(|reference| reference.strip_prefix("refs/tags/")) {
        ensure!(
            tag == format!("v{version}"),
            "Release tag must match Cargo.toml version v{version}"
        );
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "generate" => generate(&root(), false),
        [command] if command == "check" => generate(&root(), true),
        [command] if command == "source-hash" => {
            let bytes = fs::read(root().join("OpenAPI.yaml"))?;
            ensure!(!bytes.is_empty(), "Package OpenAPI schema is empty");
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({"sha256": sha256_hex(bytes)}))?
            );
            Ok(())
        }
        [command] if command == "release-check" => release_check(
            &fs::read_to_string(root().join("Cargo.toml"))?,
            std::env::var("GITHUB_REF").ok().as_deref(),
        ),
        _ => bail!("Usage: cargo xtask <generate|check|source-hash|release-check>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sha256_encoding_is_lowercase_zero_padded_and_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn release_metadata_is_checked_on_branches_pull_requests_and_locally() {
        let manifest = fs::read_to_string(root().join("Cargo.toml")).unwrap();
        for reference in [
            None,
            Some("refs/heads/master"),
            Some("refs/heads/v0.1.0"),
            Some("refs/pull/42/merge"),
        ] {
            release_check(&manifest, reference).unwrap();
        }
        let mut package: toml::Value = toml::from_str(&manifest).unwrap();
        for version in ["", "not-a-version", "01.2.3"] {
            package["package"]["version"] = toml::Value::String(version.into());
            assert!(release_check(&toml::to_string(&package).unwrap(), None).is_err());
        }
        for field in ["name", "description", "license", "repository", "readme"] {
            let mut package: toml::Value = toml::from_str(&manifest).unwrap();
            package["package"].as_table_mut().unwrap().remove(field);
            assert!(release_check(&toml::to_string(&package).unwrap(), None).is_err());
            package["package"]
                .as_table_mut()
                .unwrap()
                .insert(field.into(), toml::Value::String("  ".into()));
            assert!(release_check(&toml::to_string(&package).unwrap(), None).is_err());
        }
        assert!(release_check("", None).is_err());
        let missing_version = manifest
            .lines()
            .filter(|line| !line.starts_with("version ="))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(release_check(&missing_version, None).is_err());
    }

    #[test]
    fn release_tags_must_match_the_crate_version_including_prereleases() {
        let manifest = fs::read_to_string(root().join("Cargo.toml")).unwrap();
        let mut manifest: toml::Value = toml::from_str(&manifest).unwrap();
        for version in ["1.2.3", "1.2.3-rc.1"] {
            manifest["package"]["version"] = toml::Value::String(version.into());
            let manifest = toml::to_string(&manifest).unwrap();
            release_check(&manifest, Some(&format!("refs/tags/v{version}"))).unwrap();
            assert!(release_check(&manifest, Some("refs/tags/v9.9.9")).is_err());
            assert!(release_check(&manifest, Some(&format!("refs/tags/{version}"))).is_err());
        }
    }

    fn fixture(document: &Value) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let bytes = serde_json::to_vec(document).unwrap();
        fs::write(directory.path().join("OpenAPI.yaml"), &bytes).unwrap();
        fs::write(
            directory.path().join("openapi-source.json"),
            json!({"sha256": sha256_hex(&bytes)}).to_string(),
        )
        .unwrap();
        fs::copy(
            root().join("openapi-to-rust.toml"),
            directory.path().join("openapi-to-rust.toml"),
        )
        .unwrap();
        fs::create_dir(directory.path().join("src")).unwrap();
        directory
    }

    fn document() -> Value {
        json!({"openapi":"3.0.3", "info":{"title":"Scope fixture", "version":"1"}, "paths":{
            "/api/v1":{"get":{"operationId":"obsolete_mount","responses":{"204":{"description":"empty"}}}},
            "/api/v1/projects/":{"get":{"operationId":"obsolete_child","responses":{"204":{"description":"empty"}}}},
            "/api/v10/projects/":{"get":{"operationId":"similar_prefix","responses":{"204":{"description":"empty"}}}},
            "/_allauth/browser/v1/config":{"get":{"operationId":"browser_auth","responses":{"204":{"description":"empty"}}}},
            "/api/auth/app/v1/config":{"get":{"operationId":"app_auth","responses":{"204":{"description":"empty"}}}}
        }})
    }

    #[test]
    fn real_generator_retains_auth_and_excludes_only_obsolete_mount() {
        let fixture = fixture(&document());
        generate(fixture.path(), false).unwrap();
        generate(fixture.path(), true).unwrap();
        let client = files(&fixture.path().join("src/generated/client"))
            .unwrap()
            .into_values()
            .map(|bytes| String::from_utf8(bytes).unwrap())
            .collect::<String>();
        for name in ["similar_prefix", "browser_auth", "app_auth"] {
            assert!(client.contains(&format!("pub async fn {name}(")));
        }
        assert!(!client.contains("obsolete_mount"));
        assert!(!client.contains("obsolete_child"));
    }

    #[test]
    fn rejects_digest_failures_and_preserves_output() {
        let fixture = fixture(&document());
        generate(fixture.path(), false).unwrap();
        let before = files(&fixture.path().join("src/generated")).unwrap();
        for record in [
            "null",
            "[]",
            "{}",
            r#"{"sha256":"INVALID"}"#,
            r#"{"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        ] {
            fs::write(fixture.path().join("openapi-source.json"), record).unwrap();
            assert!(generate(fixture.path(), false).is_err());
            assert_eq!(
                before,
                files(&fixture.path().join("src/generated")).unwrap()
            );
        }
    }

    #[test]
    fn detects_byte_missing_and_extra_file_drift_without_repair() {
        let fixture = fixture(&document());
        generate(fixture.path(), false).unwrap();
        let output = fixture.path().join("src/generated");
        let types = output.join("types/mod.rs");
        let original = fs::read(&types).unwrap();
        fs::write(&types, "manual change").unwrap();
        assert!(
            generate(fixture.path(), true)
                .unwrap_err()
                .to_string()
                .contains("drift")
        );
        assert_eq!(fs::read_to_string(&types).unwrap(), "manual change");
        fs::write(&types, original).unwrap();
        fs::write(output.join("extra.rs"), "extra").unwrap();
        assert!(generate(fixture.path(), true).is_err());
        fs::remove_file(output.join("extra.rs")).unwrap();
        fs::remove_file(&types).unwrap();
        assert!(generate(fixture.path(), true).is_err());
    }

    #[test]
    fn missing_empty_and_malformed_snapshots_fail() {
        for bytes in [b"".as_slice(), b"invalid: [".as_slice()] {
            let fixture = fixture(&document());
            fs::write(fixture.path().join("OpenAPI.yaml"), bytes).unwrap();
            fs::write(
                fixture.path().join("openapi-source.json"),
                json!({"sha256": sha256_hex(bytes)}).to_string(),
            )
            .unwrap();
            assert!(load_schema(fixture.path()).is_err());
        }
        let fixture = fixture(&document());
        fs::remove_file(fixture.path().join("OpenAPI.yaml")).unwrap();
        assert!(load_schema(fixture.path()).is_err());
    }

    fn bindings() -> Value {
        let directory = root().join("src/generated/bindings");
        let mut index: Value =
            serde_json::from_slice(&fs::read(directory.join("index.json")).unwrap()).unwrap();
        let mut symbols = Vec::new();
        for name in index["symbol_files"].as_array().unwrap() {
            let shard: Vec<Value> =
                serde_json::from_slice(&fs::read(directory.join(name.as_str().unwrap())).unwrap())
                    .unwrap();
            symbols.extend(shard);
        }
        index["symbols"] = Value::Array(symbols);
        index
    }

    #[test]
    fn every_generated_accept_covers_selected_success_and_supported_errors() {
        use syn::visit::Visit;
        struct Methods(BTreeMap<String, syn::ImplItemFn>);
        impl<'ast> Visit<'ast> for Methods {
            fn visit_impl_item_fn(&mut self, method: &'ast syn::ImplItemFn) {
                self.0.insert(method.sig.ident.to_string(), method.clone());
            }
        }
        #[derive(Default)]
        struct Accepts(Vec<String>);
        impl<'ast> Visit<'ast> for Accepts {
            fn visit_expr_method_call(&mut self, call: &'ast syn::ExprMethodCall) {
                use quote::ToTokens;
                if call.method == "header"
                    && call.args.first().is_some_and(|argument| {
                        argument.to_token_stream().to_string() == "reqwest :: header :: ACCEPT"
                    })
                    && let Some(syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(value),
                        ..
                    })) = call.args.iter().nth(1)
                {
                    self.0.push(value.value());
                }
                syn::visit::visit_expr_method_call(self, call);
            }
        }
        fn resolve<'a>(document: &'a Value, mut value: &'a Value) -> &'a Value {
            while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
                value = document
                    .pointer(reference.strip_prefix('#').unwrap())
                    .unwrap();
            }
            value
        }
        let document = load_schema(&root()).unwrap();
        let mut methods = Methods(BTreeMap::new());
        for (path, bytes) in files(&root().join("src/generated/client")).unwrap() {
            if path.extension().is_some_and(|extension| extension == "rs") {
                methods.visit_file(&syn::parse_file(std::str::from_utf8(&bytes).unwrap()).unwrap());
            }
        }
        let mut checked = 0;
        for symbol in bindings()["symbols"].as_array().unwrap() {
            if !symbol["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("client::HttpClient::"))
                || symbol["signature"]["asynchronous"].as_bool() != Some(true)
            {
                continue;
            }
            let operation = &symbol["operation"];
            let Some(selected) = operation["response_media_type"].as_str() else {
                continue;
            };
            let source = document
                .pointer(operation["source_json_pointer"].as_str().unwrap())
                .unwrap();
            let errors: std::collections::BTreeSet<_> = source["responses"]
                .as_object()
                .unwrap()
                .iter()
                .filter(|(status, _)| !status.starts_with('2'))
                .flat_map(|(_, response)| {
                    resolve(&document, response)["content"]
                        .as_object()
                        .into_iter()
                        .flat_map(|content| content.iter())
                        .filter(|(media, representation)| {
                            let schema = resolve(&document, &representation["schema"]);
                            media.ends_with("/json")
                                || media.ends_with("+json")
                                || media.starts_with("text/")
                                || media.as_str() == "application/octet-stream"
                                || schema["format"] == "binary"
                        })
                        .map(|(media, _)| media)
                })
                .filter(|media| media.as_str() != selected)
                .cloned()
                .collect();
            let expected = std::iter::once(selected.to_owned())
                .chain(errors)
                .collect::<Vec<_>>()
                .join(", ");
            let name = symbol["name"].as_str().unwrap();
            let mut accepts = Accepts::default();
            accepts.visit_impl_item_fn(&methods.0[name]);
            assert_eq!(accepts.0, vec![expected], "Accept contract for {name}");
            checked += 1;
        }
        assert!(checked > 0);
    }

    #[test]
    fn form_clearing_preserves_flat_binary_and_date_branches() {
        let mut schema = json!({"type":"object", "properties":{
            "file":{"anyOf":[{"type":"string", "format":"binary", "nullable":true, "x-filename-pattern":"\\.dat$"},
                {"type":"string", "maxLength":0}], "x-drf-null-as-empty":true},
            "date":{"anyOf":[{"type":"string", "format":"date", "nullable":true},
                {"type":"string", "maxLength":0}], "x-drf-null-as-empty":true},
            "clear_only":{"type":"string", "nullable":true, "enum":[null], "x-drf-null-as-empty":true}
        }});
        form_clear_values(&mut schema);
        let properties = &schema["properties"];
        assert_eq!(properties["file"]["anyOf"].as_array().unwrap().len(), 2);
        assert_eq!(properties["file"]["anyOf"][0]["format"], "binary");
        assert_eq!(
            properties["file"]["anyOf"][0]["x-filename-pattern"],
            "\\.dat$"
        );
        assert!(properties["file"]["anyOf"][0].get("nullable").is_none());
        assert_eq!(properties["date"]["anyOf"][0]["format"], "date");
        assert!(properties["date"]["anyOf"][0].get("nullable").is_none());
        assert_eq!(
            properties["clear_only"],
            json!({"type":"string", "enum":[""]})
        );
    }

    #[test]
    fn every_original_multipart_binary_field_keeps_a_file_carrier() {
        let document = load_schema(&root()).unwrap();
        let metadata = bindings();
        let symbols = metadata["symbols"].as_array().unwrap();

        // This oracle reads the original contract, never the projected schema
        // or the production multipart detection/normalization helpers.
        fn resolve<'a>(document: &'a Value, mut schema: &'a Value) -> &'a Value {
            for _ in 0..32 {
                let Some(reference) = schema["$ref"].as_str() else {
                    return schema;
                };
                schema = document
                    .pointer(reference.strip_prefix('#').unwrap())
                    .unwrap();
            }
            panic!("Cyclic schema reference in binary-field inventory");
        }
        fn branches<'a>(document: &'a Value, schema: &'a Value) -> Vec<&'a Value> {
            let schema = resolve(document, schema);
            if let Some(options) = schema
                .get("anyOf")
                .or_else(|| schema.get("oneOf"))
                .and_then(Value::as_array)
            {
                return options
                    .iter()
                    .flat_map(|option| branches(document, option))
                    .collect();
            }
            vec![schema]
        }
        fn binary(document: &Value, schema: &Value, depth: usize) -> bool {
            assert!(depth < 32, "Excessively nested multipart binary schema");
            let schema = resolve(document, schema);
            if schema["type"].as_str() == Some("string")
                && schema["format"].as_str() == Some("binary")
            {
                return true;
            }
            if schema["type"].as_str() == Some("array") {
                return binary(document, &schema["items"], depth + 1);
            }
            ["anyOf", "oneOf", "allOf"].iter().any(|keyword| {
                schema[*keyword].as_array().is_some_and(|options| {
                    options
                        .iter()
                        .any(|option| binary(document, option, depth + 1))
                })
            })
        }
        fn request_name(ty: &syn::Type) -> String {
            let syn::Type::Path(ty) = ty else {
                panic!("Unexpected request type")
            };
            let segment = ty.path.segments.last().unwrap();
            if segment.ident == "Option" || segment.ident == "Box" {
                let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                    panic!("Missing request type argument")
                };
                let syn::GenericArgument::Type(ty) = arguments.args.first().unwrap() else {
                    panic!("Unexpected request type argument")
                };
                request_name(ty)
            } else {
                segment.ident.to_string()
            }
        }

        let mut checked = 0;
        for (path, item) in document["paths"].as_object().unwrap() {
            if path == "/api/v1" || path.starts_with("/api/v1/") {
                continue;
            }
            for method in [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                let Some(body) = operation.get("requestBody") else {
                    continue;
                };
                let body = resolve(&document, body);
                let Some(media) = body["content"].get("multipart/form-data") else {
                    continue;
                };
                let alternatives = branches(&document, &media["schema"]);
                for (index, branch) in alternatives.iter().enumerate() {
                    let source_id = operation["operationId"].as_str().unwrap();
                    let mut id = if body["content"].as_object().unwrap().len() > 1
                        || alternatives.len() > 1
                    {
                        format!("{source_id}_with_form_data")
                    } else {
                        source_id.to_owned()
                    };
                    if alternatives.len() > 1 {
                        id.push_str(&format!("_variant_{}", index + 1));
                    }
                    for (field, schema) in branch["properties"]
                        .as_object()
                        .into_iter()
                        .flat_map(|properties| properties.iter())
                    {
                        if !binary(&document, schema, 0) {
                            continue;
                        }
                        let callable = symbols
                            .iter()
                            .find(|symbol| {
                                symbol["operation"]["operation_id"].as_str() == Some(id.as_str())
                                    && symbol["path"].as_str().is_some_and(|path| {
                                        path.starts_with("client::HttpClient::")
                                    })
                                    && symbol["signature"]["asynchronous"].as_bool() == Some(true)
                            })
                            .unwrap_or_else(|| panic!("Missing multipart callable {id}"));
                        let argument = callable["signature"]["arguments"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|argument| argument["name"].as_str() == Some("request"))
                            .unwrap();
                        let request_type = request_name(
                            &syn::parse_str(argument["rust_type"].as_str().unwrap()).unwrap(),
                        );
                        let model = symbols
                            .iter()
                            .find(|symbol| {
                                symbol["kind"].as_str() == Some("struct")
                                    && symbol["name"].as_str() == Some(request_type.as_str())
                            })
                            .unwrap();
                        let emitted = model["fields"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|candidate| {
                                candidate["wire_name"].as_str() == Some(field.as_str())
                            })
                            .unwrap();
                        assert!(
                            emitted["rust_type"]
                                .as_str()
                                .unwrap()
                                .contains("MultipartFile"),
                            "Binary multipart field {method} {path} ({id}).{field} lost its file carrier: {}",
                            emitted["rust_type"]
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "Binary-field inventory must exercise the snapshot"
        );
    }

    #[test]
    fn every_supported_operation_is_generated_from_the_verified_snapshot() {
        let document = load_schema(&root()).unwrap();
        let metadata = bindings();
        let methods = [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ];
        let expected: std::collections::BTreeSet<_> = document["paths"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(path, _)| *path != "/api/v1" && !path.starts_with("/api/v1/"))
            .flat_map(|(path, item)| {
                methods.iter().filter_map(move |method| {
                    item.get(method)
                        .map(|_| (path.clone(), method.to_uppercase()))
                })
            })
            .collect();
        let actual: std::collections::BTreeSet<_> = metadata["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|symbol| {
                symbol
                    .get("operation")
                    .filter(|operation| operation.is_object())
            })
            .map(|operation| {
                (
                    operation["source_path"].as_str().unwrap().to_owned(),
                    operation["source_method"].as_str().unwrap().to_uppercase(),
                )
            })
            .collect();
        assert_eq!(expected.len(), 376, "Review contract inventory changes");
        assert_eq!(actual, expected);
    }

    #[test]
    fn every_operation_retains_its_complete_request_media_inventory() {
        let metadata = bindings();
        let document = load_schema(&root()).unwrap();
        let mut actual: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
        for operation in metadata["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|symbol| symbol.get("operation").filter(|value| value.is_object()))
        {
            actual
                .entry(
                    operation["source_operation_id"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
                .or_default()
                .insert(operation["operation_id"].as_str().unwrap().to_owned());
        }
        fn choices(document: &Value, schema: &Value) -> usize {
            if let Some(reference) = schema["$ref"].as_str() {
                return choices(
                    document,
                    document
                        .pointer(reference.strip_prefix('#').unwrap())
                        .unwrap(),
                );
            }
            if let Some(branches) = schema
                .get("oneOf")
                .or_else(|| schema.get("anyOf"))
                .and_then(Value::as_array)
            {
                return branches
                    .iter()
                    .map(|schema| choices(document, schema))
                    .sum();
            }
            1
        }
        for (path, item) in document["paths"].as_object().unwrap() {
            if path == "/api/v1" || path.starts_with("/api/v1/") {
                continue;
            }
            for method in [
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ] {
                let Some(operation) = item.get(method) else {
                    continue;
                };
                let body = operation.get("requestBody").map(|body| {
                    body.get("$ref")
                        .and_then(Value::as_str)
                        .map(|reference| {
                            document
                                .pointer(reference.strip_prefix('#').unwrap())
                                .unwrap()
                        })
                        .unwrap_or(body)
                });
                let expected = body
                    .and_then(|body| body["content"].as_object())
                    .map(|content| {
                        content
                            .iter()
                            .map(|(media, value)| {
                                if media == "multipart/form-data" {
                                    choices(&document, &value["schema"])
                                } else {
                                    1
                                }
                            })
                            .sum::<usize>()
                            .max(1)
                    })
                    .unwrap_or(1);
                let id = operation["operationId"].as_str().unwrap();
                assert_eq!(
                    actual[id].len(),
                    expected,
                    "Request representation gap for {method} {path}"
                );
            }
        }
    }

    #[test]
    fn response_sharing_preserves_distinct_contract_shapes() {
        let schema = json!({"type":"object", "properties":{"detail":{"type":"string"}}, "required":["detail"]});
        let other = json!({"type":"object", "properties":{"detail":{"type":"string", "nullable":true}}, "required":["detail"]});
        let mut document = json!({"paths":{"/a":{"get":{"responses":{
            "400":{"content":{"application/json":{"schema":schema}}},
            "403":{"content":{"application/json":{"schema":schema}}},
            "404":{"content":{"application/json":{"schema":other}}}
        }}}}});
        share_response_schemas(&mut document).unwrap();
        let responses = &document["paths"]["/a"]["get"]["responses"];
        assert_eq!(
            responses["400"]["content"]["application/json"]["schema"],
            responses["403"]["content"]["application/json"]["schema"]
        );
        let reference = responses["400"]["content"]["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        assert_eq!(
            document
                .pointer(reference.strip_prefix('#').unwrap())
                .unwrap(),
            &schema
        );
        assert_eq!(
            responses["404"]["content"]["application/json"]["schema"],
            other
        );
    }

    #[test]
    fn cargo_manifest_includes_every_generator_dependency_and_feature() {
        let manifest: toml::Value =
            toml::from_str(&fs::read_to_string(root().join("Cargo.toml")).unwrap()).unwrap();
        let required: toml::Value = toml::from_str(
            &fs::read_to_string(root().join("src/generated/REQUIRED_DEPS.toml")).unwrap(),
        )
        .unwrap();
        for (name, requirement) in required["dependencies"].as_table().unwrap() {
            assert_eq!(
                &manifest["dependencies"][name], requirement,
                "Generator requirement changed: {name}"
            );
        }
    }

    #[test]
    fn generated_tree_recreates_route_modules_without_numbered_chunks() {
        let paths = files(&root().join("src/generated")).unwrap();
        for path in [
            "client/api/v2/projects/by_id/get.rs",
            "client/api/v2/stations/by_id/resources/post.rs",
            "types/api/v2/projects/post.rs",
            "types/models/project/project.rs",
            "bindings/api/v2/projects/post.json",
        ] {
            assert!(
                paths.contains_key(Path::new(path)),
                "Missing semantic module {path}"
            );
        }
        assert!(paths.keys().all(|path| {
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("part_")
        }));
        assert!(!paths.contains_key(Path::new("client.rs")));
        assert!(!paths.contains_key(Path::new("types.rs")));
        for (path, bytes) in &paths {
            if path.extension().is_some_and(|extension| extension == "rs") {
                syn::parse_file(std::str::from_utf8(bytes).unwrap()).unwrap();
            }
        }
    }

    #[test]
    fn every_request_media_variant_is_retained_and_multipart_unions_are_callable() {
        let metadata = bindings();
        let operations: std::collections::BTreeSet<_> = metadata["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|symbol| symbol["operation"]["operation_id"].as_str())
            .collect();
        for operation in [
            "v2_projects_create_with_json",
            "v2_projects_create_with_form_data",
            "v2_projects_create_with_url_encoded",
            "v2_stations_update_with_form_data_variant_1",
            "v2_stations_update_with_form_data_variant_2",
        ] {
            assert!(
                operations.contains(operation),
                "Missing request representation {operation}"
            );
        }
        let generated = files(&root().join("src/generated/client")).unwrap();
        for bytes in generated.values() {
            assert!(
                !std::str::from_utf8(bytes)
                    .unwrap()
                    .contains("must resolve to an object")
            );
        }
    }
}
