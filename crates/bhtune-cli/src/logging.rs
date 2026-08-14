//! Structured logging (`cli-logging`), matching `opcda-bridge-gateway`'s own `tracing`
//! stack and `log.*` configuration conventions (level/directory/format/rotation, resolved
//! with the same `CLI flag > env var > config file > default` precedence as every other
//! bhtune setting) -- see `crate::config::LogConfig`/`resolve_log_settings`.
//!
//! **Deliberately never writes to stdout.** `bhtune tune`/`simulate --output json` prints a
//! single machine-readable JSON object to stdout as its whole documented contract (see
//! AGENTS.md's "Automation" section); mirroring diagnostic log lines onto that same stream
//! (as `opcda-bridge-gateway`'s equivalent does, safely, since it owns stdout outright) would
//! risk interleaving free-form log text into a stream a scheduler parses as JSON. Log lines
//! go to the rotating file always, and to **stderr** (never stdout) when a console is
//! attached -- stderr can never corrupt stdout's contract, so mirroring there is free.
//!
//! This is diagnostic/operational logging only: the CLI's actual product output (the tune
//! summary, `history`/`export` listings) stays exactly what it already was, plain `println!`
//! calls in `commands::*` -- unaffected by, and independent of, whatever this module does.

use std::path::{Path, PathBuf};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

use crate::config::LogConfig;

/// Log file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable, ANSI-free (log files aren't a terminal).
    Pretty,
    /// Newline-delimited JSON, for log shippers.
    Json,
}

/// Parse the configured log format. Defaults to `Pretty` for `None` or any unrecognized
/// value -- a config typo in `log.format` should degrade gracefully rather than stop a tune
/// from running.
pub fn parse_log_format(value: Option<&str>) -> LogFormat {
    match value {
        Some(v) if v.eq_ignore_ascii_case("json") => LogFormat::Json,
        _ => LogFormat::Pretty,
    }
}

/// Parse the configured rotation policy. Defaults to `DAILY` for `None` or any unrecognized
/// value, for the same reason as [`parse_log_format`].
pub fn parse_rotation(value: Option<&str>) -> Rotation {
    match value {
        Some(v) if v.eq_ignore_ascii_case("hourly") => Rotation::HOURLY,
        Some(v) if v.eq_ignore_ascii_case("never") => Rotation::NEVER,
        _ => Rotation::DAILY,
    }
}

/// Build an `EnvFilter` from an explicit level/directive spec (e.g. `"debug"` or
/// `"bhtune_cli=debug,sqlx=warn"`), falling back to `info` if `level` is absent or fails to
/// parse. Logging misconfiguration should degrade gracefully rather than stop a tune from
/// running.
pub fn build_env_filter(level: Option<&str>) -> EnvFilter {
    level
        .and_then(|spec| EnvFilter::try_new(spec).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

/// Resolved logging settings, after applying `CLI flag > env var > config file > default`
/// precedence to each individual field.
#[derive(Debug, Clone, PartialEq)]
pub struct LogSettings {
    pub level: Option<String>,
    pub dir: PathBuf,
    pub format: LogFormat,
    pub rotation: Rotation,
}

/// Resolve every logging setting. `cli_level` already has `RUST_LOG` folded in by clap's
/// `env` attribute on `Cli::log_level`; `dir`/`format`/`rotation` have no env var, matching
/// the rest of the config surface (see `crate::config`).
pub fn resolve_log_settings(
    cli_level: Option<String>,
    cli_dir: Option<PathBuf>,
    cli_format: Option<String>,
    cli_rotation: Option<String>,
    config: &LogConfig,
    default_dir: &Path,
) -> LogSettings {
    let level = cli_level.or_else(|| config.level.clone());
    let dir = cli_dir
        .or_else(|| config.dir.clone().map(PathBuf::from))
        .unwrap_or_else(|| default_dir.to_path_buf());
    let format = parse_log_format(cli_format.as_deref().or(config.format.as_deref()));
    let rotation = parse_rotation(cli_rotation.as_deref().or(config.rotation.as_deref()));
    LogSettings {
        level,
        dir,
        format,
        rotation,
    }
}

/// Initialize the process-global tracing subscriber: a non-blocking rolling file writer
/// under `settings.dir`, plus a stderr writer when a console is actually attached (a
/// scheduled/cron invocation typically has none). Never touches stdout -- see the module
/// doc comment.
///
/// Returns the `WorkerGuard`, which the caller **must** hold for the process lifetime --
/// dropping it early silently truncates buffered log lines that haven't yet been flushed to
/// disk on exit. Best-effort: setup failing (e.g. an unwritable log directory) is
/// intentionally not fatal to the CLI's actual job (running a tune and printing its result),
/// so callers other than this module's own tests should ignore the returned `Err` rather
/// than propagate it -- see `lib.rs::run`.
pub fn init_tracing(settings: &LogSettings) -> anyhow::Result<WorkerGuard> {
    use std::io::IsTerminal;
    init_tracing_with_stderr(settings, std::io::stderr().is_terminal())
}

/// Same as [`init_tracing`], but with "is a console attached" passed in explicitly rather
/// than detected, so tests can exercise both the stderr-attached and stderr-detached layer
/// wiring deterministically.
fn init_tracing_with_stderr(
    settings: &LogSettings,
    attach_stderr: bool,
) -> anyhow::Result<WorkerGuard> {
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    std::fs::create_dir_all(&settings.dir)?;
    let file_appender =
        RollingFileAppender::new(settings.rotation.clone(), &settings.dir, "bhtune.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let filter = build_env_filter(settings.level.as_deref());

    let file_layer = match settings.format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .with_writer(non_blocking)
            .boxed(),
        LogFormat::Pretty => tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(non_blocking)
            .boxed(),
    };
    let stderr_layer =
        attach_stderr.then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init()?;
    Ok(guard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_log_format_json() {
        assert_eq!(parse_log_format(Some("json")), LogFormat::Json);
        assert_eq!(parse_log_format(Some("JSON")), LogFormat::Json);
    }

    #[test]
    fn parse_log_format_pretty() {
        assert_eq!(parse_log_format(Some("pretty")), LogFormat::Pretty);
    }

    #[test]
    fn parse_log_format_unknown_defaults_to_pretty() {
        assert_eq!(parse_log_format(Some("yaml")), LogFormat::Pretty);
    }

    #[test]
    fn parse_log_format_none_defaults_to_pretty() {
        assert_eq!(parse_log_format(None), LogFormat::Pretty);
    }

    #[test]
    fn parse_rotation_hourly() {
        assert_eq!(parse_rotation(Some("hourly")), Rotation::HOURLY);
        assert_eq!(parse_rotation(Some("HOURLY")), Rotation::HOURLY);
    }

    #[test]
    fn parse_rotation_never() {
        assert_eq!(parse_rotation(Some("never")), Rotation::NEVER);
    }

    #[test]
    fn parse_rotation_daily() {
        assert_eq!(parse_rotation(Some("daily")), Rotation::DAILY);
    }

    #[test]
    fn parse_rotation_unknown_defaults_to_daily() {
        assert_eq!(parse_rotation(Some("weekly")), Rotation::DAILY);
    }

    #[test]
    fn parse_rotation_none_defaults_to_daily() {
        assert_eq!(parse_rotation(None), Rotation::DAILY);
    }

    #[test]
    fn build_env_filter_explicit_level() {
        // `EnvFilter` has no public equality check, so assert indirectly via Debug output,
        // which includes the directive spec.
        let filter = build_env_filter(Some("debug"));
        assert!(format!("{filter}").contains("debug"));
    }

    #[test]
    fn build_env_filter_invalid_falls_back_to_info() {
        // "level=notalevel" isn't one of the recognized level names/numbers, so this is a
        // genuine parse failure (unlike e.g. "not a valid directive!!", which `EnvFilter`
        // happily accepts as a target-name filter with an implicit "trace" level).
        let filter = build_env_filter(Some("level=notalevel"));
        assert!(format!("{filter}").contains("info"));
    }

    #[test]
    fn build_env_filter_none_defaults_to_info() {
        let filter = build_env_filter(None);
        assert!(format!("{filter}").contains("info"));
    }

    #[test]
    fn resolve_log_settings_cli_wins() {
        let config = LogConfig {
            level: Some("warn".to_string()),
            dir: Some("/config/dir".to_string()),
            format: Some("json".to_string()),
            rotation: Some("hourly".to_string()),
        };
        let settings = resolve_log_settings(
            Some("debug".to_string()),
            Some(PathBuf::from("/cli/dir")),
            Some("pretty".to_string()),
            Some("never".to_string()),
            &config,
            Path::new("/default/dir"),
        );
        assert_eq!(settings.level, Some("debug".to_string()));
        assert_eq!(settings.dir, PathBuf::from("/cli/dir"));
        assert_eq!(settings.format, LogFormat::Pretty);
        assert_eq!(settings.rotation, Rotation::NEVER);
    }

    #[test]
    fn resolve_log_settings_config_wins_over_default() {
        let config = LogConfig {
            level: Some("warn".to_string()),
            dir: Some("/config/dir".to_string()),
            format: Some("json".to_string()),
            rotation: Some("hourly".to_string()),
        };
        let settings =
            resolve_log_settings(None, None, None, None, &config, Path::new("/default/dir"));
        assert_eq!(settings.level, Some("warn".to_string()));
        assert_eq!(settings.dir, PathBuf::from("/config/dir"));
        assert_eq!(settings.format, LogFormat::Json);
        assert_eq!(settings.rotation, Rotation::HOURLY);
    }

    #[test]
    fn resolve_log_settings_defaults() {
        let settings = resolve_log_settings(
            None,
            None,
            None,
            None,
            &LogConfig::default(),
            Path::new("/default/dir"),
        );
        assert_eq!(settings.level, None);
        assert_eq!(settings.dir, PathBuf::from("/default/dir"));
        assert_eq!(settings.format, LogFormat::Pretty);
        assert_eq!(settings.rotation, Rotation::DAILY);
    }

    // `tracing_subscriber`'s global subscriber can only be installed once per process, and
    // `cargo test` runs every unit test in this crate in one shared process across multiple
    // threads. Exactly one call to `try_init()` anywhere in this binary can succeed; every
    // other call (in this module or any other) observes an error. `run_with_cli` (unlike
    // `opcda-bridge-gateway`'s `run_gateway`) deliberately never calls `init_tracing` itself
    // -- only `run()` does, which has no direct unit test of its own (see `lib.rs`) -- so
    // these are the *only* in-process calls to `init_tracing`/`init_tracing_with_stderr` in
    // the whole `cargo test` binary. The tests below therefore never assert `Ok`/`Err` on the
    // *outcome* of installing -- only that every line up to and including that call actually
    // runs, which is all the 100%-line-coverage gate requires.

    #[test]
    fn init_tracing_with_stderr_covers_json_and_pretty_layers() {
        let dir = tempfile::tempdir().unwrap();
        // "off" rather than e.g. "debug": whichever of these calls wins the one-per-process
        // global-install race stays installed for the rest of this shared test binary's run,
        // so a permissive level here would otherwise leak unrelated crates' (e.g. `sqlx`'s)
        // own debug/info-level tracing spans onto stderr for every later test -- harmless to
        // correctness, but noisy. "off" exercises the exact same layer-construction code
        // paths while guaranteeing this test can never become a noisy winner.
        let json_settings = LogSettings {
            level: Some("off".to_string()),
            dir: dir.path().to_path_buf(),
            format: LogFormat::Json,
            rotation: Rotation::NEVER,
        };
        let pretty_settings = LogSettings {
            level: Some("off".to_string()),
            dir: dir.path().to_path_buf(),
            format: LogFormat::Pretty,
            rotation: Rotation::DAILY,
        };
        // Exercise both the JSON+stderr-attached and Pretty+stderr-detached combinations so
        // every layer-construction branch runs regardless of which call (if any) wins the
        // global-install race.
        let _ = init_tracing_with_stderr(&json_settings, true);
        let _ = init_tracing_with_stderr(&pretty_settings, false);
    }

    #[test]
    fn init_tracing_wrapper_detects_terminal() {
        let dir = tempfile::tempdir().unwrap();
        // See the "off" rationale on `init_tracing_with_stderr_covers_json_and_pretty_layers`.
        let settings = LogSettings {
            level: Some("off".to_string()),
            dir: dir.path().to_path_buf(),
            format: LogFormat::Pretty,
            rotation: Rotation::NEVER,
        };
        let _ = init_tracing(&settings);
    }
}
