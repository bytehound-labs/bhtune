#![allow(rustdoc::broken_intra_doc_links)]

use bhtune_core::{
    DcsTemplate, LoopConfig, LoopTags, PidParameters, ResponseLevel, TuningResultStatus,
    opc_write_values,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    NewTuneWrite, RollbackState, TuneResultRow, TuneWriteRow, WriteKind, WriteReadback,
};
use bhtune_driver::Driver;
use chrono::Utc;

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
/// short-circuit on the first failure. `targets` is `(label, tag,
/// previous_value)` triples, in any order. Returns `Ok(())` only if every rollback write
/// succeeded; otherwise `Err` describing every one that did not.
pub(super) async fn rollback_pid_writes(
    driver: &dyn Driver,
    targets: &[(&str, &str, f32)],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for (label, tag, previous_value) in targets {
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

/// Optional adapter hook for response-level selection and progress reporting.
///
/// The runtime performs all result validation, writes, readback confirmation, rollback, and
/// audit persistence. An adapter may provide an interactive selector; non-interactive
/// callers can omit the handler or select a response level on the request.
pub trait WriteBackHandler: Send {
    fn select_response_level(&mut self, results: &[TuneResultRow]) -> WriteBackSelection;

    fn write_back_selected(&mut self, _response_level: ResponseLevel, _requested: bool) {}

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
    let mut rollback_targets: Vec<(&str, &str, f32)> = Vec::new();
    let mut failure: Option<String> = None;

    for (i, (label, tag, value, previous_value)) in steps.into_iter().enumerate() {
        written_vals[i] = Some(value);
        match write_and_verify_pid_value(driver, label, tag, value, allow_uncertain).await {
            Ok(readback) => {
                readback_vals[i] = Some(readback);
                rollback_targets.push((label, tag, previous_value));
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

    match rollback_pid_writes(driver, &rollback_targets).await {
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
/// Converts a persisted result into the exact PID values that may be written to a controller.
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
/// Writes the requested PID result, or asks an optional adapter handler to select one.
/// Without a handler, an unrequested write is skipped; the runtime never reads stdin or
/// writes to stdout.
#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_write_back(
    pool: &SqlitePool,
    run_id: i64,
    tags: &LoopTags,
    template: &DcsTemplate,
    driver: &dyn Driver,
    config: LoopConfig,
    write_pid: Option<ResponseLevel>,
    allow_uncertain: bool,
    mut handler: Option<&mut dyn WriteBackHandler>,
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
            Some(handler) => handler.select_response_level(&results),
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
    let pid = match pid_parameters_for_result(selected) {
        Ok(pid) => pid,
        Err(error) => {
            let detail = error.to_string();
            if let Some(handler) = handler.as_deref_mut() {
                handler.write_back_failed(&detail);
            }
            return Ok((WriteBackOutcome::Failed, Some(detail)));
        }
    };
    let response_level = pid.response_level;
    if let Some(handler) = handler.as_deref_mut() {
        handler.write_back_selected(response_level, write_pid.is_some());
    }
    let written = opc_write_values(pid, config.controller_type, template.integral_type);
    let target = WriteReadback {
        proportional: written.proportional,
        integral: written.integral,
        derivative: written.derivative,
    };

    let outcome = write_pid_values(
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
    )
    .await?;

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
    use super::*;
    use async_trait::async_trait;
    use bhtune_core::{
        ControllerType, LoopConfig, LoopTags, ProcessType, TuningResultInvalidReason,
        TuningResultStatus,
    };
    use bhtune_db::models::{TemplateOrigin, TuneDriver, TuneRunRow};
    use bhtune_driver::{
        BrowsePage, BrowsePageRequest, Driver, DriverError, DriverResult, TagId, TagValue,
        TagWrite, WriteOutcome,
    };
    use chrono::Utc;

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
        fn select_response_level(&mut self, _results: &[TuneResultRow]) -> WriteBackSelection {
            self.events.push("select".to_string());
            self.selection.clone()
        }

        fn write_back_selected(&mut self, level: ResponseLevel, requested: bool) {
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
        fn select_response_level(&mut self, _results: &[TuneResultRow]) -> WriteBackSelection {
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
    fn optional_handler_defaults_are_no_ops() {
        let mut handler = DefaultHandler;
        assert!(matches!(
            handler.select_response_level(&[]),
            WriteBackSelection::Skipped(reason) if reason == "no selection"
        ));
        handler.write_back_selected(ResponseLevel::Moderate, true);
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
        let result = maybe_write_back(
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
        .unwrap();
        assert_eq!(result.0, WriteBackOutcome::Failed);
        assert_eq!(handler.events.len(), 2);
        assert_eq!(handler.events[0], "selected:Moderate:true");
        assert!(handler.events[1].starts_with("finished:Moderate:Failed"));
    }
}
