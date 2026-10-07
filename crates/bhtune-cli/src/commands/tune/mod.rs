//! CLI adapter for the shared tune runtime.

mod output;

pub use output::TuneOutcome;

use bhtune_db::SqlitePool;
use bhtune_db::models::{TuneResultRow, TuneRunRow};
use bhtune_runtime::cancel::CtrlC;
use bhtune_runtime::config::BhtuneConfig;
use bhtune_runtime::tune::{
    TuneRunReport, ValidatedTuneRequest, drive_report, pid_write_preview, prepare,
    tune_outcome_for_run,
};

use crate::args::TuneArgs;
use crate::output::OutputFormat;

pub use bhtune_runtime::tune::{PidWriteOutcome, write_pid_values};

pub(crate) async fn run_with_ctrl_c(
    pool: &SqlitePool,
    args: TuneArgs,
    config: &BhtuneConfig,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<TuneOutcome> {
    let output_format = args.output;
    let requested_write_pid = args.write_pid.map(Into::into);

    #[cfg(test)]
    let config = {
        let mut config = config.clone();
        config.tuning.mrft_delay_secs = Some(args.mrft_delay);
        config.tuning.poll_interval_ms = Some(args.poll_interval_ms);
        config.tuning.timeout_secs = Some(args.timeout_secs);
        config.tuning.op_timeout_secs = Some(args.op_timeout_secs);
        config.tuning.restore_timeout_secs = Some(args.restore_timeout_secs);
        config
    };

    #[cfg(not(test))]
    let config = config.clone();

    let request = ValidatedTuneRequest::try_from(args)?;
    let prepared = prepare(pool, request, &config).await?;
    let mut handler = output::CliWriteBackHandler::new(output_format, requested_write_pid);
    let report = drive_report(pool, prepared, ctrl_c, Some(&mut handler)).await?;
    report_summary(pool, report, output_format).await
}

async fn report_summary(
    pool: &SqlitePool,
    report: TuneRunReport,
    output_format: OutputFormat,
) -> anyhow::Result<TuneOutcome> {
    let run = TuneRunRow::get(pool, report.run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {}", report.run_id))?;
    let results = TuneResultRow::list_for_run(pool, report.run_id).await?;
    let previews = results
        .iter()
        .map(|result| pid_write_preview(result, run.config.controller_type, &run.template))
        .collect::<Vec<_>>();
    let outcome = tune_outcome_for_run(&report.outcome);
    let label = outcome.label();
    tracing::info!(run_id = report.run_id, outcome = label, "tune run finished");
    Ok(output::print_summary(
        report.run_id,
        &report.outcome,
        output_format,
        &previews,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{
        ControllerTypeArg, DirectionArg, DriverKindArg, ProcessTypeArg, ResponseLevelArg,
    };
    use bhtune_core::{ControllerType, ProcessType, ResponseLevel};
    use bhtune_runtime::tune::DriverKind;

    #[tokio::test]
    async fn summary_reports_missing_persisted_context_explicitly() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let report = TuneRunReport {
            run_id: 9999,
            outcome: bhtune_runtime::tune::RunOutcome::Aborted(
                bhtune_runtime::tune::AbortReason::UserInterrupt,
            ),
        };
        let error = report_summary(&pool, report, OutputFormat::Json)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "no run with id 9999");
    }

    #[test]
    fn cli_tune_arguments_convert_to_validated_runtime_request() {
        let request = ValidatedTuneRequest::try_from(TuneArgs {
            tagname: "Unit1.LIC101.PV".to_string(),
            template: "Honeywell".to_string(),
            process_type: ProcessTypeArg::Flow,
            controller_type: ControllerTypeArg::Pi,
            relay_amp: 10.0,
            cycles_skip: Some(1),
            cycles_count: Some(2),
            noise_protection_secs: Some(3),
            mrft_delay: 0,
            driver: DriverKindArg::Opcda,
            bridge_host: Some("127.0.0.1:7600".to_string()),
            server: Some("MockServer".to_string()),
            sim_gain: 1.0,
            sim_tau: 2.0,
            sim_dead_time: 5.0,
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
            poll_interval_ms: 800,
            timeout_secs: 3600,
            op_timeout_secs: 30,
            restore_timeout_secs: 30,
            notes: Some("scheduled".to_string()),
            yes: true,
            write_pid: Some(ResponseLevelArg::Moderate),
            output: OutputFormat::Json,
        })
        .unwrap();
        let request = request.as_request();

        assert_eq!(request.process_type, ProcessType::Flow);
        assert_eq!(request.controller_type, ControllerType::Pi);
        assert_eq!(
            request.direction,
            Some(bhtune_core::ControllerDirection::Reverse)
        );
        assert_eq!(request.driver, DriverKind::Opcda);
        assert_eq!(request.write_pid, Some(ResponseLevel::Moderate));
        assert_eq!(request.notes.as_deref(), Some("scheduled"));
    }
}
