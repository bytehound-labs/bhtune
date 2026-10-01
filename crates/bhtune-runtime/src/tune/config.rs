#![allow(rustdoc::broken_intra_doc_links)]

use bhtune_core::{ControllerType, DcsTemplate, LoopConfig, LoopTags, ProcessType, TagOrValue};
use bhtune_db::models::EffectiveTuning;

use crate::driver::{SIMULATOR_MV_TAG, SIMULATOR_PV_TAG};
use crate::tune::{DriverKind, TuneRequest};

/// Concrete timing policy frozen during [`prepare`] and carried unchanged through the
/// complete tune lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EffectiveTiming {
    pub(super) mrft_delay_secs: u32,
    pub(super) poll_interval_ms: u64,
    pub(super) timeout_secs: u64,
    pub(super) op_timeout_secs: u64,
    pub(super) restore_timeout_secs: u64,
}
impl From<crate::config::EffectiveTuningConfig> for EffectiveTiming {
    fn from(value: crate::config::EffectiveTuningConfig) -> Self {
        Self {
            mrft_delay_secs: value.mrft_delay_secs,
            poll_interval_ms: value.poll_interval_ms,
            timeout_secs: value.timeout_secs,
            op_timeout_secs: value.op_timeout_secs,
            restore_timeout_secs: value.restore_timeout_secs,
        }
    }
}
impl From<EffectiveTiming> for EffectiveTuning {
    fn from(value: EffectiveTiming) -> Self {
        Self {
            mrft_delay_secs: value.mrft_delay_secs,
            poll_interval_ms: value.poll_interval_ms,
            timeout_secs: value.timeout_secs,
            op_timeout_secs: value.op_timeout_secs,
            restore_timeout_secs: value.restore_timeout_secs,
        }
    }
}
#[cfg(test)]
pub(super) fn test_effective_timing(args: &TuneRequest) -> EffectiveTiming {
    EffectiveTiming {
        mrft_delay_secs: args.mrft_delay,
        poll_interval_ms: args.poll_interval_ms,
        timeout_secs: args.timeout_secs,
        op_timeout_secs: args.op_timeout_secs,
        restore_timeout_secs: args.restore_timeout_secs,
    }
}
/// Validates the restore budget shared by CLI and HTTP-started tunes.
///
/// Every driver requires a positive timeout. OPC DA additionally needs the complete fixed MV
/// confirmation window so the authoritative restore can be read back before the operation is
/// declared successful.
pub fn validate_restore_timeout_secs(
    driver: DriverKind,
    restore_timeout_secs: u64,
) -> anyhow::Result<()> {
    if restore_timeout_secs == 0 {
        anyhow::bail!("[tuning].restore_timeout_secs must be greater than zero");
    }
    if driver == DriverKind::Opcda
        && restore_timeout_secs < crate::config::MIN_OPC_RESTORE_TIMEOUT_SECS
    {
        anyhow::bail!(
            "[tuning].restore_timeout_secs must be at least {} seconds for OPC DA MV confirmation",
            crate::config::MIN_OPC_RESTORE_TIMEOUT_SECS
        );
    }
    Ok(())
}
pub(super) fn build_loop_config_with_timing(
    args: &TuneRequest,
    timing: EffectiveTiming,
) -> anyhow::Result<LoopConfig> {
    let process_type: ProcessType = args.process_type;
    let controller_type: ControllerType = args.controller_type;

    if !controller_type.is_allowed_for(process_type) {
        anyhow::bail!(
            "{controller_type:?} controller is not valid for {process_type:?} (PID is only offered for the two Temperature process types)"
        );
    }

    let config = LoopConfig {
        process_type,
        controller_type,
        relay_amp_percent: args.relay_amp,
        num_cycles_skip: args
            .cycles_skip
            .unwrap_or_else(|| process_type.default_cycles_skip()),
        num_cycles_count: args
            .cycles_count
            .unwrap_or_else(|| process_type.default_cycles_test()),
        noise_protection_secs: args
            .noise_protection_secs
            .unwrap_or_else(|| process_type.default_noise_protection_secs()),
        mrft_delay_secs: timing.mrft_delay_secs,
    };
    // Real range validation at the model level (see `LoopConfig::validate`), not just this
    // flag parse -- catches an out-of-range `--relay-amp` (including the legacy predecessor's
    // "not blank" bug of a stray debug shortcut reaching this field) before any driver
    // connection or database write.
    config.validate()?;
    Ok(config)
}
#[cfg(test)]
pub(super) fn build_loop_config(args: &TuneRequest) -> anyhow::Result<LoopConfig> {
    build_loop_config_with_timing(args, test_effective_timing(args))
}
/// Builds the loop's full tag set. For `--driver opcda`, derives from `--tagname` and the
/// template, then layers any explicit `--pv-range-*`/`--mv-range-*`/`--direction` overrides
/// on top. For `--driver simulator`, `SimulatorDriver`'s fixed two-tag contract means the
/// range/direction overrides are mandatory (normally supplied by the `bhtune simulate`
/// adapter); a direct `bhtune tune --driver simulator` invocation
/// missing any of them is a clear usage error rather than a confusing runtime failure.
pub(super) fn build_loop_tags(
    args: &TuneRequest,
    template: &DcsTemplate,
) -> anyhow::Result<LoopTags> {
    match args.driver {
        DriverKind::Opcda => {
            let mut tags = LoopTags::derive_from_pv_tag(&args.tagname, template);
            if let Some(overrides) = &args.tag_overrides {
                overrides.apply_to(&mut tags);
            }
            // Fixed values are applied after custom read-tag overrides, so an explicit fixed
            // request remains authoritative when input contains both.
            if let Some(v) = args.pv_range_high {
                tags.upper_pv_range = TagOrValue::Value(v);
            }
            if let Some(v) = args.pv_range_low {
                tags.lower_pv_range = TagOrValue::Value(v);
            }
            if let Some(v) = args.mv_range_high {
                tags.upper_mv_range = TagOrValue::Value(v);
            }
            if let Some(v) = args.mv_range_low {
                tags.lower_mv_range = TagOrValue::Value(v);
            }
            if let Some(d) = args.direction {
                tags.controller_direction = TagOrValue::Value(d);
            }
            Ok(tags)
        }
        DriverKind::Simulator => {
            let pv_range_high = args.pv_range_high.ok_or_else(|| {
                anyhow::anyhow!(
                    "--pv-range-high is required with --driver simulator (or use `bhtune simulate`)"
                )
            })?;
            let pv_range_low = args.pv_range_low.ok_or_else(|| {
                anyhow::anyhow!(
                    "--pv-range-low is required with --driver simulator (or use `bhtune simulate`)"
                )
            })?;
            let mv_range_high = args.mv_range_high.ok_or_else(|| {
                anyhow::anyhow!(
                    "--mv-range-high is required with --driver simulator (or use `bhtune simulate`)"
                )
            })?;
            let mv_range_low = args.mv_range_low.ok_or_else(|| {
                anyhow::anyhow!(
                    "--mv-range-low is required with --driver simulator (or use `bhtune simulate`)"
                )
            })?;
            let direction = args.direction.ok_or_else(|| {
                anyhow::anyhow!(
                    "--direction is required with --driver simulator (or use `bhtune simulate`)"
                )
            })?;

            Ok(LoopTags {
                process_variable: SIMULATOR_PV_TAG.to_string(),
                manipulated_variable: SIMULATOR_MV_TAG.to_string(),
                setpoint_variable: None,
                controller_mode: None,
                mode_attribute: None,
                upper_pv_range: TagOrValue::Value(pv_range_high),
                lower_pv_range: TagOrValue::Value(pv_range_low),
                upper_mv_range: TagOrValue::Value(mv_range_high),
                lower_mv_range: TagOrValue::Value(mv_range_low),
                controller_direction: TagOrValue::Value(direction),
                proportional_constant: None,
                integral_constant: None,
                derivative_constant: None,
            })
        }
    }
}
