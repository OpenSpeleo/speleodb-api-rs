# speleodb-api agent instructions

This repository owns the standalone Rust SDK crate `speleodb-api`. Read
[docs/maintenance.md](docs/maintenance.md) before changing the contract,
generation, dependencies, tests or publication. Use Cargo for every SDK and
generator operation. Do not introduce Bun, Node, npm, Python tooling, a parent
Cargo workspace, sibling imports or monorepo-only dependencies.

## Local contract and supported scope

`OpenAPI.yaml` is the complete reviewed local snapshot. `openapi-source.json`
contains its complete-byte lowercase SHA-256. Generation verifies this digest
before parsing or filtering. Never add remote schema fetching, backend revision
gates, environment schema overrides, sibling discovery or cross-repository
snapshot checks to this standalone repository. The monorepo owns its separate
local equality check against web and both SDKs.

Exclude a path only when it equals `/api/v1` or starts with `/api/v1/`.
Authentication paths such as `/_allauth/browser/v1/config` and
`/api/auth/app/v1/config`, and the similar `/api/v10/` prefix, remain supported.
Keep the real-generator scope fixture and independent contract inventory.

Import reviewed schema bytes explicitly, then run `cargo xtask source-hash` and
update the digest after review. Do not silently rewrite a digest to hide schema
drift. Preserve the imported bytes, including whitespace and newlines.

## Generated source ownership

Never manually edit, patch or format files under `src/generated/`. They include
models, operations, binding metadata and required dependencies. Correct the
schema, `openapi-to-rust.toml` or handwritten `xtask/` implementation and run
`cargo xtask generate`. Review inputs and regenerated output together.

The generator is pinned exactly in `xtask/Cargo.toml` and its own `Cargo.lock`.
Consumers compile checked-in Rust without running generation.
`cargo xtask check` reproduces output using disposable input in OS temporary
storage and compares all filenames and bytes without repairing them. CI and
`generated-sdk-sync` enforce this. Never weaken checks to preserve a manual
generated edit.

The xtask expands multiple request representations before analysis and restores
original paths and provenance before emission. Keep this transformation generic;
never maintain a second route table or rewrite generated URL strings. Preserve
JSON, multipart, URL-encoded, binary and wildcard regression witnesses.
Generated files must recreate the API route hierarchy with real Rust modules,
named resource and path-parameter directories, and named component-model
modules. Never replace this layout with numbered chunks or monolithic
client/model files.

Generated Accept defaults must include the selected success representation and
every supported declared error representation. Preserve caller Accept overrides
without weakening multipart Content-Type guards. Keep the independent
full-operation Accept inventory, local negotiation regression and actual staging
contract tests; binary-only success headers can cause DRF to return 406 before a
view performs authentication, lookup or file download.

## Development and verification

Run commands directly from this repository using stable Rust with rustfmt and
Clippy. The crate and the unpublished xtask have independent manifests and
lockfiles; no workspace is needed. Use `--locked` for checks. Maintain both
lockfiles when changing the corresponding dependency graph. Review
`src/generated/REQUIRED_DEPS.toml` and keep direct SDK dependencies and features
in sync after every generator change.

Review the contract, generate, review output, then run the maintenance guide's
full generation, formatting, Clippy, tooling tests, wire tests, documentation
and package checks. Run `prek run -a` explicitly; never install Git hooks.
Generated formatting and lint exceptions belong at the module boundary, never in
manually patched output. Extend independent operation/request representation
inventory and meaningful loopback HTTP tests when adding support; do not claim
they prove backend validation or business behavior.

## Publication and workspace preservation

Publication belongs only to this repository's `.github/workflows/ci.yml`.
Version tags must match Cargo.toml, pass standalone verification and use the
`publish` environment with crates.io trusted publishing. Keep OIDC token scope
limited to the publish job and never add persistent registry secrets or a token
fallback. Follow documented one-time registry/bootstrap prerequisites.

Never stage, unstage, stash, commit, push, tag, publish or deploy without the
user's explicit request for that action. A commit request permits staging only
its needed files. Preserve staged and unstaged changes, branches and stashes.
Keep agent plans, review notes and scratch fixtures outside every checkout under
a unique OS temporary directory. Product documentation belongs in `docs/` and
must not contain task tracking. Never install hooks or change core.hooksPath.
