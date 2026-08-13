//! `bhtune-cli` — the headless adapter.
//!
//! Builds the `bhtune` binary: a scriptable, no-GUI way to run an MRFT tune and inspect its
//! history, intended for scheduled/unattended use (cron, CI, batch tuning campaigns) as well
//! as interactive terminal use.
//!
//! - [`args`] — the `clap` derive `Cli`/`Command` definitions and the wrapper enums adapting
//!   `bhtune-core`'s domain enums to `clap::ValueEnum` (required by Rust's orphan rule).
//! - [`config`] — `CLI > env > TOML config file > platform default` precedence for the
//!   database path, opcda-bridge gateway address, and default OPC server.
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
pub mod config;
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

/// Loads the config file, resolves the database path, dispatches to the requested
/// subcommand, and reports any error.
///
/// Config loading and DB-path resolution happen here (not inside `db::open` or each
/// `commands::*::run`) because this is the one call site that has access to real process
/// environment variables (`XDG_DATA_HOME`/`HOME`/`APPDATA`) -- everything downstream of this
/// function takes already-resolved values or the loaded [`config::BhtuneConfig`] itself,
/// keeping the config-precedence logic in `config.rs` fully unit-testable by injection.
pub(crate) async fn run_with_cli(cli: Cli) -> ExitCode {
    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(e) => return fail(&e),
    };
    let db_path = config::resolve_db_path(
        cli.db,
        &config,
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    );

    match db::open(&db_path).await {
        Err(e) => fail(&e),
        Ok(pool) => {
            let result = match cli.command {
                Command::Tune(args) => commands::tune::run(&pool, args, &config).await,
                Command::Simulate(args) => {
                    commands::tune::run(&pool, args.into_tune_args(), &config).await
                }
                Command::Template { command } => commands::template::run(&pool, command).await,
                Command::History { command } => commands::history::run(&pool, command).await,
                Command::Export(args) => commands::export::run(&pool, args).await,
                Command::Opc { command } => commands::opc::run(command, &config).await,
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
            db: Some(PathBuf::from("/nonexistent-dir/bhtune.db")),
            config: None,
            command: Command::Template {
                command: crate::args::TemplateCommand::List,
            },
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::FAILURE);
    }

    #[tokio::test]
    async fn run_with_cli_explicit_config_path_failure_is_exit_failure() {
        // An explicit `--config` path that doesn't exist is a hard error (unlike
        // auto-discovery, which silently falls back to defaults) -- confirms `run_with_cli`
        // surfaces `config::load_config`'s error before ever touching the database.
        let cli = Cli {
            db: None,
            config: Some(PathBuf::from("/nonexistent/bhtune.toml")),
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
            db: Some(db),
            config: None,
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
            db: Some(db),
            config: None,
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
                bridge_host: None,
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
                db: Some(db.clone()),
                config: None,
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
                db: Some(db.clone()),
                config: None,
                command: Command::History {
                    command: crate::args::HistoryCommand::Show { run_id },
                },
            })
            .await,
            ExitCode::SUCCESS
        );

        assert_eq!(
            run_with_cli(Cli {
                db: Some(db.clone()),
                config: None,
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
                db: Some(db),
                config: None,
                command: Command::Opc {
                    command: crate::args::OpcCommand::Read {
                        bridge_host: Some("127.0.0.1:1".to_string()),
                        server: Some("Sim.Server".to_string()),
                        tags: vec!["Unit1.LIC101.PV".to_string()],
                    },
                },
            })
            .await,
            ExitCode::FAILURE
        );
    }

    #[tokio::test]
    async fn run_with_cli_resolves_db_path_from_config_file_when_cli_flag_is_unset() {
        let (_dir, db) = temp_db_path();
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        writeln!(config_file, "db = {:?}", db.to_str().unwrap()).unwrap();

        let cli = Cli {
            db: None,
            config: Some(config_file.path().to_path_buf()),
            command: Command::Template {
                command: crate::args::TemplateCommand::List,
            },
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::SUCCESS);
        assert!(db.exists());
    }
}
