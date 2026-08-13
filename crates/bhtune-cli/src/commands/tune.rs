//! `bhtune tune`/`bhtune simulate`: runs a full MRFT test end-to-end — resolving a template,
//! deriving tags, transitioning the loop to Manual, polling the backend and driving a real
//! [`bhtune_core::MrftEngine`], persisting every tick and the final calculated results, then
//! restoring the loop and optionally writing back the chosen PID constants.
//!
//! Mirrors the legacy `MRFTstart`/`ReadInitialOPCvalues`/`ChangeControllerModeToMan`/
//! `ResetOPC` sequence from `OPCClass.cs`. The mode-transition and write-back steps
//! automatically no-op for the simulator backend, since its [`bhtune_core::LoopTags`] has no
//! setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags` below) — no
//! separate "is this the simulator?" branching is needed in that logic.

use std::time::Duration;

use bhtune_backend::{Backend, TagWrite};
use bhtune_core::{
    Action, ControllerDirection, ControllerType, DcsTemplate, InitialReadings, LoopConfig,
    LoopTags, MrftCompat, MrftEngine, PidParameters, ProcessType, PvRange, ResponseLevel,
    TagOrValue, Tick, TuningMathCompat, calculate_all, lookup, opc_write_values,
};
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    DcsTemplateRow, TuneBackend, TuneResultRow, TuneRunInitialReadings, TuneRunRow, TuneSampleRow,
    TuneWriteRow, WriteReadback,
};
use chrono::{DateTime, Utc};

use crate::args::{BackendKindArg, TuneArgs};
use crate::backend::{SIMULATOR_MV_TAG, SIMULATOR_PV_TAG};
use crate::output::OutputFormat;

/// The final disposition of a `tune`/`simulate` run -- drives the printed summary (see
/// [`print_summary`]) and, via `crate::tune_outcome_exit_code` in `lib.rs`, the process's
/// exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuneOutcome {
    /// The test completed. Either no write-back was requested/possible, or a requested
    /// write-back succeeded.
    Completed,
    /// The user pressed Ctrl+C; the loop was restored to its original mode/setpoint before
    /// returning.
    Aborted,
    /// The test itself completed, but writing the chosen PID parameters back to the DCS
    /// failed (rejected write, failed confirmation readback, or -- defensively -- a
    /// `--write-pid` level with no matching calculated result).
    WriteBackFailed,
}

impl TuneOutcome {
    /// A short machine-readable label, used in the `--output json` summary.
    pub fn label(self) -> &'static str {
        match self {
            TuneOutcome::Completed => "completed",
            TuneOutcome::Aborted => "aborted",
            TuneOutcome::WriteBackFailed => "write_back_failed",
        }
    }
}

/// Runs one full tune. Never returns `Err` for a tune that simply didn't complete
/// successfully (a failed/aborted run is recorded in the database and reported to stdout);
/// `Err` is reserved for setup problems (unknown template, invalid flag combination,
/// database errors) surfaced directly to the caller.
///
/// Resolves `args.bridge_host` (always) and `args.server` (only for the `opcda` backend,
/// since the simulator backend has no OPC server concept at all) through `app_config`'s
/// `CLI > env > config file > default` precedence before anything else runs, so every
/// downstream consumer (`crate::backend::build`, mainly) can keep reading the plain
/// `TuneArgs` fields it always has.
pub async fn run(
    pool: &SqlitePool,
    mut args: TuneArgs,
    app_config: &crate::config::BhtuneConfig,
) -> anyhow::Result<TuneOutcome> {
    // Fails before any backend/database I/O at all: an unattended write-back must be an
    // explicit, deliberate choice, not something a stray `--write-pid` without `--yes` can
    // trigger by accident.
    if args.write_pid.is_some() && !args.yes {
        anyhow::bail!(
            "--write-pid requires --yes: writing PID constants back to the DCS with no \
             human present to confirm must be an explicit, deliberate choice"
        );
    }

    args.bridge_host = Some(crate::config::resolve_bridge_host(
        args.bridge_host.take(),
        app_config,
    ));
    if matches!(args.backend, BackendKindArg::Opcda) {
        args.server = Some(crate::config::resolve_server(
            args.server.take(),
            app_config,
        )?);
    }

    let template_row = DcsTemplateRow::get_by_name(pool, &args.template)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no template named '{}'", args.template))?;
    let template = template_row.template;

    let config = build_loop_config(&args)?;
    let tags = build_loop_tags(&args, &template)?;
    let backend = crate::backend::build(&args).await?;

    let run_name = args.name.clone().unwrap_or_else(|| args.tagname.clone());
    let db_backend = match args.backend {
        BackendKindArg::Opcda => TuneBackend::Opcda,
        BackendKindArg::Simulator => TuneBackend::Simulator,
    };
    let started_at = Utc::now();
    let run = TuneRunRow::start(pool, None, &run_name, db_backend, config, started_at).await?;

    let write_pid: Option<ResponseLevel> = args.write_pid.map(Into::into);

    let outcome = execute(
        pool,
        run.id,
        &args,
        &template,
        &tags,
        backend.as_ref(),
        config,
        started_at,
        write_pid,
    )
    .await;

    match outcome {
        Ok(run_outcome) => Ok(print_summary(run.id, &run_outcome, args.output)),
        Err(e) => {
            TuneRunRow::fail(pool, run.id, Utc::now(), &e.to_string())
                .await
                .ok();
            Err(e)
        }
    }
}

enum RunOutcome {
    Completed { write_back: WriteBackOutcome },
    Aborted,
}

/// The result of `maybe_write_back`'s attempt (or non-attempt) to write calculated PID
/// parameters back to the DCS. The write itself is always fully recorded in `tune_writes`
/// regardless of this value (a DB row exists for `Written`/`Failed`, never for `Skipped`
/// since nothing was attempted at the backend at all); this enum exists purely to drive the
/// printed summary and the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteBackOutcome {
    /// No write was attempted: no PID constant tags configured, no results recorded, or
    /// (interactive path only) the user chose to skip / gave invalid input.
    Skipped,
    /// The write succeeded and was confirmed by a readback.
    Written { response_level: ResponseLevel },
    /// A write was attempted (interactively selected, or requested via `--write-pid`) but
    /// failed -- the backend rejected it, the confirmation readback failed, or (defensively)
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
        } => TuneOutcome::WriteBackFailed,
        RunOutcome::Completed { .. } => TuneOutcome::Completed,
        RunOutcome::Aborted => TuneOutcome::Aborted,
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
            } => {
                println!(
                    "Tune completed successfully (run id {run_id}); wrote {response_level:?} PID parameters."
                );
            }
            RunOutcome::Completed {
                write_back: WriteBackOutcome::Skipped,
            } => {
                println!("Tune completed successfully (run id {run_id}).");
            }
            RunOutcome::Completed {
                write_back: WriteBackOutcome::Failed,
            } => {
                println!(
                    "Tune completed successfully (run id {run_id}), but PID write-back failed; the loop was left with its previous PID constants."
                );
            }
            RunOutcome::Aborted => {
                println!("Tune aborted (Ctrl+C received; loop restored).");
            }
        },
        OutputFormat::Json => {
            let (write_back, response_level) = match outcome {
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Written { response_level },
                } => ("written", Some(*response_level)),
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped,
                } => ("skipped", None),
                RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed,
                } => ("failed", None),
                RunOutcome::Aborted => ("not_attempted", None),
            };
            let json = serde_json::json!({
                "run_id": run_id,
                "outcome": tune_outcome.label(),
                "write_back": write_back,
                "write_back_response_level": response_level,
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

    Ok(LoopConfig {
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
    })
}

/// Builds the loop's full tag set. For `--backend opcda`, derives from `--tagname` and the
/// template, then layers any explicit `--pv-range-*`/`--mv-range-*`/`--direction` overrides
/// on top. For `--backend simulator`, `SimulatorBackend`'s fixed two-tag contract means the
/// range/direction overrides are mandatory (normally supplied by
/// `SimulateArgs::into_tune_args`); a direct `bhtune tune --backend simulator` invocation
/// missing any of them is a clear usage error rather than a confusing runtime failure.
fn build_loop_tags(args: &TuneArgs, template: &DcsTemplate) -> anyhow::Result<LoopTags> {
    match args.backend {
        BackendKindArg::Opcda => {
            let mut tags = LoopTags::derive_from_pv_tag(&args.tagname, template);
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
        BackendKindArg::Simulator => {
            let pv_range_high = args.pv_range_high.ok_or_else(|| {
                anyhow::anyhow!(
                    "--pv-range-high is required with --backend simulator (or use `bhtune simulate`)"
                )
            })?;
            let pv_range_low = args.pv_range_low.ok_or_else(|| {
                anyhow::anyhow!(
                    "--pv-range-low is required with --backend simulator (or use `bhtune simulate`)"
                )
            })?;
            let mv_range_high = args.mv_range_high.ok_or_else(|| {
                anyhow::anyhow!(
                    "--mv-range-high is required with --backend simulator (or use `bhtune simulate`)"
                )
            })?;
            let mv_range_low = args.mv_range_low.ok_or_else(|| {
                anyhow::anyhow!(
                    "--mv-range-low is required with --backend simulator (or use `bhtune simulate`)"
                )
            })?;
            let direction = args.direction.ok_or_else(|| {
                anyhow::anyhow!(
                    "--direction is required with --backend simulator (or use `bhtune simulate`)"
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

/// Everything read from the backend before any mode transition is attempted — mirrors
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
}

/// State captured during `transition_to_manual` that `restore` needs later — currently just
/// the setpoint read while transitioning out of Auto (`SvValueIni` in the legacy app).
#[derive(Default)]
struct ModeRestoreState {
    setpoint_ini: Option<f32>,
}

#[allow(clippy::too_many_arguments)]
async fn execute(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    template: &DcsTemplate,
    tags: &LoopTags,
    backend: &dyn Backend,
    config: LoopConfig,
    started_at: DateTime<Utc>,
    write_pid: Option<ResponseLevel>,
) -> anyhow::Result<RunOutcome> {
    let initial = read_initial_values(backend, tags, template).await?;
    let mode_state = transition_to_manual(backend, tags, template, &initial).await?;

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
        },
    )
    .await?;

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

    let poll_result =
        run_polling_loop(pool, run_id, args, tags, backend, &mut engine, started_at).await;

    match poll_result {
        Ok(Some(completion)) => {
            let pv_range = PvRange {
                high: initial.pv_range_high,
                low: initial.pv_range_low,
            };
            persist_results(
                pool,
                run_id,
                completion,
                initial.direction,
                config,
                pv_range,
                template,
            )
            .await?;
            TuneRunRow::complete(pool, run_id, Utc::now()).await?;
            restore(backend, tags, template, &initial, &mode_state).await?;
            let write_back = maybe_write_back(
                pool,
                run_id,
                tags,
                template,
                backend,
                config,
                write_pid,
                &mut std::io::stdin().lock(),
            )
            .await?;
            Ok(RunOutcome::Completed { write_back })
        }
        Ok(None) => {
            restore(backend, tags, template, &initial, &mode_state).await?;
            TuneRunRow::abort(pool, run_id, Utc::now()).await?;
            Ok(RunOutcome::Aborted)
        }
        Err(e) => {
            // Best-effort: a failed test still stroked the valve, so try to put it back even
            // though the overall run is going to be reported as failed regardless.
            let _ = restore(backend, tags, template, &initial, &mode_state).await;
            Err(e)
        }
    }
}

async fn read_raw(backend: &dyn Backend, tag: &str) -> anyhow::Result<String> {
    let values = backend.read(&[tag.to_string()]).await?;
    let value = values
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("backend returned no value for tag '{tag}'"))?;
    Ok(value.value)
}

async fn read_f32(backend: &dyn Backend, tag: &str) -> anyhow::Result<f32> {
    let raw = read_raw(backend, tag).await?;
    raw.trim()
        .parse::<f32>()
        .map_err(|_| anyhow::anyhow!("tag '{tag}' value '{raw}' is not a number"))
}

async fn resolve_f32(backend: &dyn Backend, tag_or_value: &TagOrValue<f32>) -> anyhow::Result<f32> {
    match tag_or_value {
        TagOrValue::Value(v) => Ok(*v),
        TagOrValue::Tag(tag) => read_f32(backend, tag).await,
    }
}

async fn resolve_direction(
    backend: &dyn Backend,
    tag_or_value: &TagOrValue<ControllerDirection>,
    template: &DcsTemplate,
) -> anyhow::Result<ControllerDirection> {
    match tag_or_value {
        TagOrValue::Value(d) => Ok(*d),
        TagOrValue::Tag(tag) => {
            let raw = read_raw(backend, tag).await?;
            Ok(ControllerDirection::from_raw_tag_value(
                &raw,
                &template.controller_action_direct_value,
            ))
        }
    }
}

async fn write_raw(backend: &dyn Backend, tag: &str, value: String) -> anyhow::Result<()> {
    let outcome = backend
        .write(&tag.to_string(), TagWrite::Raw(value))
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

async fn write_value(backend: &dyn Backend, tag: &str, value: f32) -> anyhow::Result<()> {
    let outcome = backend
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
    backend: &dyn Backend,
    tags: &LoopTags,
    template: &DcsTemplate,
) -> anyhow::Result<InitialState> {
    let pv_ini = read_f32(backend, &tags.process_variable).await?;
    let mv_ini = read_f32(backend, &tags.manipulated_variable).await?;

    let mode_raw = match &tags.controller_mode {
        Some(tag) => Some(read_raw(backend, tag).await?),
        None => None,
    };
    let mode_attribute_raw = match &tags.mode_attribute {
        Some(tag) => Some(read_raw(backend, tag).await?),
        None => None,
    };

    let direction = resolve_direction(backend, &tags.controller_direction, template).await?;
    let pv_range_high = resolve_f32(backend, &tags.upper_pv_range).await?;
    let pv_range_low = resolve_f32(backend, &tags.lower_pv_range).await?;
    let mv_range_high = resolve_f32(backend, &tags.upper_mv_range).await?;
    let mv_range_low = resolve_f32(backend, &tags.lower_mv_range).await?;

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
    })
}

/// Pure port of `ChangeControllerModeToMan`. No-ops entirely when `tags.controller_mode` and
/// `tags.mode_attribute` are both `None` (the simulator's case).
async fn transition_to_manual(
    backend: &dyn Backend,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
) -> anyhow::Result<ModeRestoreState> {
    if let (Some(attr_tag), Some(program_value)) =
        (&tags.mode_attribute, &template.mode_attribute_program_value)
    {
        write_raw(backend, attr_tag, program_value.clone()).await?;
        tokio::time::sleep(Duration::from_millis(1000)).await;
    }

    let mut setpoint_ini = None;
    if let Some(mode_tag) = &tags.controller_mode {
        let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
        if mode_raw != template.mode_manual_value {
            if mode_raw == template.mode_auto_value
                && let Some(sv_tag) = &tags.setpoint_variable
            {
                setpoint_ini = Some(read_f32(backend, sv_tag).await?);
            }
            write_raw(backend, mode_tag, template.mode_manual_value.clone()).await?;
        }
    }

    Ok(ModeRestoreState { setpoint_ini })
}

/// Pure port of `ResetOPC` (minus the dead Python-model branch, which is not being ported —
/// see AGENTS.md). Writing the MV back always happens; the mode/setpoint/mode-attribute
/// reverts each naturally no-op when their tag is `None`.
async fn restore(
    backend: &dyn Backend,
    tags: &LoopTags,
    template: &DcsTemplate,
    initial: &InitialState,
    mode_state: &ModeRestoreState,
) -> anyhow::Result<()> {
    write_value(backend, &tags.manipulated_variable, initial.mv_ini).await?;
    tokio::time::sleep(Duration::from_millis(1000)).await;

    if let Some(mode_tag) = &tags.controller_mode {
        let mode_raw = initial.mode_raw.as_deref().unwrap_or_default();
        if template.revert_mode && mode_raw != template.mode_manual_value {
            write_raw(backend, mode_tag, mode_raw.to_string()).await?;
            if mode_raw == template.mode_auto_value
                && let (Some(sv_tag), Some(sv_ini)) =
                    (&tags.setpoint_variable, mode_state.setpoint_ini)
            {
                tokio::time::sleep(Duration::from_millis(1000)).await;
                write_value(backend, sv_tag, sv_ini).await?;
            }
        }
    }

    if let Some(attr_tag) = &tags.mode_attribute {
        let attr_raw = initial.mode_attribute_raw.as_deref().unwrap_or_default();
        let program_value = template
            .mode_attribute_program_value
            .as_deref()
            .unwrap_or_default();
        if attr_raw != program_value {
            write_raw(backend, attr_tag, attr_raw.to_string()).await?;
        }
    }

    Ok(())
}

/// Polls the backend on `args.poll_interval_ms`, driving `engine` once the pre-test
/// `--mrft-delay` padding period has elapsed, and continuing to record (but not evaluate)
/// samples for the same padding period after completion. Returns `Ok(Some(completion))` on a
/// normal finish, `Ok(None)` if interrupted by Ctrl+C before completion.
async fn run_polling_loop(
    pool: &SqlitePool,
    run_id: i64,
    args: &TuneArgs,
    tags: &LoopTags,
    backend: &dyn Backend,
    engine: &mut MrftEngine,
    start_time: DateTime<Utc>,
) -> anyhow::Result<Option<Action>> {
    let mut interval = tokio::time::interval(Duration::from_millis(args.poll_interval_ms.max(1)));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let pre_delay_end = start_time + chrono::Duration::seconds(args.mrft_delay as i64);
    let mut tick_index: i64 = 0;
    let mut completion: Option<Action> = None;
    let mut post_delay_end: Option<DateTime<Utc>> = None;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let now = Utc::now();
                let pv = read_f32(backend, &tags.process_variable).await?;
                let tick = Tick { time: now, pv };

                if completion.is_none() && now < pre_delay_end {
                    TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state()).await?;
                    tick_index += 1;
                    continue;
                }

                for action in engine.step(tick) {
                    match action {
                        Action::WriteMv(v) => write_value(backend, &tags.manipulated_variable, v).await?,
                        Action::Complete { .. } => {
                            completion = Some(action);
                            post_delay_end =
                                Some(now + chrono::Duration::seconds(args.mrft_delay as i64));
                        }
                    }
                }

                TuneSampleRow::insert(pool, run_id, tick_index, tick, engine.state()).await?;
                tick_index += 1;

                if let Some(end) = post_delay_end
                    && now >= end
                {
                    break;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                return Ok(None);
            }
        }
    }

    Ok(completion)
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

/// Writes back the calculated PID parameters for one response level -- chosen either
/// interactively (prompting on `reader`) or non-interactively via `write_pid`
/// (`--write-pid`; the caller has already validated `--yes` was also given before the tune
/// even started). Skips with an informational message (rather than prompting/writing)
/// whenever any of the three PID constant tags is unconfigured — true for the simulator
/// backend, and also a sane guard for any real template missing one — or when no results
/// were recorded at all. `reader` is injected (rather than reading `std::io::stdin()`
/// directly) so tests can supply a fixed `Cursor` in place of the process's real stdin; it
/// is never read from at all when `write_pid` is `Some`.
#[allow(clippy::too_many_arguments)]
async fn maybe_write_back(
    pool: &SqlitePool,
    run_id: i64,
    tags: &LoopTags,
    template: &DcsTemplate,
    backend: &dyn Backend,
    config: LoopConfig,
    write_pid: Option<ResponseLevel>,
    reader: &mut impl std::io::BufRead,
) -> anyhow::Result<WriteBackOutcome> {
    let (Some(p_tag), Some(i_tag), Some(d_tag)) = (
        &tags.proportional_constant,
        &tags.integral_constant,
        &tags.derivative_constant,
    ) else {
        println!(
            "No PID constant tags configured for this run's backend/template; skipping write-back."
        );
        return Ok(WriteBackOutcome::Skipped);
    };

    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    if results.is_empty() {
        return Ok(WriteBackOutcome::Skipped);
    }

    let selected = match write_pid {
        Some(level) => match results.iter().find(|r| r.response_level == level) {
            Some(r) => {
                println!(
                    "Non-interactively writing {level:?} PID parameters back to the DCS (--write-pid)."
                );
                r
            }
            None => {
                println!(
                    "No calculated result recorded for response level {level:?}; skipping write-back."
                );
                return Ok(WriteBackOutcome::Failed);
            }
        },
        None => {
            println!("\nCalculated PID parameters:");
            for (i, r) in results.iter().enumerate() {
                println!(
                    "  {}. {:?}: P={:.4} I={:.4} D={:.4}",
                    i + 1,
                    r.response_level,
                    r.proportional,
                    r.integral,
                    r.derivative
                );
            }
            println!(
                "Write which response level's PID parameters back to the DCS? [1-{}, or Enter/n to skip]:",
                results.len()
            );

            let mut input = String::new();
            let bytes_read = reader.read_line(&mut input).unwrap_or(0);
            let input = input.trim();
            if bytes_read == 0 || input.is_empty() || input.eq_ignore_ascii_case("n") {
                println!("Skipping PID write-back.");
                return Ok(WriteBackOutcome::Skipped);
            }

            match input.parse::<usize>() {
                Ok(n) if n >= 1 && n <= results.len() => &results[n - 1],
                _ => {
                    println!("Invalid selection; skipping PID write-back.");
                    return Ok(WriteBackOutcome::Skipped);
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
    let written_at = Utc::now();

    let p_outcome = backend
        .write(&p_tag.clone(), TagWrite::Float(written.proportional))
        .await?;
    let i_outcome = backend
        .write(&i_tag.clone(), TagWrite::Float(written.integral))
        .await?;
    let d_outcome = backend
        .write(&d_tag.clone(), TagWrite::Float(written.derivative))
        .await?;

    if p_outcome.success && i_outcome.success && d_outcome.success {
        match backend
            .read(&[p_tag.clone(), i_tag.clone(), d_tag.clone()])
            .await
        {
            Ok(values) if values.len() == 3 => {
                let readback = WriteReadback {
                    proportional: values[0].value.trim().parse().unwrap_or(f32::NAN),
                    integral: values[1].value.trim().parse().unwrap_or(f32::NAN),
                    derivative: values[2].value.trim().parse().unwrap_or(f32::NAN),
                };
                TuneWriteRow::insert_success(pool, run_id, written, readback, written_at).await?;
                println!("Wrote and confirmed {response_level:?} PID parameters.");
                Ok(WriteBackOutcome::Written { response_level })
            }
            _ => {
                TuneWriteRow::insert_failure(
                    pool,
                    run_id,
                    written,
                    written_at,
                    "readback after write failed",
                )
                .await?;
                println!("Wrote PID parameters, but the confirmation readback failed.");
                Ok(WriteBackOutcome::Failed)
            }
        }
    } else {
        let error_message = [&p_outcome, &i_outcome, &d_outcome]
            .iter()
            .filter_map(|o| {
                if o.success {
                    None
                } else {
                    Some(
                        o.error_message
                            .clone()
                            .unwrap_or_else(|| "unknown reason".to_string()),
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("; ");
        TuneWriteRow::insert_failure(pool, run_id, written, written_at, &error_message).await?;
        println!("PID write-back failed: {error_message}");
        Ok(WriteBackOutcome::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{ControllerTypeArg, DirectionArg, ProcessTypeArg};

    async fn seeded_pool() -> SqlitePool {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, Utc::now())
            .await
            .unwrap();
        pool
    }

    /// `run()`'s tests all pass explicit `TuneArgs.bridge_host`/`server` values (or the
    /// simulator backend, which ignores both), so an all-default `BhtuneConfig` never
    /// actually supplies anything here -- it's only present because `run()`'s signature
    /// requires it.
    fn test_config() -> crate::config::BhtuneConfig {
        crate::config::BhtuneConfig::default()
    }

    /// A fast-converging simulator tune: proportionally scaled down from
    /// `bhtune-backend`'s own proven `FopdtConfig::new(1.0, 2.0, 5.0, 1.0)` E2E fixture (2
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
            backend: BackendKindArg::Simulator,
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
            poll_interval_ms: 5,
            name: Some("test-loop".to_string()),
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
        assert_eq!(runs[0].loop_name, "test-loop");
        assert!(runs[0].initial_readings.is_some());

        let results = TuneResultRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert_eq!(results.len(), 3);

        let samples = TuneSampleRow::list_for_run(&pool, runs[0].id)
            .await
            .unwrap();
        assert!(!samples.is_empty());

        // The simulator backend has no PID constant tags, so write-back must have been
        // skipped entirely rather than hanging on stdin.
        let writes = TuneWriteRow::list_for_run(&pool, runs[0].id).await.unwrap();
        assert!(writes.is_empty());
    }

    /// Every range/direction override is CLI-supplied below, so `read_initial_values` never
    /// reads them from the backend; the mock only ever needs to answer for `pv_ini`/`mv_ini`
    /// and (for the Yokogawa template) has no mode/mode-attribute tags to read either. Fails
    /// starting at the 5th `read` RPC call — comfortably past every possible setup read —
    /// so the failure always lands on the first polling tick's PV read, deep inside
    /// `run_polling_loop`, not during setup.
    #[tokio::test]
    async fn run_with_opcda_backend_fails_mid_poll_and_marks_the_run_failed() {
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
        args.backend = BackendKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = Some(host);
        args.server = Some("MockServer".to_string());

        let err = run(&pool, args, &test_config()).await.unwrap_err();
        assert!(err.to_string().contains("backend operation failed"));

        let runs = TuneRunRow::list(
            &pool,
            &bhtune_db::models::TuneRunFilter::default(),
            bhtune_db::models::Pagination::first(10),
        )
        .await
        .unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].outcome, bhtune_db::models::TuneOutcome::Failed);
        assert_eq!(runs[0].backend, bhtune_db::models::TuneBackend::Opcda);
        assert!(
            runs[0]
                .failure_reason
                .as_deref()
                .unwrap()
                .contains("backend operation failed")
        );

        server.shutdown().await;
    }

    /// Proves `run()` actually resolves `bridge_host`/`server` from `app_config` (not just
    /// from `TuneArgs`) by leaving both CLI-facing fields unset and supplying them only via
    /// the config -- if resolution didn't happen, `backend::build` would either fail fast
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
        args.backend = BackendKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.bridge_host = None;
        args.server = None;

        let app_config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            server: Some("MockServer".to_string()),
            ..Default::default()
        };

        // A "backend operation failed" error (rather than "no OPC server specified" or a
        // connection error against the unresolved default host) proves setup got as far as
        // issuing a real RPC against the config-resolved mock server.
        let err = run(&pool, args, &app_config).await.unwrap_err();
        assert!(err.to_string().contains("backend operation failed"));

        server.shutdown().await;
    }

    #[tokio::test]
    async fn run_errors_when_opcda_server_is_unset_in_both_cli_and_config() {
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.backend = BackendKindArg::Opcda;
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
        args.backend = BackendKindArg::Opcda;
        args.tagname = "Unit1.LIC101.PV".to_string();
        args.direction = Some(DirectionArg::Direct);
        let tags = build_loop_tags(&args, &template).unwrap();
        assert!(tags.process_variable.starts_with("Unit1.LIC101"));
        assert_eq!(
            tags.controller_direction,
            TagOrValue::Value(ControllerDirection::Direct)
        );
        assert_eq!(tags.upper_pv_range, TagOrValue::Value(100.0));
    }

    #[tokio::test]
    async fn a_ctrl_c_style_abort_restores_and_records_aborted() {
        // Exercise the abort path directly (rather than actually raising SIGINT in a test
        // process) by calling `run_polling_loop` with an engine that will never see enough
        // ticks to complete within a near-zero timeout, then checking the run was left in a
        // consistent, restorable state. Since we cannot easily fake a real ctrl_c signal in
        // a unit test, this test instead confirms `restore` + `TuneRunRow::abort` leave the
        // database in the expected shape when called directly, which is the code path Ctrl+C
        // takes.
        let pool = seeded_pool().await;
        let template = bhtune_core::built_in_templates().remove(0);
        let args = fast_simulator_args();
        let config = build_loop_config(&args).unwrap();
        let tags = build_loop_tags(&args, &template).unwrap();
        let backend = crate::backend::build(&args).await.unwrap();

        let started_at = Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "abort-test",
            TuneBackend::Simulator,
            config,
            started_at,
        )
        .await
        .unwrap();

        let initial = read_initial_values(backend.as_ref(), &tags, &template)
            .await
            .unwrap();
        let mode_state = transition_to_manual(backend.as_ref(), &tags, &template, &initial)
            .await
            .unwrap();
        restore(backend.as_ref(), &tags, &template, &initial, &mode_state)
            .await
            .unwrap();
        TuneRunRow::abort(&pool, run.id, Utc::now()).await.unwrap();

        let stored = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(stored.outcome, bhtune_db::models::TuneOutcome::Aborted);
    }

    // --- opcda-style mode-transition/restore/write-back coverage ---------------------------
    //
    // The tests above all use the simulator backend, whose `LoopTags` has no
    // setpoint/mode/mode-attribute/PID-constant tags at all (see `build_loop_tags`), so they
    // never exercise `transition_to_manual`/`restore`/`maybe_write_back`'s real opcda-style
    // logic. `MockBackend` below is a minimal in-memory `Backend` double with a configurable
    // tag/value map, used together with the real "Honeywell Experion" built-in template
    // (which has every optional tag suffix configured) to drive that logic directly.

    /// A minimal, fully in-memory [`Backend`] test double with a fixed tag-value map, plus
    /// the ability to inject specific-tag read/write failures. `std::sync::Mutex`, not
    /// `tokio::sync::Mutex` — matching `SimulatorBackend`'s own precedent — since no
    /// `.await` point is ever held across the lock.
    #[derive(Default)]
    struct MockBackend {
        values: std::sync::Mutex<std::collections::HashMap<String, String>>,
        writes: std::sync::Mutex<Vec<(String, String)>>,
        reject_writes: std::collections::HashSet<String>,
        error_reads: std::collections::HashSet<String>,
        error_writes: std::collections::HashSet<String>,
        empty_reads: std::collections::HashSet<String>,
    }

    impl MockBackend {
        fn new(values: &[(&str, &str)]) -> MockBackend {
            MockBackend {
                values: std::sync::Mutex::new(
                    values
                        .iter()
                        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                        .collect(),
                ),
                ..Default::default()
            }
        }

        fn rejecting_write(mut self, tag: &str) -> MockBackend {
            self.reject_writes.insert(tag.to_string());
            self
        }

        fn erroring_read(mut self, tag: &str) -> MockBackend {
            self.error_reads.insert(tag.to_string());
            self
        }

        fn erroring_write(mut self, tag: &str) -> MockBackend {
            self.error_writes.insert(tag.to_string());
            self
        }

        fn empty_read(mut self, tag: &str) -> MockBackend {
            self.empty_reads.insert(tag.to_string());
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
    impl Backend for MockBackend {
        async fn read(
            &self,
            tags: &[String],
        ) -> bhtune_backend::BackendResult<Vec<bhtune_backend::TagValue>> {
            let store = self.values.lock().unwrap();
            let mut out = Vec::new();
            for tag in tags {
                if self.error_reads.contains(tag) {
                    return Err(bhtune_backend::BackendError::Operation(Box::new(
                        std::io::Error::other("mock read error"),
                    )));
                }
                if self.empty_reads.contains(tag) {
                    continue;
                }
                out.push(bhtune_backend::TagValue {
                    tag: tag.clone(),
                    value: store.get(tag).cloned().unwrap_or_default(),
                    quality: bhtune_backend::Quality::Good,
                    timestamp: None,
                });
            }
            Ok(out)
        }

        async fn write(
            &self,
            tag: &String,
            value: TagWrite,
        ) -> bhtune_backend::BackendResult<bhtune_backend::WriteOutcome> {
            if self.error_writes.contains(tag) {
                return Err(bhtune_backend::BackendError::Operation(Box::new(
                    std::io::Error::other("mock write error"),
                )));
            }
            let text = match value {
                TagWrite::Float(f) => f.to_string(),
                TagWrite::Raw(s) => s,
            };
            self.writes
                .lock()
                .unwrap()
                .push((tag.clone(), text.clone()));
            if self.reject_writes.contains(tag) {
                return Ok(bhtune_backend::WriteOutcome::failure("mock rejected write"));
            }
            self.values.lock().unwrap().insert(tag.clone(), text);
            Ok(bhtune_backend::WriteOutcome::success())
        }

        async fn browse(
            &self,
            _path: &str,
        ) -> bhtune_backend::BackendResult<Vec<bhtune_backend::TagNode>> {
            Err(bhtune_backend::BackendError::Unsupported {
                operation: "browse",
            })
        }
    }

    #[tokio::test]
    async fn mock_backend_browse_is_unsupported() {
        // `tune`'s own logic never calls `Backend::browse` -- this only exists so
        // `MockBackend` satisfies the trait -- but it should still honor the same
        // "unsupported, not a panic" convention real backends document for it.
        let err = MockBackend::new(&[]).browse("").await.unwrap_err();
        assert!(matches!(
            err,
            bhtune_backend::BackendError::Unsupported {
                operation: "browse"
            }
        ));
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

    /// A `MockBackend` pre-populated with every tag `honeywell_tags()` derives, using values
    /// that make the loop initially Auto (`MODE=1`) with its Mode Attribute not yet at the
    /// Program value (`MODEATTR=1`, program value is `"2"`) — the common starting point most
    /// of the tests below share before diverging.
    fn honeywell_backend_auto() -> MockBackend {
        MockBackend::new(&[
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
        let backend = honeywell_backend_auto();

        let initial = read_initial_values(&backend, &tags, &template)
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
        let backend = honeywell_backend_auto().empty_read("Unit1.LIC101.PV");

        let err = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no value"));
    }

    #[tokio::test]
    async fn read_f32_errors_on_a_non_numeric_value() {
        let backend = MockBackend::new(&[("Unit1.LIC101.PV", "not-a-number")]);
        let err = read_f32(&backend, "Unit1.LIC101.PV").await.unwrap_err();
        assert!(err.to_string().contains("not a number"));
    }

    #[tokio::test]
    async fn read_raw_and_write_raw_propagate_a_hard_backend_error() {
        // Distinct from a *rejected* write (`WriteOutcome::success == false`, handled by
        // `write_raw`/`write_value`'s own "was rejected" message): this is the backend call
        // itself failing (`BackendError::Operation`), which `?` should propagate as-is.
        let backend = MockBackend::new(&[("Unit1.LIC101.PV", "50.0")])
            .erroring_read("Unit1.LIC101.PV")
            .erroring_write("Unit1.LIC101.OP");

        let read_err = read_raw(&backend, "Unit1.LIC101.PV").await.unwrap_err();
        assert!(read_err.to_string().contains("backend operation failed"));

        let write_err = write_value(&backend, "Unit1.LIC101.OP", 45.0)
            .await
            .unwrap_err();
        assert!(write_err.to_string().contains("backend operation failed"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_writes_program_value_and_captures_setpoint_from_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();

        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();

        assert_eq!(mode_state.setpoint_ini, Some(55.0));
        assert_eq!(
            backend.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("2")
        );
        assert_eq!(backend.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
        // Order matters (mode attribute unlocked before the mode itself is switched), per
        // `ChangeControllerModeToMan`.
        let log = backend.write_log();
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
    async fn transition_to_manual_skips_setpoint_capture_when_original_mode_is_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        // "2" is neither the manual ("0") nor auto ("1") raw value — e.g. Cascade.
        let backend = MockBackend::new(&[
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
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();

        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();

        assert_eq!(mode_state.setpoint_ini, None);
        assert_eq!(backend.value_of("Unit1.LIC101.MODE").as_deref(), Some("0"));
    }

    #[tokio::test(start_paused = true)]
    async fn transition_to_manual_does_not_rewrite_mode_when_already_manual() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = MockBackend::new(&[
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
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();

        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();

        assert_eq!(mode_state.setpoint_ini, None);
        // The Mode Attribute write always fires unconditionally (there's no "already at the
        // program value" guard on it), but Mode itself is already Manual, so its own
        // conditional `write_raw` must not fire a second time.
        let log = backend.write_log();
        assert_eq!(
            log,
            vec![("Unit1.LIC101.MODEATTR".to_string(), "2".to_string())]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn restore_reverts_mode_setpoint_and_mode_attribute() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();
        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();

        restore(&backend, &tags, &template, &initial, &mode_state)
            .await
            .unwrap();

        assert_eq!(backend.value_of("Unit1.LIC101.OP").as_deref(), Some("45")); // mv_ini
        assert_eq!(backend.value_of("Unit1.LIC101.MODE").as_deref(), Some("1")); // original raw
        assert_eq!(backend.value_of("Unit1.LIC101.SP").as_deref(), Some("55")); // setpoint restored
        assert_eq!(
            backend.value_of("Unit1.LIC101.MODEATTR").as_deref(),
            Some("1")
        ); // reverted off the Program value
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_revert_when_template_disables_it() {
        let mut template = honeywell_template();
        template.revert_mode = false;
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();
        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();
        let writes_before_restore = backend.write_log().len();

        restore(&backend, &tags, &template, &initial, &mode_state)
            .await
            .unwrap();

        // MV is always written back regardless of `revert_mode`; Mode/Setpoint are not.
        assert_eq!(backend.value_of("Unit1.LIC101.OP").as_deref(), Some("45"));
        assert_eq!(backend.value_of("Unit1.LIC101.MODE").as_deref(), Some("0")); // untouched
        let new_writes = &backend.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODE"));
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_setpoint_revert_when_original_mode_was_not_auto() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = MockBackend::new(&[
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
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();
        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();
        let writes_before_restore = backend.write_log().len();

        restore(&backend, &tags, &template, &initial, &mode_state)
            .await
            .unwrap();

        assert_eq!(backend.value_of("Unit1.LIC101.MODE").as_deref(), Some("2")); // reverted
        let new_writes = &backend.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.SP"));
    }

    #[tokio::test(start_paused = true)]
    async fn restore_skips_mode_attribute_revert_when_already_at_program_value() {
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = MockBackend::new(&[
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
        let initial = read_initial_values(&backend, &tags, &template)
            .await
            .unwrap();
        let mode_state = transition_to_manual(&backend, &tags, &template, &initial)
            .await
            .unwrap();
        let writes_before_restore = backend.write_log().len();

        restore(&backend, &tags, &template, &initial, &mode_state)
            .await
            .unwrap();

        let new_writes = &backend.write_log()[writes_before_restore..];
        assert!(new_writes.iter().all(|(t, _)| t != "Unit1.LIC101.MODEATTR"));
    }

    #[tokio::test(start_paused = true)]
    async fn write_raw_and_write_value_error_when_the_backend_rejects_the_write() {
        let backend = MockBackend::new(&[("Unit1.LIC101.MODE", "1")])
            .rejecting_write("Unit1.LIC101.MODE")
            .rejecting_write("Unit1.LIC101.OP");

        let raw_err = write_raw(&backend, "Unit1.LIC101.MODE", "0".to_string())
            .await
            .unwrap_err();
        assert!(raw_err.to_string().contains("rejected"));

        let value_err = write_value(&backend, "Unit1.LIC101.OP", 45.0)
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
            TuneBackend::Opcda,
            config,
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
        let backend = honeywell_backend_auto();

        let outcome = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &backend,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
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
        let run = TuneRunRow::start(
            &pool,
            None,
            "no-results",
            TuneBackend::Opcda,
            config,
            Utc::now(),
        )
        .await
        .unwrap();
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();

        let outcome = maybe_write_back(
            &pool,
            run.id,
            &tags,
            &template,
            &backend,
            config,
            None,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Skipped);
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
        let backend = honeywell_backend_auto();

        let outcome = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &backend,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
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
    }

    #[tokio::test]
    async fn maybe_write_back_records_failure_when_a_write_is_rejected() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto().rejecting_write("Unit1.LIC101.K");

        let outcome = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &backend,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
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
        // The 3 PID-constant writes themselves succeed, but the confirmation re-read of the
        // P tag then errors (`erroring_read` doesn't affect `write`, only `read`).
        let backend = honeywell_backend_auto().erroring_read("Unit1.LIC101.K");

        let outcome = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &backend,
            build_loop_config(&fast_simulator_args()).unwrap(),
            None,
            &mut std::io::Cursor::new(b"1\n".as_slice()),
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);
        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();

        assert_eq!(writes.len(), 1);
        assert!(!writes[0].success);
        assert_eq!(
            writes[0].error_message.as_deref(),
            Some("readback after write failed")
        );
    }

    #[tokio::test]
    async fn maybe_write_back_writes_non_interactively_via_write_pid_without_touching_stdin() {
        let (pool, run_id) = run_with_recorded_results().await;
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();

        // An empty reader would make the *interactive* path treat this as EOF-and-skip; a
        // `write_pid` request must never even try to read it.
        let outcome = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &backend,
            build_loop_config(&fast_simulator_args()).unwrap(),
            Some(ResponseLevel::Aggressive),
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
        let run = TuneRunRow::start(
            &pool,
            None,
            "partial-results",
            TuneBackend::Opcda,
            config,
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
        let template = honeywell_template();
        let tags = honeywell_tags();
        let backend = honeywell_backend_auto();

        let outcome = maybe_write_back(
            &pool,
            run.id,
            &tags,
            &template,
            &backend,
            config,
            Some(ResponseLevel::Sluggish),
            &mut std::io::Cursor::new(b"".as_slice()),
        )
        .await
        .unwrap();

        assert_eq!(outcome, WriteBackOutcome::Failed);
        // Nothing was attempted at the backend at all, so no audit row exists either.
        assert!(
            TuneWriteRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn tune_outcome_for_run_maps_every_run_outcome_variant() {
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Skipped
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Written {
                    response_level: ResponseLevel::Moderate
                }
            }),
            TuneOutcome::Completed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Completed {
                write_back: WriteBackOutcome::Failed
            }),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            tune_outcome_for_run(&RunOutcome::Aborted),
            TuneOutcome::Aborted
        );
    }

    #[test]
    fn print_summary_returns_the_tune_outcome_matching_the_run_outcome_in_every_output_format() {
        // Full 4 (RunOutcome shape) x 2 (OutputFormat) matrix, so every `println!` arm in
        // both `match output` branches is exercised directly here rather than relying on
        // incidental coverage from `run()`-level tests (which never reach `Written`/`Failed`
        // write-back outcomes -- see the module doc comment on why that's structurally hard
        // to drive end-to-end).
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped
                },
                OutputFormat::Table
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Skipped
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
                    }
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
                    }
                },
                OutputFormat::Json
            ),
            TuneOutcome::Completed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed
                },
                OutputFormat::Table
            ),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            print_summary(
                1,
                &RunOutcome::Completed {
                    write_back: WriteBackOutcome::Failed
                },
                OutputFormat::Json
            ),
            TuneOutcome::WriteBackFailed
        );
        assert_eq!(
            print_summary(1, &RunOutcome::Aborted, OutputFormat::Table),
            TuneOutcome::Aborted
        );
        assert_eq!(
            print_summary(1, &RunOutcome::Aborted, OutputFormat::Json),
            TuneOutcome::Aborted
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

        // The check happens before any backend/database I/O, so no run row should exist.
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
        // The built-in simulator backend has no PID constant tags at all (see
        // `build_loop_tags`), so `--write-pid`/`--yes` must be accepted but remain a no-op
        // -- not an error, and not `TuneOutcome::WriteBackFailed`.
        let pool = seeded_pool().await;
        let mut args = fast_simulator_args();
        args.write_pid = Some(crate::args::ResponseLevelArg::Aggressive);
        args.yes = true;

        let outcome = run(&pool, args, &test_config()).await.unwrap();
        assert_eq!(outcome, TuneOutcome::Completed);
    }
}
