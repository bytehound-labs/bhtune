//! `bhtune-cli` — the headless adapter.
//!
//! Builds the `bhtune` binary: a scriptable, no-GUI way to run an MRFT tune,
//! intended for scheduled/unattended use (cron, CI, batch tuning campaigns)
//! as well as interactive terminal use. See AGENTS.md for the planned
//! `tune`/`template`/`history`/`export`/`simulate` subcommands and the
//! `cli-safety` guardrails required before this can write PID constants to a
//! live process unattended.
//!
//! Not yet implemented beyond this placeholder — real subcommands land in
//! the `cli-commands` phase, once `core-mrft`, `backend-opcda`, and
//! `db-schema` exist.

fn main() {
    println!(
        "bhtune {} (scaffolding — no subcommands yet)",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // Placeholder code is still code: this keeps the 100%-coverage gate
    // meaningful (green because it's genuinely exercised) rather than
    // vacuous, from the very first commit. Delete once `main` does
    // something real and gains its own targeted tests.
    #[test]
    fn main_runs_without_panicking() {
        main();
    }
}
