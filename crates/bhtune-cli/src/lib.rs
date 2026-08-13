//! `bhtune-cli` — the headless adapter.
//!
//! Builds the `bhtune` binary: a scriptable, no-GUI way to run an MRFT tune and inspect its
//! history, intended for scheduled/unattended use (cron, CI, batch tuning campaigns) as well
//! as interactive terminal use.
//!
//! - [`args`] — the `clap` derive `Cli`/`Command` definitions and the wrapper enums adapting
//!   `bhtune-core`'s domain enums to `clap::ValueEnum` (required by Rust's orphan rule).
//! - [`db`] — opens the database and seeds built-in templates on every startup.
//! - [`backend`] — constructs the selected `Backend` implementation.
//! - [`commands`] — one module per subcommand family: `tune`/`simulate`, `template`,
//!   `history`, `export`, `opc`.
//!
//! `main.rs` stays a one-line delegator to [`run`]; [`run_with_cli`] is the actual entry
//! point, kept separate so tests can exercise it against an already-parsed [`args::Cli`]
//! without needing to control `std::env::args()` — mirroring `opcda-bridge-client`'s
//! `run`/`run_with_cli` split.

pub mod args;
pub mod backend;
pub mod commands;
pub mod db;
#[cfg(test)]
mod test_support;

use std::process::ExitCode;

use clap::Parser;

use args::{Cli, Command};

/// Parses real CLI arguments and runs, returning a process exit code.
pub async fn run() -> ExitCode {
    run_with_cli(Cli::parse()).await
}

/// Opens the database, dispatches to the requested subcommand, and reports any error.
pub(crate) async fn run_with_cli(cli: Cli) -> ExitCode {
    match db::open(&cli.db).await {
        Err(e) => fail(&e),
        Ok(pool) => {
            let result = match cli.command {
                Command::Tune(args) => commands::tune::run(&pool, args).await,
                Command::Simulate(args) => commands::tune::run(&pool, args.into_tune_args()).await,
                Command::Template { command } => commands::template::run(&pool, command).await,
                Command::History { command } => commands::history::run(&pool, command).await,
                Command::Export(args) => commands::export::run(&pool, args).await,
                Command::Opc { command } => commands::opc::run(command).await,
            };
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&e),
            }
        }
    }
}

fn fail(err: &anyhow::Error) -> ExitCode {
    eprintln!("error: {err:#}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{BackendKindArg, Command, ControllerTypeArg, ProcessTypeArg, TuneArgs};
    use std::path::PathBuf;

    fn temp_db_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bhtune.db");
        (dir, path)
    }

    #[tokio::test]
    async fn run_with_cli_config_load_failure_is_exit_failure() {
        // An unwritable directory as the DB path is a hard error opening the database.
        let cli = Cli {
            db: PathBuf::from("/nonexistent-dir/bhtune.db"),
            command: Command::Template {
                command: crate::args::TemplateCommand::List,
            },
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::FAILURE);
    }

    #[tokio::test]
    async fn run_with_cli_success_is_exit_success() {
        let (_dir, db) = temp_db_path();
        let cli = Cli {
            db,
            command: Command::Template {
                command: crate::args::TemplateCommand::List,
            },
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::SUCCESS);
    }

    #[tokio::test]
    async fn run_with_cli_command_error_is_exit_failure() {
        let (_dir, db) = temp_db_path();
        let cli = Cli {
            db,
            command: Command::Tune(TuneArgs {
                tagname: "Unit1.LIC101.PV".to_string(),
                template: "Nonexistent Template".to_string(),
                process_type: ProcessTypeArg::Flow,
                controller_type: ControllerTypeArg::Pi,
                relay_amp: 10.0,
                cycles_skip: None,
                cycles_count: None,
                noise_protection_secs: None,
                mrft_delay: 0,
                backend: BackendKindArg::Simulator,
                bridge_host: String::new(),
                server: None,
                sim_gain: 1.0,
                sim_tau: 2.0,
                sim_dead_time: 5.0,
                sim_noise: 0.0,
                sim_seed: 0,
                sim_initial_pv: 50.0,
                sim_initial_mv: 50.0,
                pv_range_high: Some(100.0),
                pv_range_low: Some(0.0),
                mv_range_high: Some(100.0),
                mv_range_low: Some(0.0),
                direction: Some(crate::args::DirectionArg::Reverse),
                poll_interval_ms: 800,
                name: None,
            }),
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::FAILURE);
    }

    #[test]
    fn fail_prints_and_returns_exit_failure() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(fail(&err), ExitCode::FAILURE);
    }

    /// Every `SimulateArgs` field explicitly set to a fast-converging demo run (mirroring
    /// `commands::tune::tests::fast_simulator_args`), so `Command::Simulate`'s dispatch test
    /// below finishes in well under a second rather than using the real 800 ms default.
    fn fast_simulate_args() -> crate::args::SimulateArgs {
        crate::args::SimulateArgs {
            tagname: "Sim.Loop1.PV".to_string(),
            template: "Yokogawa CentumVP".to_string(),
            process_type: ProcessTypeArg::Flow,
            controller_type: ControllerTypeArg::Pi,
            relay_amp: 10.0,
            cycles_skip: Some(1),
            cycles_count: Some(2),
            noise_protection_secs: Some(0),
            mrft_delay: 0,
            sim_gain: 1.0,
            sim_tau: 0.01,
            sim_dead_time: 0.025,
            sim_noise: 0.0,
            sim_seed: 0,
            sim_initial_pv: 50.0,
            sim_initial_mv: 50.0,
            poll_interval_ms: 5,
            name: Some("dispatch-test".to_string()),
        }
    }

    #[tokio::test]
    async fn run_with_cli_dispatches_simulate_history_export_and_opc() {
        let (_dir, db) = temp_db_path();

        assert_eq!(
            run_with_cli(Cli {
                db: db.clone(),
                command: Command::Simulate(fast_simulate_args()),
            })
            .await,
            ExitCode::SUCCESS
        );

        // Look the run up directly so `History`/`Export` dispatch against a real run id
        // rather than a placeholder that would only exercise their own error paths.
        let pool = bhtune_db::connect(&db).await.unwrap();
        let runs = bhtune_db::models::TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(1),
        )
        .await
        .unwrap();
        let run_id = runs[0].id;
        pool.close().await;

        assert_eq!(
            run_with_cli(Cli {
                db: db.clone(),
                command: Command::History {
                    command: crate::args::HistoryCommand::Show { run_id },
                },
            })
            .await,
            ExitCode::SUCCESS
        );

        assert_eq!(
            run_with_cli(Cli {
                db: db.clone(),
                command: Command::Export(crate::args::ExportArgs {
                    run_id,
                    format: crate::args::ExportFormat::Json,
                    output: None,
                }),
            })
            .await,
            ExitCode::SUCCESS
        );

        // `Command::Opc`'s own subcommand behavior is already covered directly in
        // `commands::opc`'s tests; here we only need to prove the dispatch arm is reached —
        // an unreachable gateway host fails promptly and still counts as "dispatched".
        assert_eq!(
            run_with_cli(Cli {
                db,
                command: Command::Opc {
                    command: crate::args::OpcCommand::Read {
                        bridge_host: "127.0.0.1:1".to_string(),
                        server: "Sim.Server".to_string(),
                        tags: vec!["Unit1.LIC101.PV".to_string()],
                    },
                },
            })
            .await,
            ExitCode::FAILURE
        );
    }
}
