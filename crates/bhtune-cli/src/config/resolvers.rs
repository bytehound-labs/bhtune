#![allow(rustdoc::broken_intra_doc_links)]

use std::path::PathBuf;

use super::model::{BhtuneConfig, DEFAULT_BIND_ADDR, DEFAULT_BRIDGE_HOST};
use super::paths::default_db_path_from;

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
