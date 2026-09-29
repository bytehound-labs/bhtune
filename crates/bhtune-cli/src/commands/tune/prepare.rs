#![allow(rustdoc::broken_intra_doc_links)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use bhtune_core::{
    Action, ControllerDirection, ControllerType, DcsTemplate, InitialReadings, LoopConfig,
    LoopTags, MrftCompat, MrftEngine, MvRange, ProcessType, PvRange, ResponseLevel, TagOrValue,
    TagOverrides, lookup,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    DcsTemplateRow, TimingBasis, TimingMetrics, TuneDriver, TuneRunInitialReadings, TuneRunRow,
};
use bhtune_driver::{Driver, TagValue};
use chrono::Utc;

use crate::args::{DriverKindArg, TuneArgs};
use crate::cancel::CtrlC;
use crate::timing::{PollTimingAccumulator, RunTimeAnchor};

use super::actuation::*;
use super::config::*;
use super::output::*;
use super::poll::*;
use super::quality::*;
use super::restore::*;
use super::timing::*;
use super::writeback::*;

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
    pub(super) run_id: i64,
    pub(super) args: TuneArgs,
    pub(super) template: DcsTemplate,
    pub(super) tags: LoopTags,
    pub(super) driver: Box<dyn Driver>,
    pub(super) config: LoopConfig,
    pub(super) timing: EffectiveTiming,
    pub(super) time_anchor: RunTimeAnchor,
    pub(super) write_pid: Option<ResponseLevel>,
    pub(super) allow_uncertain_quality: bool,
}
/// Prepare a simulator tune and bind its history to a demo session before the background
/// execution is started. Live OPC DA preparation is intentionally rejected by this helper.
pub async fn prepare_owned(
    pool: &SqlitePool,
    args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
    demo_session_id: i64,
) -> anyhow::Result<PreparedTune> {
    if args.driver != DriverKindArg::Simulator {
        anyhow::bail!("demo sessions may only start simulator runs");
    }
    prepare_internal(pool, args, app_config, Some(demo_session_id)).await
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
pub(super) struct RequestSnapshot<'a> {
    pub(super) tagname: &'a str,
    pub(super) template: &'a str,
    pub(super) process_type: ProcessType,
    pub(super) controller_type: ControllerType,
    pub(super) relay_amp: f32,
    pub(super) cycles_skip: Option<u32>,
    pub(super) cycles_count: Option<u32>,
    pub(super) noise_protection_secs: Option<u32>,
    pub(super) driver: TuneDriver,
    pub(super) bridge_host: Option<&'a str>,
    pub(super) server: Option<&'a str>,
    pub(super) sim_gain: f32,
    pub(super) sim_tau: f32,
    pub(super) sim_dead_time: f32,
    pub(super) sim_noise: f32,
    pub(super) sim_seed: u64,
    pub(super) sim_initial_pv: f32,
    pub(super) sim_initial_mv: f32,
    pub(super) pv_range_high: Option<f32>,
    pub(super) pv_range_low: Option<f32>,
    pub(super) mv_range_high: Option<f32>,
    pub(super) mv_range_low: Option<f32>,
    pub(super) direction: Option<ControllerDirection>,
    pub(super) tag_overrides: Option<&'a TagOverrides>,
    pub(super) notes: Option<&'a str>,
    pub(super) yes: bool,
    pub(super) write_pid: Option<ResponseLevel>,
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
    args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
) -> anyhow::Result<PreparedTune> {
    prepare_internal(pool, args, app_config, None).await
}
pub(super) async fn prepare_internal(
    pool: &SqlitePool,
    mut args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
    demo_session_id: Option<i64>,
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
    #[allow(
        clippy::expect_used,
        reason = "RequestSnapshot is plain enums and finite scalars, serialized before any driver I/O"
    )]
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
    let gateway_compatibility = if let (DriverKindArg::Opcda, Some(bridge_host), Some(server)) = (
        args.driver,
        args.bridge_host.as_deref(),
        args.server.as_deref(),
    ) {
        Some(crate::gateway::require_live_gateway_compatible(bridge_host, Some(server)).await?)
    } else {
        None
    };

    let time_anchor = RunTimeAnchor::now();
    let started_at = time_anchor.utc();
    let run = TuneRunRow::start_with_demo_session(
        pool,
        demo_session_id,
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
    let metadata_result = async {
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
        if let Some(report) = gateway_compatibility.as_ref() {
            TuneRunRow::record_gateway_compatibility(
                pool,
                run.id,
                &crate::gateway::compatibility_json(report),
            )
            .await?;
        }
        let notes = args
            .notes
            .as_deref()
            .map(str::trim)
            .filter(|notes| !notes.is_empty());
        TuneRunRow::update_notes(pool, run.id, notes).await?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = metadata_result {
        finalize_preparation_failure(pool, run.id, &error.to_string()).await;
        return Err(error);
    }

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
pub(super) async fn finalize_preparation_failure(pool: &SqlitePool, run_id: i64, reason: &str) {
    if let Err(error) = TuneRunRow::fail(pool, run_id, Utc::now(), reason).await {
        tracing::error!(
            run_id,
            error = %error,
            "could not mark a failed preparation run terminal; attempting to delete it"
        );
        if let Err(delete_error) = TuneRunRow::delete(pool, run_id).await {
            tracing::error!(
                run_id,
                error = %delete_error,
                "could not remove a failed preparation run"
            );
        }
    }
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
pub(super) enum RunOutcome {
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
pub(super) enum AbortReason {
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
    /// the global Config > OPC quality policy set (finding 5 of the live-plant safety
    /// review). Unlike
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
pub(super) enum WriteBackOutcome {
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
/// Everything read from the driver before any mode transition is attempted — mirrors
/// `ReadInitialOPCvalues`.
#[derive(Debug)]
pub(super) struct InitialState {
    pub(super) pv_ini: f32,
    pub(super) mv_ini: f32,
    pub(super) pv_range_high: f32,
    pub(super) pv_range_low: f32,
    pub(super) mv_range_high: f32,
    pub(super) mv_range_low: f32,
    pub(super) direction: ControllerDirection,
    pub(super) mode_raw: Option<String>,
    pub(super) mode_attribute_raw: Option<String>,
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
    pub(super) setpoint_ini: Option<f32>,
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
pub(super) struct MutationGuard {
    /// The mode-attribute tag's "put in program/computer mode" write was attempted.
    pub(super) mode_attribute_written: bool,
    /// The mode tag's "put in Manual" write was attempted.
    pub(super) mode_written: bool,
    /// A relay-test MV write was attempted at least once during `run_polling_loop`. Tracked
    /// for completeness/audit parity with the other two flags -- `restore`'s MV-revert step
    /// is unconditional and never gated by this flag, since a redundant write-back is always
    /// harmless (idempotent if the pre-test value is already there, or safely rejected by
    /// the DCS if the loop never actually left Auto), so there is no correctness reason to
    /// skip attempting it even while this is still `false`.
    pub(super) mv_written: bool,
}
/// Persists the three calculated response levels after a completed MRFT test. The run's
/// terminal outcome is deliberately recorded later, after restoration, together with its
/// timing snapshot; this keeps SSE-triggered readers from seeing a terminal row before those
/// diagnostics are visible.
pub(super) async fn persist_completed_results(
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
pub(super) async fn execute_with_timing<R: std::io::BufRead>(
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
pub(super) async fn execute<R: std::io::BufRead>(
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
pub(super) async fn attempt_and_record_restore(
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
    attempt_and_record_restore_with_settling(
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
        None,
        None,
        None,
    )
    .await
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn attempt_and_record_restore_with_settling(
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
    completion: Option<&mut CompletedPoll>,
    measured_oscillation_period_ms: Option<f64>,
    timing: Option<&mut PollTimingAccumulator>,
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
        completion,
        measured_oscillation_period_ms,
        timing,
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
pub(super) async fn finish_completed_run<R: std::io::BufRead>(
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
    mut completion: CompletedPoll,
    timing: &mut PollTimingAccumulator,
    measured_oscillation_period_ms: Option<f64>,
) -> anyhow::Result<RunOutcome> {
    let pv_range = PvRange {
        high: initial.pv_range_high,
        low: initial.pv_range_low,
    };
    if let Err(error) = persist_completed_results(
        pool,
        run_id,
        completion.action.clone(),
        initial.direction,
        config,
        pv_range,
        template,
    )
    .await
    {
        let restore_attempt = attempt_and_record_restore_with_settling(
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
            Some(&mut completion),
            measured_oscillation_period_ms,
            Some(timing),
        )
        .await;
        let error = match restore_attempt {
            RestoreAttempt::Confirmed => error,
            RestoreAttempt::Incomplete { reason } => {
                tracing::warn!(
                    run_id,
                    reason = %reason,
                    "completed MRFT result persistence failed and restore was incomplete"
                );
                error
            }
        };
        record_timing_metrics_if_present(pool, run_id, timing.finish(None)).await;
        return Err(error);
    }

    let restore_attempt = attempt_and_record_restore_with_settling(
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
        Some(&mut completion),
        measured_oscillation_period_ms,
        Some(timing),
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
pub(super) async fn finish_aborted_run(
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
pub(super) async fn finish_failed_run(
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
/// Pure port of `ReadInitialOPCvalues`: everything read before any mode transition.
pub(super) async fn read_initial_values(
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
pub(super) fn validate_initial_state(initial: &InitialState) -> anyhow::Result<()> {
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
pub(super) async fn transition_to_manual(
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
