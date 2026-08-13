//! `bhtune export <run_id>`: dumps a run's recorded samples as CSV or JSON.
//!
//! CSV rows are generated from the same `#[derive(Serialize)]` struct as the header, making
//! the legacy app's MV/SV column-transposition bug (header `Time,PV,SV,MV,P,I,D` vs. data
//! `Time,PV,MV,SV,P,I,D` — see AGENTS.md's bug register) structurally impossible here.

use std::io::Write;

use bhtune_db::SqlitePool;
use bhtune_db::models::TuneSampleRow;
use serde::Serialize;

use crate::args::{ExportArgs, ExportFormat};

#[derive(Serialize)]
struct SampleRecord {
    tick: i64,
    time: chrono::DateTime<chrono::Utc>,
    pv: f32,
    hysteresis: f32,
    mv_value_current: f32,
    mv_sign_next_step: i8,
    counter_all_switches: u32,
    cycles_completed: i32,
    cycles_remaining: i32,
}

impl From<&TuneSampleRow> for SampleRecord {
    fn from(row: &TuneSampleRow) -> Self {
        SampleRecord {
            tick: row.tick_index,
            time: row.sample.time,
            pv: row.sample.pv,
            hysteresis: row.state.hysteresis,
            mv_value_current: row.state.mv_value_current,
            mv_sign_next_step: row.state.mv_sign_next_step,
            counter_all_switches: row.state.counter_all_switches,
            cycles_completed: row.state.cycles_completed,
            cycles_remaining: row.state.cycles_remaining,
        }
    }
}

pub async fn run(pool: &SqlitePool, args: ExportArgs) -> anyhow::Result<()> {
    let samples = TuneSampleRow::list_for_run(pool, args.run_id).await?;
    if samples.is_empty() {
        anyhow::bail!(
            "run {} has no recorded samples (unknown run id, or it never started)",
            args.run_id
        );
    }
    let records: Vec<SampleRecord> = samples.iter().map(SampleRecord::from).collect();

    let bytes = match args.format {
        ExportFormat::Csv => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            for record in &records {
                writer.serialize(record)?;
            }
            writer.into_inner()?
        }
        ExportFormat::Json => serde_json::to_vec_pretty(&records)?,
    };

    match &args.output {
        Some(path) => {
            std::fs::write(path, &bytes)
                .map_err(|e| anyhow::anyhow!("failed to write '{}': {e}", path.display()))?;
            println!(
                "Exported {} sample(s) from run {} to '{}'.",
                records.len(),
                args.run_id,
                path.display()
            );
        }
        None => {
            std::io::stdout().write_all(&bytes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::mrft::{MrftState, Tick};

    async fn pool_with_one_sample() -> (SqlitePool, i64) {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let run = bhtune_db::models::TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            bhtune_db::models::TuneBackend::Simulator,
            bhtune_core::LoopConfig {
                process_type: bhtune_core::ProcessType::Flow,
                controller_type: bhtune_core::ControllerType::Pi,
                relay_amp_percent: 10.0,
                num_cycles_skip: 1,
                num_cycles_count: 2,
                noise_protection_secs: 3,
                mrft_delay_secs: 0,
            },
            now,
        )
        .await
        .unwrap();

        TuneSampleRow::insert(
            &pool,
            run.id,
            0,
            Tick {
                time: now,
                pv: 50.0,
            },
            MrftState {
                hysteresis: 0.0,
                mv_value_current: 50.0,
                mv_sign_next_step: 1,
                counter_all_switches: 0,
                cycles_completed: 0,
                cycles_remaining: 2,
            },
        )
        .await
        .unwrap();

        (pool, run.id)
    }

    #[tokio::test]
    async fn exports_csv_to_stdout_when_no_output_path_given() {
        let (pool, run_id) = pool_with_one_sample().await;
        run(
            &pool,
            ExportArgs {
                run_id,
                format: ExportFormat::Csv,
                output: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn exports_csv_to_a_file_with_correct_columns() {
        let (pool, run_id) = pool_with_one_sample().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.csv");
        run(
            &pool,
            ExportArgs {
                run_id,
                format: ExportFormat::Csv,
                output: Some(path.clone()),
            },
        )
        .await
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let mut lines = contents.lines();
        let header = lines.next().unwrap();
        assert_eq!(
            header,
            "tick,time,pv,hysteresis,mv_value_current,mv_sign_next_step,counter_all_switches,cycles_completed,cycles_remaining"
        );
        let data = lines.next().unwrap();
        let fields: Vec<&str> = data.split(',').collect();
        assert_eq!(fields[0], "0"); // tick
        assert_eq!(fields[2], "50.0"); // pv, not swapped with mv
        assert_eq!(fields[4], "50.0"); // mv_value_current
    }

    #[tokio::test]
    async fn exports_json_to_a_file() {
        let (pool, run_id) = pool_with_one_sample().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        run(
            &pool,
            ExportArgs {
                run_id,
                format: ExportFormat::Json,
                output: Some(path.clone()),
            },
        )
        .await
        .unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(parsed[0]["pv"], 50.0);
    }

    #[tokio::test]
    async fn errors_for_a_run_with_no_samples() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let err = run(
            &pool,
            ExportArgs {
                run_id: 999,
                format: ExportFormat::Csv,
                output: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("999"));
    }
}
