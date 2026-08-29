//! Regenerates `openapi.json` (the workspace root's checked-in OpenAPI spec) from the live
//! route/DTO annotations in `crate::openapi::ApiDoc`. Run via:
//!
//! ```sh
//! cargo run -p bhtune-server --example gen_openapi
//! ```
//!
//! CI (`checks.yml`) runs this and then `git diff --exit-code openapi.json`: if the
//! committed file doesn't match what today's code produces, the build fails. This is the
//! same regenerate-and-diff pattern the later `docs-generated-cli` phase will reuse for the
//! CLI reference/man pages/completions -- this is the first one in the repo.

use std::path::PathBuf;

use utoipa::OpenApi;

fn main() {
    let spec = bhtune_server::openapi::ApiDoc::openapi();
    let json = spec
        .to_pretty_json()
        .expect("ApiDoc must always serialize to JSON");
    let path = workspace_root().join("openapi.json");
    std::fs::write(&path, format!("{json}\n"))
        .unwrap_or_else(|err| panic!("failed to write {}: {err}", path.display()));
    println!("wrote {}", path.display());
}

/// `CARGO_MANIFEST_DIR` is this crate's own directory (`crates/bhtune-server`); the
/// workspace root is two levels up. Deliberately not relying on the process's current
/// working directory, since `cargo run --example` does not change it to the crate's own
/// directory the way some other build tools do.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/bhtune-server must be nested two levels under the workspace root")
        .to_path_buf()
}
