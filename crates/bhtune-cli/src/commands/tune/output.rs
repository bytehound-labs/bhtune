#![allow(rustdoc::broken_intra_doc_links)]

use crate::output::OutputFormat;

use super::prepare::{AbortReason, RunOutcome, WriteBackOutcome};

/// The final disposition of a `tune`/`simulate` run -- drives the printed summary (see
/// [`print_summary`]) and, via `crate::tune_outcome_exit_code` in `lib.rs`, the process's
/// exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneOutcome {
    /// The test completed. Either no write-back was requested/possible/attempted, or a
    /// requested write-back succeeded.
    Completed,
    /// The user pressed Ctrl+C; the loop was restored to its original mode/setpoint before
    /// returning.
    Aborted,
    /// `[tuning].timeout_secs` elapsed before the engine reported completion; the loop was
    /// restored to its original mode/setpoint before returning, exactly like
    /// [`TuneOutcome::Aborted`]
    /// but distinguished so a scheduler's alerting can tell "this run had to be killed for
    /// running too long" apart from "an operator stopped it on purpose".
    TimedOut,
    /// A driver reported a non-`Good` OPC quality for a tuning-critical reading -- an
    /// initial reading (including the setpoint capture, when the loop starts in Auto) or an
    /// in-flight PV poll sample when the global Config > OPC quality policy rejects
    /// `Uncertain` (or with the policy enabled, but the quality was
    /// `Bad` rather than merely `Uncertain`) -- and the run was aborted and the
    /// loop restored before returning, exactly like
    /// [`TuneOutcome::Aborted`]/[`TuneOutcome::TimedOut`] but distinguished so a scheduler's
    /// alerting can tell "the plant data itself couldn't be trusted" apart from either of
    /// those. See `safety-quality` in AGENTS.md.
    PoorQuality,
    /// An accepted OPC DA MV command could not be confirmed before its deadline or before
    /// the engine requested a replacement relay command. The loop was restored before
    /// returning unless [`TuneOutcome::RestoreIncomplete`] took precedence.
    ActuationFailed,
    /// The test itself completed, but writing the chosen PID parameters back to the DCS
    /// failed (rejected write, failed confirmation readback, or -- defensively -- a
    /// `--write-pid` level with no matching calculated result).
    WriteBackFailed,
    /// The run ended (via normal completion, Ctrl+C, or a timeout) without being able to
    /// confirm the loop was fully restored to its pre-test mode/MV/setpoint -- a second
    /// Ctrl+C arrived while the restore was in flight, or
    /// `[tuning].restore_timeout_secs` elapsed first. The loop may still be sitting at a
    /// relay-test MV/mode; an operator must check it by hand using the tag/value named in the
    /// warning printed to stderr. See
    /// `safety-cancellation` in AGENTS.md.
    RestoreIncomplete,
}
impl TuneOutcome {
    /// A short machine-readable label, used in the `--output json` summary.
    pub fn label(self) -> &'static str {
        match self {
            TuneOutcome::Completed => "completed",
            TuneOutcome::Aborted => "aborted",
            TuneOutcome::TimedOut => "timed_out",
            TuneOutcome::PoorQuality => "poor_quality",
            TuneOutcome::ActuationFailed => "actuation_failed",
            TuneOutcome::WriteBackFailed => "write_back_failed",
            TuneOutcome::RestoreIncomplete => "restore_incomplete",
        }
    }
}
/// Maps one run's full outcome down to the coarser [`TuneOutcome`] that drives the process
/// exit code -- a write-back failure demotes an otherwise-successful test completion to
/// [`TuneOutcome::WriteBackFailed`], since an unattended caller needs to know the loop was
/// left with its *old* PID constants, not the newly calculated ones.
pub(super) fn tune_outcome_for_run(outcome: &RunOutcome) -> TuneOutcome {
    match outcome {
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Failed,
            ..
        } => TuneOutcome::WriteBackFailed,
        RunOutcome::Completed { .. } => TuneOutcome::Completed,
        RunOutcome::Aborted(AbortReason::UserInterrupt) => TuneOutcome::Aborted,
        RunOutcome::Aborted(AbortReason::Timeout { .. }) => TuneOutcome::TimedOut,
        RunOutcome::Aborted(AbortReason::OperationTimedOut { .. }) => TuneOutcome::TimedOut,
        RunOutcome::Aborted(AbortReason::PoorQuality { .. }) => TuneOutcome::PoorQuality,
        RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. }) => {
            TuneOutcome::ActuationFailed
        }
        RunOutcome::RestoreIncomplete { .. } => TuneOutcome::RestoreIncomplete,
    }
}
pub(super) fn format_mv_actuation_abort_reason(reason: &AbortReason) -> String {
    let AbortReason::MvActuationUnconfirmed {
        tag,
        target,
        readback,
        tolerance,
        elapsed_ms,
        deadline_secs,
    } = reason
    else {
        return format!("{reason:?}");
    };
    let readback = readback
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    format!(
        "MV actuation unconfirmed: tag '{tag}', target {target}, readback {readback}, tolerance {tolerance}, elapsed {elapsed_ms} ms, deadline {deadline_secs} s"
    )
}
/// Prints this run's final outcome line -- either the plain-text shape or a `--output json`
/// object -- and returns the [`TuneOutcome`] the caller should propagate as the process's
/// exit code.
pub(super) fn print_summary(
    run_id: i64,
    outcome: &RunOutcome,
    output: OutputFormat,
) -> TuneOutcome {
    let tune_outcome = tune_outcome_for_run(outcome);
    match output {
        OutputFormat::Table => print_table_summary(run_id, outcome),
        OutputFormat::Json => print_json_summary(run_id, outcome, tune_outcome),
    }
    tune_outcome
}
pub(super) fn print_table_summary(run_id: i64, outcome: &RunOutcome) {
    match outcome {
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Written { response_level },
            ..
        } => {
            println!(
                "Tune completed successfully (run id {run_id}); wrote {response_level:?} PID parameters."
            );
        }
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Skipped,
            ..
        } => {
            println!("Tune completed successfully (run id {run_id}).");
        }
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Failed,
            ..
        } => {
            println!(
                "Tune completed successfully (run id {run_id}), but PID write-back failed; the loop was left with its previous PID constants."
            );
        }
        RunOutcome::Aborted(AbortReason::UserInterrupt) => {
            println!("Tune aborted (Ctrl+C received; loop restored).");
        }
        RunOutcome::Aborted(AbortReason::Timeout { timeout_secs }) => {
            println!(
                "Tune aborted: exceeded the {timeout_secs}s [tuning].timeout_secs limit before completing; loop restored."
            );
        }
        RunOutcome::Aborted(AbortReason::OperationTimedOut {
            tag,
            op_timeout_secs,
        }) => {
            println!(
                "Tune aborted: tag '{tag}' did not respond within the {op_timeout_secs}s [tuning].op_timeout_secs limit; loop restored."
            );
        }
        RunOutcome::Aborted(AbortReason::PoorQuality { tag, quality }) => {
            println!(
                "Tune aborted: tag '{tag}' reported OPC quality {quality:?} during polling; loop restored."
            );
        }
        RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
            tag,
            target,
            readback,
            tolerance,
            elapsed_ms,
            deadline_secs,
        }) => {
            let readback = readback
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unavailable".to_string());
            println!(
                "Tune aborted: MV tag '{tag}' did not confirm target {target} (readback {readback}, tolerance {tolerance}) after {:.3}s; the confirmation deadline was {deadline_secs}s. Loop restored.",
                *elapsed_ms as f64 / 1_000.0
            );
        }
        RunOutcome::RestoreIncomplete { reason } => {
            println!(
                "Tune ended, but the loop's restore could not be confirmed ({reason}). Check the loop by hand -- see the warning above for the tag and value to check."
            );
        }
    }
}
pub(super) fn print_json_summary(run_id: i64, outcome: &RunOutcome, tune_outcome: TuneOutcome) {
    let (write_back, response_level) = match outcome {
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Written { response_level },
            ..
        } => ("written", Some(*response_level)),
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Skipped,
            ..
        } => ("skipped", None),
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Failed,
            ..
        } => ("failed", None),
        RunOutcome::Aborted(_) => ("not_attempted", None),
        RunOutcome::RestoreIncomplete { .. } => ("not_attempted", None),
    };
    let write_back_detail = match outcome {
        RunOutcome::Completed {
            write_back_detail, ..
        } => write_back_detail.clone(),
        _ => None,
    };
    let timeout_secs = match outcome {
        RunOutcome::Aborted(AbortReason::Timeout { timeout_secs }) => Some(*timeout_secs),
        _ => None,
    };
    let (poor_quality_tag, poor_quality) = match outcome {
        RunOutcome::Aborted(AbortReason::PoorQuality { tag, quality }) => (
            Some(tag.clone()),
            Some(format!("{quality:?}").to_lowercase()),
        ),
        _ => (None, None),
    };
    let (op_timeout_tag, op_timeout_secs) = match outcome {
        RunOutcome::Aborted(AbortReason::OperationTimedOut {
            tag,
            op_timeout_secs,
        }) => (Some(tag.clone()), Some(*op_timeout_secs)),
        _ => (None, None),
    };
    let restore_incomplete_reason = match outcome {
        RunOutcome::RestoreIncomplete { reason } => Some(reason.clone()),
        _ => None,
    };
    let actuation = match outcome {
        RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
            tag,
            target,
            readback,
            tolerance,
            elapsed_ms,
            deadline_secs,
        }) => Some(serde_json::json!({
            "tag": tag,
            "target": target,
            "readback": readback,
            "tolerance": tolerance,
            "elapsed_ms": elapsed_ms,
            "deadline_secs": deadline_secs,
        })),
        _ => None,
    };
    let json = serde_json::json!({
        "run_id": run_id,
        "outcome": tune_outcome.label(),
        "write_back": write_back,
        "write_back_response_level": response_level,
        "write_back_detail": write_back_detail,
        "timeout_secs": timeout_secs,
        "poor_quality_tag": poor_quality_tag,
        "poor_quality": poor_quality,
        "op_timeout_tag": op_timeout_tag,
        "op_timeout_secs": op_timeout_secs,
        "mv_actuation": actuation,
        "restore_incomplete_reason": restore_incomplete_reason,
    });
    println!(
        "{}",
        render_json_summary(&json, serde_json::to_string_pretty)
    );
}
pub(super) fn render_json_summary<E>(
    json: &serde_json::Value,
    serialize: impl FnOnce(&serde_json::Value) -> Result<String, E>,
) -> String
where
    E: std::fmt::Display,
{
    serialize(json).unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
}
