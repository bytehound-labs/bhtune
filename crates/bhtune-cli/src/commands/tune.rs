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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::time::Duration;

use bhtune_core::mrft::clamp_relay_amplitude;
use bhtune_core::{
    Action, ControllerDirection, ControllerType, DcsTemplate, InitialReadings, LoopConfig,
    LoopTags, MrftCompat, MrftEngine, MvRange, PidParameters, ProcessType, PvRange, ResponseLevel,
    TagOrValue, TagOverrides, Tick, TuningMathCompat, TuningResultStatus, calculate_all_checked,
    lookup, measure_oscillation, opc_write_values,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    DcsTemplateRow, EffectiveTuning, MvActuationKind, MvActuationStatus, NewTuneMvActuation,
    NewTuneWrite, RollbackState, SampleQuality, TimingBasis, TimingMetrics, TuneDriver,
    TuneMvActuationRow, TuneResultRow, TuneRunInitialReadings, TuneRunRow, TuneSampleRow,
    TuneWriteRow, WriteKind, WriteReadback,
};
use bhtune_driver::{Driver, TagValue, TagWrite};
use chrono::{DateTime, Utc};
use tokio::time::Instant;

use crate::args::{DriverKindArg, TuneArgs};
use crate::cancel::CtrlC;
use crate::driver::{SIMULATOR_MV_TAG, SIMULATOR_PV_TAG};
use crate::output::OutputFormat;
use crate::timing::{PollTimingAccumulator, RunTimeAnchor, TickTimeSource};

/// Maximum interval from an accepted OPC DA MV write to its mandatory confirmation check.
///
/// Public so HTTP and other non-clap adapters can expose the same validation policy without
/// copying the safety-critical literal.
pub const MV_ACTUATION_CONFIRMATION_SECS: u64 = 4;
const MV_ACTUATION_RETRY_INTERVAL: Duration = Duration::from_millis(100);
const MV_ACTUATION_DEADLINE_READ_MAX: Duration = Duration::from_secs(1);
const MV_ACTUATION_FALLBACK_HEADROOM: Duration = Duration::from_secs(1);
const MV_RESTORE_HANDOFF_READ_MAX: Duration = Duration::from_secs(1);
const MV_SPAN_TOLERANCE_FRACTION: f32 = 0.001;
const RELAY_STEP_TOLERANCE_FRACTION: f32 = 0.25;
const MIN_RELAY_STEP: f32 = 0.01;

/// Concrete timing policy frozen during [`prepare`] and carried unchanged through the
/// complete tune lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EffectiveTiming {
    mrft_delay_secs: u32,
    poll_interval_ms: u64,
    timeout_secs: u64,
    op_timeout_secs: u64,
    restore_timeout_secs: u64,
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
fn test_effective_timing(args: &TuneArgs) -> EffectiveTiming {
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
    driver: DriverKindArg,
    restore_timeout_secs: u64,
) -> anyhow::Result<()> {
    if restore_timeout_secs == 0 {
        anyhow::bail!("[tuning].restore_timeout_secs must be greater than zero");
    }
    if driver == DriverKindArg::Opcda
        && restore_timeout_secs < crate::config::MIN_OPC_RESTORE_TIMEOUT_SECS
    {
        anyhow::bail!(
            "[tuning].restore_timeout_secs must be at least {} seconds for OPC DA MV confirmation",
            crate::config::MIN_OPC_RESTORE_TIMEOUT_SECS
        );
    }
    Ok(())
}

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
    /// in-flight PV poll sample when Config > OPC quality policy rejects Uncertain (or with
    /// the policy enabled, but the
    /// quality was `Bad` rather than merely `Uncertain`) -- and the run was aborted and the
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
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
    } = prepared;

    let outcome = execute_with_timing(
        pool,
        run_id,
        &args,
        &template,
        &tags,
        driver.as_ref(),
        config,
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
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
            finalize_pending_for_run_best_effort(
                pool,
                run_id,
                "the run failed before MV confirmation completed",
            )
            .await;
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
    timing: EffectiveTiming,
    time_anchor: RunTimeAnchor,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
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
    notes: Option<&'a str>,
    yes: bool,
    write_pid: Option<ResponseLevel>,
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
    let timing: EffectiveTiming = crate::config::resolve_and_validate_tuning_config(
        &app_config.tuning,
        args.driver == DriverKindArg::Opcda,
    )?
    .into();
    if let Some(tag_overrides) = &args.tag_overrides {
        tag_overrides.validate()?;
    }
    let allow_uncertain_quality = app_config.allow_uncertain_quality;

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
        notes: args.notes.as_deref(),
        yes: args.yes,
        write_pid: args.write_pid.map(Into::into),
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

    let config = build_loop_config_with_timing(&args, timing)?;
    let tags = build_loop_tags(&args, &template)?;
    let driver = crate::driver::build_with_poll_interval(&args, timing.poll_interval_ms).await?;

    let time_anchor = RunTimeAnchor::now();
    let started_at = time_anchor.utc();
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
    TuneRunRow::record_effective_tuning(pool, run.id, timing.into()).await?;
    TuneRunRow::record_allow_uncertain_quality(pool, run.id, allow_uncertain_quality).await?;

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
        allow_uncertain_quality,
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
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
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
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
    } = prepared;

    let outcome = execute_with_timing(
        pool,
        run_id,
        &args,
        &template,
        &tags,
        driver.as_ref(),
        config,
        timing,
        time_anchor,
        write_pid,
        allow_uncertain_quality,
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
            finalize_pending_for_run_best_effort(
                pool,
                run_id,
                "the run failed before MV confirmation completed",
            )
            .await;
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
    /// restore attempt ([`attempt_restore_with_actuation`]) could not be confirmed -- a
    /// second Ctrl+C arrived, or `[tuning].restore_timeout_secs` elapsed.
    /// `reason` is a human-readable description of what happened, already including the
    /// original abort trigger (if any) -- see `execute`'s composition of it. Write-back is
    /// always skipped in this case, since writing new PID constants to a loop whose mode/MV
    /// cannot be confirmed restored would compound the uncertainty.
    RestoreIncomplete {
        reason: String,
    },
}

/// Why a run ended via [`RunOutcome::Aborted`] instead of a normal engine completion.
#[derive(Debug, Clone, PartialEq)]
enum AbortReason {
    /// Ctrl+C.
    UserInterrupt,
    /// `[tuning].timeout_secs` elapsed before the engine reported completion. Carries the
    /// configured limit that was hit, for the printed/JSON summary.
    Timeout { timeout_secs: u64 },
    /// A single driver read/write during a poll tick did not resolve within
    /// `[tuning].op_timeout_secs` -- distinct from [`AbortReason::Timeout`], which bounds the whole
    /// run rather than one operation. Carries the tag that stalled and the configured limit,
    /// for the printed/JSON summary. Maps to the same [`TuneOutcome::TimedOut`] as
    /// `Timeout`, since both mean "gave up waiting", differing only in what exactly timed
    /// out.
    OperationTimedOut { tag: String, op_timeout_secs: u64 },
    /// An in-flight PV poll sample's quality was `Bad`, or `Uncertain` without
    /// Config > OPC quality policy set to reject Uncertain (finding 5 of the live-plant
    /// safety review). Unlike
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
    /// An accepted OPC DA MV write was not physically observed within tolerance before its
    /// four-second deadline or before a later relay action needed to replace it.
    MvActuationUnconfirmed {
        tag: String,
        target: f32,
        readback: Option<f32>,
        tolerance: f32,
        elapsed_ms: u64,
        deadline_secs: u64,
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
        RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. }) => {
            TuneOutcome::ActuationFailed
        }
        RunOutcome::RestoreIncomplete { .. } => TuneOutcome::RestoreIncomplete,
    }
}

fn format_mv_actuation_abort_reason(reason: &AbortReason) -> String {
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
fn print_summary(run_id: i64, outcome: &RunOutcome, output: OutputFormat) -> TuneOutcome {
    let tune_outcome = tune_outcome_for_run(outcome);
    match output {
        OutputFormat::Table => print_table_summary(run_id, outcome),
        OutputFormat::Json => print_json_summary(run_id, outcome, tune_outcome),
    }
    tune_outcome
}

fn print_table_summary(run_id: i64, outcome: &RunOutcome) {
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

fn print_json_summary(run_id: i64, outcome: &RunOutcome, tune_outcome: TuneOutcome) {
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

fn render_json_summary<E>(
    json: &serde_json::Value,
    serialize: impl FnOnce(&serde_json::Value) -> Result<String, E>,
) -> String
where
    E: std::fmt::Display,
{
    serialize(json).unwrap_or_else(|error| format!("{{\"error\": \"{error}\"}}"))
}

fn build_loop_config_with_timing(
    args: &TuneArgs,
    timing: EffectiveTiming,
) -> anyhow::Result<LoopConfig> {
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
fn build_loop_config(args: &TuneArgs) -> anyhow::Result<LoopConfig> {
    build_loop_config_with_timing(args, test_effective_timing(args))
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

#[derive(Debug)]
struct PendingMvActuation {
    id: Option<i64>,
    kind: MvActuationKind,
    target: f32,
    tolerance: f32,
    switch_tick: DateTime<Utc>,
    switch_instant: Instant,
    accepted_instant: Instant,
    first_check_at: Instant,
    deadline: Instant,
    last_readback: Option<f32>,
}

#[derive(Debug)]
struct MvActuationTracker {
    next_sequence: i64,
    previous_commanded_mv: f32,
    confirmed_mv: Option<f32>,
    pending: Option<PendingMvActuation>,
    mv_span: f32,
}

impl MvActuationTracker {
    fn for_run(args: &TuneArgs, initial: &InitialState) -> Option<Self> {
        (args.driver == DriverKindArg::Opcda).then_some(Self {
            next_sequence: 0,
            previous_commanded_mv: initial.mv_ini,
            confirmed_mv: None,
            pending: None,
            mv_span: initial.mv_range_high - initial.mv_range_low,
        })
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    async fn record_accepted(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        kind: MvActuationKind,
        target: f32,
        first_check_at: Instant,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) -> anyhow::Result<()> {
        self.record_accepted_at_switch(
            pool,
            run_id,
            kind,
            target,
            accepted_at,
            accepted_instant,
            first_check_at,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_accepted_at_switch(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        kind: MvActuationKind,
        target: f32,
        switch_tick: DateTime<Utc>,
        switch_instant: Instant,
        first_check_at: Instant,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) -> anyhow::Result<()> {
        let deadline = accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
        let confirmation_due_at =
            accepted_at + chrono::Duration::seconds(MV_ACTUATION_CONFIRMATION_SECS as i64);
        let previous_commanded_mv = Some(self.previous_commanded_mv);
        let row = TuneMvActuationRow::insert_pending(
            pool,
            run_id,
            NewTuneMvActuation {
                sequence: self.next_sequence,
                kind,
                commanded_at: accepted_at,
                target_mv: target,
                previous_commanded_mv,
                tolerance,
                confirmation_due_at,
            },
        )
        .await?;
        self.accept_pending(PendingMvActuation {
            id: Some(row.id),
            kind,
            target,
            tolerance,
            switch_tick,
            switch_instant,
            accepted_instant,
            first_check_at: first_check_at.min(deadline),
            deadline,
            last_readback: None,
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn record_restore_accepted_best_effort(
        &mut self,
        pool: &SqlitePool,
        run_id: i64,
        target: f32,
        accepted_at: DateTime<Utc>,
        accepted_instant: Instant,
        tolerance: f32,
    ) {
        let deadline = accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
        let confirmation_due_at =
            accepted_at + chrono::Duration::seconds(MV_ACTUATION_CONFIRMATION_SECS as i64);
        let pending = PendingMvActuation {
            id: None,
            kind: MvActuationKind::Restore,
            target,
            tolerance,
            switch_tick: accepted_at,
            switch_instant: accepted_instant,
            accepted_instant,
            first_check_at: accepted_instant,
            deadline,
            last_readback: None,
        };
        let row = TuneMvActuationRow::insert_pending(
            pool,
            run_id,
            NewTuneMvActuation {
                sequence: self.next_sequence,
                kind: MvActuationKind::Restore,
                commanded_at: accepted_at,
                target_mv: target,
                previous_commanded_mv: Some(self.previous_commanded_mv),
                tolerance,
                confirmation_due_at,
            },
        )
        .await;
        let mut pending = pending;
        match row {
            Ok(row) => pending.id = Some(row.id),
            Err(error) => {
                tracing::error!(
                    run_id,
                    error = %error,
                    "failed to record accepted restore MV command; continuing physical restore"
                );
            }
        }
        self.accept_pending(pending);
    }

    fn accept_pending(&mut self, pending: PendingMvActuation) {
        self.next_sequence += 1;
        self.previous_commanded_mv = pending.target;
        self.confirmed_mv = None;
        self.pending = Some(pending);
    }

    fn next_verification_wakeup(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| {
            if pending.last_readback.is_some() {
                pending.deadline
            } else {
                pending
                    .first_check_at
                    .max(pending.deadline - MV_ACTUATION_FALLBACK_HEADROOM)
            }
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_relay_actuation(
    tracker: &mut MvActuationTracker,
    pool: &SqlitePool,
    run_id: i64,
    target: f32,
    switch_tick: DateTime<Utc>,
    switch_instant: Instant,
    first_check_at: Instant,
    elapsed_since_observation: Duration,
    accepted_instant: Instant,
    tolerance: f32,
) -> anyhow::Result<()> {
    let accepted_at = utc_after_elapsed(switch_tick, elapsed_since_observation)?;
    tracker
        .record_accepted_at_switch(
            pool,
            run_id,
            MvActuationKind::Relay,
            target,
            switch_tick,
            switch_instant,
            first_check_at,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await
}

fn f32_precision_floor(target: f32, previous: f32) -> f32 {
    4.0 * f32::EPSILON * target.abs().max(previous.abs()).max(1.0)
}

fn mv_actuation_tolerance(
    kind: MvActuationKind,
    target: f32,
    previous: f32,
    mv_span: f32,
) -> anyhow::Result<f32> {
    let uncapped = mv_actuation_uncapped_tolerance(target, previous, mv_span);
    if kind == MvActuationKind::Restore {
        return Ok(uncapped);
    }

    let step = (target - previous).abs();
    let relay_cap = step * RELAY_STEP_TOLERANCE_FRACTION;
    if !step.is_finite()
        || step < MIN_RELAY_STEP
        || relay_cap <= f32_precision_floor(target, previous)
    {
        let minimum_step = MIN_RELAY_STEP
            .max(f32_precision_floor(target, previous) / RELAY_STEP_TOLERANCE_FRACTION);
        anyhow::bail!(
            "the effective relay step {step} is too small to verify safely (minimum {})",
            minimum_step
        );
    }
    Ok(uncapped.min(relay_cap))
}

fn mv_actuation_uncapped_tolerance(target: f32, previous: f32, mv_span: f32) -> f32 {
    let precision_floor = f32_precision_floor(target, previous);
    let span_tolerance = mv_span.abs() * MV_SPAN_TOLERANCE_FRACTION;
    precision_floor + span_tolerance
}

fn validate_relay_actuation_step(
    args: &TuneArgs,
    config: LoopConfig,
    initial: &InitialState,
) -> anyhow::Result<()> {
    if args.driver != DriverKindArg::Opcda {
        return Ok(());
    }
    let relay_step = clamp_relay_amplitude(
        config.relay_amp_percent,
        initial.mv_ini,
        initial.mv_range_low,
        initial.mv_range_high,
        MrftCompat::default(),
    );
    mv_actuation_tolerance(
        MvActuationKind::Relay,
        initial.mv_ini + relay_step,
        initial.mv_ini,
        initial.mv_range_high - initial.mv_range_low,
    )
    .map(|_| ())
}

/// Persists the three calculated response levels after a completed MRFT test. The run's
/// terminal outcome is deliberately recorded later, after restoration, together with its
/// timing snapshot; this keeps SSE-triggered readers from seeing a terminal row before those
/// diagnostics are visible.
async fn persist_completed_results(
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
    .await
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
async fn execute_with_timing<R: std::io::BufRead>(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    effective_timing: EffectiveTiming,
    time_anchor: RunTimeAnchor,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    reader: &mut R,
) -> anyhow::Result<RunOutcome> {
    let started_at = time_anchor.utc();
    let initial = read_initial_values(driver, tags, template, allow_uncertain_quality).await?;
    validate_initial_state(&initial)?;
    validate_relay_actuation_step(args, config, &initial)?;

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
    let mut mv_actuations = MvActuationTracker::for_run(args, &initial);
    if let Err(e) = transition_to_manual(driver, tags, template, &initial, &mut guard).await {
        return Err(restore_best_effort_then_propagate_with_timing(
            pool,
            run_id,
            driver,
            tags,
            template,
            &initial,
            &guard,
            args,
            effective_timing,
            allow_uncertain_quality,
            ctrl_c,
            &mut mv_actuations,
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
    let timing_basis = match args.driver {
        DriverKindArg::Opcda => TimingBasis::LiveMonotonic,
        DriverKindArg::Simulator => TimingBasis::SimulatedFixedStep,
    };
    let mut timing = PollTimingAccumulator::new(timing_basis, effective_timing.poll_interval_ms);

    let poll_result = run_polling_loop_with_timing(
        pool,
        run_id,
        args,
        effective_timing,
        tags,
        driver,
        &mut engine,
        time_anchor,
        ctrl_c,
        &mut guard,
        allow_uncertain_quality,
        &mut timing,
        &mut mv_actuations,
        config,
    )
    .await;
    let measured_oscillation_period_ms = completed_oscillation_period_ms(
        &poll_result,
        initial.direction,
        config,
        PvRange {
            high: initial.pv_range_high,
            low: initial.pv_range_low,
        },
    );
    let timing_metrics_without_period = timing.finish(None);
    if let Some(timing_metrics) = timing_metrics_without_period.as_ref() {
        warn_on_missed_poll_opportunities(run_id, timing_metrics);
    }

    match poll_result {
        Ok(PollOutcome::Completed(completion)) => {
            finish_completed_run(
                pool,
                run_id,
                args,
                effective_timing,
                template,
                tags,
                driver,
                config,
                &initial,
                &guard,
                write_pid,
                allow_uncertain_quality,
                ctrl_c,
                reader,
                &mut mv_actuations,
                completion,
                &mut timing,
                timing_metrics_without_period,
                measured_oscillation_period_ms,
            )
            .await
        }
        Ok(PollOutcome::Aborted(reason)) => {
            finish_aborted_run(
                pool,
                run_id,
                args,
                effective_timing,
                template,
                tags,
                driver,
                &initial,
                &guard,
                allow_uncertain_quality,
                ctrl_c,
                &mut mv_actuations,
                reason,
                timing_metrics_without_period,
            )
            .await
        }
        Err(error) => {
            finish_failed_run(
                pool,
                run_id,
                template,
                tags,
                driver,
                &initial,
                &guard,
                args,
                effective_timing,
                allow_uncertain_quality,
                ctrl_c,
                &mut mv_actuations,
                error,
                timing_metrics_without_period,
            )
            .await
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn execute<R: std::io::BufRead>(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    time_anchor: RunTimeAnchor,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    reader: &mut R,
) -> anyhow::Result<RunOutcome> {
    execute_with_timing(
        pool,
        run_id,
        args,
        template,
        tags,
        driver,
        config,
        test_effective_timing(args),
        time_anchor,
        write_pid,
        allow_uncertain_quality,
        ctrl_c,
        reader,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn attempt_and_record_restore(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
) -> RestoreAttempt {
    let restore_attempt = attempt_restore_with_actuation_with_timing(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await;
    record_restore_status_best_effort(pool, run_id, &restore_attempt).await;
    finalize_pending_for_run_best_effort(
        pool,
        run_id,
        "the run ended before MV confirmation completed",
    )
    .await;
    restore_attempt
}

#[allow(clippy::too_many_arguments)]
async fn finish_completed_run<R: std::io::BufRead>(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    config: LoopConfig,
    initial: &InitialState,
    guard: &MutationGuard,
    write_pid: Option<ResponseLevel>,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    reader: &mut R,
    mv_actuations: &mut Option<MvActuationTracker>,
    completion: Action,
    timing: &mut PollTimingAccumulator,
    timing_metrics_without_period: Option<TimingMetrics>,
    measured_oscillation_period_ms: Option<f64>,
) -> anyhow::Result<RunOutcome> {
    let pv_range = PvRange {
        high: initial.pv_range_high,
        low: initial.pv_range_low,
    };
    if let Err(error) = persist_completed_results(
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
        let error = restore_best_effort_then_propagate_with_timing(
            pool,
            run_id,
            driver,
            tags,
            template,
            initial,
            guard,
            args,
            effective_timing,
            allow_uncertain_quality,
            ctrl_c,
            mv_actuations,
            error,
        )
        .await;
        record_timing_metrics_if_present(pool, run_id, timing_metrics_without_period).await;
        return Err(error);
    }

    let restore_attempt = attempt_and_record_restore(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await;
    TuneRunRow::complete_with_timing_metrics(
        pool,
        run_id,
        Utc::now(),
        timing.finish(measured_oscillation_period_ms),
    )
    .await?;
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
                allow_uncertain_quality,
                reader,
            )
            .await?;
            Ok(RunOutcome::Completed {
                write_back,
                write_back_detail,
            })
        }
        RestoreAttempt::Incomplete { reason } => Ok(RunOutcome::RestoreIncomplete { reason }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_aborted_run(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    reason: AbortReason,
    timing_metrics_without_period: Option<TimingMetrics>,
) -> anyhow::Result<RunOutcome> {
    let restore_attempt = attempt_and_record_restore(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await;
    if matches!(reason, AbortReason::MvActuationUnconfirmed { .. }) {
        TuneRunRow::abort_with_timing_metrics_and_reason(
            pool,
            run_id,
            Utc::now(),
            timing_metrics_without_period,
            &format_mv_actuation_abort_reason(&reason),
        )
        .await?;
    } else {
        TuneRunRow::abort_with_timing_metrics(
            pool,
            run_id,
            Utc::now(),
            timing_metrics_without_period,
        )
        .await?;
    }
    match restore_attempt {
        RestoreAttempt::Confirmed => Ok(RunOutcome::Aborted(reason)),
        RestoreAttempt::Incomplete {
            reason: restore_reason,
        } => Ok(RunOutcome::RestoreIncomplete {
            reason: format!("run aborted ({reason:?}); {restore_reason}"),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_failed_run(
    pool: &SqlitePool,
    run_id: i64,
    template: &DcsTemplate,
    tags: &LoopTags,
    driver: &dyn Driver,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    error: anyhow::Error,
    timing_metrics_without_period: Option<TimingMetrics>,
) -> anyhow::Result<RunOutcome> {
    // Best-effort: a failed test still stroked the valve, so try to put it back even
    // though the overall run is going to be reported as failed regardless. Still
    // bounded/interruptible (a second Ctrl+C or `[tuning].restore_timeout_secs` still cuts
    // it short) and still warns loudly on an incomplete restore.
    let error = restore_best_effort_then_propagate_with_timing(
        pool,
        run_id,
        driver,
        tags,
        template,
        initial,
        guard,
        args,
        effective_timing,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        error,
    )
    .await;
    record_timing_metrics_if_present(pool, run_id, timing_metrics_without_period).await;
    Err(error)
}

/// The single choke point enforcing finding 5 of the live-plant safety review
/// ("`Quality::is_trustworthy()` exists and is documented as the rule; nothing in the tune
/// path calls it"): `Quality::Bad` is never accepted; `Quality::Uncertain`
/// is accepted only when the global Config > OPC quality policy
/// (`allow_uncertain_quality` in TOML) permits it, and each use of it is logged loudly so a run executed under relaxed rules is never silently
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
    parse_f32_value(tag, &raw)
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

fn parse_f32_value(tag: &str, raw: &str) -> anyhow::Result<f32> {
    let value: f32 = raw
        .trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{raw}' is not a number"))?;
    if !value.is_finite() {
        anyhow::bail!("tag '{tag}' value '{raw}' is not a finite number");
    }
    Ok(value)
}

fn read_batch_raw(
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

fn read_batch_f32(
    values: &HashMap<String, TagValue>,
    tag: &str,
    allow_uncertain: bool,
) -> anyhow::Result<f32> {
    let raw = read_batch_raw(values, tag, allow_uncertain)?;
    parse_f32_value(tag, &raw)
}

async fn resolve_f32_from_batch(
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

async fn resolve_direction_from_batch(
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
async fn read_pv_sample(
    driver: &dyn Driver,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    read_numeric_sample(driver, tag).await
}

async fn read_numeric_sample(
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

async fn read_poll_batch(
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

fn read_numeric_from_batch(
    values: &HashMap<String, TagValue>,
    tag: &str,
) -> anyhow::Result<(f32, bhtune_driver::Quality)> {
    let value = values
        .get(tag)
        .ok_or_else(|| anyhow::anyhow!("driver returned no value for tag '{tag}'"))?;
    let numeric = parse_f32_value(tag, &value.value)?;
    Ok((numeric, value.quality))
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
    let mut requested_tags = Vec::new();
    let mut seen_tags = HashSet::new();
    let mut request_tag = |tag: &str| {
        if seen_tags.insert(tag.to_string()) {
            requested_tags.push(tag.to_string());
        }
    };

    request_tag(&tags.process_variable);
    request_tag(&tags.manipulated_variable);
    if let Some(tag) = &tags.controller_mode {
        request_tag(tag);
    }
    if let Some(tag) = &tags.mode_attribute {
        request_tag(tag);
    }
    if let TagOrValue::Tag(tag) = &tags.controller_direction {
        request_tag(tag.as_str());
    }
    for tag_or_value in [
        &tags.upper_pv_range,
        &tags.lower_pv_range,
        &tags.upper_mv_range,
        &tags.lower_mv_range,
    ] {
        if let TagOrValue::Tag(tag) = tag_or_value {
            request_tag(tag.as_str());
        }
    }

    let values_by_tag: HashMap<String, TagValue> = driver
        .read(&requested_tags)
        .await?
        .into_iter()
        .map(|value| (value.tag.clone(), value))
        .collect();

    let pv_ini = read_batch_f32(&values_by_tag, &tags.process_variable, allow_uncertain)?;
    let mv_ini = read_batch_f32(&values_by_tag, &tags.manipulated_variable, allow_uncertain)?;

    let mode_raw = match &tags.controller_mode {
        Some(tag) => Some(read_batch_raw(&values_by_tag, tag, allow_uncertain)?),
        None => None,
    };
    let mode_attribute_raw = match &tags.mode_attribute {
        Some(tag) => Some(read_batch_raw(&values_by_tag, tag, allow_uncertain)?),
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

    let direction = resolve_direction_from_batch(
        driver,
        &values_by_tag,
        &tags.controller_direction,
        template,
        allow_uncertain,
    )
    .await?;
    let pv_range_high = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.upper_pv_range,
        allow_uncertain,
    )
    .await?;
    let pv_range_low = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.lower_pv_range,
        allow_uncertain,
    )
    .await?;
    let mv_range_high = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.upper_mv_range,
        allow_uncertain,
    )
    .await?;
    let mv_range_low = resolve_f32_from_batch(
        driver,
        &values_by_tag,
        &tags.lower_mv_range,
        allow_uncertain,
    )
    .await?;

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
async fn restore_after_mv(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    mv: RestoreStepOutcome,
) -> RestoreReport {
    let mode = restore_mode_step(driver, tags, template, initial, guard).await;
    let setpoint = restore_setpoint_step(driver, tags, template, initial, guard).await;
    let mode_attribute = restore_mode_attribute_step(driver, tags, template, initial, guard).await;

    RestoreReport {
        mv,
        mode,
        setpoint,
        mode_attribute,
    }
}

async fn restore_value_step(driver: &dyn Driver, tag: &str, value: f32) -> RestoreStepOutcome {
    match write_value(driver, tag, value).await {
        Ok(()) => RestoreStepOutcome::Succeeded,
        Err(e) => RestoreStepOutcome::Failed(e.to_string()),
    }
}

#[cfg(test)]
async fn restore(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreReport {
    let mv = restore_value_step(driver, &tags.manipulated_variable, initial.mv_ini).await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    restore_after_mv(driver, tags, template, initial, guard, mv).await
}

async fn restore_raw_step(driver: &dyn Driver, tag: &str, value: &str) -> RestoreStepOutcome {
    match write_raw(driver, tag, value.to_string()).await {
        Ok(()) => RestoreStepOutcome::Succeeded,
        Err(e) => RestoreStepOutcome::Failed(e.to_string()),
    }
}

async fn restore_mode_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    let Some(mode_tag) = &tags.controller_mode else {
        return RestoreStepOutcome::NotNeeded;
    };
    let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
    if !guard.mode_written || !template.revert_mode || mode_raw == template.mode_manual_value {
        return RestoreStepOutcome::NotNeeded;
    }
    restore_raw_step(driver, mode_tag, mode_raw).await
}

async fn restore_setpoint_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    if !guard.mode_written || !template.revert_mode {
        return RestoreStepOutcome::NotNeeded;
    }
    let (Some(sv_tag), Some(sv_ini)) = (&tags.setpoint_variable, initial.setpoint_ini) else {
        return RestoreStepOutcome::NotNeeded;
    };
    tokio::time::sleep(Duration::from_millis(1000)).await;
    restore_value_step(driver, sv_tag, sv_ini).await
}

async fn restore_mode_attribute_step(
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
) -> RestoreStepOutcome {
    let Some(attr_tag) = &tags.mode_attribute else {
        return RestoreStepOutcome::NotNeeded;
    };
    let attr_raw = initial.mode_attribute_raw.as_deref().unwrap_or_default();
    let program_value = template
        .mode_attribute_program_value
        .as_deref()
        .unwrap_or_default();
    if !guard.mode_attribute_written || attr_raw == program_value {
        return RestoreStepOutcome::NotNeeded;
    }
    restore_raw_step(driver, attr_tag, attr_raw).await
}

/// The outcome of racing one driver call (a poll batch or [`write_value`], during a poll tick)
/// against Ctrl+C and `[tuning].op_timeout_secs` -- see [`bounded_driver_call`]. Distinct from a
/// genuine `Err` from the call itself (a rejected write, a malformed value, a transport error),
/// which [`bounded_driver_call`] still propagates via `?` rather than wrapping here, since those
/// are real failures, not "gave up waiting".
#[derive(Debug)]
enum TickOperation<T> {
    /// `fut` resolved before either interrupt source.
    Completed(T),
    /// Ctrl+C (or a second Ctrl+C) fired first; `fut` was dropped, abandoning it in flight.
    Cancelled,
    /// `[tuning].op_timeout_secs` elapsed first; `fut` was dropped, abandoning it in flight.
    TimedOut,
}

/// Races one driver call against `ctrl_c` and a fresh `op_timeout_secs` sleep, so a single
/// stalled read/write (gateway down, DCOM wedged, network black-holed) can never make the
/// polling loop -- or the restore, via [`attempt_restore_with_actuation`] -- uninterruptible.
/// This is what
/// fixes finding 2 of the live-plant safety review: previously, `run_polling_loop`'s Ctrl+C
/// and `[tuning].timeout_secs` listeners only ran *between* tick-body awaits, so a hung call inside
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

fn actuation_abort_reason(
    tag: &str,
    pending: &PendingMvActuation,
    readback: Option<f32>,
    now: Instant,
) -> AbortReason {
    let elapsed_ms = now
        .saturating_duration_since(pending.accepted_instant)
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    AbortReason::MvActuationUnconfirmed {
        tag: tag.to_string(),
        target: pending.target,
        readback,
        tolerance: pending.tolerance,
        elapsed_ms,
        deadline_secs: MV_ACTUATION_CONFIRMATION_SECS,
    }
}

fn actuation_matches(target: f32, readback: f32, tolerance: f32) -> bool {
    (target - readback).abs() <= tolerance
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MvVerificationTrigger {
    Scheduled,
    Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActuationAuditPolicy {
    Required,
    BestEffort,
}

#[derive(Debug, Clone, Copy)]
enum MvVerificationCallLimit {
    None,
    Restore(Instant),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MvVerificationLimitKind {
    Confirmation,
    Deadline,
    Restore,
}

async fn record_actuation_observation(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<i64>> {
    let Some(id) = pending.id else {
        return Ok(None);
    };
    match TuneMvActuationRow::record_observation(pool, id, checked_at, readback, quality).await {
        Ok(row) => Ok(Some(row.attempt_count)),
        Err(error) if policy == ActuationAuditPolicy::BestEffort => {
            tracing::error!(
                actuation_id = id,
                error = %error,
                "failed to record MV verification observation; continuing physical restore"
            );
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn record_final_actuation_observation(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    status: MvActuationStatus,
    detail: &str,
) -> Option<i64> {
    let id = pending.id?;
    match TuneMvActuationRow::record_final_observation(
        pool,
        id,
        checked_at,
        readback,
        quality,
        status,
        (!detail.is_empty()).then_some(detail),
    )
    .await
    {
        Ok(row) => Some(row.attempt_count),
        Err(error) => {
            tracing::error!(
                actuation_id = id,
                error = %error,
                "failed to record terminal MV verification observation"
            );
            None
        }
    }
}

async fn finalize_actuation_best_effort(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    status: MvActuationStatus,
    detail: &str,
) {
    let Some(id) = pending.id else {
        return;
    };
    if let Err(error) = TuneMvActuationRow::finalize(pool, id, status, Some(detail)).await {
        tracing::error!(
            actuation_id = id,
            error = %error,
            "failed to finalize MV actuation audit row"
        );
    }
}

async fn reject_replacement_for_pending_actuation(
    pool: &SqlitePool,
    tag: &str,
    tracker: &mut MvActuationTracker,
) -> anyhow::Result<AbortReason> {
    let pending = tracker
        .pending
        .take()
        .expect("called only when an MV actuation is pending");
    let (status, detail) = if pending.last_readback.is_some() {
        (
            MvActuationStatus::Failed,
            "a replacement relay command was requested while the prior MV readback remained outside tolerance",
        )
    } else {
        (
            MvActuationStatus::Unverified,
            "a replacement relay command was requested before an acceptable prior MV readback was available",
        )
    };
    finalize_actuation_best_effort(pool, &pending, status, detail).await;
    Ok(actuation_abort_reason(
        tag,
        &pending,
        pending.last_readback,
        Instant::now(),
    ))
}

fn verification_trigger(
    pending: &PendingMvActuation,
    now: Instant,
) -> Option<MvVerificationTrigger> {
    if now >= pending.deadline {
        Some(MvVerificationTrigger::Deadline)
    } else if pending.last_readback.is_none() && now >= pending.first_check_at {
        Some(MvVerificationTrigger::Scheduled)
    } else {
        None
    }
}

async fn wait_for_mv_verification(wakeup: Option<Instant>) {
    match wakeup {
        Some(wakeup) => tokio::time::sleep_until(wakeup).await,
        None => std::future::pending::<()>().await,
    }
}

fn mv_verification_read_limit(
    trigger: MvVerificationTrigger,
    pending: &PendingMvActuation,
    call_limit: MvVerificationCallLimit,
) -> (Instant, MvVerificationLimitKind) {
    let external = match call_limit {
        MvVerificationCallLimit::None => None,
        MvVerificationCallLimit::Restore(deadline) => {
            Some((deadline, MvVerificationLimitKind::Restore))
        }
    };
    if trigger == MvVerificationTrigger::Deadline {
        let deadline_read_limit = (
            Instant::now() + MV_ACTUATION_DEADLINE_READ_MAX,
            MvVerificationLimitKind::Deadline,
        );
        return match external {
            Some(external) if external.0 < deadline_read_limit.0 => external,
            _ => deadline_read_limit,
        };
    }
    match external {
        Some(external @ (deadline, _)) if deadline < pending.deadline => external,
        _ => (pending.deadline, MvVerificationLimitKind::Confirmation),
    }
}

/// Checks the accepted OPC DA MV command without producing a tune sample. Transport failures
/// remain ordinary failed-run errors, an individual operation timeout remains
/// [`AbortReason::OperationTimedOut`], and rejected quality remains [`AbortReason::PoorQuality`].
/// Only a finite, acceptable-quality mismatch can become
/// [`AbortReason::MvActuationUnconfirmed`]. Successful verification-read duration is optionally
/// recorded when this is called from the polling loop.
#[allow(clippy::too_many_arguments)]
async fn verify_pending_mv_actuation_with_timing(
    pool: &SqlitePool,
    _args: &TuneArgs,
    effective_timing: EffectiveTiming,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
    audit_policy: ActuationAuditPolicy,
    mut timing: Option<&mut PollTimingAccumulator>,
) -> anyhow::Result<Option<AbortReason>> {
    let Some(pending) = tracker.pending.as_ref() else {
        return Ok(None);
    };
    let now = Instant::now();
    if trigger == MvVerificationTrigger::Scheduled && now < pending.first_check_at {
        return Ok(None);
    }

    match read_pending_mv_verification_with_timing(
        effective_timing,
        tag,
        driver,
        ctrl_c,
        tracker,
        trigger,
        call_limit,
    )
    .await?
    {
        PendingMvVerificationRead::DeadlineTimedOut {
            pending,
            checked_at,
            checked_instant,
        } => {
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                pending.last_readback,
                None,
                MvActuationStatus::Unverified,
                "the fresh MV read at the confirmation deadline did not finish within its bounded verification window",
            )
            .await;
            Ok(Some(actuation_abort_reason(
                tag,
                &pending,
                pending.last_readback,
                checked_instant,
            )))
        }
        PendingMvVerificationRead::RestoreTimedOut {
            pending,
            checked_at,
            checked_instant,
        } => {
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                pending.last_readback,
                None,
                MvActuationStatus::Unverified,
                "the restore timeout elapsed before MV confirmation completed",
            )
            .await;
            Ok(Some(actuation_abort_reason(
                tag,
                &pending,
                pending.last_readback,
                checked_instant,
            )))
        }
        PendingMvVerificationRead::Ready {
            operation,
            checked_at,
            checked_instant,
            read_duration,
        } => match resolve_pending_mv_read(
            pool,
            effective_timing,
            tag,
            tracker,
            operation,
            checked_at,
            checked_instant,
            allow_uncertain_quality,
        )
        .await?
        {
            PendingMvVerificationResult::Abort(reason) => Ok(Some(reason)),
            PendingMvVerificationResult::Value(value) => {
                if let Some(timing) = timing.as_mut() {
                    timing.observe_mv_verification(read_duration);
                }
                finalize_pending_mv_verification(pool, tag, tracker, value, trigger, audit_policy)
                    .await
            }
        },
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn verify_pending_mv_actuation_with(
    pool: &SqlitePool,
    args: &TuneArgs,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<AbortReason>> {
    verify_pending_mv_actuation_with_timing(
        pool,
        args,
        test_effective_timing(args),
        tag,
        driver,
        ctrl_c,
        allow_uncertain_quality,
        tracker,
        trigger,
        call_limit,
        audit_policy,
        None,
    )
    .await
}

enum PendingMvVerificationRead {
    Ready {
        operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
        read_duration: Duration,
    },
    DeadlineTimedOut {
        pending: PendingMvActuation,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
    },
    RestoreTimedOut {
        pending: PendingMvActuation,
        checked_at: DateTime<Utc>,
        checked_instant: Instant,
    },
}

struct PendingMvVerificationValue {
    readback: f32,
    sample_quality: SampleQuality,
    checked_at: DateTime<Utc>,
    checked_instant: Instant,
}

enum PendingMvVerificationResult {
    Value(PendingMvVerificationValue),
    Abort(AbortReason),
}

fn pending_verification_ready(
    tracker: &MvActuationTracker,
    operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
    read_duration: Duration,
) -> anyhow::Result<PendingMvVerificationRead> {
    let checked_instant = Instant::now();
    let pending = tracker
        .pending
        .as_ref()
        .expect("pending actuation existed after the bounded read");
    Ok(PendingMvVerificationRead::Ready {
        operation,
        checked_at: checked_at_for_pending(pending, checked_instant)?,
        checked_instant,
        read_duration,
    })
}

#[allow(clippy::too_many_arguments)]
async fn read_pending_mv_verification_with_timing(
    effective_timing: EffectiveTiming,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    mut trigger: MvVerificationTrigger,
    call_limit: MvVerificationCallLimit,
) -> anyhow::Result<PendingMvVerificationRead> {
    loop {
        let limit = {
            let pending = tracker
                .pending
                .as_ref()
                .expect("pending actuation existed before the bounded read");
            mv_verification_read_limit(trigger, pending, call_limit)
        };
        let read_started = Instant::now();
        let read = bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            read_numeric_sample(driver, tag),
        );
        let (deadline, limit_kind) = limit;
        match tokio::time::timeout_at(deadline, read).await {
            Ok(operation) => {
                return pending_verification_ready(tracker, operation, read_started.elapsed());
            }
            Err(_) => match limit_kind {
                MvVerificationLimitKind::Confirmation => {
                    trigger = MvVerificationTrigger::Deadline;
                }

                MvVerificationLimitKind::Deadline => {
                    let pending = tracker
                        .pending
                        .take()
                        .expect("pending actuation existed before the deadline read");
                    let checked_instant = Instant::now();
                    let checked_at = checked_at_for_pending(&pending, checked_instant)?;
                    return Ok(PendingMvVerificationRead::DeadlineTimedOut {
                        pending,
                        checked_at,
                        checked_instant,
                    });
                }
                MvVerificationLimitKind::Restore => {
                    let pending = tracker
                        .pending
                        .take()
                        .expect("pending actuation existed before the bounded read");
                    let checked_instant = Instant::now();
                    let checked_at = checked_at_for_pending(&pending, checked_instant)?;
                    return Ok(PendingMvVerificationRead::RestoreTimedOut {
                        pending,
                        checked_at,
                        checked_instant,
                    });
                }
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn resolve_pending_mv_read(
    pool: &SqlitePool,
    effective_timing: EffectiveTiming,
    tag: &str,
    tracker: &mut MvActuationTracker,
    operation: anyhow::Result<TickOperation<(f32, bhtune_driver::Quality)>>,
    checked_at: DateTime<Utc>,
    checked_instant: Instant,
    allow_uncertain_quality: bool,
) -> anyhow::Result<PendingMvVerificationResult> {
    let operation = match operation {
        Ok(operation) => operation,
        Err(error) => {
            let pending = tracker
                .pending
                .take()
                .expect("pending actuation existed before the verification read");
            let detail = format!("MV verification read failed: {error}");
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                None,
                None,
                MvActuationStatus::Unverified,
                &detail,
            )
            .await;
            return Err(error);
        }
    };

    let (readback, quality) = match operation {
        TickOperation::Completed(value) => value,
        TickOperation::Cancelled => {
            let pending = tracker
                .pending
                .take()
                .expect("pending actuation existed before the verification read");
            finalize_actuation_best_effort(
                pool,
                &pending,
                MvActuationStatus::Unverified,
                "MV verification was interrupted before confirmation completed",
            )
            .await;
            return Ok(PendingMvVerificationResult::Abort(
                AbortReason::UserInterrupt,
            ));
        }
        TickOperation::TimedOut => {
            let pending = tracker
                .pending
                .take()
                .expect("pending actuation existed before the verification read");
            let detail = format!(
                "MV verification read did not complete within {} seconds",
                effective_timing.op_timeout_secs
            );
            record_final_actuation_observation(
                pool,
                &pending,
                checked_at,
                None,
                None,
                MvActuationStatus::Unverified,
                &detail,
            )
            .await;
            return Ok(PendingMvVerificationResult::Abort(
                AbortReason::OperationTimedOut {
                    tag: tag.to_string(),
                    op_timeout_secs: effective_timing.op_timeout_secs,
                },
            ));
        }
    };
    let sample_quality = sample_quality_from_driver(quality);
    if check_quality(tag, quality, allow_uncertain_quality).is_err() {
        let pending = tracker
            .pending
            .take()
            .expect("pending actuation existed before the verification read");
        let detail = format!("MV verification read reported OPC quality {quality:?}");
        record_final_actuation_observation(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            MvActuationStatus::Unverified,
            &detail,
        )
        .await;
        return Ok(PendingMvVerificationResult::Abort(
            AbortReason::PoorQuality {
                tag: tag.to_string(),
                quality,
            },
        ));
    }

    Ok(PendingMvVerificationResult::Value(
        PendingMvVerificationValue {
            readback,
            sample_quality,
            checked_at,
            checked_instant,
        },
    ))
}

#[allow(clippy::too_many_arguments)]
async fn resolve_pending_mv_poll(
    pool: &SqlitePool,
    effective_timing: EffectiveTiming,
    pv_mv_values: TickOperation<HashMap<String, TagValue>>,
    mv_tag: &str,
    checked_at: DateTime<Utc>,
    checked_instant: Instant,
    read_duration: Duration,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    timing: &mut PollTimingAccumulator,
) -> anyhow::Result<(Option<AbortReason>, bool)> {
    let operation = match pv_mv_values {
        TickOperation::Completed(values) => match read_numeric_from_batch(&values, mv_tag) {
            Ok(value) => Ok(TickOperation::Completed(value)),
            Err(error) => Err(error),
        },
        TickOperation::Cancelled => Ok(TickOperation::Cancelled),
        TickOperation::TimedOut => Ok(TickOperation::TimedOut),
    };

    match resolve_pending_mv_read(
        pool,
        effective_timing,
        mv_tag,
        tracker,
        operation,
        checked_at,
        checked_instant,
        allow_uncertain_quality,
    )
    .await?
    {
        PendingMvVerificationResult::Abort(reason) => Ok((Some(reason), false)),
        PendingMvVerificationResult::Value(value) => {
            timing.observe_mv_verification(read_duration);
            let outcome = finalize_pending_mv_verification(
                pool,
                mv_tag,
                tracker,
                value,
                MvVerificationTrigger::Scheduled,
                ActuationAuditPolicy::Required,
            )
            .await?;
            Ok((outcome, true))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn confirm_pending_mv_actuation(
    pool: &SqlitePool,
    tracker: &mut MvActuationTracker,
    pending: PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: f32,
    sample_quality: SampleQuality,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<()> {
    let recorded_attempts = match pending.id {
        Some(id) => {
            let result = TuneMvActuationRow::record_final_observation(
                pool,
                id,
                checked_at,
                Some(readback),
                Some(sample_quality),
                MvActuationStatus::Confirmed,
                None,
            )
            .await;
            match result {
                Ok(row) => Some(row.attempt_count),
                Err(error) if audit_policy == ActuationAuditPolicy::BestEffort => {
                    tracing::error!(
                        actuation_id = id,
                        error = %error,
                        "failed to record confirmed MV restore observation"
                    );
                    None
                }
                Err(error) => {
                    tracker.pending = Some(pending);
                    return Err(error.into());
                }
            }
        }
        None => None,
    };
    if let Some(attempt_count) = recorded_attempts.filter(|attempt_count| *attempt_count > 1) {
        tracing::warn!(
            actuation_id = ?pending.id,
            attempt_count,
            target = pending.target,
            readback,
            tolerance = pending.tolerance,
            "MV actuation confirmed after earlier unsuccessful observations"
        );
    }
    tracker.confirmed_mv = Some(pending.target);
    Ok(())
}

async fn finalize_pending_mv_verification(
    pool: &SqlitePool,
    tag: &str,
    tracker: &mut MvActuationTracker,
    value: PendingMvVerificationValue,
    trigger: MvVerificationTrigger,
    audit_policy: ActuationAuditPolicy,
) -> anyhow::Result<Option<AbortReason>> {
    let pending = tracker
        .pending
        .take()
        .expect("pending actuation existed before the verification result");
    if value.checked_instant > pending.deadline {
        let detail = if actuation_matches(pending.target, value.readback, pending.tolerance) {
            "MV readback matched the target only after the confirmation deadline"
        } else {
            "MV readback remained outside tolerance after the confirmation deadline"
        };
        record_final_actuation_observation(
            pool,
            &pending,
            value.checked_at,
            Some(value.readback),
            Some(value.sample_quality),
            MvActuationStatus::Failed,
            detail,
        )
        .await;
        return Ok(Some(actuation_abort_reason(
            tag,
            &pending,
            Some(value.readback),
            value.checked_instant,
        )));
    }
    if actuation_matches(pending.target, value.readback, pending.tolerance) {
        confirm_pending_mv_actuation(
            pool,
            tracker,
            pending,
            value.checked_at,
            value.readback,
            value.sample_quality,
            audit_policy,
        )
        .await?;
        return Ok(None);
    }

    let deadline_reached =
        trigger == MvVerificationTrigger::Deadline || value.checked_instant >= pending.deadline;
    if deadline_reached {
        record_final_actuation_observation(
            pool,
            &pending,
            value.checked_at,
            Some(value.readback),
            Some(value.sample_quality),
            MvActuationStatus::Failed,
            "MV readback remained outside tolerance at the confirmation deadline",
        )
        .await;
        return Ok(Some(actuation_abort_reason(
            tag,
            &pending,
            Some(value.readback),
            value.checked_instant,
        )));
    }

    let attempt_count = record_actuation_observation(
        pool,
        &pending,
        value.checked_at,
        Some(value.readback),
        Some(value.sample_quality),
        audit_policy,
    )
    .await?;
    tracing::warn!(
        actuation_id = ?pending.id,
        attempt_count,
        target = pending.target,
        readback = value.readback,
        tolerance = pending.tolerance,
        "MV readback is outside tolerance; confirmation remains pending"
    );
    let mut pending = pending;
    pending.last_readback = Some(value.readback);
    tracker.pending = Some(pending);
    Ok(None)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn verify_pending_mv_actuation(
    pool: &SqlitePool,
    args: &TuneArgs,
    tag: &str,
    driver: &dyn Driver,
    ctrl_c: &mut CtrlC,
    allow_uncertain_quality: bool,
    tracker: &mut MvActuationTracker,
    call_deadline: Option<Instant>,
) -> anyhow::Result<Option<AbortReason>> {
    let trigger = tracker
        .pending
        .as_ref()
        .and_then(|pending| verification_trigger(pending, Instant::now()))
        .unwrap_or(MvVerificationTrigger::Scheduled);
    verify_pending_mv_actuation_with_timing(
        pool,
        args,
        test_effective_timing(args),
        tag,
        driver,
        ctrl_c,
        allow_uncertain_quality,
        tracker,
        trigger,
        call_deadline.map_or(
            MvVerificationCallLimit::None,
            MvVerificationCallLimit::Restore,
        ),
        ActuationAuditPolicy::Required,
        None,
    )
    .await
}

async fn finalize_pending_for_run_best_effort(pool: &SqlitePool, run_id: i64, detail: &str) {
    if let Err(error) = TuneMvActuationRow::finalize_pending_for_run(
        pool,
        run_id,
        MvActuationStatus::Unverified,
        Some(detail),
    )
    .await
    {
        tracing::error!(
            run_id,
            error = %error,
            "failed to finalize pending MV actuation rows"
        );
    }
}

async fn supersede_pending_actuation_best_effort(
    pool: &SqlitePool,
    tracker: &mut MvActuationTracker,
    detail: &str,
) {
    let Some(pending) = tracker.pending.take() else {
        return;
    };
    finalize_actuation_best_effort(pool, &pending, MvActuationStatus::Superseded, detail).await;
}

async fn record_handoff_observation_best_effort(
    pool: &SqlitePool,
    pending: &PendingMvActuation,
    checked_at: DateTime<Utc>,
    readback: Option<f32>,
    quality: Option<SampleQuality>,
    detail: &str,
) {
    record_final_actuation_observation(
        pool,
        pending,
        checked_at,
        readback,
        quality,
        MvActuationStatus::Superseded,
        detail,
    )
    .await;
}

fn checked_at_for_pending(
    pending: &PendingMvActuation,
    checked_instant: Instant,
) -> anyhow::Result<DateTime<Utc>> {
    Ok(pending.switch_tick
        + chrono::Duration::from_std(
            checked_instant.saturating_duration_since(pending.switch_instant),
        )
        .map_err(|_| anyhow::anyhow!("MV actuation observation time exceeded chrono's range"))?)
}

fn utc_after_elapsed(now: DateTime<Utc>, elapsed: Duration) -> anyhow::Result<DateTime<Utc>> {
    Ok(now
        + chrono::Duration::from_std(elapsed)
            .map_err(|_| anyhow::anyhow!("MV command time exceeded chrono's range"))?)
}

/// The outcome of [`attempt_restore_with_actuation`] -- whether the restore was confirmed to run
/// every applicable step to completion, or was abandoned/only partially successful because a
/// second Ctrl+C arrived, `[tuning].restore_timeout_secs` elapsed, or one or more individual restore
/// steps themselves failed.
enum RestoreAttempt {
    /// The restore ran to completion and [`RestoreReport::all_succeeded`] was `true`. A
    /// per-step failure is not a separate `Err` case -- it is folded into
    /// [`RestoreAttempt::Incomplete`]
    /// below via the report's own failure summary.
    Confirmed,
    /// The restore could not be confirmed: a second Ctrl+C arrived, `[tuning].restore_timeout_secs`
    /// elapsed, or one or more restore steps failed. `reason` is a
    /// human-readable description of which, for composing into the final
    /// [`RunOutcome::RestoreIncomplete`] message and the stderr warning already printed by
    /// [`warn_restore_incomplete`] before this variant is returned.
    Incomplete { reason: String },
}

enum RestoreMvOutcome {
    Continue(RestoreStepOutcome),
    Interrupted(String),
}

fn restore_mv_outcome_or_failed(result: anyhow::Result<RestoreMvOutcome>) -> RestoreMvOutcome {
    match result {
        Ok(outcome) => outcome,
        Err(error) => RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(error.to_string())),
    }
}

enum RestoreHandoffOutcome {
    Confirmed,
    Rewrite,
    Interrupted(String),
}

#[allow(clippy::too_many_arguments)]
async fn try_confirm_final_snapback_handoff_with_timing(
    pool: &SqlitePool,
    _args: &TuneArgs,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    restore_deadline: Instant,
) -> anyhow::Result<Option<RestoreHandoffOutcome>> {
    let is_final_snapback = tracker.pending.as_ref().is_some_and(|pending| {
        pending.kind == MvActuationKind::Relay && pending.target == initial_mv
    });
    if !is_final_snapback {
        return Ok(None);
    }

    let pending = tracker
        .pending
        .take()
        .expect("the final-snapback predicate required a pending actuation");
    let now = Instant::now();
    let reserved_restore_window = Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS);
    let latest_handoff_finish = restore_deadline
        .checked_sub(reserved_restore_window)
        .unwrap_or(now);
    if now >= latest_handoff_finish {
        finalize_actuation_best_effort(
            pool,
            &pending,
            MvActuationStatus::Superseded,
            "the authoritative restore skipped the final-snapback handoff read to preserve its full MV confirmation budget",
        )
        .await;
        return Ok(Some(RestoreHandoffOutcome::Rewrite));
    }
    let handoff_deadline = (now + MV_RESTORE_HANDOFF_READ_MAX).min(latest_handoff_finish);
    let read = tokio::time::timeout_at(
        handoff_deadline,
        bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            read_numeric_sample(driver, tag),
        ),
    )
    .await;
    let checked_instant = Instant::now();
    let checked_at = checked_at_for_pending(&pending, checked_instant)?;

    let operation = match read {
        Err(_) => {
            finalize_actuation_best_effort(
                pool,
                &pending,
                MvActuationStatus::Superseded,
                "the authoritative restore superseded the final MRFT snapback when its tightly bounded handoff read did not finish promptly",
            )
            .await;
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
        Ok(Ok(operation)) => operation,
        Ok(Err(error)) => {
            let detail = format!(
                "the authoritative restore superseded the final MRFT snapback after its handoff read failed: {error}"
            );
            record_handoff_observation_best_effort(pool, &pending, checked_at, None, None, &detail)
                .await;
            tracing::warn!(error = %error, "final MRFT snapback handoff read failed; issuing authoritative restore write");
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
    };

    let (readback, quality) = match operation {
        TickOperation::Completed(value) => value,
        TickOperation::Cancelled => {
            tracker.pending = Some(pending);
            return Ok(Some(RestoreHandoffOutcome::Interrupted(
                "a second Ctrl+C was received while confirming the final MRFT snapback".to_string(),
            )));
        }
        TickOperation::TimedOut => {
            let detail = format!(
                "the authoritative restore superseded the final MRFT snapback after its handoff read exceeded the {}s operation timeout",
                effective_timing.op_timeout_secs
            );
            record_handoff_observation_best_effort(pool, &pending, checked_at, None, None, &detail)
                .await;
            return Ok(Some(RestoreHandoffOutcome::Rewrite));
        }
    };
    let sample_quality = sample_quality_from_driver(quality);
    if check_quality(tag, quality, allow_uncertain_quality).is_err() {
        let detail = format!(
            "the authoritative restore superseded the final MRFT snapback after its handoff read reported OPC quality {quality:?}"
        );
        record_handoff_observation_best_effort(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            &detail,
        )
        .await;
        return Ok(Some(RestoreHandoffOutcome::Rewrite));
    }

    if actuation_matches(pending.target, readback, pending.tolerance) {
        record_handoff_observation_best_effort(
            pool,
            &pending,
            checked_at,
            Some(readback),
            Some(sample_quality),
            "the authoritative restore adopted and confirmed the final MRFT snapback; no duplicate MV write was issued",
        )
        .await;
        tracker.confirmed_mv = Some(initial_mv);
        return Ok(Some(RestoreHandoffOutcome::Confirmed));
    }

    record_handoff_observation_best_effort(
        pool,
        &pending,
        checked_at,
        Some(readback),
        Some(sample_quality),
        "the authoritative restore superseded an unconfirmed final MRFT snapback and issued a replacement MV write",
    )
    .await;
    Ok(Some(RestoreHandoffOutcome::Rewrite))
}

#[allow(clippy::too_many_arguments)]
async fn restore_mv_with_verification_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut Option<MvActuationTracker>,
    restore_deadline: Instant,
) -> anyhow::Result<RestoreMvOutcome> {
    let Some(tracker) = tracker.as_mut() else {
        let write = tokio::time::timeout_at(
            restore_deadline,
            bounded_driver_call(
                effective_timing.op_timeout_secs,
                ctrl_c,
                write_value(driver, tag, initial_mv),
            ),
        )
        .await;
        return Ok(match write {
            Err(_) => RestoreMvOutcome::Interrupted(format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            )),
            Ok(Err(error)) => {
                RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(error.to_string()))
            }
            Ok(Ok(operation)) => RestoreMvOutcome::Continue(match operation {
                TickOperation::Completed(()) => RestoreStepOutcome::Succeeded,
                TickOperation::Cancelled => {
                    return Ok(RestoreMvOutcome::Interrupted(
                        "a second Ctrl+C was received while restoring the MV".to_string(),
                    ));
                }
                TickOperation::TimedOut => RestoreStepOutcome::Failed(format!(
                    "MV restore write did not complete within {}s",
                    effective_timing.op_timeout_secs
                )),
            }),
        });
    };

    if tracker.pending.is_none() && tracker.confirmed_mv == Some(initial_mv) {
        return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
    }

    match try_confirm_final_snapback_handoff_with_timing(
        pool,
        args,
        effective_timing,
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        restore_deadline,
    )
    .await?
    {
        Some(RestoreHandoffOutcome::Confirmed) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
        }
        Some(RestoreHandoffOutcome::Interrupted(reason)) => {
            return Ok(RestoreMvOutcome::Interrupted(reason));
        }
        Some(RestoreHandoffOutcome::Rewrite) | None => {}
    }

    let tolerance =
        mv_actuation_uncapped_tolerance(initial_mv, tracker.previous_commanded_mv, tracker.mv_span);
    let write = tokio::time::timeout_at(
        restore_deadline,
        bounded_driver_call(
            effective_timing.op_timeout_secs,
            ctrl_c,
            write_value(driver, tag, initial_mv),
        ),
    )
    .await;
    match write {
        Err(_) => {
            return Ok(RestoreMvOutcome::Interrupted(format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            )));
        }
        Ok(Ok(TickOperation::Completed(()))) => {}
        Ok(Ok(TickOperation::Cancelled)) => {
            return Ok(RestoreMvOutcome::Interrupted(
                "a second Ctrl+C was received while restoring the MV".to_string(),
            ));
        }
        Ok(Ok(TickOperation::TimedOut)) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                format!(
                    "MV restore write did not complete within {}s",
                    effective_timing.op_timeout_secs
                ),
            )));
        }
        Ok(Err(error)) => {
            return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                error.to_string(),
            )));
        }
    }

    if tracker.pending.is_some() {
        supersede_pending_actuation_best_effort(
            pool,
            tracker,
            "the authoritative restore write replaced this pending relay command",
        )
        .await;
    }
    let accepted_instant = Instant::now();
    let accepted_at = Utc::now();
    tracker
        .record_restore_accepted_best_effort(
            pool,
            run_id,
            initial_mv,
            accepted_at,
            accepted_instant,
            tolerance,
        )
        .await;

    loop {
        let trigger = tracker
            .pending
            .as_ref()
            .and_then(|pending| verification_trigger(pending, Instant::now()))
            .unwrap_or(MvVerificationTrigger::Scheduled);
        let verification = verify_pending_mv_actuation_with_timing(
            pool,
            args,
            effective_timing,
            tag,
            driver,
            ctrl_c,
            allow_uncertain_quality,
            tracker,
            trigger,
            MvVerificationCallLimit::Restore(restore_deadline),
            ActuationAuditPolicy::BestEffort,
            None,
        )
        .await;
        match verification {
            Ok(None) if tracker.pending.is_none() => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded));
            }
            Ok(None) => {
                let pending = tracker
                    .pending
                    .as_ref()
                    .expect("pending state was checked above");
                let remaining_confirmation =
                    pending.deadline.saturating_duration_since(Instant::now());
                let remaining_restore = restore_deadline.saturating_duration_since(Instant::now());
                tokio::time::sleep(
                    MV_ACTUATION_RETRY_INTERVAL
                        .min(remaining_confirmation)
                        .min(remaining_restore),
                )
                .await;
            }
            Ok(Some(AbortReason::UserInterrupt)) => {
                return Ok(RestoreMvOutcome::Interrupted(
                    "a second Ctrl+C was received while confirming the restored MV".to_string(),
                ));
            }
            Ok(Some(_)) if Instant::now() >= restore_deadline => {
                return Ok(RestoreMvOutcome::Interrupted(format!(
                    "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                    effective_timing.restore_timeout_secs
                )));
            }
            Ok(Some(reason)) => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                    format!("MV restore could not be confirmed: {reason:?}"),
                )));
            }
            Err(error) => {
                return Ok(RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(
                    format!("MV restore verification failed: {error}"),
                )));
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn try_confirm_final_snapback_handoff(
    pool: &SqlitePool,
    args: &TuneArgs,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut MvActuationTracker,
    restore_deadline: Instant,
) -> anyhow::Result<Option<RestoreHandoffOutcome>> {
    try_confirm_final_snapback_handoff_with_timing(
        pool,
        args,
        test_effective_timing(args),
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        restore_deadline,
    )
    .await
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn restore_mv_with_verification(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    driver: &dyn Driver,
    tag: &str,
    initial_mv: f32,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    tracker: &mut Option<MvActuationTracker>,
    restore_deadline: Instant,
) -> anyhow::Result<RestoreMvOutcome> {
    restore_mv_with_verification_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
        driver,
        tag,
        initial_mv,
        allow_uncertain_quality,
        ctrl_c,
        tracker,
        restore_deadline,
    )
    .await
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
#[allow(clippy::too_many_arguments)]
async fn attempt_restore_with_actuation_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
) -> RestoreAttempt {
    let restore_deadline =
        Instant::now() + Duration::from_secs(effective_timing.restore_timeout_secs);
    let mv = match restore_mv_outcome_or_failed(
        restore_mv_with_verification_with_timing(
            pool,
            run_id,
            args,
            effective_timing,
            driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            allow_uncertain_quality,
            ctrl_c,
            mv_actuations,
            restore_deadline,
        )
        .await,
    ) {
        RestoreMvOutcome::Continue(outcome) => outcome,
        RestoreMvOutcome::Interrupted(reason) => {
            let _ = warn_restore_incomplete(tags, initial, &reason);
            return RestoreAttempt::Incomplete { reason };
        }
    };

    tokio::select! {
        report = restore_after_mv(driver, tags, template, initial, guard, mv) => {
            if report.all_succeeded() {
                RestoreAttempt::Confirmed
            } else {
                let reason = report
                    .failure_summary()
                    .unwrap_or_else(|| "one or more restore steps failed".to_string());
                let _ = warn_restore_incomplete(tags, initial, &reason);
                RestoreAttempt::Incomplete { reason }
            }
        }
        () = ctrl_c.signalled() => {
            let reason = "a second Ctrl+C was received while restoring the loop".to_string();
            let _ = warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
        () = tokio::time::sleep_until(restore_deadline) => {
            let reason = format!(
                "the restore did not complete within the {}s [tuning].restore_timeout_secs limit",
                effective_timing.restore_timeout_secs
            );
            let _ = warn_restore_incomplete(tags, initial, &reason);
            RestoreAttempt::Incomplete { reason }
        }
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn attempt_restore_with_actuation(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
) -> RestoreAttempt {
    attempt_restore_with_actuation_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await
}

/// Prints a loud, operator-facing warning (to stderr, so it survives `--output json` and any
/// stdout redirection) naming the MV tag and the pre-test value it may not have been
/// restored to, plus a matching `tracing::error!` for anyone mining logs rather than watching
/// the terminal. The loop's mode may also not have been reverted -- see `restore`'s own
/// mode/setpoint/mode-attribute steps -- but the MV is called out specifically since it is
/// the one value every template has and the one most directly consequential if left at a
/// relay-test extreme.
fn restore_incomplete_warning_message(
    tags: &LoopTags,
    initial: &InitialState,
    reason: &str,
) -> String {
    format!(
        "WARNING: could not confirm the loop was fully restored ({reason}). Tag '{}' may still be at its last relay-test value instead of its pre-test value {}. Check it -- and the loop's mode -- by hand.",
        tags.manipulated_variable, initial.mv_ini
    )
}

fn warn_restore_incomplete(tags: &LoopTags, initial: &InitialState, reason: &str) -> String {
    let message = restore_incomplete_warning_message(tags, initial, reason);
    eprintln!("{message}");
    tracing::error!(
        mv_tag = %tags.manipulated_variable,
        mv_ini = initial.mv_ini,
        reason,
        "loop restore could not be confirmed"
    );
    message
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

fn completed_oscillation_period_ms(
    poll_result: &anyhow::Result<PollOutcome>,
    direction: ControllerDirection,
    config: LoopConfig,
    pv_range: PvRange,
) -> Option<f64> {
    let Ok(PollOutcome::Completed(Action::Complete {
        peaks,
        troughs,
        switch_times,
        mv_sign_init,
    })) = poll_result
    else {
        return None;
    };

    let oscillation = measure_oscillation(
        peaks,
        troughs,
        switch_times,
        *mv_sign_init,
        direction,
        config,
        pv_range,
        TuningMathCompat::default(),
    );
    Some(f64::from(oscillation.period_minutes) * 60_000.0)
}

fn warn_on_missed_poll_opportunities(run_id: i64, metrics: &TimingMetrics) {
    if metrics.basis != TimingBasis::LiveMonotonic || metrics.missed_poll_opportunity_count == 0 {
        return;
    }

    tracing::warn!(
        run_id,
        requested_interval_ms = metrics.requested_interval_ms,
        sample_gap_count = metrics.sample_gap_count,
        mean_sample_gap_ms = metrics.mean_sample_gap_ms,
        max_sample_gap_ms = metrics.max_sample_gap_ms,
        missed_poll_opportunity_count = metrics.missed_poll_opportunity_count,
        "live tune missed at least one complete polling opportunity"
    );
}

/// Timing diagnostics are observational: every call site defers this database write until
/// after the safety-critical restore attempt, and a failure here must never replace the
/// tune's actual completion, abort, or driver-error outcome.
async fn record_timing_metrics_best_effort(pool: &SqlitePool, run_id: i64, metrics: TimingMetrics) {
    if let Err(e) = TuneRunRow::record_timing_metrics(pool, run_id, metrics).await {
        tracing::error!(run_id, error = %e, "failed to record tune timing metrics");
    }
}

async fn record_timing_metrics_if_present(
    pool: &SqlitePool,
    run_id: i64,
    metrics: Option<TimingMetrics>,
) {
    let Some(metrics) = metrics else { return };
    record_timing_metrics_best_effort(pool, run_id, metrics).await;
}

/// Attempts a best-effort restore, records its outcome, then returns `err` **unchanged** --
/// the single choke point every early-return error path in `execute` funnels through, so a
/// partial mutation is never left un-restored just because the step that failed came before
/// `attempt_restore` was reached (`safety-restore-guard`, finding 3 of the live-plant safety
/// review, fixing three such gaps: a failed `transition_to_manual`, a failed
/// `record_initial_readings`/`persist_results` after a successful test, and any other hard
/// failure from `run_polling_loop` itself). Always returns the *original* `err`:
/// neither an incomplete restore nor a failure recording its status should ever mask the
/// real reason the run is failing.
#[allow(clippy::too_many_arguments)]
async fn restore_best_effort_then_propagate_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    err: anyhow::Error,
) -> anyhow::Error {
    let attempt = attempt_restore_with_actuation_with_timing(
        pool,
        run_id,
        args,
        effective_timing,
        driver,
        tags,
        template,
        initial,
        guard,
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
    )
    .await;
    record_restore_status_best_effort(pool, run_id, &attempt).await;
    if let Err(finalize_error) = TuneMvActuationRow::finalize_pending_for_run(
        pool,
        run_id,
        MvActuationStatus::Unverified,
        Some("the run failed before MV confirmation completed"),
    )
    .await
    {
        tracing::error!(
            run_id,
            error = %finalize_error,
            "failed to finalize pending MV actuation rows"
        );
    }
    err
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn restore_best_effort_then_propagate(
    pool: &SqlitePool,
    run_id: i64,
    driver: &dyn Driver,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    guard: &MutationGuard,
    args: &TuneArgs,
    allow_uncertain_quality: bool,
    ctrl_c: &mut CtrlC,
    mv_actuations: &mut Option<MvActuationTracker>,
    err: anyhow::Error,
) -> anyhow::Error {
    restore_best_effort_then_propagate_with_timing(
        pool,
        run_id,
        driver,
        tags,
        template,
        initial,
        guard,
        args,
        test_effective_timing(args),
        allow_uncertain_quality,
        ctrl_c,
        mv_actuations,
        err,
    )
    .await
}

/// Distinguishes *why* [`run_polling_loop`] ended without a normal engine completion, so
/// `execute` can record and report the right [`AbortReason`].
enum PollOutcome {
    /// The engine reported [`Action::Complete`] and any post-completion
    /// `[tuning].mrft_delay_secs`
    /// padding has elapsed.
    Completed(Action),
    /// Ctrl+C, `[tuning].timeout_secs`, `[tuning].op_timeout_secs`, or a poor-quality PV sample ended the
    /// run before that.
    Aborted(AbortReason),
}

async fn insert_tune_sample_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    tick_index: i64,
    tick: Tick,
    state: bhtune_core::MrftState,
    sample_quality: SampleQuality,
    timing: &mut PollTimingAccumulator,
) -> anyhow::Result<()> {
    let started = Instant::now();
    TuneSampleRow::insert(pool, run_id, tick_index, tick, state, sample_quality).await?;
    timing.observe_sample_persist(started.elapsed());
    Ok(())
}

/// Polls the driver on the frozen global polling interval, driving `engine` once the pre-test
/// `[tuning].mrft_delay_secs` padding period has elapsed, and continuing to record (but not evaluate)
/// samples for the same padding period after completion. Returns `Ok(PollOutcome::Completed`
/// on a normal finish, `Ok(PollOutcome::Aborted)` if interrupted by Ctrl+C, by
/// the frozen whole-run timeout elapsing, or by a single driver call exceeding the frozen
/// operation timeout -- the last of these via [`bounded_driver_call`], which wraps
/// every driver read/write in the tick body so a stalled call is abandoned rather than
/// awaited forever, keeping Ctrl+C and both timeouts effective even mid-hung-read/write. The
/// outer `tokio::select!` below still separately covers the *idle* wait between ticks (via
/// `ctrl_c`, shared with every `bounded_driver_call` inside the winning tick body -- see
/// that function's doc comment for why reusing it across nested `select!`s is safe) and the
/// whole-run `[tuning].timeout_secs` deadline.
#[allow(clippy::too_many_arguments)]
async fn run_polling_loop_with_timing(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    effective_timing: EffectiveTiming,
    tags: &LoopTags,
    driver: &dyn Driver,
    engine: &mut MrftEngine,
    time_anchor: RunTimeAnchor,
    ctrl_c: &mut CtrlC,
    guard: &mut MutationGuard,
    allow_uncertain_quality: bool,
    timing: &mut PollTimingAccumulator,
    mv_actuations: &mut Option<MvActuationTracker>,
    config: LoopConfig,
) -> anyhow::Result<PollOutcome> {
    let start_time = time_anchor.utc();
    let mut tick_time =
        TickTimeSource::for_driver(args.driver, time_anchor, effective_timing.poll_interval_ms)?;
    let poll_interval = Duration::from_millis(effective_timing.poll_interval_ms);
    let mut next_poll_at = Instant::now();

    let pre_delay_end =
        start_time + chrono::Duration::seconds(i64::from(effective_timing.mrft_delay_secs));
    let mut tick_index: i64 = 0;
    let mut completion: Option<Action> = None;
    let mut post_delay_end: Option<DateTime<Utc>> = None;

    // A mandatory safety net for unattended operation: an unattended run must never be able
    // to perturb a live process indefinitely (a stuck relay, a misconfigured tag mapping that
    // never crosses hysteresis, a stalled driver read). Created once and raced via
    // `tokio::select!` on every iteration below, rather than checked only after each
    // completed tick, so it fires even if a single `read_f32` call itself hangs.
    let timeout = tokio::time::sleep(Duration::from_secs(effective_timing.timeout_secs));
    tokio::pin!(timeout);

    loop {
        let verification_wakeup = mv_actuations
            .as_ref()
            .and_then(MvActuationTracker::next_verification_wakeup);
        tokio::select! {
            biased;
            _ = wait_for_mv_verification(verification_wakeup) => {
                let trigger = mv_actuations
                    .as_ref()
                    .and_then(|tracker| tracker.pending.as_ref())
                    .and_then(|pending| verification_trigger(pending, Instant::now()))
                    .expect("a verification wakeup requires a due pending actuation");
                let tracker = mv_actuations
                    .as_mut()
                    .expect("a verification trigger requires an OPC DA tracker");
                let reason = verify_pending_mv_actuation_with_timing(
                    pool,
                    args,
                    effective_timing,
                    &tags.manipulated_variable,
                    driver,
                    ctrl_c,
                    allow_uncertain_quality,
                    tracker,
                    trigger,
                    MvVerificationCallLimit::None,
                    ActuationAuditPolicy::Required,
                    Some(timing),
                )
                .await?;
                return Ok(match reason {
                    Some(reason) => PollOutcome::Aborted(reason),
                    None => continue,
                });
            }
            _ = tokio::time::sleep_until(next_poll_at) => {
                let tick_started = Instant::now();
                next_poll_at = Instant::now() + poll_interval;
                let pending_actuation = mv_actuations
                    .as_ref()
                    .is_some_and(|tracker| tracker.pending.is_some());
                let pv_read_started = Instant::now();
                let (pv, quality, poll_provided_mv_evidence, batched_mv_abort_reason) = match bounded_driver_call(
                    effective_timing.op_timeout_secs,
                    ctrl_c,
                    read_poll_batch(
                        driver,
                        &tags.process_variable,
                        pending_actuation.then_some(tags.manipulated_variable.as_str()),
                    ),
                )
                .await?
                {
                    TickOperation::Completed(values) => {
                        timing.observe_pv_read(pv_read_started.elapsed());
                        let completed_at = Instant::now();
                        let (batched_mv_abort_reason, poll_provided_mv_evidence) =
                            if pending_actuation {
                                let pending = mv_actuations
                                    .as_ref()
                                    .and_then(|tracker| tracker.pending.as_ref())
                                    .expect("pending actuation existed for the batched poll");
                                let checked_at = checked_at_for_pending(pending, completed_at)?;
                                let tracker = mv_actuations
                                    .as_mut()
                                    .expect("pending actuation requires an OPC DA tracker");
                                resolve_pending_mv_poll(
                                    pool,
                                    effective_timing,
                                    TickOperation::Completed(values.clone()),
                                    &tags.manipulated_variable,
                                    checked_at,
                                    completed_at,
                                    pv_read_started.elapsed(),
                                    allow_uncertain_quality,
                                    tracker,
                                    timing,
                                )
                                .await?
                            } else {
                                (None, false)
                            };
                        let (pv, quality) = read_numeric_from_batch(&values, &tags.process_variable)?;
                        (
                            pv,
                            quality,
                            poll_provided_mv_evidence,
                            batched_mv_abort_reason,
                        )
                    }
                    TickOperation::Cancelled => {
                        if let Some(tracker) = mv_actuations.as_mut()
                            && tracker.pending.is_some()
                        {
                            let completed_at = Instant::now();
                            let pending = tracker
                                .pending
                                .as_ref()
                                .expect("pending actuation existed for the cancelled poll");
                            let checked_at = checked_at_for_pending(pending, completed_at)?;
                            let (reason, _) = resolve_pending_mv_poll(
                                pool,
                                effective_timing,
                                TickOperation::Cancelled,
                                &tags.manipulated_variable,
                                checked_at,
                                completed_at,
                                pv_read_started.elapsed(),
                                allow_uncertain_quality,
                                tracker,
                                timing,
                            )
                            .await?;
                            let reason =
                                reason.expect("a cancelled pending MV poll must abort the run");
                            return Ok(PollOutcome::Aborted(reason));
                        }
                        tracing::warn!(run_id, tick_index, "Ctrl+C received while reading the PV; aborting run");
                        return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                    }
                    TickOperation::TimedOut => {
                        if let Some(tracker) = mv_actuations.as_mut()
                            && tracker.pending.is_some()
                        {
                            let completed_at = Instant::now();
                            let pending = tracker
                                .pending
                                .as_ref()
                                .expect("pending actuation existed for the timed-out poll");
                            let checked_at = checked_at_for_pending(pending, completed_at)?;
                            let (reason, _) = resolve_pending_mv_poll(
                                pool,
                                effective_timing,
                                TickOperation::TimedOut,
                                &tags.manipulated_variable,
                                checked_at,
                                completed_at,
                                pv_read_started.elapsed(),
                                allow_uncertain_quality,
                                tracker,
                                timing,
                            )
                            .await?;
                            let reason =
                                reason.expect("a timed-out pending MV poll must abort the run");
                            return Ok(PollOutcome::Aborted(reason));
                        }
                        tracing::warn!(
                            run_id,
                            tick_index,
                            op_timeout_secs = effective_timing.op_timeout_secs,
                            tag = %tags.process_variable,
                            "[tuning].op_timeout_secs elapsed reading the PV; aborting run"
                        );
                        return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                            tag: tags.process_variable.clone(),
                            op_timeout_secs: effective_timing.op_timeout_secs,
                        }));
                    }
                };
                // Timestamp the value after it is actually read. For OPC DA this includes the
                // read's real monotonic latency; for the simulator it advances the logical
                // process clock by exactly one fixed poll step per successful PV sample.
                let tick_observed_instant = Instant::now();
                let now = tick_time.next_timestamp()?;
                timing.observe(now)?;
                let tick = Tick { time: now, pv };
                let sample_quality = sample_quality_from_driver(quality);

                if let Err(e) = check_quality(&tags.process_variable, quality, allow_uncertain_quality) {
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
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    return Ok(PollOutcome::Aborted(AbortReason::PoorQuality {
                        tag: tags.process_variable.clone(),
                        quality,
                    }));
                }

                if let Some(reason) = batched_mv_abort_reason {
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    return Ok(PollOutcome::Aborted(reason));
                }

                if completion.is_none() && now < pre_delay_end {
                    insert_tune_sample_with_timing(
                        pool,
                        run_id,
                        tick_index,
                        tick,
                        engine.state(),
                        sample_quality,
                        timing,
                    )
                    .await?;
                    timing.observe_tick_work(tick_started.elapsed());
                    tick_index += 1;
                    continue;
                }

                // When an earlier command is still pending, evaluate this tick against a
                // clone. The real engine is committed only if the preview does not request a
                // replacement command. This keeps the engine's counters/switch timestamp and
                // final-completion state causally behind physical MV confirmation.
                let state_before_step = engine.state();
                let actions = if mv_actuations
                    .as_ref()
                    .is_some_and(|tracker| tracker.pending.is_some())
                {
                    let mut preview = engine.clone();
                    let actions = preview.step(tick);
                    if actions.iter().any(|action| matches!(action, Action::WriteMv(_))) {
                        let tracker = mv_actuations
                            .as_mut()
                            .expect("a pending actuation requires an OPC DA tracker");
                        assert!(
                            poll_provided_mv_evidence,
                            "a pending batched poll must provide MV evidence before replacement preview"
                        );
                        let reason = reject_replacement_for_pending_actuation(
                            pool,
                            &tags.manipulated_variable,
                            tracker,
                        )
                        .await?;
                        insert_tune_sample_with_timing(
                            pool,
                            run_id,
                            tick_index,
                            tick,
                            state_before_step,
                            sample_quality,
                            timing,
                        )
                        .await?;
                        timing.observe_tick_work(tick_started.elapsed());
                        return Ok(PollOutcome::Aborted(reason));
                    }
                    *engine = preview;
                    actions
                } else {
                    engine.step(tick)
                };
                for action in actions {
                    match action {
                        Action::WriteMv(v) => {
                            let tolerance = mv_actuations
                                .as_ref()
                                .map(|tracker| {
                                    mv_actuation_tolerance(
                                        MvActuationKind::Relay,
                                        v,
                                        tracker.previous_commanded_mv,
                                        tracker.mv_span,
                                    )
                                })
                                .transpose()?;
                            guard.mv_written = true;
                                let mv_write_started = Instant::now();
                                match bounded_driver_call(
                                effective_timing.op_timeout_secs,
                                ctrl_c,
                                write_value(driver, &tags.manipulated_variable, v),
                            )
                            .await?
                            {
                                TickOperation::Completed(()) => {
                                    timing.observe_mv_write(mv_write_started.elapsed());
                                    if let (Some(tracker), Some(tolerance)) =
                                        (mv_actuations.as_mut(), tolerance)
                                    {
                                        let commanded_instant = Instant::now();
                                        record_relay_actuation(
                                            tracker,
                                            pool,
                                            run_id,
                                            v,
                                            now,
                                            tick_observed_instant,
                                            tick_observed_instant
                                                + Duration::from_secs(u64::from(
                                                    config.noise_protection_secs,
                                                )),
                                            commanded_instant
                                                .saturating_duration_since(tick_observed_instant),
                                            commanded_instant,
                                            tolerance,
                                        )
                                        .await?;
                                    }
                                }
                                TickOperation::Cancelled => {
                                    // A valid sample/tick is already in hand for this iteration
                                    // (unlike the PV-read timeout/cancel case above), so record
                                    // it before aborting -- same rationale as the quality-check
                                    // abort above.
                                    insert_tune_sample_with_timing(
                                        pool,
                                        run_id,
                                        tick_index,
                                        tick,
                                        state_before_step,
                                        sample_quality,
                                        timing,
                                    )
                                    .await?;
                                    timing.observe_tick_work(tick_started.elapsed());
                                    tracing::warn!(run_id, tick_index, "Ctrl+C received while writing the MV; aborting run");
                                    return Ok(PollOutcome::Aborted(AbortReason::UserInterrupt));
                                }
                                TickOperation::TimedOut => {
                                    insert_tune_sample_with_timing(
                                        pool,
                                        run_id,
                                        tick_index,
                                        tick,
                                        engine.state(),
                                        sample_quality,
                                        timing,
                                    )
                                    .await?;
                                    timing.observe_tick_work(tick_started.elapsed());
                                    tracing::warn!(
                                        run_id,
                                        tick_index,
                                        op_timeout_secs = effective_timing.op_timeout_secs,
                                        tag = %tags.manipulated_variable,
                                        "[tuning].op_timeout_secs elapsed writing the MV; aborting run"
                                    );
                                    return Ok(PollOutcome::Aborted(AbortReason::OperationTimedOut {
                                        tag: tags.manipulated_variable.clone(),
                                        op_timeout_secs: effective_timing.op_timeout_secs,
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
                            post_delay_end = Some(
                                now + chrono::Duration::seconds(i64::from(
                                    effective_timing.mrft_delay_secs,
                                )),
                            );
                        }
                    }
                }
                tracing::trace!(run_id, tick_index, pv, "recorded tune sample");
                insert_tune_sample_with_timing(
                    pool,
                    run_id,
                    tick_index,
                    tick,
                    engine.state(),
                    sample_quality,
                    timing,
                )
                .await?;
                timing.observe_tick_work(tick_started.elapsed());
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
                    timeout_secs = effective_timing.timeout_secs,
                    "[tuning].timeout_secs elapsed before completion; aborting run"
                );
                return Ok(PollOutcome::Aborted(AbortReason::Timeout {
                    timeout_secs: effective_timing.timeout_secs,
                }));
            }
        }
    }

    Ok(PollOutcome::Completed(completion.expect(
        "the loop only `break`s after `completion` is set",
    )))
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn run_polling_loop(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    tags: &LoopTags,
    driver: &dyn Driver,
    engine: &mut MrftEngine,
    time_anchor: RunTimeAnchor,
    ctrl_c: &mut CtrlC,
    guard: &mut MutationGuard,
    allow_uncertain_quality: bool,
    timing: &mut PollTimingAccumulator,
    mv_actuations: &mut Option<MvActuationTracker>,
    config: LoopConfig,
) -> anyhow::Result<PollOutcome> {
    run_polling_loop_with_timing(
        pool,
        run_id,
        args,
        test_effective_timing(args),
        tags,
        driver,
        engine,
        time_anchor,
        ctrl_c,
        guard,
        allow_uncertain_quality,
        timing,
        mv_actuations,
        config,
    )
    .await
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

    let results = calculate_all_checked(
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

    for result in results {
        let row = TuneResultRow::from_checked(run_id, result);
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
    new_write.allow_uncertain_quality = allow_uncertain;

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

/// Represents the selected calculated PID parameters and any reason they were not written.
///
/// The write-back operation itself is documented on [`maybe_write_back`].
enum WriteBackSelection<'a> {
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

fn result_write_back_error(result: &TuneResultRow) -> Option<String> {
    pid_parameters_for_result(result)
        .err()
        .map(|error| error.to_string())
}

fn select_named_write_back_result<'a>(
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

fn select_interactive_write_back_result<'a>(
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

fn select_write_back_result<'a>(
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

fn finish_write_back(
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

fn prints_table_output(output: OutputFormat) -> bool {
    matches!(output, OutputFormat::Table)
}

fn skips_interactive_prompt(write_pid: Option<ResponseLevel>, output: OutputFormat) -> bool {
    write_pid.is_none() && matches!(output, OutputFormat::Json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{ControllerTypeArg, DirectionArg, ProcessTypeArg};
    use bhtune_db::models::{SamplingAdequacy, TemplateOrigin};

    async fn seeded_pool() -> SqlitePool {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, Utc::now())
            .await
            .unwrap();
        pool
    }

    async fn start_opc_test_run(
        pool: &SqlitePool,
        name: &str,
    ) -> (i64, LoopConfig, DcsTemplate, LoopTags) {
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            pool,
            None,
            name,
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        (run.id, config, template, tags)
    }

    /// Keep the shared execution tests fast now that timing values come from global
    /// configuration rather than per-run arguments.
    fn test_config() -> crate::config::BhtuneConfig {
        crate::config::BhtuneConfig {
            tuning: crate::config::TuningConfig {
                mrft_delay_secs: Some(0),
                poll_interval_ms: Some(5),
                timeout_secs: Some(5),
                op_timeout_secs: Some(30),
                restore_timeout_secs: Some(30),
            },
            ..crate::config::BhtuneConfig::default()
        }
    }

    fn time_anchor_at(utc: DateTime<Utc>) -> RunTimeAnchor {
        RunTimeAnchor::from_parts(utc, tokio::time::Instant::now())
    }

    fn timing_for_args(args: &TuneArgs) -> PollTimingAccumulator {
        let basis = match args.driver {
            DriverKindArg::Opcda => TimingBasis::LiveMonotonic,
            DriverKindArg::Simulator => TimingBasis::SimulatedFixedStep,
        };
        PollTimingAccumulator::new(basis, args.poll_interval_ms)
    }

    #[test]
    fn simulator_driver_requires_every_fixed_range_and_direction_value() {
        let template = bhtune_core::built_in_templates().remove(0);
        for (field, clear) in [
            (
                "pv_range_high",
                (|args: &mut TuneArgs| args.pv_range_high = None) as fn(&mut TuneArgs),
            ),
            ("pv_range_low", |args: &mut TuneArgs| {
                args.pv_range_low = None
            }),
            ("mv_range_high", |args: &mut TuneArgs| {
                args.mv_range_high = None
            }),
            ("mv_range_low", |args: &mut TuneArgs| {
                args.mv_range_low = None
            }),
            ("direction", |args: &mut TuneArgs| args.direction = None),
        ] {
            let mut args = fast_simulator_args();
            clear(&mut args);
            let error = build_loop_tags(&args, &template).unwrap_err();
            assert!(error.to_string().contains(&field.replace('_', "-")));
        }
    }

    #[test]
    fn restore_mv_errors_become_failed_restore_steps() {
        let outcome = restore_mv_outcome_or_failed(Err(anyhow::anyhow!("restore failed")));

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(detail))
                if detail == "restore failed"
        ));
    }

    #[test]
    fn json_summary_renderer_has_a_displayable_fallback_for_an_encoding_failure() {
        let rendered = render_json_summary(&serde_json::json!({"run_id": 1}), |_| {
            Err::<String, _>("injected encoding failure")
        });
        assert_eq!(rendered, r#"{"error": "injected encoding failure"}"#);
    }

    fn valid_pid_result_row() -> TuneResultRow {
        TuneResultRow {
            id: 0,
            run_id: 1,
            response_level: ResponseLevel::Moderate,
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
    fn pid_result_validation_rejects_every_malformed_shape() {
        let mut result = valid_pid_result_row();
        result.status = TuningResultStatus::Invalid;
        result.invalid_reason =
            Some(bhtune_core::TuningResultInvalidReason::NonPositivePvAmplitude);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(error.to_string().contains("PV amplitude is not positive"));

        let mut result = valid_pid_result_row();
        result.invalid_reason = Some(bhtune_core::TuningResultInvalidReason::NonFiniteKp);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("has an invalid reason despite being marked valid")
        );

        let mut proportional_missing = valid_pid_result_row();
        proportional_missing.proportional = None;
        let mut integral_missing = valid_pid_result_row();
        integral_missing.integral = None;
        let mut derivative_missing = valid_pid_result_row();
        derivative_missing.derivative = None;
        for (result, expected) in [
            (proportional_missing, "missing its proportional value"),
            (integral_missing, "missing its integral value"),
            (derivative_missing, "missing its derivative value"),
        ] {
            assert!(
                pid_parameters_for_result(&result)
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }

        let mut result = valid_pid_result_row();
        result.integral = Some(f32::NAN);
        let error = pid_parameters_for_result(&result).unwrap_err();
        assert!(error.to_string().contains("non-finite PID value"));

        let valid = pid_parameters_for_result(&valid_pid_result_row()).unwrap();
        assert_eq!(valid.response_level, ResponseLevel::Moderate);
        assert_eq!(valid.proportional, 2.0);
    }

    #[test]
    fn write_back_selection_rejects_invalid_results_in_named_and_interactive_paths() {
        let mut invalid = valid_pid_result_row();
        invalid.status = TuningResultStatus::Invalid;
        invalid.invalid_reason =
            Some(bhtune_core::TuningResultInvalidReason::NonPositivePvAmplitude);
        let results = vec![invalid];

        assert!(matches!(
            select_named_write_back_result(&results, ResponseLevel::Moderate, OutputFormat::Table),
            WriteBackSelection::Failed(detail) if detail.contains("PV amplitude is not positive")
        ));
        assert!(matches!(
            select_named_write_back_result(&results, ResponseLevel::Moderate, OutputFormat::Json),
            WriteBackSelection::Failed(detail) if detail.contains("PV amplitude is not positive")
        ));

        let mut reader = std::io::Cursor::new(b"1\n");
        assert!(matches!(
            select_interactive_write_back_result(&results, &mut reader),
            WriteBackSelection::Failed(detail) if detail.contains("PV amplitude is not positive")
        ));
    }

    #[test]
    fn utc_after_elapsed_rejects_a_duration_outside_chrono_range() {
        let error = utc_after_elapsed(Utc::now(), Duration::MAX).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("MV command time exceeded chrono's range")
        );
    }

    #[tokio::test]
    async fn relay_actuation_timestamp_conversion_error_is_propagated() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let now = Utc::now();
        let instant = Instant::now();

        let error = record_relay_actuation(
            &mut tracker,
            &pool,
            0,
            55.0,
            now,
            instant,
            instant,
            Duration::MAX,
            instant,
            1.0,
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("MV command time exceeded chrono's range")
        );
        assert!(tracker.pending.is_none());
    }

    fn delayed_live_timing_metrics() -> TimingMetrics {
        TimingMetrics {
            basis: TimingBasis::LiveMonotonic,
            requested_interval_ms: 800,
            sample_gap_count: 2,
            mean_sample_gap_ms: Some(1_200.0),
            max_sample_gap_ms: Some(1_600.0),
            missed_poll_opportunity_count: 1,
            measured_oscillation_period_ms: None,
            approximate_samples_per_period: None,
            sampling_adequacy: SamplingAdequacy::NotAssessed,
            poll_latency: None,
        }
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
            // Keep ordinary tune tests bounded even when a mutation prevents the
            // simulator from completing. The dedicated timeout test overrides this.
            timeout_secs: 5,
            op_timeout_secs: 30,
            restore_timeout_secs: 30,
            notes: Some("test note".to_string()),
            yes: false,
            write_pid: None,
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
        let timing = runs[0]
            .timing_metrics
            .expect("completed simulator run should record timing diagnostics");
        assert_eq!(timing.basis, TimingBasis::SimulatedFixedStep);
        assert_eq!(timing.requested_interval_ms, 5);
        assert!(timing.sample_gap_count > 0);
        assert_eq!(timing.mean_sample_gap_ms, Some(5.0));
        assert_eq!(timing.max_sample_gap_ms, Some(5.0));
        assert_eq!(timing.missed_poll_opportunity_count, 0);
        assert!(
            timing
                .measured_oscillation_period_ms
                .is_some_and(|period| period > 0.0)
        );
        assert!(
            timing
                .approximate_samples_per_period
                .is_some_and(|samples| samples > 1.0)
        );

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
        for timing_field in [
            "mrft_delay",
            "mrft_delay_secs",
            "poll_interval_ms",
            "timeout_secs",
            "op_timeout_secs",
            "restore_timeout_secs",
        ] {
            assert!(
                request.get(timing_field).is_none(),
                "{timing_field} must not be part of the per-run request snapshot"
            );
        }
        assert_eq!(
            runs[0].effective_tuning,
            Some(EffectiveTuning {
                mrft_delay_secs: 0,
                poll_interval_ms: 5,
                timeout_secs: 5,
                op_timeout_secs: 30,
                restore_timeout_secs: 30,
            })
        );
        assert!(
            TuneMvActuationRow::list_for_run(&pool, runs[0].id)
                .await
                .unwrap()
                .is_empty(),
            "simulator runs must not create OPC DA MV actuation audit rows"
        );

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

    #[tokio::test]
    async fn prepared_tune_run_id_matches_the_persisted_run_id() {
        let pool = seeded_pool().await;
        let first = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let second = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();

        assert!(first.run_id() > 0);
        assert_eq!(second.run_id(), first.run_id() + 1);
    }

    #[tokio::test]
    async fn prepare_rejects_invalid_tag_overrides_before_creating_a_run() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.tag_overrides = Some(TagOverrides {
            process_variable: Some("bad\0tag".to_string()),
            ..TagOverrides::default()
        });

        let result = prepare(&pool, args, &test_config()).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("process_variable"));
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty()
        );
    }

    #[tokio::test]
    async fn drive_completes_a_prepared_simulator_run() {
        let pool = seeded_pool().await;
        let prepared = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let run_id = prepared.run_id();

        let outcome = drive(&pool, prepared, &mut CtrlC::never()).await.unwrap();

        assert_eq!(outcome, TuneOutcome::Completed);
        assert_eq!(
            TuneRunRow::get(&pool, run_id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            bhtune_db::models::TuneOutcome::Completed
        );
    }

    #[tokio::test]
    async fn drive_marks_a_prepared_run_failed_when_execution_errors() {
        let pool = seeded_pool().await;
        let mut prepared = prepare(&pool, fast_simulator_args(), &test_config())
            .await
            .unwrap();
        let run_id = prepared.run_id();
        prepared.driver = Box::new(MockDriver::default().empty_read(SIMULATOR_PV_TAG));

        let err = drive(&pool, prepared, &mut CtrlC::never())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("no value"));
        let run = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
        assert_eq!(run.outcome, bhtune_db::models::TuneOutcome::Failed);
        assert!(run.failure_reason.is_some());
    }

    /// Every range/direction override is CLI-supplied below, so `read_initial_values` never
    /// reads them from the driver; the mock only ever needs to answer for `pv_ini`/`mv_ini`
    /// and (for the Yokogawa template) has no mode/mode-attribute tags to read either. Fails
    /// starting at the 2nd `read` RPC call — after the one batched setup read — so the
    /// failure always lands on the first polling tick's PV read, deep inside
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
            .failing_read_from_call(2),
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

    /// `[tuning].mrft_delay_secs` is whole seconds (the smallest non-zero value costs ~1s of logical
    /// simulator time both before switching and after completion). Fixed-step timestamps make
    /// the result deterministic, but the polling interval still paces those ticks in real time;
    /// the SQLite-backed test cannot use Tokio's paused clock without also expiring sqlx's own
    /// pool timers.
    #[tokio::test]
    async fn mrft_delay_pads_the_run_with_extra_recorded_samples() {
        let pool = seeded_pool().await;
        let args = fast_simulator_args();
        // This test intentionally consumes about two seconds before ordinary MRFT work. Keep
        // its safety budget independent of a loaded CI host while retaining the real timeout.
        let mut config = test_config();
        config.tuning.mrft_delay_secs = Some(1);
        config.tuning.timeout_secs = Some(30);
        run(&pool, args, &config).await.unwrap();

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
    async fn mrft_delay_keeps_the_engine_idle_during_pre_test_padding() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let mut args = fast_simulator_args();
        args.mrft_delay = 1;
        args.cycles_count = Some(1_000);
        let config = build_loop_config(&args).unwrap();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "pre-delay",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto();
        let initial = sample_initial_state();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
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

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            true,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            !TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            driver.write_log().is_empty(),
            "MRFT writes must not occur during pre-test padding"
        );
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

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { ref tag, quality })
                if tag == &tags.process_variable && quality == bhtune_driver::Quality::Bad
        ));

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

    #[tokio::test]
    async fn live_sample_timestamp_includes_driver_read_delay() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.process_variable, Duration::from_millis(50))
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let config = build_loop_config(&args).unwrap();
        let started_at = DateTime::UNIX_EPOCH;
        let time_anchor = time_anchor_at(started_at);
        let run = TuneRunRow::start(
            &pool,
            None,
            "delayed-live-read",
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
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor,
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { .. })
        ));
        let samples = TuneSampleRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert!(
            samples[0].sample.time - started_at >= chrono::Duration::milliseconds(45),
            "live sample timestamp should include the driver's 50 ms read delay"
        );
    }

    // --- safety-cancellation: `[tuning].op_timeout_secs` / mid-tick Ctrl+C via `bounded_driver_call`

    /// Proves the wiring, not just the mechanism (see the dedicated `bounded_driver_call`
    /// unit tests below for that): a PV read that never resolves at all -- the gateway is
    /// down, DCOM is wedged, the network is black-holed -- must abort the run via
    /// `[tuning].op_timeout_secs` rather than hang the poll loop forever, exactly the scenario
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

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut {
                ref tag,
                op_timeout_secs,
            }) if tag == &tags.process_variable && op_timeout_secs == 1
        ));

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

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            true,
            &mut timing,
            &mut None,
            build_loop_config(&args).unwrap(),
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
        read_sequences:
            std::sync::Mutex<std::collections::HashMap<String, std::collections::VecDeque<String>>>,
        read_batches: std::sync::Mutex<Vec<Vec<String>>>,
        reverse_read_results: bool,
        writes: std::sync::Mutex<Vec<(String, String)>>,
        reject_writes: std::collections::HashSet<String>,
        error_reads: std::collections::HashSet<String>,
        error_writes: std::collections::HashSet<String>,
        empty_reads: std::collections::HashSet<String>,
        /// Tags whose `read`/`write` never resolves (`.await`s `std::future::pending`
        /// forever), simulating a stalled OPC DA call (gateway down, DCOM wedged, network
        /// black-holed) so `[tuning].op_timeout_secs`/Ctrl+C-during-a-tick can actually be exercised.
        /// Finite delays are configured separately below; both delay forms await before any
        /// mutex guard is acquired.
        hang_reads: std::collections::HashSet<String>,
        hang_writes: std::collections::HashSet<String>,
        /// Per-tag finite read latency, used to prove that live monotonic sample timestamps
        /// include the time spent awaiting the driver rather than being captured before it.
        read_delays: std::collections::HashMap<String, Duration>,
        /// Tags whose configured finite read delay was cancelled before it completed.
        cancelled_delayed_reads: std::sync::Mutex<std::collections::HashSet<String>>,
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
        /// elapsed time or a Ctrl+C race, unlike `[tuning].timeout_secs`/
        /// `[tuning].op_timeout_secs`-driven
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
        /// Per-tag prefix of float writes to perturb before later writes resume normal
        /// behavior, allowing an unconfirmed relay command followed by a healthy restore.
        prefix_write_offsets: std::collections::HashMap<String, (usize, f32)>,
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

        fn delaying_read(mut self, tag: &str, delay: Duration) -> MockDriver {
            self.read_delays.insert(tag.to_string(), delay);
            self
        }

        /// Returns multi-tag reads in reverse order, proving callers map responses by tag
        /// rather than relying on the driver's request-order convention.
        fn reversing_read_results(mut self) -> MockDriver {
            self.reverse_read_results = true;
            self
        }

        async fn apply_read_delay(&self, tags: &[String]) {
            if let Some(delay) = tags
                .iter()
                .filter_map(|tag| self.read_delays.get(tag))
                .max()
            {
                let delayed_tags = tags
                    .iter()
                    .filter(|tag| self.read_delays.contains_key(*tag))
                    .cloned()
                    .collect();
                let mut observer = DelayedReadObserver {
                    driver: self,
                    tags: delayed_tags,
                    completed: false,
                };
                tokio::time::sleep(*delay).await;
                observer.completed = true;
            }
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

        fn with_read_sequence(self, tag: &str, values: &[&str]) -> MockDriver {
            self.read_sequences.lock().unwrap().insert(
                tag.to_string(),
                values.iter().map(|value| (*value).to_string()).collect(),
            );
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

        fn distorting_first_writes(
            mut self,
            tag: &str,
            write_count: usize,
            offset: f32,
        ) -> MockDriver {
            self.prefix_write_offsets
                .insert(tag.to_string(), (write_count, offset));
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

        fn read_batches(&self) -> Vec<Vec<String>> {
            self.read_batches.lock().unwrap().clone()
        }

        fn next_read_count(&self, tag: &str) -> usize {
            let mut counts = self.read_counts.lock().unwrap();
            let count = counts.entry(tag.to_string()).or_insert(0);
            *count += 1;
            *count
        }

        fn quality_for_read(&self, tag: &str, count: usize) -> bhtune_driver::Quality {
            let baseline_quality = self
                .qualities
                .lock()
                .unwrap()
                .get(tag)
                .copied()
                .unwrap_or(bhtune_driver::Quality::Good);
            self.degrade_quality_after.get(tag).map_or(
                baseline_quality,
                |(good_reads, degraded)| {
                    if count > *good_reads {
                        *degraded
                    } else {
                        baseline_quality
                    }
                },
            )
        }

        fn read_tag(
            &self,
            tag: &str,
            store: &std::collections::HashMap<String, String>,
            sequences: &mut std::collections::HashMap<String, std::collections::VecDeque<String>>,
        ) -> bhtune_driver::DriverResult<Option<bhtune_driver::TagValue>> {
            if self.error_reads.contains(tag) {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock read error"),
                )));
            }
            if self.empty_reads.contains(tag) {
                return Ok(None);
            }

            let count = self.next_read_count(tag);
            if self
                .error_reads_after
                .get(tag)
                .is_some_and(|good_reads| count > *good_reads)
            {
                return Err(bhtune_driver::DriverError::Operation(Box::new(
                    std::io::Error::other("mock read error after good reads"),
                )));
            }

            let quality = self.quality_for_read(tag, count);
            let value = sequences
                .get_mut(tag)
                .and_then(std::collections::VecDeque::pop_front)
                .or_else(|| store.get(tag).cloned())
                .unwrap_or_default();
            Ok(Some(bhtune_driver::TagValue {
                tag: tag.to_string(),
                value,
                quality,
                timestamp: None,
            }))
        }

        fn delayed_read_was_cancelled(&self, tag: &str) -> bool {
            self.cancelled_delayed_reads.lock().unwrap().contains(tag)
        }
    }

    struct DelayedReadObserver<'a> {
        driver: &'a MockDriver,
        tags: Vec<String>,
        completed: bool,
    }

    impl Drop for DelayedReadObserver<'_> {
        fn drop(&mut self) {
            if !self.completed {
                self.driver
                    .cancelled_delayed_reads
                    .lock()
                    .unwrap()
                    .extend(self.tags.iter().cloned());
            }
        }
    }

    #[async_trait::async_trait]
    impl Driver for MockDriver {
        async fn read(
            &self,
            tags: &[String],
        ) -> bhtune_driver::DriverResult<Vec<bhtune_driver::TagValue>> {
            self.read_batches.lock().unwrap().push(tags.to_vec());
            if tags.iter().any(|tag| self.hang_reads.contains(tag)) {
                std::future::pending::<()>().await;
            }
            self.apply_read_delay(tags).await;
            let store = self.values.lock().unwrap();
            let mut sequences = self.read_sequences.lock().unwrap();
            let mut out = Vec::new();
            for tag in tags {
                if let Some(value) = self.read_tag(tag, &store, &mut sequences)? {
                    out.push(value);
                }
            }
            if self.reverse_read_results {
                out.reverse();
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
            let write_count = {
                let mut counts = self.write_counts.lock().unwrap();
                let count = counts.entry(tag.clone()).or_insert(0);
                *count += 1;
                *count
            };
            if self.reject_writes.contains(tag) {
                return Ok(bhtune_driver::WriteOutcome::failure("mock rejected write"));
            }
            if let Some(good_writes) = self.reject_writes_after.get(tag)
                && write_count > *good_writes
            {
                return Ok(bhtune_driver::WriteOutcome::failure(
                    "mock rejected write after good writes",
                ));
            }
            // Store the (possibly silently distorted) value that a subsequent read would
            // observe, while `writes` above kept a log of what was actually requested -- see
            // the `write_offsets` field doc comment.
            let stored = if let TagWrite::Float(f) = value {
                let prefix_offset = self
                    .prefix_write_offsets
                    .get(tag)
                    .filter(|(prefix, _)| write_count <= *prefix)
                    .map_or(0.0, |(_, offset)| *offset);
                (f + self
                    .write_offsets
                    .get(tag)
                    .copied()
                    .unwrap_or(prefix_offset))
                .to_string()
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

    #[test]
    fn relay_actuation_tolerance_uses_span_allowance_and_step_cap() {
        let uncapped = mv_actuation_tolerance(MvActuationKind::Relay, 60.0, 50.0, 100.0).unwrap();
        assert!(uncapped > 0.1);
        assert!(uncapped < 0.101);

        let capped = mv_actuation_tolerance(MvActuationKind::Relay, 50.2, 50.0, 100.0).unwrap();
        assert!((capped - 0.05).abs() < 1e-6);

        let restore = mv_actuation_tolerance(MvActuationKind::Restore, 50.0, 50.2, 100.0).unwrap();
        assert!(restore > capped);
    }

    #[test]
    fn relay_actuation_tolerance_rejects_a_step_below_the_f32_floor() {
        let error =
            mv_actuation_tolerance(MvActuationKind::Relay, 50.0 + f32::EPSILON, 50.0, 100.0)
                .unwrap_err();
        assert!(error.to_string().contains("too small to verify safely"));
        assert!(mv_actuation_tolerance(MvActuationKind::Relay, 50.005, 50.0, 100.0).is_err());
    }

    #[test]
    fn relay_actuation_tolerance_rejects_large_magnitude_step_below_precision_floor() {
        let previous = 100_000_000.0_f32;
        let target = previous + 8.0;
        assert_eq!(target - previous, 8.0);
        let error =
            mv_actuation_tolerance(MvActuationKind::Relay, target, previous, 100.0).unwrap_err();
        assert!(error.to_string().contains("too small to verify safely"));
    }

    fn pending_actuation(
        id: Option<i64>,
        kind: MvActuationKind,
        target: f32,
        first_check_at: Instant,
        deadline: Instant,
        last_readback: Option<f32>,
    ) -> PendingMvActuation {
        let now = Instant::now();
        PendingMvActuation {
            id,
            kind,
            target,
            tolerance: 0.1,
            switch_tick: Utc::now(),
            switch_instant: now,
            accepted_instant: now,
            first_check_at,
            deadline,
            last_readback,
        }
    }

    fn tracker_with_pending(pending: PendingMvActuation) -> MvActuationTracker {
        MvActuationTracker {
            next_sequence: 1,
            previous_commanded_mv: 55.0,
            confirmed_mv: None,
            pending: Some(pending),
            mv_span: 100.0,
        }
    }

    fn batched_mv_value(tag: &str, value: &str, quality: bhtune_driver::Quality) -> TagValue {
        TagValue {
            tag: tag.to_string(),
            value: value.to_string(),
            quality,
            timestamp: None,
        }
    }

    fn pending_poll_test_state() -> (
        SqlitePool,
        EffectiveTiming,
        MvActuationTracker,
        PollTimingAccumulator,
    ) {
        let args = {
            let mut args = fast_simulator_args();
            args.driver = DriverKindArg::Opcda;
            args
        };
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(10),
            None,
        );
        (
            SqlitePool::connect_lazy("sqlite::memory:").unwrap(),
            test_effective_timing(&args),
            tracker_with_pending(pending),
            timing_for_args(&args),
        )
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_reports_missing_mv_data() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let error = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Completed(HashMap::new()),
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("no value for tag"));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_reports_malformed_mv_data() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let values = HashMap::from([(
            "Unit1.LIC101.OP".to_string(),
            batched_mv_value(
                "Unit1.LIC101.OP",
                "not-a-number",
                bhtune_driver::Quality::Good,
            ),
        )]);
        let error = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Completed(values),
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("not a number"));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_preserves_a_cancelled_operation() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let (reason, provided_evidence) = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::Cancelled,
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap();

        assert_eq!(reason, Some(AbortReason::UserInterrupt));
        assert!(!provided_evidence);
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn resolve_pending_mv_poll_preserves_a_timed_out_operation() {
        let (pool, effective_timing, mut tracker, mut timing) = pending_poll_test_state();
        let (reason, provided_evidence) = resolve_pending_mv_poll(
            &pool,
            effective_timing,
            TickOperation::TimedOut,
            "Unit1.LIC101.OP",
            Utc::now(),
            Instant::now(),
            Duration::from_millis(1),
            false,
            &mut tracker,
            &mut timing,
        )
        .await
        .unwrap();

        assert!(matches!(
            reason,
            Some(AbortReason::OperationTimedOut {
                tag,
                op_timeout_secs: 30,
            }) if tag == "Unit1.LIC101.OP"
        ));
        assert!(!provided_evidence);
        assert!(tracker.pending.is_none());
    }

    #[test]
    fn mv_actuation_abort_format_falls_back_for_other_abort_reasons() {
        assert_eq!(
            format_mv_actuation_abort_reason(&AbortReason::UserInterrupt),
            "UserInterrupt"
        );
    }

    #[tokio::test]
    async fn audit_helpers_skip_rows_without_an_audit_id_or_pending_actuation() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Restore,
            45.0,
            now,
            now + Duration::from_secs(1),
            None,
        );

        assert_eq!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            record_final_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                MvActuationStatus::Confirmed,
                "",
            )
            .await,
            None
        );
        finalize_actuation_best_effort(
            &pool,
            &pending,
            MvActuationStatus::Superseded,
            "no audit row",
        )
        .await;

        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();
        supersede_pending_actuation_best_effort(&pool, &mut tracker, "nothing pending").await;
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn audit_helpers_apply_required_and_best_effort_failure_policies() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            Some(i64::MAX),
            MvActuationKind::Restore,
            45.0,
            now,
            now + Duration::from_secs(1),
            None,
        );
        pool.close().await;

        assert_eq!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::BestEffort,
            )
            .await
            .unwrap(),
            None
        );
        assert!(
            record_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                ActuationAuditPolicy::Required,
            )
            .await
            .is_err()
        );
        assert_eq!(
            record_final_actuation_observation(
                &pool,
                &pending,
                Utc::now(),
                Some(45.0),
                Some(SampleQuality::Good),
                MvActuationStatus::Confirmed,
                "closed pool",
            )
            .await,
            None
        );
        finalize_actuation_best_effort(
            &pool,
            &pending,
            MvActuationStatus::Superseded,
            "closed pool",
        )
        .await;
    }

    #[tokio::test]
    async fn replacement_before_any_readback_is_finalized_as_unverified() {
        let pool = seeded_pool().await;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        );
        let mut tracker = tracker_with_pending(pending);

        let reason =
            reject_replacement_for_pending_actuation(&pool, "Unit1.LIC101.OP", &mut tracker)
                .await
                .unwrap();

        assert!(matches!(
            reason,
            AbortReason::MvActuationUnconfirmed { readback: None, .. }
        ));
        assert!(tracker.pending.is_none());
    }

    #[tokio::test]
    async fn verification_without_pending_work_or_before_first_check_is_a_noop() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();

        assert_eq!(
            verify_pending_mv_actuation_with(
                &pool,
                &args,
                "Unit1.LIC101.OP",
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                MvVerificationTrigger::Scheduled,
                MvVerificationCallLimit::None,
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );

        let now = Instant::now();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now + Duration::from_secs(1),
            now + Duration::from_secs(2),
            None,
        ));
        assert_eq!(
            verify_pending_mv_actuation_with(
                &pool,
                &args,
                "Unit1.LIC101.OP",
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                MvVerificationTrigger::Scheduled,
                MvVerificationCallLimit::None,
                ActuationAuditPolicy::Required,
            )
            .await
            .unwrap(),
            None
        );
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn explicit_deadline_trigger_rejects_a_mismatch_before_the_clock_deadline() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-explicit-deadline").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let now = Instant::now();
        let mut tracker = MvActuationTracker::for_run(&args, &sample_initial_state()).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                now,
                Utc::now(),
                now,
                0.1,
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Deadline,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::Required,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn first_verification_uses_the_switch_tick_even_when_write_acceptance_is_late() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, _tags) =
            start_opc_test_run(&pool, "actuation-switch-causality").await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let switch_instant = Instant::now();
        let switch_tick = DateTime::UNIX_EPOCH + chrono::Duration::seconds(12);
        let accepted_instant = switch_instant + Duration::from_secs(3);
        let accepted_at = switch_tick + chrono::Duration::seconds(3);
        let first_check_at = switch_instant + Duration::from_secs(2);

        tracker
            .record_accepted_at_switch(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                switch_tick,
                switch_instant,
                first_check_at,
                accepted_at,
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let pending = tracker.pending.as_ref().unwrap();
        assert_eq!(pending.switch_tick, switch_tick);
        assert_eq!(pending.switch_instant, switch_instant);
        assert_eq!(pending.accepted_instant, accepted_instant);
        assert_eq!(pending.first_check_at, first_check_at);
        assert_eq!(
            pending.deadline,
            accepted_instant + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS)
        );
    }

    #[tokio::test]
    async fn accepted_mv_command_is_confirmed_without_waiting_when_readback_matches() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-confirmed").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let target = 55.0;
        write_value(&driver, &tags.manipulated_variable, target)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        let tolerance =
            mv_actuation_tolerance(MvActuationKind::Relay, target, initial.mv_ini, 100.0).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                target,
                commanded_instant,
                commanded_at,
                commanded_instant,
                tolerance,
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].readback_mv, Some(target));
    }

    #[tokio::test]
    async fn later_retry_can_confirm_after_an_earlier_mismatch() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-late-confirmation").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "55"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        for _ in 0..2 {
            assert_eq!(
                verify_pending_mv_actuation(
                    &pool,
                    &args,
                    &tags.manipulated_variable,
                    &driver,
                    &mut CtrlC::never(),
                    false,
                    &mut tracker,
                    None,
                )
                .await
                .unwrap(),
                None
            );
        }

        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].readback_mv, Some(55.0));
    }

    #[tokio::test]
    async fn early_mismatch_stays_pending_but_blocks_a_replacement_relay() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-mismatch").await;
        let driver = honeywell_driver_auto().distorting_write(&tags.manipulated_variable, -5.0);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let target = 55.0;
        write_value(&driver, &tags.manipulated_variable, target)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        let tolerance =
            mv_actuation_tolerance(MvActuationKind::Relay, target, initial.mv_ini, 100.0).unwrap();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                target,
                commanded_instant,
                commanded_at,
                commanded_instant,
                tolerance,
            )
            .await
            .unwrap();

        let first = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert_eq!(first, None);
        assert!(tracker.pending.is_some());

        let forced = reject_replacement_for_pending_actuation(
            &pool,
            &tags.manipulated_variable,
            &mut tracker,
        )
        .await
        .unwrap();
        assert!(matches!(
            forced,
            AbortReason::MvActuationUnconfirmed {
                target: 55.0,
                readback: Some(50.0),
                ..
            }
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn later_check_reads_fresh_instead_of_failing_from_an_earlier_mismatch() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "55"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        write_value(&driver, &tags.manipulated_variable, 55.0)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            verify_pending_mv_actuation(
                &pool,
                &args,
                &tags.manipulated_variable,
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                None,
            )
            .await
            .unwrap(),
            None
        );
        // Trigger another verification without crossing the real confirmation deadline. The
        // second read must be evaluated on its own rather than inheriting the first mismatch.
        tracker.pending.as_mut().unwrap().deadline = Instant::now() + Duration::from_secs(1);

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].readback_mv, Some(55.0));
    }

    #[tokio::test]
    async fn predeadline_read_is_dropped_and_late_matching_readback_still_fails() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-predeadline-bound").await;
        let driver = honeywell_driver_auto()
            .with_value(&tags.manipulated_variable, "55")
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(50));
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now() + Duration::from_millis(25);

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed {
                readback: Some(55.0),
                ..
            })
        ));
        assert!(tracker.pending.is_none());
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![tags.manipulated_variable.clone()]
            ],
            "the read cancelled at the deadline must not be reused as deadline evidence"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn fresh_deadline_read_is_tightly_bounded_below_the_operation_timeout() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline-read-bound").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now();

        let started = Instant::now();
        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed { readback: None, .. })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn stalled_shared_pv_mv_poll_is_cancelled_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-shared-poll-cancel").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_secs(2))
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.mrft_delay = 10;
        args.timeout_secs = 3;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant + Duration::from_secs(1),
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            Utc::now(),
            MrftCompat::default(),
        );
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            RunTimeAnchor::now(),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            driver.delayed_read_was_cancelled(&tags.manipulated_variable),
            "the shared PV/MV read must be dropped when Ctrl+C cancels the operation"
        );
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]]
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty(),
            "a cancelled shared read has no valid PV sample to persist"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 0);
    }

    #[tokio::test]
    async fn stalled_shared_pv_mv_poll_times_out_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-shared-poll-timeout").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.mrft_delay = 10;
        args.op_timeout_secs = 0;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut {
                ref tag,
                op_timeout_secs: 0,
            }) if tag == &tags.manipulated_variable
        ));
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]]
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty(),
            "a timed-out shared read has no valid PV sample to persist"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
        assert!(tracker.is_none() || tracker.as_ref().unwrap().pending.is_none());
    }

    #[tokio::test]
    async fn scheduled_mv_verification_keeps_an_early_mismatch_pending() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-early-mismatch").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let pending = tracker.pending.as_mut().unwrap();
        pending.first_check_at = accepted_instant;
        pending.deadline = accepted_instant + Duration::from_millis(200);
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![
                    tags.process_variable.clone(),
                    tags.manipulated_variable.clone()
                ],
                vec![tags.manipulated_variable.clone()]
            ]
        );
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[0].attempt_count, 3);
    }

    #[tokio::test]
    async fn verification_deadline_wakes_without_waiting_for_a_long_poll_interval() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-deadline-wakeup").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 2;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(25);
        let pending = tracker.pending.as_mut().unwrap();
        pending.first_check_at = deadline;
        pending.deadline = deadline;
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let started = Instant::now();
        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        let reads = driver.read_batches();
        assert_eq!(
            reads[0],
            vec![
                tags.process_variable.clone(),
                tags.manipulated_variable.clone()
            ]
        );
        assert_eq!(reads[1], vec![tags.manipulated_variable.clone()]);
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn due_mv_verification_precedes_a_due_pv_poll() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-before-due-poll").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.poll_interval_ms = 10_000;
        args.mrft_delay = 10;
        args.timeout_secs = 2;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        tracker.pending.as_mut().unwrap().deadline = Instant::now();
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        assert_eq!(
            driver.read_batches(),
            vec![vec![tags.manipulated_variable.clone()]],
            "a due verification deadline must be handled before the due PV poll"
        );
        assert!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_preview_uses_deadline_verification_and_records_the_abort_sample() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "deadline-preview-verification").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let now = Instant::now();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            35.0,
            now + Duration::from_secs(1),
            now,
            None,
        ));
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        assert_eq!(engine.state(), state_before);
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run_id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn replacement_preview_commits_after_confirming_the_prior_command() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "confirmed-preview-replacement").await;
        let driver = honeywell_driver_auto().degrade_quality_after(
            &tags.process_variable,
            1,
            bhtune_driver::Quality::Bad,
        );
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let now = Instant::now();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.pending = Some(pending_actuation(
            None,
            MvActuationKind::Relay,
            initial.mv_ini,
            now + Duration::from_secs(1),
            now + Duration::from_secs(2),
            None,
        ));
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::PoorQuality { .. })
        ));
        assert_ne!(engine.state(), state_before);
        assert_eq!(driver.write_log().len(), 1);
    }

    #[tokio::test]
    async fn pending_actuation_preview_does_not_commit_or_write_a_replacement_relay() {
        let pool = seeded_pool().await;
        let (run_id, config, _template, tags) =
            start_opc_test_run(&pool, "actuation-preview").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                35.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 35.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            verify_pending_mv_actuation(
                &pool,
                &args,
                &tags.manipulated_variable,
                &driver,
                &mut CtrlC::never(),
                false,
                &mut tracker,
                None,
            )
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            tracker.pending.as_ref().unwrap().last_readback,
            Some(initial.mv_ini)
        );
        let mut tracker = Some(tracker);
        let started_at = Utc::now();
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );
        let state_before = engine.state();
        let mut timing = timing_for_args(&args);

        let outcome = run_polling_loop(
            &pool,
            run_id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut tracker,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                readback: Some(45.0),
                ..
            })
        ));
        assert_eq!(engine.state(), state_before);
        assert!(driver.write_log().is_empty());
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![tags.manipulated_variable.clone()],
                vec![
                    tags.process_variable.clone(),
                    tags.manipulated_variable.clone()
                ],
            ],
            "the replacement preview must use the fresh batched PV/MV read from the preview tick"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn verification_operation_timeout_preserves_the_timed_out_outcome() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) = start_opc_test_run(&pool, "actuation-hang").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.op_timeout_secs = 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let started = Instant::now();
        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(
            outcome,
            Some(AbortReason::OperationTimedOut {
                op_timeout_secs: 1,
                ..
            })
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn ctrl_c_during_verification_preserves_pending_row_for_restore_handoff() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-ctrl-c").await;
        let hanging_driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let healthy_driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &hanging_driver,
            &mut ctrl_c,
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert_eq!(outcome, Some(AbortReason::UserInterrupt));
        assert!(tracker.pending.is_none());

        let mut tracker = Some(tracker);
        let restored = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &healthy_driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();
        assert!(matches!(
            restored,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn poor_quality_verification_preserves_the_poor_quality_outcome() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-quality-retry").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.manipulated_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            Some(AbortReason::PoorQuality {
                quality: bhtune_driver::Quality::Bad,
                ..
            })
        ));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].readback_quality, Some(SampleQuality::Bad));
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn verification_transport_error_is_an_ordinary_failed_run_error() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-timeout-retry").await;
        let driver = honeywell_driver_auto().erroring_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                commanded_instant,
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();

        let error = verify_pending_mv_actuation(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            None,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("driver operation failed"));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
        assert_eq!(rows[0].readback_mv, None);
    }

    #[tokio::test]
    async fn restore_verification_timeout_finalizes_the_pending_row_as_unverified() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-verification-timeout").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Restore,
                initial.mv_ini,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Restore, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Deadline,
            MvVerificationCallLimit::Restore(Instant::now()),
            ActuationAuditPolicy::BestEffort,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(AbortReason::MvActuationUnconfirmed { readback: None, .. })
        ));
        assert!(tracker.pending.is_none());
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].attempt_count, 1);
    }

    #[tokio::test]
    async fn required_confirmation_audit_failure_keeps_the_pending_state_for_retry() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-required-audit-failure").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "55");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        pool.close().await;

        let error = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Scheduled,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::Required,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("pool"));
        assert!(tracker.pending.is_some());
        assert_eq!(tracker.confirmed_mv, None);
    }

    #[tokio::test]
    async fn best_effort_confirmation_audit_failure_does_not_block_restore_state() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-best-effort-audit-failure").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "55");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Restore,
                55.0,
                accepted_instant,
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Restore, 55.0, 45.0, 100.0).unwrap(),
            )
            .await
            .unwrap();
        pool.close().await;

        let outcome = verify_pending_mv_actuation_with(
            &pool,
            &args,
            &tags.manipulated_variable,
            &driver,
            &mut CtrlC::never(),
            false,
            &mut tracker,
            MvVerificationTrigger::Scheduled,
            MvVerificationCallLimit::None,
            ActuationAuditPolicy::BestEffort,
        )
        .await
        .unwrap();

        assert_eq!(outcome, None);
        assert!(tracker.pending.is_none());
        assert_eq!(tracker.confirmed_mv, Some(55.0));
    }

    #[tokio::test]
    async fn restore_supersedes_final_snapback_still_pending_during_padding() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore").await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        write_value(&driver, &tags.manipulated_variable, initial.mv_ini)
            .await
            .unwrap();
        let commanded_instant = Instant::now();
        let commanded_at = Utc::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                commanded_instant + Duration::from_secs(10),
                commanded_at,
                commanded_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log().len(),
            1,
            "restore must adopt a confirmed final snapback instead of writing MV again"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MvActuationKind::Relay);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[0].attempt_count, 1);
        assert!(
            rows[0]
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("no duplicate MV write"))
        );
    }

    #[tokio::test]
    async fn restore_rewrites_an_unconfirmed_final_snapback_without_waiting_twice() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-rewrite").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "50");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                accepted_instant + Duration::from_secs(4),
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let started = Instant::now();
        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log(),
            vec![(
                tags.manipulated_variable.clone(),
                initial.mv_ini.to_string()
            )]
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[0].readback_mv, Some(50.0));
        assert_eq!(rows[1].kind, MvActuationKind::Restore);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn slow_snapback_handoff_reserves_time_for_authoritative_restore() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "actuation-restore-budget").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(1_100));
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.restore_timeout_secs = MV_ACTUATION_CONFIRMATION_SECS + 1;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial).unwrap();
        tracker.previous_commanded_mv = 55.0;
        let accepted_instant = Instant::now();
        tracker
            .record_accepted(
                &pool,
                run_id,
                MvActuationKind::Relay,
                initial.mv_ini,
                accepted_instant + Duration::from_secs(4),
                Utc::now(),
                accepted_instant,
                mv_actuation_tolerance(MvActuationKind::Relay, initial.mv_ini, 55.0, 100.0)
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut tracker = Some(tracker);

        let started = Instant::now();
        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        assert_eq!(
            driver.write_log(),
            vec![(
                tags.manipulated_variable.clone(),
                initial.mv_ini.to_string()
            )],
            "the handoff read must fall back before consuming the restore write budget"
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].status, MvActuationStatus::Superseded);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn snapback_handoff_skips_reads_that_would_consume_the_restore_budget() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let now = Instant::now();

        for restore_deadline in [
            now + Duration::from_secs(1),
            Instant::now() + Duration::from_secs(MV_ACTUATION_CONFIRMATION_SECS),
        ] {
            let pending = pending_actuation(
                None,
                MvActuationKind::Relay,
                45.0,
                now,
                now + Duration::from_secs(10),
                None,
            );
            let mut tracker = tracker_with_pending(pending);
            let outcome = try_confirm_final_snapback_handoff(
                &pool,
                &args,
                &driver,
                "Unit1.LIC101.OP",
                45.0,
                false,
                &mut CtrlC::never(),
                &mut tracker,
                restore_deadline,
            )
            .await
            .unwrap();

            assert!(matches!(outcome, Some(RestoreHandoffOutcome::Rewrite)));
            assert!(tracker.pending.is_none());
        }
        assert!(driver.read_batches().is_empty());
    }

    #[tokio::test]
    async fn snapback_handoff_failures_fall_back_to_an_authoritative_rewrite() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let restore_deadline = Instant::now() + Duration::from_secs(10);

        let cases = [
            honeywell_driver_auto().erroring_read("Unit1.LIC101.OP"),
            honeywell_driver_auto()
                .hanging_read("Unit1.LIC101.OP")
                .with_quality("Unit1.LIC101.OP", bhtune_driver::Quality::Good),
            honeywell_driver_auto().with_quality("Unit1.LIC101.OP", bhtune_driver::Quality::Bad),
        ];
        for (index, driver) in cases.into_iter().enumerate() {
            let now = Instant::now();
            let pending = pending_actuation(
                None,
                MvActuationKind::Relay,
                45.0,
                now,
                now + Duration::from_secs(10),
                None,
            );
            let mut tracker = tracker_with_pending(pending);
            if index == 1 {
                args.op_timeout_secs = 0;
            } else {
                args.op_timeout_secs = 30;
            }
            let outcome = try_confirm_final_snapback_handoff(
                &pool,
                &args,
                &driver,
                "Unit1.LIC101.OP",
                45.0,
                false,
                &mut CtrlC::never(),
                &mut tracker,
                restore_deadline,
            )
            .await
            .unwrap();

            assert!(matches!(outcome, Some(RestoreHandoffOutcome::Rewrite)));
            assert!(tracker.pending.is_none());
        }
    }

    #[tokio::test]
    async fn snapback_handoff_preserves_pending_state_when_ctrl_c_interrupts_the_read() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_read("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let now = Instant::now();
        let pending = pending_actuation(
            None,
            MvActuationKind::Relay,
            45.0,
            now,
            now + Duration::from_secs(10),
            None,
        );
        let mut tracker = tracker_with_pending(pending);
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = try_confirm_final_snapback_handoff(
            &pool,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            Some(RestoreHandoffOutcome::Interrupted(_))
        ));
        assert!(tracker.pending.is_some());
    }

    #[tokio::test]
    async fn restore_mv_propagates_an_interrupted_final_snapback_handoff() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_read("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let now = Instant::now();
        let mut tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            45.0,
            now,
            now + Duration::from_secs(10),
            None,
        )));
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();

        let outcome = restore_mv_with_verification(
            &pool,
            0,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(10),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("final MRFT snapback")
        ));
        assert!(tracker.as_ref().unwrap().pending.is_some());
    }

    #[tokio::test]
    async fn restore_mv_without_a_tracker_reports_an_operation_timeout() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_write("Unit1.LIC101.OP");
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 0;
        let mut tracker = None;

        let outcome = restore_mv_with_verification(
            &pool,
            0,
            &args,
            &driver,
            "Unit1.LIC101.OP",
            45.0,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(ref detail))
                if detail.contains("restore write did not complete")
        ));
    }

    #[tokio::test]
    async fn tracked_restore_write_handles_deadline_cancel_and_operation_timeout() {
        let pool = seeded_pool().await;
        let driver = honeywell_driver_auto().hanging_write("Unit1.LIC101.OP");
        let initial = sample_initial_state();

        let now = Instant::now();
        let mut deadline_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let deadline_outcome = restore_mv_with_verification(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut deadline_tracker,
            Instant::now(),
        )
        .await
        .unwrap();
        assert!(matches!(
            deadline_outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("[tuning].restore_timeout_secs")
        ));

        let now = Instant::now();
        let mut cancel_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tx.send(1).unwrap();
        let cancel_outcome = restore_mv_with_verification(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut ctrl_c,
            &mut cancel_tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            cancel_outcome,
            RestoreMvOutcome::Interrupted(ref detail) if detail.contains("second Ctrl+C")
        ));

        let now = Instant::now();
        let mut timeout_tracker = Some(tracker_with_pending(pending_actuation(
            None,
            MvActuationKind::Relay,
            55.0,
            now,
            now + Duration::from_secs(1),
            None,
        )));
        let mut timeout_args = fast_simulator_args();
        timeout_args.op_timeout_secs = 0;
        let timeout_outcome = restore_mv_with_verification(
            &pool,
            0,
            &timeout_args,
            &driver,
            "Unit1.LIC101.OP",
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut timeout_tracker,
            Instant::now() + Duration::from_secs(1),
        )
        .await
        .unwrap();
        assert!(matches!(
            timeout_outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Failed(ref detail))
                if detail.contains("restore write did not complete")
        ));
    }

    #[tokio::test]
    async fn restore_verification_retries_a_mismatch_then_confirms() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-retry-confirm").await;
        let driver =
            honeywell_driver_auto().with_read_sequence(&tags.manipulated_variable, &["50", "45"]);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Continue(RestoreStepOutcome::Succeeded)
        ));
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows[0].attempt_count, 2);
        assert_eq!(rows[0].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn ctrl_c_during_restore_verification_interrupts_after_the_write() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-verification-cancel").await;
        let driver = honeywell_driver_auto()
            .delaying_read(&tags.manipulated_variable, Duration::from_millis(500));
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(1);
        });

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut ctrl_c,
            &mut tracker,
            Instant::now() + Duration::from_secs(args.restore_timeout_secs),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("confirming the restored MV")
        ));
    }

    #[tokio::test]
    async fn restore_verification_reports_the_expired_restore_deadline() {
        let pool = seeded_pool().await;
        let (run_id, _config, _template, tags) =
            start_opc_test_run(&pool, "restore-verification-deadline").await;
        let driver = honeywell_driver_auto().hanging_read(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.op_timeout_secs = 30;
        let initial = sample_initial_state();
        let mut tracker = Some(MvActuationTracker::for_run(&args, &initial).unwrap());

        let outcome = restore_mv_with_verification(
            &pool,
            run_id,
            &args,
            &driver,
            &tags.manipulated_variable,
            initial.mv_ini,
            false,
            &mut CtrlC::never(),
            &mut tracker,
            Instant::now() + Duration::from_millis(30),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RestoreMvOutcome::Interrupted(ref detail)
                if detail.contains("[tuning].restore_timeout_secs")
        ));
    }

    #[tokio::test]
    async fn restore_attempts_mode_setpoint_and_attribute_after_mv_quality_failure() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_opc_test_run(&pool, "actuation-restore-quality").await;
        let driver = honeywell_driver_auto()
            .with_quality(&tags.manipulated_variable, bhtune_driver::Quality::Bad);
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial);
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let outcome = attempt_restore_with_actuation(
            &pool,
            run_id,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut tracker,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Incomplete { .. }));
        let writes = driver.write_log();
        let mv_index = writes
            .iter()
            .position(|(tag, _)| tag == &tags.manipulated_variable)
            .unwrap();
        let mode_index = writes
            .iter()
            .position(|(tag, _)| Some(tag) == tags.controller_mode.as_ref())
            .unwrap();
        assert!(
            mv_index < mode_index,
            "the MV must be restored while the loop is still in Manual"
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.setpoint_variable.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.mode_attribute.as_ref())
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, MvActuationKind::Restore);
        assert_eq!(rows[0].status, MvActuationStatus::Unverified);
        assert_eq!(rows[0].readback_quality, Some(SampleQuality::Bad));
    }

    #[tokio::test]
    async fn restore_audit_failure_does_not_prevent_any_physical_restore_step() {
        let pool = seeded_pool().await;
        let (run_id, _config, template, tags) =
            start_opc_test_run(&pool, "actuation-restore-audit-failure").await;
        pool.close().await;
        let driver = honeywell_driver_auto();
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let initial = sample_initial_state();
        let mut tracker = MvActuationTracker::for_run(&args, &initial);
        let guard = MutationGuard {
            mode_attribute_written: true,
            mode_written: true,
            mv_written: true,
        };

        let outcome = attempt_restore_with_actuation(
            &pool,
            run_id,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut tracker,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        let writes = driver.write_log();
        assert!(
            writes
                .iter()
                .any(|(tag, _)| tag == &tags.manipulated_variable)
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.controller_mode.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.setpoint_variable.as_ref())
        );
        assert!(
            writes
                .iter()
                .any(|(tag, _)| Some(tag) == tags.mode_attribute.as_ref())
        );
    }

    #[tokio::test]
    async fn completed_opc_run_audits_final_snapback_through_post_test_padding() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.cycles_skip = Some(0);
        args.cycles_count = Some(1);
        args.mrft_delay = 1;
        args.poll_interval_ms = 20;
        args.timeout_secs = 5;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-padding-snapback",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .with_read_sequence(&tags.process_variable, &["50", "55", "45", "55"]);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::Completed { .. }));
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].kind, MvActuationKind::Relay);
        assert_eq!(rows[2].target_mv, sample_initial_state().mv_ini);
        assert_eq!(rows[2].status, MvActuationStatus::Confirmed);
        assert!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .len()
                > 3,
            "post-test padding must continue recording PV samples"
        );
    }

    #[tokio::test]
    async fn execute_rejects_a_zero_effective_relay_step_before_any_mutation() {
        let pool = seeded_pool().await;
        let (run_id, config, template, tags) =
            start_opc_test_run(&pool, "actuation-zero-step").await;
        let driver = honeywell_driver_auto().with_value(&tags.manipulated_variable, "100");
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;

        let error = execute(
            &pool,
            run_id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("too small to verify safely"));
        assert!(driver.write_log().is_empty());
    }

    #[test]
    fn shared_restore_timeout_policy_keeps_simulator_positive_only() {
        assert!(validate_restore_timeout_secs(DriverKindArg::Simulator, 0).is_err());
        assert!(validate_restore_timeout_secs(DriverKindArg::Simulator, 1).is_ok());
        assert!(
            validate_restore_timeout_secs(DriverKindArg::Opcda, MV_ACTUATION_CONFIRMATION_SECS - 1)
                .is_err()
        );
        assert!(
            validate_restore_timeout_secs(DriverKindArg::Opcda, MV_ACTUATION_CONFIRMATION_SECS)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn prepare_rejects_short_opcda_restore_timeout_before_driver_or_database_mutation() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.server = Some("Mock.Server".to_string());
        args.bridge_host = Some("127.0.0.1:1".to_string());
        let mut config = test_config();
        config.tuning.restore_timeout_secs = Some(MV_ACTUATION_CONFIRMATION_SECS - 1);

        let error = prepare(&pool, args, &config)
            .await
            .err()
            .expect("short OPC DA restore timeout must be rejected");

        assert!(error.to_string().contains("tuning.restore_timeout_secs"));
        assert!(
            TuneRunRow::list(
                &pool,
                &bhtune_db::models::TuneRunFilter::default(),
                bhtune_db::models::Pagination::first(10),
            )
            .await
            .unwrap()
            .is_empty(),
            "validation must run before the tune_runs insert"
        );
    }

    #[tokio::test]
    async fn execute_aborts_and_records_reason_when_a_relay_command_is_unconfirmed() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.noise_protection_secs = Some(0);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-abort",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver =
            honeywell_driver_auto().distorting_first_writes(&tags.manipulated_variable, 1, -5.0);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed { .. })
        ));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert!(
            stored
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("MV actuation unconfirmed"))
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, MvActuationKind::Relay);
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
        assert_eq!(rows[1].kind, MvActuationKind::Restore);
        assert_eq!(rows[1].status, MvActuationStatus::Confirmed);
    }

    #[tokio::test]
    async fn execute_finalizes_pending_actuation_when_sample_persistence_fails() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.noise_protection_secs = Some(10);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-db-failure",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();
        let initial = sample_initial_state();
        let engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            time_anchor.utc(),
            MrftCompat::default(),
        );
        TuneSampleRow::insert(
            &pool,
            run.id,
            0,
            Tick {
                time: time_anchor.utc(),
                pv: initial.pv_ini,
            },
            engine.state(),
            SampleQuality::Good,
        )
        .await
        .unwrap();

        let error = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &honeywell_driver_auto(),
            config,
            time_anchor,
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("UNIQUE constraint failed"));
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert!(!rows.is_empty());
        assert!(
            rows.iter()
                .all(|row| row.status != MvActuationStatus::Pending)
        );
    }

    #[tokio::test]
    async fn restore_incomplete_takes_precedence_over_actuation_failure() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        args.noise_protection_secs = Some(0);
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-and-restore-fail",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            Utc::now(),
        )
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .distorting_first_writes(&tags.manipulated_variable, 1, -5.0)
            .rejecting_write_after(&tags.manipulated_variable, 1);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        assert_eq!(
            tune_outcome_for_run(&outcome),
            TuneOutcome::RestoreIncomplete
        );
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert!(
            stored
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("MV actuation unconfirmed"))
        );
        let rows = TuneMvActuationRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, MvActuationStatus::Failed);
    }

    #[tokio::test]
    async fn audit_cleanup_failure_does_not_override_restore_incomplete() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.driver = DriverKindArg::Opcda;
        let config = build_loop_config(&args).unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "actuation-cleanup-failure",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            started_at,
        )
        .await
        .unwrap();
        TuneMvActuationRow::insert_pending(
            &pool,
            run.id,
            NewTuneMvActuation {
                sequence: 0,
                kind: MvActuationKind::Relay,
                commanded_at: started_at,
                target_mv: 55.0,
                previous_commanded_mv: Some(45.0),
                tolerance: 0.1,
                confirmation_due_at: started_at + chrono::Duration::seconds(4),
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_actuation_cleanup \
             BEFORE UPDATE OF status ON tune_mv_actuations \
             WHEN OLD.status = 'pending' AND NEW.status = 'unverified' \
             BEGIN SELECT RAISE(FAIL, 'forced actuation cleanup failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        let driver = honeywell_driver_auto()
            .degrade_quality_after(&tags.process_variable, 1, bhtune_driver::Quality::Bad)
            .erroring_write(&tags.manipulated_variable);

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            RunTimeAnchor::now(),
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
    }

    // --- `check_quality`: finding 5's single enforcement choke point ------------------------

    #[test]
    fn check_quality_accepts_good_regardless_of_the_quality_policy() {
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, false).is_ok());
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Good, true).is_ok());
    }

    #[test]
    fn check_quality_rejects_uncertain_unless_the_policy_allows_it() {
        let err =
            check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, false).unwrap_err();
        assert!(err.to_string().contains("Uncertain"));
        assert!(err.to_string().contains("Unit1.LIC101.PV"));
        assert!(check_quality("Unit1.LIC101.PV", bhtune_driver::Quality::Uncertain, true).is_ok());
    }

    #[test]
    fn check_quality_never_accepts_bad_regardless_of_the_policy() {
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

    fn yokogawa_template() -> DcsTemplate {
        bhtune_core::built_in_templates()
            .into_iter()
            .find(|t| t.name == "Yokogawa CentumVP")
            .expect("Yokogawa CentumVP is a built-in template")
    }

    fn yokogawa_tags() -> LoopTags {
        LoopTags::derive_from_pv_tag("Unit1.FIC101.PV", &yokogawa_template())
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
    async fn read_initial_values_batches_the_opcda_tag_set_before_auto_setpoint() {
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
        assert_eq!(
            driver.read_batches(),
            vec![
                vec![
                    "Unit1.LIC101.PV".to_string(),
                    "Unit1.LIC101.OP".to_string(),
                    "Unit1.LIC101.MODE".to_string(),
                    "Unit1.LIC101.MODEATTR".to_string(),
                    "Unit1.LIC101.CTLACTN".to_string(),
                    "Unit1.LIC101.PVEUHI".to_string(),
                    "Unit1.LIC101.PVEULO".to_string(),
                    "Unit1.LIC101.CVEUHI".to_string(),
                    "Unit1.LIC101.CVEULO".to_string(),
                ],
                vec!["Unit1.LIC101.SP".to_string()],
            ]
        );
    }

    #[tokio::test]
    async fn read_initial_values_batches_manual_tags_without_reading_setpoint() {
        let template = yokogawa_template();
        let tags = yokogawa_tags();
        let driver = MockDriver::new(&[
            ("Unit1.FIC101.PV", "50.0"),
            ("Unit1.FIC101.MV", "45.0"),
            ("Unit1.FIC101.MODE", "MAN"),
            ("Unit1.FIC101.DR", "0"),
            ("Unit1.FIC101.SH", "100.0"),
            ("Unit1.FIC101.SL", "0.0"),
            ("Unit1.FIC101.MSH", "100.0"),
            ("Unit1.FIC101.MSL", "0.0"),
        ]);

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.mode_raw.as_deref(), Some("MAN"));
        assert_eq!(initial.setpoint_ini, None);
        assert_eq!(
            driver.read_batches(),
            vec![vec![
                "Unit1.FIC101.PV".to_string(),
                "Unit1.FIC101.MV".to_string(),
                "Unit1.FIC101.MODE".to_string(),
                "Unit1.FIC101.DR".to_string(),
                "Unit1.FIC101.SH".to_string(),
                "Unit1.FIC101.SL".to_string(),
                "Unit1.FIC101.MSH".to_string(),
                "Unit1.FIC101.MSL".to_string(),
            ]]
        );
    }

    #[tokio::test]
    async fn read_initial_values_deduplicates_tags_and_skips_fixed_overrides() {
        let template = honeywell_template();
        let mut tags = honeywell_tags();
        tags.manipulated_variable = tags.process_variable.clone();
        tags.controller_mode = None;
        tags.mode_attribute = None;
        tags.controller_direction = TagOrValue::Value(ControllerDirection::Reverse);
        tags.upper_pv_range = TagOrValue::Value(100.0);
        tags.lower_pv_range = TagOrValue::Value(0.0);
        tags.upper_mv_range = TagOrValue::Value(100.0);
        tags.lower_mv_range = TagOrValue::Value(0.0);
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "50.0")]);

        let initial = read_initial_values(&driver, &tags, &template, false)
            .await
            .unwrap();

        assert_eq!(initial.pv_ini, 50.0);
        assert_eq!(initial.mv_ini, 50.0);
        assert_eq!(initial.direction, ControllerDirection::Reverse);
        assert_eq!(
            driver.read_batches(),
            vec![vec!["Unit1.LIC101.PV".to_string()]]
        );
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
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "bad-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
            time_anchor,
            None,
            true,
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
    async fn execute_hard_fails_when_the_pv_tag_reports_uncertain_quality_policy_rejects_it() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto()
            .with_quality(&tags.process_variable, bhtune_driver::Quality::Uncertain);
        let config = build_loop_config(&fast_simulator_args()).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "uncertain-quality-initial",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
            time_anchor,
            None,
            false,
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
    async fn read_initial_values_and_transition_accept_uncertain_pv_quality_when_policy_allows_it()
    {
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
        // alone returned `Ok` -- the real proof the global quality policy has an effect,
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
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "invalid-mv-range",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
            time_anchor,
            None,
            true,
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
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "transition-to-manual-fails",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
            time_anchor,
            None,
            true,
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
    /// `persist_completed_results` (here, `persist_results` colliding with the
    /// `UNIQUE (run_id, response_level)` constraint) *after* a real, successful MRFT
    /// completion must still trigger a best-effort restore. Uses the real `SimulatorDriver`
    /// (via `crate::driver::build`, exactly like `a_ctrl_c_style_abort_restores_and_records_aborted`
    /// above) rather than a scripted `MockDriver`, since this needs an actual engine
    /// completion, not just a mocked one -- the simulator's `LoopTags` has no mode/setpoint/
    /// mode-attribute tags at all, so its restore only ever has the MV step to confirm.
    #[tokio::test]
    async fn execute_attempts_restore_when_persist_completed_results_fails() {
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let driver = crate::driver::build(&args).await.unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "finish-completed-run-fails",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
                kp: Some(1.0),
                ti_minutes: Some(1.0),
                td_minutes: Some(1.0),
                proportional: Some(1.0),
                integral: Some(1.0),
                derivative: Some(1.0),
                status: TuningResultStatus::Valid,
                invalid_reason: None,
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
            time_anchor,
            None,
            true,
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
        let timing = stored
            .timing_metrics
            .expect("cadence metrics should survive a post-poll persistence failure");
        assert!(timing.sample_gap_count > 0);
        assert_eq!(timing.measured_oscillation_period_ms, None);
        assert_eq!(timing.approximate_samples_per_period, None);
    }

    #[derive(Debug)]
    struct RestoreFailingSimulator {
        inner: bhtune_driver::SimulatorDriver,
        writes: std::sync::Mutex<u32>,
        successful_writes: u32,
    }

    #[async_trait::async_trait]
    impl Driver for RestoreFailingSimulator {
        async fn read(&self, tags: &[String]) -> bhtune_driver::DriverResult<Vec<TagValue>> {
            self.inner.read(tags).await
        }

        async fn write(
            &self,
            tag: &String,
            value: TagWrite,
        ) -> bhtune_driver::DriverResult<bhtune_driver::WriteOutcome> {
            let reject = {
                let mut writes = self.writes.lock().unwrap();
                *writes += 1;
                *writes > self.successful_writes
            };
            if reject {
                Ok(bhtune_driver::WriteOutcome::failure(
                    "restore intentionally rejected",
                ))
            } else {
                self.inner.write(tag, value).await
            }
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
    async fn execute_reports_restore_incomplete_after_a_completed_run_cannot_restore_mv() {
        let pool = seeded_pool().await;
        let args = fast_simulator_args();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = build_loop_tags(&args, &template).unwrap();
        let config = build_loop_config(&args).unwrap();
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "completed-restore-incomplete",
            TuneDriver::Simulator,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
        )
        .await
        .unwrap();
        let simulator = bhtune_driver::SimulatorDriver::new(
            SIMULATOR_PV_TAG,
            SIMULATOR_MV_TAG,
            bhtune_driver::FopdtConfig::new(
                args.sim_gain,
                args.sim_tau,
                args.sim_dead_time,
                args.poll_interval_ms as f32 / 1000.0,
            ),
            args.sim_initial_pv,
            args.sim_initial_mv,
            args.sim_seed,
        );
        let driver = RestoreFailingSimulator {
            inner: simulator,
            writes: std::sync::Mutex::new(0),
            successful_writes: 7,
        };

        let outcome = execute(
            &pool,
            run.id,
            &args,
            &template,
            &tags,
            &driver,
            config,
            time_anchor,
            None,
            false,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(outcome, RunOutcome::RestoreIncomplete { .. }));
        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            stored.restore_status,
            Some(bhtune_db::models::RestoreStatus::Incomplete)
        );
        assert!(matches!(
            driver.browse("").await,
            Err(bhtune_driver::DriverError::Unsupported {
                operation: "browse"
            })
        ));
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
        let time_anchor = RunTimeAnchor::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "poor-quality-abort-restore-incomplete",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            time_anchor.utc(),
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
            time_anchor,
            None,
            true,
            &mut CtrlC::never(),
            &mut std::io::empty(),
        )
        .await
        .unwrap();

        assert!(matches!(&outcome, RunOutcome::RestoreIncomplete { .. }));
        let outcome_text = format!("{outcome:?}");
        assert!(outcome_text.contains("run aborted"));
        assert!(outcome_text.contains("PoorQuality"));
        assert!(outcome_text.contains("MV"));

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

    #[test]
    fn parse_f32_value_accepts_trimmed_finite_numbers() {
        assert_eq!(parse_f32_value("Unit1.LIC101.PV", " 42.5 ").unwrap(), 42.5);
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

    #[tokio::test]
    async fn resolve_f32_reads_a_tag_backed_value() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "42.5")]);
        let value = resolve_f32(
            &driver,
            &TagOrValue::Tag("Unit1.LIC101.PV".to_string()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(value, 42.5);
    }

    #[tokio::test]
    async fn resolve_f32_from_batch_reads_a_tag_from_the_batch() {
        let values = HashMap::from([(
            "Unit1.LIC101.PV".to_string(),
            TagValue {
                tag: "Unit1.LIC101.PV".to_string(),
                value: "42.5".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);
        let value = resolve_f32_from_batch(
            &MockDriver::default(),
            &values,
            &TagOrValue::Tag("Unit1.LIC101.PV".to_string()),
            false,
        )
        .await
        .unwrap();
        assert_eq!(value, 42.5);
    }

    #[tokio::test]
    async fn resolve_direction_from_batch_reads_and_maps_a_tag() {
        let template = honeywell_template();
        let tags = TagOrValue::Tag("Unit1.LIC101.CTLACTN".to_string());
        let direction_tag = "Unit1.LIC101.CTLACTN".to_string();
        let values = HashMap::from([(
            direction_tag.clone(),
            TagValue {
                tag: direction_tag,
                value: "0".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);
        let direction =
            resolve_direction_from_batch(&MockDriver::default(), &values, &tags, &template, false)
                .await
                .unwrap();
        assert_eq!(direction, ControllerDirection::Direct);
    }

    #[tokio::test]
    async fn resolve_direction_reads_and_maps_a_tag_directly() {
        let template = honeywell_template();
        let driver = MockDriver::new(&[("Unit1.LIC101.CTLACTN", "0")]);
        let direction = resolve_direction(
            &driver,
            &TagOrValue::Tag("Unit1.LIC101.CTLACTN".to_string()),
            &template,
            false,
        )
        .await
        .unwrap();
        assert_eq!(direction, ControllerDirection::Direct);
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
    async fn read_pv_sample_rejects_non_finite_values() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "nan")]);
        let err = read_pv_sample(&driver, "Unit1.LIC101.PV")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("finite"));
    }

    #[test]
    fn read_batch_f32_returns_a_good_numeric_value() {
        let values = HashMap::from([(
            "Unit1.LIC101.PV".to_string(),
            TagValue {
                tag: "Unit1.LIC101.PV".to_string(),
                value: "42.5".to_string(),
                quality: bhtune_driver::Quality::Good,
                timestamp: None,
            },
        )]);

        assert_eq!(
            read_batch_f32(&values, "Unit1.LIC101.PV", false).unwrap(),
            42.5
        );
    }

    #[tokio::test]
    async fn read_poll_batch_maps_reordered_responses_by_tag() {
        let driver = MockDriver::new(&[("Unit1.LIC101.PV", "42.5"), ("Unit1.LIC101.OP", "60.0")])
            .reversing_read_results();

        let values = read_poll_batch(&driver, "Unit1.LIC101.PV", Some("Unit1.LIC101.OP"))
            .await
            .unwrap();

        assert_eq!(
            read_numeric_from_batch(&values, "Unit1.LIC101.PV")
                .unwrap()
                .0,
            42.5
        );
        assert_eq!(
            read_numeric_from_batch(&values, "Unit1.LIC101.OP")
                .unwrap()
                .0,
            60.0
        );
    }

    #[test]
    fn sample_quality_mapping_covers_all_driver_qualities() {
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Good),
            SampleQuality::Good
        );
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Uncertain),
            SampleQuality::Uncertain
        );
        assert_eq!(
            sample_quality_from_driver(bhtune_driver::Quality::Bad),
            SampleQuality::Bad
        );
    }

    #[test]
    fn completed_oscillation_period_is_reported_for_a_successful_poll_result() {
        let completion = PollOutcome::Completed(Action::Complete {
            peaks: vec![52.0, 48.0, 52.0],
            troughs: vec![46.0, 50.0],
            switch_times: vec![
                Utc::now(),
                Utc::now() + chrono::Duration::seconds(30),
                Utc::now() + chrono::Duration::seconds(60),
                Utc::now() + chrono::Duration::seconds(90),
                Utc::now() + chrono::Duration::seconds(120),
            ],
            mv_sign_init: 1,
        });
        let result = completed_oscillation_period_ms(
            &Ok(completion),
            ControllerDirection::Reverse,
            build_loop_config(&fast_simulator_args()).unwrap(),
            PvRange {
                high: 100.0,
                low: 0.0,
            },
        );
        assert!(result.is_some());
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
                    kp: Some(kp),
                    ti_minutes: Some(ti),
                    td_minutes: Some(td),
                    proportional: Some(p),
                    integral: Some(i),
                    derivative: Some(d),
                    status: TuningResultStatus::Valid,
                    invalid_reason: None,
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
    async fn maybe_write_back_reports_blank_and_n_as_no_selection() {
        for input in [b"\n".as_slice(), b"N\n".as_slice()] {
            let (pool, run_id) = run_with_recorded_results().await;
            let template = honeywell_template();
            let tags = honeywell_tags();
            let driver = honeywell_driver_auto();

            let (outcome, detail) = maybe_write_back(
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

            assert_eq!(outcome, WriteBackOutcome::Skipped);
            assert_eq!(
                detail.as_deref(),
                Some("skipped interactively (no selection made)")
            );
            assert!(
                TuneWriteRow::list_for_run(&pool, run_id)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
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
                    kp: Some(kp),
                    ti_minutes: Some(ti),
                    td_minutes: Some(td),
                    proportional: Some(p),
                    integral: Some(i),
                    derivative: Some(d),
                    status: TuningResultStatus::Valid,
                    invalid_reason: None,
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
    fn output_predicates_match_table_and_json_write_back_modes() {
        assert!(prints_table_output(OutputFormat::Table));
        assert!(!prints_table_output(OutputFormat::Json));
        assert!(skips_interactive_prompt(None, OutputFormat::Json));
        assert!(!skips_interactive_prompt(
            Some(ResponseLevel::Aggressive),
            OutputFormat::Json
        ));
        assert!(!skips_interactive_prompt(None, OutputFormat::Table));
    }

    #[test]
    fn delayed_live_timing_emits_the_observational_warning() {
        warn_on_missed_poll_opportunities(42, &delayed_live_timing_metrics());
    }

    #[tokio::test]
    async fn timing_persistence_failure_does_not_replace_the_run_outcome() {
        let pool = seeded_pool().await;
        pool.close().await;

        record_timing_metrics_best_effort(&pool, 42, delayed_live_timing_metrics()).await;
    }

    #[tokio::test]
    async fn present_timing_metrics_are_persisted_by_the_optional_wrapper() {
        let (pool, run_id) = run_with_recorded_results().await;
        let metrics = delayed_live_timing_metrics();

        record_timing_metrics_if_present(&pool, run_id, Some(metrics)).await;

        let stored = TuneRunRow::get(&pool, run_id).await.unwrap().unwrap();
        assert_eq!(stored.timing_metrics, Some(metrics));
    }

    #[test]
    fn restore_incomplete_warning_message_names_the_reason_and_mv_restore_target() {
        let message = restore_incomplete_warning_message(
            &honeywell_tags(),
            &sample_initial_state(),
            "restore timed out",
        );

        assert!(message.contains("restore timed out"));
        assert!(message.contains("Unit1.LIC101.OP"));
        assert!(message.contains("45"));
        assert!(message.contains("loop's mode"));
    }

    #[test]
    fn warn_restore_incomplete_returns_the_message_it_emits() {
        let tags = honeywell_tags();
        let initial = sample_initial_state();
        let message = warn_restore_incomplete(&tags, &initial, "restore timed out");

        assert_eq!(
            message,
            restore_incomplete_warning_message(&tags, &initial, "restore timed out")
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
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                tag: "Unit1.LIC101.OP".to_string(),
                target: 55.0,
                readback: Some(50.0),
                tolerance: 0.1,
                elapsed_ms: 4_000,
                deadline_secs: 4,
            })),
            TuneOutcome::ActuationFailed
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
        for output in [OutputFormat::Table, OutputFormat::Json] {
            assert_eq!(
                print_summary(
                    1,
                    &RunOutcome::Aborted(AbortReason::MvActuationUnconfirmed {
                        tag: "Unit1.LIC101.OP".to_string(),
                        target: 55.0,
                        readback: Some(50.0),
                        tolerance: 0.1,
                        elapsed_ms: 4_000,
                        deadline_secs: 4,
                    }),
                    output,
                ),
                TuneOutcome::ActuationFailed
            );
        }
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
        let result: TickOperation<()> = bounded_driver_call(
            30,
            &mut ctrl_c,
            std::future::pending::<anyhow::Result<()>>(),
        )
        .await
        .unwrap();
        assert!(matches!(result, TickOperation::Cancelled));
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_driver_call_returns_timed_out_when_the_driver_call_stalls() {
        // No `SqlitePool` involved here (unlike the `run_polling_loop`-level tests), so
        // `start_paused` is safe -- see the precedent/caveat noted on the timeout test above.
        let mut ctrl_c = CtrlC::never();
        let result: TickOperation<()> =
            bounded_driver_call(1, &mut ctrl_c, std::future::pending::<anyhow::Result<()>>())
                .await
                .unwrap();
        assert!(matches!(result, TickOperation::TimedOut));
    }

    #[tokio::test]
    async fn a_stalled_pv_read_during_a_tick_is_cancelled_without_recording_a_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_read(&tags.process_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 30;
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
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = tx.send(1);
        });

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut ctrl_c,
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::UserInterrupt)
        ));
        assert!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_stalled_mv_write_during_a_tick_times_out_after_recording_the_sample() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(&tags.manipulated_variable);
        let mut args = fast_simulator_args();
        args.op_timeout_secs = 1;
        let config = build_loop_config(&args).unwrap();
        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "stalled-mv-write-timeout",
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
        let mut engine = MrftEngine::new(
            config,
            initial.direction,
            lookup(
                config.process_type,
                config.controller_type,
                ResponseLevel::Aggressive,
            )
            .beta,
            InitialReadings {
                pv_ini: initial.pv_ini,
                mv_ini: initial.mv_ini,
                mv_range_low: initial.mv_range_low,
                mv_range_high: initial.mv_range_high,
            },
            started_at,
            MrftCompat::default(),
        );

        let mut timing = timing_for_args(&args);
        let outcome = run_polling_loop(
            &pool,
            run.id,
            &args,
            &tags,
            &driver,
            &mut engine,
            time_anchor_at(started_at),
            &mut CtrlC::never(),
            &mut MutationGuard::default(),
            false,
            &mut timing,
            &mut None,
            config,
        )
        .await
        .unwrap();

        assert!(matches!(
            outcome,
            PollOutcome::Aborted(AbortReason::OperationTimedOut { ref tag, op_timeout_secs })
                if tag == &tags.manipulated_variable && op_timeout_secs == 1
        ));
        assert_eq!(
            TuneSampleRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    // --- `attempt_restore` / `RestoreAttempt`: confirmed vs. incomplete, and both ways to ---
    // --- become incomplete (a second Ctrl+C, and `[tuning].restore_timeout_secs` elapsing) ---

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
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let args = fast_simulator_args();

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        assert!(matches!(outcome, RestoreAttempt::Confirmed));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test]
    async fn record_restore_status_best_effort_swallows_database_errors() {
        let pool = seeded_pool().await;
        record_restore_status_best_effort(&pool, i64::MAX, &RestoreAttempt::Confirmed).await;
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
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let mut args = fast_simulator_args();
        args.restore_timeout_secs = MV_ACTUATION_CONFIRMATION_SECS;
        args.op_timeout_secs = 30;

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("[tuning].restore_timeout_secs"));
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
        let pool = seeded_pool().await;
        let args = fast_simulator_args();

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut ctrl_c,
            &mut None,
        )
        .await;

        match outcome {
            RestoreAttempt::Incomplete { reason } => {
                assert!(reason.contains("second Ctrl+C"));
            }
            RestoreAttempt::Confirmed => panic!("expected RestoreAttempt::Incomplete"),
        }
    }

    #[tokio::test]
    async fn restore_wrapper_handles_ctrl_c_after_mv_restore_completes() {
        let pool = seeded_pool().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(tags.controller_mode.as_ref().unwrap());
        let initial = sample_initial_state();
        let guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };
        let (mut ctrl_c, tx) = CtrlC::test_pair();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = tx.send(1);
        });

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &fast_simulator_args(),
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut ctrl_c,
            &mut None,
        )
        .await;

        assert!(matches!(
            outcome,
            RestoreAttempt::Incomplete { ref reason } if reason.contains("second Ctrl+C")
        ));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn restore_wrapper_handles_deadline_after_mv_restore_completes() {
        let pool = SqlitePool::connect_lazy("sqlite::memory:").unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let driver = honeywell_driver_auto().hanging_write(tags.controller_mode.as_ref().unwrap());
        let initial = sample_initial_state();
        let guard = MutationGuard {
            mode_written: true,
            ..MutationGuard::default()
        };
        let mut args = fast_simulator_args();
        args.restore_timeout_secs = 1;
        args.op_timeout_secs = 30;

        let outcome = attempt_restore_with_actuation(
            &pool,
            0,
            &args,
            &driver,
            &tags,
            &template,
            &initial,
            &guard,
            false,
            &mut CtrlC::never(),
            &mut None,
        )
        .await;

        assert!(matches!(
            outcome,
            RestoreAttempt::Incomplete { ref reason }
                if reason.contains("[tuning].restore_timeout_secs")
        ));
        assert_eq!(
            driver.value_of(&tags.manipulated_variable).as_deref(),
            Some("45")
        );
    }

    #[tokio::test]
    async fn restore_failure_wrapper_preserves_error_when_status_and_cleanup_writes_fail() {
        let pool = seeded_pool().await;
        pool.close().await;
        let original = anyhow::anyhow!("original polling failure");
        let error = restore_best_effort_then_propagate(
            &pool,
            42,
            &honeywell_driver_auto(),
            &honeywell_tags(),
            &honeywell_template(),
            &sample_initial_state(),
            &MutationGuard::default(),
            &fast_simulator_args(),
            false,
            &mut CtrlC::never(),
            &mut None,
            original,
        )
        .await;

        assert_eq!(error.to_string(), "original polling failure");
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
