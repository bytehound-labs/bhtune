#![allow(rustdoc::broken_intra_doc_links)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::demo::{DemoPolicyConfig, ServerMode};
use super::log::LogConfig;
use super::tuning::TuningConfig;

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
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct BhtuneConfig {
    /// Runtime server mode. The environment variable `BHTUNE_SERVER_MODE` overrides this.
    #[serde(default)]
    pub server_mode: Option<ServerMode>,
    /// Safety limits for the simulator-only public demo mode.
    #[serde(default)]
    pub demo: DemoPolicyConfig,
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
    /// Optional exact browser origin for state-changing HTTP requests. `BHTUNE_ORIGIN`
    /// overrides this value. Full mode automatically matches the browser origin to the
    /// request host when this is unset; set it for a reverse proxy that rewrites `Host` or
    /// to pin one public origin. Demo mode always requires an exact configured origin and
    /// requires HTTPS, except for explicit loopback HTTP origins used by local tests and
    /// development.
    #[serde(default)]
    pub origin: Option<String>,
    /// IP address or matching-family CIDR of a reverse proxy trusted to supply the
    /// single-address `X-BHTune-Client-IP` header for Demo quota accounting.
    #[serde(default)]
    pub trusted_proxy: Option<String>,
    /// Age-based history retention (`history-retention`): tune runs with `started_at` older
    /// than this many days are deleted automatically on every startup (both binaries, via
    /// `crate::db::open`) and, for `bhtune-server`, again on a periodic timer while it keeps
    /// running -- see `crate::retention`. A present value must be at least 1. `None` (the
    /// default) means retain forever: there is no built-in number of days, since at this
    /// project's data volumes (see AGENTS.md's History explorer notes) an unexpected
    /// auto-delete of someone's baseline tune is a worse failure mode than an ever-growing
    /// database file. See [`resolve_retention_days`].
    #[serde(default, deserialize_with = "deserialize_retention_days")]
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub retention_days: Option<u32>,
    /// Default OPC sample-quality policy for the server config page: `true` accepts
    /// `Uncertain` quality, while `false` rejects it. A missing key is treated as `true`
    /// when the config file is parsed, matching the configuration-page default rather than
    /// `bool`'s ordinary `false`.
    #[serde(default = "default_allow_uncertain_quality")]
    pub allow_uncertain_quality: bool,
    /// Global tune timing defaults. Missing keys remain `None` and resolve through
    /// [`resolve_tuning_config`] only when a tune is prepared.
    #[serde(default)]
    pub tuning: TuningConfig,
    /// `[log]` sub-table: level/directory/format/rotation for `crate::logging`'s tracing
    /// setup, mirroring `opcda-bridge-gateway`'s own `log.*` config conventions.
    #[serde(default)]
    pub log: LogConfig,
}

/// The default configuration-page quality policy: absent `allow_uncertain_quality` resolves
/// to `true` rather than `bool`'s usual `false`.
pub const fn default_allow_uncertain_quality() -> bool {
    true
}

fn deserialize_retention_days<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let days = Option::<u32>::deserialize(deserializer)?;
    match days {
        Some(0) => Err(serde::de::Error::custom(
            "retention_days must be at least 1 or omitted",
        )),
        other => Ok(other),
    }
}

impl Default for BhtuneConfig {
    fn default() -> Self {
        Self {
            server_mode: None,
            demo: DemoPolicyConfig::default(),
            db: None,
            bridge_host: None,
            server: None,
            templates: None,
            bind: None,
            origin: None,
            trusted_proxy: None,
            retention_days: None,
            allow_uncertain_quality: default_allow_uncertain_quality(),
            tuning: TuningConfig::default(),
            log: LogConfig::default(),
        }
    }
}

/// Parses the in-memory TOML representation of a config file.
///
/// Keeping the parser separate from filesystem discovery gives property tests and fuzz
/// targets a narrow, side-effect-free boundary to exercise.
pub fn parse_config_contents(contents: &str) -> anyhow::Result<BhtuneConfig> {
    toml::from_str(contents).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::super::browser::load_config_file;
    use super::super::tuning::MAX_TUNING_MRFT_DELAY_SECS;
    use super::*;
    use proptest::prelude::*;
    use std::io::Write;
    proptest::proptest! {
        #[test]
        fn serialized_configs_round_trip(
            db in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bridge_host in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            server in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            templates in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            bind in prop::option::of("[A-Za-z0-9_.:-]{0,32}"),
            retention_days in prop::option::of(1u32..),
            allow_uncertain_quality in any::<bool>(),
            mrft_delay_secs in prop::option::of(0u32..=MAX_TUNING_MRFT_DELAY_SECS),
            poll_interval_ms in prop::option::of(1u64..=1_000_000),
            timeout_secs in prop::option::of(1u64..=1_000_000),
            op_timeout_secs in prop::option::of(1u64..=1_000_000),
            restore_timeout_secs in prop::option::of(1u64..=1_000_000),
            level in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            dir in prop::option::of("[A-Za-z0-9_./:-]{0,32}"),
            format in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
            rotation in prop::option::of("[A-Za-z0-9_.:-]{0,16}"),
        ) {
            let config = BhtuneConfig {
                server_mode: None,
                demo: DemoPolicyConfig::default(),
                db: db.map(PathBuf::from),
                bridge_host,
                server,
                templates: templates.map(PathBuf::from),
                bind,
                origin: None,
                trusted_proxy: None,
                retention_days,
                allow_uncertain_quality,
                tuning: TuningConfig {
                    mrft_delay_secs,
                    poll_interval_ms,
                    timeout_secs,
                    op_timeout_secs,
                    restore_timeout_secs,
                },
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

    #[test]
    fn parse_config_rejects_zero_retention_days() {
        let error = parse_config_contents("retention_days = 0").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("retention_days must be at least 1")
        );
    }

    #[test]
    fn default_allow_uncertain_quality_is_true() {
        assert!(BhtuneConfig::default().allow_uncertain_quality);
    }

    #[test]
    fn load_config_file_missing_allow_uncertain_quality_key_defaults_to_true() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "bridge_host = \"gateway:7600\"").unwrap();
        let config = load_config_file(file.path(), true).unwrap();
        assert!(config.allow_uncertain_quality);
        assert_eq!(config.bridge_host, Some("gateway:7600".to_string()));
    }
}
