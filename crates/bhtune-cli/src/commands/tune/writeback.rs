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

use crate::output::OutputFormat;

use super::prepare::WriteBackOutcome;
use super::quality::{read_f32, write_value};

/// Reads the existing Proportional/Integral/Derivative values before any write is attempted
/// -- `safety-writeback-rollback`'s pre-read step. Reading all three is a hard stop on the
/// first failure, mirroring findings 4/5's "refuse before mutating" pattern, and has the
/// useful side effect of guaranteeing that a rollback, if one later turns out to be
/// necessary, always has a known-good value to roll back to. `pub(crate)`: also reused by
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
/// failed -- mirroring `restore()`'s "attempt every step independently, don't short-circuit
/// on the first failure" philosophy (`safety-restore-guard`). `targets` is `(label, tag,
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
/// Pre-reads the existing Proportional/Integral/Derivative values, writes and verifies
/// `target` (Proportional then Integral then Derivative, stopping at the first failure),
/// rolls back to the pre-read values on partial failure (only for `kind =
/// `[`WriteKind::Write`]` -- [`WriteKind::Revert`] never does, so a revert can't chase its
/// own failure with a nested rollback; see [`WriteKind`]'s own doc comment), and records
/// exactly one [`TuneWriteRow`] audit row for the attempt, success or not
/// (`safety-writeback-rollback`, finding 6 of the live-plant safety review).
///
/// The one implementation of "pre-read, write, verify, roll back, audit" in the whole
/// workspace, shared by three callers: [`maybe_write_back`]'s in-run write-back,
/// `commands::history::revert`, and `bhtune-server`'s post-hoc `POST /api/runs/{id}/write`/
/// `.../revert` (`api-post-run-write`) -- `pub` (not `pub(crate)`) specifically so that
/// third, different-crate caller can reach it. `bhtune-server` calls only this function, not
/// the lower-level [`read_previous_pid_values`]/[`write_and_verify_pid_value`] helpers this
/// builds on -- those stay `pub(crate)`, since nothing outside `bhtune-cli` needs the
/// individual pre-read/write-single-value steps, only the complete audited sequence.
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
/// Represents the selected calculated PID parameters and any reason they were not written.
///
/// The write-back operation itself is documented on [`maybe_write_back`].
pub(super) enum WriteBackSelection<'a> {
    Selected(&'a TuneResultRow),
    Skipped(String),
    Failed(String),
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
pub(super) fn result_write_back_error(result: &TuneResultRow) -> Option<String> {
    pid_parameters_for_result(result)
        .err()
        .map(|error| error.to_string())
}
pub(super) fn select_named_write_back_result<'a>(
    results: &'a [TuneResultRow],
    level: ResponseLevel,
    output: OutputFormat,
) -> WriteBackSelection<'a> {
    match results.iter().find(|r| r.response_level == level) {
        Some(result) => {
            if let Some(detail) = result_write_back_error(result) {
                if prints_table_output(output) {
                    println!(
                        "Calculated {level:?} result is invalid; skipping write-back: {detail}"
                    );
                }
                return WriteBackSelection::Failed(detail);
            }
            if prints_table_output(output) {
                println!(
                    "Non-interactively writing {level:?} PID parameters back to the DCS (--write-pid)."
                );
            }
            WriteBackSelection::Selected(result)
        }
        None => {
            let detail = format!("no calculated result recorded for response level {level:?}");
            if prints_table_output(output) {
                println!(
                    "No calculated result recorded for response level {level:?}; skipping write-back."
                );
            }
            WriteBackSelection::Failed(detail)
        }
    }
}
pub(super) fn select_interactive_write_back_result<'a>(
    results: &'a [TuneResultRow],
    reader: &mut impl std::io::BufRead,
) -> WriteBackSelection<'a> {
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
        return WriteBackSelection::Skipped(
            "skipped interactively (no selection made)".to_string(),
        );
    }

    match input.parse::<usize>() {
        Ok(n) if n >= 1 && n <= results.len() => {
            let result = &results[n - 1];
            match result_write_back_error(result) {
                Some(detail) => {
                    eprintln!("Selected result is invalid; skipping PID write-back: {detail}");
                    WriteBackSelection::Failed(detail)
                }
                None => WriteBackSelection::Selected(result),
            }
        }
        _ => {
            eprintln!("Invalid selection; skipping PID write-back.");
            WriteBackSelection::Skipped("invalid response level selection".to_string())
        }
    }
}
pub(super) fn select_write_back_result<'a>(
    results: &'a [TuneResultRow],
    write_pid: Option<ResponseLevel>,
    output: OutputFormat,
    reader: &mut impl std::io::BufRead,
) -> WriteBackSelection<'a> {
    match write_pid {
        Some(level) => select_named_write_back_result(results, level, output),
        None if skips_interactive_prompt(write_pid, output) => WriteBackSelection::Skipped(
            "--output json was set without --write-pid; skipped the interactive \
             write-back prompt since there is no human present to answer it"
                .to_string(),
        ),
        None => select_interactive_write_back_result(results, reader),
    }
}
pub(super) fn finish_write_back(
    output: OutputFormat,
    response_level: ResponseLevel,
    outcome: PidWriteOutcome,
) -> (WriteBackOutcome, Option<String>) {
    match outcome {
        PidWriteOutcome::Written => {
            if prints_table_output(output) {
                println!("Wrote and confirmed {response_level:?} PID parameters.");
            }
            (WriteBackOutcome::Written { response_level }, None)
        }
        PidWriteOutcome::Failed { detail } => {
            if prints_table_output(output) {
                println!("PID write-back failed: {detail}");
            }
            (WriteBackOutcome::Failed, Some(detail))
        }
    }
}
/// Writes back the calculated PID parameters for one response level -- chosen either
/// interactively (prompting on `reader`) or non-interactively via `write_pid`
/// (`--write-pid`; the caller has already validated `--yes` was also given before the tune
/// even started). Skips with an informational message (rather than prompting/writing)
/// whenever any of the three PID constant tags is unconfigured — true for the simulator
/// driver, and also a sane guard for any real template missing one — or when no results
/// were recorded at all. `reader` is injected (rather than reading `std::io::stdin()`
/// directly) so tests can supply a fixed `Cursor` in place of the process's real stdin; it
/// is never read from at all when `write_pid` is `Some`, or when `output` is
/// [`OutputFormat::Json`] (see below).
///
/// Pre-reads the existing constants, writes and verifies Proportional then Integral then
/// Derivative in sequence (stopping at the first failure), and rolls back whatever was
/// already confirmed if a later constant fails -- `safety-writeback-rollback` (finding 6).
/// Every attempt, including a pre-read failure or a transport error mid-write, produces
/// exactly one [`TuneWriteRow`] audit row.
///
/// `output` makes this function format-aware (`safety-json-contract`, finding 8):
///
/// - Under [`OutputFormat::Table`], status/result lines print with `println!` exactly as
///   before, and the interactive listing/menu print with `eprintln!` -- a prompt has no
///   business on stdout in *any* format, since a caller piping stdout elsewhere shouldn't
///   see it interleaved with the tune's actual result.
/// - Under [`OutputFormat::Json`], none of those `println!`s fire at all. The reason for
///   every `Skipped`/`Failed` outcome is instead returned as the second element of the
///   tuple -- a human-readable detail string -- so `print_summary`'s JSON branch can fold
///   it into the one JSON object this whole run must still emit on stdout. Without this,
///   the interactive prompt or a plain status line would print ahead of that object and
///   break `--output json` for every scripted/scheduled caller trying to parse stdout.
/// - When `output` is `Json` and `write_pid` is `None` (no response level was named
///   non-interactively), the interactive prompt is skipped entirely -- `reader` (real
///   stdin outside tests) is never touched, since there is no human present to answer it
///   and a scripted caller could otherwise hang waiting on input that will never arrive.
#[allow(clippy::too_many_arguments)]
pub(super) async fn maybe_write_back(
    pool: &SqlitePool,
    run_id: i64,
    tags: &LoopTags,
    template: &DcsTemplate,
    driver: &dyn Driver,
    config: LoopConfig,
    write_pid: Option<ResponseLevel>,
    output: OutputFormat,
    allow_uncertain: bool,
    reader: &mut impl std::io::BufRead,
) -> anyhow::Result<(WriteBackOutcome, Option<String>)> {
    let (Some(p_tag), Some(i_tag), Some(d_tag)) = (
        &tags.proportional_constant,
        &tags.integral_constant,
        &tags.derivative_constant,
    ) else {
        let detail = "no PID constant tags configured for this run's driver/template";
        if prints_table_output(output) {
            println!(
                "No PID constant tags configured for this run's driver/template; skipping write-back."
            );
        }
        return Ok((WriteBackOutcome::Skipped, Some(detail.to_string())));
    };

    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    if results.is_empty() {
        return Ok((
            WriteBackOutcome::Skipped,
            Some("no calculated results were recorded for this run".to_string()),
        ));
    }

    let selected = match select_write_back_result(&results, write_pid, output, reader) {
        WriteBackSelection::Selected(result) => result,
        WriteBackSelection::Skipped(detail) => {
            return Ok((WriteBackOutcome::Skipped, Some(detail)));
        }
        WriteBackSelection::Failed(detail) => {
            return Ok((WriteBackOutcome::Failed, Some(detail)));
        }
    };

    let pid = pid_parameters_for_result(selected)?;
    let response_level = pid.response_level;
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

    Ok(finish_write_back(output, response_level, outcome))
}
pub(super) fn prints_table_output(output: OutputFormat) -> bool {
    matches!(output, OutputFormat::Table)
}
pub(super) fn skips_interactive_prompt(
    write_pid: Option<ResponseLevel>,
    output: OutputFormat,
) -> bool {
    write_pid.is_none() && matches!(output, OutputFormat::Json)
}
