//! CLI adapter for the shared tune runtime.

mod output;

pub use output::TuneOutcome;

use bhtune_core::{ControllerType, ProcessType, ResponseLevel};
use bhtune_db::SqlitePool;
use bhtune_runtime::cancel::CtrlC;
use bhtune_runtime::config::BhtuneConfig;
use bhtune_runtime::tune::{
    DriverKind, TuneRequest, TuneRunReport, drive_report, prepare, tune_outcome_for_run,
};

use crate::args::{DriverKindArg, TuneArgs};
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

    let request = into_tune_request(args);
    let prepared = prepare(pool, request, &config).await?;
    let mut handler = output::CliWriteBackHandler::new(output_format, requested_write_pid);
    let report = drive_report(pool, prepared, ctrl_c, Some(&mut handler)).await?;
    Ok(report_summary(report, output_format))
}

fn report_summary(report: TuneRunReport, output_format: OutputFormat) -> TuneOutcome {
    let outcome = tune_outcome_for_run(&report.outcome);
    let label = outcome.label();
    tracing::info!(run_id = report.run_id, outcome = label, "tune run finished");
    output::print_summary(report.run_id, &report.outcome, output_format)
}

fn into_tune_request(args: TuneArgs) -> TuneRequest {
    TuneRequest {
        tagname: args.tagname,
        template: args.template,
        process_type: ProcessType::from(args.process_type),
        controller_type: ControllerType::from(args.controller_type),
        relay_amp: args.relay_amp,
        cycles_skip: args.cycles_skip,
        cycles_count: args.cycles_count,
        noise_protection_secs: args.noise_protection_secs,
        driver: match args.driver {
            DriverKindArg::Opcda => DriverKind::Opcda,
            DriverKindArg::Simulator => DriverKind::Simulator,
        },
        bridge_host: args.bridge_host,
        server: args.server,
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
        tag_overrides: args.tag_overrides,
        notes: args.notes,
        yes: args.yes,
        write_pid: args.write_pid.map(ResponseLevel::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{
        ControllerTypeArg, DirectionArg, DriverKindArg, ProcessTypeArg, ResponseLevelArg,
    };

    #[test]
    fn cli_tune_arguments_convert_to_transport_neutral_runtime_request() {
        let request = into_tune_request(TuneArgs {
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
        });

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
