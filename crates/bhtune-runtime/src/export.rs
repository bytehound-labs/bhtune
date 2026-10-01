//! Shared serialization for persisted run samples.

use bhtune_db::models::{SampleQuality, TuneSampleRow};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleExportFormat {
    Csv,
    Json,
}

#[derive(Serialize)]
pub struct SampleRecord {
    tick: i64,
    time: chrono::DateTime<chrono::Utc>,
    pv: f32,
    pv_quality: SampleQuality,
    hysteresis: f32,
    mv_value_current: f32,
    mv_sign_next_step: i8,
    counter_all_switches: u32,
    cycles_completed: i32,
    cycles_remaining: i32,
}

impl From<&TuneSampleRow> for SampleRecord {
    fn from(row: &TuneSampleRow) -> Self {
        Self {
            tick: row.tick_index,
            time: row.sample.time,
            pv: row.sample.pv,
            pv_quality: row.pv_quality,
            hysteresis: row.state.hysteresis,
            mv_value_current: row.state.mv_value_current,
            mv_sign_next_step: row.state.mv_sign_next_step,
            counter_all_switches: row.state.counter_all_switches,
            cycles_completed: row.state.cycles_completed,
            cycles_remaining: row.state.cycles_remaining,
        }
    }
}

/// Serializes a run's recorded samples to CSV or JSON bytes. The CLI and HTTP adapters use
/// this function so their export formats stay identical.
pub fn samples_to_bytes(
    samples: &[TuneSampleRow],
    format: SampleExportFormat,
) -> anyhow::Result<Vec<u8>> {
    let records: Vec<SampleRecord> = samples.iter().map(SampleRecord::from).collect();
    match format {
        SampleExportFormat::Csv => {
            let mut writer = csv::Writer::from_writer(Vec::new());
            for record in &records {
                writer.serialize(record)?;
            }
            Ok(writer.into_inner()?)
        }
        SampleExportFormat::Json => Ok(serde_json::to_vec_pretty(&records)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::mrft::{MrftState, Tick};

    async fn pool_with_one_sample() -> (bhtune_db::SqlitePool, i64) {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let now = chrono::Utc::now();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let run = bhtune_db::models::TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            bhtune_db::models::TuneDriver::Simulator,
            bhtune_core::LoopConfig {
                process_type: bhtune_core::ProcessType::Flow,
                controller_type: bhtune_core::ControllerType::Pi,
                relay_amp_percent: 10.0,
                num_cycles_skip: 1,
                num_cycles_count: 2,
                noise_protection_secs: 3,
                mrft_delay_secs: 0,
            },
            bhtune_db::models::TemplateOrigin::Builtin,
            &template,
            &tags,
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
            SampleQuality::Good,
        )
        .await
        .unwrap();

        (pool, run.id)
    }

    #[tokio::test]
    async fn serializes_samples_to_csv_with_ordered_columns() {
        let (pool, run_id) = pool_with_one_sample().await;
        let samples = TuneSampleRow::list_for_run(&pool, run_id).await.unwrap();
        let bytes = samples_to_bytes(&samples, SampleExportFormat::Csv).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next().unwrap(),
            "tick,time,pv,pv_quality,hysteresis,mv_value_current,mv_sign_next_step,counter_all_switches,cycles_completed,cycles_remaining"
        );
        assert!(lines.next().unwrap().starts_with("0,"));
    }

    #[tokio::test]
    async fn serializes_samples_to_json_with_the_cli_export_shape() {
        let (pool, run_id) = pool_with_one_sample().await;
        let samples = TuneSampleRow::list_for_run(&pool, run_id).await.unwrap();
        let bytes = samples_to_bytes(&samples, SampleExportFormat::Json).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed[0]["pv"], 50.0);
        assert_eq!(parsed[0]["tick"], 0);
    }
}
