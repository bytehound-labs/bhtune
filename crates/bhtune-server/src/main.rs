//! `bhtune-server` — roadmap stub for an HTTP/REST adapter.
//!
//! Not part of v1 scope (see the locked decisions in AGENTS.md: v1 adapters
//! are the CLI and the Tauri desktop GUI only). This crate is a placeholder
//! so the roadmap item — an Axum-based server exposing the same
//! `bhtune-core`/`bhtune-backend`/`bhtune-db` stack over HTTP, so a browser
//! or a remote `httpClient` frontend build can drive BHTune without a local
//! Tauri install — has a home in the workspace when it is prioritized.

fn main() {
    println!("bhtune-server: roadmap stub, not implemented (see AGENTS.md)");
}

#[cfg(test)]
mod tests {
    use super::*;

    // See the matching note in bhtune-cli/src/main.rs: keeps the coverage
    // gate meaningful rather than vacuous. Delete once this does something
    // real and gains its own targeted tests.
    #[test]
    fn main_runs_without_panicking() {
        main();
    }
}
