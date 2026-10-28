//! Read-only contract checks against the actual staging renderer. Run explicitly
//! with `cargo test --test stage_negotiation -- --ignored --test-threads=1`.
use speleodb_api::{ApiOpError, HttpClient, types::ProjectLatestCommit};
use std::{fmt::Debug, time::Duration};

struct Stage {
    base: String,
    token: String,
    populated: String,
    empty: String,
    layer: String,
}
impl Stage {
    fn load() -> Self {
        let required = |name| {
            std::env::var(name).unwrap_or_else(|_| panic!("Required staging secret: {name}"))
        };
        let stage = Self {
            base: required("TEST_SPELEODB_INSTANCE"),
            token: required("TEST_SPELEODB_OAUTH"),
            populated: required("TEST_SPELEODB_PROJECT_ID"),
            empty: required("TEST_SPELEODB_EMPTY_PROJECT_ID"),
            layer: required("TEST_SPELEODB_GIS_LAYER_ID"),
        };
        let url = reqwest::Url::parse(&stage.base).expect("Invalid staging origin");
        assert_eq!(url.scheme(), "https");
        assert_eq!(
            url.host_str().unwrap().to_ascii_lowercase(),
            "stage.speleodb.org"
        );
        assert!(url.username().is_empty() && url.password().is_none());
        assert!(url.query().is_none() && url.fragment().is_none() && url.path() == "/");
        uuid::Uuid::parse_str(&stage.populated).expect("Invalid populated fixture UUID");
        uuid::Uuid::parse_str(&stage.empty).expect("Invalid empty fixture UUID");
        stage
    }
    fn client(&self) -> HttpClient {
        HttpClient::new()
            .with_base_url(self.base.trim_end_matches('/'))
            .with_header("Authorization", format!("Token {}", self.token))
    }
}
fn status<T, E: Debug>(result: Result<T, ApiOpError<E>>) -> u16 {
    match result {
        Ok(_) => panic!("Expected an inspectable HTTP error/redirect"),
        Err(error) => {
            error
                .api()
                .expect("Expected an HTTP response, not a transport failure")
                .status
        }
    }
}
fn location<T, E: Debug>(result: Result<T, ApiOpError<E>>) -> String {
    let error = match result {
        Ok(_) => panic!("Expected signed-download handoff"),
        Err(error) => error,
    };
    let response = error.api().expect("Expected an HTTP handoff");
    assert_eq!(
        response.status, 302,
        "Download must pass renderer negotiation"
    );
    response
        .headers
        .get(reqwest::header::LOCATION)
        .expect("Signed handoff needs Location")
        .to_str()
        .expect("Invalid Location")
        .to_owned()
}

#[tokio::test]
#[ignore = "requires dedicated staging secrets; required in trusted CI"]
async fn actual_renderer_accept_matrix_and_json_operations() {
    let stage = Stage::load();
    let auth = stage.client().v2_user_auth_token_retrieve(None).await;
    assert!(
        auth.is_ok(),
        "Staging authentication must succeed without printing credentials"
    );
    let projects = stage
        .client()
        .v2_projects_list(None)
        .await
        .expect("Staging project list must decode");
    assert!(
        projects
            .iter()
            .any(|project| project.id.to_string() == stage.populated)
    );
    for (accept, expected) in [
        ("application/octet-stream", 406),
        ("application/xml", 406),
        ("text/plain", 406),
        ("text/html", 200),
        ("text/*", 200),
        ("application/json", 200),
        ("application/json; indent=4", 200),
        ("application/octet-stream, application/json", 200),
        ("application/*", 200),
        ("*/*", 200),
        // DRF ignores quality values; the real server is the oracle here.
        ("application/octet-stream;q=1, application/json;q=0", 200),
    ] {
        let response = stage
            .client()
            .with_header("aCcEpT", accept)
            .v2_projects_download_retrieve_by_format("compass_zip", &stage.populated, None)
            .await;
        assert_eq!(
            response
                .map(|_| 200)
                .unwrap_or_else(|error| error.api().unwrap().status),
            expected,
            "Actual renderer for Accept {accept}"
        );
    }
    // A format override filters renderers; it does not bypass Accept matching.
    assert_eq!(
        status(
            stage
                .client()
                .with_header("Accept", "text/html")
                .v2_projects_download_retrieve_by_format(
                    "compass_zip",
                    &stage.populated,
                    Some(speleodb_api::client::V2ProjectsDownloadRetrieveByFormatFormat::Json)
                )
                .await
        ),
        406
    );
    for (accept, media) in [
        ("text/html, */*", "text/html"),
        ("text/*, application/json", "application/json"),
        ("application/json;q=0, text/html;q=1", "application/json"),
        ("text/html;level=1, application/json", "text/html"),
    ] {
        let error = stage
            .client()
            .with_header("Accept", accept)
            .v2_projects_download_retrieve_by_format(
                "compass_zip",
                uuid::Uuid::nil().to_string(),
                None,
            )
            .await
            .unwrap_err();
        let response = error.api().expect("Expected real renderer response");
        assert_eq!(response.status, 404);
        assert!(
            response.headers[reqwest::header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with(media),
            "Renderer for {accept}"
        );
    }
}

#[tokio::test]
#[ignore = "requires dedicated staging secrets; required in trusted CI"]
async fn actual_download_errors_are_not_masked_by_renderer_406() {
    let stage = Stage::load();
    assert_eq!(
        status(
            stage
                .client()
                .v2_projects_download_retrieve_by_format(
                    "compass_zip",
                    uuid::Uuid::nil().to_string(),
                    None
                )
                .await
        ),
        404
    );
    assert_eq!(
        status(
            stage
                .client()
                .v2_projects_download_retrieve_by_format("compass_zip", &stage.empty, None)
                .await
        ),
        422
    );
    let invalid = HttpClient::new()
        .with_base_url(&stage.base)
        .with_header("Authorization", "Token invalid-sdk-negotiation-token");
    let actual = status(
        invalid
            .v2_projects_download_retrieve_by_format("compass_zip", &stage.populated, None)
            .await,
    );
    assert!(
        matches!(actual, 401 | 403),
        "Expected authentication error, got {actual}"
    );
}

#[tokio::test]
#[ignore = "requires dedicated staging secrets; required in trusted CI"]
async fn actual_binary_download_variants_and_signed_gis_source() {
    let stage = Stage::load();
    let client = stage.client();
    let bytes = client
        .v2_projects_download_retrieve_by_format("compass_zip", &stage.populated, None)
        .await
        .expect("Buffered ZIP must negotiate");
    assert!(bytes.starts_with(b"PK"));
    let stream = client
        .v2_projects_download_retrieve_by_format_binary_stream(
            "compass_zip",
            &stage.populated,
            None,
        )
        .await
        .expect("Streaming ZIP must negotiate");
    let mut stream = std::pin::pin!(stream);
    let mut streamed = Vec::new();
    use futures_util::TryStreamExt;
    while let Some(bytes) = stream.try_next().await.unwrap() {
        streamed.extend_from_slice(&bytes);
    }
    assert!(streamed.starts_with(b"PK"));
    let project = client
        .v2_project_retrieve(&stage.populated, None)
        .await
        .expect("Fixture metadata must decode");
    assert_eq!(project.name, "sdk-negotiation-populated");
    let Some(ProjectLatestCommit::ProjectLatestCommitInlineVariant(commit)) = project.latest_commit
    else {
        panic!("Populated fixture must have a commit");
    };
    let bytes = client
        .v2_projects_download_retrieve_by_format_at_hash(
            "compass_zip",
            &commit.id,
            &stage.populated,
            None,
        )
        .await
        .expect("Revision ZIP must negotiate");
    assert!(bytes.starts_with(b"PK"));
    let stream = client
        .v2_projects_download_retrieve_by_format_at_hash_binary_stream(
            "compass_zip",
            &commit.id,
            &stage.populated,
            None,
        )
        .await
        .expect("Revision streaming ZIP must negotiate");
    let mut stream = std::pin::pin!(stream);
    let mut revision = Vec::new();
    while let Some(bytes) = stream.try_next().await.unwrap() {
        revision.extend_from_slice(&bytes);
    }
    assert!(revision.starts_with(b"PK"));
    let blob = commit
        .tree
        .iter()
        .find(|entry| entry.r#type == "blob")
        .expect("Fixture needs a Git blob");
    let bytes = client
        .v2_projects_download_blob_retrieve(&blob.object, &stage.populated, None)
        .await
        .expect("Blob must pass JSON negotiation");
    assert!(!bytes.is_empty());
    let stream = client
        .v2_projects_download_blob_retrieve_binary_stream(&blob.object, &stage.populated, None)
        .await
        .expect("Streaming blob must pass JSON negotiation");
    let mut stream = std::pin::pin!(stream);
    let mut streamed_blob = Vec::new();
    while let Some(chunk) = stream.try_next().await.unwrap() {
        streamed_blob.extend_from_slice(&chunk);
    }
    assert_eq!(streamed_blob.as_slice(), bytes.as_ref());
    let signed = location(
        client
            .v2_gis_layers_source_retrieve(&stage.layer, None)
            .await,
    );
    // A new, anonymous client follows storage redirects with no API credentials.
    let transport = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .unwrap();
    let response = transport
        .get(signed)
        .send()
        .await
        .map_err(reqwest::Error::without_url)
        .expect("Signed source request failed");
    assert!(response.status().is_success());
    let bytes = response
        .bytes()
        .await
        .map_err(reqwest::Error::without_url)
        .expect("Signed source body failed");
    assert!(
        String::from_utf8_lossy(&bytes).contains("SDK fixture"),
        "Expected actual KML bytes from signed storage"
    );
}
