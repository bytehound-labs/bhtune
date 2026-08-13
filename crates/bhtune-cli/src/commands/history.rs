//! `bhtune history list/show`.

use bhtune_db::SqlitePool;
use bhtune_db::models::{
    Pagination, TuneResultRow, TuneRunFilter, TuneRunRow, TuneSampleRow, TuneWriteRow,
};

use crate::args::HistoryCommand;

pub async fn run(pool: &SqlitePool, command: HistoryCommand) -> anyhow::Result<()> {
    match command {
        HistoryCommand::List {
            outcome,
            limit,
            offset,
        } => list(pool, outcome.map(Into::into), limit, offset).await,
        HistoryCommand::Show { run_id } => show(pool, run_id).await,
    }
}

async fn list(
    pool: &SqlitePool,
    outcome: Option<bhtune_db::models::TuneOutcome>,
    limit: i64,
    offset: i64,
) -> anyhow::Result<()> {
    let mut filter = TuneRunFilter::default();
    if let Some(outcome) = outcome {
        filter = filter.with_outcome(outcome);
    }
    let pagination = Pagination::new(limit, offset);
    let runs = TuneRunRow::list(pool, &filter, pagination).await?;
    let total = TuneRunRow::count(pool, &filter).await?;

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
    Ok(())
}

async fn show(pool: &SqlitePool, run_id: i64) -> anyhow::Result<()> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;

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

    let samples = TuneSampleRow::list_for_run(pool, run_id).await?;
    println!("  Samples recorded: {}", samples.len());

    let results = TuneResultRow::list_for_run(pool, run_id).await?;
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

    let writes = TuneWriteRow::list_for_run(pool, run_id).await?;
    if !writes.is_empty() {
        println!("  PID write-back audit:");
        for w in &writes {
            println!(
                "    [{}] {:?} level: success={} P={} I={} D={}{}",
                w.written_at.to_rfc3339(),
                w.response_level,
                w.success,
                w.proportional_written,
                w.integral_written,
                w.derivative_written,
                w.error_message
                    .as_ref()
                    .map(|m| format!(" error={m}"))
                    .unwrap_or_default(),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{ControllerType, LoopConfig, ProcessType};
    use bhtune_db::models::{TuneBackend, TuneRunInitialReadings};

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

    #[tokio::test]
    async fn list_handles_an_empty_database() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        list(&pool, None, 50, 0).await.unwrap();
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
            },
        )
        .await
        .unwrap();

        TuneRunRow::complete(&pool, run.id, now).await.unwrap();

        list(&pool, None, 50, 0).await.unwrap();
        list(
            &pool,
            Some(bhtune_db::models::TuneOutcome::Completed),
            50,
            0,
        )
        .await
        .unwrap();
        show(&pool, run.id).await.unwrap();
    }

    #[tokio::test]
    async fn show_errors_for_an_unknown_run() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let err = show(&pool, 999).await.unwrap_err();
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
            now,
        )
        .await
        .unwrap();
        TuneRunRow::fail(&pool, run.id, now, "connection refused")
            .await
            .unwrap();
        show(&pool, run.id).await.unwrap();
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

        let written = bhtune_core::OpcWriteValues {
            response_level: bhtune_core::ResponseLevel::Moderate,
            proportional: 12.0,
            integral: 2.5,
            derivative: 0.6,
        };
        TuneWriteRow::insert_success(
            &pool,
            run.id,
            written,
            bhtune_db::models::WriteReadback {
                proportional: 12.0,
                integral: 2.5,
                derivative: 0.6,
            },
            now,
        )
        .await
        .unwrap();
        TuneWriteRow::insert_failure(&pool, run.id, written, now, "mock failure")
            .await
            .unwrap();

        (pool, run.id)
    }

    #[tokio::test]
    async fn show_prints_calculated_results_and_write_back_audit_rows() {
        let (pool, run_id) = run_with_results_and_writes().await;
        show(&pool, run_id).await.unwrap();
    }

    #[tokio::test]
    async fn run_dispatches_list_and_show() {
        let (pool, run_id) = run_with_results_and_writes().await;
        run(
            &pool,
            HistoryCommand::List {
                outcome: None,
                limit: 50,
                offset: 0,
            },
        )
        .await
        .unwrap();
        run(&pool, HistoryCommand::Show { run_id }).await.unwrap();
    }
}
