//! Shared history operations that can affect a live controller.

use bhtune_core::ResponseLevel;
use bhtune_db::SqlitePool;
use bhtune_db::models::{TuneDriver, TuneRunRow, TuneWriteRow, WriteKind, WriteReadback};
use bhtune_driver::OpcDaDriver;
use thiserror::Error;

use crate::tune::{PidWriteOutcome, write_pid_values};

#[derive(Debug, Error)]
pub enum RevertRunError {
    #[error("{0}")]
    Invalid(anyhow::Error),
    #[error("{0}")]
    Connection(anyhow::Error),
    #[error("{0}")]
    Gateway(anyhow::Error),
    #[error(transparent)]
    Persistence(anyhow::Error),
}

pub struct RevertRequest {
    pub run_id: i64,
    pub bridge_host: Option<String>,
    pub server: Option<String>,
    pub confirmed: bool,
}

pub struct RevertProgress<'a> {
    pub run_id: i64,
    pub loop_name: &'a str,
    pub response_level: ResponseLevel,
    pub target: WriteReadback,
}

pub struct RevertRunResult {
    pub response_level: ResponseLevel,
    pub target: WriteReadback,
    pub outcome: PidWriteOutcome,
}

/// Resolves a revert connection only from the original run's recorded connection. Explicit
/// flags are cross-checks and cannot redirect a revert to another controller or gateway.
pub fn resolve_revert_connection(
    run: &TuneRunRow,
    bridge_host_flag: Option<&str>,
    server_flag: Option<&str>,
) -> anyhow::Result<(String, String)> {
    let stored_server = run.opc_server.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "run {}'s recorded OPC server is missing; refusing to guess which server to \
             revert against",
            run.id
        )
    })?;
    let stored_bridge_host = run.bridge_host.as_deref().ok_or_else(|| {
        anyhow::anyhow!(
            "run {}'s recorded bridge host is missing; refusing to guess which gateway to \
             revert against",
            run.id
        )
    })?;

    if let Some(server_flag) = server_flag
        && server_flag != stored_server
    {
        anyhow::bail!(
            "--server {server_flag:?} contradicts run {}'s recorded OPC server \
             {stored_server:?}; refusing to revert against a different server than the run \
             actually used -- omit --server to use the recorded value",
            run.id
        );
    }
    if let Some(bridge_host_flag) = bridge_host_flag
        && bridge_host_flag != stored_bridge_host
    {
        anyhow::bail!(
            "--bridge-host {bridge_host_flag:?} contradicts run {}'s recorded bridge host \
             {stored_bridge_host:?}; refusing to revert through a different gateway than the \
             run actually used -- omit --bridge-host to use the recorded value",
            run.id
        );
    }

    Ok((stored_bridge_host.to_string(), stored_server.to_string()))
}

/// Restores the pre-write-back PID values recorded for a run. Every eligibility, confirmation,
/// and connection check completes before the live write sequence begins.
pub async fn revert_run(
    pool: &SqlitePool,
    request: RevertRequest,
    allow_uncertain_quality: bool,
) -> Result<RevertRunResult, RevertRunError> {
    revert_run_with_progress(pool, request, allow_uncertain_quality, |_| {}).await
}

/// Revert the recorded PID values and notify an adapter after all safety checks have passed,
/// immediately before the first live write.
pub async fn revert_run_with_progress(
    pool: &SqlitePool,
    request: RevertRequest,
    allow_uncertain_quality: bool,
    on_ready: impl FnOnce(RevertProgress<'_>),
) -> Result<RevertRunResult, RevertRunError> {
    let run_id = request.run_id;
    let run = TuneRunRow::get(pool, run_id)
        .await
        .map_err(|error| RevertRunError::Persistence(error.into()))?
        .ok_or_else(|| RevertRunError::Invalid(anyhow::anyhow!("no run with id {run_id}")))?;

    if run.driver != TuneDriver::Opcda {
        return Err(RevertRunError::Invalid(anyhow::anyhow!(
            "run {run_id} used the {:?} driver, which has no live loop to revert a write \
             against",
            run.driver
        )));
    }

    let writes = TuneWriteRow::list_for_run(pool, run_id)
        .await
        .map_err(|error| RevertRunError::Persistence(error.into()))?;
    let last_write = writes
        .iter()
        .rev()
        .find(|write| write.kind == WriteKind::Write)
        .ok_or_else(|| {
            RevertRunError::Invalid(anyhow::anyhow!(
                "run {run_id} has no recorded PID write-back to revert"
            ))
        })?;
    let response_level = last_write.response_level;
    let target = last_write.previous.ok_or_else(|| {
        RevertRunError::Invalid(anyhow::anyhow!(
            "run {run_id}'s {response_level:?} PID write-back never recorded pre-write \
             values (its pre-read failed at the time); nothing to revert to"
        ))
    })?;

    if !request.confirmed {
        return Err(RevertRunError::Invalid(anyhow::anyhow!(
            "reverting writes PID constants back to a live loop; pass --yes to confirm"
        )));
    }

    let (Some(p_tag), Some(i_tag), Some(d_tag)) = (
        &run.tags.proportional_constant,
        &run.tags.integral_constant,
        &run.tags.derivative_constant,
    ) else {
        return Err(RevertRunError::Invalid(anyhow::anyhow!(
            "run {run_id}'s snapshotted tags have no PID constant tags configured"
        )));
    };

    let (bridge_host, server) = resolve_revert_connection(
        &run,
        request.bridge_host.as_deref(),
        request.server.as_deref(),
    )
    .map_err(RevertRunError::Invalid)?;
    let driver = OpcDaDriver::connect(&bridge_host, &server)
        .await
        .map_err(|error| RevertRunError::Connection(anyhow::Error::new(error)))?;
    let compatibility =
        crate::gateway::require_live_gateway_compatible(&bridge_host, Some(&server))
            .await
            .map_err(RevertRunError::Gateway)?;
    record_gateway_compatibility_if_missing(pool, &run, &compatibility).await?;

    on_ready(RevertProgress {
        run_id,
        loop_name: &run.loop_name,
        response_level,
        target,
    });

    let outcome = write_pid_values(
        pool,
        run_id,
        &driver,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        WriteKind::Revert,
        allow_uncertain_quality,
    )
    .await
    .map_err(RevertRunError::Persistence)?;

    Ok(RevertRunResult {
        response_level,
        target,
        outcome,
    })
}

async fn record_gateway_compatibility_if_missing(
    pool: &SqlitePool,
    run: &TuneRunRow,
    compatibility: &bhtune_driver::OpcDaGatewayCompatibility,
) -> Result<(), RevertRunError> {
    if run.gateway_compatibility_json.is_none() {
        let snapshot = crate::gateway::compatibility_json(compatibility);
        TuneRunRow::record_gateway_compatibility(pool, run.id, &snapshot)
            .await
            .map_err(|error| RevertRunError::Persistence(error.into()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bhtune_core::{ControllerType, LoopConfig, ProcessType};
    use bhtune_db::models::{TemplateOrigin, TuneRunRow};

    fn recorded_run() -> TuneRunRow {
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        TuneRunRow {
            id: 7,
            loop_id: None,
            demo_session_id: None,
            loop_name: "Unit1.LIC101.PV".to_string(),
            driver: TuneDriver::Opcda,
            opc_server: Some("Kepware.KEPServerEX.V6".to_string()),
            bridge_host: Some("127.0.0.1:7600".to_string()),
            started_at: chrono::Utc::now(),
            completed_at: None,
            outcome: bhtune_db::models::TuneOutcome::Running,
            failure_reason: None,
            config: bhtune_core::LoopConfig {
                process_type: bhtune_core::ProcessType::Flow,
                controller_type: bhtune_core::ControllerType::Pi,
                relay_amp_percent: 10.0,
                num_cycles_skip: 1,
                num_cycles_count: 2,
                noise_protection_secs: 3,
                mrft_delay_secs: 0,
            },
            template_origin: bhtune_db::models::TemplateOrigin::Builtin,
            tags,
            template,
            request_json: "{}".to_string(),
            notes: None,
            initial_readings: None,
            allow_uncertain_quality: false,
            timing_metrics: None,
            effective_tuning: None,
            gateway_compatibility_json: None,
            restore_status: None,
            restore_detail: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn revert_connection_uses_the_recorded_values() {
        let run = recorded_run();

        assert_eq!(
            resolve_revert_connection(&run, None, None).unwrap(),
            (
                "127.0.0.1:7600".to_string(),
                "Kepware.KEPServerEX.V6".to_string()
            )
        );
        assert!(
            resolve_revert_connection(&run, Some("127.0.0.1:7600"), Some("Kepware.KEPServerEX.V6"))
                .is_ok()
        );
    }

    #[test]
    fn revert_connection_rejects_missing_or_contradictory_values() {
        let mut run = recorded_run();
        run.bridge_host = None;
        assert!(
            resolve_revert_connection(&run, None, None)
                .unwrap_err()
                .to_string()
                .contains("recorded bridge host is missing")
        );

        let run = recorded_run();
        assert!(
            resolve_revert_connection(&run, None, Some("Different.Server"))
                .unwrap_err()
                .to_string()
                .contains("contradicts")
        );
        assert!(
            resolve_revert_connection(&run, Some("different:7600"), None)
                .unwrap_err()
                .to_string()
                .contains("contradicts")
        );
    }

    #[tokio::test]
    async fn gateway_compatibility_snapshot_is_recorded_only_when_missing() {
        let pool = bhtune_db::connect_in_memory().await.unwrap();
        bhtune_db::seed_builtin_templates(&pool, chrono::Utc::now())
            .await
            .unwrap();
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = bhtune_core::LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let run = TuneRunRow::start(
            &pool,
            None,
            "Unit1.LIC101.PV",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        let compatibility = bhtune_driver::OpcDaGatewayCompatibility::unverified();
        record_gateway_compatibility_if_missing(&pool, &run, &compatibility)
            .await
            .unwrap();

        let expected = crate::gateway::compatibility_json(&compatibility);
        let saved = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(
            saved.gateway_compatibility_json.as_deref(),
            Some(expected.as_str())
        );

        let existing = r#"{"status":"full","source":"original"}"#;
        TuneRunRow::record_gateway_compatibility(&pool, run.id, existing)
            .await
            .unwrap();
        let saved = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        record_gateway_compatibility_if_missing(&pool, &saved, &compatibility)
            .await
            .unwrap();
        let saved = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        assert_eq!(saved.gateway_compatibility_json.as_deref(), Some(existing));
    }
}
