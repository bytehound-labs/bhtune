#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::HashMap;

use bhtune_core::{ControllerDirection, DcsTemplate, TagOrValue};
use bhtune_db::models::SampleQuality;
use bhtune_driver::{Driver, TagValue, TagWrite};

/// Enforces the live-run quality policy: `Quality::Bad` is never accepted;
/// `Quality::Uncertain` is accepted only when the global Config > OPC quality policy
/// (`allow_uncertain_quality` in TOML) permits it, and each use of it is logged so a run
/// executed under relaxed rules is not silently indistinguishable from a normal one.
/// `Quality::Good` always passes.
pub(super) fn check_quality(
    tag: &str,
    quality: bhtune_driver::Quality,
    allow_uncertain: bool,
) -> anyhow::Result<()> {
    match quality {
        bhtune_driver::Quality::Good => Ok(()),
        bhtune_driver::Quality::Uncertain if allow_uncertain => {
            tracing::warn!(
                tag,
                "accepting Uncertain-quality reading because Config > OPC quality policy \
                 (allow_uncertain_quality) permits it"
            );
            Ok(())
        }
        bhtune_driver::Quality::Uncertain => {
            anyhow::bail!(
                "tag '{tag}' reported OPC quality Uncertain; refusing to trust it for a \
                 tuning-critical reading (set Config > OPC quality policy \
                 `allow_uncertain_quality = true` to accept Uncertain readings; Bad is never \
                 accepted)"
            )
        }
        bhtune_driver::Quality::Bad => {
            anyhow::bail!(
                "tag '{tag}' reported OPC quality Bad; refusing to trust it for a \
                 tuning-critical reading"
            )
        }
    }
}
/// Maps the driver's live [`bhtune_driver::Quality`] to the database's persisted
/// [`SampleQuality`] -- two separate enums (rather than one shared type) because
/// `bhtune-driver` and `bhtune-db` are sibling crates that each depend only on
/// `bhtune-core`, not on each other; only a crate depending on both needs this mapping, so
/// it lives here rather than forcing a new cross-dependency onto either crate. `pub` (not
/// just used by [`run_polling_loop`] below) because `bhtune-server`'s `routes::opc` reuses
/// it verbatim for `GET /api/opc/read`'s quality field, rather than a second copy of the
/// same three-arm match.
pub fn sample_quality_from_driver(quality: bhtune_driver::Quality) -> SampleQuality {
    match quality {
        bhtune_driver::Quality::Good => SampleQuality::Good,
        bhtune_driver::Quality::Uncertain => SampleQuality::Uncertain,
        bhtune_driver::Quality::Bad => SampleQuality::Bad,
    }
}
pub(super) async fn read_raw(
    driver: &dyn Driver,
    tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<String> {
    let values = driver.read(&[tag.to_string()]).await?;
    let value = values
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    check_quality(tag, value.quality, allow_uncertain)?;
    Ok(value.value)
}
pub(super) async fn read_f32(
    driver: &dyn Driver,
    tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<f32> {
    let raw = read_raw(driver, tag, allow_uncertain).await?;
    parse_f32_value(tag, &raw)
}
pub(super) async fn resolve_f32(
    driver: &dyn Driver,
    tag_or_value: &TagOrValue<f32>,
    allow_uncertain: bool,
) -> anyhow::Result<f32> {
    match tag_or_value {
        TagOrValue::Value(v) => {
            if !v.is_finite() {
                anyhow::bail!("value {v} is not a finite number");
            }
            Ok(*v)
        }
        TagOrValue::Tag(tag) => read_f32(driver, tag, allow_uncertain).await,
    }
}
pub(super) async fn resolve_direction(
    driver: &dyn Driver,
    tag_or_value: &TagOrValue<ControllerDirection>,
    template: &DcsTemplate,
    allow_uncertain: bool,
) -> anyhow::Result<ControllerDirection> {
    match tag_or_value {
        TagOrValue::Value(d) => Ok(*d),
        TagOrValue::Tag(tag) => {
            let raw = read_raw(driver, tag, allow_uncertain).await?;
            Ok(ControllerDirection::from_raw_tag_value(
                &raw,
                &template.controller_action_direct_value,
            ))
        }
    }
}
pub(super) fn parse_f32_value(tag: &str, raw: &str) -> anyhow::Result<f32> {
    let value: f32 = raw
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{raw}' is not a number"))?;
    if !value.is_finite() {
        anyhow::bail!("tag '{tag}' value '{raw}' is not a finite number");
    }
    Ok(value)
}
pub(super) fn read_batch_raw(
    values: &HashMap<String, TagValue>,
    tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<String> {
    let value = values
        .get(tag)
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    check_quality(tag, value.quality, allow_uncertain)?;
    Ok(value.value.clone())
}
pub(super) fn read_batch_f32(
    values: &HashMap<String, TagValue>,
    tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<f32> {
    let raw = read_batch_raw(values, tag, allow_uncertain)?;
    parse_f32_value(tag, &raw)
}
pub(super) async fn resolve_f32_from_batch(
    driver: &dyn Driver,
    values: &HashMap<String, TagValue>,
    tag_or_value: &TagOrValue<f32>,
    allow_uncertain: bool,
) -> anyhow::Result<f32> {
    match tag_or_value {
        TagOrValue::Value(_) => resolve_f32(driver, tag_or_value, allow_uncertain).await,
        TagOrValue::Tag(tag) => read_batch_f32(values, tag, allow_uncertain),
    }
}
pub(super) async fn resolve_direction_from_batch(
    driver: &dyn Driver,
    values: &HashMap<String, TagValue>,
    tag_or_value: &TagOrValue<ControllerDirection>,
    template: &DcsTemplate,
    allow_uncertain: bool,
) -> anyhow::Result<ControllerDirection> {
    match tag_or_value {
        TagOrValue::Value(_) => {
            resolve_direction(driver, tag_or_value, template, allow_uncertain).await
        }
        TagOrValue::Tag(tag) => {
            let raw = read_batch_raw(values, tag, allow_uncertain)?;
            Ok(ControllerDirection::from_raw_tag_value(
                &raw,
                &template.controller_action_direct_value,
            ))
        }
    }
}
/// Test-only single-tag PV reader that preserves raw quality alongside the value. The production
/// polling path uses [`read_poll_batch`] so pending OPC relay checks can share one read with PV
/// sampling. This helper remains for focused tests of the single-tag parsing behavior and still
/// hard-fails on non-numeric/non-finite values regardless of quality, exactly like [`read_f32`],
/// since that's a data-shape problem no quality policy can excuse.
#[cfg(test)]
pub(super) async fn read_pv_sample(
    driver: &dyn Driver,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    read_numeric_sample(driver, tag).await
}
pub(super) async fn read_numeric_sample(
    driver: &dyn Driver,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    let values = driver.read(&[tag.to_string()]).await?;
    let value = values
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    let numeric: f32 = value
        .value
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{}' is not a number", value.value))?;
    if !numeric.is_finite() {
        anyhow::bail!("tag '{tag}' value '{}' is not a finite number", value.value);
    }
    Ok((numeric, value.quality))
}
pub(super) async fn read_poll_batch(
    driver: &dyn Driver,
    pv_tag: &str,
    mv_tag: Option<&str>,
) -> anyhow::Result<HashMap<String, TagValue>> {
    let mut requested_tags = vec![pv_tag.to_string()];
    if let Some(mv_tag) = mv_tag
        && mv_tag != pv_tag
    {
        requested_tags.push(mv_tag.to_string());
    }

    Ok(driver
        .read(&requested_tags)
        .await?
        .into_iter()
        .map(|value| (value.tag.clone(), value))
        .collect())
}
pub(super) fn read_numeric_from_batch(
    values: &HashMap<String, TagValue>,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    let value = values
        .get(tag)
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    let numeric = parse_f32_value(tag, &value.value)?;
    Ok((numeric, value.quality))
}
pub(super) async fn write_raw(driver: &dyn Driver, tag: &str, value: String) -> anyhow::Result<()> {
    let outcome = driver.write(&tag.to_string(), TagWrite::Raw(value)).await?;
    if outcome.success {
        Ok(())
    } else {
        anyhow::bail!(
            "write to '{tag}' was rejected: {}",
            outcome
                .error_message
                .unwrap_or_else(|| "unknown reason".to_string())
        )
    }
}
pub(super) async fn write_value(driver: &dyn Driver, tag: &str, value: f32) -> anyhow::Result<()> {
    let outcome = driver
        .write(&tag.to_string(), TagWrite::Float(value))
        .await?;
    if outcome.success {
        Ok(())
    } else {
        anyhow::bail!(
            "write to '{tag}' was rejected: {}",
            outcome
                .error_message
                .unwrap_or_else(|| "unknown reason".to_string())
        )
    }
}
