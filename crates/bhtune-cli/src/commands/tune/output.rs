#![allow(rustdoc::broken_intra_doc_links)]

use crate::output::OutputFormat;
use bhtune_core::ResponseLevel;
use bhtune_db::models::TuneResultRow;
pub use bhtune_runtime::tune::TuneOutcome;
use bhtune_runtime::tune::{
    AbortReason, PidWriteOutcome, RunOutcome, WriteBackHandler, WriteBackOutcome,
    WriteBackSelection, WriteBackSkipReason, pid_parameters_for_result, tune_outcome_for_run,
};
use std::io::BufRead;

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
    println!("{}", render_table_summary(run_id, outcome));
}

fn render_table_summary(run_id: i64, outcome: &RunOutcome) -> String {
    match outcome {
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Written { response_level },
            ..
        } => format!(
            "Tune completed successfully (run id {run_id}); wrote {response_level:?} PID parameters."
        ),
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Skipped,
            ..
        } => format!("Tune completed successfully (run id {run_id})."),
        RunOutcome::Completed {
            write_back: WriteBackOutcome::Failed,
            ..
        } => format!(
            "Tune completed successfully (run id {run_id}), but PID write-back failed; the loop was left with its previous PID constants."
        ),
        RunOutcome::Aborted(AbortReason::UserInterrupt) => {
            "Tune aborted (Ctrl+C received; loop restored).".to_string()
        }
        RunOutcome::Aborted(AbortReason::Timeout { timeout_secs }) => {
            format!(
                "Tune aborted: exceeded the {timeout_secs}s [tuning].timeout_secs limit before completing; loop restored."
            )
        }
        RunOutcome::Aborted(AbortReason::OperationTimedOut {
            tag,
            op_timeout_secs,
        }) => {
            format!(
                "Tune aborted: tag '{tag}' did not respond within the {op_timeout_secs}s [tuning].op_timeout_secs limit; loop restored."
            )
        }
        RunOutcome::Aborted(AbortReason::PoorQuality { tag, quality }) => {
            format!(
                "Tune aborted: tag '{tag}' reported OPC quality {quality:?} during polling; loop restored."
            )
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
            format!(
                "Tune aborted: MV tag '{tag}' did not confirm target {target} (readback {readback}, tolerance {tolerance}) after {:.3}s; the confirmation deadline was {deadline_secs}s. Loop restored.",
                *elapsed_ms as f64 / 1_000.0
            )
        }
        RunOutcome::RestoreIncomplete { reason } => {
            format!(
                "Tune ended, but the loop's restore could not be confirmed ({reason}). Check the loop by hand -- see the warning above for the tag and value to check."
            )
        }
    }
}
pub(super) fn print_json_summary(run_id: i64, outcome: &RunOutcome, tune_outcome: TuneOutcome) {
    let json = build_json_summary(run_id, outcome, tune_outcome);
    println!(
        "{}",
        render_json_summary(&json, serde_json::to_string_pretty)
    );
}

fn build_json_summary(
    run_id: i64,
    outcome: &RunOutcome,
    tune_outcome: TuneOutcome,
) -> serde_json::Value {
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
    serde_json::json!({
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
    })
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

pub(super) struct CliWriteBackHandler {
    output: OutputFormat,
    requested_level: Option<ResponseLevel>,
    selection_failure_reported: bool,
}

impl CliWriteBackHandler {
    pub(super) fn new(output: OutputFormat, requested_level: Option<ResponseLevel>) -> Self {
        Self {
            output,
            requested_level,
            selection_failure_reported: false,
        }
    }
}

impl WriteBackHandler for CliWriteBackHandler {
    fn select_response_level(&mut self, results: &[TuneResultRow]) -> WriteBackSelection {
        if self.output == OutputFormat::Json {
            return WriteBackSelection::Skipped(
                "--output json was set without --write-pid; skipped the interactive \
                 write-back prompt since there is no human present to answer it"
                    .to_string(),
            );
        }

        let stdin = std::io::stdin();
        let mut reader = stdin.lock();
        let (selection, reported_failure) =
            select_interactive_write_back_result(results, &mut reader);
        self.selection_failure_reported = reported_failure;
        selection
    }

    fn write_back_selected(&mut self, response_level: ResponseLevel, requested: bool) {
        if self.output == OutputFormat::Table && requested {
            println!(
                "Non-interactively writing {response_level:?} PID parameters back to the DCS (--write-pid)."
            );
        }
    }

    fn write_back_skipped(&mut self, reason: WriteBackSkipReason) {
        if self.output == OutputFormat::Table && reason == WriteBackSkipReason::NoPidTags {
            println!(
                "No PID constant tags configured for this run's driver/template; skipping write-back."
            );
        }
    }

    fn write_back_failed(&mut self, detail: &str) {
        if self.output != OutputFormat::Table {
            return;
        }
        if self.selection_failure_reported {
            self.selection_failure_reported = false;
            return;
        }

        match self.requested_level {
            Some(level) if detail.starts_with(&format!("{level:?} calculated result")) => {
                println!("Calculated {level:?} result is invalid; skipping write-back: {detail}");
            }
            Some(level)
                if detail
                    == format!("no calculated result recorded for response level {level:?}") =>
            {
                println!(
                    "No calculated result recorded for response level {level:?}; skipping write-back."
                );
            }
            _ => println!("PID write-back failed: {detail}"),
        }
    }

    fn write_back_finished(&mut self, response_level: ResponseLevel, outcome: &PidWriteOutcome) {
        if self.output != OutputFormat::Table {
            return;
        }
        match outcome {
            PidWriteOutcome::Written => {
                println!("Wrote and confirmed {response_level:?} PID parameters.");
            }
            PidWriteOutcome::Failed { detail } => {
                println!("PID write-back failed: {detail}");
            }
        }
    }
}

fn select_interactive_write_back_result(
    results: &[TuneResultRow],
    reader: &mut impl BufRead,
) -> (WriteBackSelection, bool) {
    eprintln!("\nCalculated PID parameters:");
    for (i, result) in results.iter().enumerate() {
        match pid_parameters_for_result(result) {
            Ok(pid) => eprintln!(
                "  {}. {:?}: P={:.4} I={:.4} D={:.4}",
                i + 1,
                result.response_level,
                pid.proportional,
                pid.integral,
                pid.derivative
            ),
            Err(error) => eprintln!(
                "  {}. {:?}: INVALID ({error})",
                i + 1,
                result.response_level
            ),
        }
    }
    eprintln!(
        "Write which response level's PID parameters back to the DCS? [1-{}, or Enter/n to skip]:",
        results.len()
    );

    let mut input = String::new();
    let bytes_read = reader.read_line(&mut input).unwrap_or(0);
    let input = input.trim();
    if bytes_read == 0 || input.is_empty() || input.eq_ignore_ascii_case("n") {
        eprintln!("Skipping PID write-back.");
        return (
            WriteBackSelection::Skipped("skipped interactively (no selection made)".to_string()),
            false,
        );
    }

    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= results.len() => match pid_parameters_for_result(&results[n - 1]) {
            Ok(pid) => (WriteBackSelection::Selected(pid.response_level), false),
            Err(error) => {
                let detail = error.to_string();
                eprintln!("Selected result is invalid; skipping PID write-back: {detail}");
                (WriteBackSelection::Failed(detail), true)
            }
        },
        _ => {
            eprintln!("Invalid selection; skipping PID write-back.");
            (
                WriteBackSelection::Skipped("invalid response level selection".to_string()),
                false,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{TuningResultInvalidReason, TuningResultStatus};
    use bhtune_driver::Quality;
    use std::io::Cursor;

    fn valid_result(response_level: ResponseLevel) -> TuneResultRow {
        TuneResultRow {
            id: 0,
            run_id: 1,
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

    fn invalid_result(response_level: ResponseLevel) -> TuneResultRow {
        TuneResultRow {
            status: TuningResultStatus::Invalid,
            invalid_reason: Some(TuningResultInvalidReason::NonPositivePvAmplitude),
            proportional: None,
            integral: None,
            derivative: None,
            ..valid_result(response_level)
        }
    }

    fn summary_cases() -> Vec<(RunOutcome, &'static str)> {
        vec![
            (
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Written {
                        response_level: ResponseLevel::Moderate,
                    },
                    write_back_detail: None,
                },
                "Tune completed successfully (run id 42); wrote Moderate PID parameters.",
            ),
            (
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped,
                    write_back_detail: Some("no selection".to_string()),
                },
                "Tune completed successfully (run id 42).",
            ),
            (
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed,
                    write_back_detail: Some("write rejected".to_string()),
                },
                "Tune completed successfully (run id 42), but PID write-back failed; the loop was left with its previous PID constants.",
            ),
            (
                RunOutcome::Aborted(AbortReason::UserInterrupt),
                "Tune aborted (Ctrl+C received; loop restored).",
            ),
            (
                RunOutcome::Aborted(AbortReason::Timeout { timeout_secs: 10 }),
                "Tune aborted: exceeded the 10s [tuning].timeout_secs limit before completing; loop restored.",
            ),
            (
                RunOutcome::Aborted(AbortReason::OperationTimedOut {
                    tag: "Unit1.LIC101.PV".to_string(),
                    op_timeout_secs: 2,
                }),
                "Tune aborted: tag 'Unit1.LIC101.PV' did not respond within the 2s [tuning].op_timeout_secs limit; loop restored.",
            ),
            (
                RunOutcome::Aborted(AbortReason::PoorQuality {
                    tag: "Unit1.LIC101.PV".to_string(),
                    quality: Quality::Bad,
                }),
                "Tune aborted: tag 'Unit1.LIC101.PV' reported OPC quality Bad during polling; loop restored.",
            ),
            (
                RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                    tag: "Unit1.LIC101.MV".to_string(),
                    target: 50.0,
                    readback: Some(49.0),
                    tolerance: 0.5,
                    elapsed_ms: 1_500,
                    deadline_secs: 4,
                }),
                "Tune aborted: MV tag 'Unit1.LIC101.MV' did not confirm target 50 (readback 49, tolerance 0.5) after 1.500s; the confirmation deadline was 4s. Loop restored.",
            ),
            (
                RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                    tag: "Unit1.LIC101.MV".to_string(),
                    target: 50.0,
                    readback: None,
                    tolerance: 0.5,
                    elapsed_ms: 4_000,
                    deadline_secs: 4,
                }),
                "Tune aborted: MV tag 'Unit1.LIC101.MV' did not confirm target 50 (readback unavailable, tolerance 0.5) after 4.000s; the confirmation deadline was 4s. Loop restored.",
            ),
            (
                RunOutcome::RestoreIncomplete {
                    reason: "MV restore not confirmed".to_string(),
                },
                "Tune ended, but the loop's restore could not be confirmed (MV restore not confirmed). Check the loop by hand -- see the warning above for the tag and value to check.",
            ),
        ]
    }

    #[test]
    fn json_summary_renderer_has_a_displayable_fallback_for_an_encoding_failure() {
        let rendered = render_json_summary(&serde_json::json!({"run_id": 1}), |_| {
            Err::<String, _>("injected encoding failure")
        });
        assert_eq!(rendered, r#"{"error": "injected encoding failure"}"#);
    }

    #[test]
    fn summary_renderers_preserve_all_cli_outcome_messages_and_statuses() {
        for (outcome, expected_table) in summary_cases() {
            let expected_status = tune_outcome_for_run(&outcome);
            assert_eq!(render_table_summary(42, &outcome), expected_table);
            assert_eq!(
                print_summary(42, &outcome, OutputFormat::Table),
                expected_status
            );
            assert_eq!(
                print_summary(42, &outcome, OutputFormat::Json),
                expected_status
            );
        }
    }

    #[test]
    fn json_summary_preserves_the_machine_readable_failure_details() {
        let cases = summary_cases();
        for (outcome, _) in &cases {
            let tune_outcome = tune_outcome_for_run(outcome);
            let json = build_json_summary(42, outcome, tune_outcome);
            assert_eq!(json["run_id"], 42);
            assert_eq!(json["outcome"], tune_outcome.label());
        }

        let timeout = build_json_summary(
            42,
            &RunOutcome::Aborted(AbortReason::Timeout { timeout_secs: 10 }),
            TuneOutcome::TimedOut,
        );
        assert_eq!(timeout["timeout_secs"], 10);

        let quality = build_json_summary(
            42,
            &RunOutcome::Aborted(AbortReason::PoorQuality {
                tag: "Unit1.LIC101.PV".to_string(),
                quality: Quality::Bad,
            }),
            TuneOutcome::PoorQuality,
        );
        assert_eq!(quality["poor_quality_tag"], "Unit1.LIC101.PV");
        assert_eq!(quality["poor_quality"], "bad");

        let operation_timeout = build_json_summary(
            42,
            &RunOutcome::Aborted(AbortReason::OperationTimedOut {
                tag: "Unit1.LIC101.PV".to_string(),
                op_timeout_secs: 2,
            }),
            TuneOutcome::TimedOut,
        );
        assert_eq!(operation_timeout["op_timeout_tag"], "Unit1.LIC101.PV");
        assert_eq!(operation_timeout["op_timeout_secs"], 2);

        let actuation = build_json_summary(
            42,
            &RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                tag: "Unit1.LIC101.MV".to_string(),
                target: 50.0,
                readback: Some(49.0),
                tolerance: 0.5,
                elapsed_ms: 1_500,
                deadline_secs: 4,
            }),
            TuneOutcome::ActuationFailed,
        );
        assert_eq!(
            actuation["mv_actuation"],
            serde_json::json!({
                "tag": "Unit1.LIC101.MV",
                "target": 50.0,
                "readback": 49.0,
                "tolerance": 0.5,
                "elapsed_ms": 1_500,
                "deadline_secs": 4,
            })
        );

        let restore = build_json_summary(
            42,
            &RunOutcome::RestoreIncomplete {
                reason: "MV restore not confirmed".to_string(),
            },
            TuneOutcome::RestoreIncomplete,
        );
        assert_eq!(
            restore["restore_incomplete_reason"],
            "MV restore not confirmed"
        );
    }

    #[test]
    fn interactive_write_back_selection_skips_on_empty_input() {
        let mut input = Cursor::new(b"\n");
        let (selection, failure_reported) = select_interactive_write_back_result(&[], &mut input);
        assert_eq!(
            selection,
            WriteBackSelection::Skipped("skipped interactively (no selection made)".to_string())
        );
        assert!(!failure_reported);
    }

    #[test]
    fn interactive_write_back_selection_rejects_out_of_range_input() {
        let mut input = Cursor::new(b"1\n");
        let (selection, failure_reported) = select_interactive_write_back_result(&[], &mut input);
        assert_eq!(
            selection,
            WriteBackSelection::Skipped("invalid response level selection".to_string())
        );
        assert!(!failure_reported);
    }

    #[test]
    fn interactive_write_back_selection_handles_valid_and_invalid_results() {
        let mut input = Cursor::new(b"1\n");
        let (selection, failure_reported) = select_interactive_write_back_result(
            &[valid_result(ResponseLevel::Moderate)],
            &mut input,
        );
        assert_eq!(
            selection,
            WriteBackSelection::Selected(ResponseLevel::Moderate)
        );
        assert!(!failure_reported);

        let mut input = Cursor::new(b"1\n");
        let (selection, failure_reported) = select_interactive_write_back_result(
            &[invalid_result(ResponseLevel::Moderate)],
            &mut input,
        );
        assert!(matches!(selection, WriteBackSelection::Failed(_)));
        assert!(failure_reported);
    }

    #[test]
    fn cli_write_back_handler_keeps_terminal_and_json_policies_separate() {
        let mut json = CliWriteBackHandler::new(OutputFormat::Json, None);
        assert!(matches!(
            json.select_response_level(&[]),
            WriteBackSelection::Skipped(detail) if detail.contains("--output json")
        ));
        json.write_back_selected(ResponseLevel::Moderate, true);
        json.write_back_skipped(WriteBackSkipReason::NoPidTags);
        json.write_back_skipped(WriteBackSkipReason::NoResults);
        json.write_back_failed("write failed");
        json.write_back_finished(ResponseLevel::Moderate, &PidWriteOutcome::Written);

        let mut table =
            CliWriteBackHandler::new(OutputFormat::Table, Some(ResponseLevel::Moderate));
        assert!(matches!(
            table.select_response_level(&[]),
            WriteBackSelection::Skipped(_)
        ));
        table.write_back_selected(ResponseLevel::Moderate, true);
        table.write_back_selected(ResponseLevel::Moderate, false);
        table.write_back_skipped(WriteBackSkipReason::NoPidTags);
        table.write_back_skipped(WriteBackSkipReason::NoResults);
        table.write_back_failed("Moderate calculated result is invalid");
        table.write_back_failed("no calculated result recorded for response level Moderate");
        table.write_back_failed("gateway rejected the write");
        table.selection_failure_reported = true;
        table.write_back_failed("selected result is invalid");
        assert!(!table.selection_failure_reported);
        table.write_back_finished(ResponseLevel::Moderate, &PidWriteOutcome::Written);
        table.write_back_finished(
            ResponseLevel::Moderate,
            &PidWriteOutcome::Failed {
                detail: "write failed".to_string(),
            },
        );

        let mut table_without_request = CliWriteBackHandler::new(OutputFormat::Table, None);
        table_without_request.write_back_failed("gateway rejected the write");
    }
}
