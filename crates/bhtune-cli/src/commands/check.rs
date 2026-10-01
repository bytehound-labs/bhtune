//! Read-only tune preflight command.

use std::process::ExitCode;

use bhtune_core::DcsTemplate;
use bhtune_db::SqlitePool;
use bhtune_runtime::{
    config::BhtuneConfig,
    tune::{
        PreflightCheck, PreflightCheckStatus, PreflightReport, PreflightTagRead,
        ValidatedTuneRequest, preflight,
    },
};
use serde::Serialize;

use crate::{EXIT_CHECK_FAILED, args::CheckArgs, output::OutputFormat};

#[derive(Serialize)]
struct CheckJson<'a> {
    status: &'static str,
    strict: bool,
    checks: &'a [PreflightCheck],
    tag_reads: &'a [PreflightTagRead],
}

pub(crate) async fn run(
    args: CheckArgs,
    config: &BhtuneConfig,
    database: Option<&SqlitePool>,
    user_templates: Option<&[DcsTemplate]>,
) -> anyhow::Result<ExitCode> {
    let output = args.tune.output;
    let strict = args.strict;
    let report = preflight(
        ValidatedTuneRequest::try_from(args.tune)?,
        config,
        database,
        user_templates,
    )
    .await?;
    print_report(&report, output, strict)?;

    Ok(exit_code(&report, strict))
}

fn exit_code(report: &PreflightReport, strict: bool) -> ExitCode {
    if report.passes(strict) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_CHECK_FAILED)
    }
}

fn print_report(
    report: &PreflightReport,
    output: OutputFormat,
    strict: bool,
) -> anyhow::Result<()> {
    let status = if report.passes(strict) {
        report.status()
    } else {
        PreflightCheckStatus::Fail
    };
    match output {
        OutputFormat::Json => {
            let json = CheckJson {
                status: status.as_str(),
                strict,
                checks: &report.checks,
                tag_reads: &report.tag_reads,
            };
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        OutputFormat::Table => print_table(report, status, strict),
    }
    Ok(())
}

fn print_table(report: &PreflightReport, status: PreflightCheckStatus, strict: bool) {
    println!("Preflight: {}", status.as_str());
    if strict && report.has_warnings() && !report.has_failures() {
        println!("  --strict treats warnings as failures");
    }

    println!();
    println!("{:<6} {:<30} DETAIL", "STATUS", "CHECK");
    for check in &report.checks {
        println!(
            "{:<6} {:<30} {}",
            check.status.as_str(),
            check.name,
            check.detail
        );
    }

    if !report.tag_reads.is_empty() {
        println!();
        println!(
            "{:<6} {:<36} {:<28} {:<10} {:<11} DETAIL",
            "STATUS", "TAG", "ROLES", "VALUE", "QUALITY"
        );
        for read in &report.tag_reads {
            println!(
                "{:<6} {:<36} {:<28} {:<10} {:<11} {}",
                read.status.as_str(),
                read.tag,
                read.roles.join(","),
                read.value.as_deref().unwrap_or("-"),
                read.quality.as_deref().unwrap_or("-"),
                read.detail
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::ExitCode;

    use bhtune_runtime::tune::{
        PreflightCheck, PreflightCheckStatus, PreflightReport, PreflightTagRead,
    };

    use clap::Parser;

    use super::{CheckJson, exit_code, print_report};
    use crate::EXIT_CHECK_FAILED;
    use crate::output::OutputFormat;

    #[test]
    fn strict_mode_changes_warning_summary_to_failure() {
        let report = PreflightReport {
            checks: vec![PreflightCheck {
                name: "compatibility".to_string(),
                status: PreflightCheckStatus::Warn,
                detail: "unknown".to_string(),
            }],
            tag_reads: Vec::new(),
        };
        assert_eq!(exit_code(&report, false), ExitCode::SUCCESS);
        assert_eq!(exit_code(&report, true), ExitCode::from(EXIT_CHECK_FAILED));
    }

    #[tokio::test]
    async fn check_returns_exit_eight_when_a_check_fails() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("missing.db");
        let config = dir.path().join("bhtune.toml");
        std::fs::write(&config, "").unwrap();
        let cli = crate::args::Cli::try_parse_from([
            "bhtune".to_string(),
            "--db".to_string(),
            db.display().to_string(),
            "--config".to_string(),
            config.display().to_string(),
            "check".to_string(),
            "--driver".to_string(),
            "simulator".to_string(),
            "--tagname".to_string(),
            "Sim.Loop1.PV".to_string(),
            "--template".to_string(),
            "Yokogawa CentumVP".to_string(),
            "--process-type".to_string(),
            "flow".to_string(),
            "--controller-type".to_string(),
            "pi".to_string(),
            "--relay-amp".to_string(),
            "0".to_string(),
            "--pv-range-high".to_string(),
            "100".to_string(),
            "--pv-range-low".to_string(),
            "0".to_string(),
            "--mv-range-high".to_string(),
            "100".to_string(),
            "--mv-range-low".to_string(),
            "0".to_string(),
            "--direction".to_string(),
            "reverse".to_string(),
            "--output".to_string(),
            "json".to_string(),
        ])
        .unwrap();

        assert_eq!(
            crate::run_with_cli(cli).await,
            ExitCode::from(EXIT_CHECK_FAILED)
        );
        assert!(!db.exists());
    }

    #[test]
    fn json_report_includes_overall_status_strict_setting_and_check_details() {
        let report = bhtune_runtime::tune::PreflightReport {
            checks: vec![PreflightCheck {
                name: "configuration".to_string(),
                status: PreflightCheckStatus::Pass,
                detail: "valid".to_string(),
            }],
            tag_reads: Vec::new(),
        };
        let output = serde_json::to_string(&CheckJson {
            status: report.status().as_str(),
            strict: false,
            checks: &report.checks,
            tag_reads: &report.tag_reads,
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(value["status"], "pass");
        assert_eq!(value["strict"], false);
        assert_eq!(value["checks"][0]["name"], "configuration");
    }

    #[test]
    fn report_printing_handles_both_formats() {
        let report = PreflightReport {
            checks: vec![PreflightCheck {
                name: "configuration".to_string(),
                status: PreflightCheckStatus::Pass,
                detail: "valid".to_string(),
            }],
            tag_reads: vec![PreflightTagRead {
                tag: "Unit1.LIC101.PV".to_string(),
                roles: vec!["process_variable".to_string()],
                value: None,
                quality: None,
                status: PreflightCheckStatus::Fail,
                detail: "driver returned no value".to_string(),
            }],
        };
        print_report(&report, OutputFormat::Json, false).unwrap();
        print_report(&report, OutputFormat::Table, false).unwrap();

        let warning_report = PreflightReport {
            checks: vec![PreflightCheck {
                name: "compatibility".to_string(),
                status: PreflightCheckStatus::Warn,
                detail: "not verified".to_string(),
            }],
            tag_reads: Vec::new(),
        };
        print_report(&warning_report, OutputFormat::Table, true).unwrap();
    }
}
