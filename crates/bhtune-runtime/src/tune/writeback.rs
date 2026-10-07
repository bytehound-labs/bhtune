#![allow(rustdoc::broken_intra_doc_links)]

use bhtune_core::{
    ControllerPidValues, ControllerType, DcsTemplate, LoopConfig, LoopTags, PidParameters,
    ResponseLevel, TuningResultStatus, controller_pid_values,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    LiveOwnershipRow, NewTuneWrite, RollbackState, TuneResultRow, TuneRunRow, TuneWriteRow,
    WriteKind, WriteReadback,
};
use bhtune_driver::Driver;
use chrono::Utc;

use crate::live_ownership::{AuditedDriver, LiveOperationGuard};

use super::outcome::WriteBackOutcome;
use super::quality::{read_f32, write_value};

/// Reads the existing Proportional/Integral/Derivative values before any write is attempted.
/// Reading all three is a hard stop on the first failure and guarantees that a later rollback
/// always has a known-good value to restore. `pub(crate)`: also reused by
/// `commands::history::revert`, which needs the identical pre-read step before writing a
/// past run's recorded values back.
pub(crate) async fn read_previous_pid_values(
    driver: &dyn Driver,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<WriteReadback> {
    let proportional = read_f32(driver, p_tag, allow_uncertain)
        .await
        .map_err(|e| anyhow::anyhow!("pre-read of Proportional tag '{p_tag}' failed: {e}"))?;
    let integral = read_f32(driver, i_tag, allow_uncertain)
        .await
        .map_err(|e| anyhow::anyhow!("pre-read of Integral tag '{i_tag}' failed: {e}"))?;
    let derivative = read_f32(driver, d_tag, allow_uncertain)
        .await
        .map_err(|e| anyhow::anyhow!("pre-read of Derivative tag '{d_tag}' failed: {e}"))?;
    Ok(WriteReadback {
        proportional,
        integral,
        derivative,
    })
}

async fn persist_pid_restore_intent(
    pool: &SqlitePool,
    ownership: &LiveOperationGuard,
    run_id: i64,
    kind: WriteKind,
    response_level: ResponseLevel,
    previous: WriteReadback,
    target: WriteReadback,
) -> anyhow::Result<()> {
    let owner = LiveOwnershipRow::get(pool, ownership.owner().id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("live owner {} disappeared", ownership.owner().id))?;
    let mut intent = match owner.restore_intent_json {
        Some(json) => serde_json::from_str::<serde_json::Value>(&json)?,
        None => serde_json::json!({}),
    };
    let Some(root) = intent.as_object_mut() else {
        anyhow::bail!("persisted live restore intent is not a JSON object");
    };
    root.insert(
        "pid_restore".to_string(),
        serde_json::json!({
            "run_id": run_id,
            "write_kind": kind,
            "response_level": response_level,
            "previous": {
                "proportional": previous.proportional,
                "integral": previous.integral,
                "derivative": previous.derivative,
            },
            "target": {
                "proportional": target.proportional,
                "integral": target.integral,
                "derivative": target.derivative,
            },
        }),
    );
    ownership
        .persist_restore_intent(&serde_json::to_string(&intent)?)
        .await
}

/// Whether a PID write-back's confirmation readback is close enough to `requested` to count
/// as confirmed. Combined absolute (1e-3) and relative (1%) tolerance rather than exact
/// equality, since a DCS's own internal unit conversion/precision means the readback of a
/// just-written float is not guaranteed to be bit-identical -- and a purely relative
/// tolerance breaks down for a requested value at or near zero (e.g. `D = 0` for a PI
/// controller).
pub(super) fn pid_value_within_tolerance(requested: f32, actual: f32) -> bool {
    let tolerance = (1e-3_f32).max(0.01 * requested.abs());
    (actual - requested).abs() <= tolerance
}
/// Writes `value` to `tag` and reads it back to confirm the DCS accepted it within
/// [`pid_value_within_tolerance`], reusing [`write_value`] (so a transport error and a
/// rejected write both surface the same way) and [`read_f32`] (so a poor-quality or
/// non-numeric readback is never mistaken for confirmation). `label` is only used to prefix
/// the error message so a caller writing several constants in sequence can tell which one
/// failed. `pub(crate)`: also reused by `commands::history::revert` for the identical
/// write-and-verify step against a run's recorded previous values.
pub(crate) async fn write_and_verify_pid_value(
    driver: &dyn Driver,
    label: &str,
    tag: &str,
    value: f32,
    allow_uncertain: bool,
) -> Result<f32, String> {
    write_value(driver, tag, value)
        .await
        .map_err(|e| format!("{label} write to '{tag}' failed: {e}"))?;
    let readback = read_f32(driver, tag, allow_uncertain)
        .await
        .map_err(|e| format!("{label} readback from '{tag}' failed: {e}"))?;
    if pid_value_within_tolerance(value, readback) {
        Ok(readback)
    } else {
        Err(format!(
            "{label} readback {readback} from '{tag}' is outside tolerance of requested {value}"
        ))
    }
}
/// Best-effort rollback of whichever PID constants were confirmed written before a later one
/// failed -- mirroring `restore()`'s rule to attempt every step independently rather than
/// short-circuit on the first failure. `targets` contains `(label, tag, written_value,
/// previous_value)` tuples, in any order. Returns `Ok(())` only if every rollback write
/// succeeded; otherwise `Err` describing every one that did not.
pub(super) async fn rollback_pid_writes(
    driver: &dyn Driver,
    targets: &[(&str, &str, f32, f32)],
    ownership: &LiveOperationGuard,
) -> Result<(), String> {
    let mut failures = Vec::new();
    for (label, tag, value_written, previous_value) in targets {
        let Some(previous_json) = serde_json::Number::from_f64(f64::from(*value_written))
            .map(|number| number.to_string())
        else {
            failures.push(format!(
                "{label} rollback audit intent for '{tag}' has a non-finite write value"
            ));
            continue;
        };
        ownership
            .queue_previous_write_value(tag, previous_json)
            .await;
        if let Err(e) = write_value(driver, tag, *previous_value).await {
            failures.push(format!("{label} rollback write to '{tag}' failed: {e}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
/// The result of [`write_pid_values`] -- deliberately simpler than [`WriteBackOutcome`]
/// below, which layers CLI-only concerns (a `Skipped` variant covering unconfigured tags, no
/// recorded results, or an interactive skip) on top of this. `write_pid_values` only ever
/// runs once a specific, available target has already been chosen, so there is nothing left
/// to "skip" by the time it's called. Named distinctly from `bhtune_driver::WriteOutcome`
/// (that one describes a single raw tag write's own outcome; this one describes the full
/// pre-read/write/verify/rollback/audit sequence across all three PID constants).
#[derive(Debug, Clone, PartialEq)]
pub enum PidWriteOutcome {
    /// Every constant was written and confirmed within tolerance.
    Written,
    /// The pre-read, a write, or a readback failed. `detail` is a human-readable summary
    /// suitable for surfacing directly to a CLI user or an HTTP error body -- including
    /// whether/how rollback resolved, for a [`WriteKind::Write`] that failed partway
    /// through. A [`TuneWriteRow`] audit row was still inserted recording the same story in
    /// full detail; this is only ever a summary of it.
    Failed { detail: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteBackSelection {
    Selected(ResponseLevel),
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBackSkipReason {
    NoPidTags,
    NoResults,
}

/// Presentation of one raw result's controller target, including precision failures that
/// do not change the recorded calculation status.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PidWritePreview {
    pub response_level: ResponseLevel,
    pub values: Option<ControllerPidValues>,
    pub unavailable_reason: Option<String>,
}

/// Optional adapter hook for response-level selection and progress reporting.
///
/// The runtime performs all result validation, writes, readback confirmation, rollback, and
/// audit persistence. An adapter may provide an interactive selector; non-interactive
/// callers can omit the handler or select a response level on the request.
pub trait WriteBackHandler: Send {
    /// Receives persisted result previews after restoration attempts, even without PID tags.
    fn pid_results_ready(&mut self, _previews: &[PidWritePreview]) {}

    fn select_response_level(&mut self, previews: &[PidWritePreview]) -> WriteBackSelection;

    fn write_back_selected(&mut self, _values: &ControllerPidValues, _requested: bool) {}

    fn write_back_skipped(&mut self, _reason: WriteBackSkipReason) {}

    fn write_back_failed(&mut self, _detail: &str) {}

    fn write_back_finished(&mut self, _response_level: ResponseLevel, _outcome: &PidWriteOutcome) {}
}
/// Pre-reads the existing Proportional/Integral/Derivative values, writes and verifies
/// `target` (Proportional then Integral then Derivative, stopping at the first failure),
/// rolls back to the pre-read values on partial failure (only for `kind =
/// `[`WriteKind::Write`]` -- [`WriteKind::Revert`] never does, so a revert can't chase its
/// own failure with a nested rollback; see [`WriteKind`]'s own doc comment), and records
/// exactly one [`TuneWriteRow`] audit row for the attempt, success or not.
///
/// The one implementation of "pre-read, write, verify, roll back, audit" in the whole
/// workspace, shared by three callers: `maybe_write_back`'s in-run write-back,
/// [`crate::history::revert_run`], and `bhtune-server`'s post-hoc `POST /api/runs/{id}/write`/
/// `.../revert` -- `pub` (not `pub(crate)`) specifically so that
/// third, different-crate caller can reach it. `bhtune-server` calls only this function, not
/// the lower-level `read_previous_pid_values`/`write_and_verify_pid_value` helpers this
/// builds on -- those stay `pub(crate)`, since adapters need only the complete audited
/// sequence.
///
/// `target` is the caller-selected P/I/D values to write: freshly calculated parameters for a
/// [`WriteKind::Write`], or a past write's recorded `previous` values for a
/// [`WriteKind::Revert`]. Never propagates a driver/database error via `?` for an
/// operational failure -- a pre-read failure, a rejected write, a failed confirmation
/// readback, or a failed rollback all still produce their audit row and return
/// [`PidWriteOutcome::Failed`]; the `Err` case is reserved for the one thing that really is
/// exceptional here, [`TuneWriteRow::insert`] itself failing.
#[allow(clippy::too_many_arguments)]
pub async fn write_pid_values(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain: bool,
) -> anyhow::Result<PidWriteOutcome> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;
    let operation_kind = match kind {
        WriteKind::Write => bhtune_db::models::LiveOperationKind::PidWrite,
        WriteKind::Revert => bhtune_db::models::LiveOperationKind::PidRevert,
    };
    let restore_intent = serde_json::to_string(&serde_json::json!({
        "version": 1,
        "kind": "pid_restore",
        "run_id": run_id,
        "write_kind": kind,
        "state": "awaiting_prewrite_readings",
    }))?;
    let ownership =
        LiveOperationGuard::acquire_for_recorded_run(pool, &run, operation_kind, restore_intent)
            .await?;
    let audited = AuditedDriver::new(driver, pool, &ownership, Some(run_id));
    let result = write_pid_values_inner(
        pool,
        run_id,
        &audited,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain,
        &ownership,
    )
    .await;
    let release_result = ownership.release().await;
    match (result, release_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(release_error)) => Err(anyhow::anyhow!(
            "{error}; additionally failed to persist live ownership release: {release_error}"
        )),
    }
}

/// Performs an already-owned PID write sequence. `driver` must be the `AuditedDriver` for
/// the same guard so each write intent is persisted before I/O.
#[allow(clippy::too_many_arguments)]
pub async fn write_pid_values_with_owner(
    pool: &SqlitePool,
    run_id: i64,
    driver: &AuditedDriver<'_>,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain: bool,
) -> anyhow::Result<PidWriteOutcome> {
    if driver.run_id() != Some(run_id) || driver.owner().owner().run_id != Some(run_id) {
        anyhow::bail!("live ownership does not match PID write run {run_id}");
    }
    let valid_operation =
        operation_kind_authorizes_pid_write(kind, driver.owner().owner().operation_kind);
    if !valid_operation {
        anyhow::bail!("live ownership kind does not authorize the requested PID operation");
    }
    write_pid_values_inner(
        pool,
        run_id,
        driver,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain,
        driver.owner(),
    )
    .await
}

fn operation_kind_authorizes_pid_write(
    kind: WriteKind,
    operation_kind: bhtune_db::models::LiveOperationKind,
) -> bool {
    if kind == WriteKind::Write {
        operation_kind == bhtune_db::models::LiveOperationKind::Tune
            || operation_kind == bhtune_db::models::LiveOperationKind::PidWrite
    } else {
        operation_kind == bhtune_db::models::LiveOperationKind::PidRevert
    }
}

#[allow(clippy::too_many_arguments)]
async fn write_pid_values_inner(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain: bool,
    ownership: &LiveOperationGuard,
) -> anyhow::Result<PidWriteOutcome> {
    ownership.ensure_healthy()?;
    if ownership.owner().run_id != Some(run_id) {
        anyhow::bail!("live ownership does not match PID write run {run_id}");
    }
    let written_at = Utc::now();
    let mut new_write = NewTuneWrite::new(response_level, written_at);
    new_write.kind = kind;
    new_write.allow_uncertain_quality = allow_uncertain;

    let previous =
        match read_previous_pid_values(driver, p_tag, i_tag, d_tag, allow_uncertain).await {
            Ok(previous) => previous,
            Err(e) => {
                let error_message = e.to_string();
                new_write.error_message = Some(error_message.clone());
                TuneWriteRow::insert(pool, run_id, new_write).await?;
                tracing::error!(
                    run_id,
                    ?response_level,
                    ?kind,
                    %error_message,
                    "PID pre-read failed"
                );
                return Ok(PidWriteOutcome::Failed {
                    detail: format!("pre-read failed: {error_message}"),
                });
            }
        };
    new_write.previous = Some(previous);
    persist_pid_restore_intent(
        pool,
        ownership,
        run_id,
        kind,
        response_level,
        previous,
        target,
    )
    .await?;

    // Write and verify Proportional, then Integral, then Derivative, stopping at the first
    // failure. `rollback_targets` accumulates only the constants confirmed written so far,
    // so a failure partway through knows exactly what needs rolling back (when rollback
    // applies at all -- see `kind` below).
    let steps: [(&str, &str, f32, f32); 3] = [
        (
            "Proportional",
            p_tag,
            target.proportional,
            previous.proportional,
        ),
        ("Integral", i_tag, target.integral, previous.integral),
        ("Derivative", d_tag, target.derivative, previous.derivative),
    ];
    let mut written_vals: [Option<f32>; 3] = [None; 3];
    let mut readback_vals: [Option<f32>; 3] = [None; 3];
    let mut rollback_targets: Vec<(&str, &str, f32, f32)> = Vec::new();
    let mut failure: Option<String> = None;

    for (i, (label, tag, value, previous_value)) in steps.into_iter().enumerate() {
        written_vals[i] = Some(value);
        let previous_json = serde_json::to_string(&previous_value)?;
        ownership
            .queue_previous_write_value(tag, previous_json)
            .await;
        match write_and_verify_pid_value(driver, label, tag, value, allow_uncertain).await {
            Ok(readback) => {
                readback_vals[i] = Some(readback);
                rollback_targets.push((label, tag, value, previous_value));
            }
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }

    new_write.proportional_written = written_vals[0];
    new_write.integral_written = written_vals[1];
    new_write.derivative_written = written_vals[2];
    new_write.proportional_readback = readback_vals[0];
    new_write.integral_readback = readback_vals[1];
    new_write.derivative_readback = readback_vals[2];

    let Some(error_message) = failure else {
        new_write.success = true;
        TuneWriteRow::insert(pool, run_id, new_write).await?;
        tracing::info!(run_id, ?response_level, ?kind, "PID write succeeded");
        return Ok(PidWriteOutcome::Written);
    };

    new_write.success = false;
    new_write.error_message = Some(error_message.clone());

    // `WriteKind::Revert` never chases its own failure with a nested rollback (see that
    // variant's doc comment); neither does a `Write` that failed before confirming even one
    // constant, since there is nothing yet to roll back.
    if kind != WriteKind::Write || rollback_targets.is_empty() {
        TuneWriteRow::insert(pool, run_id, new_write).await?;
        tracing::error!(run_id, ?response_level, ?kind, %error_message, "PID write failed");
        return Ok(PidWriteOutcome::Failed {
            detail: error_message,
        });
    }

    match rollback_pid_writes(driver, &rollback_targets, ownership).await {
        Ok(()) => {
            new_write.rollback_state = Some(RollbackState::Succeeded);
            TuneWriteRow::insert(pool, run_id, new_write).await?;
            tracing::error!(run_id, ?response_level, %error_message, "PID write failed partway through; rollback succeeded");
            Ok(PidWriteOutcome::Failed {
                detail: format!("{error_message} (rolled back)"),
            })
        }
        Err(rollback_error) => {
            new_write.rollback_state = Some(RollbackState::Failed);
            new_write.rollback_error = Some(rollback_error.clone());
            TuneWriteRow::insert(pool, run_id, new_write).await?;
            tracing::error!(
                run_id,
                ?response_level,
                %error_message,
                %rollback_error,
                "PID write failed partway through; rollback also failed"
            );
            Ok(PidWriteOutcome::Failed {
                detail: format!(
                    "{error_message}; rollback also failed: {rollback_error} -- the loop may \
                     hold a mismatched set of PID constants, see \
                     `bhtune history revert {run_id}`"
                ),
            })
        }
    }
}
/// Validates a persisted result's raw, full-precision PID parameters.
///
/// This is the single validity gate shared by the CLI and HTTP write paths. A result must be
/// explicitly valid and contain finite values for all three constants; malformed historical
/// rows are rejected rather than being allowed to reach a live driver.
pub fn pid_parameters_for_result(result: &TuneResultRow) -> anyhow::Result<PidParameters> {
    if result.status != TuningResultStatus::Valid {
        let reason = result
            .invalid_reason
            .map(|reason| reason.to_string())
            .unwrap_or_else(|| "no invalid reason was recorded".to_string());
        anyhow::bail!(
            "{:?} calculated result is invalid: {reason}",
            result.response_level
        );
    }
    if result.invalid_reason.is_some() {
        anyhow::bail!(
            "{:?} calculated result has an invalid reason despite being marked valid",
            result.response_level
        );
    }

    let proportional = result.proportional.ok_or_else(|| {
        anyhow::anyhow!(
            "{:?} calculated result is missing its proportional value",
            result.response_level
        )
    })?;
    let integral = result.integral.ok_or_else(|| {
        anyhow::anyhow!(
            "{:?} calculated result is missing its integral value",
            result.response_level
        )
    })?;
    let derivative = result.derivative.ok_or_else(|| {
        anyhow::anyhow!(
            "{:?} calculated result is missing its derivative value",
            result.response_level
        )
    })?;
    if !proportional.is_finite() || !integral.is_finite() || !derivative.is_finite() {
        anyhow::bail!(
            "{:?} calculated result contains a non-finite PID value",
            result.response_level
        );
    }

    Ok(PidParameters {
        response_level: result.response_level,
        proportional,
        integral,
        derivative,
    })
}

/// The shared new-write validity/precision gate. Restore and revert paths deliberately
/// do not call this: their recorded targets must retain their original precision.
pub fn controller_pid_for_result(
    result: &TuneResultRow,
    controller_type: ControllerType,
    template: &DcsTemplate,
) -> anyhow::Result<ControllerPidValues> {
    let pid = pid_parameters_for_result(result)?;
    controller_pid_values(pid, controller_type, template).map_err(|error| {
        anyhow::anyhow!(
            "{:?} controller PID target is unavailable: {error}",
            result.response_level
        )
    })
}

pub fn pid_write_preview(
    result: &TuneResultRow,
    controller_type: ControllerType,
    template: &DcsTemplate,
) -> PidWritePreview {
    let (values, unavailable_reason) =
        match controller_pid_for_result(result, controller_type, template) {
            Ok(values) => (Some(values), None),
            Err(error) => (None, Some(error.to_string())),
        };
    PidWritePreview {
        response_level: result.response_level,
        values,
        unavailable_reason,
    }
}
/// Writes the requested PID result, or asks an optional adapter handler to select one.
/// Without a handler, an unrequested write is skipped; the runtime never reads stdin or
/// writes to stdout.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(super) async fn maybe_write_back(
    pool: &SqlitePool,
    run_id: i64,
    tags: &LoopTags,
    template: &DcsTemplate,
    driver: &dyn Driver,
    config: LoopConfig,
    write_pid: Option<ResponseLevel>,
    allow_uncertain: bool,
    handler: Option<&mut dyn WriteBackHandler>,
) -> anyhow::Result<(WriteBackOutcome, Option<String>)> {
    maybe_write_back_with_owner(
        pool,
        run_id,
        tags,
        template,
        driver,
        config,
        write_pid,
        allow_uncertain,
        handler,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_write_back_with_owner(
    pool: &SqlitePool,
    run_id: i64,
    tags: &LoopTags,
    template: &DcsTemplate,
    driver: &dyn Driver,
    config: LoopConfig,
    write_pid: Option<ResponseLevel>,
    allow_uncertain: bool,
    mut handler: Option<&mut dyn WriteBackHandler>,
    ownership: Option<&LiveOperationGuard>,
) -> anyhow::Result<(WriteBackOutcome, Option<String>)> {
    let (Some(p_tag), Some(i_tag), Some(d_tag)) = (
        &tags.proportional_constant,
        &tags.integral_constant,
        &tags.derivative_constant,
    ) else {
        if let Some(handler) = handler.as_deref_mut() {
            handler.write_back_skipped(WriteBackSkipReason::NoPidTags);
        }
        return Ok((
            WriteBackOutcome::Skipped,
            Some("no PID constant tags configured for this run's driver/template".to_string()),
        ));
    };

    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    if results.is_empty() {
        if let Some(handler) = handler.as_deref_mut() {
            handler.write_back_skipped(WriteBackSkipReason::NoResults);
        }
        return Ok((
            WriteBackOutcome::Skipped,
            Some("no calculated results were recorded for this run".to_string()),
        ));
    }

    let selection = match write_pid {
        Some(level) => WriteBackSelection::Selected(level),
        None => match handler.as_deref_mut() {
            Some(handler) => handler.select_response_level(
                &results
                    .iter()
                    .map(|result| pid_write_preview(result, config.controller_type, template))
                    .collect::<Vec<_>>(),
            ),
            None => WriteBackSelection::Skipped(
                "no response level was selected by the caller".to_string(),
            ),
        },
    };
    let response_level = match selection {
        WriteBackSelection::Selected(level) => level,
        WriteBackSelection::Skipped(detail) => {
            return Ok((WriteBackOutcome::Skipped, Some(detail)));
        }
        WriteBackSelection::Failed(detail) => {
            if let Some(handler) = handler.as_deref_mut() {
                handler.write_back_failed(&detail);
            }
            return Ok((WriteBackOutcome::Failed, Some(detail)));
        }
    };

    let Some(selected) = results
        .iter()
        .find(|result| result.response_level == response_level)
    else {
        let detail = format!("no calculated result recorded for response level {response_level:?}");
        if let Some(handler) = handler.as_deref_mut() {
            handler.write_back_failed(&detail);
        }
        return Ok((WriteBackOutcome::Failed, Some(detail)));
    };
    let written = match controller_pid_for_result(selected, config.controller_type, template) {
        Ok(written) => written,
        Err(error) => {
            let detail = error.to_string();
            if let Some(handler) = handler.as_deref_mut() {
                handler.write_back_failed(&detail);
            }
            return Ok((WriteBackOutcome::Failed, Some(detail)));
        }
    };
    let response_level = written.response_level;
    if let Some(handler) = handler.as_deref_mut() {
        handler.write_back_selected(&written, write_pid.is_some());
    }
    let target = WriteReadback {
        proportional: written.proportional.value,
        integral: written.integral.value,
        derivative: written.derivative.value,
    };

    let outcome = match ownership {
        Some(ownership) => {
            write_pid_values_inner(
                pool,
                run_id,
                driver,
                p_tag,
                i_tag,
                d_tag,
                response_level,
                target,
                WriteKind::Write,
                allow_uncertain,
                ownership,
            )
            .await?
        }
        None => anyhow::bail!(
            "PID write-back requires a live ownership guard before controller mutation"
        ),
    };

    if let Some(handler) = handler {
        handler.write_back_finished(response_level, &outcome);
    }

    let report = match outcome {
        PidWriteOutcome::Written => (WriteBackOutcome::Written { response_level }, None),
        PidWriteOutcome::Failed { detail } => (WriteBackOutcome::Failed, Some(detail)),
    };
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use bhtune_core::{
        ControllerType, LoopConfig, LoopTags, ProcessType, TuningResultInvalidReason,
        TuningResultStatus,
    };
    use bhtune_db::models::{
        LiveMutationStepRow, LiveOperationKind, TemplateOrigin, TuneDriver, TuneRunRow,
    };
    use bhtune_driver::{
        BrowsePage, BrowsePageRequest, Driver, DriverError, DriverResult, Quality, TagId, TagValue,
        TagWrite, WriteOutcome,
    };
    use chrono::Utc;

    use super::*;
    use crate::live_ownership::AuditedDriver;

    #[test]
    fn pid_write_ownership_is_operation_specific() {
        assert!(operation_kind_authorizes_pid_write(
            WriteKind::Write,
            LiveOperationKind::Tune
        ));
        assert!(operation_kind_authorizes_pid_write(
            WriteKind::Write,
            LiveOperationKind::PidWrite
        ));
        assert!(operation_kind_authorizes_pid_write(
            WriteKind::Revert,
            LiveOperationKind::PidRevert
        ));
        assert!(!operation_kind_authorizes_pid_write(
            WriteKind::Write,
            LiveOperationKind::OpcWrite
        ));
        assert!(!operation_kind_authorizes_pid_write(
            WriteKind::Revert,
            LiveOperationKind::Tune
        ));
    }

    struct FailingDriver;

    #[async_trait]
    impl Driver for FailingDriver {
        async fn read(&self, _tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
            Err(DriverError::Unsupported { operation: "test" })
        }

        async fn write(&self, _tag: &TagId, _value: TagWrite) -> DriverResult<WriteOutcome> {
            Err(DriverError::Unsupported { operation: "test" })
        }

        async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
            Err(DriverError::Unsupported { operation: "test" })
        }
    }

    struct PidDriver {
        values: std::sync::Mutex<HashMap<String, String>>,
        write_counts: std::sync::Mutex<HashMap<String, usize>>,
        failure_calls: HashMap<String, Vec<usize>>,
        read_calls: AtomicUsize,
        write_calls: AtomicUsize,
    }

    impl PidDriver {
        fn new(tags: &LoopTags, failure_calls: HashMap<String, Vec<usize>>) -> Self {
            let mut values = HashMap::new();
            for (tag, value) in [
                (tags.proportional_constant.as_ref(), "1.0"),
                (tags.integral_constant.as_ref(), "2.0"),
                (tags.derivative_constant.as_ref(), "0.0"),
            ] {
                if let Some(tag) = tag {
                    values.insert(tag.clone(), value.to_string());
                }
            }
            Self {
                values: std::sync::Mutex::new(values),
                write_counts: std::sync::Mutex::new(HashMap::new()),
                failure_calls,
                read_calls: AtomicUsize::new(0),
                write_calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl Driver for PidDriver {
        async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
            self.read_calls.fetch_add(1, Ordering::Relaxed);
            let values = self.values.lock().unwrap();
            tags.iter()
                .map(|tag| {
                    let value = values.get(tag).cloned().ok_or_else(|| {
                        DriverError::Operation(Box::new(std::io::Error::other(
                            "test PID tag has no value",
                        )))
                    })?;
                    Ok(TagValue {
                        tag: tag.clone(),
                        value,
                        quality: Quality::Good,
                        timestamp: None,
                    })
                })
                .collect()
        }

        async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
            self.write_calls.fetch_add(1, Ordering::Relaxed);
            let call = {
                let mut counts = self.write_counts.lock().unwrap();
                let count = counts.entry(tag.clone()).or_default();
                *count += 1;
                *count
            };
            if self
                .failure_calls
                .get(tag)
                .is_some_and(|calls| calls.contains(&call))
            {
                return Ok(WriteOutcome::failure("simulated write rejection"));
            }
            let value = match value {
                TagWrite::Float(value) => value.to_string(),
                TagWrite::Raw(value) => value,
            };
            self.values.lock().unwrap().insert(tag.clone(), value);
            Ok(WriteOutcome::success())
        }

        async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
            Err(DriverError::Unsupported { operation: "test" })
        }
    }

    struct RecordingHandler {
        selection: WriteBackSelection,
        events: Vec<String>,
    }

    impl RecordingHandler {
        fn new(selection: WriteBackSelection) -> Self {
            Self {
                selection,
                events: Vec::new(),
            }
        }
    }

    impl WriteBackHandler for RecordingHandler {
        fn select_response_level(&mut self, _previews: &[PidWritePreview]) -> WriteBackSelection {
            self.events.push("select".to_string());
            self.selection.clone()
        }

        fn write_back_selected(&mut self, values: &ControllerPidValues, requested: bool) {
            let level = values.response_level;
            self.events.push(format!("selected:{level:?}:{requested}"));
        }

        fn write_back_skipped(&mut self, reason: WriteBackSkipReason) {
            self.events.push(format!("skipped:{reason:?}"));
        }

        fn write_back_failed(&mut self, detail: &str) {
            self.events.push(format!("failed:{detail}"));
        }

        fn write_back_finished(&mut self, level: ResponseLevel, outcome: &PidWriteOutcome) {
            self.events.push(format!("finished:{level:?}:{outcome:?}"));
        }
    }

    struct DefaultHandler;

    impl WriteBackHandler for DefaultHandler {
        fn select_response_level(&mut self, _previews: &[PidWritePreview]) -> WriteBackSelection {
            WriteBackSelection::Skipped("no selection".to_string())
        }
    }

    async fn fixture() -> (SqlitePool, i64, DcsTemplate, LoopTags, LoopConfig) {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, Utc::now())
            .await
            .unwrap();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        (pool, run.id, template, tags, config)
    }

    async fn recorded_fixture() -> (SqlitePool, i64, DcsTemplate, LoopTags, LoopConfig) {
        let (pool, run_id, template, tags, config) = fixture().await;
        TuneRunRow::record_connection(
            &pool,
            run_id,
            Some("Mock.Kepware.Sim"),
            Some("127.0.0.1:7602"),
            "{}",
        )
        .await
        .unwrap();
        (pool, run_id, template, tags, config)
    }

    fn result_row(run_id: i64, response_level: ResponseLevel) -> TuneResultRow {
        TuneResultRow {
            id: 0,
            run_id,
            response_level,
            kp: Some(2.0),
            ti_minutes: Some(4.0),
            td_minutes: Some(0.5),
            proportional: Some(2.0),
            integral: Some(4.0),
            derivative: Some(0.5),
            status: TuningResultStatus::Valid,
            invalid_reason: None,
        }
    }

    #[test]
    fn controller_preview_preserves_raw_values_and_reports_precision_failures() {
        let mut template = bhtune_core::built_in_templates().remove(0);
        let mut result = result_row(1, ResponseLevel::Moderate);
        result.proportional = Some(155.21378);
        result.integral = Some(2.482169);
        let preview = pid_write_preview(&result, ControllerType::Pi, &template);
        let values = preview.values.unwrap();
        assert_eq!(values.proportional.value, 155.2);
        assert_eq!(values.proportional.display, "155.2");
        assert_eq!(values.integral.value, 2.5);
        assert_eq!(values.integral.display, "2.5");
        assert_eq!(values.derivative.value, 0.0);
        assert_eq!(values.derivative.display, "0.0");
        assert!(preview.unavailable_reason.is_none());
        assert_eq!(result.proportional, Some(155.21378));
        assert_eq!(result.integral, Some(2.482169));
        assert_eq!(result.status, TuningResultStatus::Valid);

        result.integral = Some(0.049);
        let preview = pid_write_preview(&result, ControllerType::Pi, &template);
        assert!(preview.values.is_none());
        assert!(preview.unavailable_reason.unwrap().contains("integral"));
        assert_eq!(result.status, TuningResultStatus::Valid);

        template.pid_rounding = bhtune_core::PidRounding::default();
        let values = controller_pid_for_result(&result, ControllerType::P, &template).unwrap();
        assert_eq!(values.integral.value, 9999.0);
        assert_eq!(values.integral.display, "9999");
        assert_eq!(values.derivative.value, 0.0);
    }

    #[tokio::test]
    async fn rounding_to_zero_rejects_automated_and_interactive_writes_before_pid_io() {
        let (pool, run_id, template, tags, config) = fixture().await;
        let mut result = result_row(run_id, ResponseLevel::Moderate);
        result.integral = Some(0.049);
        TuneResultRow::insert(&pool, &result).await.unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());

        for requested in [Some(ResponseLevel::Moderate), None] {
            let mut handler =
                RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));
            let (outcome, detail) = maybe_write_back(
                &pool,
                run_id,
                &tags,
                &template,
                &driver,
                config,
                requested,
                true,
                Some(&mut handler),
            )
            .await
            .unwrap();
            assert_eq!(outcome, WriteBackOutcome::Failed);
            assert!(detail.unwrap().contains("erase an active term to zero"));
            assert!(handler.events.last().unwrap().starts_with("failed:"));
        }
        assert_eq!(driver.read_calls.load(Ordering::Relaxed), 0);
        assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
        assert!(
            TuneWriteRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn rounded_write_matches_preview_and_audit_but_revert_preserves_original_precision() {
        let (pool, run_id, template, tags, config) = recorded_fixture().await;
        let mut result = result_row(run_id, ResponseLevel::Moderate);
        result.proportional = Some(155.21378);
        result.integral = Some(2.482169);
        TuneResultRow::insert(&pool, &result).await.unwrap();
        let expected =
            controller_pid_for_result(&result, config.controller_type, &template).unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());
        let original = WriteReadback {
            proportional: 7.123456,
            integral: 8.765432,
            derivative: 0.123456,
        };
        for (tag, value) in [
            (
                tags.proportional_constant.as_ref().unwrap(),
                original.proportional,
            ),
            (tags.integral_constant.as_ref().unwrap(), original.integral),
            (
                tags.derivative_constant.as_ref().unwrap(),
                original.derivative,
            ),
        ] {
            driver
                .values
                .lock()
                .unwrap()
                .insert(tag.clone(), value.to_string());
        }
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
        let (outcome, detail) = maybe_write_back_with_owner(
            &pool,
            run_id,
            &tags,
            &template,
            &audited,
            config,
            Some(ResponseLevel::Moderate),
            false,
            None,
            Some(&owner),
        )
        .await
        .unwrap();
        assert!(matches!(outcome, WriteBackOutcome::Written { .. }));
        assert!(detail.is_none());
        let written = TuneWriteRow::list_for_run(&pool, run_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(written.previous, Some(original));
        assert_eq!(
            written.proportional_written,
            Some(expected.proportional.value)
        );
        assert_eq!(written.integral_written, Some(expected.integral.value));
        assert_eq!(written.derivative_written, Some(expected.derivative.value));
        assert_eq!(written.proportional_readback, written.proportional_written);
        assert_eq!(written.integral_readback, written.integral_written);
        assert_eq!(written.derivative_readback, written.derivative_written);
        owner.release().await.unwrap();

        let outcome = write_pid_values(
            &pool,
            run_id,
            &driver,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            original,
            WriteKind::Revert,
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome, PidWriteOutcome::Written);
        let reverted = TuneWriteRow::list_for_run(&pool, run_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reverted.kind, WriteKind::Revert);
        assert_eq!(reverted.proportional_written, Some(original.proportional));
        assert_eq!(reverted.integral_written, Some(original.integral));
        assert_eq!(reverted.derivative_written, Some(original.derivative));
    }

    #[test]
    fn optional_handler_defaults_are_no_ops() {
        let mut handler = DefaultHandler;
        handler.pid_results_ready(&[]);
        assert!(matches!(
            handler.select_response_level(&[]),
            WriteBackSelection::Skipped(reason) if reason == "no selection"
        ));
        let values = controller_pid_for_result(
            &result_row(1, ResponseLevel::Moderate),
            ControllerType::Pi,
            &bhtune_core::built_in_templates().remove(0),
        )
        .unwrap();
        handler.write_back_selected(&values, true);
        handler.write_back_skipped(WriteBackSkipReason::NoPidTags);
        handler.write_back_failed("ignored");
        handler.write_back_finished(ResponseLevel::Moderate, &PidWriteOutcome::Written);
    }

    #[tokio::test]
    async fn failing_driver_reports_unsupported_write_and_browse() {
        let driver = FailingDriver;
        assert!(matches!(
            driver.write(&"tag".to_string(), TagWrite::Float(1.0)).await,
            Err(DriverError::Unsupported { operation: "test" })
        ));
        assert!(matches!(
            driver.browse(BrowsePageRequest::root(1)).await,
            Err(DriverError::Unsupported { operation: "test" })
        ));
    }

    #[tokio::test]
    async fn pid_driver_reports_missing_value_and_unsupported_browse() {
        let (_pool, _run_id, _template, tags, _config) = fixture().await;
        let driver = PidDriver::new(&tags, HashMap::new());

        assert!(matches!(
            driver.read(&["missing".to_string()]).await,
            Err(DriverError::Operation(_))
        ));
        assert!(matches!(
            driver.browse(BrowsePageRequest::root(1)).await,
            Err(DriverError::Unsupported { operation: "test" })
        ));
    }

    #[tokio::test]
    async fn pid_writeback_notifies_handler_after_successful_write() {
        let (pool, run_id, template, tags, config) = fixture().await;
        TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
            .await
            .unwrap();
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
        let mut handler =
            RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));

        let (outcome, detail) = maybe_write_back_with_owner(
            &pool,
            run_id,
            &tags,
            &template,
            &audited,
            config,
            Some(ResponseLevel::Moderate),
            false,
            Some(&mut handler),
            Some(&owner),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Moderate
            }
        ));
        assert!(detail.is_none());
        assert_eq!(
            handler.events,
            ["selected:Moderate:true", "finished:Moderate:Written",]
        );
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn pid_rollback_rejects_non_finite_audit_values_before_writing() {
        let (pool, run_id, _template, tags, _config) = fixture().await;
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());

        let error = rollback_pid_writes(
            &driver,
            &[("Proportional", "Unit1.LIC101.P", f32::NAN, 1.0)],
            &owner,
        )
        .await
        .unwrap_err();

        assert!(error.contains("non-finite write value"));
        assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn owned_writeback_audits_all_pid_mutations_before_reporting_success() {
        let (pool, run_id, template, tags, config) = fixture().await;
        TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
            .await
            .unwrap();
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));

        let (outcome, detail) = maybe_write_back_with_owner(
            &pool,
            run_id,
            &tags,
            &template,
            &audited,
            config,
            Some(ResponseLevel::Moderate),
            false,
            None,
            Some(&owner),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Moderate
            }
        ));
        assert!(detail.is_none());
        assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);
        assert!(
            TuneWriteRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .last()
                .unwrap()
                .success
        );
        let audit = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
            .await
            .unwrap();
        assert_eq!(audit.len(), 3);
        assert!(
            audit
                .iter()
                .all(|step| step.status == bhtune_db::models::MutationStepStatus::Confirmed)
        );
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn owned_pid_write_accepts_tune_ownership() {
        let (pool, run_id, _template, tags, _config) = fixture().await;
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::Tune,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));

        let outcome = write_pid_values_with_owner(
            &pool,
            run_id,
            &audited,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 5.0,
                integral: 6.0,
                derivative: 7.0,
            },
            WriteKind::Write,
            false,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, PidWriteOutcome::Written));
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn pid_write_with_owner_reverts_only_constants_confirmed_before_a_later_failure() {
        let (pool, run_id, _template, tags, _config) = fixture().await;
        let integral = tags.integral_constant.as_ref().unwrap().clone();
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::from([(integral, vec![1])]));
        let original_proportional = 1.234567_f32;
        driver.values.lock().unwrap().insert(
            tags.proportional_constant.as_ref().unwrap().clone(),
            original_proportional.to_string(),
        );
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
        let target = WriteReadback {
            proportional: 5.0,
            integral: 6.0,
            derivative: 7.0,
        };

        let outcome = write_pid_values_with_owner(
            &pool,
            run_id,
            &audited,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            target,
            WriteKind::Write,
            false,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PidWriteOutcome::Failed { ref detail } if detail.contains("rolled back")
        ));
        assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);
        assert_eq!(
            driver
                .values
                .lock()
                .unwrap()
                .get(tags.proportional_constant.as_deref().unwrap())
                .map(String::as_str),
            Some(original_proportional.to_string().as_str())
        );
        let persisted = TuneWriteRow::list_for_run(&pool, run_id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(persisted.rollback_state, Some(RollbackState::Succeeded));
        let audit = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
            .await
            .unwrap();
        assert_eq!(audit.len(), 3);
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn pid_write_wrapper_owns_success_and_revert_pre_read_failure_paths() {
        let (pool, run_id, _template, tags, _config) = recorded_fixture().await;
        let driver = PidDriver::new(&tags, HashMap::new());
        let target = WriteReadback {
            proportional: 2.0,
            integral: 3.0,
            derivative: 0.5,
        };
        assert_eq!(
            write_pid_values(
                &pool,
                run_id,
                &driver,
                tags.proportional_constant.as_deref().unwrap(),
                tags.integral_constant.as_deref().unwrap(),
                tags.derivative_constant.as_deref().unwrap(),
                ResponseLevel::Moderate,
                target,
                WriteKind::Write,
                false,
            )
            .await
            .unwrap(),
            PidWriteOutcome::Written
        );
        assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);

        assert!(
            write_pid_values(
                &pool,
                run_id,
                &FailingDriver,
                tags.proportional_constant.as_deref().unwrap(),
                tags.integral_constant.as_deref().unwrap(),
                tags.derivative_constant.as_deref().unwrap(),
                ResponseLevel::Moderate,
                target,
                WriteKind::Revert,
                false,
            )
            .await
            .is_ok_and(|outcome| matches!(outcome, PidWriteOutcome::Failed { .. }))
        );
        let owners = bhtune_db::models::LiveOwnershipRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(owners.len(), 2);
        assert!(
            owners
                .iter()
                .all(|owner| owner.state == bhtune_db::models::LiveOwnershipState::Released)
        );
        assert!(
            write_pid_values(
                &pool,
                run_id + 100,
                &FailingDriver,
                "P",
                "I",
                "D",
                ResponseLevel::Moderate,
                target,
                WriteKind::Write,
                false,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn pid_write_wrapper_never_hides_write_or_release_persistence_failures() {
        let (pool, run_id, _template, tags, _config) = recorded_fixture().await;
        sqlx::query(
            "CREATE TRIGGER reject_pid_write_insert BEFORE INSERT ON tune_writes BEGIN SELECT RAISE(FAIL, 'injected PID write insert failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(
            write_pid_values(
                &pool,
                run_id,
                &FailingDriver,
                tags.proportional_constant.as_deref().unwrap(),
                tags.integral_constant.as_deref().unwrap(),
                tags.derivative_constant.as_deref().unwrap(),
                ResponseLevel::Moderate,
                WriteReadback {
                    proportional: 2.0,
                    integral: 3.0,
                    derivative: 0.5,
                },
                WriteKind::Write,
                false,
            )
            .await
            .is_err()
        );
        sqlx::query("DROP TRIGGER reject_pid_write_insert")
            .execute(&pool)
            .await
            .unwrap();

        let (release_pool, release_run_id, _template, release_tags, _config) =
            recorded_fixture().await;
        sqlx::query(
            "CREATE TRIGGER reject_pid_owner_release BEFORE UPDATE OF state ON live_operation_owners WHEN NEW.state = 'released' BEGIN SELECT RAISE(FAIL, 'injected owner release failure'); END",
        )
        .execute(&release_pool)
        .await
        .unwrap();
        let release_error = write_pid_values(
            &release_pool,
            release_run_id,
            &FailingDriver,
            release_tags.proportional_constant.as_deref().unwrap(),
            release_tags.integral_constant.as_deref().unwrap(),
            release_tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 2.0,
                integral: 3.0,
                derivative: 0.5,
            },
            WriteKind::Revert,
            false,
        )
        .await
        .unwrap_err();
        assert!(release_error.to_string().contains("owner release failure"));
        sqlx::query("DROP TRIGGER reject_pid_owner_release")
            .execute(&release_pool)
            .await
            .unwrap();

        let (combined_pool, combined_run_id, _template, combined_tags, _config) =
            recorded_fixture().await;
        sqlx::query(
            "CREATE TRIGGER reject_pid_restore_intent BEFORE UPDATE OF restore_intent_json ON live_operation_owners BEGIN SELECT RAISE(FAIL, 'injected restore intent failure'); END",
        )
        .execute(&combined_pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_pid_owner_release_again BEFORE UPDATE OF state ON live_operation_owners WHEN NEW.state = 'released' BEGIN SELECT RAISE(FAIL, 'injected owner release failure'); END",
        )
        .execute(&combined_pool)
        .await
        .unwrap();
        let combined_error = write_pid_values(
            &combined_pool,
            combined_run_id,
            &PidDriver::new(&combined_tags, HashMap::new()),
            combined_tags.proportional_constant.as_deref().unwrap(),
            combined_tags.integral_constant.as_deref().unwrap(),
            combined_tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 2.0,
                integral: 3.0,
                derivative: 0.5,
            },
            WriteKind::Write,
            false,
        )
        .await
        .unwrap_err();
        assert!(
            combined_error
                .to_string()
                .contains("additionally failed to persist live ownership release"),
            "{combined_error:#}"
        );
    }

    #[tokio::test]
    async fn owned_pid_write_rejects_mismatched_run_and_operation_kind() {
        let (pool, run_id, _template, tags, _config) = fixture().await;
        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        let driver = PidDriver::new(&tags, HashMap::new());
        let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
        let target = WriteReadback {
            proportional: 2.0,
            integral: 3.0,
            derivative: 0.5,
        };
        assert!(
            write_pid_values_with_owner(
                &pool,
                run_id + 1,
                &audited,
                "P",
                "I",
                "D",
                ResponseLevel::Moderate,
                target,
                WriteKind::Write,
                false,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("does not match PID write run")
        );
        assert!(
            write_pid_values_with_owner(
                &pool,
                run_id,
                &audited,
                "P",
                "I",
                "D",
                ResponseLevel::Moderate,
                target,
                WriteKind::Revert,
                false,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("does not authorize")
        );
        assert!(
            write_pid_values_inner(
                &pool,
                run_id + 1,
                &audited,
                "P",
                "I",
                "D",
                ResponseLevel::Moderate,
                target,
                WriteKind::Write,
                false,
                &owner,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("does not match PID write run")
        );
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn pid_restore_intent_requires_a_present_json_object_and_persists_target_values() {
        let (pool, run_id, _template, tags, _config) = fixture().await;
        let target = WriteReadback {
            proportional: 2.0,
            integral: 3.0,
            derivative: 0.5,
        };
        let previous = WriteReadback {
            proportional: 1.0,
            integral: 2.0,
            derivative: 0.0,
        };

        let owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            None,
        )
        .await
        .unwrap();
        persist_pid_restore_intent(
            &pool,
            &owner,
            run_id,
            WriteKind::Write,
            ResponseLevel::Moderate,
            previous,
            target,
        )
        .await
        .unwrap();
        let persisted = bhtune_db::models::LiveOwnershipRow::get(&pool, owner.owner().id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(
                persisted.restore_intent_json.as_deref().unwrap()
            )
            .unwrap()["pid_restore"]["target"]["proportional"]
                == 2.0
        );
        owner.release().await.unwrap();

        let non_object_owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("[]".to_string()),
        )
        .await
        .unwrap();
        assert!(
            persist_pid_restore_intent(
                &pool,
                &non_object_owner,
                run_id,
                WriteKind::Write,
                ResponseLevel::Moderate,
                previous,
                target,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("not a JSON object")
        );
        non_object_owner.release().await.unwrap();

        let missing_owner = LiveOperationGuard::acquire(
            &pool,
            Some(run_id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &tags.manipulated_variable,
            Some("{}".to_string()),
        )
        .await
        .unwrap();
        sqlx::query("DELETE FROM live_operation_owners WHERE id = ?")
            .bind(missing_owner.owner().id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            persist_pid_restore_intent(
                &pool,
                &missing_owner,
                run_id,
                WriteKind::Write,
                ResponseLevel::Moderate,
                previous,
                target,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("disappeared")
        );
        drop(missing_owner);
    }

    #[tokio::test]
    async fn maybe_write_back_reports_skips_and_selection_failures() {
        let (pool, run_id, template, tags, config) = fixture().await;
        let driver = FailingDriver;

        let mut no_tags = tags.clone();
        no_tags.proportional_constant = None;
        let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
        let result = maybe_write_back(
            &pool,
            run_id,
            &no_tags,
            &template,
            &driver,
            config,
            None,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Skipped);
        assert_eq!(handler.events, ["skipped:NoPidTags"]);

        let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
        let result = maybe_write_back(
            &pool,
            run_id + 1,
            &tags,
            &template,
            &driver,
            config,
            None,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Skipped);
        assert_eq!(handler.events, ["skipped:NoResults"]);

        TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
            .await
            .unwrap();

        let mut handler =
            RecordingHandler::new(WriteBackSelection::Failed("selection failed".to_string()));
        let result = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            None,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Failed);
        assert_eq!(handler.events, ["select", "failed:selection failed"]);

        let mut handler =
            RecordingHandler::new(WriteBackSelection::Skipped("operator skipped".to_string()));
        let result = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            None,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Skipped);
        assert_eq!(handler.events, ["select"]);
    }

    #[tokio::test]
    async fn maybe_write_back_reports_missing_invalid_and_live_write_results() {
        let (pool, run_id, template, tags, config) = fixture().await;
        let driver = FailingDriver;
        TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
            .await
            .unwrap();
        TuneResultRow::insert(
            &pool,
            &TuneResultRow {
                status: TuningResultStatus::Invalid,
                invalid_reason: Some(TuningResultInvalidReason::NonPositivePvAmplitude),
                kp: None,
                ti_minutes: None,
                td_minutes: None,
                proportional: None,
                integral: None,
                derivative: None,
                ..result_row(run_id, ResponseLevel::Aggressive)
            },
        )
        .await
        .unwrap();

        let mut handler =
            RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));
        let result = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            Some(ResponseLevel::Sluggish),
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Failed);
        assert!(handler.events[0].starts_with("failed:no calculated result"));

        let mut handler =
            RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Aggressive));
        let result = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            None,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Failed);
        assert_eq!(handler.events.len(), 2);
        assert_eq!(handler.events[0], "select");
        assert!(handler.events[1].starts_with("failed:"));

        let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
        let error = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            Some(ResponseLevel::Moderate),
            true,
            Some(&mut handler),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires a live ownership guard")
        );
        assert_eq!(handler.events, ["selected:Moderate:true"]);
    }
}
