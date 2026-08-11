//! `bhtune-desktop` — placeholder for the Tauri v2 desktop GUI.
//!
//! Intentionally has **no Tauri dependency yet**. Adding real `tauri` crates
//! now would require system GTK/WebKit packages this workspace doesn't
//! assume are present, and would risk breaking `cargo build --workspace` /
//! CI before the `tauri-runner` phase is actually ready to use them. This
//! crate exists as a placeholder so the workspace member list matches the
//! target architecture in AGENTS.md from day one, and so the eventual Tauri
//! scaffolding has a home.
//!
//! When implemented, this will host a Tauri v2 app whose commands route to
//! `bhtune-core`/`bhtune-backend`/`bhtune-db`, paired with a React + TS +
//! Vite frontend under `frontend/` (not yet created — see AGENTS.md).

fn main() {
    println!("bhtune-desktop: not implemented yet (see AGENTS.md, tauri-runner phase)");
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
