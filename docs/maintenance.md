# Maintaining speleodb-api

This standalone repository owns its reviewed OpenAPI snapshot and generated Rust
SDK. It works independently of the monorepo, backend and TypeScript SDK. Use
stable Rust with rustfmt and Clippy. All generation and verification tooling is
implemented in Rust and invoked with Cargo.

## Ownership and reproducibility

| Location                               | Purpose                                                                          |
| -------------------------------------- | -------------------------------------------------------------------------------- |
| `OpenAPI.yaml`                         | Complete reviewed source snapshot, preserved byte-for-byte                       |
| `openapi-source.json`                  | SHA-256 of the complete source bytes                                             |
| `openapi-to-rust.toml`                 | Upstream generator configuration                                                 |
| `xtask/Cargo.toml`, `xtask/Cargo.lock` | Independent unpublished generation tool; exact openapi-to-rust 0.23.0 pin        |
| `xtask/src/main.rs`                    | Source verification, scope, request representations, generation and drift checks |
| `xtask/src/split.rs`                   | Syntax-based nested module generation and binding metadata layout                |
| `src/generated/`                       | Generated artifacts; never edit or format directly                               |
| `src/lib.rs`                           | Public exports and generated-module lint boundary                                |
| `src/form.rs`                          | Text-only multipart adapter for typed dynamic fields                             |
| `Cargo.toml`, `Cargo.lock`             | Standalone consumer crate and validation dependency graph                        |
| `tests/http.rs`                        | Real loopback HTTP serialization and response regressions                        |

`cargo xtask generate` verifies the local digest before parsing, applies the
exact obsolete-mount exclusion, and calls the pinned generator as a Rust
library. It does not download schemas or discover neighboring repositories.
Temporary schema/configuration inputs live in OS temporary storage and are
removed automatically. Generation completes before replacing existing output. A
failed source check, schema analysis or rendering leaves the old output intact.

`cargo xtask check` performs the same generation and compares every filename and
byte. It never repairs output or updates the digest. Missing, extra or modified
files fail, including the nested module tree and binding metadata. The CI
workflow and explicit prek hook run this check. Never preserve a manual output
edit by copying it into generation input or weakening the comparison.

## Importing a reviewed contract

Copy the reviewed export into `OpenAPI.yaml` explicitly, preserving its bytes.
Review the schema diff before recording its new digest:

```sh
cargo xtask source-hash
```

Copy that JSON output into `openapi-source.json`, or explicitly redirect it:

```sh
cargo xtask source-hash > openapi-source.json
cargo xtask generate
```

The digest describes the entire original snapshot, including any excluded legacy
paths. The generator discards only paths equal to `/api/v1` or beginning
`/api/v1/`. It retains `/api/v10/...`, `/_allauth/browser/v1/...`, and
`/api/auth/app/v1/...`. Both the scope fixture and the independent operation
inventory protect this distinction.

The monorepo separately compares its web snapshot with both SDK snapshots and
their digests. This integration check belongs to the parent repository. Never
copy it into standalone Rust generation, CI, hooks or publication. The package
archive includes the local snapshot and digest so each release carries its own
contract.

## Request representations and nested modules

Upstream openapi-to-rust emits one preferred request representation per
operation and monolithic client/types files. The Cargo xtask adapts its inputs
and output syntax while retaining its serializers and model generation.

For multiple request media types, generation analyzes a separate operation for
each representation. It restores each operation's original wire path and source
provenance before emitting code. Methods have `_with_json`, `_with_form_data`,
`_with_url_encoded` or a media-derived suffix. Multipart unions are expanded
into branch-specific `_variant_N` methods because upstream requires an object
schema for multipart serialization. No endpoint list is maintained by hand.

Exact repeated object response schemas are lifted into shared temporary
components before analysis, and unreachable models are pruned. This preserves
wire schemas while avoiding thousands of duplicate error models across request
representations. The original snapshot remains unchanged.

For required multipart objects with typed string additional properties, a
checked syntax transformation supplies the handwritten text-field adapter. It
preserves every declared and dynamic string field. Other unsupported additional
property shapes fail generation rather than producing a client that drops data.
Scalar-union multipart fields use the same syntax-aware adaptation to send plain
text rather than JSON parts. Explicit per-field MIME declarations, genuine JSON
objects and binary file fields retain their declared encodings.

Default client construction derives its request deadline from
`http_client.timeout_seconds` in the generator configuration. The constructor
adaptation requires the expected upstream initialization shape and fails
generation if that shape changes. The default API transport also disables
redirect following, even when the timeout configuration is absent. This keeps
documented 302 signed-storage handoffs inspectable and prevents API session
headers from reaching the storage server. Caller-supplied Reqwest transports
keep their own timeout and redirect policies through the unchanged `with_client`
constructor. Download operations expose 302 and `Location` through the `Api`
error envelope; consume the signed URL with a separate credential-free client.

For form fields carrying the backend's `x-drf-null-as-empty` extension, the
projection removes nullability along the field's union wrappers and represents a
clear as an empty-string alternative. Existing unions remain flat, preserving
binary-file detection and format/validation annotations on their concrete
branches. This avoids Serde's nested optional values silently omitting a
requested clear in URL encoding. Clearable binary fields retain the
`MultipartFile` carrier: an empty part without filename/content type sends the
empty form value; omission retains the existing file. Other nullable fields
retain their declared semantics. JSON fields are unaffected. JSON text fields
stay text; file parts use `MultipartFile` with optional filename and content
type. Do not JSON-encode opaque Git bytes.

The generator emits its complete Rust AST first. The adapter groups operation
implementations, parameter types and errors by their original path and HTTP
method. It recreates directories for static segments and `by_<parameter>`
segments, for example `client/api/v2/projects/by_id/get.rs`. Inline models use
the same route hierarchy under `types/`; reusable component schemas use named
modules grouped under `types/models/`, such as
`types/models/project/project.rs`. The modules re-export their public types so
`client::HealthStatusFormat` and `types::Project` remain convenient. Shared
transport helpers remain in `client/mod.rs`; shared model codecs remain in
`types/mod.rs`. Binding metadata follows the same resource hierarchy with a
`bindings/index.json` index. No output contains opaque numbered source chunks.

Change this adaptation in `xtask/` and regenerate; never patch generated Rust. A
generator upgrade requires reviewing operation names, serializers, model
optionality, dependencies, public module paths and wire-test results even when
the snapshot is unchanged. Update the exact xtask dependency and its lock,
regenerate, and reconcile all required dependencies and features from
`src/generated/REQUIRED_DEPS.toml` into the crate's manifest. The dependency
regression test enforces that correspondence.

## Verification

From this repository:

```sh
cargo xtask check
cargo xtask release-check
cargo fmt --all -- --check
cargo fmt --manifest-path xtask/Cargo.toml -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo clippy --manifest-path xtask/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path xtask/Cargo.toml --locked
cargo test --locked --all-targets --all-features
cargo test --locked --doc
RUSTDOCFLAGS='-D warnings' cargo doc --locked --no-deps
cargo publish --locked --dry-run
prek run -a
```

Do not install Git hooks. CI serializes Cargo compilation to bound memory use
for the generated contract. On memory-constrained hosts, set
`CARGO_BUILD_JOBS=1` for the same behavior. Source modules remain formatted by
the generator adapter; rustfmt skips that tree. Clippy exceptions are scoped to
the generated module, while handwritten tooling and tests must pass normally.

The tooling suite covers snapshot digest errors, malformed/missing input, output
preservation, filename/byte drift, exact scope filtering through the real
generator, the independent full supported-operation inventory and dependency
requirements. Wire tests use a local TCP server and inspect actual request
bytes: authentication, queries, escaped paths, JSON omission/null, URL encoding,
repeated multipart files, filenames/MIME/opaque bytes and Git downloads. They
also check typed errors, unexpected HTML, malformed success JSON, response
limits, transport errors, optional binary replacement/clearing and deadlines
against a stalled response. Deadline tests establish real socket readiness
before advancing an isolated Tokio clock. Two-server handoff witnesses verify
302/Location preservation and that the default API transport never contacts
signed storage or forwards session headers; an explicit caller-owned follow
policy is verified separately. An independent snapshot inventory requires every
binary multipart field to retain a file carrier, including nullable fields,
arrays and request-union alternatives. These tests demonstrate serialization
behavior; they do not prove backend business validation or replace backend
endpoint tests.

The package dry-run builds the actual archive outside the source tree and
verifies it. It performs no upload. Locally, before files are committed, use
`cargo publish --locked --dry-run --allow-dirty` solely to inspect and build the
working tree without changing Git state. Releases must use the clean CI
checkout.

`cargo xtask release-check` runs locally and on every CI event. It validates the
crate's semantic version and requires nonempty name, description, license,
repository and readme metadata. On a tag ref (`GITHUB_REF=refs/tags/...`), it
also requires the tag to equal `v` plus the crate version. Branch and pull
request refs do not undergo tag matching. The dependency security hooks check
both the SDK and the independent xtask manifests and lockfiles using the shared
`deny.toml` policy. Every CI job explicitly installs stable Rust.

Taplo checks TOML syntax and uses explicit schema associations from
`.taplo.toml` for Cargo manifests, Cargo configuration and the Rust toolchain.
The lint hook overrides its default catalog-discovery arguments because Taplo
0.9.3 rejects SchemaStore's updated catalog identifier. Direct schema URLs
preserve schema validation without loading that catalog.

## Publication with trusted publishers

The standalone `.github/workflows/ci.yml` verifies pull requests, master pushes
and version tags. The test job checks release metadata, enforces tag/version
matching on tags, verifies generation and completes package verification. The
publish job runs only for `v*` tags after these checks and publishes
`speleodb-api` to crates.io. Only this job has `id-token: write`.
`rust-lang/crates-io-auth-action@v1` exchanges GitHub OIDC identity for a
short-lived registry token and revokes it after the job. There is no persistent
registry secret or token fallback. Root monorepo CI never publishes this crate.

Configure the registry-side trusted publisher as follows:

| Setting           | Exact value       |
| ----------------- | ----------------- |
| Crate             | `speleodb-api`    |
| GitHub owner      | `OpenSpeleo`      |
| Repository        | `speleodb-api-rs` |
| Workflow filename | `ci.yml`          |
| Environment       | `publish`         |

Create the `publish` environment in the child GitHub repository, restrict its
release refs and configure any desired reviewers. In the crate's crates.io
settings, add the matching GitHub trusted publisher. Registry/environment setup
is external account configuration; committing the workflow does not create it.

The
[crates.io trusted publishing documentation](https://crates.io/docs/trusted-publishing)
currently requires the crate to exist before adding a publisher. If this name
has never been published, an authorized owner must bootstrap the first release
with a narrowly scoped short-lived API token, revoke it, then configure OIDC. Do
not add that bootstrap token to this workflow or repository. Subsequent releases
use only trusted publishing. See the
[official authentication action](https://github.com/rust-lang/crates-io-auth-action).

Before an authorized release, update Cargo.toml's version and Cargo.lock,
complete verification, and confirm this repository's original upstream is
`https://github.com/OpenSpeleo/speleodb-api-rs`. Publishing, tags, commits and
pushes require explicit user authorization. Publish child commits before a
parent commit records their gitlink, and verify upstream reachability. Preserve
all existing staging and stash entries throughout.

## Renderer negotiation and actual staging contracts

SpeleoDB negotiates a renderer before running a view. Its current staging
configuration offers JSON and browsable HTML renderers. A download may therefore
return binary bytes after JSON negotiation; asking only for
`application/octet-stream` produces 406 before authentication, project lookup or
empty-project handling. The `format=json` parameter filters renderer candidates
but does not bypass Accept matching. DRF selects by media specificity and
configured renderer order, and intentionally ignores quality values.

Local negotiation fixtures reproduce the binary-only failure and preserve
endpoint errors. The separate live suite uses the actual server as its oracle
for JSON/HTML, media wildcards, parameters, specificity, renderer order, quality
values and format filtering. It also checks authenticated JSON operations,
401/403, 404, 422, direct ZIP/revision/blob bytes, and a real 302 GIS source
handoff fetched anonymously. It performs no schema fetches or remote generation.
These are read-only tests: CI neither creates projects nor changes locks/files.

Configure these repository Actions secrets in each SDK repository:

| Secret                           | Purpose                                                                                      |
| -------------------------------- | -------------------------------------------------------------------------------------------- |
| `TEST_SPELEODB_INSTANCE`         | `https://stage.speleodb.org`; live tests reject other hosts                                  |
| `TEST_SPELEODB_OAUTH`            | Staging test account token with read access to the fixtures                                  |
| `TEST_SPELEODB_PROJECT_ID`       | Dedicated `sdk-negotiation-populated` Compass project with a valid ZIP, commit and Git blobs |
| `TEST_SPELEODB_EMPTY_PROJECT_ID` | Dedicated `sdk-negotiation-empty` Compass project without commits/artifacts                  |
| `TEST_SPELEODB_GIS_LAYER_ID`     | Dedicated `sdk-negotiation-source` layer whose KML contains `SDK fixture`                    |

The dedicated fixtures are provisioned once. Recreate them through SDK project
creation, mutex acquisition, a minimal Compass ZIP upload, mutex release, and
GIS KML source upload if staging is reset; then replace the corresponding
secrets. The populated ZIP contains `compass.toml` and `test.mak`, and its
`latest_commit.tree` must contain a blob. Use a staging test account and keep
credentials out of source, command output and URLs. Signed storage is fetched
with a separate client without API Authorization or cookies.

Trusted pushes, tags and same-repository pull requests run the stage contract
job. Missing secrets fail that job; they never turn a live run into a successful
skip. Fork pull requests run deterministic tests without secrets. Publication
waits for the stage contract job as well as ordinary verification.

Rust live verification is explicit because its secrets are absent from ordinary
local unit-test environments:

```sh
cargo test --test stage_negotiation --locked -- --ignored --test-threads=1
```

The local renderer regression is `cargo test --test negotiation --locked`.
Generation derives default Accept from the selected success representation plus
every supported declared error media type. The selected success representation
stays first, and the generated successful response decoder remains unchanged.
Explicit caller Accept wins, while request Content-Type safeguards and response
decoding remain intact.
