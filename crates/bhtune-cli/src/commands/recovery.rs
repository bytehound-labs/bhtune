use std::{future::Future, process::ExitCode};

use bhtune_runtime::tune::{
    RecoveryStepReport, RecoveryStepStatus, RestoreLoopReport, RestoreLoopRequest, restore_loop,
};

use crate::{EXIT_RESTORE_INCOMPLETE, args::RestoreLoopArgs, cancel::CtrlC, output::OutputFormat};

pub async fn run(
    pool: &bhtune_db::SqlitePool,
    args: RestoreLoopArgs,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<ExitCode> {
    let output = args.output;
    let recovery = restore_loop(
        pool,
        RestoreLoopRequest {
            run_id: args.run_id,
            yes: args.yes,
            bridge_host: args.bridge_host,
            server: args.server,
        },
        ctrl_c,
    );
    run_with_report(recovery, output).await
}

async fn run_with_report<F>(recovery: F, output: OutputFormat) -> anyhow::Result<ExitCode>
where
    F: Future<Output = anyhow::Result<RestoreLoopReport>>,
{
    let report = recovery.await?;
    print_report(&report, output)?;
    match report.status {
        bhtune_db::models::RecoveryAttemptStatus::Confirmed if report.persisted => {
            Ok(ExitCode::SUCCESS)
        }
        _ => {
            eprintln!(
                "WARNING: loop recovery was not confirmed for run {}. Inspect the loop and the per-step evidence manually{}",
                report.run_id,
                report
                    .detail
                    .as_deref()
                    .map(|detail| format!(": {detail}"))
                    .unwrap_or_default()
            );
            Ok(ExitCode::from(EXIT_RESTORE_INCOMPLETE))
        }
    }
}

fn print_report(report: &RestoreLoopReport, output: OutputFormat) -> anyhow::Result<()> {
    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report)?);
        }
        OutputFormat::Table => print_table(report),
    }
    Ok(())
}

fn print_table(report: &RestoreLoopReport) {
    println!("Restore loop run: {}", report.run_id);
    println!("Recovery attempt: {}", report.attempt_id);
    println!("Status: {:?}", report.status);
    println!(
        "Audit persisted: {}",
        if report.persisted { "yes" } else { "no" }
    );
    println!("Evidence export: {}", report.export_path);
    println!();
    println!("{:<18} {:<13} {:<24} DETAIL", "STEP", "STATUS", "TARGET");
    for step in &report.steps {
        print_step(step);
    }
    if let Some(detail) = &report.detail {
        println!();
        println!("Detail: {}", single_line(detail));
    }
}

fn print_step(step: &RecoveryStepReport) {
    println!(
        "{:<18} {:<13} {:<24} {}",
        step.step,
        step_status_label(step.status),
        step.target.as_deref().unwrap_or("-"),
        step.detail.as_deref().map(single_line).unwrap_or_default()
    );
}

fn step_status_label(status: RecoveryStepStatus) -> &'static str {
    match status {
        RecoveryStepStatus::NotNeeded => "not needed",
        RecoveryStepStatus::Succeeded => "succeeded",
        RecoveryStepStatus::Failed => "failed",
    }
}

fn single_line(value: &str) -> String {
    value.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        status: bhtune_db::models::RecoveryAttemptStatus,
        persisted: bool,
    ) -> RestoreLoopReport {
        RestoreLoopReport {
            run_id: 12,
            attempt_id: 34,
            status,
            persisted,
            export_path: "bhtune.db.recovery/evidence.json".to_string(),
            steps: vec![
                RecoveryStepReport {
                    step: "mv".to_string(),
                    status: RecoveryStepStatus::Succeeded,
                    target: Some("50".to_string()),
                    detail: Some("confirmed".to_string()),
                },
                RecoveryStepReport {
                    step: "mode".to_string(),
                    status: RecoveryStepStatus::NotNeeded,
                    target: None,
                    detail: None,
                },
            ],
            detail: Some("first line\nsecond line".to_string()),
        }
    }

    #[test]
    fn step_statuses_have_human_readable_labels() {
        assert_eq!(
            step_status_label(RecoveryStepStatus::NotNeeded),
            "not needed"
        );
        assert_eq!(
            step_status_label(RecoveryStepStatus::Succeeded),
            "succeeded"
        );
        assert_eq!(step_status_label(RecoveryStepStatus::Failed), "failed");
        assert_eq!(single_line("one\ntwo\rthree"), "one two three");
    }

    #[test]
    fn recovery_report_has_one_json_value() {
        let value = report(bhtune_db::models::RecoveryAttemptStatus::Incomplete, false);
        let json = serde_json::to_string(&value).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["run_id"], 12);
        assert_eq!(value["steps"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn run_requires_explicit_confirmation_before_recovery() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        let args = RestoreLoopArgs {
            run_id: 12,
            bridge_host: None,
            server: None,
            yes: false,
            output: OutputFormat::Table,
        };
        let (mut ctrl_c, _handle) = CtrlC::manual();
        let error = run(&pool, args, &mut ctrl_c).await.unwrap_err();
        assert!(error.to_string().contains("requires --yes"));
    }

    #[tokio::test]
    async fn run_report_distinguishes_persisted_confirmation_and_incomplete_recovery() {
        let confirmed = run_with_report(
            async {
                Ok(report(
                    bhtune_db::models::RecoveryAttemptStatus::Confirmed,
                    true,
                ))
            },
            OutputFormat::Table,
        )
        .await
        .unwrap();
        assert_eq!(confirmed, ExitCode::SUCCESS);

        let incomplete = run_with_report(
            async {
                Ok(report(
                    bhtune_db::models::RecoveryAttemptStatus::Incomplete,
                    false,
                ))
            },
            OutputFormat::Json,
        )
        .await
        .unwrap();
        assert_eq!(incomplete, ExitCode::from(EXIT_RESTORE_INCOMPLETE));

        let error = run_with_report(
            async { Err(anyhow::anyhow!("recovery setup failed")) },
            OutputFormat::Table,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("recovery setup failed"));
    }
}
