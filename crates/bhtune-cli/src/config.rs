//! `CLI flag > env var > TOML config file > built-in default` precedence for bhtune's global
//! settings (database location, opcda-bridge gateway address, default OPC server), mirroring
//! `opcda-bridge-client`'s `config.rs` (see AGENTS.md's `cli-config` notes) so both projects'
//! configuration surfaces stay recognizable to the same user.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default opcda-bridge gateway address bhtune connects to when nothing else specifies one.
pub const DEFAULT_BRIDGE_HOST: &str = "localhost:7600";

/// Default address `bhtune-server` binds to when nothing else specifies one -- loopback
/// only, matching the "v1 binds to `127.0.0.1` by default" decision in AGENTS.md. Lives
/// alongside [`DEFAULT_BRIDGE_HOST`] in this shared config module (rather than in
/// `bhtune-server` itself) even though only the server binary ever calls
/// [`resolve_bind_addr`], the same way `templates`/`log` below are settings only some
/// commands consume -- one `bhtune.toml` file and one precedence chain for every bhtune
/// setting, CLI or server.
pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8787";

/// bhtune's configuration, loaded from an optional TOML file. Every field is optional; a
/// value missing from the file (or the file itself missing) falls back to the env var / CLI
/// flag / built-in default resolution in the `resolve_*` functions below.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BhtuneConfig {
    /// Overrides the default SQLite database path (see [`default_db_path_from`]).
    pub db: Option<PathBuf>,
    /// Overrides [`DEFAULT_BRIDGE_HOST`] for every `tune --driver opcda` and `opc`
    /// subcommand invocation that doesn't pass `--bridge-host` explicitly.
    pub bridge_host: Option<String>,
    /// Default OPC DA server ProgID, used when `--server` is omitted. Unlike the other
    /// fields there is no built-in default -- if this is unset and `--server` is omitted,
    /// the command errors (see [`resolve_server`]).
    pub server: Option<String>,
    /// Overrides the default user-supplied DCS/PLC template catalog path (see
    /// [`templates_path_from`]). A file here is loaded on every startup in addition to the
    /// embedded built-in catalog (see `crate::db::open` and [`load_user_templates`]),
    /// attributed `TemplateOrigin::Catalog`.
    pub templates: Option<PathBuf>,
    /// Overrides [`DEFAULT_BIND_ADDR`] -- the `host:port` `bhtune-server` listens on. Only
    /// meaningful to the server binary; see [`resolve_bind_addr`].
    pub bind: Option<String>,
    /// Age-based history retention (`history-retention`): tune runs with `started_at` older
    /// than this many days are deleted automatically on every startup (both binaries, via
    /// `crate::db::open`) and, for `bhtune-server`, again on a periodic timer while it keeps
    /// running -- see `crate::retention`. `None` (the default) means retain forever: there is
    /// no built-in number of days, since at this project's data volumes (see AGENTS.md's
    /// History explorer notes) an unexpected auto-delete of someone's baseline tune is a
    /// worse failure mode than an ever-growing database file. See [`resolve_retention_days`].
    pub retention_days: Option<u32>,
    /// `[log]` sub-table: level/directory/format/rotation for `crate::logging`'s tracing
    /// setup, mirroring `opcda-bridge-gateway`'s own `log.*` config conventions.
    #[serde(default)]
    pub log: LogConfig,
}

/// Logging configuration keys (a `[log]` table in `bhtune.toml`), consumed by
/// `crate::logging::resolve_log_settings`. Every field is optional and falls back through
/// the same `CLI flag > env var > config file > default` precedence as the rest of
/// [`BhtuneConfig`].
#[derive(Debug, Default, Clone, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct LogConfig {
    pub level: Option<String>,
    pub dir: Option<String>,
    pub format: Option<String>,
    pub rotation: Option<String>,
}

/// Derive bhtune's config file location from raw environment values rather than reading
/// `std::env` directly -- keeps discovery fully unit-testable across every permutation
/// without mutating real process environment variables.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\bhtune.toml`.
/// - Elsewhere: `$XDG_CONFIG_HOME/bhtune/bhtune.toml`, falling back to
///   `$HOME/.config/bhtune/bhtune.toml`.
pub fn config_path_from(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    if is_windows {
        return appdata.map(|dir| Path::new(dir).join("bhtune").join("bhtune.toml"));
    }
    if let Some(dir) = xdg_config_home {
        return Some(Path::new(dir).join("bhtune").join("bhtune.toml"));
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".config")
            .join("bhtune")
            .join("bhtune.toml")
    })
}

/// Derive bhtune's default *user template catalog* location the same way [`config_path_from`]
/// derives `bhtune.toml`'s -- deliberately the same directory, since both are per-user
/// settings a site admin edits by hand, not persistent application data (contrast
/// [`default_db_path_from`]/[`default_log_dir_from`], which live under the platform data
/// directory instead). See [`load_user_templates`] for how this default fits into the full
/// `template-user-catalog` precedence chain.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\templates.toml`.
/// - Elsewhere: `$XDG_CONFIG_HOME/bhtune/templates.toml`, falling back to
///   `$HOME/.config/bhtune/templates.toml`.
pub fn templates_path_from(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> Option<PathBuf> {
    if is_windows {
        return appdata.map(|dir| Path::new(dir).join("bhtune").join("templates.toml"));
    }
    if let Some(dir) = xdg_config_home {
        return Some(Path::new(dir).join("bhtune").join("templates.toml"));
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".config")
            .join("bhtune")
            .join("templates.toml")
    })
}

/// Derive bhtune's default *database* location the same way config files are discovered, but
/// under the platform's data directory rather than its config directory -- a database is
/// persistent user data, not settings, per the XDG base directory specification.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\bhtune.db`.
/// - Elsewhere: `$XDG_DATA_HOME/bhtune/bhtune.db`, falling back to
///   `$HOME/.local/share/bhtune/bhtune.db`.
/// - A database location must always resolve to *something* usable (unlike the config
///   file, whose absence is fine) -- if none of the above are available, this falls back
///   further to `bhtune.db` in the current directory, matching the CLI's original
///   hardcoded placeholder default.
pub fn default_db_path_from(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    if is_windows {
        return appdata
            .map(|dir| Path::new(dir).join("bhtune").join("bhtune.db"))
            .unwrap_or_else(|| PathBuf::from("bhtune.db"));
    }
    if let Some(dir) = xdg_data_home {
        return Path::new(dir).join("bhtune").join("bhtune.db");
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".local")
            .join("share")
            .join("bhtune")
            .join("bhtune.db")
    })
    .unwrap_or_else(|| PathBuf::from("bhtune.db"))
}

/// Derive bhtune's default *log directory* the same way the database path is derived (see
/// [`default_db_path_from`]) -- under the platform data directory, not next to the compiled
/// binary. Unlike `opcda-bridge-gateway`'s equivalent (`log_dir_from_exe`), a `cargo
/// install`ed binary's own directory (e.g. `~/.cargo/bin/`) isn't a sensible place to write
/// logs, and bhtune already has this exact precedence machinery for the database, so the log
/// directory reuses it rather than inventing a second convention.
///
/// - Windows (`is_windows = true`): `%APPDATA%\bhtune\logs`.
/// - Elsewhere: `$XDG_DATA_HOME/bhtune/logs`, falling back to `$HOME/.local/share/bhtune/logs`.
/// - Falls back further to `logs` in the current directory if none of the above are
///   available, matching [`default_db_path_from`]'s own final fallback.
pub fn default_log_dir_from(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    if is_windows {
        return appdata
            .map(|dir| Path::new(dir).join("bhtune").join("logs"))
            .unwrap_or_else(|| PathBuf::from("logs"));
    }
    if let Some(dir) = xdg_data_home {
        return Path::new(dir).join("bhtune").join("logs");
    }
    home.map(|dir| {
        Path::new(dir)
            .join(".local")
            .join("share")
            .join("bhtune")
            .join("logs")
    })
    .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Load a bhtune config from `path`.
///
/// A missing file resolves to `Ok(BhtuneConfig::default())` when `missing_is_error` is
/// false (the auto-discovered path may legitimately not exist yet); with an explicit
/// `--config` path a missing file is a hard error instead. A file that exists but fails to
/// parse as TOML is always a hard error -- a config typo should never be silently ignored.
pub fn load_config_file(path: &Path, missing_is_error: bool) -> anyhow::Result<BhtuneConfig> {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_config_contents(&contents)
            .map_err(|e| anyhow::anyhow!("failed to parse config file {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !missing_is_error => {
            Ok(BhtuneConfig::default())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(anyhow::anyhow!("config file not found: {}", path.display()))
        }
        Err(e) => Err(anyhow::anyhow!(
            "failed to read config file {}: {e}",
            path.display()
        )),
    }
}

/// Parses the in-memory TOML representation of a config file.
///
/// Keeping the parser separate from filesystem discovery gives property tests and fuzz
/// targets a narrow, side-effect-free boundary to exercise.
pub fn parse_config_contents(contents: &str) -> anyhow::Result<BhtuneConfig> {
    toml::from_str(contents).map_err(Into::into)
}

/// Load the config from an auto-discovered path, falling back to defaults when no path
/// could be discovered at all (e.g. neither `XDG_CONFIG_HOME` nor `HOME` is set on a
/// non-Windows host). Split out from [`load_config`] so the no-path-discovered branch is
/// directly unit-testable with a literal `None`, without mutating real process-global
/// environment variables in a parallel test binary.
fn load_discovered_config(path: Option<PathBuf>) -> anyhow::Result<BhtuneConfig> {
    match path {
        Some(p) => load_config_file(&p, false),
        None => Ok(BhtuneConfig::default()),
    }
}

/// Resolve and load the bhtune config: an explicit `--config` path if given, otherwise the
/// platform's auto-discovered path (silently falls back to defaults if none of the relevant
/// environment variables are set, or if the discovered file doesn't exist).
pub fn load_config(explicit_path: Option<&Path>) -> anyhow::Result<BhtuneConfig> {
    match explicit_path {
        Some(path) => load_config_file(path, true),
        None => {
            let path = config_path_from(
                std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
                std::env::var("HOME").ok().as_deref(),
                std::env::var("APPDATA").ok().as_deref(),
                cfg!(target_os = "windows"),
            );
            load_discovered_config(path)
        }
    }
}

/// Resolve and load the user-supplied DCS/PLC template catalog (`template-user-catalog`):
/// `--templates` / `BHTUNE_TEMPLATES` (already folded into `cli_templates` by clap's `env`
/// attribute) / the config file's `templates` key, or else the platform's auto-discovered
/// `templates.toml` next to `bhtune.toml` (see [`templates_path_from`]) -- `CLI flag > env
/// var > config file > platform default`, the same precedence chain as every other bhtune
/// setting.
///
/// Returns `Ok(None)` when no catalog applies at all: nothing was requested explicitly and
/// no file exists at the auto-discovered default path -- the common case, since most
/// installs never create this file. A path named *explicitly* (by any of the first three
/// tiers) that doesn't exist is a hard error, exactly mirroring `bhtune.toml` itself
/// (`load_config_file`'s `missing_is_error`); a file that exists (whichever tier it came
/// from) but fails to parse as TOML or fails `DcsTemplate::validate()` is always a hard
/// error naming the file and the problem -- a malformed catalog should never be silently
/// ignored. Parsing and validating are both handled by
/// [`bhtune_core::template::parse_catalog`], so this function does no TOML-shape or
/// cross-field checking of its own.
pub fn load_user_templates(
    cli_templates: Option<PathBuf>,
    config: &BhtuneConfig,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> anyhow::Result<Option<Vec<bhtune_core::DcsTemplate>>> {
    let (path, missing_is_error) = match cli_templates.or_else(|| config.templates.clone()) {
        Some(explicit) => (Some(explicit), true),
        None => (
            templates_path_from(xdg_config_home, home, appdata, is_windows),
            false,
        ),
    };
    let Some(path) = path else {
        return Ok(None);
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => bhtune_core::template::parse_catalog(&contents)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("failed to parse templates file {}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !missing_is_error => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(anyhow::anyhow!(
            "templates file not found: {}",
            path.display()
        )),
        Err(e) => Err(anyhow::anyhow!(
            "failed to read templates file {}: {e}",
            path.display()
        )),
    }
}

/// Resolve the database path with `CLI flag > env var > config file > platform default`
/// precedence. The env var is already folded into `cli_db` by clap's `env` attribute on
/// `Cli::db`; the platform-default tier takes its own raw environment values (rather than
/// reading `std::env` internally, like [`default_db_path_from`]) so this stays fully
/// unit-testable without touching real process environment variables.
#[allow(clippy::too_many_arguments)]
pub fn resolve_db_path(
    cli_db: Option<PathBuf>,
    config: &BhtuneConfig,
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    appdata: Option<&str>,
    is_windows: bool,
) -> PathBuf {
    cli_db
        .or_else(|| config.db.clone())
        .unwrap_or_else(|| default_db_path_from(xdg_data_home, home, appdata, is_windows))
}

/// Resolve the opcda-bridge gateway address with `CLI flag > env var > config file >
/// default` precedence. The env var is already folded into `cli_host` by clap's `env`
/// attribute on `TuneArgs::bridge_host`/`OpcCommand`'s per-variant `bridge_host`.
pub fn resolve_bridge_host(cli_host: Option<String>, config: &BhtuneConfig) -> String {
    cli_host
        .or_else(|| config.bridge_host.clone())
        .unwrap_or_else(|| DEFAULT_BRIDGE_HOST.to_string())
}

/// Resolve the history retention policy (`history-retention`) with `CLI flag > env var >
/// config file > default` precedence, matching [`resolve_bridge_host`]'s shape. The env var
/// is already folded into `cli_days` by clap's `env` attribute on `Cli::retention_days`.
/// `None` means retain forever -- there is no built-in default number of days; see
/// [`BhtuneConfig::retention_days`] for why.
pub fn resolve_retention_days(cli_days: Option<u32>, config: &BhtuneConfig) -> Option<u32> {
    cli_days.or(config.retention_days)
}

/// Resolve `bhtune-server`'s bind address with `CLI flag > env var > config file > default`
/// precedence, matching [`resolve_bridge_host`]'s shape exactly. `bhtune-server` has no
/// `clap` dependency (see AGENTS.md's "Deferred setup"), so unlike `resolve_bridge_host` the
/// env var isn't folded in by a derive attribute upstream -- callers pass
/// `std::env::var("BHTUNE_BIND").ok()` (or a real CLI flag, if one is ever added) directly as
/// `cli_bind`.
pub fn resolve_bind_addr(cli_bind: Option<String>, config: &BhtuneConfig) -> String {
    cli_bind
        .or_else(|| config.bind.clone())
        .unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string())
}

/// Resolve the OPC DA server ProgID with `CLI flag > config file` precedence, erroring if
/// neither is set -- there's no sensible default for which OPC server to talk to.
pub fn resolve_server(cli_server: Option<String>, config: &BhtuneConfig) -> anyhow::Result<String> {
    cli_server.or_else(|| config.server.clone()).ok_or_else(|| {
        anyhow::anyhow!(
            "no OPC server specified: pass --server or set `server` in the bhtune config file"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest::proptest! {
        #[test]
        fn serialized_configs_round_trip(
            db in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bridge_host in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            server in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            templates in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bind in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            retention_days in prop::option::of(any::<u32>()),
            level in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            dir in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            format in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            rotation in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
        ) {
            let config = BhtuneConfig {
                db: db.map(PathBuf::from),
                bridge_host,
                server,
                templates: templates.map(PathBuf::from),
                bind,
                retention_days,
                log: LogConfig {
                    level,
                    dir,
                    format,
                    rotation,
                },
            };
            let encoded = toml::to_string(&config).unwrap();
            prop_assert_eq!(parse_config_contents(&encoded).unwrap(), config);
        }

        #[test]
        fn arbitrary_config_text_never_panics(input in any::<String>()) {
            let _ = parse_config_contents(&input);
        }
    }
    use std::io::Write;

    #[test]
    fn config_path_from_windows_with_appdata() {
        let path = config_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Roaming/bhtune/bhtune.toml"
            ))
        );
    }

    #[test]
    fn config_path_from_windows_no_appdata() {
        assert_eq!(
            config_path_from(Some("/xdg"), Some("/home"), None, true),
            None
        );
    }

    #[test]
    fn config_path_from_unix_xdg_config_home() {
        let path = config_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/bhtune.toml")));
    }

    #[test]
    fn config_path_from_unix_falls_back_to_home() {
        let path = config_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            Some(PathBuf::from("/home/me/.config/bhtune/bhtune.toml"))
        );
    }

    #[test]
    fn config_path_from_unix_no_env_vars() {
        assert_eq!(config_path_from(None, None, None, false), None);
    }

    #[test]
    fn config_path_from_unix_xdg_takes_precedence_over_home() {
        let path = config_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/bhtune.toml")));
    }

    #[test]
    fn templates_path_from_windows_with_appdata() {
        let path = templates_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            Some(PathBuf::from(
                r"C:\Users\me\AppData\Roaming/bhtune/templates.toml"
            ))
        );
    }

    #[test]
    fn templates_path_from_windows_no_appdata() {
        assert_eq!(
            templates_path_from(Some("/xdg"), Some("/home"), None, true),
            None
        );
    }

    #[test]
    fn templates_path_from_unix_xdg_config_home() {
        let path = templates_path_from(Some("/xdg"), Some("/home/me"), None, false);
        assert_eq!(path, Some(PathBuf::from("/xdg/bhtune/templates.toml")));
    }

    #[test]
    fn templates_path_from_unix_falls_back_to_home() {
        let path = templates_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            Some(PathBuf::from("/home/me/.config/bhtune/templates.toml"))
        );
    }

    #[test]
    fn templates_path_from_unix_no_env_vars() {
        assert_eq!(templates_path_from(None, None, None, false), None);
    }

    #[test]
    fn default_db_path_from_windows_with_appdata() {
        let path = default_db_path_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\me\AppData\Roaming/bhtune/bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_windows_no_appdata_falls_back_to_cwd() {
        assert_eq!(
            default_db_path_from(None, None, None, true),
            PathBuf::from("bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_unix_xdg_data_home() {
        let path = default_db_path_from(Some("/xdg-data"), Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/xdg-data/bhtune/bhtune.db"));
    }

    #[test]
    fn default_db_path_from_unix_falls_back_to_home() {
        let path = default_db_path_from(None, Some("/home/me"), None, false);
        assert_eq!(
            path,
            PathBuf::from("/home/me/.local/share/bhtune/bhtune.db")
        );
    }

    #[test]
    fn default_db_path_from_unix_no_env_vars_falls_back_to_cwd() {
        assert_eq!(
            default_db_path_from(None, None, None, false),
            PathBuf::from("bhtune.db")
        );
    }

    #[test]
    fn default_log_dir_from_windows_with_appdata() {
        let path = default_log_dir_from(None, None, Some(r"C:\Users\me\AppData\Roaming"), true);
        assert_eq!(
            path,
            PathBuf::from(r"C:\Users\me\AppData\Roaming/bhtune/logs")
        );
    }

    #[test]
    fn default_log_dir_from_windows_no_appdata_falls_back_to_cwd() {
        assert_eq!(
            default_log_dir_from(None, None, None, true),
            PathBuf::from("logs")
        );
    }

    #[test]
    fn default_log_dir_from_unix_xdg_data_home() {
        let path = default_log_dir_from(Some("/xdg-data"), Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/xdg-data/bhtune/logs"));
    }

    #[test]
    fn default_log_dir_from_unix_falls_back_to_home() {
        let path = default_log_dir_from(None, Some("/home/me"), None, false);
        assert_eq!(path, PathBuf::from("/home/me/.local/share/bhtune/logs"));
    }

    #[test]
    fn default_log_dir_from_unix_no_env_vars_falls_back_to_cwd() {
        assert_eq!(
            default_log_dir_from(None, None, None, false),
            PathBuf::from("logs")
        );
    }

    #[test]
    fn load_config_file_valid() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "db = \"/data/bhtune.db\"\nbridge_host = \"gateway:7600\"\nserver = \"Kepware.KEPServerEX.V6\""
        )
        .unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(config.db, Some(PathBuf::from("/data/bhtune.db")));
        assert_eq!(config.bridge_host, Some("gateway:7600".to_string()));
        assert_eq!(config.server, Some("Kepware.KEPServerEX.V6".to_string()));
        assert_eq!(config.log, LogConfig::default());
    }

    #[test]
    fn load_config_file_parses_the_log_table() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[log]\nlevel = \"debug\"\ndir = \"/var/log/bhtune\"\nformat = \"json\"\nrotation = \"hourly\""
        )
        .unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(
            config.log,
            LogConfig {
                level: Some("debug".to_string()),
                dir: Some("/var/log/bhtune".to_string()),
                format: Some("json".to_string()),
                rotation: Some("hourly".to_string()),
            }
        );
    }

    #[test]
    fn load_config_file_empty_is_all_defaults() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_config_file_malformed() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "db = 12345").unwrap();
        let err = load_config_file(file.path(), true).unwrap_err();
        assert!(err.to_string().contains("failed to parse config file"));
    }

    #[test]
    fn load_config_file_missing_not_error() {
        let config = load_config_file(Path::new("/nonexistent/bhtune.toml"), false).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_config_file_missing_is_error() {
        let err = load_config_file(Path::new("/nonexistent/bhtune.toml"), true).unwrap_err();
        assert!(err.to_string().contains("config file not found"));
    }

    #[test]
    fn load_config_file_generic_io_error() {
        // Reading a directory as a file fails with an `IsADirectory`-style error, distinct
        // from `NotFound` -- exercises the catch-all I/O error branch (e.g. permission
        // denied in real usage).
        let dir = tempfile::tempdir().unwrap();
        let err = load_config_file(dir.path(), true).unwrap_err();
        assert!(err.to_string().contains("failed to read config file"));
    }

    #[test]
    fn load_config_explicit_path() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "bridge_host = \"custom:9999\"").unwrap();
        let config = load_config(Some(file.path())).unwrap();
        assert_eq!(config.bridge_host, Some("custom:9999".to_string()));
    }

    #[test]
    fn load_config_explicit_path_missing_errors() {
        let err = load_config(Some(Path::new("/nonexistent/bhtune.toml"))).unwrap_err();
        assert!(err.to_string().contains("config file not found"));
    }

    #[test]
    fn load_config_default_discovery() {
        // No file will exist next to the test binary's own executable path, so this
        // exercises the "missing, not an error" auto-discovery path against whatever the
        // real test machine's environment happens to be.
        let config = load_config(None).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    #[test]
    fn load_discovered_config_with_no_path_found_is_all_defaults() {
        // Exercises the "neither XDG_CONFIG_HOME nor HOME is set" case directly, without
        // mutating real process environment variables (unsafe/flaky in a parallel test
        // binary) to force `config_path_from` itself to return `None`.
        let config = load_discovered_config(None).unwrap();
        assert_eq!(config, BhtuneConfig::default());
    }

    /// A minimal, `DcsTemplate::validate()`-passing `[[template]]` block: non-empty `name`,
    /// PV/MV suffixes, and every other field either populated or left as an intentionally
    /// empty (not missing) suffix -- see `bhtune_core::template::DcsTemplate::validate`.
    fn valid_templates_toml(name: &str) -> String {
        format!(
            r#"
[[template]]
name = "{name}"
revert_mode = false
proportional_type = "band"
integral_type = "reset_time"
integral_unit = "seconds"
derivative_type = "derivative_time"
derivative_unit = "seconds"
process_variable_suffix = "PV"
manipulated_variable_suffix = "MV"
setpoint_variable_suffix = "SV"
controller_direction_suffix = ""
controller_mode_suffix = ""
mode_attribute_suffix = ""
upper_pv_range_suffix = "SH"
lower_pv_range_suffix = "SL"
upper_mv_range_suffix = "MSH"
lower_mv_range_suffix = "MSL"
proportional_constant_suffix = "P"
integral_constant_suffix = "I"
derivative_constant_suffix = "D"
mode_manual_value = ""
mode_auto_value = ""
controller_action_direct_value = "0"
"#
        )
    }

    #[test]
    fn load_user_templates_nothing_explicit_and_nothing_discovered_returns_none() {
        let templates =
            load_user_templates(None, &BhtuneConfig::default(), None, None, None, false).unwrap();
        assert_eq!(templates, None);
    }

    #[test]
    fn load_user_templates_auto_discovered_path_missing_is_not_an_error() {
        // Points the auto-discovery tier at a real, empty directory that (by construction of
        // a fresh tempdir) has no `bhtune/templates.toml` inside it -- proves a missing file
        // at the *discovered* default is `Ok(None)`, not an error, unlike the explicit-path
        // case below.
        let dir = tempfile::tempdir().unwrap();
        let templates = load_user_templates(
            None,
            &BhtuneConfig::default(),
            Some(dir.path().to_str().unwrap()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(templates, None);
    }

    #[test]
    fn load_user_templates_explicit_cli_path_missing_is_an_error() {
        let err = load_user_templates(
            Some(PathBuf::from("/nonexistent/templates.toml")),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("templates file not found"));
    }

    #[test]
    fn load_user_templates_explicit_config_key_path_missing_is_an_error() {
        let config = BhtuneConfig {
            templates: Some(PathBuf::from("/nonexistent/templates.toml")),
            ..Default::default()
        };
        let err = load_user_templates(None, &config, None, None, None, false).unwrap_err();
        assert!(err.to_string().contains("templates file not found"));
    }

    #[test]
    fn load_user_templates_valid_file_is_parsed_and_validated() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        write!(file, "{}", valid_templates_toml("Test Template")).unwrap();
        let templates = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Test Template");
    }

    #[test]
    fn load_user_templates_malformed_toml_is_an_error_naming_the_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "this is not valid toml [[[").unwrap();
        let err = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("failed to parse templates file"));
        assert!(message.contains(&file.path().display().to_string()));
    }

    #[test]
    fn load_user_templates_a_template_failing_validation_is_an_error() {
        // `manipulated_variable_suffix` left empty fails `DcsTemplate::validate()` --
        // proves `parse_catalog`'s validation pass (not just its TOML-shape parsing) is
        // surfaced as a hard error too.
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let toml = valid_templates_toml("Broken Template").replace(
            "manipulated_variable_suffix = \"MV\"",
            "manipulated_variable_suffix = \"\"",
        );
        write!(file, "{toml}").unwrap();
        let err = load_user_templates(
            Some(file.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("manipulated_variable_suffix must not be empty")
        );
    }

    #[test]
    fn load_user_templates_generic_io_error_is_an_error() {
        // Reading a directory as a file fails with an `IsADirectory`-style error, distinct
        // from `NotFound` -- exercises the catch-all I/O error branch (e.g. permission
        // denied in real usage), mirroring `load_config_file_generic_io_error`.
        let dir = tempfile::tempdir().unwrap();
        let err = load_user_templates(
            Some(dir.path().to_path_buf()),
            &BhtuneConfig::default(),
            None,
            None,
            None,
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("failed to read templates file"));
    }

    #[test]
    fn load_user_templates_cli_flag_wins_over_config_key() {
        let mut cli_file = tempfile::NamedTempFile::new().unwrap();
        write!(cli_file, "{}", valid_templates_toml("From CLI Flag")).unwrap();
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        write!(config_file, "{}", valid_templates_toml("From Config File")).unwrap();

        let config = BhtuneConfig {
            templates: Some(config_file.path().to_path_buf()),
            ..Default::default()
        };
        let templates = load_user_templates(
            Some(cli_file.path().to_path_buf()),
            &config,
            None,
            None,
            None,
            false,
        )
        .unwrap()
        .unwrap();
        assert_eq!(templates[0].name, "From CLI Flag");
    }

    #[test]
    fn load_user_templates_config_key_is_used_when_no_cli_flag_is_given() {
        let mut config_file = tempfile::NamedTempFile::new().unwrap();
        write!(config_file, "{}", valid_templates_toml("From Config File")).unwrap();
        let config = BhtuneConfig {
            templates: Some(config_file.path().to_path_buf()),
            ..Default::default()
        };
        let templates = load_user_templates(None, &config, None, None, None, false)
            .unwrap()
            .unwrap();
        assert_eq!(templates[0].name, "From Config File");
    }

    #[test]
    fn resolve_db_path_cli_wins() {
        let config = BhtuneConfig {
            db: Some(PathBuf::from("/config/bhtune.db")),
            ..Default::default()
        };
        let resolved = resolve_db_path(
            Some(PathBuf::from("/cli/bhtune.db")),
            &config,
            Some("/xdg-data"),
            Some("/home/me"),
            None,
            false,
        );
        assert_eq!(resolved, PathBuf::from("/cli/bhtune.db"));
    }

    #[test]
    fn resolve_db_path_config_wins_over_platform_default() {
        let config = BhtuneConfig {
            db: Some(PathBuf::from("/config/bhtune.db")),
            ..Default::default()
        };
        let resolved = resolve_db_path(None, &config, Some("/xdg-data"), None, None, false);
        assert_eq!(resolved, PathBuf::from("/config/bhtune.db"));
    }

    #[test]
    fn resolve_db_path_falls_back_to_platform_default() {
        let resolved = resolve_db_path(
            None,
            &BhtuneConfig::default(),
            Some("/xdg-data"),
            Some("/home/me"),
            None,
            false,
        );
        assert_eq!(resolved, PathBuf::from("/xdg-data/bhtune/bhtune.db"));
    }

    #[test]
    fn resolve_bridge_host_cli_wins() {
        let config = BhtuneConfig {
            bridge_host: Some("configured:1".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bridge_host(Some("cli:2".to_string()), &config),
            "cli:2".to_string()
        );
    }

    #[test]
    fn resolve_bridge_host_config_wins_over_default() {
        let config = BhtuneConfig {
            bridge_host: Some("configured:1".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bridge_host(None, &config),
            "configured:1".to_string()
        );
    }

    #[test]
    fn resolve_bridge_host_default() {
        assert_eq!(
            resolve_bridge_host(None, &BhtuneConfig::default()),
            DEFAULT_BRIDGE_HOST.to_string()
        );
    }

    #[test]
    fn resolve_bind_addr_cli_wins() {
        let config = BhtuneConfig {
            bind: Some("0.0.0.0:9999".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_bind_addr(Some("127.0.0.1:1234".to_string()), &config),
            "127.0.0.1:1234".to_string()
        );
    }

    #[test]
    fn resolve_bind_addr_config_wins_over_default() {
        let config = BhtuneConfig {
            bind: Some("0.0.0.0:9999".into()),
            ..Default::default()
        };
        assert_eq!(resolve_bind_addr(None, &config), "0.0.0.0:9999".to_string());
    }

    #[test]
    fn resolve_bind_addr_default() {
        assert_eq!(
            resolve_bind_addr(None, &BhtuneConfig::default()),
            DEFAULT_BIND_ADDR.to_string()
        );
    }

    #[test]
    fn resolve_retention_days_cli_wins() {
        let config = BhtuneConfig {
            retention_days: Some(90),
            ..Default::default()
        };
        assert_eq!(resolve_retention_days(Some(30), &config), Some(30));
    }

    #[test]
    fn resolve_retention_days_config_wins_over_default() {
        let config = BhtuneConfig {
            retention_days: Some(90),
            ..Default::default()
        };
        assert_eq!(resolve_retention_days(None, &config), Some(90));
    }

    #[test]
    fn resolve_retention_days_default_is_retain_forever() {
        // No CLI flag, env var, or config key at all -- the deliberate "ships disabled by
        // default" behavior `history-retention`'s design note calls for, not merely the
        // absence of a hardcoded number.
        assert_eq!(resolve_retention_days(None, &BhtuneConfig::default()), None);
    }

    #[test]
    fn resolve_server_cli_wins() {
        let config = BhtuneConfig {
            server: Some("ConfigServer".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_server(Some("CliServer".to_string()), &config).unwrap(),
            "CliServer"
        );
    }

    #[test]
    fn resolve_server_config_fallback() {
        let config = BhtuneConfig {
            server: Some("ConfigServer".into()),
            ..Default::default()
        };
        assert_eq!(resolve_server(None, &config).unwrap(), "ConfigServer");
    }

    #[test]
    fn resolve_server_neither_set_errors() {
        let err = resolve_server(None, &BhtuneConfig::default()).unwrap_err();
        assert!(err.to_string().contains("no OPC server specified"));
    }
}
