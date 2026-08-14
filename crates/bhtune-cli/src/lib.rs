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
//! - [`output`] — the `--output table|json` format shared by `history list`/`history show`
//!   and `tune`/`simulate`'s final summary, plus error formatting.
//! - [`logging`] — `tracing`/`tracing-subscriber` structured logging (`cli-logging`),
//!   initialized once in [`run`], never touching stdout so it can never interleave with
//!   `--output json`'s single-object contract.
//!
//! `main.rs` stays a one-line delegator to [`run`]; [`run_with_cli`] is the actual entry
//! point, kept separate so tests can exercise it against an already-parsed [`args::Cli`]
//! without needing to control `std::env::args()` — mirroring `opcda-bridge-client`'s
//! `run`/`run_with_cli` split. Logging is initialized in [`run`], not [`run_with_cli`], for
//! the same reason: it keeps tracing setup (and its process-global, only-succeeds-once
//! subscriber installation) entirely out of `run_with_cli`'s own large, injection-based test
//! suite -- see `logging`'s test module doc comment.
//!
//! Non-interactive/scheduled use (cron, CI, batch campaigns) is `tune`/`simulate`'s
//! `--yes`/`--write-pid <level>`/`--dry-run`/`--timeout-secs` flags (bypassing the
//! interactive write-back prompt, rehearsing a write-back, and mandatorily bounding an
//! unattended run's wall-clock duration) plus this module's distinguished exit codes
//! ([`EXIT_ABORTED`], [`EXIT_TIMED_OUT`], [`EXIT_WRITE_BACK_FAILED`]), so a scheduler can tell
//! "aborted", "timed out", "test ran but the write-back failed", and "never ran at
//! all" apart without parsing stdout. See AGENTS.md's `cli-automation`/`cli-safety` sections.

pub mod args;
pub mod backend;
pub mod commands;
pub mod config;
pub mod db;
pub mod logging;
pub mod output;
#[cfg(test)]
mod test_support;

use std::process::ExitCode;

use clap::Parser;

use args::{Cli, Command};
use output::OutputFormat;

/// Process exited normally, and if this was `tune`/`simulate`, any PID write-back either
/// succeeded or was cleanly skipped. Equal to [`ExitCode::SUCCESS`].
pub const EXIT_SUCCESS: u8 = 0;
/// A setup problem (bad flags, an unreadable config file, a database error, an unexpected
/// backend error) prevented the command from running to completion at all. Equal to
/// [`ExitCode::FAILURE`].
pub const EXIT_FAILURE: u8 = 1;
/// A `tune`/`simulate` run was aborted (Ctrl+C) before it finished; the loop was restored to
/// its pre-test mode. Distinct from [`EXIT_FAILURE`] so a scheduler can tell "someone
/// intentionally stopped this" apart from "this broke".
pub const EXIT_ABORTED: u8 = 2;
/// A `tune`/`simulate` run completed the MRFT test itself, but writing the selected PID
/// constants back to the DCS failed (the write was rejected, errored, or its confirmation
/// readback didn't match). Distinct from both [`EXIT_SUCCESS`] (nothing to report) and
/// [`EXIT_FAILURE`] (the test itself never produced a result) so an unattended
/// `--write-pid`/`--yes` run can tell "the test ran fine but the loop was NOT updated" apart
/// from either of those. See `commands::tune::TuneOutcome` and AGENTS.md's `cli-automation`
/// section.
pub const EXIT_WRITE_BACK_FAILED: u8 = 3;
/// A `tune`/`simulate` run was aborted because `--timeout-secs` elapsed before the engine
/// reported completion; the loop was restored to its pre-test mode, exactly like
/// [`EXIT_ABORTED`]. Distinct from it so a scheduler's alerting can tell "this run had to be
/// killed for running too long" (possibly a stuck relay, a misconfigured tag mapping, or a
/// stalled backend read -- worth investigating) apart from "an operator stopped it on
/// purpose" (routine). See `commands::tune::TuneOutcome::TimedOut` and AGENTS.md's
/// `cli-safety` section.
pub const EXIT_TIMED_OUT: u8 = 4;

/// Parses real CLI arguments, initializes structured logging, and runs, returning a process
/// exit code.
///
/// Logging is resolved and initialized here rather than in [`run_with_cli`] -- see the crate
/// doc comment. It reads the config file once, purely for `[log]` settings; `run_with_cli`
/// reads it again moments later for the database path and other settings. That small,
/// one-time duplication keeps `run_with_cli`'s own extensively unit-tested call path (which
/// never touches a real log directory) fully decoupled from tracing setup, which can only
/// ever be installed once per process. A logging setup failure (e.g. an unwritable log
/// directory) is deliberately non-fatal -- see [`logging::init_tracing`] -- so it never
/// prevents the actual command (and its `println!`-based result) from running.
pub async fn run() -> ExitCode {
    let cli = Cli::parse();

    let config = config::load_config(cli.config.as_deref()).unwrap_or_default();
    let default_log_dir = config::default_log_dir_from(
        std::env::var("XDG_DATA_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        cfg!(target_os = "windows"),
    );
    let log_settings = logging::resolve_log_settings(
        cli.log_level.clone(),
        cli.log_dir.clone(),
        cli.log_format.clone(),
        cli.log_rotation.clone(),
        &config.log,
        &default_log_dir,
    );
    // Held for the rest of this function's scope, which is the whole remaining lifetime of
    // the process (`main.rs` immediately returns whatever `ExitCode` this call resolves to)
    // -- dropping it any earlier would risk silently truncating buffered log lines.
    let _log_guard = logging::init_tracing(&log_settings);

    run_with_cli(cli).await
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
    // Captured before `cli.command` is moved into the dispatch match below, so a config/db
    // error can still be reported in the format the command actually asked for.
    let output_format = cli.command.output_format();

    let config = match config::load_config(cli.config.as_deref()) {
        Ok(config) => config,
        Err(e) => return fail(&e, output_format),
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
        Err(e) => fail(&e, output_format),
        Ok(pool) => {
            let result: anyhow::Result<ExitCode> = match cli.command {
                Command::Tune(args) => commands::tune::run(&pool, args, &config)
                    .await
                    .map(tune_outcome_exit_code),
                Command::Simulate(args) => {
                    commands::tune::run(&pool, args.into_tune_args(), &config)
                        .await
                        .map(tune_outcome_exit_code)
                }
                Command::Template { command } => commands::template::run(&pool, command)
                    .await
                    .map(|()| ExitCode::SUCCESS),
                Command::History { command } => commands::history::run(&pool, command)
                    .await
                    .map(|()| ExitCode::SUCCESS),
                Command::Export(args) => commands::export::run(&pool, args)
                    .await
                    .map(|()| ExitCode::SUCCESS),
                Command::Opc { command } => commands::opc::run(command, &config)
                    .await
                    .map(|()| ExitCode::SUCCESS),
            };
            match result {
                Ok(code) => code,
                Err(e) => fail(&e, output_format),
            }
        }
    }
}

/// Maps a completed `tune`/`simulate` invocation's [`commands::tune::TuneOutcome`] to a
/// process exit code -- the one place [`EXIT_ABORTED`]/[`EXIT_WRITE_BACK_FAILED`] are chosen.
fn tune_outcome_exit_code(outcome: commands::tune::TuneOutcome) -> ExitCode {
    match outcome {
        commands::tune::TuneOutcome::Completed => ExitCode::SUCCESS,
        commands::tune::TuneOutcome::Aborted => ExitCode::from(EXIT_ABORTED),
        commands::tune::TuneOutcome::TimedOut => ExitCode::from(EXIT_TIMED_OUT),
        commands::tune::TuneOutcome::WriteBackFailed => ExitCode::from(EXIT_WRITE_BACK_FAILED),
    }
}

fn fail(err: &anyhow::Error, format: OutputFormat) -> ExitCode {
    eprintln!("{}", output::format_error(err, format));
    ExitCode::from(EXIT_FAILURE)
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
            log_level: None,
            log_dir: None,
            log_format: None,
            log_rotation: None,
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
            log_level: None,
            log_dir: None,
            log_format: None,
            log_rotation: None,
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
            log_level: None,
            log_dir: None,
            log_format: None,
            log_rotation: None,
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
            log_level: None,
            log_dir: None,
            log_format: None,
            log_rotation: None,
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
                timeout_secs: 3600,
                name: None,
                dry_run: false,
                yes: false,
                write_pid: None,
                output: OutputFormat::Table,
            }),
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::FAILURE);
    }

    #[test]
    fn fail_prints_and_returns_exit_failure_in_table_format() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(fail(&err, OutputFormat::Table), ExitCode::FAILURE);
    }

    #[test]
    fn fail_returns_exit_failure_in_json_format_too() {
        // `fail`'s exit code is always `EXIT_FAILURE` regardless of the requested output
        // format -- only the printed message shape changes (see `output::format_error`).
        let err = anyhow::anyhow!("boom");
        assert_eq!(fail(&err, OutputFormat::Json), ExitCode::FAILURE);
    }

    #[test]
    fn tune_outcome_exit_code_maps_every_variant() {
        assert_eq!(
            tune_outcome_exit_code(commands::tune::TuneOutcome::Completed),
            ExitCode::SUCCESS
        );
        assert_eq!(
            tune_outcome_exit_code(commands::tune::TuneOutcome::Aborted),
            ExitCode::from(EXIT_ABORTED)
        );
        assert_eq!(
            tune_outcome_exit_code(commands::tune::TuneOutcome::TimedOut),
            ExitCode::from(EXIT_TIMED_OUT)
        );
        assert_eq!(
            tune_outcome_exit_code(commands::tune::TuneOutcome::WriteBackFailed),
            ExitCode::from(EXIT_WRITE_BACK_FAILED)
        );
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
            timeout_secs: 3600,
            name: Some("dispatch-test".to_string()),
            dry_run: false,
            yes: false,
            write_pid: None,
            output: OutputFormat::Table,
        }
    }

    #[tokio::test]
    async fn run_with_cli_dispatches_simulate_history_export_and_opc() {
        let (_dir, db) = temp_db_path();

        assert_eq!(
            run_with_cli(Cli {
                db: Some(db.clone()),
                config: None,
                log_level: None,
                log_dir: None,
                log_format: None,
                log_rotation: None,
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
                log_level: None,
                log_dir: None,
                log_format: None,
                log_rotation: None,
                command: Command::History {
                    command: crate::args::HistoryCommand::Show {
                        run_id,
                        output: OutputFormat::Table,
                    },
                },
            })
            .await,
            ExitCode::SUCCESS
        );

        assert_eq!(
            run_with_cli(Cli {
                db: Some(db.clone()),
                config: None,
                log_level: None,
                log_dir: None,
                log_format: None,
                log_rotation: None,
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
                log_level: None,
                log_dir: None,
                log_format: None,
                log_rotation: None,
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
            log_level: None,
            log_dir: None,
            log_format: None,
            log_rotation: None,
            command: Command::Template {
                command: crate::args::TemplateCommand::List,
            },
        };
        assert_eq!(run_with_cli(cli).await, ExitCode::SUCCESS);
        assert!(db.exists());
    }
}
