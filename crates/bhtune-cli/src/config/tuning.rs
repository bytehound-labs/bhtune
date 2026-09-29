#![allow(rustdoc::broken_intra_doc_links)]

use serde::{Deserialize, Serialize};

/// Built-in pre/post-MRFT recording padding when `[tuning].mrft_delay_secs` is absent.
pub const DEFAULT_TUNING_MRFT_DELAY_SECS: u32 = 0;

/// Built-in driver polling interval when `[tuning].poll_interval_ms` is absent.
pub const DEFAULT_TUNING_POLL_INTERVAL_MS: u64 = 800;

/// Built-in whole-run timeout when `[tuning].timeout_secs` is absent.
pub const DEFAULT_TUNING_TIMEOUT_SECS: u64 = 3_600;

/// Built-in per-driver-operation timeout when `[tuning].op_timeout_secs` is absent.
pub const DEFAULT_TUNING_OP_TIMEOUT_SECS: u64 = 30;

/// Built-in post-run restoration timeout when `[tuning].restore_timeout_secs` is absent.
pub const DEFAULT_TUNING_RESTORE_TIMEOUT_SECS: u64 = 30;

/// Largest supported pre/post-MRFT recording delay.
pub const MAX_TUNING_MRFT_DELAY_SECS: u32 = 3_600;

/// Minimum restoration timeout for OPC DA runs.
pub const MIN_OPC_RESTORE_TIMEOUT_SECS: u64 = 4;

/// Optional values authored in the `[tuning]` table.
///
/// Missing keys stay `None` so callers can distinguish an explicit TOML value from a
/// built-in default. Use [`resolve_tuning_config`] to obtain the concrete values used by a
/// tune and [`validate_tuning_config`] before preparing the run.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct TuningConfig {
    #[cfg_attr(feature = "schemars", schemars(range(max = 3_600)))]
    pub mrft_delay_secs: Option<u32>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub poll_interval_ms: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub timeout_secs: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub op_timeout_secs: Option<u64>,
    #[cfg_attr(feature = "schemars", schemars(range(min = 1)))]
    pub restore_timeout_secs: Option<u64>,
}

/// Concrete tuning timing policy after absent TOML keys have received built-in defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveTuningConfig {
    pub mrft_delay_secs: u32,
    pub poll_interval_ms: u64,
    pub timeout_secs: u64,
    pub op_timeout_secs: u64,
    pub restore_timeout_secs: u64,
}

impl Default for EffectiveTuningConfig {
    fn default() -> Self {
        Self {
            mrft_delay_secs: DEFAULT_TUNING_MRFT_DELAY_SECS,
            poll_interval_ms: DEFAULT_TUNING_POLL_INTERVAL_MS,
            timeout_secs: DEFAULT_TUNING_TIMEOUT_SECS,
            op_timeout_secs: DEFAULT_TUNING_OP_TIMEOUT_SECS,
            restore_timeout_secs: DEFAULT_TUNING_RESTORE_TIMEOUT_SECS,
        }
    }
}

/// Origin of one effective tuning value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningConfigSource {
    Toml,
    BuiltInDefault,
}

/// Per-field provenance for the effective `[tuning]` policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuningConfigSources {
    pub mrft_delay_secs: TuningConfigSource,
    pub poll_interval_ms: TuningConfigSource,
    pub timeout_secs: TuningConfigSource,
    pub op_timeout_secs: TuningConfigSource,
    pub restore_timeout_secs: TuningConfigSource,
}

/// Validation error for concrete tuning timing values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuningConfigError {
    MrftDelayOutOfRange { value: u32 },
    PollIntervalTooSmall { value: u64 },
    TimeoutTooSmall { value: u64 },
    OpTimeoutTooSmall { value: u64 },
    RestoreTimeoutTooSmall { value: u64, minimum: u64 },
}

impl std::fmt::Display for TuningConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MrftDelayOutOfRange { value } => write!(
                f,
                "tuning.mrft_delay_secs must be between 0 and {MAX_TUNING_MRFT_DELAY_SECS}, got {value}"
            ),
            Self::PollIntervalTooSmall { value } => {
                write!(f, "tuning.poll_interval_ms must be at least 1, got {value}")
            }
            Self::TimeoutTooSmall { value } => {
                write!(f, "tuning.timeout_secs must be at least 1, got {value}")
            }
            Self::OpTimeoutTooSmall { value } => {
                write!(f, "tuning.op_timeout_secs must be at least 1, got {value}")
            }
            Self::RestoreTimeoutTooSmall { value, minimum } => write!(
                f,
                "tuning.restore_timeout_secs must be at least {minimum}, got {value}"
            ),
        }
    }
}

impl std::error::Error for TuningConfigError {}

/// Resolve optional `[tuning]` values against the built-in defaults.
pub fn resolve_tuning_config(config: &TuningConfig) -> EffectiveTuningConfig {
    EffectiveTuningConfig {
        mrft_delay_secs: config
            .mrft_delay_secs
            .unwrap_or(DEFAULT_TUNING_MRFT_DELAY_SECS),
        poll_interval_ms: config
            .poll_interval_ms
            .unwrap_or(DEFAULT_TUNING_POLL_INTERVAL_MS),
        timeout_secs: config.timeout_secs.unwrap_or(DEFAULT_TUNING_TIMEOUT_SECS),
        op_timeout_secs: config
            .op_timeout_secs
            .unwrap_or(DEFAULT_TUNING_OP_TIMEOUT_SECS),
        restore_timeout_secs: config
            .restore_timeout_secs
            .unwrap_or(DEFAULT_TUNING_RESTORE_TIMEOUT_SECS),
    }
}

/// Report whether each effective tuning value came from TOML or a built-in default.
pub fn tuning_config_sources(config: &TuningConfig) -> TuningConfigSources {
    fn source<T>(value: Option<T>) -> TuningConfigSource {
        if value.is_some() {
            TuningConfigSource::Toml
        } else {
            TuningConfigSource::BuiltInDefault
        }
    }

    TuningConfigSources {
        mrft_delay_secs: source(config.mrft_delay_secs),
        poll_interval_ms: source(config.poll_interval_ms),
        timeout_secs: source(config.timeout_secs),
        op_timeout_secs: source(config.op_timeout_secs),
        restore_timeout_secs: source(config.restore_timeout_secs),
    }
}

/// Validate concrete tuning timing values.
///
/// `require_opc_restore_minimum` raises the restoration minimum from one second to
/// [`MIN_OPC_RESTORE_TIMEOUT_SECS`], matching the live OPC DA actuation-confirmation window.
pub fn validate_tuning_config(
    config: &EffectiveTuningConfig,
    require_opc_restore_minimum: bool,
) -> Result<(), TuningConfigError> {
    if config.mrft_delay_secs > MAX_TUNING_MRFT_DELAY_SECS {
        return Err(TuningConfigError::MrftDelayOutOfRange {
            value: config.mrft_delay_secs,
        });
    }
    if config.poll_interval_ms == 0 {
        return Err(TuningConfigError::PollIntervalTooSmall {
            value: config.poll_interval_ms,
        });
    }
    if config.timeout_secs == 0 {
        return Err(TuningConfigError::TimeoutTooSmall {
            value: config.timeout_secs,
        });
    }
    if config.op_timeout_secs == 0 {
        return Err(TuningConfigError::OpTimeoutTooSmall {
            value: config.op_timeout_secs,
        });
    }
    let restore_minimum = if require_opc_restore_minimum {
        MIN_OPC_RESTORE_TIMEOUT_SECS
    } else {
        1
    };
    if config.restore_timeout_secs < restore_minimum {
        return Err(TuningConfigError::RestoreTimeoutTooSmall {
            value: config.restore_timeout_secs,
            minimum: restore_minimum,
        });
    }
    Ok(())
}

/// Resolve and validate a raw `[tuning]` table in one step.
pub fn resolve_and_validate_tuning_config(
    config: &TuningConfig,
    require_opc_restore_minimum: bool,
) -> Result<EffectiveTuningConfig, TuningConfigError> {
    let effective = resolve_tuning_config(config);
    validate_tuning_config(&effective, require_opc_restore_minimum)?;
    Ok(effective)
}

#[cfg(test)]
mod tests {
    use super::super::model::parse_config_contents;
    use super::*;
    #[test]
    fn missing_tuning_table_preserves_none_and_resolves_built_in_defaults() {
        let config = parse_config_contents("bridge_host = \"gateway:7600\"\n").unwrap();

        assert_eq!(config.tuning, TuningConfig::default());
        assert_eq!(
            resolve_and_validate_tuning_config(&config.tuning, true).unwrap(),
            EffectiveTuningConfig::default()
        );
    }

    #[test]
    fn tuning_table_round_trips_all_optional_values() {
        let raw = r"
[tuning]
mrft_delay_secs = 12
poll_interval_ms = 900
timeout_secs = 4000
op_timeout_secs = 31
restore_timeout_secs = 32
";
        let config = parse_config_contents(raw).unwrap();

        assert_eq!(
            config.tuning,
            TuningConfig {
                mrft_delay_secs: Some(12),
                poll_interval_ms: Some(900),
                timeout_secs: Some(4_000),
                op_timeout_secs: Some(31),
                restore_timeout_secs: Some(32),
            }
        );
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(parse_config_contents(&encoded).unwrap(), config);
    }

    #[test]
    fn tuning_resolution_preserves_partial_raw_values_and_tracks_sources() {
        let raw = TuningConfig {
            poll_interval_ms: Some(250),
            restore_timeout_secs: Some(8),
            ..Default::default()
        };

        assert_eq!(
            resolve_tuning_config(&raw),
            EffectiveTuningConfig {
                poll_interval_ms: 250,
                restore_timeout_secs: 8,
                ..Default::default()
            }
        );
        assert_eq!(
            tuning_config_sources(&raw),
            TuningConfigSources {
                mrft_delay_secs: TuningConfigSource::BuiltInDefault,
                poll_interval_ms: TuningConfigSource::Toml,
                timeout_secs: TuningConfigSource::BuiltInDefault,
                op_timeout_secs: TuningConfigSource::BuiltInDefault,
                restore_timeout_secs: TuningConfigSource::Toml,
            }
        );
    }

    #[test]
    fn tuning_validation_rejects_each_invalid_general_value() {
        let cases = [
            (
                EffectiveTuningConfig {
                    mrft_delay_secs: MAX_TUNING_MRFT_DELAY_SECS + 1,
                    ..Default::default()
                },
                TuningConfigError::MrftDelayOutOfRange {
                    value: MAX_TUNING_MRFT_DELAY_SECS + 1,
                },
            ),
            (
                EffectiveTuningConfig {
                    poll_interval_ms: 0,
                    ..Default::default()
                },
                TuningConfigError::PollIntervalTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::TimeoutTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    op_timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::OpTimeoutTooSmall { value: 0 },
            ),
            (
                EffectiveTuningConfig {
                    restore_timeout_secs: 0,
                    ..Default::default()
                },
                TuningConfigError::RestoreTimeoutTooSmall {
                    value: 0,
                    minimum: 1,
                },
            ),
        ];

        for (config, expected) in cases {
            assert_eq!(validate_tuning_config(&config, false), Err(expected));
            assert!(!expected.to_string().is_empty());
        }
    }

    #[test]
    fn opc_restore_timeout_requires_four_seconds_but_general_validation_allows_one() {
        let config = EffectiveTuningConfig {
            restore_timeout_secs: MIN_OPC_RESTORE_TIMEOUT_SECS - 1,
            ..Default::default()
        };

        assert_eq!(validate_tuning_config(&config, false), Ok(()));
        assert_eq!(
            validate_tuning_config(&config, true),
            Err(TuningConfigError::RestoreTimeoutTooSmall {
                value: MIN_OPC_RESTORE_TIMEOUT_SECS - 1,
                minimum: MIN_OPC_RESTORE_TIMEOUT_SECS,
            })
        );
        assert!(
            validate_tuning_config(
                &EffectiveTuningConfig {
                    restore_timeout_secs: MIN_OPC_RESTORE_TIMEOUT_SECS,
                    ..Default::default()
                },
                true,
            )
            .is_ok()
        );
    }
}
