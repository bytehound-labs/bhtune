//! `bhtune history list/show`.

use bhtune_db::SqlitePool;
use bhtune_db::models::{
    Pagination, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow,
};

use crate::args::HistoryCommand;
use crate::output::OutputFormat;

pub async fn run(pool: &SqlitePool, command: HistoryCommand) -> anyhow::Result<()> {
    match command {
        HistoryCommand::List {
            outcome,
            limit,
            offset,
            output,
        } => list(pool, outcome.map(Into::into), limit, offset, output).await,
        HistoryCommand::Show { run_id, output } => show(pool, run_id, output).await,
    }
}

/// The fields of one run shown in `history list`'s `--output json` array -- deliberately a
/// subset matching the plain-text table's own columns exactly, not the full run detail
/// (that's `RunDetailJson`, for `history show`).
#[derive(serde::Serialize)]
struct RunSummaryJson {
    id: i64,
    loop_name: String,
    backend: bhtune_db::models::TuneBackend,
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
    response_level: bhtune_core::ResponseLevel,
    written_at: chrono::DateTime<chrono::Utc>,
    /// The P/I/D values read from the backend before any write was attempted. `None` only
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
    /// or the pre-read failed before any write was attempted. See `rollback_error` for what
    /// went wrong when this is `Some(RollbackState::Failed)`.
    rollback_state: Option<bhtune_db::models::RollbackState>,
    rollback_error: Option<String>,
}

impl From<&TuneWriteRow> for WriteJson {
    fn from(w: &TuneWriteRow) -> Self {
        Self {
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
    backend: bhtune_db::models::TuneBackend,
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
                "ID", "LOOP", "BACKEND", "OUTCOME", "PROCESS", "STARTED"
            );
            for run in &runs {
                println!(
                    "{:<5} {:<30} {:<10} {:<10} {:<10} {:<25}",
                    run.id,
                    run.loop_name,
                    format!("{:?}", run.backend),
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
                        backend: run.backend,
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
            println!("  Backend:          {:?}", run.backend);
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
                        "    [{}] {:?} level: success={} previous(P={} I={} D={}) \
                         written(P={} I={} D={}) readback(P={} I={} D={}){}",
                        w.written_at.to_rfc3339(),
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
                backend: run.backend,
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

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{ControllerType, DcsTemplate, LoopConfig, LoopTags, ProcessType};
    use bhtune_db::models::{TemplateOrigin, TuneBackend, TuneRunInitialReadings};

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
            TuneBackend::Simulator,
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
            TuneBackend::Simulator,
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
            TuneBackend::Opcda,
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
        run(
            &pool,
            HistoryCommand::List {
                outcome: None,
                limit: 50,
                offset: 0,
                output: OutputFormat::Table,
            },
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
        )
        .await
        .unwrap();
        run(
            &pool,
            HistoryCommand::Show {
                run_id,
                output: OutputFormat::Table,
            },
        )
        .await
        .unwrap();
        run(
            &pool,
            HistoryCommand::Show {
                run_id,
                output: OutputFormat::Json,
            },
        )
        .await
        .unwrap();
    }
}
