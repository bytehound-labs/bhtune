//! `bhtune-server` — placeholder for the primary v1 GUI adapter.
//!
//! BHTune's only GUI is a browser-based web app: this crate will become an
//! Axum server exposing `bhtune-core`/`bhtune-backend`/`bhtune-db` over an
//! OpenAPI-described HTTP API (`server-http-api`), embedding the built React
//! SPA into the binary (`server-embed-spa`) so a target host needs no Node,
//! nginx, or WebView runtime — see "Key architectural decisions" in
//! AGENTS.md. This crate is a placeholder until that phase starts; it
//! intentionally has no `axum` dependency yet (see AGENTS.md's "Deferred
//! setup" section for why adding it prematurely is avoided).

fn main() {
    println!("bhtune-server: not implemented yet (see AGENTS.md, server-http-api phase)");
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
