//! `bhtune history list/show/revert`.

use bhtune_db::SqlitePool;
use bhtune_db::models::{
    NewTuneWrite, Pagination, TuneDriver, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow,
    TuneWriteRow, WriteKind,
};
use bhtune_driver::OpcDaDriver;

use crate::args::HistoryCommand;
use crate::output::OutputFormat;

pub async fn run(
    pool: &SqlitePool,
    command: HistoryCommand,
    config: &crate::config::BhtuneConfig,
) -> anyhow::Result<()> {
    match command {
        HistoryCommand::List {
            outcome,
            limit,
            offset,
            output,
        } => list(pool, outcome.map(Into::into), limit, offset, output).await,
        HistoryCommand::Show { run_id, output } => show(pool, run_id, output).await,
        HistoryCommand::Revert {
            run_id,
            bridge_host,
            server,
            yes,
            output,
        } => revert(pool, config, run_id, bridge_host, server, yes, output).await,
        HistoryCommand::Prune {
            older_than_days,
            dry_run,
            output,
        } => prune(pool, config, older_than_days, dry_run, output).await,
    }
}

/// The fields of one run shown in `history list`'s `--output json` array -- deliberately a
/// subset matching the plain-text table's own columns exactly, not the full run detail
/// (that's `RunDetailJson`, for `history show`).
#[derive(serde::Serialize)]
struct RunSummaryJson {
    id: i64,
    loop_name: String,
    driver: bhtune_db::models::TuneDriver,
    outcome: bhtune_db::models::TuneOutcome,
    process_type: bhtune_core::ProcessType,
    started_at: chrono::DateTime<chrono::Utc>,
}

#[derive(serde::Serialize)]
struct RunListJson {
    runs: Vec<RunSummaryJson>,
    /// How many rows are in `runs` (i.e. this page) -- distinct from `total`, the count of
    /// every run matching the filter across all pages.
    returned: usize,
    total: i64,
}

/// Local projection of [`bhtune_db::models::TuneRunInitialReadings`] for JSON output: that
/// type deliberately doesn't derive `Serialize` itself (DB row shape stays decoupled from
/// any API/CLI JSON shape), so `history show --output json` needs its own copy.
#[derive(serde::Serialize)]
struct InitialReadingsJson {
    pv_ini: f32,
    mv_ini: f32,
    mv_range_low: f32,
    mv_range_high: f32,
    pv_range_high: f32,
    pv_range_low: f32,
    controller_direction: bhtune_core::ControllerDirection,
    mode_raw: Option<String>,
    mode_attribute_raw: Option<String>,
    setpoint_ini: Option<f32>,
}

impl From<bhtune_db::models::TuneRunInitialReadings> for InitialReadingsJson {
    fn from(r: bhtune_db::models::TuneRunInitialReadings) -> Self {
        Self {
            pv_ini: r.pv_ini,
            mv_ini: r.mv_ini,
            mv_range_low: r.mv_range_low,
            mv_range_high: r.mv_range_high,
            pv_range_high: r.pv_range_high,
            pv_range_low: r.pv_range_low,
            controller_direction: r.controller_direction,
            mode_raw: r.mode_raw,
            mode_attribute_raw: r.mode_attribute_raw,
            setpoint_ini: r.setpoint_ini,
        }
    }
}

/// Local projection of [`bhtune_db::models::TuneResultRow`] for JSON output (see
/// [`InitialReadingsJson`] for why this can't just derive `Serialize` on the DB type
/// directly).
#[derive(serde::Serialize)]
struct ResultJson {
    response_level: bhtune_core::ResponseLevel,
    kp: f32,
    ti_minutes: f32,
    td_minutes: f32,
    proportional: f32,
    integral: f32,
    derivative: f32,
}

impl From<&TuneResultRow> for ResultJson {
    fn from(r: &TuneResultRow) -> Self {
        Self {
            response_level: r.response_level,
            kp: r.kp,
            ti_minutes: r.ti_minutes,
            td_minutes: r.td_minutes,
            proportional: r.proportional,
            integral: r.integral,
            derivative: r.derivative,
        }
    }
}

/// Local projection of [`bhtune_db::models::TuneWriteRow`] for JSON output (see
/// [`InitialReadingsJson`] for why this can't just derive `Serialize` on the DB type
/// directly).
#[derive(serde::Serialize)]
struct WriteJson {
    /// Whether this is an original write-back of freshly calculated PID parameters, or a
    /// `bhtune history revert` undoing one (see [`WriteKind`]).
    kind: WriteKind,
    response_level: bhtune_core::ResponseLevel,
    written_at: chrono::DateTime<chrono::Utc>,
    /// The P/I/D values read from the driver before any write was attempted. `None` only
    /// when the pre-read itself failed, in which case every field below is also `None`.
    proportional_previous: Option<f32>,
    integral_previous: Option<f32>,
    derivative_previous: Option<f32>,
    /// `None` for a constant that was never attempted because an earlier one in the
    /// P/I/D write-and-verify sequence had already failed.
    proportional_written: Option<f32>,
    integral_written: Option<f32>,
    derivative_written: Option<f32>,
    proportional_readback: Option<f32>,
    integral_readback: Option<f32>,
    derivative_readback: Option<f32>,
    success: bool,
    error_message: Option<String>,
    /// `None` means no rollback was applicable -- either every constant wrote successfully
    /// or the pre-read failed before any write was attempted; always `None` for a `Revert`
    /// row. See `rollback_error` for what went wrong when this is
    /// `Some(RollbackState::Failed)`.
    rollback_state: Option<bhtune_db::models::RollbackState>,
    rollback_error: Option<String>,
}

impl From<&TuneWriteRow> for WriteJson {
    fn from(w: &TuneWriteRow) -> Self {
        Self {
            kind: w.kind,
            response_level: w.response_level,
            written_at: w.written_at,
            proportional_previous: w.previous.map(|p| p.proportional),
            integral_previous: w.previous.map(|p| p.integral),
            derivative_previous: w.previous.map(|p| p.derivative),
            proportional_written: w.proportional_written,
            integral_written: w.integral_written,
            derivative_written: w.derivative_written,
            proportional_readback: w.proportional_readback,
            integral_readback: w.integral_readback,
            derivative_readback: w.derivative_readback,
            success: w.success,
            error_message: w.error_message.clone(),
            rollback_state: w.rollback_state,
            rollback_error: w.rollback_error.clone(),
        }
    }
}

#[derive(serde::Serialize)]
struct RunDetailJson {
    id: i64,
    loop_name: String,
    driver: bhtune_db::models::TuneDriver,
    outcome: bhtune_db::models::TuneOutcome,
    failure_reason: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Name of the template snapshotted onto this run at start time -- not necessarily the
    /// template `template_name` currently resolves to in the catalog, since templates can be
    /// edited or re-versioned after a run is recorded (`safety-run-snapshot`).
    template_name: String,
    template_origin: bhtune_db::models::TemplateOrigin,
    config: bhtune_core::LoopConfig,
    initial_readings: Option<InitialReadingsJson>,
    samples_recorded: usize,
    results: Vec<ResultJson>,
    writes: Vec<WriteJson>,
    /// Outcome of the best-effort restore attempted after this run ended -- `None` if the
    /// run never mutated the loop, or hasn't ended yet (`safety-restore-guard`).
    restore_status: Option<bhtune_db::models::RestoreStatus>,
    restore_detail: Option<String>,
}

async fn list(
    pool: &SqlitePool,
    outcome: Option<bhtune_db::models::TuneOutcome>,
    limit: i64,
    offset: i64,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let mut filter = TuneRunFilter::default();
    if let Some(outcome) = outcome {
        filter = filter.with_outcome(outcome);
    }
    let pagination = Pagination::new(limit, offset);
    let runs = TuneRunRow::list(pool, &filter, pagination).await?;
    let total = TuneRunRow::count(pool, &filter).await?;

    match output {
        OutputFormat::Table => {
            if runs.is_empty() {
                println!("No runs found.");
                return Ok(());
            }

            println!(
                "{:<5} {:<30} {:<10} {:<10} {:<10} {:<25}",
                "ID", "LOOP", "DRIVER", "OUTCOME", "PROCESS", "STARTED"
            );
            for run in &runs {
                println!(
                    "{:<5} {:<30} {:<10} {:<10} {:<10} {:<25}",
                    run.id,
                    run.loop_name,
                    format!("{:?}", run.driver),
                    format!("{:?}", run.outcome),
                    format!("{:?}", run.config.process_type),
                    run.started_at.to_rfc3339(),
                );
            }
            println!("Showing {} of {total} total run(s).", runs.len());
        }
        OutputFormat::Json => {
            let json = RunListJson {
                returned: runs.len(),
                total,
                runs: runs
                    .iter()
                    .map(|run| RunSummaryJson {
                        id: run.id,
                        loop_name: run.loop_name.clone(),
                        driver: run.driver,
                        outcome: run.outcome,
                        process_type: run.config.process_type,
                        started_at: run.started_at,
                    })
                    .collect(),
            };
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }
    Ok(())
}

/// `bhtune history prune` -- applies `history-retention`'s age-based policy on demand,
/// instead of waiting for the next startup or (for `bhtune-server`) the next periodic sweep.
///
/// `older_than_days` overrides the configured `retention_days` policy for this invocation
/// only, matching this project's usual per-invocation-flag-overrides-persistent-config
/// pattern (mirroring `--db`/`--templates`); if neither is given there is nothing to prune
/// against, and that's a plain error rather than silently doing nothing -- an operator who
/// runs `bhtune history prune` clearly wants *something* deleted.
///
/// `--dry-run` reports a count and the exact cutoff timestamp that would be used, without
/// deleting anything, via [`TuneRunRow::count`] against the same [`TuneRunFilter`] shape
/// [`crate::retention::sweep_retention`] would delete against -- so a preview and the real
/// run can never disagree about which runs match. It deliberately doesn't itemize every
/// matching run (unlike `history list`, which paginates): the history table is allowed to be
/// large, and a prune preview only needs to answer "how many, and as of when", matching the
/// automatic sweep's own INFO log shape.
async fn prune(
    pool: &SqlitePool,
    config: &crate::config::BhtuneConfig,
    older_than_days: Option<u32>,
    dry_run: bool,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let days = older_than_days.or(config.retention_days).ok_or_else(|| {
        anyhow::anyhow!(
            "no retention policy configured; pass --older-than-days, or set --retention-days \
             / BHTUNE_RETENTION_DAYS / the config file's retention_days key"
        )
    })?;
    let now = chrono::Utc::now();
    let cutoff = crate::retention::cutoff_for(days, now);

    if dry_run {
        let count =
            TuneRunRow::count(pool, &TuneRunFilter::default().with_started_before(cutoff)).await?;
        match output {
            OutputFormat::Table => {
                println!(
                    "Would delete {count} run(s) started at or before {} ({days} day(s)).",
                    cutoff.to_rfc3339()
                );
            }
            OutputFormat::Json => {
                let json = PruneJson {
                    retention_days: days,
                    cutoff,
                    dry_run: true,
                    deleted: count as u64,
                };
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
        }
        return Ok(());
    }

    let deleted = crate::retention::sweep_retention(pool, days, now).await?;
    match output {
        OutputFormat::Table => {
            println!(
                "Deleted {deleted} run(s) started at or before {} ({days} day(s)).",
                cutoff.to_rfc3339()
            );
        }
        OutputFormat::Json => {
            let json = PruneJson {
                retention_days: days,
                cutoff,
                dry_run: false,
                deleted,
            };
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct PruneJson {
    retention_days: u32,
    cutoff: chrono::DateTime<chrono::Utc>,
    dry_run: bool,
    deleted: u64,
}

/// Renders an optional PID constant reading/write for `history show`'s plain-text table --
/// `"-"` for `None` (never attempted, or the pre-read failed), 4 decimal places otherwise.
fn fmt_opt_f32(value: Option<f32>) -> String {
    match value {
        Some(v) => format!("{v:.4}"),
        None => "-".to_string(),
    }
}

async fn show(pool: &SqlitePool, run_id: i64, output: OutputFormat) -> anyhow::Result<()> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;
    let samples = TuneSampleRow::list_for_run(pool, run_id).await?;
    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(pool, run_id).await?;

    match output {
        OutputFormat::Table => {
            println!("Run #{}: {}", run.id, run.loop_name);
            println!("  Driver:          {:?}", run.driver);
            println!("  Outcome:          {:?}", run.outcome);
            if let Some(reason) = &run.failure_reason {
                println!("  Failure reason:   {reason}");
            }
            println!("  Started at:       {}", run.started_at.to_rfc3339());
            if let Some(completed_at) = run.completed_at {
                println!("  Completed at:     {}", completed_at.to_rfc3339());
            }
            println!(
                "  Template:         {} ({:?})",
                run.template.name, run.template_origin
            );
            println!(
                "  Process/controller: {:?} / {:?}",
                run.config.process_type, run.config.controller_type
            );
            println!("  Relay amplitude:  {}%", run.config.relay_amp_percent);
            println!(
                "  Cycles skip/count: {} / {}",
                run.config.num_cycles_skip, run.config.num_cycles_count
            );

            if let Some(readings) = run.initial_readings {
                println!(
                    "  Initial PV / MV:  {} / {}",
                    readings.pv_ini, readings.mv_ini
                );
                println!(
                    "  MV range:         {} - {}",
                    readings.mv_range_low, readings.mv_range_high
                );
                println!(
                    "  PV range:         {} - {}",
                    readings.pv_range_low, readings.pv_range_high
                );
                println!("  Direction:        {:?}", readings.controller_direction);
            }

            println!("  Samples recorded: {}", samples.len());

            match (run.restore_status, &run.restore_detail) {
                (Some(bhtune_db::models::RestoreStatus::Confirmed), _) => {
                    println!("  Restore:          confirmed");
                }
                (Some(bhtune_db::models::RestoreStatus::Incomplete), detail) => {
                    println!(
                        "  Restore:          INCOMPLETE -- {}",
                        detail.as_deref().unwrap_or("no detail recorded")
                    );
                }
                (None, _) => {}
            }

            if !results.is_empty() {
                println!("  Calculated results:");
                println!(
                    "    {:<12} {:<10} {:<10} {:<10} {:<12} {:<10} {:<10}",
                    "LEVEL", "KP", "TI(min)", "TD(min)", "PROP", "INTEGRAL", "DERIV"
                );
                for r in &results {
                    println!(
                        "    {:<12} {:<10.4} {:<10.4} {:<10.4} {:<12.4} {:<10.4} {:<10.4}",
                        format!("{:?}", r.response_level),
                        r.kp,
                        r.ti_minutes,
                        r.td_minutes,
                        r.proportional,
                        r.integral,
                        r.derivative
                    );
                }
            }

            if !writes.is_empty() {
                println!("  PID write-back audit:");
                for w in &writes {
                    println!(
                        "    [{}] {:?} ({:?} level): success={} previous(P={} I={} D={}) \
                         written(P={} I={} D={}) readback(P={} I={} D={}){}",
                        w.written_at.to_rfc3339(),
                        w.kind,
                        w.response_level,
                        w.success,
                        fmt_opt_f32(w.previous.map(|p| p.proportional)),
                        fmt_opt_f32(w.previous.map(|p| p.integral)),
                        fmt_opt_f32(w.previous.map(|p| p.derivative)),
                        fmt_opt_f32(w.proportional_written),
                        fmt_opt_f32(w.integral_written),
                        fmt_opt_f32(w.derivative_written),
                        fmt_opt_f32(w.proportional_readback),
                        fmt_opt_f32(w.integral_readback),
                        fmt_opt_f32(w.derivative_readback),
                        w.error_message
                            .as_ref()
                            .map(|m| format!(" error={m}"))
                            .unwrap_or_default(),
                    );
                    if let Some(rollback_state) = w.rollback_state {
                        println!(
                            "        rollback: {rollback_state:?}{}",
                            w.rollback_error
                                .as_ref()
                                .map(|m| format!(" error={m}"))
                                .unwrap_or_default(),
                        );
                    }
                }
            }
        }
        OutputFormat::Json => {
            let json = RunDetailJson {
                id: run.id,
                loop_name: run.loop_name.clone(),
                driver: run.driver,
                outcome: run.outcome,
                failure_reason: run.failure_reason.clone(),
                started_at: run.started_at,
                completed_at: run.completed_at,
                template_name: run.template.name.clone(),
                template_origin: run.template_origin,
                config: run.config,
                initial_readings: run.initial_readings.map(InitialReadingsJson::from),
                samples_recorded: samples.len(),
                results: results.iter().map(ResultJson::from).collect(),
                writes: writes.iter().map(WriteJson::from).collect(),
                restore_status: run.restore_status,
                restore_detail: run.restore_detail.clone(),
            };
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
    }

    Ok(())
}

/// The result of an attempted revert, in `--output json` mode -- `history revert`'s
/// equivalent of `tune`'s [`WriteBackOutcome`], but never printed as prose ahead of it (see
/// this function's own doc comment for why).
#[derive(serde::Serialize)]
struct RevertJson {
    run_id: i64,
    /// The response level of the write-back being undone (recorded on the original `Write`
    /// row; the revert's own audit row is written under the same response level).
    response_level: bhtune_core::ResponseLevel,
    reverted_to: RevertedTargetJson,
    success: bool,
    error_message: Option<String>,
}

#[derive(serde::Serialize)]
struct RevertedTargetJson {
    proportional: f32,
    integral: f32,
    derivative: f32,
}

/// `bhtune history revert <run-id>`: writes a run's recorded pre-write-back PID values back
/// to the live loop, undoing whichever [`WriteKind::Write`] write-back that run last
/// recorded (`safety-writeback-rollback`, finding 6's revert companion command). Reuses
/// `commands::tune`'s own pre-read/write-and-verify machinery
/// ([`crate::commands::tune::read_previous_pid_values`]/
/// [`crate::commands::tune::write_and_verify_pid_value`]), so a revert is audited exactly
/// like an original write -- a new [`TuneWriteRow`] with `kind = WriteKind::Revert` -- the
/// one difference being that a revert never attempts a nested rollback of itself if it
/// fails partway through (see [`WriteKind`]'s doc comment for why).
///
/// Every rejection that happens *before* anything is attempted (no such run, wrong driver,
/// no write-back recorded, its pre-read failed so there is nothing to revert to, missing
/// `--yes`, no PID constant tags, or a failed connection) is a plain `Err`, which
/// `lib.rs`'s existing `fail()` reports through `--output json`'s own error contract --
/// exactly the same path `history show`'s "no such run" error already takes. Only the
/// outcome of an *attempted* revert is reported here, and only ever as prose gated on
/// `output == OutputFormat::Table` (never unconditionally, unlike `tune`'s own write-back
/// step -- see finding 8) or as the one `RevertJson` object printed on success.
#[allow(clippy::too_many_arguments)]
async fn revert(
    pool: &SqlitePool,
    config: &crate::config::BhtuneConfig,
    run_id: i64,
    bridge_host: Option<String>,
    server: Option<String>,
    yes: bool,
    output: OutputFormat,
) -> anyhow::Result<()> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;

    if run.driver != TuneDriver::Opcda {
        anyhow::bail!(
            "run {run_id} used the {:?} driver, which has no live loop to revert a write \
             against",
            run.driver
        );
    }

    let writes = TuneWriteRow::list_for_run(pool, run_id).await?;
    let last_write = writes
        .iter()
        .rev()
        .find(|w| w.kind == WriteKind::Write)
        .ok_or_else(|| anyhow::anyhow!("run {run_id} has no recorded PID write-back to revert"))?;
    let response_level = last_write.response_level;
    let target = last_write.previous.ok_or_else(|| {
        anyhow::anyhow!(
            "run {run_id}'s {response_level:?} PID write-back never recorded pre-write \
             values (its pre-read failed at the time); nothing to revert to"
        )
    })?;

    if !yes {
        anyhow::bail!("reverting writes PID constants back to a live loop; pass --yes to confirm");
    }

    let (Some(p_tag), Some(i_tag), Some(d_tag)) = (
        &run.tags.proportional_constant,
        &run.tags.integral_constant,
        &run.tags.derivative_constant,
    ) else {
        anyhow::bail!("run {run_id}'s snapshotted tags have no PID constant tags configured");
    };

    let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
    let server = crate::config::resolve_server(server, config)?;
    let driver = OpcDaDriver::connect(&bridge_host, &server).await?;

    if output == OutputFormat::Table {
        println!(
            "Reverting run {run_id}'s {response_level:?} PID write-back on '{}' to \
             P={:.4} I={:.4} D={:.4}...",
            run.loop_name, target.proportional, target.integral, target.derivative
        );
    }

    let written_at = chrono::Utc::now();
    let mut new_write = NewTuneWrite::new(response_level, written_at);
    new_write.kind = WriteKind::Revert;

    // Pre-read the loop's *current* values before writing -- same rationale as `tune`'s own
    // write-back: whatever is live right now becomes this revert row's `previous`, so a
    // second `history revert` could undo this one too if it ever turned out to be wrong.
    let live_previous = match crate::commands::tune::read_previous_pid_values(
        &driver,
        p_tag,
        i_tag,
        d_tag,
        run.allow_uncertain_quality,
    )
    .await
    {
        Ok(previous) => previous,
        Err(e) => {
            let message = e.to_string();
            new_write.error_message = Some(message.clone());
            TuneWriteRow::insert(pool, run_id, new_write).await?;
            anyhow::bail!("revert pre-read failed: {message}");
        }
    };
    new_write.previous = Some(live_previous);

    let steps: [(&str, &str, f32); 3] = [
        ("Proportional", p_tag.as_str(), target.proportional),
        ("Integral", i_tag.as_str(), target.integral),
        ("Derivative", d_tag.as_str(), target.derivative),
    ];
    let mut written_vals: [Option<f32>; 3] = [None; 3];
    let mut readback_vals: [Option<f32>; 3] = [None; 3];
    let mut failure: Option<String> = None;

    for (i, (label, tag, value)) in steps.into_iter().enumerate() {
        written_vals[i] = Some(value);
        match crate::commands::tune::write_and_verify_pid_value(
            &driver,
            label,
            tag,
            value,
            run.allow_uncertain_quality,
        )
        .await
        {
            Ok(readback) => readback_vals[i] = Some(readback),
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
    new_write.success = failure.is_none();
    new_write.error_message = failure.clone();

    TuneWriteRow::insert(pool, run_id, new_write).await?;

    match failure {
        None => {
            tracing::info!(run_id, ?response_level, "PID revert succeeded");
            match output {
                OutputFormat::Table => {
                    println!(
                        "Reverted and confirmed run {run_id}'s {response_level:?} PID write-back."
                    );
                }
                OutputFormat::Json => {
                    let json = RevertJson {
                        run_id,
                        response_level,
                        reverted_to: RevertedTargetJson {
                            proportional: target.proportional,
                            integral: target.integral,
                            derivative: target.derivative,
                        },
                        success: true,
                        error_message: None,
                    };
                    println!("{}", serde_json::to_string_pretty(&json)?);
                }
            }
            Ok(())
        }
        Some(error_message) => {
            tracing::error!(run_id, ?response_level, %error_message, "PID revert failed partway through; the loop may hold a mismatched set of PID constants -- see `history show` for the recorded partial state");
            anyhow::bail!(
                "revert failed partway through: {error_message} (the loop may now hold a \
                 mismatched set of PID constants -- see `history show {run_id}` for the \
                 recorded partial state)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{ControllerType, DcsTemplate, LoopConfig, LoopTags, ProcessType};
    use bhtune_db::models::{TemplateOrigin, TuneDriver, TuneRunInitialReadings};

    fn sample_config() -> LoopConfig {
        LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        }
    }

    fn sample_template() -> DcsTemplate {
        bhtune_core::built_in_templates().remove(0)
    }

    fn sample_tags() -> LoopTags {
        LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &sample_template())
    }

    #[tokio::test]
    async fn list_handles_an_empty_database() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        list(&pool, None, 50, 0, OutputFormat::Table).await.unwrap();
        list(&pool, None, 50, 0, OutputFormat::Json).await.unwrap();
    }

    #[tokio::test]
    async fn list_and_show_reflect_a_real_run() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Simulator,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            now,
        )
        .await
        .unwrap();

        TuneRunRow::record_initial_readings(
            &pool,
            run.id,
            TuneRunInitialReadings {
                pv_ini: 50.0,
                mv_ini: 50.0,
                mv_range_low: 0.0,
                mv_range_high: 100.0,
                pv_range_high: 100.0,
                pv_range_low: 0.0,
                controller_direction: bhtune_core::ControllerDirection::Reverse,
                mode_raw: Some("1".to_string()),
                mode_attribute_raw: None,
                setpoint_ini: Some(50.0),
            },
        )
        .await
        .unwrap();

        TuneRunRow::complete(&pool, run.id, now).await.unwrap();

        list(&pool, None, 50, 0, OutputFormat::Table).await.unwrap();
        list(
            &pool,
            Some(bhtune_db::models::TuneOutcome::Completed),
            50,
            0,
            OutputFormat::Table,
        )
        .await
        .unwrap();
        show(&pool, run.id, OutputFormat::Table).await.unwrap();
        // This run has recorded initial readings (unlike the other JSON-path fixtures in
        // this file), so this is the one call site that exercises `InitialReadingsJson`'s
        // conversion.
        show(&pool, run.id, OutputFormat::Json).await.unwrap();
    }

    #[tokio::test]
    async fn show_errors_for_an_unknown_run() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let err = show(&pool, 999, OutputFormat::Table).await.unwrap_err();
        assert!(err.to_string().contains("999"));
    }

    #[tokio::test]
    async fn show_handles_a_failed_run_with_no_initial_readings() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Simulator,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            now,
        )
        .await
        .unwrap();
        TuneRunRow::fail(&pool, run.id, now, "connection refused")
            .await
            .unwrap();
        show(&pool, run.id, OutputFormat::Table).await.unwrap();
        show(&pool, run.id, OutputFormat::Json).await.unwrap();
    }

    /// A run carrying at least one `TuneResultRow` and one `TuneWriteRow`, so `show`'s
    /// "Calculated results" and "PID write-back audit" print blocks (otherwise never
    /// exercised, since every other fixture in this file completes with no results/writes
    /// recorded) both execute.
    async fn run_with_results_and_writes() -> (SqlitePool, i64) {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Opcda,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            now,
        )
        .await
        .unwrap();
        TuneRunRow::complete(&pool, run.id, now).await.unwrap();

        TuneResultRow::insert(
            &pool,
            &TuneResultRow {
                id: 0,
                run_id: run.id,
                response_level: bhtune_core::ResponseLevel::Moderate,
                kp: 1.5,
                ti_minutes: 0.7,
                td_minutes: 0.15,
                proportional: 12.0,
                integral: 2.5,
                derivative: 0.6,
            },
        )
        .await
        .unwrap();

        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 2.0,
            derivative: 0.5,
        });
        successful.proportional_written = Some(12.0);
        successful.integral_written = Some(2.5);
        successful.derivative_written = Some(0.6);
        successful.proportional_readback = Some(12.0);
        successful.integral_readback = Some(2.5);
        successful.derivative_readback = Some(0.6);
        successful.success = true;
        TuneWriteRow::insert(&pool, run.id, successful)
            .await
            .unwrap();

        let mut failed =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        failed.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 2.0,
            derivative: 0.5,
        });
        failed.proportional_written = Some(12.0);
        failed.error_message = Some("mock failure".to_string());
        failed.rollback_state = Some(bhtune_db::models::RollbackState::Failed);
        failed.rollback_error = Some("mock rollback failure".to_string());
        TuneWriteRow::insert(&pool, run.id, failed).await.unwrap();

        (pool, run.id)
    }

    #[tokio::test]
    async fn show_prints_calculated_results_and_write_back_audit_rows() {
        let (pool, run_id) = run_with_results_and_writes().await;
        show(&pool, run_id, OutputFormat::Table).await.unwrap();
    }

    #[tokio::test]
    async fn list_output_json_is_valid_json_with_the_expected_shape() {
        let (pool, _run_id) = run_with_results_and_writes().await;
        // Can't easily capture stdout here, so this test's main job is proving the JSON
        // path doesn't panic/error across every branch -- the DTOs' field shapes are a
        // straightforward 1:1 projection of already-tested `bhtune-db` row structs.
        list(&pool, None, 50, 0, OutputFormat::Json).await.unwrap();
        list(
            &pool,
            Some(bhtune_db::models::TuneOutcome::Completed),
            50,
            0,
            OutputFormat::Json,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn show_output_json_covers_readings_results_and_writes() {
        let (pool, run_id) = run_with_results_and_writes().await;
        show(&pool, run_id, OutputFormat::Json).await.unwrap();
    }

    #[tokio::test]
    async fn run_dispatches_list_and_show_in_both_output_formats() {
        let (pool, run_id) = run_with_results_and_writes().await;
        let config = crate::config::BhtuneConfig::default();
        run(
            &pool,
            HistoryCommand::List {
                outcome: None,
                limit: 50,
                offset: 0,
                output: OutputFormat::Table,
            },
            &config,
        )
        .await
        .unwrap();
        run(
            &pool,
            HistoryCommand::List {
                outcome: None,
                limit: 50,
                offset: 0,
                output: OutputFormat::Json,
            },
            &config,
        )
        .await
        .unwrap();
        run(
            &pool,
            HistoryCommand::Show {
                run_id,
                output: OutputFormat::Table,
            },
            &config,
        )
        .await
        .unwrap();
        run(
            &pool,
            HistoryCommand::Show {
                run_id,
                output: OutputFormat::Json,
            },
            &config,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn run_dispatches_revert_and_surfaces_its_error() {
        // No live gateway is running, so `revert` fails while connecting -- but this still
        // proves `run` actually dispatches `HistoryCommand::Revert` to the `revert` function
        // rather than, say, silently no-op'ing. `revert`'s own success/failure/validation
        // behavior is covered directly by the `revert_*` tests below.
        let (pool, run_id) = run_with_results_and_writes().await;
        let config = crate::config::BhtuneConfig::default();
        let err = run(
            &pool,
            HistoryCommand::Revert {
                run_id,
                bridge_host: Some("127.0.0.1:1".to_string()),
                server: Some("Sim.Server".to_string()),
                yes: true,
                output: OutputFormat::Table,
            },
            &config,
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    /// Starts an `Opcda`-driver run (using the sample template/tags, which have PID
    /// constant tags configured) and returns it alongside the `BhtuneConfig` `revert`'s
    /// tests dispatch against, without recording any write-back yet -- each `revert_*` test
    /// below inserts whatever `TuneWriteRow` fixture its scenario needs.
    async fn opcda_run_with_no_writes() -> (SqlitePool, crate::config::BhtuneConfig, i64) {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Opcda,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            now,
        )
        .await
        .unwrap();
        let config = crate::config::BhtuneConfig::default();
        (pool, config, run.id)
    }

    #[tokio::test]
    async fn revert_errors_when_no_such_run_exists() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let config = crate::config::BhtuneConfig::default();
        let err = revert(&pool, &config, 999, None, None, true, OutputFormat::Table)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no run with id 999"));
    }

    #[tokio::test]
    async fn revert_errors_when_the_run_did_not_use_the_opcda_driver() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Simulator,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            now,
        )
        .await
        .unwrap();
        let config = crate::config::BhtuneConfig::default();
        let err = revert(
            &pool,
            &config,
            run.id,
            None,
            None,
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("Simulator"));
    }

    #[tokio::test]
    async fn revert_errors_when_no_write_back_is_recorded() {
        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let err = revert(
            &pool,
            &config,
            run_id,
            None,
            None,
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no recorded PID write-back"));
    }

    #[tokio::test]
    async fn revert_errors_when_the_original_writes_pre_read_failed() {
        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut failed_pre_read =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        failed_pre_read.error_message = Some("pre-read of Proportional tag 'X' failed".to_string());
        TuneWriteRow::insert(&pool, run_id, failed_pre_read)
            .await
            .unwrap();

        let err = revert(
            &pool,
            &config,
            run_id,
            None,
            None,
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("nothing to revert to"));
    }

    #[tokio::test]
    async fn revert_errors_when_yes_is_not_set() {
        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 2.0,
            derivative: 0.5,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run_id, successful)
            .await
            .unwrap();

        let err = revert(
            &pool,
            &config,
            run_id,
            None,
            None,
            false,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--yes"));
    }

    #[tokio::test]
    async fn revert_errors_when_the_snapshotted_tags_have_no_pid_constant_tags() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let mut tags = sample_tags();
        tags.proportional_constant = None;
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Opcda,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &tags,
            now,
        )
        .await
        .unwrap();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 2.0,
            derivative: 0.5,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run.id, successful)
            .await
            .unwrap();

        let config = crate::config::BhtuneConfig::default();
        let err = revert(
            &pool,
            &config,
            run.id,
            None,
            None,
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("no PID constant tags"));
    }

    #[tokio::test]
    async fn revert_errors_when_the_driver_connection_fails() {
        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 2.0,
            derivative: 0.5,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run_id, successful)
            .await
            .unwrap();

        // Port 1 is a privileged/unlikely-bound port; connecting should fail promptly,
        // proving every validation step above passed and `revert` genuinely reached the
        // connect step.
        let err = revert(
            &pool,
            &config,
            run_id,
            Some("127.0.0.1:1".to_string()),
            Some("Sim.Server".to_string()),
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn revert_succeeds_and_records_a_revert_kind_write() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

        // Every read (both the live pre-read and every write's confirmation readback)
        // returns this same fixed "10.0"/Good response regardless of which tag was
        // requested -- so the fixture's recorded `previous` values are all set to 10.0 too,
        // ensuring `write_and_verify_pid_value`'s tolerance check always sees a matching
        // readback no matter which constant is being reverted.
        let (host, server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "ignored".to_string(),
                    value: "10.0".to_string(),
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
        })
        .await;

        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 10.0,
            derivative: 10.0,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run_id, successful)
            .await
            .unwrap();

        revert(
            &pool,
            &config,
            run_id,
            Some(host),
            Some("Sim.Server".to_string()),
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap();

        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        let revert_row = writes.iter().find(|w| w.kind == WriteKind::Revert).unwrap();
        assert!(revert_row.success);
        assert_eq!(revert_row.proportional_written, Some(10.0));
        assert_eq!(revert_row.integral_written, Some(10.0));
        assert_eq!(revert_row.derivative_written, Some(10.0));
        assert_eq!(revert_row.proportional_readback, Some(10.0));
        assert_eq!(revert_row.integral_readback, Some(10.0));
        assert_eq!(revert_row.derivative_readback, Some(10.0));
        // Reverts never chain a nested rollback-of-a-revert.
        assert_eq!(revert_row.rollback_state, None);
        // The pre-read of the *live* current value (also fed by the fixed mock response)
        // becomes this revert row's own `previous`, so a second revert could undo it too.
        assert_eq!(
            revert_row.previous,
            Some(bhtune_db::models::WriteReadback {
                proportional: 10.0,
                integral: 10.0,
                derivative: 10.0,
            })
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn revert_records_a_failed_audit_row_when_a_later_constant_fails_verification() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

        // Calls 1-3: the live pre-read of P/I/D (all succeed). Call 4: Proportional's
        // post-write verification readback (succeeds). Call 5: Integral's post-write
        // verification readback -- fails, per `failing_read_from_call(5)`. Derivative is
        // never attempted, since the loop breaks on the first failure.
        let (host, server) = start_mock_server(
            MockBridgeService {
                read_response: ReadResponse {
                    values: vec![ProtoTagValue {
                        tag_id: "ignored".to_string(),
                        value: "10.0".to_string(),
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

        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 10.0,
            derivative: 10.0,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run_id, successful)
            .await
            .unwrap();

        let err = revert(
            &pool,
            &config,
            run_id,
            Some(host),
            Some("Sim.Server".to_string()),
            true,
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("revert failed partway through"));

        let writes = TuneWriteRow::list_for_run(&pool, run_id).await.unwrap();
        let revert_row = writes.iter().find(|w| w.kind == WriteKind::Revert).unwrap();
        assert!(!revert_row.success);
        assert!(
            revert_row
                .error_message
                .as_ref()
                .unwrap()
                .contains("Integral")
        );
        assert_eq!(revert_row.proportional_written, Some(10.0));
        assert_eq!(revert_row.proportional_readback, Some(10.0));
        assert_eq!(revert_row.integral_written, Some(10.0));
        assert_eq!(revert_row.integral_readback, None);
        assert_eq!(revert_row.derivative_written, None);
        assert_eq!(revert_row.derivative_readback, None);
        assert_eq!(revert_row.rollback_state, None);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn revert_succeeds_with_json_output_format() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

        // This isn't a substitute for a real stdout-capture test proving JSON mode never
        // interleaves prose ahead of the final object (that end-to-end contract belongs to
        // `safety-json-contract`'s subprocess test, across every subcommand at once) -- it
        // only proves `revert`'s `OutputFormat::Json` branch itself runs to completion
        // without erroring, i.e. that constructing and serializing `RevertJson` from a real
        // successful revert actually works, which the Table-mode test above never exercises.
        let (host, server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "ignored".to_string(),
                    value: "10.0".to_string(),
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
        })
        .await;

        let (pool, config, run_id) = opcda_run_with_no_writes().await;
        let now = chrono::Utc::now();
        let mut successful =
            bhtune_db::models::NewTuneWrite::new(bhtune_core::ResponseLevel::Moderate, now);
        successful.previous = Some(bhtune_db::models::WriteReadback {
            proportional: 10.0,
            integral: 10.0,
            derivative: 10.0,
        });
        successful.success = true;
        TuneWriteRow::insert(&pool, run_id, successful)
            .await
            .unwrap();

        revert(
            &pool,
            &config,
            run_id,
            Some(host),
            Some("Sim.Server".to_string()),
            true,
            OutputFormat::Json,
        )
        .await
        .unwrap();

        server.shutdown().await;
    }

    /// Starts a run with `started_at` set explicitly (unlike every other fixture in this
    /// file, which always uses `chrono::Utc::now()`) so `prune`'s tests can put a run
    /// unambiguously on either side of a retention cutoff.
    async fn start_run_at(pool: &SqlitePool, started_at: chrono::DateTime<chrono::Utc>) -> i64 {
        TuneRunRow::start(
            pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Simulator,
            sample_config(),
            TemplateOrigin::Builtin,
            &sample_template(),
            &sample_tags(),
            started_at,
        )
        .await
        .unwrap()
        .id
    }

    #[tokio::test]
    async fn prune_errors_when_no_retention_policy_is_configured() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let config = crate::config::BhtuneConfig::default();
        let err = prune(&pool, &config, None, false, OutputFormat::Table)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no retention policy configured"));
    }

    #[tokio::test]
    async fn prune_dry_run_reports_the_count_without_deleting_anything() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let old_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
        let recent_id = start_run_at(&pool, now - chrono::Duration::days(1)).await;
        let config = crate::config::BhtuneConfig {
            retention_days: Some(30),
            ..Default::default()
        };

        prune(&pool, &config, None, true, OutputFormat::Table)
            .await
            .unwrap();
        prune(&pool, &config, None, true, OutputFormat::Json)
            .await
            .unwrap();

        // Nothing was actually deleted by either dry-run call.
        assert!(TuneRunRow::get(&pool, old_id).await.unwrap().is_some());
        assert!(TuneRunRow::get(&pool, recent_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn prune_deletes_matching_runs_when_not_a_dry_run() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let old_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
        let recent_id = start_run_at(&pool, now - chrono::Duration::days(1)).await;
        let config = crate::config::BhtuneConfig {
            retention_days: Some(30),
            ..Default::default()
        };

        prune(&pool, &config, None, false, OutputFormat::Table)
            .await
            .unwrap();

        assert!(TuneRunRow::get(&pool, old_id).await.unwrap().is_none());
        assert!(TuneRunRow::get(&pool, recent_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn prune_older_than_days_overrides_the_configured_policy() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        // 45 days old: survives the configured 90-day policy, but not an ad-hoc
        // `--older-than-days 30` override.
        let run_id = start_run_at(&pool, now - chrono::Duration::days(45)).await;
        let config = crate::config::BhtuneConfig {
            retention_days: Some(90),
            ..Default::default()
        };

        prune(&pool, &config, Some(30), false, OutputFormat::Table)
            .await
            .unwrap();

        assert!(TuneRunRow::get(&pool, run_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn prune_json_output_is_a_single_parseable_object() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let config = crate::config::BhtuneConfig {
            retention_days: Some(30),
            ..Default::default()
        };
        // Only proves the `Json` branch runs to completion and produces a well-formed
        // `PruneJson` in both the dry-run and real-deletion cases -- the full
        // one-JSON-value-on-stdout contract across every subcommand belongs to
        // `safety-json-contract`'s dedicated subprocess test, not to this unit test.
        prune(&pool, &config, None, true, OutputFormat::Json)
            .await
            .unwrap();
        prune(&pool, &config, None, false, OutputFormat::Json)
            .await
            .unwrap();
    }
}
