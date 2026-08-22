//! `bhtune tune`/`bhtune simulate`: runs a full MRFT test end-to-end — resolving a template,
//! deriving tags, transitioning the loop to Manual, polling the driver and driving a real
//! [`bhtune_core::MrftEngine`], persisting every tick and the final calculated results, then
//! restoring the loop and optionally writing back the chosen PID constants.
//!
//! Mirrors the legacy `MRFTstart`/`ReadInitialOPCvalues`/`ChangeControllerModeToMan`/
//! `ResetOPC` sequence from `OPCClass.cs`. The mode-transition and write-back steps
//! automatically no-op for the simulator driver, since its [`bhtune_core::LoopTags`] has no
//! setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags` below) — no
//! separate "is this the simulator?" branching is needed in that logic.

use std::future::Future;
use std::time::Duration;

use bhtune_core::{
    Action, ControllerDirection, ControllerType, DcsTemplate, InitialReadings, LoopConfig,
    LoopTags, MrftCompat, MrftEngine, MvRange, PidParameters, ProcessType, PvRange, ResponseLevel,
    TagOrValue, TagOverrides, Tick, TuningMathCompat, calculate_all, lookup, opc_write_values,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    DcsTemplateRow, NewTuneWrite, RollbackState, SampleQuality, TuneDriver, TuneResultRow,
    TuneRunInitialReadings, TuneRunRow, TuneSampleRow, TuneWriteRow, WriteKind, WriteReadback,
};
use bhtune_driver::{Driver, TagWrite};
use chrono::{DateTime, Utc};

use crate::args::{DriverKindArg, TuneArgs};
use crate::cancel::CtrlC;
use crate::driver::{SIMULATOR_MV_TAG, SIMULATOR_PV_TAG};
use crate::output::OutputFormat;

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
    /// `--timeout-secs` elapsed before the engine reported completion; the loop was restored
    /// to its original mode/setpoint before returning, exactly like [`TuneOutcome::Aborted`]
    /// but distinguished so a scheduler's alerting can tell "this run had to be killed for
    /// running too long" apart from "an operator stopped it on purpose".
    TimedOut,
    /// A driver reported a non-`Good` OPC quality for a tuning-critical reading -- an
    /// initial reading (including the setpoint capture, when the loop starts in Auto) or an
    /// in-flight PV poll sample without `--allow-uncertain-quality` (or with it, but the
    /// quality was `Bad` rather than merely `Uncertain`) -- and the run was aborted and the
    /// loop restored before returning, exactly like
    /// [`TuneOutcome::Aborted`]/[`TuneOutcome::TimedOut`] but distinguished so a scheduler's
    /// alerting can tell "the plant data itself couldn't be trusted" apart from either of
    /// those. See `safety-quality` in AGENTS.md.
    PoorQuality,
    /// The test itself completed, but writing the chosen PID parameters back to the DCS
    /// failed (rejected write, failed confirmation readback, or -- defensively -- a
    /// `--write-pid` level with no matching calculated result).
    WriteBackFailed,
    /// The run ended (via normal completion, Ctrl+C, or a timeout) without being able to
    /// confirm the loop was fully restored to its pre-test mode/MV/setpoint -- a second
    /// Ctrl+C arrived while the restore was in flight, or `--restore-timeout-secs` elapsed
    /// first. The loop may still be sitting at a relay-test MV/mode; an operator must check
    /// it by hand using the tag/value named in the warning printed to stderr. See
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
            TuneOutcome::WriteBackFailed => "write_back_failed",
            TuneOutcome::RestoreIncomplete => "restore_incomplete",
        }
    }
}

/// Runs one full tune. Never returns `Err` for a tune that simply didn't complete
/// successfully (a failed/aborted run is recorded in the database and reported to stdout);
/// `Err` is reserved for setup problems (unknown template, invalid flag combination,
/// database errors) surfaced directly to the caller.
///
/// Test-facing entry point: delegates to [`run_with_ctrl_c`] with a [`CtrlC::never`] handle,
/// so this crate's large existing test suite never installs a real process-wide signal
/// handler -- see `cancel`'s module doc comment for why that matters. `#[cfg(test)]`-gated
/// (rather than merely unused outside tests) because it depends on [`CtrlC::never`], itself
/// only defined for test builds. Real dispatch (`lib.rs::run_with_cli_and_ctrl_c`) calls
/// [`run_with_ctrl_c`] directly with a real, installed [`CtrlC`] instead of going through
/// this wrapper.
#[cfg(test)]
pub async fn run(
    pool: &SqlitePool,
    args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
) -> anyhow::Result<TuneOutcome> {
    run_with_ctrl_c(pool, args, app_config, &mut CtrlC::never()).await
}

/// Resolves `args.bridge_host` (always) and `args.server` (only for the `opcda` driver,
/// since the simulator driver has no OPC server concept at all) through `app_config`'s
/// `CLI > env > config file > default` precedence before anything else runs, so every
/// downstream consumer (`crate::driver::build`, mainly) can keep reading the plain
/// `TuneArgs` fields it always has.
///
/// `ctrl_c` is threaded through to [`execute`] (and, from there, to every driver await in
/// [`run_polling_loop`] and the final restore), so a Ctrl+C delivered at any point during
/// the run -- not just while idle between polls -- is observed. See `safety-cancellation`
/// in AGENTS.md.
pub(crate) async fn run_with_ctrl_c(
    pool: &SqlitePool,
    args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<TuneOutcome> {
    let prepared = prepare(pool, args, app_config).await?;
    let PreparedTune {
        run_id,
        args,
        template,
        tags,
        driver,
        config,
        started_at,
        write_pid,
    } = prepared;

    let outcome = execute(
        pool,
        run_id,
        &args,
        &template,
        &tags,
        driver.as_ref(),
        config,
        started_at,
        write_pid,
        ctrl_c,
        &mut std::io::stdin().lock(),
    )
    .await;

    match outcome {
        Ok(run_outcome) => {
            let tune_outcome = print_summary(run_id, &run_outcome, args.output);
            let outcome_label = tune_outcome.label();
            tracing::info!(run_id, outcome = outcome_label, "tune run finished");
            Ok(tune_outcome)
        }
        Err(e) => {
            tracing::error!(run_id, error = %e, "tune run failed");
            TuneRunRow::fail(pool, run_id, Utc::now(), &e.to_string())
                .await
                .ok();
            Err(e)
        }
    }
}

/// Everything [`prepare`] resolves before a tune's long-running polling phase can start:
/// the already-validated/defaulted [`TuneArgs`], the resolved template and derived tags, a
/// connected driver, the built [`LoopConfig`], the run's start time, and the response level
/// (if any) to write back at the end -- plus the `tune_runs` row's assigned id.
///
/// Exists to split [`run_with_ctrl_c`]'s single monolithic body into a fast, synchronous-ish
/// setup phase (this struct's construction: template lookup, tag derivation, driver
/// connect, and the `tune_runs` insert that assigns [`PreparedTune::run_id`]) and a
/// long-running phase ([`drive`]/[`execute`]'s actual polling loop, potentially minutes
/// long) -- so an HTTP caller (`bhtune-server`'s `POST /api/runs`) can run the first phase
/// inline in its request handler (fast enough to await directly, and any failure here -- bad
/// template name, unreachable driver -- is exactly the kind of problem an HTTP client
/// expects a synchronous error response for) and `tokio::spawn` the second, returning the
/// assigned `run_id` immediately rather than blocking the HTTP response for the whole test.
///
/// Every field but `run_id` is private: a caller that isn't this module has no legitimate
/// reason to inspect a template/tags/driver/config mid-flight, only to hand the whole
/// prepared bundle to [`drive`] (or, internally, [`run_with_ctrl_c`]) unchanged.
pub struct PreparedTune {
    run_id: i64,
    args: TuneArgs,
    template: DcsTemplate,
    tags: LoopTags,
    driver: Box<dyn Driver>,
    config: LoopConfig,
    started_at: DateTime<Utc>,
    write_pid: Option<ResponseLevel>,
}

impl PreparedTune {
    /// The `tune_runs` row id assigned to this run -- returned to an HTTP caller immediately
    /// (before the run has necessarily finished, or even started polling) so it can be used
    /// to poll `GET /api/runs/{id}` or issue `POST /api/runs/{id}/cancel`.
    pub fn run_id(&self) -> i64 {
        self.run_id
    }
}

/// The shape persisted into `tune_runs.request_json` (`db-run-request-snapshot`).
/// Deliberately mirrors `bhtune-server`'s `StartRunRequest` DTO field-for-field -- core
/// types, not this crate's clap-facing `*Arg` wrapper enums -- so a CLI-originated and an
/// HTTP-originated run produce byte-identical JSON snapshots with no duplicated
/// construction logic between the two crates (`bhtune-server` can't reuse this struct
/// directly, since `bhtune-cli` doesn't depend on it, but the two shapes are kept in sync by
/// convention the same way `StartRunRequest::into_tune_args` already keeps its own field
/// list in sync with [`TuneArgs`]).
///
/// Built from `args` *before* [`prepare`]'s own `bridge_host`/`server` resolution mutates
/// them, so a field left unset by the caller stays absent here rather than silently baking
/// in a resolved default -- this is what lets `ui-prefill-last-run` show blanks where the
/// user relied on a default, instead of freezing yesterday's resolved values into today's
/// form. `output` is deliberately excluded: it's a CLI/HTTP-transport concern with no
/// meaning as a "setting" to remember or duplicate.
#[derive(serde::Serialize)]
struct RequestSnapshot<'a> {
    tagname: &'a str,
    template: &'a str,
    process_type: ProcessType,
    controller_type: ControllerType,
    relay_amp: f32,
    cycles_skip: Option<u32>,
    cycles_count: Option<u32>,
    noise_protection_secs: Option<u32>,
    mrft_delay: u32,
    driver: TuneDriver,
    bridge_host: Option<&'a str>,
    server: Option<&'a str>,
    sim_gain: f32,
    sim_tau: f32,
    sim_dead_time: f32,
    sim_noise: f32,
    sim_seed: u64,
    sim_initial_pv: f32,
    sim_initial_mv: f32,
    pv_range_high: Option<f32>,
    pv_range_low: Option<f32>,
    mv_range_high: Option<f32>,
    mv_range_low: Option<f32>,
    direction: Option<ControllerDirection>,
    tag_overrides: Option<&'a TagOverrides>,
    poll_interval_ms: u64,
    timeout_secs: u64,
    notes: Option<&'a str>,
    yes: bool,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    op_timeout_secs: u64,
    restore_timeout_secs: u64,
}

/// The fast setup phase shared by [`run_with_ctrl_c`] (the CLI's entry point) and [`drive`]
/// (the entry point for a caller -- `bhtune-server` -- that needs to start a run and return
/// control to its own caller before the run finishes). See [`PreparedTune`]'s doc comment
/// for why this split exists.
///
/// Identical in behavior to what `run_with_ctrl_c` did inline before this split: the
/// `--write-pid`-without-`--yes` guard, `bridge_host`/`server` resolution, template lookup,
/// `LoopConfig`/`LoopTags` construction, driver connection, and the `tune_runs` insert all
/// run in exactly the same order against exactly the same inputs. Extracting this into its
/// own function changes nothing about what runs or when -- only who else can call it.
pub async fn prepare(
    pool: &SqlitePool,
    mut args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
) -> anyhow::Result<PreparedTune> {
    // Fails before any driver/database I/O at all: an unattended write-back must be an
    // explicit, deliberate choice, not something a stray `--write-pid` without `--yes` can
    // trigger by accident.
    if args.write_pid.is_some() && !args.yes {
        anyhow::bail!(
            "--write-pid requires --yes: writing PID constants back to the DCS with no \
             human present to confirm must be an explicit, deliberate choice"
        );
    }
    if let Some(tag_overrides) = &args.tag_overrides {
        tag_overrides.validate()?;
    }

    let db_driver = match args.driver {
        DriverKindArg::Opcda => TuneDriver::Opcda,
        DriverKindArg::Simulator => TuneDriver::Simulator,
    };

    // Snapshotted before `bridge_host`/`server` are resolved to their effective values just
    // below, so a field the caller left unset stays absent here instead of silently baking
    // in a resolved default (`db-run-request-snapshot`) -- see `RequestSnapshot`'s doc
    // comment.
    let request_json = serde_json::to_string(&RequestSnapshot {
        tagname: &args.tagname,
        template: &args.template,
        process_type: args.process_type.into(),
        controller_type: args.controller_type.into(),
        relay_amp: args.relay_amp,
        cycles_skip: args.cycles_skip,
        cycles_count: args.cycles_count,
        noise_protection_secs: args.noise_protection_secs,
        mrft_delay: args.mrft_delay,
        driver: db_driver,
        bridge_host: args.bridge_host.as_deref(),
        server: args.server.as_deref(),
        sim_gain: args.sim_gain,
        sim_tau: args.sim_tau,
        sim_dead_time: args.sim_dead_time,
        sim_noise: args.sim_noise,
        sim_seed: args.sim_seed,
        sim_initial_pv: args.sim_initial_pv,
        sim_initial_mv: args.sim_initial_mv,
        pv_range_high: args.pv_range_high,
        pv_range_low: args.pv_range_low,
        mv_range_high: args.mv_range_high,
        mv_range_low: args.mv_range_low,
        direction: args.direction.map(Into::into),
        tag_overrides: args.tag_overrides.as_ref(),
        poll_interval_ms: args.poll_interval_ms,
        timeout_secs: args.timeout_secs,
        notes: args.notes.as_deref(),
        yes: args.yes,
        write_pid: args.write_pid.map(Into::into),
        allow_uncertain_quality: args.allow_uncertain_quality,
        op_timeout_secs: args.op_timeout_secs,
        restore_timeout_secs: args.restore_timeout_secs,
    })
    .expect(
        "RequestSnapshot serialization is infallible: plain enum/scalar fields, no maps and \
         no floats that JSON can't represent (every f32 here is validated finite before \
         reaching this call, per safety-validation)",
    );

    args.bridge_host = Some(crate::config::resolve_bridge_host(
        args.bridge_host.take(),
        app_config,
    ));
    if matches!(args.driver, DriverKindArg::Opcda) {
        args.server = Some(crate::config::resolve_server(
            args.server.take(),
            app_config,
        )?);
    }

    let template_row = DcsTemplateRow::get_by_name(pool, &args.template)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{}'", args.template))?;
    let template_origin = template_row.origin;
    let template = template_row.template;

    let config = build_loop_config(&args)?;
    let tags = build_loop_tags(&args, &template)?;
    let driver = crate::driver::build(&args).await?;

    let started_at = Utc::now();
    let run = TuneRunRow::start(
        pool,
        None,
        &args.tagname,
        db_driver,
        config,
        template_origin,
        &template,
        &tags,
        started_at,
    )
    .await?;
    TuneRunRow::record_allow_uncertain_quality(pool, run.id, args.allow_uncertain_quality).await?;

    // The *resolved, effective* connection this run actually used -- `None`/`None` for a
    // non-opcda run even though `args.bridge_host` was just unconditionally resolved to a
    // default above (a pre-existing, harmless quirk of that resolution call). Forcing both
    // to `None` here is what makes a simulator/replay run correctly show "no connection"
    // rather than an OPC DA gateway it never actually touched -- load-bearing for
    // `history revert`'s connection safety fix.
    let (opc_server, bridge_host) = if db_driver == TuneDriver::Opcda {
        (args.server.as_deref(), args.bridge_host.as_deref())
    } else {
        (None, None)
    };
    TuneRunRow::record_connection(pool, run.id, opc_server, bridge_host, &request_json).await?;
    let notes = args
        .notes
        .as_deref()
        .map(str::trim)
        .filter(|notes| !notes.is_empty());
    TuneRunRow::update_notes(pool, run.id, notes).await?;

    tracing::info!(
        run_id = run.id,
        template = %args.template,
        process_type = ?config.process_type,
        controller_type = ?config.controller_type,
        driver = ?db_driver,
        allow_uncertain_quality = args.allow_uncertain_quality,
        "starting tune run"
    );

    let write_pid: Option<ResponseLevel> = args.write_pid.map(Into::into);

    Ok(PreparedTune {
        run_id: run.id,
        args,
        template,
        tags,
        driver,
        config,
        started_at,
        write_pid,
    })
}

/// Runs an already-[`prepare`]d tune to completion -- the print-free counterpart to
/// [`run_with_ctrl_c`], for a caller with no terminal to print a summary to and no stdin to
/// prompt on (`bhtune-server`'s background tune task, `tokio::spawn`ed after its
/// `POST /api/runs` handler has already returned `prepared.run_id()` to the HTTP client).
///
/// Calls the exact same [`execute`] this module's CLI path calls, with the exact same
/// arguments, so the actual tuning behavior -- quality checks, restore-on-abort, write-back
/// rollback, all of it -- is identical between the CLI and an HTTP-started run; only the
/// reporting differs. On success, returns the same coarse [`TuneOutcome`]
/// `run_with_ctrl_c`'s printed summary would have shown, computed via the same
/// `tune_outcome_for_run` mapping, and logs it exactly as `run_with_ctrl_c` does. On
/// failure, records the same `tune_runs.fail` row `run_with_ctrl_c` would have.
///
/// A caller that wants the same rich per-response-level detail `print_summary` shows on the
/// CLI should instead read the run back from the database once this resolves (over HTTP,
/// `GET /api/runs/{id}`) -- `execute`'s own DB writes (`tune_results`/`tune_writes`) are the
/// authoritative record of everything `print_summary` would have printed, so there is
/// nothing this function needs to hand back beyond the coarse outcome.
///
/// `prepared.args.output` should be [`OutputFormat::Json`] for every caller of this
/// function, even though nothing here actually prints: [`maybe_write_back`] (called from
/// inside [`execute`]) skips its interactive stdin prompt only when `output ==
/// OutputFormat::Json` (see that function's doc comment) -- a caller with no stdin to read
/// from at all must never risk hitting that prompt. Accordingly, this function passes
/// [`execute`] a [`std::io::empty()`] reader rather than real stdin -- besides there being
/// no human to prompt, `std::io::Empty` is `Send` (unlike a real [`std::io::StdinLock`]),
/// which is what allows the future returned by a call to this function to be
/// `tokio::spawn`ed at all (see `execute`'s own doc comment).
pub async fn drive(
    pool: &SqlitePool,
    prepared: PreparedTune,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<TuneOutcome> {
    let PreparedTune {
        run_id,
        args,
        template,
        tags,
        driver,
        config,
        started_at,
        write_pid,
    } = prepared;

    let outcome = execute(
        pool,
        run_id,
        &args,
        &template,
        &tags,
        driver.as_ref(),
        config,
        started_at,
        write_pid,
        ctrl_c,
        &mut std::io::empty(),
    )
    .await;

    match outcome {
        Ok(run_outcome) => {
            let tune_outcome = tune_outcome_for_run(&run_outcome);
            tracing::info!(run_id, outcome = tune_outcome.label(), "tune run finished");
            Ok(tune_outcome)
        }
        Err(e) => {
            tracing::error!(run_id, error = %e, "tune run failed");
            TuneRunRow::fail(pool, run_id, Utc::now(), &e.to_string())
                .await
                .ok();
            Err(e)
        }
    }
}

#[derive(Debug)]
enum RunOutcome {
    Completed {
        write_back: WriteBackOutcome,
        /// Human-readable reason `write_back` was `Skipped`/`Failed`, or `None` when it
        /// succeeded ([`WriteBackOutcome::Written`]) or the outcome is otherwise
        /// self-explanatory. Exists so `--output json` can report the same explanation
        /// `maybe_write_back`'s suppressed `println!`s would have shown in `Table` mode,
        /// without printing anything ahead of the run's final JSON object
        /// (`safety-json-contract`, finding 8).
        write_back_detail: Option<String>,
    },
    Aborted(AbortReason),
    /// The run ended (via normal completion or [`RunOutcome::Aborted`]) but the subsequent
    /// restore attempt ([`attempt_restore`]) could not be confirmed -- a second Ctrl+C
    /// arrived, or `--restore-timeout-secs` elapsed, before `restore()` itself resolved.
    /// `reason` is a human-readable description of what happened, already including the
    /// original abort trigger (if any) -- see `execute`'s composition of it. Write-back is
    /// always skipped in this case, since writing new PID constants to a loop whose mode/MV
    /// cannot be confirmed restored would compound the uncertainty.
    RestoreIncomplete {
        reason: String,
    },
}

/// Why a run ended via [`RunOutcome::Aborted`] instead of a normal engine completion.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AbortReason {
    /// Ctrl+C.
    UserInterrupt,
    /// `--timeout-secs` elapsed before the engine reported completion. Carries the
    /// configured limit that was hit, for the printed/JSON summary.
    Timeout { timeout_secs: u64 },
    /// A single driver read/write during a poll tick did not resolve within
    /// `--op-timeout-secs` -- distinct from [`AbortReason::Timeout`], which bounds the whole
    /// run rather than one operation. Carries the tag that stalled and the configured limit,
    /// for the printed/JSON summary. Maps to the same [`TuneOutcome::TimedOut`] as
    /// `Timeout`, since both mean "gave up waiting", differing only in what exactly timed
    /// out.
    OperationTimedOut { tag: String, op_timeout_secs: u64 },
    /// An in-flight PV poll sample's quality was `Bad`, or `Uncertain` without
    /// `--allow-uncertain-quality` set (finding 5 of the live-plant safety review). Unlike
    /// the two variants above, this is checked and constructed from inside
    /// [`run_polling_loop`] itself rather than from [`execute`]'s outer `tokio::select!`,
    /// since it depends on the value just read, not an independent timer/signal. A poor
    /// quality reading *before* the mode transition (any of `read_initial_values`'s
    /// readings, including the setpoint capture) is instead a hard failure via a plain
    /// `anyhow::Error` -- see `check_quality` -- since nothing has been mutated yet at that
    /// point, so there's no loop state to restore and no reason to route it through this
    /// enum. Carries `tag`/`quality` for the printed/JSON summary.
    PoorQuality {
        tag: String,
        quality: bhtune_driver::Quality,
    },
}

/// The result of `maybe_write_back`'s attempt (or non-attempt) to write calculated PID
/// parameters back to the DCS. A real write (`Written`/`Failed`) is always fully recorded in
/// `tune_writes`; `Skipped` never touches the driver at all, so it leaves no row there. This
/// enum exists purely to drive the printed summary and the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteBackOutcome {
    /// No write was attempted: no PID constant tags configured, no results recorded, or
    /// (interactive path only) the user chose to skip / gave invalid input.
    Skipped,
    /// The write succeeded and was confirmed by a readback.
    Written { response_level: ResponseLevel },
    /// A write was attempted (interactively selected, or requested via `--write-pid`) but
    /// failed -- the driver rejected it, the confirmation readback failed, or (defensively)
    /// `--write-pid` named a response level with no recorded calculated result.
    Failed,
}

/// Maps one run's full outcome down to the coarser [`TuneOutcome`] that drives the process
/// exit code -- a write-back failure demotes an otherwise-successful test completion to
/// [`TuneOutcome::WriteBackFailed`], since an unattended caller needs to know the loop was
/// left with its *old* PID constants, not the newly calculated ones.
fn tune_outcome_for_run(outcome: &RunOutcome) -> TuneOutcome {
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
        RunOutcome::RestoreIncomplete { .. } => TuneOutcome::RestoreIncomplete,
    }
}

/// Prints this run's final outcome line -- either the plain-text shape or a `--output json`
/// object -- and returns the [`TuneOutcome`] the caller should propagate as the process's
/// exit code.
fn print_summary(run_id: i64, outcome: &RunOutcome, output: OutputFormat) -> TuneOutcome {
    let tune_outcome = tune_outcome_for_run(outcome);
    match output {
        OutputFormat::Table => match outcome {
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
                    "Tune aborted: exceeded the {timeout_secs}s --timeout-secs limit before completing; loop restored."
                );
            }
            RunOutcome::Aborted(AbortReason::OperationTimedOut {
                tag,
                op_timeout_secs,
            }) => {
                println!(
                    "Tune aborted: tag '{tag}' did not respond within the {op_timeout_secs}s --op-timeout-secs limit; loop restored."
                );
            }
            RunOutcome::Aborted(AbortReason::PoorQuality { tag, quality }) => {
                println!(
                    "Tune aborted: tag '{tag}' reported OPC quality {quality:?} during polling; loop restored."
                );
            }
            RunOutcome::RestoreIncomplete { reason } => {
                println!(
                    "Tune ended, but the loop's restore could not be confirmed ({reason}). Check the loop by hand -- see the warning above for the tag and value to check."
                );
            }
        },
        OutputFormat::Json => {
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
                "restore_incomplete_reason": restore_incomplete_reason,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&json)
                    .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
            );
        }
    }
    tune_outcome
}

fn build_loop_config(args: &TuneArgs) -> anyhow::Result<LoopConfig> {
    let process_type: ProcessType = args.process_type.into();
    let controller_type: ControllerType = args.controller_type.into();

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
        mrft_delay_secs: args.mrft_delay,
    };
    // Real range validation at the model level (see `LoopConfig::validate`), not just this
    // flag parse -- catches an out-of-range `--relay-amp` (including the legacy predecessor's
    // "not blank" bug of a stray debug shortcut reaching this field) before any driver
    // connection or database write.
    config.validate()?;
    Ok(config)
}

/// Builds the loop's full tag set. For `--driver opcda`, derives from `--tagname` and the
/// template, then layers any explicit `--pv-range-*`/`--mv-range-*`/`--direction` overrides
/// on top. For `--driver simulator`, `SimulatorDriver`'s fixed two-tag contract means the
/// range/direction overrides are mandatory (normally supplied by
/// `SimulateArgs::into_tune_args`); a direct `bhtune tune --driver simulator` invocation
/// missing any of them is a clear usage error rather than a confusing runtime failure.
fn build_loop_tags(args: &TuneArgs, template: &DcsTemplate) -> anyhow::Result<LoopTags> {
    match args.driver {
        DriverKindArg::Opcda => {
            let mut tags = LoopTags::derive_from_pv_tag(&args.tagname, template);
            if let Some(overrides) = &args.tag_overrides {
                overrides.apply_to(&mut tags);
            }
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
                tags.controller_direction = TagOrValue::Value(d.into());
            }
            Ok(tags)
        }
        DriverKindArg::Simulator => {
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
                controller_direction: TagOrValue::Value(direction.into()),
                proportional_constant: None,
                integral_constant: None,
                derivative_constant: None,
            })
        }
    }
}

/// Everything read from the driver before any mode transition is attempted — mirrors
/// `ReadInitialOPCvalues`.
#[derive(Debug)]
struct InitialState {
    pv_ini: f32,
    mv_ini: f32,
    pv_range_high: f32,
    pv_range_low: f32,
    mv_range_high: f32,
    mv_range_low: f32,
    direction: ControllerDirection,
    mode_raw: Option<String>,
    mode_attribute_raw: Option<String>,
    /// The setpoint, captured here -- before any mutation of the loop -- whenever the loop's
    /// original mode is Auto and both a mode and a setpoint tag are configured (mirrors
    /// `SvValueIni` in the legacy app, which captured it later, at the moment of actually
    /// transitioning out of Auto). Hoisting the read this early means it's durably persisted
    /// via [`TuneRunRow::record_initial_readings`] before `transition_to_manual`'s first
    /// mutating write is even attempted, so a crashed run's restore intent survives the
    /// process dying outright (`safety-restore-guard`, finding 3 of the live-plant safety
    /// review). Note that this field being `Some(..)` only proves the loop *was* in Auto --
    /// not that a mode transition was actually attempted, since `read_initial_values` runs
    /// unconditionally, before any such decision is made -- so `restore`'s setpoint-revert
    /// step is additionally gated on [`MutationGuard::mode_written`], not on this field
    /// alone.
    setpoint_ini: Option<f32>,
}

/// Tracks which of `transition_to_manual`'s mutations were actually *attempted* -- armed
/// immediately before each write is issued, not after it succeeds -- so `restore` can
/// independently decide what's safe/necessary to revert even when `transition_to_manual`
/// itself returns partway through with an error (`safety-restore-guard`, finding 3 of the
/// live-plant safety review). Renamed from the former `ModeRestoreState`, which held only
/// the captured setpoint; that value now lives on [`InitialState`] instead, read before any
/// mutation rather than during `transition_to_manual` -- see that field's doc comment for
/// why.
#[derive(Debug, Default)]
struct MutationGuard {
    /// The mode-attribute tag's "put in program/computer mode" write was attempted.
    mode_attribute_written: bool,
    /// The mode tag's "put in Manual" write was attempted.
    mode_written: bool,
    /// A relay-test MV write was attempted at least once during `run_polling_loop`. Tracked
    /// for completeness/audit parity with the other two flags -- `restore`'s MV-revert step
    /// is unconditional and never gated by this flag, since a redundant write-back is always
    /// harmless (idempotent if the pre-test value is already there, or safely rejected by
    /// the DCS if the loop never actually left Auto), so there is no correctness reason to
    /// skip attempting it even while this is still `false`.
    mv_written: bool,
}

/// Small, private helper bundling the two DB writes that follow a completed MRFT test, so
/// `execute` can route a failure in either one through a single best-effort restore attempt
/// instead of silently skipping it (`safety-restore-guard`, finding 3 of the live-plant
/// safety review) -- previously, both ran ahead of `attempt_restore` with a bare `?` each.
async fn finish_completed_run(
    pool: &SqlitePool,
    run_id: i64,
    completion: Action,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
) -> anyhow::Result<()> {
    persist_results(
        pool, run_id, completion, direction, config, pv_range, template,
    )
    .await?;
    TuneRunRow::complete(pool, run_id, Utc::now()).await?;
    Ok(())
}

/// Generic over `reader` (rather than hardcoding `std::io::stdin().lock()` internally) so
/// this function's generated future is monomorphized separately per call site: [`run_with_ctrl_c`]
/// (the CLI path) instantiates it with the real, process-wide [`std::io::StdinLock`], which
/// is `!Send` -- fine there, since that future is only ever `.await`ed directly, never
/// `tokio::spawn`ed. [`drive`] (the HTTP path) instantiates it with [`std::io::Empty`]
/// (`std::io::empty()`), which *is* `Send`, so `bhtune-server` can spawn the resulting
/// future onto its background tune task. Passing a live `StdinLock` through as a plain
/// parameter of a single non-generic `execute` would force both instantiations to share one
/// concrete (and therefore `!Send`) future type, which is exactly the compile error this
/// split avoids -- see `ActiveRun::start`'s `Send` bound in `bhtune-server`.
#[allow(clippy::too_many_arguments)]
async fn execute<R: std::io::BufRead>(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    started_at: DateTime<Utc>,
    write_pid: Option<ResponseLevel>,
    ctrl_c: &mut CtrlC,
    reader: &mut R,
) -> anyhow::Result<RunOutcome> {
    let allow_uncertain = args.allow_uncertain_quality;
    let initial = read_initial_values(driver, tags, template, allow_uncertain).await?;
    validate_initial_state(&initial)?;

    // Persisted before any mutation is attempted (`safety-restore-guard`): a crash between
    // here and a confirmed restore still leaves a durable record of the mode/mode-attribute/
    // setpoint as they were *before* anything was written, so `bhtune restore-loop` can
    // reconstruct and restore the loop later even if the process never gets to do so itself.
    TuneRunRow::record_initial_readings(
        pool,
        run_id,
        TuneRunInitialReadings {
            pv_ini: initial.pv_ini,
            mv_ini: initial.mv_ini,
            mv_range_low: initial.mv_range_low,
            mv_range_high: initial.mv_range_high,
            pv_range_high: initial.pv_range_high,
            pv_range_low: initial.pv_range_low,
            controller_direction: initial.direction,
            mode_raw: initial.mode_raw.clone(),
            mode_attribute_raw: initial.mode_attribute_raw.clone(),
            setpoint_ini: initial.setpoint_ini,
        },
    )
    .await?;

    let mut guard = MutationGuard::default();
    if let Err(e) = transition_to_manual(driver, tags, template, &initial, &mut guard).await {
        return Err(restore_best_effort_then_propagate(
            pool,
            run_id,
            driver,
            tags,
            template,
            &initial,
            &guard,
            args.restore_timeout_secs,
            ctrl_c,
            e,
        )
        .await);
    }

    let beta = lookup(
        config.process_type,
        config.controller_type,
        ResponseLevel::Aggressive,
    )
    .beta;

    let mut engine = MrftEngine::new(
        config,
        initial.direction,
        beta,
        InitialReadings {
            pv_ini: initial.pv_ini,
            mv_ini: initial.mv_ini,
            mv_range_low: initial.mv_range_low,
            mv_range_high: initial.mv_range_high,
        },
        started_at,
        MrftCompat::default(),
    );

    let poll_result = run_polling_loop(
        pool,
        run_id,
        args,
        tags,
        driver,
        &mut engine,
        started_at,
        ctrl_c,
        &mut guard,
    )
    .await;

    match poll_result {
        Ok(PollOutcome::Completed(completion)) => {
            let pv_range = PvRange {
                high: initial.pv_range_high,
                low: initial.pv_range_low,
            };
            if let Err(e) = finish_completed_run(
                pool,
                run_id,
                completion,
                initial.direction,
                config,
                pv_range,
                template,
            )
            .await
            {
                return Err(restore_best_effort_then_propagate(
                    pool,
                    run_id,
                    driver,
                    tags,
                    template,
                    &initial,
                    &guard,
                    args.restore_timeout_secs,
                    ctrl_c,
                    e,
                )
                .await);
            }

            let restore_attempt = attempt_restore(
                driver,
                tags,
                template,
                &initial,
                &guard,
                args.restore_timeout_secs,
                ctrl_c,
            )
            .await;
            record_restore_status_best_effort(pool, run_id, &restore_attempt).await;
            match restore_attempt {
                RestoreAttempt::Confirmed => {
                    let (write_back, write_back_detail) = maybe_write_back(
                        pool,
                        run_id,
                        tags,
                        template,
                        driver,
                        config,
                        write_pid,
                        args.output,
                        allow_uncertain,
                        reader,
                    )
                    .await?;
                    Ok(RunOutcome::Completed {
                        write_back,
                        write_back_detail,
                    })
                }
                RestoreAttempt::Incomplete { reason } => {
                    Ok(RunOutcome::RestoreIncomplete { reason })
                }
            }
        }
        Ok(PollOutcome::Aborted(reason)) => {
            let restore_attempt = attempt_restore(
                driver,
                tags,
                template,
                &initial,
                &guard,
                args.restore_timeout_secs,
                ctrl_c,
            )
            .await;
            record_restore_status_best_effort(pool, run_id, &restore_attempt).await;
            TuneRunRow::abort(pool, run_id, Utc::now()).await?;
            match restore_attempt {
                RestoreAttempt::Confirmed => Ok(RunOutcome::Aborted(reason)),
                RestoreAttempt::Incomplete {
                    reason: restore_reason,
                } => Ok(RunOutcome::RestoreIncomplete {
                    reason: format!("run aborted ({reason:?}); {restore_reason}"),
                }),
            }
        }
        Err(e) => {
            // Best-effort: a failed test still stroked the valve, so try to put it back even
            // though the overall run is going to be reported as failed regardless. Still
            // bounded/interruptible (a second Ctrl+C or `--restore-timeout-secs` still cuts
            // it short) and still warns loudly on an incomplete restore.
            Err(restore_best_effort_then_propagate(
                pool,
                run_id,
                driver,
                tags,
                template,
                &initial,
                &guard,
                args.restore_timeout_secs,
                ctrl_c,
                e,
            )
            .await)
        }
    }
}

/// The single choke point enforcing finding 5 of the live-plant safety review
/// ("`Quality::is_trustworthy()` exists and is documented as the rule; nothing in the tune
/// path calls it"): `Quality::Bad` is never accepted, flag or no flag; `Quality::Uncertain`
/// is accepted only when `allow_uncertain` is set (`--allow-uncertain-quality`), and each use
/// of it is logged loudly so a run executed under relaxed rules is never silently
/// indistinguishable from a normal one; `Quality::Good` always passes.
fn check_quality(
    tag: &str,
    quality: bhtune_driver::Quality,
    allow_uncertain: bool,
) -> anyhow::Result<()> {
    match quality {
        bhtune_driver::Quality::Good => Ok(()),
        bhtune_driver::Quality::Uncertain if allow_uncertain => {
            tracing::warn!(
                tag,
                "accepting Uncertain-quality reading because --allow-uncertain-quality is set"
            );
            Ok(())
        }
        bhtune_driver::Quality::Uncertain => {
            anyhow::bail!(
                "tag '{tag}' reported OPC quality Uncertain; refusing to trust it for a \
                 tuning-critical reading (pass --allow-uncertain-quality to accept Uncertain \
                 readings; Bad is never accepted)"
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

async fn read_raw(driver: &dyn Driver, tag: &str, allow_uncertain: bool) -> anyhow::Result<String> {
    let values = driver.read(&[tag.to_string()]).await?;
    let value = values
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    check_quality(tag, value.quality, allow_uncertain)?;
    Ok(value.value)
}

async fn read_f32(driver: &dyn Driver, tag: &str, allow_uncertain: bool) -> anyhow::Result<f32> {
    let raw = read_raw(driver, tag, allow_uncertain).await?;
    let value: f32 = raw
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{raw}' is not a number"))?;
    if !value.is_finite() {
        anyhow::bail!("tag '{tag}' value '{raw}' is not a finite number");
    }
    Ok(value)
}

async fn resolve_f32(
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

async fn resolve_direction(
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

/// Reads the PV tag for one in-flight MRFT poll tick, without hard-failing on its
/// [`bhtune_driver::Quality`] the way [`read_f32`] does. `run_polling_loop` needs the raw
/// quality alongside the value so it can record the sample (with its quality) *before*
/// deciding whether to abort -- finding 5 requires "the sample that triggered it is
/// recorded", which a propagated `anyhow::Error` from a `check_quality`-enforcing read would
/// lose. Still hard-fails on a non-numeric/non-finite value regardless of quality, exactly
/// like [`read_f32`], since that's a data-shape problem no quality flag can excuse.
async fn read_pv_sample(
    driver: &dyn Driver,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    let values = driver.read(&[tag.to_string()]).await?;
    let value = values
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    let pv: f32 = value
        .value
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{}' is not a number", value.value))?;
    if !pv.is_finite() {
        anyhow::bail!("tag '{tag}' value '{}' is not a finite number", value.value);
    }
    Ok((pv, value.quality))
}

async fn write_raw(driver: &dyn Driver, tag: &str, value: String) -> anyhow::Result<()> {
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

async fn write_value(driver: &dyn Driver, tag: &str, value: f32) -> anyhow::Result<()> {
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

/// Pure port of `ReadInitialOPCvalues`: everything read before any mode transition.
async fn read_initial_values(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    allow_uncertain: bool,
) -> anyhow::Result<InitialState> {
    let pv_ini = read_f32(driver, &tags.process_variable, allow_uncertain).await?;
    let mv_ini = read_f32(driver, &tags.manipulated_variable, allow_uncertain).await?;

    let mode_raw = match &tags.controller_mode {
        Some(tag) => Some(read_raw(driver, tag, allow_uncertain).await?),
        None => None,
    };
    let mode_attribute_raw = match &tags.mode_attribute {
        Some(tag) => Some(read_raw(driver, tag, allow_uncertain).await?),
        None => None,
    };

    // Captured here, before any mutation, whenever the loop's original mode is Auto -- see
    // `InitialState::setpoint_ini`'s doc comment for why this read is hoisted out of
    // `transition_to_manual`, where the legacy app captures the analogous `SvValueIni`.
    let setpoint_ini = match (&tags.setpoint_variable, &mode_raw) {
        (Some(sv_tag), Some(mode_raw)) if mode_raw == &template.mode_auto_value => {
            Some(read_f32(driver, sv_tag, allow_uncertain).await?)
        }
        _ => None,
    };

    let direction = resolve_direction(
        driver,
        &tags.controller_direction,
        template,
        allow_uncertain,
    )
    .await?;
    let pv_range_high = resolve_f32(driver, &tags.upper_pv_range, allow_uncertain).await?;
    let pv_range_low = resolve_f32(driver, &tags.lower_pv_range, allow_uncertain).await?;
    let mv_range_high = resolve_f32(driver, &tags.upper_mv_range, allow_uncertain).await?;
    let mv_range_low = resolve_f32(driver, &tags.lower_mv_range, allow_uncertain).await?;

    Ok(InitialState {
        pv_ini,
        mv_ini,
        pv_range_high,
        pv_range_low,
        mv_range_high,
        mv_range_low,
        direction,
        mode_raw,
        mode_attribute_raw,
        setpoint_ini,
    })
}

/// The single choke point validating an `InitialState` -- from live driver tags and/or CLI
/// flag overrides alike -- before any mutation of the loop happens (i.e. called between
/// `read_initial_values` and `transition_to_manual` in `execute`, never after). `read_f32`/
/// `resolve_f32` already reject a non-finite individual value as it's read; this additionally
/// checks values *together*: range ordering, zero span, and that the initial MV actually
/// lies inside its own reported range. Closes finding 4 of the live-plant safety review --
/// see AGENTS.md's "Live-plant safety hardening" section.
fn validate_initial_state(initial: &InitialState) -> anyhow::Result<()> {
    PvRange::new(initial.pv_range_high, initial.pv_range_low)
        .map_err(|e| anyhow::anyhow!("invalid PV range: {e}"))?;
    let mv_range = MvRange::new(initial.mv_range_high, initial.mv_range_low)
        .map_err(|e| anyhow::anyhow!("invalid MV range: {e}"))?;
    if !mv_range.contains(initial.mv_ini) {
        anyhow::bail!(
            "initial MV {} is outside the MV range [{}, {}]",
            initial.mv_ini,
            initial.mv_range_low,
            initial.mv_range_high
        );
    }
    Ok(())
}

/// Pure port of `ChangeControllerModeToMan`. No-ops entirely when `tags.controller_mode` and
/// `tags.mode_attribute` are both `None` (the simulator's case). `guard`'s flags are armed
/// immediately before each write is attempted, not after it succeeds -- see
/// [`MutationGuard`]'s doc comment for why that ordering matters -- so a caller still knows
/// exactly what was attempted even if this function returns partway through with an error.
async fn transition_to_manual(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &mut MutationGuard,
) -> anyhow::Result<()> {
    if let (Some(attr_tag), Some(program_value)) =
        (&tags.mode_attribute, &template.mode_attribute_program_value)
    {
        guard.mode_attribute_written = true;
        write_raw(driver, attr_tag, program_value.clone()).await?;
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    if let Some(mode_tag) = &tags.controller_mode {
        let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
        if mode_raw != template.mode_manual_value {
            guard.mode_written = true;
            write_raw(driver, mode_tag, template.mode_manual_value.clone()).await?;
        }
    }

    Ok(())
}

/// One step of a [`RestoreReport`]. `NotNeeded` means the step's precondition wasn't met --
/// the loop wasn't in a state requiring reverting that aspect, or the corresponding mutation
/// was never attempted per [`MutationGuard`] -- not that it failed.
#[derive(Debug, Clone, PartialEq, Default)]
enum RestoreStepOutcome {
    #[default]
    NotNeeded,
    Succeeded,
    Failed(String),
}

/// The result of one [`restore`] call: each of the (up to) four independent revert steps,
/// attempted regardless of whether an earlier one failed (`safety-restore-guard`, finding 3
/// of the live-plant safety review, "aggregated best-effort restore") -- so a rejected MV
/// write, say, can never prevent the mode from also being put back.
#[derive(Debug, Clone, Default)]
struct RestoreReport {
    mv: RestoreStepOutcome,
    mode: RestoreStepOutcome,
    setpoint: RestoreStepOutcome,
    mode_attribute: RestoreStepOutcome,
}

impl RestoreReport {
    /// `true` only if every step that was attempted succeeded -- a step that was
    /// [`RestoreStepOutcome::NotNeeded`] doesn't count against this, since nothing needed
    /// doing there in the first place.
    fn all_succeeded(&self) -> bool {
        [&self.mv, &self.mode, &self.setpoint, &self.mode_attribute]
            .into_iter()
            .all(|step| !matches!(step, RestoreStepOutcome::Failed(_)))
    }

    /// A human-readable summary of every step that failed, or `None` if none did. Used both
    /// for [`bhtune_db::models::RestoreStatus::Incomplete`]'s persisted `detail` and the
    /// operator-facing warning.
    fn failure_summary(&self) -> Option<String> {
        let labelled = [
            ("MV", &self.mv),
            ("mode", &self.mode),
            ("setpoint", &self.setpoint),
            ("mode attribute", &self.mode_attribute),
        ];
        let failures: Vec<String> = labelled
            .into_iter()
            .filter_map(|(label, step)| match step {
                RestoreStepOutcome::Failed(e) => Some(format!("{label}: {e}")),
                _ => None,
            })
            .collect();
        if failures.is_empty() {
            None
        } else {
            Some(failures.join("; "))
        }
    }
}

/// Pure port of `ResetOPC` (minus the dead Python-model branch, which is not being ported —
/// see AGENTS.md), restructured for `safety-restore-guard` (finding 3 of the live-plant
/// safety review): every step is attempted independently, via a per-step `match` rather than
/// `?`, so one step failing can never prevent the others from being tried. The MV write-back
/// is unconditional and never gated by `guard` at all -- proven safe by the fact that a
/// no-op MV write-back is always harmless (idempotent if the pre-test value is already
/// there, or safely rejected by the DCS if the loop never actually left Auto) -- while the
/// mode/setpoint/mode-attribute reverts are each gated by both their original value-based
/// condition (as before) *and* the matching `guard` flag, so nothing is "restored" that was
/// never actually mutated in the first place.
async fn restore(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreReport {
    let mv = match write_value(driver, &tags.manipulated_variable, initial.mv_ini).await {
        Ok(()) => RestoreStepOutcome::Succeeded,
        Err(e) => RestoreStepOutcome::Failed(e.to_string()),
    };
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let mut mode = RestoreStepOutcome::NotNeeded;
    if let Some(mode_tag) = &tags.controller_mode {
        let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
        if guard.mode_written && template.revert_mode && mode_raw != template.mode_manual_value {
            mode = match write_raw(driver, mode_tag, mode_raw.to_string()).await {
                Ok(()) => RestoreStepOutcome::Succeeded,
                Err(e) => RestoreStepOutcome::Failed(e.to_string()),
            };
        }
    }

    let mut setpoint = RestoreStepOutcome::NotNeeded;
    if guard.mode_written
        && template.revert_mode
        && let (Some(sv_tag), Some(sv_ini)) = (&tags.setpoint_variable, initial.setpoint_ini)
    {
        tokio::time::sleep(Duration::from_millis(1000)).await;
        setpoint = match write_value(driver, sv_tag, sv_ini).await {
            Ok(()) => RestoreStepOutcome::Succeeded,
            Err(e) => RestoreStepOutcome::Failed(e.to_string()),
        };
    }

    let mut mode_attribute = RestoreStepOutcome::NotNeeded;
    if let Some(attr_tag) = &tags.mode_attribute {
        let attr_raw = initial.mode_attribute_raw.as_deref().unwrap_or_default();
        let program_value = template
            .mode_attribute_program_value
            .as_deref()
            .unwrap_or_default();
        if guard.mode_attribute_written && attr_raw != program_value {
            mode_attribute = match write_raw(driver, attr_tag, attr_raw.to_string()).await {
                Ok(()) => RestoreStepOutcome::Succeeded,
                Err(e) => RestoreStepOutcome::Failed(e.to_string()),
            };
        }
    }

    RestoreReport {
        mv,
        mode,
        setpoint,
        mode_attribute,
    }
}

/// The outcome of racing one driver call ([`read_pv_sample`]/[`write_value`], during a poll
/// tick) against Ctrl+C and `--op-timeout-secs` -- see [`bounded_driver_call`]. Distinct
/// from a genuine `Err` from the call itself (a rejected write, a malformed value, a
/// transport error), which [`bounded_driver_call`] still propagates via `?` rather than
/// wrapping here, since those are real failures, not "gave up waiting".
#[derive(Debug)]
enum TickOperation<T> {
    /// `fut` resolved before either interrupt source.
    Completed(T),
    /// Ctrl+C (or a second Ctrl+C) fired first; `fut` was dropped, abandoning it in flight.
    Cancelled,
    /// `--op-timeout-secs` elapsed first; `fut` was dropped, abandoning it in flight.
    TimedOut,
}

/// Races one driver call against `ctrl_c` and a fresh `op_timeout_secs` sleep, so a single
/// stalled read/write (gateway down, DCOM wedged, network black-holed) can never make the
/// polling loop -- or the restore, via [`attempt_restore`] -- uninterruptible. This is what
/// fixes finding 2 of the live-plant safety review: previously, `run_polling_loop`'s Ctrl+C
/// and `--timeout-secs` listeners only ran *between* tick-body awaits, so a hung call inside
/// one was invisible to both. `fut` is taken by value (not `&mut`) and is simply dropped,
/// abandoning the in-flight operation, on the losing branches -- there is no cancellation
/// signal sent to the driver itself, only to this call's own wait for it. A genuine `Err`
/// from `fut` resolving still propagates through the `?` here, distinct from either
/// [`TickOperation`] interrupt case.
async fn bounded_driver_call<T>(
    op_timeout_secs: u64,
    ctrl_c: &mut CtrlC,
    fut: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<TickOperation<T>> {
    tokio::select! {
        result = fut => result.map(TickOperation::Completed),
        () = ctrl_c.signalled() => Ok(TickOperation::Cancelled),
        () = tokio::time::sleep(Duration::from_secs(op_timeout_secs)) => Ok(TickOperation::TimedOut),
    }
}

/// The outcome of [`attempt_restore`] -- whether `restore()` itself was confirmed to run
/// every applicable step to completion, or was abandoned/only partially successful because a
/// second Ctrl+C arrived, `--restore-timeout-secs` elapsed, or one or more individual restore
/// steps themselves failed.
enum RestoreAttempt {
    /// `restore()` ran to completion and [`RestoreReport::all_succeeded`] was `true`. A
    /// per-step failure inside `restore()` itself is not a separate `Err` case any more --
    /// see [`restore`]'s doc comment -- it's folded into [`RestoreAttempt::Incomplete`]
    /// below via the report's own failure summary.
    Confirmed,
    /// The restore could not be confirmed: a second Ctrl+C arrived, `--restore-timeout-secs`
    /// elapsed, or `restore()` ran to completion but one or more steps failed. `reason` is a
    /// human-readable description of which, for composing into the final
    /// [`RunOutcome::RestoreIncomplete`] message and the stderr warning already printed by
    /// [`warn_restore_incomplete`] before this variant is returned.
    Incomplete { reason: String },
}

/// Restores the loop, bounded by `restore_timeout_secs` and a second Ctrl+C, so a restore
/// that itself hangs (the same class of stalled-driver-call risk `bounded_driver_call`
/// guards the polling loop against) can never block the process indefinitely. Unlike
/// [`bounded_driver_call`], this takes `restore`'s exact parameters directly rather than a
/// generic `impl Future` -- there is only one real call shape (one `restore(...)` call per
/// run), so a generic signature would add no value. Infallible: [`restore`] itself no longer
/// returns a `Result` (every step is now attempted independently and reported via
/// [`RestoreReport`] instead of short-circuiting), so this function's only remaining
/// "failure" shapes are the two abandonment cases already covered by
/// [`RestoreAttempt::Incomplete`], plus a completed-but-not-fully-successful report, which
/// maps to that same variant. On [`RestoreAttempt::Incomplete`], calls
/// [`warn_restore_incomplete`] before returning, so the operator-facing warning is printed
/// exactly once, at the one place that decides a restore could not be confirmed -- callers
/// only need to fold the returned `reason` into their own context (e.g. the original
/// [`AbortReason`], if any) for the final [`RunOutcome`].
async fn attempt_restore(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    restore_timeout_secs: u64,
    ctrl_c: &mut CtrlC,
) -> RestoreAttempt {
    tokio::select! {
        report = restore(driver, tags, template, initial, guard) => {
            if report.all_succeeded() {
                RestoreAttempt::Confirmed
            } else {
                let reason = report
                    .failure_summary()
                    .unwrap_or_else(|| "one or more restore steps failed".to_string());
                warn_restore_incomplete(tags, initial, &reason);
                RestoreAttempt::Incomplete { reason }
            }
        }
        () = ctrl_c.signalled() => {
            let reason = "a second Ctrl+C was received while restoring the loop".to_string();
            warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
        () = tokio::time::sleep(Duration::from_secs(restore_timeout_secs)) => {
            let reason = format!(
                "the restore did not complete within the {restore_timeout_secs}s --restore-timeout-secs limit"
            );
            warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
    }
}

/// Prints a loud, operator-facing warning (to stderr, so it survives `--output json` and any
/// stdout redirection) naming the MV tag and the pre-test value it may not have been
/// restored to, plus a matching `tracing::error!` for anyone mining logs rather than watching
/// the terminal. The loop's mode may also not have been reverted -- see `restore`'s own
/// mode/setpoint/mode-attribute steps -- but the MV is called out specifically since it is
/// the one value every template has and the one most directly consequential if left at a
/// relay-test extreme.
fn warn_restore_incomplete(tags: &LoopTags, initial: &InitialState, reason: &str) {
    eprintln!(
        "WARNING: could not confirm the loop was fully restored ({reason}). Tag '{}' may still be at its last relay-test value instead of its pre-test value {}. Check it -- and the loop's mode -- by hand.",
        tags.manipulated_variable, initial.mv_ini
    );
    tracing::error!(
        mv_tag = %tags.manipulated_variable,
        mv_ini = initial.mv_ini,
        reason,
        "loop restore could not be confirmed"
    );
}

/// Best-effort records a restore attempt's outcome on the run (`safety-restore-guard`,
/// finding 3 of the live-plant safety review) -- logs and swallows its own failure rather
/// than propagating, since failing to *record* that a restore was attempted must never
/// itself change what error (if any) a run reports.
async fn record_restore_status_best_effort(
    pool: &SqlitePool,
    run_id: i64,
    attempt: &RestoreAttempt,
) {
    let (status, detail) = match attempt {
        RestoreAttempt::Confirmed => (bhtune_db::models::RestoreStatus::Confirmed, None),
        RestoreAttempt::Incomplete { reason } => (
            bhtune_db::models::RestoreStatus::Incomplete,
            Some(reason.as_str()),
        ),
    };
    if let Err(e) = TuneRunRow::record_restore_status(pool, run_id, status, detail).await {
        tracing::error!(run_id, error = %e, "failed to record restore status");
    }
}

/// Attempts a best-effort restore, records its outcome, then returns `err` **unchanged** --
/// the single choke point every early-return error path in `execute` funnels through, so a
/// partial mutation is never left un-restored just because the step that failed came before
/// `attempt_restore` was reached (`safety-restore-guard`, finding 3 of the live-plant safety
/// review, fixing three such gaps: a failed `transition_to_manual`, a failed
/// `record_initial_readings`/`persist_results`/`complete` after a successful test, and any
/// other hard failure from `run_polling_loop` itself). Always returns the *original* `err`:
/// neither an incomplete restore nor a failure recording its status should ever mask the
/// real reason the run is failing.
#[allow(clippy::too_many_arguments)]
async fn restore_best_effort_then_propagate(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    restore_timeout_secs: u64,
    ctrl_c: &mut CtrlC,
    err: anyhow::Error,
) -> anyhow::Error {
    let attempt = attempt_restore(
        driver,
        tags,
        template,
        initial,
        guard,
        restore_timeout_secs,
        ctrl_c,
    )
    .await;
    record_restore_status_best_effort(pool, run_id, &attempt).await;
    err
}

/// Distinguishes *why* [`run_polling_loop`] ended without a normal engine completion, so
/// `execute` can record and report the right [`AbortReason`].
enum PollOutcome {
    /// The engine reported [`Action::Complete`] and any post-completion `--mrft-delay`
    /// padding has elapsed.
    Completed(Action),
    /// Ctrl+C, `--timeout-secs`, `--op-timeout-secs`, or a poor-quality PV sample ended the
    /// run before that.
    Aborted(AbortReason),
}

/// Polls the driver on `args.poll_interval_ms`, driving `engine` once the pre-test
/// `--mrft-delay` padding period has elapsed, and continuing to record (but not evaluate)
/// samples for the same padding period after completion. Returns `Ok(PollOutcome::Completed`
/// on a normal finish, `Ok(PollOutcome::Aborted)` if interrupted by Ctrl+C, by
/// `args.timeout_secs` elapsing, or by a single driver call exceeding
/// `args.op_timeout_secs` -- the last of these via [`bounded_driver_call`], which wraps
/// every driver read/write in the tick body so a stalled call is abandoned rather than
/// awaited forever, keeping Ctrl+C and both timeouts effective even mid-hung-read/write. The
/// outer `tokio::select!` below still separately covers the *idle* wait between ticks (via
/// `ctrl_c`, shared with every `bounded_driver_call` inside the winning tick body -- see
/// that function's doc comment for why reusing it across nested `select!`s is safe) and the
/// whole-run `--timeout-secs` deadline.
#[allow(clippy::too_many_arguments)]
async fn run_polling_loop(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    tags: &LoopTags,
    driver: &dyn Driver,
    engine: &mut MrftEngine,
    start_time: DateTime<Utc>,
    ctrl_c: &mut CtrlC,
    guard: &mut MutationGuard,
) -> anyhow::Result<PollOutcome> {
    let mut interval = tokio::time::interval(Duration::from_millis(args.poll_interval_ms.max(1)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let pre_delay_end = start_time + chrono::Duration::seconds(args.mrft_delay as i64);
    let mut tick_index: i64 = 0;
    let mut completion: Option<Action> = None;
    let mut post_delay_end: Option<DateTime<Utc>> = None;

    // A mandatory safety net for unattended operation: an unattended run must never be able
    // to perturb a live process indefinitely (a stuck relay, a misconfigured tag mapping that
    // never crosses hysteresis, a stalled driver read). Created once and raced via
    // `tokio::select!` on every iteration below, rather than checked only after each
    // completed tick, so it fires even if a single `read_f32` call itself hangs.
    let timeout = tokio::time::sleep(Duration::from_secs(args.timeout_secs));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let now = Utc::now();
                let (pv, quality) = match bounded_driver_call(
                    args.op_timeout_secs,
                    ctrl_c,
                    read_pv_sample(driver, &tags.process_variable),
                )
                .await?
                {
                    TickOperation::Completed(sample) => sample,
                    TickOperation::Cancelled => {
                        tracing::warn!(run_id, tick_index, "Ctrl+C received while reading the PV; aborting run");
                        return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                    }
                    TickOperation::TimedOut => {
                        tracing::warn!(
                            run_id,
                            tick_index,
                            op_timeout_secs = args.op_timeout_secs,
                            tag = %tags.process_variable,
                            "--op-timeout-secs elapsed reading the PV; aborting run"
                        );
                        return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                            tag: tags.process_variable.clone(),
                            op_timeout_secs: args.op_timeout_secs,
                        }));
                    }
                };
                let tick = Tick { time: now, pv };
                let sample_quality = sample_quality_from_driver(quality);

                if let Err(e) = check_quality(&tags.process_variable, quality, args.allow_uncertain_quality) {
                    tracing::warn!(
                        run_id,
                        tick_index,
                        tag = %tags.process_variable,
                        quality = ?quality,
                        error = %e,
                        "PV quality check failed; aborting run"
                    );
                    // Record the triggering sample (with its real quality) before aborting, so
                    // the history explorer can show exactly what was seen when the run gave up.
                    TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state(), sample_quality).await?;
                    return Ok(PollOutcome::Aborted(AbortReason::PoorQuality {
                        tag: tags.process_variable.clone(),
                        quality,
                    }));
                }

                if completion.is_none() && now < pre_delay_end {
                    TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state(), sample_quality).await?;
                    tick_index += 1;
                    continue;
                }

                for action in engine.step(tick) {
                    match action {
                        Action::WriteMv(v) => {
                            guard.mv_written = true;
                            match bounded_driver_call(
                                args.op_timeout_secs,
                                ctrl_c,
                                write_value(driver, &tags.manipulated_variable, v),
                            )
                            .await?
                            {
                                TickOperation::Completed(()) => {}
                                TickOperation::Cancelled => {
                                    // A valid sample/tick is already in hand for this iteration
                                    // (unlike the PV-read timeout/cancel case above), so record
                                    // it before aborting -- same rationale as the quality-check
                                    // abort above.
                                    TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state(), sample_quality).await?;
                                    tracing::warn!(run_id, tick_index, "Ctrl+C received while writing the MV; aborting run");
                                    return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                                }
                                TickOperation::TimedOut => {
                                    TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state(), sample_quality).await?;
                                    tracing::warn!(
                                        run_id,
                                        tick_index,
                                        op_timeout_secs = args.op_timeout_secs,
                                        tag = %tags.manipulated_variable,
                                        "--op-timeout-secs elapsed writing the MV; aborting run"
                                    );
                                    return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                                        tag: tags.manipulated_variable.clone(),
                                        op_timeout_secs: args.op_timeout_secs,
                                    }));
                                }
                            }
                        }
                        Action::Complete { .. } => {
                            tracing::info!(
                                run_id,
                                tick_index,
                                "MRFT engine reported completion; recording post-test padding"
                            );
                            completion = Some(action);
                            post_delay_end =
                                Some(now + chrono::Duration::seconds(args.mrft_delay as i64));
                        }
                    }
                }

                tracing::trace!(run_id, tick_index, pv, "recorded tune sample");
                TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state(), sample_quality).await?;
                tick_index += 1;

                if let Some(end) = post_delay_end
                    && now >= end
                {
                    break;
                }
            }
            () = ctrl_c.signalled() => {
                tracing::warn!(run_id, tick_index, "Ctrl+C received; aborting run");
                return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
            }
            _ = &mut timeout => {
                tracing::warn!(
                    run_id,
                    tick_index,
                    timeout_secs = args.timeout_secs,
                    "--timeout-secs elapsed before completion; aborting run"
                );
                return Ok(PollOutcome::Aborted(AbortReason::Timeout {
                    timeout_secs: args.timeout_secs,
                }));
            }
        }
    }

    Ok(PollOutcome::Completed(completion.expect(
        "the loop only `break`s after `completion` is set",
    )))
}

async fn persist_results(
    pool: &SqlitePool,
    run_id: i64,
    action: Action,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
    template: &DcsTemplate,
) -> anyhow::Result<()> {
    let Action::Complete {
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
    } = action
    else {
        anyhow::bail!("internal error: persist_results called with a non-Complete action");
    };

    let results = calculate_all(
        &peaks,
        &troughs,
        &switch_times,
        mv_sign_init,
        direction,
        config,
        pv_range,
        template,
        TuningMathCompat::default(),
    );

    for (tuning, pid) in results {
        let row = TuneResultRow::from_calculated(run_id, tuning, pid);
        TuneResultRow::insert(pool, &row).await?;
    }

    Ok(())
}

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
fn pid_value_within_tolerance(requested: f32, actual: f32) -> bool {
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
async fn rollback_pid_writes(
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

    let previous =
        match read_previous_pid_values(driver, p_tag, i_tag, d_tag, allow_uncertain).await {
            Ok(previous) => previous,
            Err(e) => {
                let message = e.to_string();
                new_write.error_message = Some(message.clone());
                TuneWriteRow::insert(pool, run_id, new_write).await?;
                tracing::error!(run_id, ?response_level, ?kind, %message, "PID pre-read failed");
                return Ok(PidWriteOutcome::Failed {
                    detail: format!("pre-read failed: {message}"),
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
async fn maybe_write_back(
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
        if output == OutputFormat::Table {
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

    let selected = match write_pid {
        Some(level) => match results.iter().find(|r| r.response_level == level) {
            Some(r) => {
                if output == OutputFormat::Table {
                    println!(
                        "Non-interactively writing {level:?} PID parameters back to the DCS (--write-pid)."
                    );
                }
                r
            }
            None => {
                let detail = format!("no calculated result recorded for response level {level:?}");
                if output == OutputFormat::Table {
                    println!(
                        "No calculated result recorded for response level {level:?}; skipping write-back."
                    );
                }
                return Ok((WriteBackOutcome::Failed, Some(detail)));
            }
        },
        None if output == OutputFormat::Json => {
            return Ok((
                WriteBackOutcome::Skipped,
                Some(
                    "--output json was set without --write-pid; skipped the interactive \
                     write-back prompt since there is no human present to answer it"
                        .to_string(),
                ),
            ));
        }
        None => {
            eprintln!("\nCalculated PID parameters:");
            for (i, r) in results.iter().enumerate() {
                eprintln!(
                    "  {}. {:?}: P={:.4} I={:.4} D={:.4}",
                    i + 1,
                    r.response_level,
                    r.proportional,
                    r.integral,
                    r.derivative
                );
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
                return Ok((
                    WriteBackOutcome::Skipped,
                    Some("skipped interactively (no selection made)".to_string()),
                ));
            }

            match input.parse::<usize>() {
                Ok(n) if n >= 1 && n <= results.len() => &results[n - 1],
                _ => {
                    eprintln!("Invalid selection; skipping PID write-back.");
                    return Ok((
                        WriteBackOutcome::Skipped,
                        Some("invalid response level selection".to_string()),
                    ));
                }
            }
        }
    };

    let response_level = selected.response_level;
    let pid = PidParameters {
        response_level: selected.response_level,
        proportional: selected.proportional,
        integral: selected.integral,
        derivative: selected.derivative,
    };
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

    match outcome {
        PidWriteOutcome::Written => {
            if output == OutputFormat::Table {
                println!("Wrote and confirmed {response_level:?} PID parameters.");
            }
            Ok((WriteBackOutcome::Written { response_level }, None))
        }
        PidWriteOutcome::Failed { detail } => {
            if output == OutputFormat::Table {
                println!("PID write-back failed: {detail}");
            }
            Ok((WriteBackOutcome::Failed, Some(detail)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{ControllerTypeArg, DirectionArg, ProcessTypeArg};
    use bhtune_db::models::TemplateOrigin;

    async fn seeded_pool() -> SqlitePool {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, Utc::now())
            .await
            .unwrap();
        pool
    }

    /// `run()`'s tests all pass explicit `TuneArgs.bridge_host`/`server` values (or the
    /// simulator driver, which ignores both), so an all-default `BhtuneConfig` never
    /// actually supplies anything here -- it's only present because `run()`'s signature
    /// requires it.
    fn test_config() -> crate::config::BhtuneConfig {
        crate::config::BhtuneConfig::default()
    }

    /// A fast-converging simulator tune: proportionally scaled down from
    /// `bhtune-driver`'s own proven `FopdtConfig::new(1.0, 2.0, 5.0, 1.0)` E2E fixture (2
    /// ticks of lag, 5 ticks of dead time) so the whole test — which polls on a real
    /// `tokio::time::interval`, unlike that lower-level test's manually driven ticks —
    /// finishes in well under a second of real wall-clock time.
    fn fast_simulator_args() -> TuneArgs {
        TuneArgs {
            tagname: "ignored-for-simulator".to_string(),
            template: "Yokogawa CentumVP".to_string(),
            process_type: ProcessTypeArg::Flow,
            controller_type: ControllerTypeArg::Pi,
            relay_amp: 10.0,
            cycles_skip: Some(1),
            cycles_count: Some(2),
            noise_protection_secs: Some(0),
            mrft_delay: 0,
            driver: DriverKindArg::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: 1.0,
            sim_tau: 0.01,
            sim_dead_time: 0.025,
            sim_noise: 0.0,
            sim_seed: 0,
            sim_initial_pv: 50.0,
            sim_initial_mv: 50.0,
            pv_range_high: Some(100.0),
            pv_range_low: Some(0.0),
            mv_range_high: Some(100.0),
            mv_range_low: Some(0.0),
            direction: Some(DirectionArg::Reverse),
            tag_overrides: None,
            poll_interval_ms: 5,
            timeout_secs: 3600,
            op_timeout_secs: 30,
            restore_timeout_secs: 30,
            notes: Some("test note".to_string()),
            yes: false,
            write_pid: None,
            allow_uncertain_quality: false,
            output: OutputFormat::Table,
        }
    }

    #[tokio::test]
    async fn a_full_simulator_tune_completes_and_persists_results() {
        let pool = seeded_pool().await;
        run(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Completed);
        assert_eq!(runs[0].loop_name, "ignored-for-simulator");
        assert_eq!(runs[0].notes.as_deref(), Some("test note"));
        assert!(runs[0].initial_readings.is_some());

        // A simulator run has no OPC DA connection at all -- `db-run-request-snapshot`
        // requires both to be `None` here regardless of whatever `--bridge-host` default
        // `prepare` resolved internally, since the driver never actually contacted a
        // gateway.
        assert_eq!(runs[0].opc_server, None);
        assert_eq!(runs[0].bridge_host, None);

        // The submitted request is snapshotted verbatim (pre-resolution), so a field the
        // test left unset (`server`) stays absent/null rather than showing a resolved
        // default.
        let request: serde_json::Value = serde_json::from_str(&runs[0].request_json).unwrap();
        assert_eq!(request["tagname"], "ignored-for-simulator");
        assert_eq!(request["driver"], "simulator");
        assert_eq!(request["server"], serde_json::Value::Null);
        assert_eq!(request["notes"], "test note");

        let results = TuneResultRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);

        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(!samples.is_empty());

        // The simulator driver has no PID constant tags, so write-back must have been
        // skipped entirely rather than hanging on stdin.
        let writes = TuneWriteRow::list_for_run(&pool, runs[0].id).await.unwrap();
        assert!(writes.is_empty());
    }

    /// Every range/direction override is CLI-supplied below, so `read_initial_values` never
    /// reads them from the driver; the mock only ever needs to answer for `pv_ini`/`mv_ini`
    /// and (for the Yokogawa template) has no mode/mode-attribute tags to read either. Fails
    /// starting at the 5th `read` RPC call — comfortably past every possible setup read —
    /// so the failure always lands on the first polling tick's PV read, deep inside
    /// `run_polling_loop`, not during setup. Also covers `db-run-request-snapshot`'s opcda
    /// path: `prepare` records the resolved connection and the request snapshot before the
    /// polling loop ever runs, so both must already be persisted on the row even though this
    /// run goes on to fail.
    #[tokio::test]
    async fn run_with_opcda_driver_fails_mid_poll_and_marks_the_run_failed() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

        let (host, server) = start_mock_server(
            MockBridgeService {
                read_response: ReadResponse {
                    values: vec![ProtoTagValue {
                        tag_id: "ignored".to_string(),
                        value: "50".to_string(),
                        quality: "Good".to_string(),
                        timestamp: "2024-01-15 10:23:45".to_string(),
                    }],
                },
                write_response: WriteResponse {
                    tag_id: "ignored".to_string(),
                    success: true,
                    error: None,
                },
                ..Default::default()
            }
            .failing_read_from_call(5),
        )
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = Some(host.clone());
        args.server = Some("MockServer".to_string());

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("driver operation failed"));

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Failed);
        assert_eq!(runs[0].driver, bhtune_db::models::TuneDriver::Opcda);
        assert!(
            runs[0]
                .failure_reason
                .as_deref()
                .unwrap()
                .contains("driver operation failed")
        );

        // `record_connection` runs inside `prepare`, before the polling loop that goes on
        // to fail -- so the resolved connection and the submitted request must already be
        // persisted even though the run itself never completes (`db-run-request-snapshot`).
        assert_eq!(runs[0].opc_server.as_deref(), Some("MockServer"));
        assert_eq!(runs[0].bridge_host.as_deref(), Some(host.as_str()));
        let request: serde_json::Value = serde_json::from_str(&runs[0].request_json).unwrap();
        assert_eq!(request["tagname"], "Unit1.LIC101.PV");
        assert_eq!(request["driver"], "opcda");
        assert_eq!(request["server"], "MockServer");

        server.shutdown().await;
    }

    /// Proves `run()` actually resolves `bridge_host`/`server` from `app_config` (not just
    /// from `TuneArgs`) by leaving both CLI-facing fields unset and supplying them only via
    /// the config -- if resolution didn't happen, `driver::build` would either fail fast
    /// with "no OPC server specified" (server never resolved) or try to dial
    /// `DEFAULT_BRIDGE_HOST` instead of the mock (bridge_host never resolved), producing a
    /// different failure than the one asserted below. The mock is configured to fail
    /// starting on its very first `read` call so this stays a fast, deterministic setup
    /// failure -- there is no wall-clock timeout in `run_polling_loop` yet (that lands in
    /// `cli-safety`), so a config-resolution bug that instead let the run reach a real
    /// polling loop against a frozen PV value would hang this test forever rather than
    /// fail cleanly.
    #[tokio::test]
    async fn run_resolves_bridge_host_and_server_from_config_when_cli_flags_are_unset() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue};

        let (host, server) = start_mock_server(
            MockBridgeService {
                read_response: ReadResponse {
                    values: vec![ProtoTagValue {
                        tag_id: "ignored".to_string(),
                        value: "50".to_string(),
                        quality: "Good".to_string(),
                        timestamp: "2024-01-15 10:23:45".to_string(),
                    }],
                },
                ..Default::default()
            }
            .failing_read_from_call(1),
        )
        .await;

        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = None;
        args.server = None;

        let app_config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            server: Some("MockServer".to_string()),
            ..Default::default()
        };

        // A "driver operation failed" error (rather than "no OPC server specified" or a
        // connection error against the unresolved default host) proves setup got as far as
        // issuing a real RPC against the config-resolved mock server.
        let err = run(&pool, args, &app_config).await.unwrap_err();
        assert!(err.to_string().contains("driver operation failed"));

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_errors_when_opcda_server_is_unset_in_both_cli_and_config() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.server = None;

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("no OPC server specified"));
    }

    /// `--mrft-delay` is whole seconds (the smallest non-zero value costs ~1s of real
    /// wall-clock time, both before the test starts switching and after it completes), so
    /// this is deliberately the one slower test in the suite -- there is no way to fast
    /// forward `Utc::now()`-based padding-window comparisons the way paused `tokio` time
    /// fast-forwards `interval`/`sleep`.
    #[tokio::test]
    async fn mrft_delay_pads_the_run_with_extra_recorded_samples() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.mrft_delay = 1;
        run(&pool, args, &test_config()).await.unwrap();

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Completed);

        // ~1s of pre-test padding plus ~1s of post-test padding at a 5ms poll interval is on
        // the order of 400 padding ticks alone, dwarfing the handful of ticks the actual
        // (near-instant) MRFT switching test itself takes -- so a generous lower bound
        // safely distinguishes "padding samples were recorded" from "they weren't".
        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(samples.len() > 100);
    }

    #[tokio::test]
    async fn unknown_template_is_a_clean_error() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.template = "Does Not Exist".to_string();
        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("Does Not Exist"));
    }

    #[test]
    fn build_loop_config_rejects_pid_for_a_non_temperature_process_type() {
        let mut args = fast_simulator_args();
        args.controller_type = ControllerTypeArg::Pid;
        args.process_type = ProcessTypeArg::Flow;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("Pid"));
    }

    #[test]
    fn build_loop_config_uses_process_type_defaults_when_unset() {
        let mut args = fast_simulator_args();
        args.cycles_skip = None;
        args.cycles_count = None;
        args.noise_protection_secs = None;
        let config = build_loop_config(&args).unwrap();
        assert_eq!(
            config.num_cycles_skip,
            ProcessType::Flow.default_cycles_skip()
        );
        assert_eq!(
            config.num_cycles_count,
            ProcessType::Flow.default_cycles_test()
        );
        assert_eq!(
            config.noise_protection_secs,
            ProcessType::Flow.default_noise_protection_secs()
        );
    }

    #[test]
    fn build_loop_config_rejects_an_out_of_range_relay_amp_before_any_driver_or_db_io() {
        // Mirrors the `--write-pid`-requires-`--yes` fail-fast precedent: a bad
        // `--relay-amp` (including a leftover legacy debug-code magic number like 2014) must
        // be caught by `LoopConfig::validate` here, at construction time, not discovered
        // later against a live driver.
        let mut args = fast_simulator_args();
        args.relay_amp = 2014.0;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("relay amplitude 2014"));
        assert!(err.to_string().contains("out of range"));
    }

    #[test]
    fn build_loop_config_rejects_a_relay_amp_below_the_minimum() {
        let mut args = fast_simulator_args();
        args.relay_amp = 0.0;
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("out of range"));
    }

    /// The reproduced panic: `--cycles-count 0` used to reach
    /// `bhtune_core::measure_oscillation`'s internal `assert!` and panic mid-run, after the
    /// loop had already been switched to manual and stroked. `LoopConfig::validate` (called
    /// from `build_loop_config`, before any driver or DB I/O) must reject it cleanly
    /// instead. The clap-level `positive_u32` parser (see `args.rs`) also rejects `0` for
    /// this flag before it ever reaches here, but this test exercises the model-level
    /// guarantee directly, independent of how the value arrived.
    #[test]
    fn build_loop_config_rejects_zero_cycles_count_before_any_driver_or_db_io() {
        let mut args = fast_simulator_args();
        args.cycles_count = Some(0);
        let err = build_loop_config(&args).unwrap_err();
        assert!(err.to_string().contains("at least 1"));
    }

    #[test]
    fn build_loop_tags_simulator_requires_all_overrides() {
        let template = bhtune_core::built_in_templates().remove(0);
        let mut args = fast_simulator_args();
        args.pv_range_high = None;
        let err = build_loop_tags(&args, &template).unwrap_err();
        assert!(err.to_string().contains("--pv-range-high"));
    }

    #[test]
    fn build_loop_tags_simulator_requires_every_override_individually() {
        // Each of the 4 remaining mandatory simulator overrides has its own `ok_or_else`
        // error message; clearing exactly one at a time (rather than just the first, as
        // above) exercises each closure and confirms the flag name in every message.
        type ClearFn = fn(&mut TuneArgs);
        let template = bhtune_core::built_in_templates().remove(0);
        let cases: &[(&str, ClearFn)] = &[
            ("--pv-range-low", |a| a.pv_range_low = None),
            ("--mv-range-high", |a| a.mv_range_high = None),
            ("--mv-range-low", |a| a.mv_range_low = None),
            ("--direction", |a| a.direction = None),
        ];
        for (flag, clear) in cases {
            let mut args = fast_simulator_args();
            clear(&mut args);
            let err = build_loop_tags(&args, &template).unwrap_err();
            assert!(
                err.to_string().contains(flag),
                "expected error for missing {flag}, got: {err}"
            );
        }
    }

    #[test]
    fn build_loop_tags_simulator_uses_fixed_tag_names() {
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let tags = build_loop_tags(&args, &template).unwrap();
        assert_eq!(tags.process_variable, SIMULATOR_PV_TAG);
        assert_eq!(tags.manipulated_variable, SIMULATOR_MV_TAG);
        assert!(tags.controller_mode.is_none());
        assert!(tags.proportional_constant.is_none());
    }

    #[test]
    fn build_loop_tags_opcda_derives_and_applies_overrides() {
        let template = bhtune_core::built_in_templates().remove(0);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.direction = Some(DirectionArg::Direct);
        args.tag_overrides = Some(TagOverrides {
            manipulated_variable: Some("Unit1.LIC101.PY".to_string()),
            proportional_constant: Some("Unit1.LIC101.PB".to_string()),
            ..TagOverrides::default()
        });
        let tags = build_loop_tags(&args, &template).unwrap();
        assert!(tags.process_variable.starts_with("Unit1.LIC101"));
        assert_eq!(tags.manipulated_variable, "Unit1.LIC101.PY");
        assert_eq!(
            tags.proportional_constant,
            Some("Unit1.LIC101.PB".to_string())
        );
        assert_eq!(
            tags.controller_direction,
            TagOrValue::Value(ControllerDirection::Direct)
        );
        assert_eq!(tags.upper_pv_range, TagOrValue::Value(100.0));
    }

    #[tokio::test]
    async fn a_ctrl_c_style_abort_restores_and_records_aborted() {
        // Exercises the DB/restore shape of an abort directly -- calling `restore` +
        // `TuneRunRow::abort` exactly as the real Ctrl+C path does -- rather than going
        // through a full `run_polling_loop`/`run_with_ctrl_c` cycle. `CtrlC::test_pair()` can
        // and does fake a real signal end to end elsewhere (see
        // `a_stalled_mv_write_during_a_tick_is_cancelled_and_still_records_the_sample` and
        // `run_with_ctrl_c_aborts_the_run_when_signalled_during_the_poll`, both further down
        // in this module); this test is kept as a narrower, cheaper check of just the
        // resulting database row shape.
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let driver = crate::driver::build(&args).await.unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "abort-test",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = read_initial_values(driver.as_ref(), &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(driver.as_ref(), &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let report = restore(driver.as_ref(), &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());
        TuneRunRow::abort(&pool, run.id, Utc::now()).await.unwrap();

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
    }

    /// Unlike Ctrl+C/timeout (which need a real signal or elapsed wall-clock time and so are
    /// only exercised indirectly, see the test above), finding 5's `PoorQuality` abort is
    /// purely data-driven -- the driver just has to report a non-`Good` reading -- so this
    /// test drives `run_polling_loop` for real and checks its returned `PollOutcome`
    /// directly, then confirms `restore` leaves the loop in the same consistent state
    /// `execute`'s `Aborted` branch would.
    #[tokio::test]
    async fn poor_quality_pv_during_polling_aborts_records_the_sample_and_restores() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "poor-quality-poll",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        // Built directly (bypassing `read_initial_values`) using `honeywell_driver_auto()`'s
        // own fixture values (see `sample_initial_state`, defined below), because
        // `read_initial_values` itself enforces finding 5 on this very same PV tag and would
        // hard-fail before ever reaching the polling loop this test targets.
        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            started_at,
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
        )
        .await
        .unwrap();

        match outcome {
            PollOutcome::Aborted(AbortReason::PoorQuality { tag, quality }) => {
                assert_eq!(tag, tags.process_variable);
                assert_eq!(quality, bhtune_driver::Quality::Bad);
            }
            _ => panic!("expected PollOutcome::Aborted(AbortReason::PoorQuality)"),
        }

        // The triggering sample was recorded (with its real, poor quality) before the abort
        // -- finding 5 explicitly requires the operator can see exactly what was seen when
        // the run gave up, not just that it gave up.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].pv_quality, SampleQuality::Bad);

        // Same DB-shape check the Ctrl+C-style test above uses: a `PoorQuality` abort must
        // be indistinguishable in its cleanup guarantees from any other abort reason. Uses a
        // bare default `MutationGuard` -- `transition_to_manual` was never called on this
        // path -- which is exactly what proves `restore`'s MV-revert step is unconditional
        // and never gated by the guard: it still succeeds and writes the MV back even though
        // `guard.mv_written` is `false`, while the guard-gated mode/setpoint/mode-attribute
        // steps correctly report `NotNeeded` since nothing was ever attempted for them.
        let guard = MutationGuard::default();
        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert_eq!(report.mv, RestoreStepOutcome::Succeeded);
        assert_eq!(report.mode, RestoreStepOutcome::NotNeeded);
        assert_eq!(report.setpoint, RestoreStepOutcome::NotNeeded);
        assert_eq!(report.mode_attribute, RestoreStepOutcome::NotNeeded);
        TuneRunRow::abort(&pool, run.id, Utc::now()).await.unwrap();

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);

        // The MV was actually written back to its initial value during restore.
        assert!(
            driver
                .write_log()
                .iter()
                .any(|(tag, value)| tag == &tags.manipulated_variable && value == "45")
        );
    }

    // --- safety-cancellation: `--op-timeout-secs` / mid-tick Ctrl+C via `bounded_driver_call`

    /// Proves the wiring, not just the mechanism (see the dedicated `bounded_driver_call`
    /// unit tests below for that): a PV read that never resolves at all -- the gateway is
    /// down, DCOM is wedged, the network is black-holed -- must abort the run via
    /// `--op-timeout-secs` rather than hang the poll loop forever, exactly the scenario
    /// finding 2 of the live-plant safety review names as the most severe of the three
    /// consequences of the pre-`safety-cancellation` design. Real (unpaused) time, paying a
    /// real ~1s wall-clock cost: `start_paused` interacts badly with the real sqlx
    /// `SqlitePool` this test also creates (it fast-forwards the pool's own internal
    /// connection-acquire timeout too), matching the documented precedent in
    /// `run_times_out_and_aborts_when_timeout_secs_elapses_before_completion` below.
    #[tokio::test]
    async fn a_stalled_pv_read_aborts_the_poll_loop_via_op_timeout_secs() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_read(&tags.process_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 1;
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-pv-read",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            started_at,
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
        )
        .await
        .unwrap();

        match outcome {
            PollOutcome::Aborted(AbortReason::OperationTimedOut {
                tag,
                op_timeout_secs,
            }) => {
                assert_eq!(tag, tags.process_variable);
                assert_eq!(op_timeout_secs, 1);
            }
            _ => panic!("expected PollOutcome::Aborted(AbortReason::OperationTimedOut)"),
        }

        // Unlike the poor-quality/mid-write-cancellation cases, the PV read itself is what
        // stalled -- there is no valid tick/sample to record for this iteration at all.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert!(samples.is_empty());
    }

    /// The write-side counterpart of the read-stall test above, and also the one integration
    /// test exercising a mid-tick Ctrl+C (as opposed to the pre-existing idle-between-ticks
    /// coverage in `a_ctrl_c_style_abort_restores_and_records_aborted`): the PV read for tick
    /// 1 succeeds normally (so a real `tick`/`sample_quality` is in hand), the engine's very
    /// first `step` call always emits `Action::WriteMv` (see `MrftEngine::switch_is_needed`:
    /// `hysteresis` is still zero and `counter_all_switches == 0` on tick 1), and that write
    /// then hangs forever via `hanging_write`. A background task -- standing in for a human
    /// pressing Ctrl+C mid-write, which a unit test can't do with a real signal without
    /// hitting every other concurrently running test (see `tests/ctrlc_abort.rs`'s doc
    /// comment) -- sends the cancellation a short real delay later. Deliberately *not*
    /// `start_paused`: the delay has to be observed as "genuinely still in flight" by a
    /// concurrently running task, which paused virtual time (where nothing advances until
    /// every task is parked) can't model as naturally as a small real sleep.
    #[tokio::test]
    async fn a_stalled_mv_write_during_a_tick_is_cancelled_and_still_records_the_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        // Large enough that the op-timeout branch never wins the race against the
        // background task's much shorter real delay below.
        args.op_timeout_secs = 30;
        let config = build_loop_config(&args).unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-mv-write",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();

        let initial = sample_initial_state();
        let beta = lookup(
            config.process_type,
            config.controller_type,
            ResponseLevel::Aggressive,
        )
        .beta;
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            started_at,
            &mut ctrl_c,
            &mut MutationGuard::default(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));

        // Unlike the read-stall case, a valid sample for this tick was already in hand
        // before the write was attempted, so it must still be recorded before aborting.
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);

        // The hung write never actually completed -- the loop was left at its relay-test MV,
        // matching `warn_restore_incomplete`'s premise for why the operator warning names
        // the MV specifically.
        assert!(driver.write_log().is_empty());
    }

    // --- opcda-style mode-transition/restore/write-back coverage ---------------------------
    //
    // The tests above all use the simulator driver, whose `LoopTags` has no
    // setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags`), so they
    // never exercise `transition_to_manual`/`restore`/`maybe_write_back`'s real opcda-style
    // logic. `MockDriver` below is a minimal in-memory `Driver` double with a configurable
    // tag/value map, used together with the real "Honeywell Experion" built-in template
    // (which has every optional tag suffix configured) to drive that logic directly.

    /// A minimal, fully in-memory [`Driver`] test double with a fixed tag-value map, plus
    /// the ability to inject specific-tag read/write failures. `std::sync::Mutex`, not
    /// `tokio::sync::Mutex` — matching `SimulatorDriver`'s own precedent — since no
    /// `.await` point is ever held across the lock.
    #[derive(Default)]
    struct MockDriver {
        values: std::sync::Mutex<std::collections::HashMap<String, String>>,
        writes: std::sync::Mutex<Vec<(String, String)>>,
        reject_writes: std::collections::HashSet<String>,
        error_reads: std::collections::HashSet<String>,
        error_writes: std::collections::HashSet<String>,
        empty_reads: std::collections::HashSet<String>,
        /// Tags whose `read`/`write` never resolves (`.await`s `std::future::pending`
        /// forever), simulating a stalled OPC DA call (gateway down, DCOM wedged, network
        /// black-holed) so `--op-timeout-secs`/Ctrl+C-during-a-tick can actually be exercised
        /// -- every other mock read/write body resolves synchronously and has no real
        /// `.await` point, so it can never be caught mid-flight by a racing timeout/cancel.
        hang_reads: std::collections::HashSet<String>,
        hang_writes: std::collections::HashSet<String>,
        /// Per-tag OPC quality override, defaulting to `Quality::Good` for any tag not
        /// listed -- matching a healthy real driver and letting most tests ignore quality
        /// entirely while a handful exercise finding 5's enforcement via `with_quality`.
        qualities: std::sync::Mutex<std::collections::HashMap<String, bhtune_driver::Quality>>,
        /// Per-tag: reports the tag's ordinarily-configured quality (from `qualities`,
        /// defaulting to `Good`) for the tag's first `usize` reads, then switches to the
        /// paired [`bhtune_driver::Quality`] for every read after that. Lets a test put a
        /// tag's *initial* read (before any mutation is attempted, subject to finding 5 the
        /// same as every other read) in good standing while still forcing quality to
        /// degrade partway through polling -- deterministically, with no reliance on real
        /// elapsed time or a Ctrl+C race, unlike `--timeout-secs`/`--op-timeout-secs`-driven
        /// aborts.
        degrade_quality_after: std::collections::HashMap<String, (usize, bhtune_driver::Quality)>,
        /// Tracks how many times each tag has been read so far, for
        /// `degrade_quality_after`/`erroring_read_after`.
        read_counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
        /// Per-tag: the tag's first `usize` reads resolve normally, then every read after
        /// that returns a transport-level error -- the same "succeeds at first, degrades
        /// partway through" shape as `degrade_quality_after`, but a hard read error rather
        /// than a quality downgrade, so `safety-writeback-rollback`'s pre-read-succeeds/
        /// verify-readback-errors path can be exercised distinctly from the
        /// verify-readback-reports-poor-quality path.
        error_reads_after: std::collections::HashMap<String, usize>,
        /// Tags whose float writes are silently perturbed by a fixed offset before being
        /// stored, simulating a DCS that clamps/rounds a written value rather than accepting
        /// it exactly -- lets a test exercise `pid_value_within_tolerance`'s rejection path
        /// deterministically.
        write_offsets: std::collections::HashMap<String, f32>,
        /// Per-tag: the tag's first `usize` writes are accepted normally, then every write
        /// after that is rejected (mirrors `reject_writes`'s `WriteOutcome::failure`, not a
        /// transport error). Lets a test make a constant's *forward* write succeed while its
        /// later *rollback* write (the same tag, written a second time with the previous
        /// value) is rejected -- exercising `rollback_state = Failed`.
        reject_writes_after: std::collections::HashMap<String, usize>,
        /// Tracks how many times each tag has been written so far, for
        /// `reject_writes_after`.
        write_counts: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    }

    impl MockDriver {
        fn new(values: &[(&str, &str)]) -> MockDriver {
            MockDriver {
                values: std::sync::Mutex::new(
                    values
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                ),
                ..Default::default()
            }
        }

        fn rejecting_write(mut self, tag: &str) -> MockDriver {
            self.reject_writes.insert(tag.to_string());
            self
        }

        fn erroring_read(mut self, tag: &str) -> MockDriver {
            self.error_reads.insert(tag.to_string());
            self
        }

        fn erroring_write(mut self, tag: &str) -> MockDriver {
            self.error_writes.insert(tag.to_string());
            self
        }

        fn empty_read(mut self, tag: &str) -> MockDriver {
            self.empty_reads.insert(tag.to_string());
            self
        }

        /// Makes reading `tag` hang forever -- see the `hang_reads` field doc comment.
        fn hanging_read(mut self, tag: &str) -> MockDriver {
            self.hang_reads.insert(tag.to_string());
            self
        }

        /// Makes writing `tag` hang forever -- see the `hang_writes` field doc comment.
        fn hanging_write(mut self, tag: &str) -> MockDriver {
            self.hang_writes.insert(tag.to_string());
            self
        }

        /// Overrides a single tag's fixture value -- e.g. to make an otherwise-valid
        /// baseline driver (like `honeywell_driver_auto()`) report one bad reading.
        fn with_value(self, tag: &str, value: &str) -> MockDriver {
            self.values
                .lock()
                .unwrap()
                .insert(tag.to_string(), value.to_string());
            self
        }

        /// Overrides a single tag's reported [`bhtune_driver::Quality`] -- every other tag
        /// keeps reporting `Quality::Good`, matching a healthy real driver.
        fn with_quality(self, tag: &str, quality: bhtune_driver::Quality) -> MockDriver {
            self.qualities
                .lock()
                .unwrap()
                .insert(tag.to_string(), quality);
            self
        }

        /// See the `degrade_quality_after` field doc comment: `tag`'s first `good_reads`
        /// reads keep reporting `Good` (or whatever `with_quality` set), then every read
        /// after that reports `degraded` instead.
        fn degrade_quality_after(
            mut self,
            tag: &str,
            good_reads: usize,
            degraded: bhtune_driver::Quality,
        ) -> MockDriver {
            self.degrade_quality_after
                .insert(tag.to_string(), (good_reads, degraded));
            self
        }

        /// See the `error_reads_after` field doc comment: `tag`'s first `good_reads` reads
        /// succeed normally, then every read after that returns a transport-level error.
        fn erroring_read_after(mut self, tag: &str, good_reads: usize) -> MockDriver {
            self.error_reads_after.insert(tag.to_string(), good_reads);
            self
        }

        /// See the `write_offsets` field doc comment: writing a float to `tag` silently
        /// stores `value + offset` instead of `value`, so a subsequent readback observes a
        /// value that differs from what was requested.
        fn distorting_write(mut self, tag: &str, offset: f32) -> MockDriver {
            self.write_offsets.insert(tag.to_string(), offset);
            self
        }

        /// See the `reject_writes_after` field doc comment: `tag`'s first `good_writes`
        /// writes are accepted normally, then every write after that is rejected.
        fn rejecting_write_after(mut self, tag: &str, good_writes: usize) -> MockDriver {
            self.reject_writes_after
                .insert(tag.to_string(), good_writes);
            self
        }

        fn value_of(&self, tag: &str) -> Option<String> {
            self.values.lock().unwrap().get(tag).cloned()
        }

        fn write_log(&self) -> Vec<(String, String)> {
            self.writes.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Driver for MockDriver {
        async fn read(
            &self,
            tags: &[String],
        ) -> bhtune_driver::DriverResult<Vec<bhtune_driver::TagValue>> {
            if tags.iter().any(|tag| self.hang_reads.contains(tag)) {
                std::future::pending::<()>().await;
            }
            let store = self.values.lock().unwrap();
            let mut out = Vec::new();
            for tag in tags {
                if self.error_reads.contains(tag) {
                    return Err(bhtune_driver::DriverError::Operation(Box::new(
                        std::io::Error::other("mock read error"),
                    )));
                }
                if self.empty_reads.contains(tag) {
                    continue;
                }
                // Shared per-tag read counter, consulted by both `degrade_quality_after` and
                // `erroring_read_after` -- each test only ever registers a tag in one of the
                // two, but counting once keeps the two mechanisms consistent if that changed.
                let count = {
                    let mut counts = self.read_counts.lock().unwrap();
                    let count = counts.entry(tag.clone()).or_insert(0);
                    *count += 1;
                    *count
                };
                if let Some(good_reads) = self.error_reads_after.get(tag)
                    && count > *good_reads
                {
                    return Err(bhtune_driver::DriverError::Operation(Box::new(
                        std::io::Error::other("mock read error after good reads"),
                    )));
                }
                let baseline_quality = self
                    .qualities
                    .lock()
                    .unwrap()
                    .get(tag)
                    .copied()
                    .unwrap_or(bhtune_driver::Quality::Good);
                let quality = match self.degrade_quality_after.get(tag) {
                    Some((good_reads, degraded)) => {
                        if count > *good_reads {
                            *degraded
                        } else {
                            baseline_quality
                        }
                    }
                    None => baseline_quality,
                };
                out.push(bhtune_driver::TagValue {
                    tag: tag.clone(),
                    value: store.get(tag).cloned().unwrap_or_default(),
                    quality,
                    timestamp: None,
                });
            }
            Ok(out)
        }

        async fn write(
            &self,
            tag: &String,
            value: TagWrite,
        ) -> bhtune_driver::DriverResult<bhtune_driver::WriteOutcome> {
            if self.hang_writes.contains(tag) {
                std::future::pending::<()>().await;
            }
            if self.error_writes.contains(tag) {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock write error"),
                )));
            }
            let text = match &value {
                TagWrite::Float(f) => f.to_string(),
                TagWrite::Raw(s) => s.clone(),
            };
            self.writes
                .lock()
                .unwrap()
                .push((tag.clone(), text.clone()));
            if self.reject_writes.contains(tag) {
                return Ok(bhtune_driver::WriteOutcome::failure("mock rejected write"));
            }
            if let Some(good_writes) = self.reject_writes_after.get(tag) {
                let mut counts = self.write_counts.lock().unwrap();
                let count = counts.entry(tag.clone()).or_insert(0);
                *count += 1;
                if *count > *good_writes {
                    return Ok(bhtune_driver::WriteOutcome::failure(
                        "mock rejected write after good writes",
                    ));
                }
            }
            // Store the (possibly silently distorted) value that a subsequent read would
            // observe, while `writes` above kept a log of what was actually requested -- see
            // the `write_offsets` field doc comment.
            let stored = if let TagWrite::Float(f) = value {
                (f + self.write_offsets.get(tag).copied().unwrap_or(0.0)).to_string()
            } else {
                text
            };
            self.values.lock().unwrap().insert(tag.clone(), stored);
            Ok(bhtune_driver::WriteOutcome::success())
        }

        async fn browse(
            &self,
            _path: &str,
        ) -> bhtune_driver::DriverResult<Vec<bhtune_driver::TagNode>> {
            Err(bhtune_driver::DriverError::Unsupported {
                operation: "browse",
            })
        }
    }

    #[tokio::test]
    async fn mock_driver_browse_is_unsupported() {
        // `tune`'s own logic never calls `Driver::browse` -- this only exists so
        // `MockDriver` satisfies the trait -- but it should still honor the same
        // "unsupported, not a panic" convention real drivers document for it.
        let err = MockDriver::new(&[]).browse("").await.unwrap_err();
        assert!(matches!(
            err,
            bhtune_driver::DriverError::Unsupported {
                operation: "browse"
            }
        ));
    }

    // --- `check_quality`: finding 5's single enforcement choke point ------------------------

    #[test]
    fn check_quality_accepts_good_regardless_of_the_allow_uncertain_flag() {
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, false).is_ok());
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, true).is_ok());
    }

    #[test]
    fn check_quality_rejects_uncertain_unless_the_flag_is_set() {
        let err =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, false).unwrap_err();
        assert!(err.to_string().contains("Uncertain"));
        assert!(err.to_string().contains("Unit1.LIC101.PV"));
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, true).is_ok());
    }

    #[test]
    fn check_quality_never_accepts_bad_regardless_of_the_flag() {
        let err_without_flag =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Bad, false).unwrap_err();
        assert!(err_without_flag.to_string().contains("Bad"));
        let err_with_flag =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Bad, true).unwrap_err();
        assert!(err_with_flag.to_string().contains("Bad"));
    }

    // --- `pid_value_within_tolerance`: finding 6's write-back confirmation rule -------------

    #[test]
    fn pid_value_within_tolerance_accepts_an_exact_match() {
        assert!(pid_value_within_tolerance(10.0, 10.0));
        assert!(pid_value_within_tolerance(0.0, 0.0));
    }

    #[test]
    fn pid_value_within_tolerance_accepts_within_the_one_percent_relative_band() {
        // 1% of 10.0 is 0.1, so 10.09 is inside and 10.2 is outside.
        assert!(pid_value_within_tolerance(10.0, 10.09));
        assert!(pid_value_within_tolerance(10.0, 9.91));
        assert!(!pid_value_within_tolerance(10.0, 10.2));
        assert!(!pid_value_within_tolerance(10.0, 9.8));
    }

    #[test]
    fn pid_value_within_tolerance_uses_the_absolute_floor_near_zero() {
        // 1% of a requested 0.0 (or anything smaller than 0.1) would be a tolerance under
        // 1e-3, which would reject even a harmless floating-point rounding difference -- the
        // absolute 1e-3 floor exists precisely for a requested `D = 0` on a PI controller.
        assert!(pid_value_within_tolerance(0.0, 0.0009));
        assert!(!pid_value_within_tolerance(0.0, 0.002));
    }

    #[test]
    fn pid_value_within_tolerance_handles_negative_requested_values() {
        // The tolerance formula uses `requested.abs()`, so the relative band is symmetric
        // around a negative requested value too (relevant for reverse-acting controllers'
        // sign conventions).
        assert!(pid_value_within_tolerance(-10.0, -10.09));
        assert!(!pid_value_within_tolerance(-10.0, -10.2));
    }

    /// "Honeywell Experion" is the one built-in template with every optional tag (setpoint,
    /// mode, mode attribute, PID constants) configured, making it the right fixture for
    /// exercising every opcda-style branch in one place.
    fn honeywell_template() -> DcsTemplate {
        bhtune_core::built_in_templates()
            .into_iter()
            .find(|t| t.name == "Honeywell Experion")
            .expect("Honeywell Experion is a built-in template")
    }

    fn honeywell_tags() -> LoopTags {
        LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &honeywell_template())
    }

    /// A `MockDriver` pre-populated with every tag `honeywell_tags()` derives, using values
    /// that make the loop initially Auto (`MODE=1`) with its Mode Attribute not yet at the
    /// Program value (`MODEATTR=1`, program value is `"2"`) — the common starting point most
    /// of the tests below share before diverging.
    fn honeywell_driver_auto() -> MockDriver {
        MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "1"),
            ("Unit1.LIC101.MODEATTR", "1"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
            ("Unit1.LIC101.K", "10.0"),
            ("Unit1.LIC101.T1", "2.0"),
            ("Unit1.LIC101.T2", "0.5"),
        ])
    }

    #[tokio::test]
    async fn read_initial_values_reads_the_full_opcda_tag_set() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        assert_eq!(initial.pv_ini, 50.0);
        assert_eq!(initial.mv_ini, 45.0);
        assert_eq!(initial.pv_range_high, 100.0);
        assert_eq!(initial.pv_range_low, 0.0);
        assert_eq!(initial.mv_range_high, 100.0);
        assert_eq!(initial.mv_range_low, 0.0);
        assert_eq!(initial.direction, ControllerDirection::Direct);
        assert_eq!(initial.mode_raw.as_deref(), Some("1"));
        assert_eq!(initial.mode_attribute_raw.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn read_initial_values_errors_when_a_tag_returns_no_value() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().empty_read("Unit1.LIC101.PV");

        let err = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no value"));
    }

    /// Matches `honeywell_driver_auto()`'s values -- a valid baseline for
    /// `validate_initial_state` tests to mutate one field at a time from. `setpoint_ini` is
    /// `Some(55.0)`, matching the fixture's `SP` tag, since `mode_raw` ("1") equals the
    /// template's `mode_auto_value` -- exactly the condition `read_initial_values` itself
    /// checks before reading the setpoint.
    fn sample_initial_state() -> InitialState {
        InitialState {
            pv_ini: 50.0,
            mv_ini: 45.0,
            pv_range_high: 100.0,
            pv_range_low: 0.0,
            mv_range_high: 100.0,
            mv_range_low: 0.0,
            direction: ControllerDirection::Direct,
            mode_raw: Some("1".to_string()),
            mode_attribute_raw: Some("1".to_string()),
            setpoint_ini: Some(55.0),
        }
    }

    #[test]
    fn validate_initial_state_accepts_a_typical_reading() {
        assert!(validate_initial_state(&sample_initial_state()).is_ok());
    }

    #[test]
    fn validate_initial_state_rejects_a_zero_span_pv_range() {
        let mut initial = sample_initial_state();
        initial.pv_range_high = 50.0;
        initial.pv_range_low = 50.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("PV range"));
    }

    #[test]
    fn validate_initial_state_rejects_an_mv_range_with_low_not_below_high() {
        let mut initial = sample_initial_state();
        initial.mv_range_high = 0.0;
        initial.mv_range_low = 100.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("MV range"));
    }

    #[test]
    fn validate_initial_state_rejects_equal_mv_range_bounds() {
        let mut initial = sample_initial_state();
        initial.mv_range_high = 50.0;
        initial.mv_range_low = 50.0;
        assert!(validate_initial_state(&initial).is_err());
    }

    #[test]
    fn validate_initial_state_rejects_an_initial_mv_outside_the_mv_range() {
        let mut initial = sample_initial_state();
        initial.mv_ini = 150.0;
        let err = validate_initial_state(&initial).unwrap_err();
        assert!(err.to_string().contains("outside the MV range"));
    }

    #[test]
    fn validate_initial_state_accepts_the_initial_mv_on_the_range_boundary() {
        let mut initial = sample_initial_state();
        initial.mv_ini = initial.mv_range_high;
        assert!(validate_initial_state(&initial).is_ok());
    }

    // --- finding 5, end to end: a poor-quality initial reading must fail `execute` before --
    // --- any mutation of the loop, exactly like finding 4's invalid-range checks below -----

    #[tokio::test]
    async fn execute_hard_fails_when_the_pv_tag_reports_bad_quality() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "bad-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains(&tags.process_variable));
        assert!(err.to_string().contains("Bad"));
        // Nothing was mutated -- the mode transition never ran, matching the invalid-range
        // test's own safety assertion below.
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn execute_hard_fails_when_the_pv_tag_reports_uncertain_quality_without_the_flag() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Uncertain);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "uncertain-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains(&tags.process_variable));
        assert!(err.to_string().contains("Uncertain"));
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn read_initial_values_and_transition_accept_uncertain_pv_quality_when_the_flag_is_set() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Uncertain);

        let initial = read_initial_values(&driver, &tags, &template, true)
            .await
            .unwrap();
        assert_eq!(initial.pv_ini, 50.0);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        // Proves the run actually proceeded to mutate the loop (the mode/mode-attribute
        // writes `transition_to_manual` performs), not just that `read_initial_values`
        // alone returned `Ok` -- the real proof `--allow-uncertain-quality` has an effect,
        // not just that this specific error string disappeared.
        assert!(!driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn read_initial_values_hard_fails_when_the_setpoint_tag_reports_bad_quality() {
        // The Honeywell fixture starts in Auto (`MODE=1` == `mode_auto_value`), so
        // `read_initial_values` reads the setpoint tag as part of computing
        // `InitialState::setpoint_ini` -- finding 5 applies to that read exactly as it does
        // to every other tuning-critical read. This read was hoisted out of
        // `transition_to_manual` (see `InitialState::setpoint_ini`'s doc comment) so it can
        // be persisted before any mutation is attempted; this test moved with it.
        let template = honeywell_template();
        let tags = honeywell_tags();
        let sp_tag = tags.setpoint_variable.clone().unwrap();
        let driver = honeywell_driver_auto().with_quality(&sp_tag, bhtune_driver::Quality::Bad);

        let err = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(&sp_tag));
        assert!(err.to_string().contains("Bad"));
    }

    /// The end-to-end proof that finding 4's fix closes the actual safety gap, not just the
    /// isolated unit: a driver reporting an MV range with `low >= high` must fail `execute`
    /// before `transition_to_manual`'s first write -- i.e. before the loop is touched at
    /// all, not merely before the tuning math runs.
    #[tokio::test]
    async fn execute_rejects_an_invalid_mv_range_before_any_mutation_of_the_loop() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // Swaps CVEUHI/CVEULO so mv_range_high (0.0) < mv_range_low (100.0), violating
        // MvRange::new's low-strictly-below-high requirement.
        let driver = honeywell_driver_auto()
            .with_value("Unit1.LIC101.CVEUHI", "0.0")
            .with_value("Unit1.LIC101.CVEULO", "100.0");
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "invalid-mv-range",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("MV range"));
        // The real safety property: no write ever reached the driver -- not the mode
        // attribute, not the mode, not the MV. `transition_to_manual` never ran.
        assert!(driver.write_log().is_empty());
    }

    /// Covers `execute`'s first `restore_best_effort_then_propagate` call site: a failure
    /// from `transition_to_manual` itself, partway through (the mode-attribute write, the
    /// very first write it attempts) must still trigger a best-effort restore rather than
    /// propagating the error with the loop left half-mutated. `honeywell_driver_auto()`
    /// starts in Auto with `MODEATTR` not yet at the Program value, so `transition_to_manual`
    /// always attempts the mode-attribute write first.
    #[tokio::test]
    async fn execute_attempts_restore_when_transition_to_manual_fails_partway() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().erroring_write("Unit1.LIC101.MODEATTR");
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "transition-to-manual-fails",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        let err = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        // `restore_best_effort_then_propagate` always returns the *original* error
        // unchanged -- this is that original `transition_to_manual` failure, not some
        // restore-side error masking it.
        assert!(err.to_string().contains("driver operation failed"));

        // `restore()`'s MV step is unconditional (never gated by the guard), so it still ran
        // despite `transition_to_manual` never getting anywhere near the MV -- proving the
        // restore was genuinely attempted, not skipped because "nothing was mutated yet".
        assert!(
            driver
                .write_log()
                .iter()
                .any(|(tag, _)| tag == "Unit1.LIC101.OP")
        );
        // `guard.mode_written` was correctly never armed: `transition_to_manual` failed on
        // the mode-attribute write, before it ever reached the mode write, so `restore()`
        // must not attempt to revert a mode change that was never made.
        assert!(
            driver
                .write_log()
                .iter()
                .all(|(tag, _)| tag != "Unit1.LIC101.MODE")
        );

        // The mode-attribute *restore* step, unlike mode/setpoint, retries the same
        // permanently-erroring tag and fails again -- so the restore is recorded as
        // incomplete, naming exactly which step could not be confirmed.
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
        assert!(
            stored
                .restore_detail
                .as_deref()
                .unwrap_or_default()
                .contains("mode attribute")
        );
    }

    /// Covers `execute`'s second `restore_best_effort_then_propagate` call site: a failure in
    /// `finish_completed_run` (here, `persist_results` colliding with the
    /// `UNIQUE (run_id, response_level)` constraint) *after* a real, successful MRFT
    /// completion must still trigger a best-effort restore. Uses the real `SimulatorDriver`
    /// (via `crate::driver::build`, exactly like `a_ctrl_c_style_abort_restores_and_records_aborted`
    /// above) rather than a scripted `MockDriver`, since this needs an actual engine
    /// completion, not just a mocked one -- the simulator's `LoopTags` has no mode/setpoint/
    /// mode-attribute tags at all, so its restore only ever has the MV step to confirm.
    #[tokio::test]
    async fn execute_attempts_restore_when_finish_completed_run_fails_after_a_successful_test() {
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let driver = crate::driver::build(&args).await.unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "finish-completed-run-fails",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        // Pre-inserts a conflicting row so `persist_results`'s own insert of the same
        // `(run_id, Aggressive)` pair -- once the simulator tune genuinely completes --
        // collides with the `UNIQUE (run_id, response_level)` constraint and fails
        // deterministically, without needing to fake or corrupt anything about the tune
        // itself.
        TuneResultRow::insert(
            &pool,
            &TuneResultRow {
                id: 0,
                run_id: run.id,
                response_level: ResponseLevel::Aggressive,
                kp: 1.0,
                ti_minutes: 1.0,
                td_minutes: 1.0,
                proportional: 1.0,
                integral: 1.0,
                derivative: 1.0,
            },
        )
        .await
        .unwrap();

        let result = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            driver.as_ref(),
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await;

        // Loose assertion by design, matching this codebase's existing convention for
        // constraint-violation tests (see `tests/schema.rs`): SQLite's exact wording for a
        // UNIQUE-constraint violation is version/implementation detail, not part of this
        // test's contract.
        assert!(result.is_err());

        // The simulator driver has no mode/setpoint/mode-attribute tags, so the only
        // applicable restore step is the always-succeeding MV write -- confirming the
        // restore ran to completion despite the DB-level failure that follows it.
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Confirmed)
        );
    }

    /// Covers the `Aborted` branch's `RestoreAttempt::Incomplete` mapping -- the sibling of
    /// the `Completed` branch's equivalent (deliberately not separately covered; see
    /// AGENTS.md's `safety-restore-guard` notes) -- with a real, deterministic abort: the PV
    /// tag's quality degrades to `Bad` starting on the very first poll tick (its one
    /// `read_initial_values` read stays `Good`, via `degrade_quality_after`), and the MV tag
    /// is error-injected so the subsequent restore's unconditional MV step fails while
    /// mode/setpoint/mode-attribute all succeed normally. Deliberately plain `#[tokio::test]`
    /// rather than `start_paused = true`: pairing a paused clock with `seeded_pool()`'s real
    /// `sqlx` connection pool reliably deadlocks (`PoolTimedOut`), so -- matching every other
    /// `execute`-level test in this module that needs a real pool -- this test pays the real
    /// wall-clock cost of `transition_to_manual`/`restore`'s inter-write pacing sleeps
    /// instead (~3s: one in `transition_to_manual`, two more in `restore`).
    #[tokio::test]
    async fn execute_reports_restore_incomplete_after_a_poor_quality_abort() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .erroring_write("Unit1.LIC101.OP")
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "poor-quality-abort-restore-incomplete",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();

        let outcome = execute(
            &pool,
            run.id,
            &fast_simulator_args(),
            &template,
            &tags,
            &driver,
            config,
            Utc::now(),
            None,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        let RunOutcome::RestoreIncomplete { reason } = outcome else {
            panic!("expected RunOutcome::RestoreIncomplete, got {outcome:?}");
        };
        assert!(reason.contains("run aborted"));
        assert!(reason.contains("PoorQuality"));
        assert!(reason.contains("MV"));

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
    }

    #[tokio::test]
    async fn read_f32_errors_on_a_non_numeric_value() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "not-a-number")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a number"));
    }

    /// Rust's `f32::from_str` happily parses the literal strings `"nan"`/`"inf"` --
    /// confirming this gap is what motivated hardening `read_f32` (finding 4 of the
    /// live-plant safety review): a driver tag returning either string used to flow
    /// unchecked into the engine.
    #[tokio::test]
    async fn read_f32_rejects_nan() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "nan")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn read_f32_rejects_infinity() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "inf")]);
        let err = read_f32(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn resolve_f32_accepts_a_finite_tag_or_value() {
        let driver = MockDriver::new(&[]);
        let value = resolve_f32(&driver, &TagOrValue::Value(42.0), false)
            .await
            .unwrap();
        assert_eq!(value, 42.0);
    }

    /// Defense in depth against a hypothetical future caller (e.g. a `bhtune-server` HTTP
    /// handler) constructing a `TagOrValue::Value` directly without going through clap's
    /// `finite_f32` parser at all -- see `args::finite_f32`.
    #[tokio::test]
    async fn resolve_f32_rejects_a_non_finite_direct_value() {
        let driver = MockDriver::new(&[]);
        let err = resolve_f32(&driver, &TagOrValue::Value(f32::NAN), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[tokio::test]
    async fn read_raw_and_write_raw_propagate_a_hard_driver_error() {
        // Distinct from a *rejected* write (`WriteOutcome::success == false`, handled by
        // `write_raw`/`write_value`'s own "was rejected" message): this is the driver call
        // itself failing (`DriverError::Operation`), which `?` should propagate as-is.
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "50.0")])
            .erroring_read("Unit1.LIC101.PV")
            .erroring_write("Unit1.LIC101.OP");

        let read_err = read_raw(&driver, "Unit1.LIC101.PV", false)
            .await
            .unwrap_err();
        assert!(read_err.to_string().contains("driver operation failed"));

        let write_err = write_value(&driver, "Unit1.LIC101.OP", 45.0)
            .await
            .unwrap_err();
        assert!(write_err.to_string().contains("driver operation failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_writes_program_value_and_mode_when_starting_in_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        // The setpoint is captured here, in `read_initial_values`, before any mutation of
        // the loop -- see `InitialState::setpoint_ini`'s doc comment for why -- not during
        // `transition_to_manual` any more.
        assert_eq!(initial.setpoint_ini, Some(55.0));

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        assert_eq!(
            driver.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("2")
        );
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
        assert!(guard.mode_attribute_written);
        assert!(guard.mode_written);
        // Order matters (mode attribute unlocked before the mode itself is switched), per
        // `ChangeControllerModeToMan`.
        let log = driver.write_log();
        let attr_index = log
            .iter()
            .position(|(t, _)| t == "Unit1.LIC101.MODEATTR")
            .unwrap();
        let mode_index = log
            .iter()
            .position(|(t, _)| t == "Unit1.LIC101.MODE")
            .unwrap();
        assert!(attr_index < mode_index);
    }

    #[tokio::test(start_paused = true)]
    async fn read_initial_values_skips_setpoint_capture_when_original_mode_is_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        // "2" is neither the manual ("0") nor auto ("1") raw value — e.g. Cascade.
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "2"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.setpoint_ini, None);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_does_not_rewrite_mode_when_already_manual() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "0"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        assert_eq!(initial.setpoint_ini, None);

        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        // The Mode Attribute write always fires unconditionally (there's no "already at the
        // program value" guard on it), but Mode itself is already Manual, so its own
        // conditional `write_raw` must not fire a second time.
        let log = driver.write_log();
        assert_eq!(
            log,
            vec![("Unit1.LIC101.MODEATTR".to_string(), "2".to_string())]
        );
        assert!(guard.mode_attribute_written);
        assert!(!guard.mode_written);
    }

    #[tokio::test(start_paused = true)]
    async fn restore_reverts_mode_setpoint_and_mode_attribute() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        assert_eq!(driver.value_of("Unit1.LIC101.OP").as_deref(), Some("45")); // mv_ini
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("1")); // original raw
        assert_eq!(driver.value_of("Unit1.LIC101.SP").as_deref(), Some("55")); // setpoint restored
        assert_eq!(
            driver.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("1")
        ); // reverted off the Program value
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_revert_when_template_disables_it() {
        let mut template = honeywell_template();
        template.revert_mode = false;
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        // MV is always written back regardless of `revert_mode`; Mode/Setpoint are not.
        assert_eq!(driver.value_of("Unit1.LIC101.OP").as_deref(), Some("45"));
        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("0")); // untouched
        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODE"));
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_setpoint_revert_when_original_mode_was_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "2"),
            ("Unit1.LIC101.MODEATTR", "2"),
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        assert_eq!(driver.value_of("Unit1.LIC101.MODE").as_deref(), Some("2")); // reverted
        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_attribute_revert_when_already_at_program_value() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = MockDriver::new(&[
            ("Unit1.LIC101.PV", "50.0"),
            ("Unit1.LIC101.OP", "45.0"),
            ("Unit1.LIC101.MODE", "1"),
            ("Unit1.LIC101.MODEATTR", "2"), // already at the Program value
            ("Unit1.LIC101.CTLACTN", "0"),
            ("Unit1.LIC101.PVEUHI", "100.0"),
            ("Unit1.LIC101.PVEULO", "0.0"),
            ("Unit1.LIC101.CVEUHI", "100.0"),
            ("Unit1.LIC101.CVEULO", "0.0"),
            ("Unit1.LIC101.SP", "55.0"),
        ]);
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();
        let writes_before_restore = driver.write_log().len();

        let report = restore(&driver, &tags, &template, &initial, &guard).await;
        assert!(report.all_succeeded());

        let new_writes = &driver.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODEATTR"));
    }

    /// The heart of `safety-restore-guard`'s "aggregated best-effort restore" (Option C): one
    /// step failing must never prevent the others from being *attempted*, even in the
    /// pathological case where every single one of them also fails. Calls `restore` directly
    /// with a fully-armed `guard` (bypassing `transition_to_manual` entirely, since this test
    /// is only interested in `restore`'s own aggregation behavior in isolation, not in
    /// propagating any one mutation's own error -- that's covered by the `execute`-level
    /// tests around `restore_best_effort_then_propagate` instead).
    #[tokio::test(start_paused = true)]
    async fn restore_reports_each_step_failed_independently_without_short_circuiting() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .erroring_write("Unit1.LIC101.OP")
            .erroring_write("Unit1.LIC101.MODE")
            .erroring_write("Unit1.LIC101.SP")
            .erroring_write("Unit1.LIC101.MODEATTR");
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let report = restore(&driver, &tags, &template, &initial, &guard).await;

        assert!(!report.all_succeeded());
        assert!(matches!(report.mv, RestoreStepOutcome::Failed(_)));
        assert!(matches!(report.mode, RestoreStepOutcome::Failed(_)));
        assert!(matches!(report.setpoint, RestoreStepOutcome::Failed(_)));
        assert!(matches!(
            report.mode_attribute,
            RestoreStepOutcome::Failed(_)
        ));

        // Every step's own failure is independently attributable in the summary -- an
        // operator reading this must be able to tell all four apart, not just "something
        // failed".
        let summary = report.failure_summary().unwrap();
        assert!(summary.contains("MV:"));
        assert!(summary.contains("mode:"));
        assert!(summary.contains("setpoint:"));
        assert!(summary.contains("mode attribute:"));
    }

    #[test]
    fn restore_report_failure_summary_is_none_when_nothing_failed() {
        // The complement of the all-failed test above: a report where every step is
        // `NotNeeded` (the `Default`) has nothing to summarize.
        let report = RestoreReport::default();
        assert!(report.all_succeeded());
        assert!(report.failure_summary().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn write_raw_and_write_value_error_when_the_driver_rejects_the_write() {
        let driver = MockDriver::new(&[("Unit1.LIC101.MODE", "1")])
            .rejecting_write("Unit1.LIC101.MODE")
            .rejecting_write("Unit1.LIC101.OP");

        let raw_err = write_raw(&driver, "Unit1.LIC101.MODE", "0".to_string())
            .await
            .unwrap_err();
        assert!(raw_err.to_string().contains("rejected"));

        let value_err = write_value(&driver, "Unit1.LIC101.OP", 45.0)
            .await
            .unwrap_err();
        assert!(value_err.to_string().contains("rejected"));
    }

    #[tokio::test]
    async fn persist_results_bails_on_a_non_complete_action() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let err = persist_results(
            &pool,
            1,
            Action::WriteMv(0.0),
            ControllerDirection::Direct,
            build_loop_config(&fast_simulator_args()).unwrap(),
            PvRange {
                high: 100.0,
                low: 0.0,
            },
            &template,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("internal error"));
    }

    /// Sets up a run with 3 recorded `TuneResultRow`s (matching a real completed tune) using
    /// the Honeywell template/tags, whose PID constant tags are all configured — the
    /// precondition for `maybe_write_back` to prompt at all rather than skip immediately.
    async fn run_with_recorded_results() -> (SqlitePool, i64) {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let run = TuneRunRow::start(
            &pool,
            None,
            "write-back-test",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &honeywell_template(),
            &honeywell_tags(),
            Utc::now(),
        )
        .await
        .unwrap();
        for (level, kp, ti, td, p, i, d) in [
            (ResponseLevel::Aggressive, 1.0, 0.5, 0.1, 10.0, 2.0, 0.5),
            (ResponseLevel::Moderate, 1.5, 0.7, 0.15, 12.0, 2.5, 0.6),
            (ResponseLevel::Sluggish, 2.0, 0.9, 0.2, 14.0, 3.0, 0.7),
        ] {
            TuneResultRow::insert(
                &pool,
                &TuneResultRow {
                    id: 0,
                    run_id: run.id,
                    response_level: level,
                    kp,
                    ti_minutes: ti,
                    td_minutes: td,
                    proportional: p,
                    integral: i,
                    derivative: d,
                },
            )
            .await
            .unwrap();
        }
        (pool, run.id)
    }

    #[tokio::test]
    async fn maybe_write_back_skips_when_no_pid_constant_tags_are_configured() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let mut tags = honeywell_tags();
        tags.proportional_constant = None;
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no PID constant tags configured for this run's driver/template")
        );
        assert!(
            TuneWriteRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn maybe_write_back_skips_when_no_results_were_recorded() {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "no-results",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run.id,
            &tags,
            &template,
            &driver,
            config,
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no calculated results were recorded for this run")
        );
        assert!(
            TuneWriteRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Runs `maybe_write_back` against `run_with_recorded_results()`'s fixture with the
    /// given stdin-equivalent input, returning both the outcome and the recorded
    /// write-back audit rows (0 or 1).
    async fn write_back_with_input(
        input: &[u8],
    ) -> (WriteBackOutcome, Vec<bhtune_db::models::TuneWriteRow>) {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(input),
        )
        .await
        .unwrap();

        (
            outcome,
            TuneWriteRow::list_for_run(&pool, run_id).await.unwrap(),
        )
    }

    #[tokio::test]
    async fn maybe_write_back_skips_on_eof() {
        let (outcome, writes) = write_back_with_input(b"").await;
        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_skips_on_blank_input() {
        let (outcome, writes) = write_back_with_input(b"\n").await;
        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_skips_on_n() {
        let (outcome, writes) = write_back_with_input(b"N\n").await;
        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_skips_on_out_of_range_selection() {
        let (outcome, writes) = write_back_with_input(b"99\n").await;
        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_skips_on_non_numeric_selection() {
        let (outcome, writes) = write_back_with_input(b"banana\n").await;
        assert_eq!(outcome, WriteBackOutcome::Skipped);
        assert!(writes.is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_writes_and_confirms_a_valid_selection() {
        let (outcome, writes) = write_back_with_input(b"2\n").await; // Moderate (index 1)
        assert_eq!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Moderate
            }
        );
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(write.success);
        assert_eq!(write.response_level, ResponseLevel::Moderate);
        assert!(write.error_message.is_none());
        // A full success pre-reads, writes, and verifies all three constants, and never
        // rolls anything back.
        assert!(write.previous.is_some());
        assert!(write.proportional_written.is_some());
        assert!(write.integral_written.is_some());
        assert!(write.derivative_written.is_some());
        assert!(write.proportional_readback.is_some());
        assert!(write.integral_readback.is_some());
        assert!(write.derivative_readback.is_some());
        assert_eq!(write.rollback_state, None);
        assert!(write.rollback_error.is_none());
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_pre_read_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // Every read of the P tag fails, including the very first (pre-read) one -- nothing
        // is ever written.
        let driver = honeywell_driver_auto().erroring_read("Unit1.LIC101.K");

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert!(
            write_back_detail
                .as_deref()
                .unwrap_or_default()
                .starts_with("pre-read failed:")
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        // The pre-read never produced a known-good value, so there is nothing to record as
        // "previous", and nothing was ever attempted.
        assert!(write.previous.is_none());
        assert!(write.proportional_written.is_none());
        assert!(write.integral_written.is_none());
        assert!(write.derivative_written.is_none());
        assert!(
            write
                .error_message
                .as_deref()
                .unwrap_or_default()
                .starts_with("pre-read of Proportional")
        );
        assert_eq!(write.rollback_state, None);
        // Nothing was written to the driver at all -- confirms the pre-read is a genuine
        // hard stop, not just a reported failure alongside attempted writes.
        assert!(driver.write_log().is_empty());
    }

    #[tokio::test]
    async fn maybe_write_back_rolls_back_a_confirmed_write_when_a_later_constant_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // P writes and verifies successfully; I's write is then rejected. P was already
        // confirmed, so it must be rolled back to its pre-read value.
        let driver = honeywell_driver_auto().rejecting_write("Unit1.LIC101.T1");

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert!(
            write_back_detail
                .as_deref()
                .unwrap_or_default()
                .ends_with("(rolled back)")
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        assert!(write.previous.is_some());
        // P was confirmed (written + read back), I was attempted but never confirmed, D was
        // never attempted at all.
        assert!(write.proportional_written.is_some());
        assert!(write.proportional_readback.is_some());
        assert!(write.integral_written.is_some());
        assert!(write.integral_readback.is_none());
        assert!(write.derivative_written.is_none());
        assert!(write.derivative_readback.is_none());
        assert_eq!(write.rollback_state, Some(RollbackState::Succeeded));
        assert!(write.rollback_error.is_none());
        // The rollback actually put P's original value back on the driver, not just in the
        // audit row.
        let p_previous = write.previous.as_ref().unwrap().proportional;
        assert_eq!(
            driver
                .value_of("Unit1.LIC101.K")
                .and_then(|v| v.parse::<f32>().ok()),
            Some(p_previous)
        );
    }

    #[tokio::test]
    async fn maybe_write_back_records_a_failed_rollback_when_the_rollback_write_is_also_rejected() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // P's forward write succeeds (1st write to the tag), but its rollback write (2nd
        // write to the same tag, once I fails) is rejected -- "wrote some and could not put
        // it back" must be a distinguishable, clearly reported outcome.
        let driver = honeywell_driver_auto()
            .rejecting_write("Unit1.LIC101.T1")
            .rejecting_write_after("Unit1.LIC101.K", 1);

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let detail = write_back_detail.unwrap_or_default();
        assert!(detail.contains("rollback also failed"));
        assert!(detail.contains("history revert"));
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        assert_eq!(write.rollback_state, Some(RollbackState::Failed));
        let rollback_error = write.rollback_error.as_deref().unwrap_or_default();
        assert!(rollback_error.contains("Proportional"));
        assert!(rollback_error.contains("rollback"));
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_is_outside_tolerance() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The write itself succeeds and the readback parses fine at Good quality, but the
        // DCS silently stored a value far outside tolerance of what was requested -- a
        // distinct failure mode from an erroring or poor-quality readback.
        let driver = honeywell_driver_auto().distorting_write("Unit1.LIC101.K", 5.0);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        let write = &writes[0];
        assert!(!write.success);
        let message = write.error_message.as_deref().unwrap_or_default();
        assert!(message.contains("outside tolerance"));
        assert!(message.contains("Proportional"));
        // Nothing was confirmed before the tolerance rejection, so there is nothing to roll
        // back.
        assert_eq!(write.rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_a_write_is_rejected() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().rejecting_write("Unit1.LIC101.K");

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        assert!(writes[0].error_message.is_some());
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_fails() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The pre-read of P succeeds (the tag's 1st read), the write itself succeeds, but
        // the confirmation re-read of the P tag (its 2nd read) then errors.
        let driver = honeywell_driver_auto().erroring_read_after("Unit1.LIC101.K", 1);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();

        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        // The pre-read succeeded, so the audit row still records the previous values, even
        // though the write-and-verify step for P failed.
        assert!(writes[0].previous.is_some());
        let message = writes[0].error_message.as_deref().unwrap_or_default();
        assert!(message.starts_with("Proportional readback from"));
        assert!(!message.starts_with("pre-read of"));
        // Nothing was confirmed before the failure, so there is nothing to roll back.
        assert_eq!(writes[0].rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_the_readback_reports_poor_quality() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        // The pre-read of P (its 1st read) succeeds at the default `Good` quality; only the
        // confirmation re-read after the write (its 2nd read) reports a poor OPC quality --
        // finding 5's rule applies to this readback exactly as it does to any other
        // tuning-critical read, so a stale/clamped value must not be mistaken for proof the
        // write actually landed.
        let driver = honeywell_driver_auto().degrade_quality_after(
            "Unit1.LIC101.K",
            1,
            bhtune_driver::Quality::Bad,
        );

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);

        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        assert!(writes[0].previous.is_some());
        let message = writes[0].error_message.as_deref().unwrap_or_default();
        assert!(message.contains("quality"));
        assert!(message.contains("Unit1.LIC101.K"));
        // Confirms this is the *readback* failing, not the pre-read -- the pre-read's
        // message format is "pre-read of ... failed", distinct from "... readback from ...
        // failed", so an operator (or the history explorer) can tell a poor-quality
        // confirmation apart from a pre-read failure or an outright transport read failure.
        assert!(message.starts_with("Proportional readback from"));
        assert!(!message.starts_with("pre-read of"));
        assert_eq!(writes[0].rollback_state, None);
    }

    #[tokio::test]
    async fn maybe_write_back_accepts_an_uncertain_readback_when_the_flag_is_set() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality("Unit1.LIC101.K", bhtune_driver::Quality::Uncertain);

        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Table,
            true, // allow_uncertain
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, WriteBackOutcome::Written { .. }));
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].success);
    }

    #[tokio::test]
    async fn maybe_write_back_writes_non_interactively_via_write_pid_without_touching_stdin() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();

        // An empty reader would make the *interactive* path treat this as EOF-and-skip; a
        // `write_pid` request must never even try to read it.
        let (outcome, _write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            WriteBackOutcome::Written {
                response_level: ResponseLevel::Aggressive
            }
        );
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        assert_eq!(writes.len(), 1);
        assert!(writes[0].success);
        assert_eq!(writes[0].response_level, ResponseLevel::Aggressive);
    }

    #[tokio::test]
    async fn maybe_write_back_fails_when_write_pid_names_a_level_with_no_recorded_result() {
        let pool = seeded_pool().await;
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "partial-results",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        // Deliberately only record Aggressive and Moderate -- Sluggish is missing, which
        // should never actually happen (`calculate_all` always computes all 3), but
        // `maybe_write_back` must still fail safely rather than panic on an out-of-bounds
        // index or silently write the wrong level.
        for (level, kp, ti, td, p, i, d) in [
            (ResponseLevel::Aggressive, 1.0, 0.5, 0.1, 10.0, 2.0, 0.5),
            (ResponseLevel::Moderate, 1.5, 0.7, 0.15, 12.0, 2.5, 0.6),
        ] {
            TuneResultRow::insert(
                &pool,
                &TuneResultRow {
                    id: 0,
                    run_id: run.id,
                    response_level: level,
                    kp,
                    ti_minutes: ti,
                    td_minutes: td,
                    proportional: p,
                    integral: i,
                    derivative: d,
                },
            )
            .await
            .unwrap();
        }
        let driver = honeywell_driver_auto();

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run.id,
            &tags,
            &template,
            &driver,
            config,
            Some(ResponseLevel::Sluggish),
            OutputFormat::Table,
            false,
            &mut std::io::Cursor::new(b"".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert_eq!(
            write_back_detail.as_deref(),
            Some("no calculated result recorded for response level Sluggish")
        );
        // Nothing was attempted at the driver at all, so no audit row exists either.
        assert!(
            TuneWriteRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn maybe_write_back_skips_the_interactive_prompt_without_touching_stdin_when_json_output_is_set_without_write_pid()
     {
        // `safety-json-contract` (finding 8): under `--output json`, there is no human to
        // answer an interactive write-back prompt, so `write_pid: None` must skip straight
        // to `Skipped` -- without printing a prompt to stdout (which would break the JSON
        // contract) and, just as importantly, without reading a single byte from `reader`
        // (real stdin outside tests), since consuming input meant for something else, or
        // hanging a scripted caller waiting on input that will never arrive, would both be
        // real bugs. A `Cursor` that still reports a real value selection queued up proves
        // the byte was never consumed: if `maybe_write_back` had actually prompted and read
        // it, the outcome would be `Written`, not `Skipped`.
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let mut reader = std::io::Cursor::new(b"1\n".as_slice());

        let (outcome, write_back_detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            OutputFormat::Json,
            false,
            &mut reader,
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
        let detail = write_back_detail.unwrap_or_default();
        assert!(detail.contains("--output json"));
        assert!(detail.contains("--write-pid"));
        assert_eq!(reader.position(), 0, "reader must not be read from at all");
        assert!(
            TuneWriteRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tune_outcome_for_run_maps_every_run_outcome_variant() {
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Skipped,
                write_back_detail: None,
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Written {
                    response_level: ResponseLevel::Moderate
                },
                write_back_detail: None,
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Failed,
                write_back_detail: None,
            }),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::UserInterrupt)),
            TuneOutcome::Aborted
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::Timeout {
                timeout_secs: 3600
            })),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::PoorQuality {
                tag: "Unit1.LIC101.PV".to_string(),
                quality: bhtune_driver::Quality::Bad,
            })),
            TuneOutcome::PoorQuality
        );
    }

    #[test]
    fn print_summary_returns_the_tune_outcome_matching_the_run_outcome_in_every_output_format() {
        // Full 8 (RunOutcome shape) x 2 (OutputFormat) matrix, so every `println!` arm in
        // both `match output` branches is exercised directly here rather than relying on
        // incidental coverage from `run()`-level tests (which never reach `Written`/`Failed`
        // write-back outcomes -- see the module doc comment on why that's structurally hard
        // to drive end-to-end).
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped,
                    write_back_detail: None,
                },
                OutputFormat::Table
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped,
                    write_back_detail: None,
                },
                OutputFormat::Json
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Written {
                        response_level: ResponseLevel::Aggressive
                    },
                    write_back_detail: None,
                },
                OutputFormat::Table
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Written {
                        response_level: ResponseLevel::Aggressive
                    },
                    write_back_detail: None,
                },
                OutputFormat::Json
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed,
                    write_back_detail: None,
                },
                OutputFormat::Table
            ),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed,
                    write_back_detail: None,
                },
                OutputFormat::Json
            ),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::UserInterrupt),
                OutputFormat::Table
            ),
            TuneOutcome::Aborted
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::UserInterrupt),
                OutputFormat::Json
            ),
            TuneOutcome::Aborted
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::Timeout { timeout_secs: 3600 }),
                OutputFormat::Table
            ),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::Timeout { timeout_secs: 3600 }),
                OutputFormat::Json
            ),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::OperationTimedOut {
                    tag: "Unit1.LIC101.PV".to_string(),
                    op_timeout_secs: 30,
                }),
                OutputFormat::Table
            ),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::OperationTimedOut {
                    tag: "Unit1.LIC101.PV".to_string(),
                    op_timeout_secs: 30,
                }),
                OutputFormat::Json
            ),
            TuneOutcome::TimedOut
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::PoorQuality {
                    tag: "Unit1.LIC101.PV".to_string(),
                    quality: bhtune_driver::Quality::Uncertain,
                }),
                OutputFormat::Table
            ),
            TuneOutcome::PoorQuality
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Aborted(AbortReason::PoorQuality {
                    tag: "Unit1.LIC101.PV".to_string(),
                    quality: bhtune_driver::Quality::Uncertain,
                }),
                OutputFormat::Json
            ),
            TuneOutcome::PoorQuality
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::RestoreIncomplete {
                    reason: "run aborted (UserInterrupt); a second Ctrl+C was received while \
                             restoring the loop"
                        .to_string(),
                },
                OutputFormat::Table
            ),
            TuneOutcome::RestoreIncomplete
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::RestoreIncomplete {
                    reason: "run aborted (UserInterrupt); a second Ctrl+C was received while \
                             restoring the loop"
                        .to_string(),
                },
                OutputFormat::Json
            ),
            TuneOutcome::RestoreIncomplete
        );
    }

    #[tokio::test]
    async fn run_rejects_write_pid_without_yes_before_starting_the_tune() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.write_pid = Some(crate::args::ResponseLevelArg::Aggressive);
        args.yes = false;

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("--write-pid requires --yes"));

        // The check happens before any driver/database I/O, so no run row should exist.
        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert!(runs.is_empty());
    }

    #[tokio::test]
    async fn a_full_simulator_tune_with_write_pid_and_yes_still_skips_write_back() {
        // The built-in simulator driver has no PID constant tags at all (see
        // `build_loop_tags`), so `--write-pid`/`--yes` must be accepted but remain a no-op
        // -- not an error, and not `TuneOutcome::WriteBackFailed`.
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.write_pid = Some(crate::args::ResponseLevelArg::Aggressive);
        args.yes = true;

        let outcome = run(&pool, args, &test_config()).await.unwrap();
        assert_eq!(outcome, TuneOutcome::Completed);
    }

    #[tokio::test]
    async fn run_times_out_and_aborts_when_timeout_secs_elapses_before_completion() {
        // Real (unpaused) time: `start_paused` was tried here first but interacts badly with
        // the real sqlx `SqlitePool` -- pausing tokio's clock also fast-forwards the pool's
        // own internal connection-acquire timeout, which fires instantly and turns every
        // query into a spurious `PoolTimedOut` error. So this test pays a real ~1s wall-clock
        // cost instead, matching `tests/ctrlc_abort.rs`'s existing precedent of a similar
        // real-time cost for the same reason (an actual signal/timeout has to actually
        // elapse). `poll_interval_ms: 3` is not a divisor of `timeout_secs: 1`'s 1000ms, so
        // the timeout can never land exactly on a tick boundary. `cycles_count: 100_000`
        // makes it impossible for the MRFT test to legitimately finish within the handful of
        // ticks that occur in one real second at this poll rate (a real oscillation cycle
        // needs at least 2 ticks, so 100,000 cycles needs at least 200,000 -- nowhere near
        // reachable in ~333 ticks).
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.poll_interval_ms = 3;
        args.timeout_secs = 1;
        args.cycles_count = Some(100_000);

        let outcome = run(&pool, args, &test_config()).await.unwrap();
        assert_eq!(outcome, TuneOutcome::TimedOut);

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        // A timeout-triggered abort reuses the exact same DB outcome as Ctrl+C -- only the
        // CLI-level `TuneOutcome`/exit code/printed message distinguish *why*.
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Aborted);

        // Ticks were actually recorded before the timeout fired, proving the loop really ran
        // rather than aborting instantly with nothing sampled.
        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(!samples.is_empty());
    }

    // --- `bounded_driver_call` / `TickOperation`: the four possible race outcomes, tested --
    // --- directly and in isolation from the polling loop that's the only real caller --------

    #[tokio::test]
    async fn bounded_driver_call_returns_completed_when_the_call_finishes_first() {
        let mut ctrl_c = CtrlC::never();
        let result = bounded_driver_call(30, &mut ctrl_c, async { Ok::<_, anyhow::Error>(42) })
            .await
            .unwrap();
        assert!(matches!(result, TickOperation::Completed(42)));
    }

    #[tokio::test]
    async fn bounded_driver_call_propagates_a_genuine_error_from_the_call() {
        // A real failure from the call itself (a rejected write, a malformed value, a
        // transport error) must still propagate through `?` at the call site -- it is not
        // "gave up waiting", so it has no `TickOperation` variant of its own.
        let mut ctrl_c = CtrlC::never();
        let err = bounded_driver_call(30, &mut ctrl_c, async {
            Err::<(), _>(anyhow::anyhow!("boom"))
        })
        .await
        .unwrap_err();
        assert!(err.to_string().contains("boom"));
    }

    #[tokio::test]
    async fn bounded_driver_call_returns_cancelled_when_ctrl_c_fires_first() {
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        let result = bounded_driver_call(30, &mut ctrl_c, async {
            std::future::pending::<anyhow::Result<()>>().await
        })
        .await
        .unwrap();
        assert!(matches!(result, TickOperation::Cancelled));
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_driver_call_returns_timed_out_when_the_driver_call_stalls() {
        // No `SqlitePool` involved here (unlike the `run_polling_loop`-level tests), so
        // `start_paused` is safe -- see the precedent/caveat noted on the timeout test above.
        let mut ctrl_c = CtrlC::never();
        let result = bounded_driver_call(1, &mut ctrl_c, async {
            std::future::pending::<anyhow::Result<()>>().await
        })
        .await
        .unwrap();
        assert!(matches!(result, TickOperation::TimedOut));
    }

    // --- `attempt_restore` / `RestoreAttempt`: confirmed vs. incomplete, and both ways to ---
    // --- become incomplete (a second Ctrl+C, and `--restore-timeout-secs` elapsing) ---------

    #[tokio::test(start_paused = true)]
    async fn attempt_restore_confirms_a_normal_restore() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto();
        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();
        let mut guard = MutationGuard::default();
        transition_to_manual(&driver, &tags, &template, &initial, &mut guard)
            .await
            .unwrap();

        let outcome = attempt_restore(
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            30,
            &mut CtrlC::never(),
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn attempt_restore_reports_incomplete_when_restore_timeout_secs_elapses() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        // `restore`'s very first step writes the MV -- hanging it means `restore()` itself
        // can never resolve on its own, so only the timeout branch can win this race.
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let initial = sample_initial_state();
        let guard = MutationGuard::default();

        let outcome = attempt_restore(
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            1,
            &mut CtrlC::never(),
        )
        .await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("--restore-timeout-secs"));
            }
            RestoreAttempt::Confirmed => panic!("expected RestoreAttempt::Incomplete"),
        }
    }

    #[tokio::test]
    async fn attempt_restore_reports_incomplete_on_a_second_ctrl_c() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let initial = sample_initial_state();
        let guard = MutationGuard::default();
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome =
            attempt_restore(&driver, &tags, &template, &initial, &guard, 30, &mut ctrl_c).await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("second Ctrl+C"));
            }
            RestoreAttempt::Confirmed => panic!("expected RestoreAttempt::Incomplete"),
        }
    }

    // --- `run_with_ctrl_c`: the real, ctrl-c-aware entry point, exercised end to end with ---
    // --- a simulated signal rather than only through the `CtrlC::never()`-backed `run` above

    /// The one test exercising `run_with_ctrl_c` itself (every other test in this module goes
    /// through the `#[cfg(test)]`-only `run` wrapper, which always passes `CtrlC::never()` --
    /// see its doc comment) -- proving this is what `lib.rs::run_with_cli_and_ctrl_c` actually
    /// calls in production correctly reacts to a signalled `CtrlC` end to end: dispatch, the
    /// poll loop, the restore, and the final `TuneOutcome`/DB row.
    #[tokio::test]
    async fn run_with_ctrl_c_aborts_the_run_when_signalled_during_the_poll() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        // Impossible to legitimately finish within the ~50ms before the signal below fires,
        // matching the timeout test's own precedent for making a real completion race moot.
        args.cycles_count = Some(100_000);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let outcome = run_with_ctrl_c(&pool, args, &test_config(), &mut ctrl_c)
            .await
            .unwrap();
        assert_eq!(outcome, TuneOutcome::Aborted);

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Aborted);
    }
}
