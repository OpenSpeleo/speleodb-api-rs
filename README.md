# speleodb-api

Async Rust SDK for SpeleoDB, generated with
[gpu-cli/openapi-to-rust](https://github.com/gpu-cli/openapi-to-rust) from the
reviewed, package-local OpenAPI snapshot. Crate name: `speleodb-api`; Rust
import: `speleodb_api`. License: AGPL-3.0-only.

```toml
[dependencies]
speleodb-api = "0.1.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust,no_run
use speleodb_api::HttpClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HttpClient::new()
        .with_base_url("https://www.speleodb.org")
        .with_api_key(std::env::var("SPELEODB_TOKEN")?);
    client.health_status(None).await?;
    Ok(())
}
```

`with_api_key` sends Bearer authentication. For SpeleoDB Token authentication,
use `.with_header("Authorization", "Token …")` instead. Session clients can
supply cookies, `X-Session-Token`, and CSRF headers as required by the selected
endpoint. Keep credentials out of logs and source code. Generated operations
provide typed request, response, parameter and error types under `types` and
`client`. Authentication session management remains the application's concern.

Accept defaults include the selected success representation and declared error
representations. This allows SpeleoDB to negotiate its renderer before returning
binary data or a redirect, without masking authentication or lookup errors with
HTTP 406. Explicit `.with_header("Accept", "…")` values take precedence,
case-insensitively. Response decoding and its declared success type stay the
same. Both buffered and streaming operations use this policy.

`HttpClient::with_client(reqwest_client)` accepts a configured Reqwest transport
for custom timeouts, TLS, proxies, cookies and redirect policy. The generated
default has a 30-second request timeout and buffers at most 8 MiB per response;
configure larger downloads with `.with_max_response_body_bytes(...)` or choose a
generated live-stream method. The default client has no automatic retries. The
default API transport does not follow redirects. `with_client` preserves the
supplied transport's timeout and redirect policy, so credential-bearing custom
transports should also use `reqwest::redirect::Policy::none()`. Signed-download
operations preserve their 302 handoff in the `ApiOpError::Api` envelope: inspect
`error.api()` for status 302 and its `Location` header, then download that URL
using a separate credential-free transport. A custom transport that follows
redirects opts into its own handoff and credential policy.

HTTP and decoding failures return an operation-specific `ApiOpError`. Its `Api`
variant preserves status, headers and original response bytes, with a typed
error body when it matches a declared error schema. Its `Transport` variant
describes failures without an inspectable response. A successful HTTP status
with malformed JSON or an unexpected success status remains inspectable through
the `Api` envelope. Read error fields deliberately; they can contain
server-provided data.

The supported contract includes the current SpeleoDB, authentication, OGC and
Git routes. Only the obsolete SpeleoDB mount `/api/v1` and `/api/v1/...` is
excluded; authentication's own `/v1` paths remain. JSON, URL-encoded, multipart
and binary request alternatives have separate generated methods. Multipart union
bodies have additional `_variant_1`, `_variant_2`, … methods with
branch-specific request types. `MultipartFile` carries file bytes, filename and
MIME type. For form fields explicitly marked `x-drf-null-as-empty`, pass the
generated empty-string variant to clear a scalar value; omit it to preserve the
value. For a clearable file field, use an empty `MultipartFile` with neither
filename nor content type; a populated `MultipartFile` uploads replacement
bytes. JSON requests retain their separate absent versus null representation.

Generated files follow the API's nested route structure, including named path
parameter directories. Examples:

```text
src/generated/client/api/v2/projects/post.rs
src/generated/client/api/v2/projects/by_id/get.rs
src/generated/client/api/v2/stations/by_id/resources/post.rs
src/generated/types/api/v2/projects/post.rs
src/generated/types/models/project/project.rs
```

These are real Rust modules, with models and operation errors re-exported from
`types` and `client` for convenient imports. The generator adapter builds this
structure from the schema and the upstream Rust syntax tree; there are no
manually maintained route tables or numbered source chunks.

Consumers compile the checked-in Rust sources. They do not install a generator
or fetch a schema. Maintainers use Cargo exclusively:

```sh
cargo xtask generate
cargo xtask check
cargo test --manifest-path xtask/Cargo.toml --locked
cargo test --locked --tests
```

See [maintenance](docs/maintenance.md) for complete checks, schema updates,
module generation and crates.io trusted publishing setup.
