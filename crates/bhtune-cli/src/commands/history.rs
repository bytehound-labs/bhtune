//! `bhtune history list/show/revert`.

use crate::args::HistoryCommand;
use crate::output::OutputFormat;
use bhtune_db::SqlitePool;
use bhtune_db::models::{
    MvActuationKind, MvActuationStatus, Pagination, SampleQuality, SamplingAdequacy, TimingSummary,
    TuneMvActuationRow, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow,
    WriteKind,
};
use bhtune_runtime::tune::pid_write_preview;

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
        } => revert(pool, run_id, bridge_host, server, yes, output, config).await,
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
    tag_name: String,
    notes: Option<String>,
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
    kp: Option<f32>,
    ti_minutes: Option<f32>,
    td_minutes: Option<f32>,
    proportional: Option<f32>,
    integral: Option<f32>,
    derivative: Option<f32>,
    status: bhtune_core::TuningResultStatus,
    invalid_reason: Option<bhtune_core::TuningResultInvalidReason>,
    controller_values: Option<bhtune_core::ControllerPidValues>,
    controller_target_error: Option<String>,
}

impl ResultJson {
    fn from_recorded(r: &TuneResultRow, run: &TuneRunRow) -> Self {
        let preview = pid_write_preview(r, run.config.controller_type, &run.template);
        Self {
            response_level: r.response_level,
            kp: r.kp,
            ti_minutes: r.ti_minutes,
            td_minutes: r.td_minutes,
            proportional: r.proportional,
            integral: r.integral,
            derivative: r.derivative,
            status: r.status,
            invalid_reason: r.invalid_reason,
            controller_values: preview.values,
            controller_target_error: preview.unavailable_reason,
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
struct MvActuationJson {
    id: i64,
    sequence: i64,
    kind: MvActuationKind,
    commanded_at: chrono::DateTime<chrono::Utc>,
    target_mv: f32,
    previous_commanded_mv: Option<f32>,
    tolerance: f32,
    confirmation_due_at: chrono::DateTime<chrono::Utc>,
    last_checked_at: Option<chrono::DateTime<chrono::Utc>>,
    readback_mv: Option<f32>,
    readback_quality: Option<SampleQuality>,
    attempt_count: i64,
    status: MvActuationStatus,
    detail: Option<String>,
}

impl From<&TuneMvActuationRow> for MvActuationJson {
    fn from(row: &TuneMvActuationRow) -> Self {
        Self {
            id: row.id,
            sequence: row.sequence,
            kind: row.kind,
            commanded_at: row.commanded_at,
            target_mv: row.target_mv,
            previous_commanded_mv: row.previous_commanded_mv,
            tolerance: row.tolerance,
            confirmation_due_at: row.confirmation_due_at,
            last_checked_at: row.last_checked_at,
            readback_mv: row.readback_mv,
            readback_quality: row.readback_quality,
            attempt_count: row.attempt_count,
            status: row.status,
            detail: row.detail.clone(),
        }
    }
}

#[derive(serde::Serialize)]
struct RunDetailJson {
    id: i64,
    tag_name: String,
    notes: Option<String>,
    driver: bhtune_db::models::TuneDriver,
    outcome: bhtune_db::models::TuneOutcome,
    failure_reason: Option<String>,
    started_at: chrono::DateTime<chrono::Utc>,
    completed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Name of the template snapshotted onto this run at start time -- not necessarily the
    /// template `template_name` currently resolves to in the catalog, since templates can be
    /// edited or re-versioned after a run is recorded.
    template_name: String,
    template_origin: bhtune_db::models::TemplateOrigin,
    config: bhtune_core::LoopConfig,
    pid_rounding: bhtune_core::PidRounding,
    /// The resolved OPC DA server ProgID this run actually used, or `None` for a
    /// simulator/replay run. This is what `history revert`
    /// trusts over any `--server` flag, rather than re-resolving one.
    opc_server: Option<String>,
    /// The resolved bridge host this run actually used, matching `opc_server` above.
    bridge_host: Option<String>,
    /// The gateway compatibility snapshot recorded before this run mutated the loop.
    gateway_compatibility: Option<bhtune_driver::OpcDaGatewayCompatibility>,
    initial_readings: Option<InitialReadingsJson>,
    timing_metrics: Option<bhtune_db::models::TimingMetrics>,
    samples_recorded: usize,
    results: Vec<ResultJson>,
    writes: Vec<WriteJson>,
    mv_actuations: Vec<MvActuationJson>,
    /// Outcome of the best-effort restore attempted after this run ended -- `None` if the
    /// run never mutated the loop, or hasn't ended yet.
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
                "ID", "TAG NAME", "DRIVER", "OUTCOME", "PROCESS", "STARTED"
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
                        tag_name: run.loop_name.clone(),
                        notes: run.notes.clone(),
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

fn has_rows<T>(rows: &[T]) -> bool {
    !rows.is_empty()
}

fn has_recorded_connection(opc_server: Option<&str>, bridge_host: Option<&str>) -> bool {
    opc_server.is_some() || bridge_host.is_some()
}

fn is_table_output(output: OutputFormat) -> bool {
    output == OutputFormat::Table
}

/// `bhtune history prune` -- applies the age-based retention policy on demand,
/// instead of waiting for the next startup or (for `bhtune-server`) the next periodic sweep.
///
/// `older_than_days` overrides the configured `retention_days` policy for this invocation
/// only, matching this project's usual per-invocation-flag-overrides-persistent-config
/// pattern (mirroring `--db`/`--templates`); if neither is given there is nothing to prune
/// against, and that's a plain error rather than silently doing nothing -- an operator who
/// runs `bhtune history prune` clearly wants *something* deleted.
///
/// `--dry-run` reports a count and the exact cutoff timestamp that would be used, without
/// deleting anything, via [`TuneRunRow::count_deletable_matching`] against the same
/// [`TuneRunFilter`] shape [`crate::retention::sweep_retention`] would delete against. Runs
/// with an eligible, active, or incomplete live recovery are excluded from both the preview
/// and the actual delete. It deliberately doesn't itemize every matching run (unlike
/// `history list`, which paginates): the history table is allowed to be large, and a prune
/// preview only needs to answer "how many, and as of when", matching the automatic sweep's
/// own INFO log shape.
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
        let count = TuneRunRow::count_deletable_matching(
            pool,
            &TuneRunFilter::default().with_started_before(cutoff),
        )
        .await?;
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

fn fmt_opt_f64(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.3}"),
        None => "-".to_string(),
    }
}

fn sampling_adequacy_label(adequacy: SamplingAdequacy) -> &'static str {
    match adequacy {
        SamplingAdequacy::Adequate => "adequate",
        SamplingAdequacy::Marginal => "marginal",
        SamplingAdequacy::NotAssessed => "not assessed",
    }
}

fn print_latency_summary(label: &str, summary: TimingSummary) {
    if summary.count == 0 {
        println!("    {label:<26} none");
        return;
    }

    println!(
        "    {label:<26} count={} mean={} ms max={} ms",
        summary.count,
        fmt_opt_f64(summary.mean_ms),
        fmt_opt_f64(summary.max_ms),
    );
}

fn print_show_table(
    run: &TuneRunRow,
    samples: &[TuneSampleRow],
    results: &[TuneResultRow],
    writes: &[TuneWriteRow],
    mv_actuations: &[TuneMvActuationRow],
) -> anyhow::Result<()> {
    let gateway_compatibility = run
        .gateway_compatibility_json
        .as_deref()
        .map(crate::gateway::parse_stored_compatibility)
        .transpose()?;

    println!("Run #{} — Tag name: {}", run.id, run.loop_name);
    println!("  Notes:           {}", run.notes.as_deref().unwrap_or("—"));
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
    if has_recorded_connection(run.opc_server.as_deref(), run.bridge_host.as_deref()) {
        println!(
            "  Connection:       server={} bridge_host={}",
            run.opc_server.as_deref().unwrap_or("-"),
            run.bridge_host.as_deref().unwrap_or("-"),
        );
    }
    if let Some(snapshot) = gateway_compatibility {
        println!(
            "  Gateway compatibility: {} ({})",
            snapshot.status.as_str(),
            snapshot
                .gateway_version
                .as_deref()
                .unwrap_or("version not reported"),
        );
    }
    println!(
        "  Process/controller: {:?} / {:?}",
        run.config.process_type, run.config.controller_type
    );
    println!("  Relay amplitude:  {}%", run.config.relay_amp_percent);
    println!(
        "  Cycles skip/count: {} / {}",
        run.config.num_cycles_skip, run.config.num_cycles_count
    );

    if let Some(readings) = &run.initial_readings {
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

    if let Some(timing) = run.timing_metrics {
        println!("  Timing:");
        println!("    Basis:                    {:?}", timing.basis);
        println!(
            "    Requested interval:       {} ms",
            timing.requested_interval_ms
        );
        println!("    Observed sample gaps:     {}", timing.sample_gap_count);
        println!(
            "    Mean / max sample gap:    {} / {} ms",
            fmt_opt_f64(timing.mean_sample_gap_ms),
            fmt_opt_f64(timing.max_sample_gap_ms)
        );
        println!(
            "    Missed poll opportunities: {}",
            timing.missed_poll_opportunity_count
        );
        println!(
            "    Oscillation period:       {} ms",
            fmt_opt_f64(timing.measured_oscillation_period_ms)
        );
        println!(
            "    Approx. samples / period: {}",
            fmt_opt_f64(timing.approximate_samples_per_period)
        );
        println!(
            "    Sampling adequacy:        {}",
            sampling_adequacy_label(timing.sampling_adequacy)
        );
        if let Some(poll_latency) = timing.poll_latency {
            println!("    Poll latency:");
            print_latency_summary("PV reads:", poll_latency.pv_read);
            print_latency_summary("MV writes:", poll_latency.mv_write);
            print_latency_summary("MV verification:", poll_latency.mv_verification);
            print_latency_summary("Sample persistence:", poll_latency.sample_persist);
            print_latency_summary("Total tick work:", poll_latency.tick_work);
        }
    }

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

    print_show_results(run, results);
    print_show_writes(writes);
    print_show_mv_actuations(mv_actuations);
    Ok(())
}

fn print_show_results(run: &TuneRunRow, results: &[TuneResultRow]) {
    if !has_rows(results) {
        return;
    }
    println!("  Calculated results:");
    println!(
        "    PROP / INTEGRAL / DERIV are controller-ready targets; KP / TI / TD are raw calculations."
    );
    println!(
        "    {:<12} {:<10} {:<10} {:<10} {:<10} {:<12} {:<10} {:<10}  REASON",
        "LEVEL", "STATUS", "KP", "TI(min)", "TD(min)", "PROP", "INTEGRAL", "DERIV"
    );
    for result in results {
        let preview = pid_write_preview(result, run.config.controller_type, &run.template);
        let values = preview.values.as_ref();
        println!(
            "    {:<12} {:<10} {:<10} {:<10} {:<10} {:<12} {:<10} {:<10}  {}",
            format!("{:?}", result.response_level),
            format!("{:?}", result.status),
            fmt_opt_f32(result.kp),
            fmt_opt_f32(result.ti_minutes),
            fmt_opt_f32(result.td_minutes),
            values
                .map(|values| values.proportional.display.as_str())
                .unwrap_or("-"),
            values
                .map(|values| values.integral.display.as_str())
                .unwrap_or("-"),
            values
                .map(|values| values.derivative.display.as_str())
                .unwrap_or("-"),
            preview.unavailable_reason.as_deref().unwrap_or("-"),
        );
    }
}

fn print_show_writes(writes: &[TuneWriteRow]) {
    if !has_rows(writes) {
        return;
    }
    println!("  PID write-back audit:");
    for write in writes {
        println!(
            "    [{}] {:?} ({:?} level): success={} previous(P={} I={} D={}) \
             written(P={} I={} D={}) readback(P={} I={} D={}){}",
            write.written_at.to_rfc3339(),
            write.kind,
            write.response_level,
            write.success,
            fmt_pid_recorded(write.previous.map(|p| p.proportional)),
            fmt_pid_recorded(write.previous.map(|p| p.integral)),
            fmt_pid_recorded(write.previous.map(|p| p.derivative)),
            fmt_pid_recorded(write.proportional_written),
            fmt_pid_recorded(write.integral_written),
            fmt_pid_recorded(write.derivative_written),
            fmt_pid_recorded(write.proportional_readback),
            fmt_pid_recorded(write.integral_readback),
            fmt_pid_recorded(write.derivative_readback),
            write
                .error_message
                .as_ref()
                .map(|message| format!(" error={message}"))
                .unwrap_or_default(),
        );
        print_show_rollback(write);
    }
}

fn fmt_pid_recorded(value: Option<f32>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn print_show_rollback(write: &TuneWriteRow) {
    let Some(rollback_state) = write.rollback_state else {
        return;
    };
    println!(
        "        rollback: {rollback_state:?}{}",
        write
            .rollback_error
            .as_ref()
            .map(|message| format!(" error={message}"))
            .unwrap_or_default(),
    );
}

fn print_show_mv_actuations(actuations: &[TuneMvActuationRow]) {
    if !has_rows(actuations) {
        return;
    }
    println!("  MV actuation verification:");
    println!(
        "    {:<5} {:<8} {:<25} {:<10} {:<10} {:<10} {:<10} {:<12} DETAIL",
        "SEQ", "KIND", "COMMANDED", "TARGET", "READBACK", "TOLERANCE", "ATTEMPTS", "STATUS"
    );
    for actuation in actuations {
        println!(
            "    {:<5} {:<8} {:<25} {:<10} {:<10} {:<10} {:<10} {:<12} {}",
            actuation.sequence,
            format!("{:?}", actuation.kind),
            actuation.commanded_at.to_rfc3339(),
            fmt_opt_f32(Some(actuation.target_mv)),
            fmt_opt_f32(actuation.readback_mv),
            fmt_opt_f32(Some(actuation.tolerance)),
            actuation.attempt_count,
            format!("{:?}", actuation.status),
            actuation.detail.as_deref().unwrap_or("-"),
        );
        println!(
            "        confirmation due: {}  last checked: {}  quality: {}",
            actuation.confirmation_due_at.to_rfc3339(),
            actuation
                .last_checked_at
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
            actuation
                .readback_quality
                .map(|quality| format!("{quality:?}"))
                .unwrap_or_else(|| "-".to_string()),
        );
    }
}

fn print_show_json(
    run: &TuneRunRow,
    samples: &[TuneSampleRow],
    results: &[TuneResultRow],
    writes: &[TuneWriteRow],
    mv_actuations: &[TuneMvActuationRow],
) -> anyhow::Result<()> {
    let json = RunDetailJson {
        id: run.id,
        tag_name: run.loop_name.clone(),
        notes: run.notes.clone(),
        driver: run.driver,
        outcome: run.outcome,
        failure_reason: run.failure_reason.clone(),
        started_at: run.started_at,
        completed_at: run.completed_at,
        template_name: run.template.name.clone(),
        template_origin: run.template_origin,
        config: run.config,
        pid_rounding: run.template.pid_rounding,
        opc_server: run.opc_server.clone(),
        bridge_host: run.bridge_host.clone(),
        gateway_compatibility: run
            .gateway_compatibility_json
            .as_deref()
            .map(crate::gateway::parse_stored_compatibility)
            .transpose()?,
        initial_readings: run
            .initial_readings
            .as_ref()
            .map(|readings| InitialReadingsJson::from(readings.clone())),
        timing_metrics: run.timing_metrics,
        samples_recorded: samples.len(),
        results: results
            .iter()
            .map(|result| ResultJson::from_recorded(result, run))
            .collect(),
        writes: writes.iter().map(WriteJson::from).collect(),
        mv_actuations: mv_actuations.iter().map(MvActuationJson::from).collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail.clone(),
    };
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

async fn show(pool: &SqlitePool, run_id: i64, output: OutputFormat) -> anyhow::Result<()> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;
    let samples = TuneSampleRow::list_for_run(pool, run_id).await?;
    let results = TuneResultRow::list_for_run(pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(pool, run_id).await?;
    let mv_actuations = TuneMvActuationRow::list_for_run(pool, run_id).await?;

    match output {
        OutputFormat::Table => print_show_table(&run, &samples, &results, &writes, &mv_actuations)?,
        OutputFormat::Json => print_show_json(&run, &samples, &results, &writes, &mv_actuations)?,
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

/// Resolves the OPC DA connection a revert should use for `run` -- always its own recorded
/// `opc_server`/`bridge_host`, never a value re-resolved from
/// `--server`/`--bridge-host`/config at revert time. Re-resolving at revert time is exactly
/// unsafe: running `history revert` from a shell whose flags/config point at a *different*
/// gateway could write the wrong plant's old PID constants under tag names that may happen
/// to exist on both. An explicit `--server`/`--bridge-host`
/// flag is still accepted, but purely as a cross-check against the recorded value -- a
/// contradicting flag is a hard error, never a silent override, and there is no fallback to
/// config when a flag is omitted (unlike every other command's connection resolution).
#[cfg(test)]
fn resolve_revert_connection(
    run: &TuneRunRow,
    bridge_host_flag: Option<&str>,
    server_flag: Option<&str>,
) -> anyhow::Result<(String, String)> {
    bhtune_runtime::history::resolve_revert_connection(run, bridge_host_flag, server_flag)
}

/// `bhtune history revert <run-id>`: adapts the confirmation and output behavior for the
/// shared runtime's audited revert operation. The runtime owns eligibility checks,
/// connection provenance, pre-read/write/verify, and audit persistence, including the rule
/// that a revert never attempts a nested rollback of itself.
///
/// The OPC DA connection is never re-resolved from `--server`/`--bridge-host`/config the way
/// every other command resolves it -- see [`resolve_revert_connection`] for why that would
/// be a live-plant safety bug, not just an inconsistency. The runtime performs this
/// cross-check before it connects or writes.
///
/// Every rejection that happens *before* anything is attempted (no such run, wrong driver,
/// no write-back recorded, its pre-read failed so there is nothing to revert to, missing
/// `--yes`, no PID constant tags, a contradicting/missing recorded connection, or a failed
/// connection) is a plain `Err`, which `lib.rs`'s existing `fail()` reports through
/// `--output json`'s own error contract -- exactly the same path `history show`'s "no such
/// run" error already takes. Only the outcome of an *attempted* revert is reported here, as
/// prose gated on `output == OutputFormat::Table` or as the one `RevertJson` object printed
/// on success.
#[allow(clippy::too_many_arguments)]
async fn revert(
    pool: &SqlitePool,
    run_id: i64,
    bridge_host: Option<String>,
    server: Option<String>,
    yes: bool,
    output: OutputFormat,
    config: &crate::config::BhtuneConfig,
) -> anyhow::Result<()> {
    let result = bhtune_runtime::history::revert_run_with_progress(
        pool,
        bhtune_runtime::history::RevertRequest {
            run_id,
            bridge_host,
            server,
            confirmed: yes,
        },
        config.allow_uncertain_quality,
        |progress| {
            if is_table_output(output) {
                println!(
                    "Reverting run {}'s {:?} PID write-back on tag '{}' to \
                     P={} I={} D={}...",
                    progress.run_id,
                    progress.response_level,
                    progress.loop_name,
                    progress.target.proportional,
                    progress.target.integral,
                    progress.target.derivative
                );
            }
        },
    )
    .await?;
    let response_level = result.response_level;
    let target = result.target;

    match result.outcome {
        bhtune_runtime::tune::PidWriteOutcome::Written => {
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
        bhtune_runtime::tune::PidWriteOutcome::Failed { detail } => {
            tracing::error!(run_id, ?response_level, error_message = %detail, "PID revert failed partway through; the loop may hold a mismatched set of PID constants -- see `history show` for the recorded partial state");
            anyhow::bail!(
                "revert failed partway through: {detail} (the loop may now hold a mismatched \
                 set of PID constants -- see `history show {run_id}` for the recorded partial \
                 state)"
            );
        }
    }
}

#[cfg(test)]
mod tests;
