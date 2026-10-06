use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    time::Duration,
};

use crate::{
    cancel::CtrlC,
    live_ownership::{AuditedDriver, LiveOperationGuard, claim_key, resource_key},
};
use bhtune_db::{
    SqlitePool, database_path,
    models::{
        CompleteRecoveryUpdate, LiveMutationStepRow, LiveOperationKind, LiveOwnershipRow,
        LiveOwnershipState, MutationStepStatus, MvActuationKind, MvActuationStatus,
        NewLiveOwnership, OrphanOwnerUpdate, RecoveryAttemptStatus, RestartRecoveryUpdate,
        TuneDriver, TuneMvActuationRow, TuneOutcome, TuneRecoveryAttemptRow,
        TuneRunInitialReadings, TuneRunRow, UnrecoverableOwnerUpdate,
    },
};
use bhtune_driver::OpcDaDriver;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    actuation::{ActuationAuditPolicy, MvActuationTracker},
    config::{EffectiveTiming, validate_restore_timeout_secs},
    prepare::{InitialState, MutationGuard, validate_initial_state},
    request::{
        DEFAULT_SIM_DEAD_TIME, DEFAULT_SIM_GAIN, DEFAULT_SIM_INITIAL_VALUE, DEFAULT_SIM_NOISE,
        DEFAULT_SIM_SEED, DEFAULT_SIM_TAU, DriverKind, TuneRequest,
    },
    restore::{
        RestoreAttempt, attempt_restore_with_actuation_with_timing_and_audit_policy,
        should_settle_before_auto_release,
    },
};

const ORPHAN_STALE_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq)]
pub struct RestoreLoopRequest {
    pub run_id: i64,
    pub yes: bool,
    pub bridge_host: Option<String>,
    pub server: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStepStatus {
    NotNeeded,
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecoveryStepReport {
    pub step: String,
    pub status: RecoveryStepStatus,
    pub target: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RestoreLoopReport {
    pub run_id: i64,
    pub attempt_id: i64,
    pub status: RecoveryAttemptStatus,
    pub persisted: bool,
    pub export_path: String,
    pub steps: Vec<RecoveryStepReport>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct StartupOrphanSweepReport {
    pub skipped_for_live_owner: bool,
    pub examined: usize,
    pub marked_orphaned: usize,
    pub recoverable: usize,
    pub interrupted_recoveries: usize,
    pub retired_unrecoverable: usize,
    pub exports: Vec<String>,
}

#[derive(Debug, Serialize)]
struct RecoveryExport<'a> {
    version: u32,
    exported_at: DateTime<Utc>,
    reason: &'a str,
    candidate_owner_id: i64,
    run_id: Option<i64>,
    run: Option<&'a TuneRunRow>,
    owners: &'a [LiveOwnershipRow],
    mutation_steps: &'a [LiveMutationStepRow],
    attempts: &'a [TuneRecoveryAttemptRow],
    mv_actuations: &'a [TuneMvActuationRow],
}

/// Full-mode startup records every affected row before retiring a dead live owner. Holding
/// the process-shared database lock distinguishes a killed process from a paused one whose
/// heartbeat may be stale but whose controller operation is still live.
pub async fn recover_startup_orphans(
    pool: &SqlitePool,
) -> anyhow::Result<StartupOrphanSweepReport> {
    let Some(file_guard) = LiveOperationGuard::try_acquire_database_file(pool).await? else {
        tracing::warn!(
            "skipping live-orphan startup sweep because another process holds the controller ownership lock"
        );
        return Ok(StartupOrphanSweepReport {
            skipped_for_live_owner: true,
            ..StartupOrphanSweepReport::default()
        });
    };

    let stale_before = Utc::now()
        - chrono::Duration::from_std(ORPHAN_STALE_AFTER)
            .map_err(|error| anyhow::anyhow!("invalid orphan heartbeat age: {error}"))?;
    let candidates = LiveOwnershipRow::stale_active_owners(pool, stale_before).await?;
    let db_path = database_path(pool).await?;
    let mut report = StartupOrphanSweepReport {
        examined: candidates.len(),
        ..StartupOrphanSweepReport::default()
    };

    for owner in candidates {
        let recorded_claim_key = claim_key(&owner.database_key, &owner.resource_key)?;
        if owner.database_key != file_guard.database_key()
            || !LiveOwnershipRow::has_claim(pool, owner.id, &recorded_claim_key).await?
        {
            anyhow::bail!(
                "stale owner {} has a mismatched database identity or claim; refusing to alter its ownership record",
                owner.id
            );
        }

        if owner.operation_kind == LiveOperationKind::Recovery {
            let export_path = abandon_interrupted_recovery(
                pool,
                &db_path,
                &owner,
                stale_before,
                &recorded_claim_key,
            )
            .await?;
            report.interrupted_recoveries += 1;
            report.exports.push(export_path);
            continue;
        }

        let run = match owner.run_id {
            Some(run_id) => TuneRunRow::get(pool, run_id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!("stale owner {} references missing run {run_id}", owner.id)
                })?
                .into(),
            None => None,
        };
        match run.as_ref() {
            Some(run)
                if owner.operation_kind == LiveOperationKind::Tune
                    && run.driver == TuneDriver::Opcda
                    && run.outcome == TuneOutcome::Running =>
            {
                let (export_path, eligible) = mark_stale_tune_orphan(
                    pool,
                    &db_path,
                    run,
                    &owner,
                    stale_before,
                    &recorded_claim_key,
                )
                .await?;
                report.marked_orphaned += 1;
                report.recoverable += usize::from(eligible);
                report.exports.push(export_path);
            }
            run => {
                let export_path = retire_stale_unrecoverable_owner(
                    pool,
                    &db_path,
                    &owner,
                    run,
                    stale_before,
                    &recorded_claim_key,
                )
                .await?;
                report.retired_unrecoverable += 1;
                report.exports.push(export_path);
            }
        }
    }
    Ok(report)
}

async fn retire_stale_unrecoverable_owner(
    pool: &SqlitePool,
    db_path: &Path,
    owner: &LiveOwnershipRow,
    run: Option<&TuneRunRow>,
    stale_before: DateTime<Utc>,
    recorded_claim_key: &str,
) -> anyhow::Result<String> {
    let export_path = if let Some(run) = run {
        export_run_evidence(pool, db_path, run, owner.id, "stale_owner_retirement").await?
    } else {
        export_owner_evidence(pool, db_path, owner, "stale_owner_retirement").await?
    };
    let evidence = serde_json::to_string(&json!({
        "version": 1,
        "event": "stale_owner_retirement",
        "owner_id": owner.id,
        "run_id": owner.run_id,
        "operation_kind": owner.operation_kind,
        "database_key": owner.database_key,
        "resource_key": owner.resource_key,
        "export_path": export_path,
        "recovery_eligible": false,
        "eligibility_detail": "the owner is stale but its operation/run state does not qualify for automated live-loop recovery",
        "heartbeat_at": owner.heartbeat_at,
        "stale_before": stale_before,
        "observed_at": Utc::now(),
        "controller_contacted": false,
    }))?;
    let detail = "a live-operation owner became stale without sufficient evidence for automatic recovery; inspect the exported evidence and verify the controller manually";
    let fail_running_run_id = run
        .filter(|run| run.outcome == TuneOutcome::Running)
        .map(|run| run.id);
    let retired = LiveOwnershipRow::mark_orphaned_unrecoverable(
        pool,
        UnrecoverableOwnerUpdate {
            owner_id: owner.id,
            expected_heartbeat: owner.heartbeat_at,
            expected_claim_key: recorded_claim_key,
            now: Utc::now(),
            evidence_json: &evidence,
            fail_running_run_id,
            detail,
        },
    )
    .await?;
    if !retired {
        anyhow::bail!(
            "stale owner {} changed after its evidence was exported to {export_path}; no ownership rows were changed",
            owner.id
        );
    }
    Ok(export_path)
}

async fn mark_stale_tune_orphan(
    pool: &SqlitePool,
    db_path: &Path,
    run: &TuneRunRow,
    owner: &LiveOwnershipRow,
    stale_before: DateTime<Utc>,
    recorded_claim_key: &str,
) -> anyhow::Result<(String, bool)> {
    let run_id = run.id;
    let owners = LiveOwnershipRow::list_for_run(pool, run_id).await?;
    let mutation_steps = LiveMutationStepRow::list_for_run(pool, run_id).await?;
    let attempts = TuneRecoveryAttemptRow::list_for_run(pool, run_id).await?;
    let owner_steps: Vec<_> = mutation_steps
        .iter()
        .filter(|step| step.owner_id == owner.id)
        .cloned()
        .collect();
    let (mut eligible, mut eligibility_detail) = recovery_eligibility(run, owner, &owner_steps);
    let resource_matches = match (run.bridge_host.as_deref(), run.opc_server.as_deref()) {
        (Some(host), Some(server)) => resource_key(host, server, &run.tags.manipulated_variable)
            .is_ok_and(|expected| expected == owner.resource_key),
        _ => false,
    };
    if eligible && !resource_matches {
        eligible = false;
        eligibility_detail =
            "recorded database/controller/MV identity does not match the owner's canonical resource key"
                .to_string();
    }

    let mv_actuations = TuneMvActuationRow::list_for_run(pool, run_id).await?;
    let export_path = write_recovery_export(
        db_path,
        &RecoveryExport {
            version: 1,
            exported_at: Utc::now(),
            reason: "full_mode_startup_orphan_sweep",
            candidate_owner_id: owner.id,
            run_id: Some(run_id),
            run: Some(run),
            owners: &owners,
            mutation_steps: &mutation_steps,
            attempts: &attempts,
            mv_actuations: &mv_actuations,
        },
    )?;
    let evidence = serde_json::to_string(&json!({
        "version": 1,
        "event": "startup_orphan_sweep",
        "run_id": run_id,
        "owner_id": owner.id,
        "database_key": owner.database_key,
        "resource_key": owner.resource_key,
        "resource_matches_run": resource_matches,
        "export_path": export_path,
        "recovery_eligible": eligible,
        "eligibility_detail": eligibility_detail,
        "heartbeat_at": owner.heartbeat_at,
        "stale_before": stale_before,
        "observed_at": Utc::now(),
        "controller_contacted": false,
    }))?;
    let detail = if eligible {
        "the live tune process ended with a durably recorded restore intent and controller mutation; use `bhtune restore-loop --yes` after reviewing the exported evidence"
    } else {
        "the live tune process ended, but its persisted ownership or mutation evidence is insufficient for automated recovery; inspect the exported evidence and restore the loop manually"
    };
    let marked = LiveOwnershipRow::mark_orphaned(
        pool,
        OrphanOwnerUpdate {
            owner_id: owner.id,
            expected_heartbeat: owner.heartbeat_at,
            expected_claim_key: recorded_claim_key,
            now: Utc::now(),
            evidence_json: &evidence,
            eligible,
            detail,
        },
    )
    .await?;
    if !marked {
        anyhow::bail!(
            "orphan owner {} changed after its evidence was exported to {export_path}; no ownership rows were changed",
            owner.id
        );
    }
    Ok((export_path, eligible))
}

async fn abandon_interrupted_recovery(
    pool: &SqlitePool,
    db_path: &Path,
    owner: &LiveOwnershipRow,
    stale_before: DateTime<Utc>,
    expected_claim_key: &str,
) -> anyhow::Result<String> {
    let run_id = owner.run_id.ok_or_else(|| {
        anyhow::anyhow!(
            "stale recovery owner {} has no run ID; refusing to alter its claim",
            owner.id
        )
    })?;
    let run = TuneRunRow::get(pool, run_id).await?.ok_or_else(|| {
        anyhow::anyhow!(
            "stale recovery owner {} references missing run {run_id}",
            owner.id
        )
    })?;
    if owner.operation_kind != LiveOperationKind::Recovery
        || run.outcome != TuneOutcome::Failed
        || run.recovery_state != Some(bhtune_db::models::TuneRecoveryState::Running)
    {
        anyhow::bail!(
            "stale recovery owner {} does not match a failed run with a running recovery attempt; refusing to guess how to release its claim",
            owner.id
        );
    }
    let source_owners: Vec<_> = LiveOwnershipRow::list_for_run(pool, run_id)
        .await?
        .into_iter()
        .filter(|source| {
            source.operation_kind == LiveOperationKind::Tune
                && source.run_id == Some(run_id)
                && source.state == LiveOwnershipState::Orphaned
                && source.orphan_eligible
        })
        .collect();
    let source_owner = match source_owners.as_slice() {
        [source_owner] => source_owner.clone(),
        _ => anyhow::bail!(
            "stale recovery owner {} has {} eligible source owners; exactly one is required",
            owner.id,
            source_owners.len()
        ),
    };
    let expected_resource = match (run.bridge_host.as_deref(), run.opc_server.as_deref()) {
        (Some(host), Some(server)) => resource_key(host, server, &run.tags.manipulated_variable)?,
        _ => anyhow::bail!(
            "run {run_id} has incomplete recorded connection provenance; stale recovery ownership is not safe to release automatically"
        ),
    };
    if source_owner.run_id != Some(run_id)
        || source_owner.database_key != owner.database_key
        || source_owner.resource_key != owner.resource_key
        || owner.resource_key != expected_resource
        || !LiveOwnershipRow::has_claim(pool, owner.id, expected_claim_key).await?
    {
        anyhow::bail!(
            "stale recovery owner {} does not match its source owner, recorded controller/MV, or claim; refusing to alter ownership",
            owner.id
        );
    }
    let attempts = TuneRecoveryAttemptRow::list_for_run(pool, run_id).await?;
    let mut running_attempts = attempts.iter().filter(|attempt| {
        attempt.recovery_owner_id == owner.id && attempt.status == RecoveryAttemptStatus::Running
    });
    let attempt = running_attempts.next().ok_or_else(|| {
        anyhow::anyhow!(
            "stale recovery owner {} has no matching running recovery attempt",
            owner.id
        )
    })?;
    let export_path = export_run_evidence(
        pool,
        db_path,
        &run,
        owner.id,
        "interrupted_recovery_restart",
    )
    .await?;
    let detail = format!(
        "the recovery process ended without a durable final audit; startup contacted no controller. Review {export_path}, then retry with `bhtune restore-loop --yes`"
    );
    let evidence = serde_json::to_string(&json!({
        "version": 1,
        "event": "interrupted_recovery_restart",
        "run_id": run_id,
        "source_owner_id": source_owner.id,
        "recovery_owner_id": owner.id,
        "attempt_id": attempt.id,
        "database_key": owner.database_key,
        "resource_key": owner.resource_key,
        "export_path": export_path,
        "heartbeat_at": owner.heartbeat_at,
        "stale_before": stale_before,
        "observed_at": Utc::now(),
        "controller_contacted": false,
        "recovery_eligible": true,
        "detail": detail,
    }))?;
    if !TuneRecoveryAttemptRow::abandon_after_restart(
        pool,
        RestartRecoveryUpdate {
            attempt_id: attempt.id,
            recovery_owner_id: owner.id,
            expected_heartbeat: owner.heartbeat_at,
            source_owner_id: source_owner.id,
            run_id,
            claim_key: expected_claim_key,
            now: Utc::now(),
            detail: &detail,
            evidence_json: &evidence,
        },
    )
    .await?
    {
        anyhow::bail!(
            "stale recovery owner {} changed while its restart evidence was being exported; no ownership rows were changed",
            owner.id
        );
    }
    Ok(export_path)
}

/// Restores an interrupted live run only after its original database/resource owner is
/// atomically replaced and a durable evidence export exists.
pub async fn restore_loop(
    pool: &SqlitePool,
    request: RestoreLoopRequest,
    ctrl_c: &mut CtrlC,
) -> anyhow::Result<RestoreLoopReport> {
    anyhow::ensure!(
        request.yes,
        "restore-loop requires --yes; no controller connection or mutation was attempted"
    );
    load_recovery_run(pool, request.run_id).await?;
    let Some(file_guard) = LiveOperationGuard::try_acquire_database_file(pool).await? else {
        anyhow::bail!(
            "another live controller operation holds the database ownership lock; recovery is blocked"
        );
    };

    let run = load_recovery_run(pool, request.run_id).await?;
    let (bridge_host, server) = crate::history::resolve_revert_connection(
        &run,
        request.bridge_host.as_deref(),
        request.server.as_deref(),
    )?;
    let source_owner = recovery_source_owner(pool, &run).await?;
    let source_steps = LiveMutationStepRow::list_for_owner(pool, source_owner.id).await?;
    validate_recovery_run(&run, &source_owner, &source_steps)?;
    let effective_timing = recovery_timing(&run)?;
    let initial = initial_state(&run)?;
    let guard = mutation_guard(&run, &source_steps);
    let args = recovery_request(&run, &bridge_host, &server, effective_timing, &initial);
    let mut mv_actuations = Some(recovery_mv_tracker(pool, &run, &initial, &args).await?);
    let current_db_key = file_guard.database_key().to_owned();
    if source_owner.database_key != current_db_key {
        anyhow::bail!(
            "run {}'s recorded database ownership identity does not match the opened database; recovery is blocked",
            run.id
        );
    }
    let expected_resource_key =
        resource_key(&bridge_host, &server, &run.tags.manipulated_variable)?;
    if source_owner.resource_key != expected_resource_key {
        anyhow::bail!(
            "run {}'s recorded controller/MV ownership identity does not match its snapshots; recovery is blocked",
            run.id
        );
    }
    let claim_key = claim_key(&current_db_key, &expected_resource_key)?;
    if !LiveOwnershipRow::has_claim(pool, source_owner.id, &claim_key).await? {
        anyhow::bail!(
            "run {} no longer owns the canonical controller/MV recovery claim; recovery is blocked",
            run.id
        );
    }
    let db_path = database_path(pool).await?;
    let export_path = export_run_evidence(
        pool,
        &db_path,
        &run,
        source_owner.id,
        "restore_loop_attempt",
    )
    .await?;
    let now = Utc::now();
    let attempt_evidence = serde_json::to_string(&json!({
        "version": 1,
        "event": "restore_loop_attempt",
        "run_id": run.id,
        "source_owner_id": source_owner.id,
        "database_key": current_db_key,
        "resource_key": expected_resource_key,
        "recorded_bridge_host": bridge_host,
        "recorded_server": server,
        "export_path": export_path,
        "started_at": now,
        "controller_mutation_attempted": false,
    }))?;
    let new_owner = NewLiveOwnership {
        run_id: Some(run.id),
        operation_kind: LiveOperationKind::Recovery,
        database_key: current_db_key,
        resource_key: expected_resource_key,
        owner_pid: i64::from(std::process::id()),
        acquired_at: now,
        restore_intent_json: source_owner.restore_intent_json.clone(),
    };
    let handoff_result = match TuneRecoveryAttemptRow::start_with_claim(
        pool,
        new_owner,
        &claim_key,
        source_owner.id,
        run.id,
        &export_path,
        &attempt_evidence,
    )
    .await?
    {
        Some(ownership) => Ok(ownership),
        None => Err(anyhow::anyhow!(
            "run {} lost its orphan recovery claim to another contender; no controller connection was attempted",
            run.id
        )),
    };
    let (recovery_owner, attempt) = handoff_result?;
    let mut ownership =
        LiveOperationGuard::from_database_owner(pool, file_guard, recovery_owner, claim_key);

    let finalization_result = match TuneMvActuationRow::finalize_pending_for_run(
        pool,
        run.id,
        MvActuationStatus::Unverified,
        Some("an interrupted tune left this accepted MV command unverified; a restore-loop attempt is taking over"),
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            let detail =
                format!("could not finalize an interrupted MV audit row before recovery: {error}");
            finish_recovery_setup_failure(
                pool,
                &mut ownership,
                &source_owner,
                RecoverySetupFailure {
                    run_id: run.id,
                    attempt_id: attempt.id,
                    status: RecoveryAttemptStatus::Failed,
                    detail: &detail,
                    evidence_json: &attempt_evidence,
                },
            )
            .await?;
            Err(anyhow::anyhow!("{detail}"))
        }
    };
    finalization_result?;

    let driver_result = match OpcDaDriver::connect(&bridge_host, &server).await {
        Ok(driver) => Ok(driver),
        Err(error) => {
            let detail = format!(
                "could not connect to the run's recorded OPC DA gateway/server; no restore write was attempted: {error}"
            );
            finish_recovery_setup_failure(
                pool,
                &mut ownership,
                &source_owner,
                RecoverySetupFailure {
                    run_id: run.id,
                    attempt_id: attempt.id,
                    status: RecoveryAttemptStatus::Failed,
                    detail: &detail,
                    evidence_json: &attempt_evidence,
                },
            )
            .await?;
            Err(anyhow::anyhow!("{detail}"))
        }
    };
    let driver = driver_result?;
    let compatibility_result = match crate::gateway::require_live_gateway_compatible(
        &bridge_host,
        Some(&server),
    )
    .await
    {
        Ok(report) => Ok(report),
        Err(error) => {
            let detail = format!(
                "the recorded gateway/server failed the live compatibility check; no restore write was attempted: {error}"
            );
            finish_recovery_setup_failure(
                pool,
                &mut ownership,
                &source_owner,
                RecoverySetupFailure {
                    run_id: run.id,
                    attempt_id: attempt.id,
                    status: RecoveryAttemptStatus::Failed,
                    detail: &detail,
                    evidence_json: &attempt_evidence,
                },
            )
            .await?;
            Err(anyhow::anyhow!("{detail}"))
        }
    };
    let compatibility = compatibility_result?;
    if run.gateway_compatibility_json.is_none() {
        let compatibility_json = crate::gateway::compatibility_json(&compatibility);
        let persistence_result =
            TuneRunRow::record_gateway_compatibility(pool, run.id, &compatibility_json).await;
        let persistence_result = match persistence_result {
            Ok(_) => Ok(()),
            Err(error) => {
                let detail = format!(
                    "could not persist gateway compatibility evidence before restoration: {error}"
                );
                finish_recovery_setup_failure(
                    pool,
                    &mut ownership,
                    &source_owner,
                    RecoverySetupFailure {
                        run_id: run.id,
                        attempt_id: attempt.id,
                        status: RecoveryAttemptStatus::Failed,
                        detail: &detail,
                        evidence_json: &attempt_evidence,
                    },
                )
                .await?;
                Err(anyhow::anyhow!("{detail}"))
            }
        };
        persistence_result?;
    }

    let audited = AuditedDriver::new(&driver, pool, &ownership, Some(run.id));
    let restore_attempt = attempt_restore_with_actuation_with_timing_and_audit_policy(
        pool,
        run.id,
        &args,
        effective_timing,
        &audited,
        &run.tags,
        &run.template,
        &initial,
        &guard,
        run.allow_uncertain_quality,
        ctrl_c,
        &mut mv_actuations,
        None,
        run.timing_metrics
            .and_then(|metrics| metrics.measured_oscillation_period_ms),
        None,
        ActuationAuditPolicy::Required,
    )
    .await;
    let mut detail = match &restore_attempt {
        RestoreAttempt::Confirmed => None,
        RestoreAttempt::Incomplete { reason } => Some(reason.clone()),
    };
    append_error_detail(&mut detail, ownership.ensure_healthy());

    let (step_reports, audit_error, controller_mutation_attempted) =
        recovery_step_reports(pool, &run, &initial, &guard, ownership.owner().id, &args).await;
    let audit_passed = audit_error.is_none();
    append_optional_detail(&mut detail, audit_error);
    let all_steps_confirmed = step_reports.iter().all(|step| {
        matches!(
            step.status,
            RecoveryStepStatus::Succeeded | RecoveryStepStatus::NotNeeded
        )
    });
    let status = if matches!(restore_attempt, RestoreAttempt::Confirmed)
        && ownership.ensure_healthy().is_ok()
        && audit_passed
        && all_steps_confirmed
    {
        RecoveryAttemptStatus::Confirmed
    } else {
        RecoveryAttemptStatus::Incomplete
    };
    let evidence = serde_json::to_string(&json!({
        "version": 1,
        "event": "restore_loop_result",
        "run_id": run.id,
        "source_owner_id": source_owner.id,
        "recovery_owner_id": ownership.owner().id,
        "attempt_id": attempt.id,
        "export_path": export_path,
        "status": status,
        "steps": step_reports,
        "detail": detail,
        "completed_at": Utc::now(),
        "controller_mutation_attempted": controller_mutation_attempted,
    }))?;

    ownership.stop_heartbeat().await;
    let persisted = TuneRecoveryAttemptRow::complete_with_ownership(
        pool,
        CompleteRecoveryUpdate {
            attempt_id: attempt.id,
            recovery_owner_id: ownership.owner().id,
            source_owner_id: source_owner.id,
            run_id: run.id,
            claim_key: ownership.claim_key(),
            status,
            now: Utc::now(),
            detail: detail.as_deref(),
            evidence_json: &evidence,
        },
    )
    .await;
    drop(ownership);
    let persisted = match persisted {
        Ok(()) => true,
        Err(error) => {
            detail = Some(final_audit_failure_detail(detail.as_deref(), &error));
            false
        }
    };

    Ok(RestoreLoopReport {
        run_id: run.id,
        attempt_id: attempt.id,
        status: if persisted {
            status
        } else {
            RecoveryAttemptStatus::Incomplete
        },
        persisted,
        export_path,
        steps: step_reports,
        detail,
    })
}

struct RecoverySetupFailure<'a> {
    run_id: i64,
    attempt_id: i64,
    status: RecoveryAttemptStatus,
    detail: &'a str,
    evidence_json: &'a str,
}

async fn finish_recovery_setup_failure(
    pool: &SqlitePool,
    ownership: &mut LiveOperationGuard,
    source_owner: &LiveOwnershipRow,
    failure: RecoverySetupFailure<'_>,
) -> anyhow::Result<()> {
    ownership.stop_heartbeat().await;
    TuneRecoveryAttemptRow::complete_with_ownership(
        pool,
        CompleteRecoveryUpdate {
            attempt_id: failure.attempt_id,
            recovery_owner_id: ownership.owner().id,
            source_owner_id: source_owner.id,
            run_id: failure.run_id,
            claim_key: ownership.claim_key(),
            status: failure.status,
            now: Utc::now(),
            detail: Some(failure.detail),
            evidence_json: failure.evidence_json,
        },
    )
    .await?;
    Ok(())
}

async fn load_recovery_run(pool: &SqlitePool, run_id: i64) -> anyhow::Result<TuneRunRow> {
    let run = TuneRunRow::get(pool, run_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no run with id {run_id}"))?;
    if run.driver != TuneDriver::Opcda {
        anyhow::bail!(
            "run {run_id} used the {:?} driver; only a live OPC DA tune can be restored",
            run.driver
        );
    }
    let recovery_state_is_eligible = matches!(
        run.recovery_state,
        Some(
            bhtune_db::models::TuneRecoveryState::Eligible
                | bhtune_db::models::TuneRecoveryState::Incomplete
        )
    );
    if run.outcome != TuneOutcome::Failed || !recovery_state_is_eligible {
        anyhow::bail!(
            "run {run_id} is not an explicitly eligible orphaned run; legacy and active rows fail closed"
        );
    }
    Ok(run)
}

async fn recovery_source_owner(
    pool: &SqlitePool,
    run: &TuneRunRow,
) -> anyhow::Result<LiveOwnershipRow> {
    let owners = LiveOwnershipRow::list_for_run(pool, run.id).await?;
    let mut eligible: Vec<_> = owners
        .into_iter()
        .filter(|owner| {
            owner.operation_kind == LiveOperationKind::Tune
                && owner.run_id == Some(run.id)
                && owner.state == LiveOwnershipState::Orphaned
                && owner.orphan_eligible
        })
        .collect();
    if eligible.len() != 1 {
        anyhow::bail!(
            "run {} has {} eligible orphan ownership records; exactly one is required",
            run.id,
            eligible.len()
        );
    }
    Ok(eligible.remove(0))
}

fn validate_recovery_run(
    run: &TuneRunRow,
    source_owner: &LiveOwnershipRow,
    source_steps: &[LiveMutationStepRow],
) -> anyhow::Result<()> {
    if !source_owner.orphan_eligible
        || source_owner.state != LiveOwnershipState::Orphaned
        || source_owner.operation_kind != LiveOperationKind::Tune
        || source_owner.run_id != Some(run.id)
    {
        anyhow::bail!(
            "run {} has no matching eligible orphaned tune owner; recovery is blocked",
            run.id
        );
    }
    let (eligible, detail) = recovery_eligibility(run, source_owner, source_steps);
    if !eligible {
        anyhow::bail!(
            "run {} lacks complete structured restore evidence ({detail}); use operator-guided manual restore",
            run.id
        );
    }
    Ok(())
}

fn recovery_eligibility(
    run: &TuneRunRow,
    owner: &LiveOwnershipRow,
    steps: &[LiveMutationStepRow],
) -> (bool, String) {
    if run.driver != TuneDriver::Opcda
        || (run.outcome != TuneOutcome::Running
            && !(run.outcome == TuneOutcome::Failed
                && matches!(
                    run.recovery_state,
                    Some(
                        bhtune_db::models::TuneRecoveryState::Eligible
                            | bhtune_db::models::TuneRecoveryState::Incomplete
                    )
                )))
        || owner.operation_kind != LiveOperationKind::Tune
        || owner.run_id != Some(run.id)
    {
        return (
            false,
            "run and owner provenance does not identify a running OPC DA tune".to_string(),
        );
    }
    if run.bridge_host.as_deref().is_none_or(str::is_empty)
        || run.opc_server.as_deref().is_none_or(str::is_empty)
    {
        return (
            false,
            "recorded bridge host or OPC server is missing".to_string(),
        );
    }
    let Some(initial) = run.initial_readings.as_ref() else {
        return (false, "initial controller readings are missing".to_string());
    };
    let initial_setpoint_is_required = run.template.revert_mode
        && run.tags.controller_mode.is_some()
        && initial.mode_raw.as_deref() == Some(run.template.mode_auto_value.as_str())
        && run.tags.setpoint_variable.is_some();
    if initial_setpoint_is_required && initial.setpoint_ini.is_none() {
        return (
            false,
            "the configured setpoint tag has no recorded initial value".to_string(),
        );
    }
    let Some(intent_json) = owner.restore_intent_json.as_deref() else {
        return (false, "durable restore intent is missing".to_string());
    };
    if let Err(error) = validate_restore_intent(run, intent_json, initial) {
        return (false, error.to_string());
    }
    let Some(_) = run.effective_tuning else {
        return (
            false,
            "recorded restore timing policy is missing".to_string(),
        );
    };
    if let Err(error) = recovery_timing(run) {
        return (
            false,
            format!("recorded restore timing policy is invalid: {error}"),
        );
    }
    if let Err(error) = run.config.validate() {
        return (
            false,
            format!("recorded loop configuration is invalid: {error}"),
        );
    }
    let initial_state = initial_state_from_readings(initial);
    if validate_initial_state(&initial_state).is_err()
        || !initial_state.pv_ini.is_finite()
        || initial_state
            .setpoint_ini
            .is_some_and(|value| !value.is_finite())
        || (run.tags.controller_mode.is_some() && initial_state.mode_raw.is_none())
        || (run.tags.mode_attribute.is_some() && initial_state.mode_attribute_raw.is_none())
    {
        return (
            false,
            "persisted initial readings fail range, finiteness, or tagged-mode validation"
                .to_string(),
        );
    }
    let allowed_tags = [
        Some(run.tags.manipulated_variable.as_str()),
        run.tags.controller_mode.as_deref(),
        run.tags.mode_attribute.as_deref(),
        run.tags.setpoint_variable.as_deref(),
    ];
    let mut mutation_seen = false;
    for step in steps.iter().filter(|step| step.step == "controller_write") {
        if step.status == MutationStepStatus::NotNeeded {
            continue;
        }
        let Some((tag, _)) = parse_write_target(&step.target_json) else {
            return (
                false,
                "a controller-write audit target is malformed".to_string(),
            );
        };
        if allowed_tags.iter().flatten().any(|allowed| *allowed == tag) {
            mutation_seen = true;
        }
    }
    if !mutation_seen {
        return (
            false,
            "no durable audit records a possible mutation to the loop's MV, mode, setpoint, or mode attribute".to_string(),
        );
    }
    (
        true,
        "complete structured restore and mutation evidence is present".to_string(),
    )
}

fn validate_restore_intent(
    run: &TuneRunRow,
    intent_json: &str,
    initial: &TuneRunInitialReadings,
) -> anyhow::Result<()> {
    let intent: Value = serde_json::from_str(intent_json)
        .map_err(|_| anyhow::anyhow!("persisted restore intent is not valid JSON"))?;
    let initial_json = serde_json::to_value(initial)?;
    let expected_policy = json!({
        "revert_mode": run.template.revert_mode,
        "mode_auto_value": run.template.mode_auto_value,
        "mode_manual_value": run.template.mode_manual_value,
        "mode_attribute_program_value": run.template.mode_attribute_program_value,
    });
    if intent.get("version").and_then(Value::as_u64) != Some(1)
        || intent.get("kind").and_then(Value::as_str) != Some("tune_restore")
        || intent.get("run_id").and_then(Value::as_i64) != Some(run.id)
        || intent.get("state").and_then(Value::as_str) != Some("ready_to_restore")
        || intent.get("mv_tag").and_then(Value::as_str)
            != Some(run.tags.manipulated_variable.as_str())
        || intent.get("initial_readings") != Some(&initial_json)
        || intent.get("template_policy") != Some(&expected_policy)
    {
        anyhow::bail!(
            "durable restore intent does not exactly match the run's recorded readings, tags, template policy, and version"
        );
    }
    Ok(())
}

fn initial_state(run: &TuneRunRow) -> anyhow::Result<InitialState> {
    let initial = run
        .initial_readings
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("run {} has no recorded initial readings", run.id))?;
    Ok(initial_state_from_readings(initial))
}

fn initial_state_from_readings(initial: &TuneRunInitialReadings) -> InitialState {
    InitialState {
        pv_ini: initial.pv_ini,
        mv_ini: initial.mv_ini,
        pv_range_high: initial.pv_range_high,
        pv_range_low: initial.pv_range_low,
        mv_range_high: initial.mv_range_high,
        mv_range_low: initial.mv_range_low,
        direction: initial.controller_direction,
        mode_raw: initial.mode_raw.clone(),
        mode_attribute_raw: initial.mode_attribute_raw.clone(),
        setpoint_ini: initial.setpoint_ini,
    }
}

fn append_error_detail(detail: &mut Option<String>, result: anyhow::Result<()>) {
    if let Err(error) = result {
        append_optional_detail(detail, Some(error.to_string()));
    }
}

fn append_optional_detail(detail: &mut Option<String>, additional: Option<String>) {
    if let Some(additional) = additional {
        *detail = Some(match detail.take() {
            Some(existing) => format!("{existing}; {additional}"),
            None => additional,
        });
    }
}

fn final_audit_failure_detail(existing: Option<&str>, error: impl std::fmt::Display) -> String {
    match existing {
        Some(existing) => format!(
            "{existing}; failed to persist the final recovery audit: {error}; inspect the loop manually"
        ),
        None => format!(
            "failed to persist the final recovery audit: {error}; the restore is not confirmed and the loop must be inspected manually"
        ),
    }
}

fn mutation_guard(run: &TuneRunRow, steps: &[LiveMutationStepRow]) -> MutationGuard {
    let was_written = |tag: Option<&str>, expected: Option<Value>| {
        let Some(tag) = tag else {
            return false;
        };
        steps.iter().any(|step| {
            if step.step != "controller_write" || step.status == MutationStepStatus::NotNeeded {
                return false;
            }
            let Some((written_tag, written_value)) = parse_write_target(&step.target_json) else {
                return false;
            };
            written_tag == tag
                && expected
                    .as_ref()
                    .is_none_or(|value| value == &written_value)
        })
    };
    MutationGuard {
        mode_attribute_written: was_written(
            run.tags.mode_attribute.as_deref(),
            run.template
                .mode_attribute_program_value
                .as_ref()
                .map(|value| Value::String(value.clone())),
        ),
        mode_written: was_written(
            run.tags.controller_mode.as_deref(),
            Some(Value::String(run.template.mode_manual_value.clone())),
        ),
        mv_written: was_written(Some(run.tags.manipulated_variable.as_str()), None),
    }
}

fn parse_write_target(target_json: &str) -> Option<(String, Value)> {
    let target: Value = serde_json::from_str(target_json).ok()?;
    Some((
        target.get("tag")?.as_str()?.to_string(),
        target.get("value")?.clone(),
    ))
}

fn recovery_timing(run: &TuneRunRow) -> anyhow::Result<EffectiveTiming> {
    let tuning = run
        .effective_tuning
        .ok_or_else(|| anyhow::anyhow!("run {} has no saved timing policy", run.id))?;
    if tuning.poll_interval_ms == 0 || tuning.op_timeout_secs == 0 {
        anyhow::bail!(
            "run {} has invalid saved poll/operation timing; recovery is blocked",
            run.id
        );
    }
    validate_restore_timeout_secs(DriverKind::Opcda, tuning.restore_timeout_secs)?;
    Ok(EffectiveTiming {
        mrft_delay_secs: tuning.mrft_delay_secs,
        poll_interval_ms: tuning.poll_interval_ms,
        timeout_secs: tuning.timeout_secs,
        op_timeout_secs: tuning.op_timeout_secs,
        restore_timeout_secs: tuning.restore_timeout_secs,
    })
}

fn recovery_request(
    run: &TuneRunRow,
    bridge_host: &str,
    server: &str,
    timing: EffectiveTiming,
    initial: &InitialState,
) -> TuneRequest {
    let _ = timing;
    TuneRequest {
        tagname: run.loop_name.clone(),
        template: run.template.name.clone(),
        process_type: run.config.process_type,
        controller_type: run.config.controller_type,
        relay_amp: run.config.relay_amp_percent,
        cycles_skip: Some(run.config.num_cycles_skip),
        cycles_count: Some(run.config.num_cycles_count),
        noise_protection_secs: Some(run.config.noise_protection_secs),
        driver: DriverKind::Opcda,
        bridge_host: Some(bridge_host.to_string()),
        server: Some(server.to_string()),
        sim_gain: DEFAULT_SIM_GAIN,
        sim_tau: DEFAULT_SIM_TAU,
        sim_dead_time: DEFAULT_SIM_DEAD_TIME,
        sim_noise: DEFAULT_SIM_NOISE,
        sim_seed: DEFAULT_SIM_SEED,
        sim_initial_pv: DEFAULT_SIM_INITIAL_VALUE,
        sim_initial_mv: DEFAULT_SIM_INITIAL_VALUE,
        pv_range_high: None,
        pv_range_low: None,
        mv_range_high: None,
        mv_range_low: None,
        direction: Some(initial.direction),
        tag_overrides: None,
        notes: None,
        yes: true,
        write_pid: None,
        #[cfg(test)]
        mrft_delay: timing.mrft_delay_secs,
        #[cfg(test)]
        poll_interval_ms: timing.poll_interval_ms,
        #[cfg(test)]
        timeout_secs: timing.timeout_secs,
        #[cfg(test)]
        op_timeout_secs: timing.op_timeout_secs,
        #[cfg(test)]
        restore_timeout_secs: timing.restore_timeout_secs,
    }
}

async fn recovery_mv_tracker(
    pool: &SqlitePool,
    run: &TuneRunRow,
    initial: &InitialState,
    args: &TuneRequest,
) -> anyhow::Result<MvActuationTracker> {
    let rows = TuneMvActuationRow::list_for_run(pool, run.id).await?;
    let next_sequence = rows
        .iter()
        .map(|row| row.sequence)
        .max()
        .map_or(Some(0), |sequence| sequence.checked_add(1))
        .ok_or_else(|| anyhow::anyhow!("run {} MV actuation sequence is exhausted", run.id))?;
    let mut tracker = MvActuationTracker::for_run(args, initial).ok_or_else(|| {
        anyhow::anyhow!(
            "run {} cannot initialize its OPC DA MV audit tracker",
            run.id
        )
    })?;
    tracker.next_sequence = next_sequence;
    tracker.previous_commanded_mv = rows
        .iter()
        .max_by_key(|row| row.sequence)
        .map_or(initial.mv_ini, |row| row.target_mv);
    Ok(tracker)
}

async fn recovery_step_reports(
    pool: &SqlitePool,
    run: &TuneRunRow,
    initial: &InitialState,
    guard: &MutationGuard,
    recovery_owner_id: i64,
    args: &TuneRequest,
) -> (Vec<RecoveryStepReport>, Option<String>, bool) {
    let mutations = match LiveMutationStepRow::list_for_owner(pool, recovery_owner_id).await {
        Ok(steps) => steps,
        Err(error) => {
            return (
                vec![failed_step(
                    "audit",
                    None,
                    format!("could not read persisted recovery mutation audit: {error}"),
                )],
                Some(format!(
                    "could not read persisted recovery mutation audit: {error}"
                )),
                true,
            );
        }
    };
    let controller_mutation_attempted =
        mutations.iter().any(|step| step.step == "controller_write");
    let actuations = match TuneMvActuationRow::list_for_run(pool, run.id).await {
        Ok(rows) => rows,
        Err(error) => {
            return (
                vec![failed_step(
                    "mv",
                    Some(initial.mv_ini.to_string()),
                    format!("could not read MV confirmation audit: {error}"),
                )],
                Some(format!("could not read MV confirmation audit: {error}")),
                controller_mutation_attempted,
            );
        }
    };

    let mut reports = Vec::with_capacity(4);
    let mv = actuations
        .iter()
        .filter(|row| row.kind == MvActuationKind::Restore)
        .max_by_key(|row| row.sequence);
    let mv_confirmed = mv.is_some_and(|row| row.status == MvActuationStatus::Confirmed);
    reports.push(match mv {
        Some(row) if row.status == MvActuationStatus::Confirmed => RecoveryStepReport {
            step: "mv".to_string(),
            status: RecoveryStepStatus::Succeeded,
            target: Some(row.target_mv.to_string()),
            detail: row.detail.clone(),
        },
        Some(row) => failed_step(
            "mv",
            Some(row.target_mv.to_string()),
            row.detail
                .clone()
                .unwrap_or_else(|| format!("MV restore confirmation ended as {:?}", row.status)),
        ),
        None => failed_step(
            "mv",
            Some(initial.mv_ini.to_string()),
            "no durable MV restore confirmation was recorded".to_string(),
        ),
    });

    let mode_deferred =
        should_settle_before_auto_release(args, &run.tags, &run.template, initial, guard)
            && !mv_confirmed;
    let mode_needed = !mode_deferred
        && guard.mode_written
        && run.template.revert_mode
        && run.tags.controller_mode.is_some()
        && initial.mode_raw.as_deref() != Some(run.template.mode_manual_value.as_str());
    let mut mode_report = write_step_report(
        "mode",
        run.tags.controller_mode.as_deref(),
        initial
            .mode_raw
            .as_deref()
            .map(|value| Value::String(value.to_string())),
        mode_needed,
        &mutations,
    );
    if mode_deferred {
        mode_report.detail = Some(
            "MV restore was not confirmed, so automatic-mode release was intentionally withheld; inspect the loop before changing modes"
                .to_string(),
        );
    }
    reports.push(mode_report);

    let setpoint_needed = guard.mode_written
        && run.template.revert_mode
        && run.tags.setpoint_variable.is_some()
        && initial.setpoint_ini.is_some();
    reports.push(write_step_report(
        "setpoint",
        run.tags.setpoint_variable.as_deref(),
        initial.setpoint_ini.map(|value| json!(value)),
        setpoint_needed,
        &mutations,
    ));

    let mode_attribute_needed = guard.mode_attribute_written
        && run.tags.mode_attribute.is_some()
        && initial.mode_attribute_raw.as_deref()
            != run.template.mode_attribute_program_value.as_deref();
    reports.push(write_step_report(
        "mode_attribute",
        run.tags.mode_attribute.as_deref(),
        initial
            .mode_attribute_raw
            .as_deref()
            .map(|value| Value::String(value.to_string())),
        mode_attribute_needed,
        &mutations,
    ));

    let audit_error = reports
        .iter()
        .find(|step| step.status == RecoveryStepStatus::Failed)
        .and_then(|step| step.detail.clone());
    (reports, audit_error, controller_mutation_attempted)
}

fn write_step_report(
    name: &str,
    tag: Option<&str>,
    expected: Option<Value>,
    needed: bool,
    mutation_steps: &[LiveMutationStepRow],
) -> RecoveryStepReport {
    if !needed {
        return RecoveryStepReport {
            step: name.to_string(),
            status: RecoveryStepStatus::NotNeeded,
            target: expected.as_ref().map(value_label),
            detail: None,
        };
    }
    let (Some(tag), Some(expected)) = (tag, expected) else {
        return failed_step(
            name,
            None,
            "restore preconditions require a recorded tag and target value".to_string(),
        );
    };
    let latest = mutation_steps
        .iter()
        .filter(|step| step.step == "controller_write")
        .filter_map(|step| {
            let (written_tag, written_value) = parse_write_target(&step.target_json)?;
            (written_tag == tag && json_values_match(&expected, &written_value)).then_some(step)
        })
        .max_by_key(|step| step.id);
    match latest {
        Some(step) if step.status == MutationStepStatus::Confirmed => RecoveryStepReport {
            step: name.to_string(),
            status: RecoveryStepStatus::Succeeded,
            target: Some(value_label(&expected)),
            detail: step.detail.clone(),
        },
        Some(step) => failed_step(
            name,
            Some(value_label(&expected)),
            step.detail
                .clone()
                .unwrap_or_else(|| format!("write audit ended as {:?}", step.status)),
        ),
        None => failed_step(
            name,
            Some(value_label(&expected)),
            "no durable restore write audit was recorded".to_string(),
        ),
    }
}

fn json_values_match(expected: &Value, actual: &Value) -> bool {
    match (expected.as_f64(), actual.as_f64()) {
        (Some(expected), Some(actual)) => expected == actual,
        _ => expected == actual,
    }
}

fn value_label(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn failed_step(name: &str, target: Option<String>, detail: String) -> RecoveryStepReport {
    RecoveryStepReport {
        step: name.to_string(),
        status: RecoveryStepStatus::Failed,
        target,
        detail: Some(detail),
    }
}

async fn export_run_evidence(
    pool: &SqlitePool,
    db_path: &Path,
    run: &TuneRunRow,
    candidate_owner_id: i64,
    reason: &str,
) -> anyhow::Result<String> {
    let owners = LiveOwnershipRow::list_for_run(pool, run.id).await?;
    let mutation_steps = LiveMutationStepRow::list_for_run(pool, run.id).await?;
    let attempts = TuneRecoveryAttemptRow::list_for_run(pool, run.id).await?;
    let mv_actuations = TuneMvActuationRow::list_for_run(pool, run.id).await?;
    write_recovery_export(
        db_path,
        &RecoveryExport {
            version: 1,
            exported_at: Utc::now(),
            reason,
            candidate_owner_id,
            run_id: Some(run.id),
            run: Some(run),
            owners: &owners,
            mutation_steps: &mutation_steps,
            attempts: &attempts,
            mv_actuations: &mv_actuations,
        },
    )
}

async fn export_owner_evidence(
    pool: &SqlitePool,
    db_path: &Path,
    owner: &LiveOwnershipRow,
    reason: &str,
) -> anyhow::Result<String> {
    let owners = vec![owner.clone()];
    let mutation_steps = LiveMutationStepRow::list_for_owner(pool, owner.id).await?;
    let attempts = Vec::new();
    let mv_actuations = Vec::new();
    write_recovery_export(
        db_path,
        &RecoveryExport {
            version: 1,
            exported_at: Utc::now(),
            reason,
            candidate_owner_id: owner.id,
            run_id: None,
            run: None,
            owners: &owners,
            mutation_steps: &mutation_steps,
            attempts: &attempts,
            mv_actuations: &mv_actuations,
        },
    )
}

fn write_recovery_export(db_path: &Path, evidence: &RecoveryExport<'_>) -> anyhow::Result<String> {
    write_recovery_export_with_open(db_path, evidence, |path| {
        OpenOptions::new().write(true).create_new(true).open(path)
    })
}

fn write_recovery_export_with_open(
    db_path: &Path,
    evidence: &RecoveryExport<'_>,
    mut open: impl FnMut(&Path) -> std::io::Result<File>,
) -> anyhow::Result<String> {
    let parent = db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let database_name = db_path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| anyhow::anyhow!("database filename is not valid UTF-8"))?;
    let export_directory = parent.join(format!("{database_name}.recovery"));
    fs::create_dir_all(&export_directory).map_err(|error| {
        anyhow::anyhow!(
            "could not create recovery evidence directory {}: {error}",
            export_directory.display()
        )
    })?;
    let timestamp = evidence.exported_at.format("%Y%m%dT%H%M%S%.3fZ");
    let run_label = evidence
        .run_id
        .map_or_else(|| "no-run".to_string(), |run_id| format!("run-{run_id}"));
    for suffix in 0..100_u8 {
        let path = export_directory.join(format!(
            "{timestamp}-{run_label}-owner-{}-{suffix:02}.json",
            evidence.candidate_owner_id,
        ));
        match open(&path) {
            Ok(mut file) => {
                serde_json::to_writer_pretty(&mut file, evidence)?;
                file.write_all(b"\n")?;
                file.sync_all()?;
                #[cfg(unix)]
                File::open(&export_directory)?.sync_all()?;
                return Ok(path.to_string_lossy().into_owned());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(anyhow::anyhow!(
                    "could not write recovery evidence export {}: {error}",
                    path.display()
                ));
            }
        }
    }
    anyhow::bail!(
        "could not choose a unique recovery evidence export path under {}",
        export_directory.display()
    )
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::path::PathBuf;
    use std::sync::Arc;

    use bhtune_core::{ControllerDirection, ControllerType, LoopConfig, LoopTags, ProcessType};
    use bhtune_db::models::{
        EffectiveTuning, LiveMutationStepRow, LiveOperationKind, LiveOwnershipRow,
        MutationStepStatus, MvActuationKind, NewLiveMutationStep, NewLiveOwnership,
        NewTuneMvActuation, RecoveryAttemptStatus, TemplateOrigin, TuneDriver,
        TuneRecoveryAttemptRow, TuneRunInitialReadings, TuneRunRow,
    };
    use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue, WriteResponse};

    use super::*;
    use crate::{
        cancel::CtrlC,
        test_support::{MockBridgeService, start_mock_server},
    };

    fn running_run() -> TuneRunRow {
        let template = bhtune_core::built_in_templates().remove(0);
        let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
        TuneRunRow {
            id: 11,
            loop_id: None,
            demo_session_id: None,
            loop_name: "Unit1.LIC101.PV".to_string(),
            driver: TuneDriver::Opcda,
            opc_server: Some("Kepware.KEPServerEX.V6".to_string()),
            bridge_host: Some("localhost:7600".to_string()),
            started_at: Utc::now(),
            completed_at: None,
            outcome: TuneOutcome::Running,
            failure_reason: None,
            config: LoopConfig {
                process_type: ProcessType::Flow,
                controller_type: ControllerType::Pi,
                relay_amp_percent: 10.0,
                num_cycles_skip: 1,
                num_cycles_count: 2,
                noise_protection_secs: 3,
                mrft_delay_secs: 0,
            },
            template_origin: TemplateOrigin::Builtin,
            template,
            tags,
            request_json: "{}".to_string(),
            notes: None,
            initial_readings: Some(TuneRunInitialReadings {
                pv_ini: 40.0,
                mv_ini: 45.0,
                mv_range_low: 0.0,
                mv_range_high: 100.0,
                pv_range_high: 100.0,
                pv_range_low: 0.0,
                controller_direction: ControllerDirection::Direct,
                mode_raw: Some("Auto".to_string()),
                mode_attribute_raw: Some("Computer".to_string()),
                setpoint_ini: Some(40.0),
            }),
            allow_uncertain_quality: false,
            timing_metrics: None,
            effective_tuning: Some(bhtune_db::models::EffectiveTuning {
                mrft_delay_secs: 0,
                poll_interval_ms: 1000,
                timeout_secs: 3600,
                op_timeout_secs: 3,
                restore_timeout_secs: 10,
            }),
            gateway_compatibility_json: None,
            restore_status: None,
            restore_detail: None,
            recovery_state: None,
            recovery_evidence_json: None,
            created_at: Utc::now(),
        }
    }

    fn owner(run: &TuneRunRow) -> LiveOwnershipRow {
        let initial = run.initial_readings.as_ref().unwrap();
        let intent = json!({
            "version": 1,
            "kind": "tune_restore",
            "run_id": run.id,
            "state": "ready_to_restore",
            "mv_tag": run.tags.manipulated_variable,
            "initial_readings": initial,
            "template_policy": {
                "revert_mode": run.template.revert_mode,
                "mode_auto_value": run.template.mode_auto_value,
                "mode_manual_value": run.template.mode_manual_value,
                "mode_attribute_program_value": run.template.mode_attribute_program_value,
            }
        });
        LiveOwnershipRow {
            id: 19,
            run_id: Some(run.id),
            operation_kind: LiveOperationKind::Tune,
            database_key: "/db/bhtune.sqlite".to_string(),
            resource_key: "resource".to_string(),
            owner_pid: 100,
            acquired_at: Utc::now(),
            heartbeat_at: Utc::now(),
            released_at: None,
            state: LiveOwnershipState::Active,
            restore_intent_json: Some(intent.to_string()),
            orphan_eligible: false,
            orphan_evidence_json: None,
        }
    }

    fn mode_write(run: &TuneRunRow, owner_id: i64) -> LiveMutationStepRow {
        LiveMutationStepRow {
            id: 1,
            owner_id,
            run_id: Some(run.id),
            step: "controller_write".to_string(),
            target_json: json!({
                "tag": run.tags.manipulated_variable,
                "value": 50.0,
            })
            .to_string(),
            previous_json: Some(json!(45.0).to_string()),
            status: MutationStepStatus::Confirmed,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            readback_json: None,
            detail: None,
        }
    }

    async fn file_pool() -> (tempfile::TempDir, SqlitePool) {
        let directory = tempfile::tempdir().unwrap();
        let pool = bhtune_db::connect(&directory.path().join("recovery-test.db"))
            .await
            .unwrap();
        (directory, pool)
    }

    async fn create_run(pool: &SqlitePool, bridge_host: &str) -> TuneRunRow {
        let template = bhtune_core::built_in_templates()
            .into_iter()
            .find(|template| template.name == "Honeywell Experion")
            .unwrap();
        let tags = LoopTags::derive_from_pv_tag("TestLoop.PV", &template);
        let config = LoopConfig {
            process_type: ProcessType::Flow,
            controller_type: ControllerType::Pi,
            relay_amp_percent: 10.0,
            num_cycles_skip: 1,
            num_cycles_count: 2,
            noise_protection_secs: 3,
            mrft_delay_secs: 0,
        };
        let now = Utc::now();
        let run = TuneRunRow::start(
            pool,
            None,
            "TestLoop.PV",
            TuneDriver::Opcda,
            config,
            TemplateOrigin::Builtin,
            &template,
            &tags,
            now,
        )
        .await
        .unwrap();
        TuneRunRow::record_effective_tuning(
            pool,
            run.id,
            EffectiveTuning {
                mrft_delay_secs: 0,
                poll_interval_ms: 1000,
                timeout_secs: 3600,
                op_timeout_secs: 3,
                restore_timeout_secs: 10,
            },
        )
        .await
        .unwrap();
        TuneRunRow::record_connection(
            pool,
            run.id,
            Some("Mock.Kepware.Sim"),
            Some(bridge_host),
            "{}",
        )
        .await
        .unwrap();
        let initial = TuneRunInitialReadings {
            pv_ini: 40.0,
            mv_ini: 45.0,
            mv_range_low: 0.0,
            mv_range_high: 100.0,
            pv_range_high: 100.0,
            pv_range_low: 0.0,
            controller_direction: ControllerDirection::Direct,
            mode_raw: Some(template.mode_auto_value.clone()),
            mode_attribute_raw: Some("1".to_string()),
            setpoint_ini: Some(40.0),
        };
        TuneRunRow::record_initial_readings(pool, run.id, initial)
            .await
            .unwrap();
        TuneRunRow::record_allow_uncertain_quality(pool, run.id, false)
            .await
            .unwrap();
        TuneRunRow::get(pool, run.id).await.unwrap().unwrap()
    }

    async fn create_live_run(
        pool: &SqlitePool,
        bridge_host: &str,
        include_mode_mutations: bool,
    ) -> (TuneRunRow, LiveOperationGuard) {
        create_live_run_with_optional_mutations(pool, bridge_host, include_mode_mutations, true)
            .await
    }

    async fn create_live_run_with_optional_mutations(
        pool: &SqlitePool,
        bridge_host: &str,
        include_mode_mutations: bool,
        include_optional_tags: bool,
    ) -> (TuneRunRow, LiveOperationGuard) {
        let mut run = create_run(pool, bridge_host).await;
        if !include_optional_tags {
            run.tags.controller_mode = None;
            run.tags.setpoint_variable = None;
            run.tags.mode_attribute = None;
        }
        let initial_json = serde_json::to_value(run.initial_readings.as_ref().unwrap()).unwrap();
        let restore_intent = serde_json::to_string(&json!({
            "version": 1,
            "kind": "tune_restore",
            "run_id": run.id,
            "state": "ready_to_restore",
            "mv_tag": run.tags.manipulated_variable,
            "initial_readings": initial_json,
            "template_policy": {
                "revert_mode": run.template.revert_mode,
                "mode_auto_value": run.template.mode_auto_value,
                "mode_manual_value": run.template.mode_manual_value,
                "mode_attribute_program_value": run.template.mode_attribute_program_value,
            },
        }))
        .unwrap();
        let ownership = LiveOperationGuard::acquire(
            pool,
            Some(run.id),
            LiveOperationKind::Tune,
            bridge_host,
            "Mock.Kepware.Sim",
            &run.tags.manipulated_variable,
            Some(restore_intent),
        )
        .await
        .unwrap();
        add_source_mutation(
            pool,
            &run,
            ownership.owner().id,
            &run.tags.manipulated_variable,
            json!(55.0),
            json!(45.0),
        )
        .await;
        if include_mode_mutations {
            if let Some(mode_tag) = run.tags.controller_mode.as_deref() {
                add_source_mutation(
                    pool,
                    &run,
                    ownership.owner().id,
                    mode_tag,
                    json!(run.template.mode_manual_value),
                    json!(run.template.mode_auto_value),
                )
                .await;
            }
            if let (Some(setpoint_tag), Some(setpoint)) = (
                run.tags.setpoint_variable.as_deref(),
                run.initial_readings
                    .as_ref()
                    .and_then(|initial| initial.setpoint_ini),
            ) {
                add_source_mutation(
                    pool,
                    &run,
                    ownership.owner().id,
                    setpoint_tag,
                    json!(setpoint + 1.0),
                    json!(setpoint),
                )
                .await;
            }
            if let (Some(attribute_tag), Some(program_value)) = (
                run.tags.mode_attribute.as_deref(),
                run.template.mode_attribute_program_value.as_deref(),
            ) {
                add_source_mutation(
                    pool,
                    &run,
                    ownership.owner().id,
                    attribute_tag,
                    json!(program_value),
                    json!("1"),
                )
                .await;
            }
        }
        (run, ownership)
    }

    async fn add_source_mutation(
        pool: &SqlitePool,
        run: &TuneRunRow,
        owner_id: i64,
        tag: &str,
        target: Value,
        previous: Value,
    ) {
        let started_at = Utc::now();
        let step = LiveMutationStepRow::begin(
            pool,
            NewLiveMutationStep {
                owner_id,
                run_id: Some(run.id),
                step: "controller_write".to_string(),
                target_json: json!({"tag": tag, "value": target}).to_string(),
                previous_json: Some(previous.to_string()),
                started_at,
            },
        )
        .await
        .unwrap();
        LiveMutationStepRow::finish(
            pool,
            step.id,
            MutationStepStatus::Confirmed,
            Utc::now(),
            None,
            Some("simulated accepted write in recovery fixture"),
        )
        .await
        .unwrap();
    }

    async fn age_owner_heartbeat(pool: &SqlitePool, owner: &mut LiveOperationGuard) {
        owner.stop_heartbeat().await;
        sqlx::query("UPDATE live_operation_owners SET heartbeat_at = ? WHERE id = ?")
            .bind(Utc::now() - chrono::Duration::seconds(31))
            .bind(owner.owner().id)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn mark_fixture_orphaned(
        pool: &SqlitePool,
        mut owner: LiveOperationGuard,
    ) -> (i64, String) {
        let owner_id = owner.owner().id;
        age_owner_heartbeat(pool, &mut owner).await;
        drop(owner);
        let report = recover_startup_orphans(pool).await.unwrap();
        assert_eq!(report.marked_orphaned, 1);
        assert_eq!(report.recoverable, 1);
        assert_eq!(report.exports.len(), 1);
        let source = LiveOwnershipRow::get(pool, owner_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.state, LiveOwnershipState::Orphaned);
        assert!(source.orphan_eligible);
        (owner_id, report.exports[0].clone())
    }

    fn mock_gateway(read_value: &str, quality: &str, write_success: bool) -> MockBridgeService {
        MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "ignored".to_string(),
                    value: read_value.to_string(),
                    quality: quality.to_string(),
                    timestamp: "N/A".to_string(),
                }],
            },
            write_response: WriteResponse {
                tag_id: "ignored".to_string(),
                success: write_success,
                error: (!write_success).then(|| "simulated rejection".to_string()),
            },
            ..Default::default()
        }
    }

    async fn start_interrupted_recovery(
        pool: &SqlitePool,
        run_id: i64,
        source_owner_id: i64,
        export_path: &str,
    ) -> (LiveOperationGuard, TuneRecoveryAttemptRow) {
        let file_guard = LiveOperationGuard::try_acquire_database_file(pool)
            .await
            .unwrap()
            .unwrap();
        let source = LiveOwnershipRow::get(pool, source_owner_id)
            .await
            .unwrap()
            .unwrap();
        let now = Utc::now();
        let expected_claim_key = claim_key(&source.database_key, &source.resource_key).unwrap();
        let evidence = json!({
            "version": 1,
            "event": "test_interrupted_recovery",
            "run_id": run_id,
            "source_owner_id": source_owner_id,
            "controller_mutation_attempted": false,
        })
        .to_string();
        let (owner, attempt) = TuneRecoveryAttemptRow::start_with_claim(
            pool,
            NewLiveOwnership {
                run_id: Some(run_id),
                operation_kind: LiveOperationKind::Recovery,
                database_key: source.database_key.clone(),
                resource_key: source.resource_key.clone(),
                owner_pid: i64::from(std::process::id()),
                acquired_at: now,
                restore_intent_json: source.restore_intent_json.clone(),
            },
            &expected_claim_key,
            source_owner_id,
            run_id,
            export_path,
            &evidence,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(file_guard.database_key(), source.database_key);
        (
            LiveOperationGuard::from_database_owner(pool, file_guard, owner, expected_claim_key),
            attempt,
        )
    }

    async fn interrupted_recovery_fixture(
        pool: &SqlitePool,
    ) -> (
        TuneRunRow,
        i64,
        String,
        LiveOperationGuard,
        TuneRecoveryAttemptRow,
    ) {
        let (run, owner) = create_live_run(pool, "127.0.0.1:7602", false).await;
        let (source_owner_id, export_path) = mark_fixture_orphaned(pool, owner).await;
        let (recovery_owner, attempt) =
            start_interrupted_recovery(pool, run.id, source_owner_id, &export_path).await;
        (run, source_owner_id, export_path, recovery_owner, attempt)
    }

    #[test]
    fn eligibility_requires_intent_and_audited_loop_mutation() {
        let run = running_run();
        let mut owner = owner(&run);
        let step = mode_write(&run, owner.id);
        assert!(recovery_eligibility(&run, &owner, std::slice::from_ref(&step)).0);

        owner.restore_intent_json = None;
        assert!(!recovery_eligibility(&run, &owner, &[step]).0);
    }

    #[test]
    fn eligibility_rejects_mismatched_provenance_and_corrupt_audit_targets() {
        let run = running_run();
        let mut ownership = owner(&run);
        let mut step = mode_write(&run, ownership.id);
        ownership.run_id = Some(run.id + 1);
        assert!(!recovery_eligibility(&run, &ownership, std::slice::from_ref(&step)).0);

        ownership = owner(&run);
        step.target_json = "not-json".to_string();
        assert!(!recovery_eligibility(&run, &ownership, &[step]).0);
    }

    #[test]
    fn eligibility_requires_complete_connection_readings_timing_and_mutation_evidence() {
        let run = running_run();
        let valid_owner = owner(&run);
        let step = mode_write(&run, valid_owner.id);

        let mut invalid_run = running_run();
        invalid_run.driver = TuneDriver::Simulator;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run.outcome = TuneOutcome::Completed;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        let mut invalid_owner = owner(&run);
        invalid_owner.operation_kind = LiveOperationKind::PidWrite;
        assert!(!recovery_eligibility(&run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_owner = owner(&run);
        invalid_owner.run_id = Some(run.id + 1);
        assert!(!recovery_eligibility(&run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.bridge_host = None;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run.opc_server = None;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run.initial_readings = None;
        assert!(!recovery_eligibility(&invalid_run, &owner(&run), std::slice::from_ref(&step)).0);
        assert!(initial_state(&invalid_run).is_err());

        invalid_owner = owner(&run);
        invalid_owner.restore_intent_json = None;
        assert!(!recovery_eligibility(&run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_owner = owner(&run);
        invalid_owner.restore_intent_json = Some("{".to_string());
        assert!(!recovery_eligibility(&run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.effective_tuning = None;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run
            .effective_tuning
            .as_mut()
            .unwrap()
            .poll_interval_ms = 0;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run.config.relay_amp_percent = 0.0;
        assert!(
            !recovery_eligibility(
                &invalid_run,
                &owner(&invalid_run),
                std::slice::from_ref(&step)
            )
            .0
        );

        invalid_run = running_run();
        invalid_run.initial_readings.as_mut().unwrap().mv_ini = 101.0;
        invalid_owner = owner(&invalid_run);
        assert!(!recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.initial_readings.as_mut().unwrap().pv_ini = f32::NAN;
        invalid_owner = owner(&invalid_run);
        assert!(!recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.tags.setpoint_variable = Some("TestLoop.SP".to_string());
        invalid_run.tags.controller_mode = Some("TestLoop.MODE".to_string());
        invalid_run.template.revert_mode = true;
        invalid_run.template.mode_auto_value = "AUTO".to_string();
        invalid_run.template.mode_manual_value = "MANUAL".to_string();
        invalid_run.initial_readings.as_mut().unwrap().mode_raw =
            Some(invalid_run.template.mode_auto_value.clone());
        invalid_run.initial_readings.as_mut().unwrap().setpoint_ini = None;
        invalid_owner = owner(&invalid_run);
        assert!(!recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.tags.setpoint_variable = Some("TestLoop.SP".to_string());
        invalid_run.tags.controller_mode = Some("TestLoop.MODE".to_string());
        invalid_run.template.revert_mode = true;
        invalid_run.template.mode_auto_value = "AUTO".to_string();
        invalid_run.template.mode_manual_value = "MANUAL".to_string();
        invalid_run.initial_readings.as_mut().unwrap().mode_raw =
            Some(invalid_run.template.mode_manual_value.clone());
        invalid_run.initial_readings.as_mut().unwrap().setpoint_ini = None;
        invalid_owner = owner(&invalid_run);
        assert!(recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.tags.controller_mode = Some("TestLoop.MODE".to_string());
        invalid_run.initial_readings.as_mut().unwrap().mode_raw = None;
        invalid_owner = owner(&invalid_run);
        assert!(!recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        invalid_run = running_run();
        invalid_run.tags.mode_attribute = Some("TestLoop.MODE_ATTRIBUTE".to_string());
        invalid_run
            .initial_readings
            .as_mut()
            .unwrap()
            .mode_attribute_raw = None;
        invalid_owner = owner(&invalid_run);
        assert!(!recovery_eligibility(&invalid_run, &invalid_owner, std::slice::from_ref(&step)).0);

        let mut not_needed = step.clone();
        not_needed.status = MutationStepStatus::NotNeeded;
        let mut unrelated = step.clone();
        unrelated.step = "other".to_string();
        assert!(recovery_eligibility(&run, &valid_owner, &[unrelated, not_needed, step.clone()]).0);
        assert!(!recovery_eligibility(&run, &valid_owner, &[]).0);

        let mut malformed_step = step.clone();
        malformed_step.target_json = r#"{"tag":4,"value":45.0}"#.to_string();
        assert!(!recovery_eligibility(&run, &valid_owner, std::slice::from_ref(&malformed_step)).0);

        malformed_step.target_json = json!({
            "tag": "unrelated.tag",
            "value": 45.0,
        })
        .to_string();
        assert!(!recovery_eligibility(&run, &valid_owner, &[malformed_step]).0);
    }

    #[test]
    fn recovery_timing_uses_only_the_saved_budget() {
        let mut run = running_run();
        assert_eq!(recovery_timing(&run).unwrap().restore_timeout_secs, 10);
        run.effective_tuning = None;
        assert!(recovery_timing(&run).is_err());

        for tuning in [
            EffectiveTuning {
                poll_interval_ms: 0,
                ..run.effective_tuning.unwrap_or(EffectiveTuning {
                    mrft_delay_secs: 0,
                    poll_interval_ms: 1000,
                    timeout_secs: 3600,
                    op_timeout_secs: 3,
                    restore_timeout_secs: 10,
                })
            },
            EffectiveTuning {
                op_timeout_secs: 0,
                poll_interval_ms: 1000,
                ..EffectiveTuning {
                    mrft_delay_secs: 0,
                    poll_interval_ms: 1000,
                    timeout_secs: 3600,
                    op_timeout_secs: 3,
                    restore_timeout_secs: 10,
                }
            },
            EffectiveTuning {
                restore_timeout_secs: 0,
                mrft_delay_secs: 0,
                poll_interval_ms: 1000,
                timeout_secs: 3600,
                op_timeout_secs: 3,
            },
        ] {
            run.effective_tuning = Some(tuning);
            assert!(recovery_timing(&run).is_err());
        }
    }

    #[test]
    fn mutation_guard_and_step_reports_use_exact_persisted_write_evidence() {
        let mut run = running_run();
        run.tags.controller_mode = Some("Unit1.LIC101.MODE".to_string());
        run.tags.mode_attribute = Some("Unit1.LIC101.MODE_ATTRIBUTE".to_string());
        run.template.mode_attribute_program_value = Some("1".to_string());
        let ownership = owner(&run);
        let mv_step = mode_write(&run, ownership.id);
        let guard = mutation_guard(&run, std::slice::from_ref(&mv_step));
        assert!(guard.mv_written);
        assert!(!guard.mode_written);
        assert!(!guard.mode_attribute_written);

        let mut steps = vec![mv_step.clone()];
        let mut unrelated = mv_step.clone();
        unrelated.step = "other".to_string();
        steps.push(unrelated);
        let mut not_needed = mv_step.clone();
        not_needed.status = MutationStepStatus::NotNeeded;
        steps.push(not_needed);
        if let Some(mode_tag) = run.tags.controller_mode.as_deref() {
            steps.push(LiveMutationStepRow {
                id: 2,
                owner_id: ownership.id,
                run_id: Some(run.id),
                step: "controller_write".to_string(),
                target_json: json!({
                    "tag": mode_tag,
                    "value": run.template.mode_manual_value,
                })
                .to_string(),
                previous_json: None,
                status: MutationStepStatus::Confirmed,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                readback_json: None,
                detail: None,
            });
        }
        if let (Some(attribute_tag), Some(program_value)) = (
            run.tags.mode_attribute.as_deref(),
            run.template.mode_attribute_program_value.as_deref(),
        ) {
            steps.push(LiveMutationStepRow {
                id: 3,
                owner_id: ownership.id,
                run_id: Some(run.id),
                step: "controller_write".to_string(),
                target_json: json!({
                    "tag": attribute_tag,
                    "value": program_value,
                })
                .to_string(),
                previous_json: None,
                status: MutationStepStatus::Confirmed,
                started_at: Utc::now(),
                completed_at: Some(Utc::now()),
                readback_json: None,
                detail: None,
            });
        }
        let guard = mutation_guard(&run, &steps);
        assert_eq!(guard.mode_written, run.tags.controller_mode.is_some());
        assert_eq!(
            guard.mode_attribute_written,
            run.tags.mode_attribute.is_some()
                && run.template.mode_attribute_program_value.is_some()
        );
        let mut malformed = mv_step.clone();
        malformed.target_json = "not-json".to_string();
        assert!(!mutation_guard(&run, &[malformed]).mv_written);

        let not_needed = write_step_report("mode", None, None, false, &[]);
        assert_eq!(not_needed.status, RecoveryStepStatus::NotNeeded);
        assert_eq!(
            write_step_report("mode", None, None, true, &[]).status,
            RecoveryStepStatus::Failed
        );

        let tag = run.tags.manipulated_variable.as_str();
        let expected = json!(50.0);
        assert_eq!(
            write_step_report("mv", Some(tag), Some(expected.clone()), true, &[]).status,
            RecoveryStepStatus::Failed
        );
        assert_eq!(
            write_step_report(
                "mv",
                Some(tag),
                Some(expected.clone()),
                true,
                std::slice::from_ref(&mv_step),
            )
            .status,
            RecoveryStepStatus::Succeeded
        );

        let mut failed = mv_step.clone();
        failed.status = MutationStepStatus::Intent;
        failed.detail = None;
        assert!(
            write_step_report(
                "mv",
                Some(tag),
                Some(expected.clone()),
                true,
                std::slice::from_ref(&failed),
            )
            .detail
            .unwrap()
            .contains("Intent")
        );
        failed.detail = Some("simulated failure".to_string());
        assert_eq!(
            write_step_report("mv", Some(tag), Some(expected.clone()), true, &[failed])
                .detail
                .as_deref(),
            Some("simulated failure")
        );

        assert!(json_values_match(&json!(1.0), &json!(1)));
        assert!(!json_values_match(&json!("1"), &json!(1)));
        assert_eq!(value_label(&json!("Auto")), "Auto");
        assert_eq!(value_label(&json!(50.0)), "50.0");
        assert!(parse_write_target("{}").is_none());
        assert!(parse_write_target(r#"{"tag":4,"value":50}"#).is_none());
        assert!(parse_write_target(r#"{"tag":"MV"}"#).is_none());
    }

    #[test]
    fn recovery_validation_rejects_unowned_and_incomplete_evidence() {
        let run = running_run();
        let mut source = owner(&run);
        source.state = LiveOwnershipState::Orphaned;
        source.orphan_eligible = true;
        let step = mode_write(&run, source.id);
        assert!(validate_recovery_run(&run, &source, std::slice::from_ref(&step)).is_ok());

        source.state = LiveOwnershipState::Active;
        assert!(
            validate_recovery_run(&run, &source, std::slice::from_ref(&step))
                .unwrap_err()
                .to_string()
                .contains("no matching eligible orphaned tune owner")
        );

        source.state = LiveOwnershipState::Orphaned;
        source.restore_intent_json = Some(r#"{"kind":"tune_restore"}"#.to_string());
        assert!(
            validate_recovery_run(&run, &source, std::slice::from_ref(&step))
                .unwrap_err()
                .to_string()
                .contains("lacks complete structured restore evidence")
        );
    }

    #[test]
    fn recovery_error_details_preserve_existing_context_and_finalization_failure() {
        let mut detail = Some("restore was incomplete".to_string());
        append_error_detail(&mut detail, Ok(()));
        append_error_detail(&mut detail, Err(anyhow::anyhow!("heartbeat failed")));
        assert_eq!(
            detail.as_deref(),
            Some("restore was incomplete; heartbeat failed")
        );
        append_optional_detail(&mut detail, None);
        append_optional_detail(&mut detail, Some("audit failed".to_string()));
        assert_eq!(
            detail.as_deref(),
            Some("restore was incomplete; heartbeat failed; audit failed")
        );
        let mut first_detail = None;
        append_optional_detail(&mut first_detail, Some("first failure".to_string()));
        assert_eq!(first_detail.as_deref(), Some("first failure"));

        assert!(
            final_audit_failure_detail(None, anyhow::anyhow!("disk full"))
                .contains("the restore is not confirmed")
        );
        assert!(
            final_audit_failure_detail(Some("restore failed"), anyhow::anyhow!("disk full"))
                .contains("restore failed; failed to persist the final recovery audit")
        );
    }

    #[test]
    fn restore_intent_must_match_the_full_run_snapshot() {
        let run = running_run();
        let initial = run.initial_readings.as_ref().unwrap();
        let valid = owner(&run).restore_intent_json.unwrap();
        assert!(validate_restore_intent(&run, &valid, initial).is_ok());

        let mut intent: Value = serde_json::from_str(&valid).unwrap();
        intent["mv_tag"] = json!("DifferentLoop.MV");
        assert!(
            validate_restore_intent(&run, &intent.to_string(), initial)
                .unwrap_err()
                .to_string()
                .contains("does not exactly match")
        );
    }

    #[test]
    fn recovery_eligibility_accepts_only_explicit_failed_orphan_states() {
        let mut run = running_run();
        let ownership = owner(&run);
        let step = mode_write(&run, ownership.id);
        run.outcome = TuneOutcome::Failed;
        run.recovery_state = Some(bhtune_db::models::TuneRecoveryState::Eligible);
        assert!(recovery_eligibility(&run, &ownership, std::slice::from_ref(&step)).0);
        run.recovery_state = Some(bhtune_db::models::TuneRecoveryState::Incomplete);
        assert!(recovery_eligibility(&run, &ownership, std::slice::from_ref(&step)).0);
        run.recovery_state = Some(bhtune_db::models::TuneRecoveryState::NotRecoverable);
        assert!(!recovery_eligibility(&run, &ownership, std::slice::from_ref(&step)).0);
    }

    #[test]
    fn mutation_guard_ignores_missing_optional_tags() {
        let mut run = running_run();
        run.tags.controller_mode = None;
        run.tags.mode_attribute = None;
        run.tags.setpoint_variable = None;

        let guard = mutation_guard(&run, &[]);

        assert!(!guard.mode_written);
        assert!(!guard.mode_attribute_written);
        assert!(!guard.mv_written);
    }

    #[tokio::test]
    async fn recovery_mv_tracker_rejects_wrong_driver_and_exhausted_sequences() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        let initial = initial_state(&run).unwrap();
        let timing = recovery_timing(&run).unwrap();
        let mut args =
            recovery_request(&run, "127.0.0.1:7602", "Mock.Kepware.Sim", timing, &initial);
        let tracker = recovery_mv_tracker(&pool, &run, &initial, &args)
            .await
            .unwrap();
        assert_eq!(tracker.next_sequence, 0);
        args.driver = DriverKind::Simulator;
        assert!(
            recovery_mv_tracker(&pool, &run, &initial, &args)
                .await
                .unwrap_err()
                .to_string()
                .contains("cannot initialize")
        );

        args.driver = DriverKind::Opcda;
        let now = Utc::now();
        TuneMvActuationRow::insert_pending(
            &pool,
            run.id,
            NewTuneMvActuation {
                sequence: i64::MAX,
                kind: MvActuationKind::Relay,
                commanded_at: now,
                target_mv: 50.0,
                previous_commanded_mv: Some(45.0),
                tolerance: 0.5,
                confirmation_due_at: now + chrono::Duration::seconds(4),
            },
        )
        .await
        .unwrap();
        assert!(
            recovery_mv_tracker(&pool, &run, &initial, &args)
                .await
                .unwrap_err()
                .to_string()
                .contains("sequence is exhausted")
        );
    }

    #[tokio::test]
    async fn recovery_step_reporting_fails_closed_when_audit_tables_are_unavailable() {
        let (_directory, mutation_pool) = file_pool().await;
        let run = create_run(&mutation_pool, "127.0.0.1:7602").await;
        let initial = initial_state(&run).unwrap();
        let args = recovery_request(
            &run,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            recovery_timing(&run).unwrap(),
            &initial,
        );
        let guard = MutationGuard {
            mode_attribute_written: false,
            mode_written: false,
            mv_written: false,
        };
        sqlx::query("DROP TABLE live_mutation_steps")
            .execute(&mutation_pool)
            .await
            .unwrap();
        let (reports, error, attempted) =
            recovery_step_reports(&mutation_pool, &run, &initial, &guard, 1, &args).await;
        assert!(
            error
                .unwrap()
                .contains("could not read persisted recovery mutation audit")
        );
        assert!(attempted);
        assert_eq!(reports[0].step, "audit");

        let (_directory, actuation_pool) = file_pool().await;
        let run = create_run(&actuation_pool, "127.0.0.1:7602").await;
        let initial = initial_state(&run).unwrap();
        let args = recovery_request(
            &run,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            recovery_timing(&run).unwrap(),
            &initial,
        );
        sqlx::query("DROP TABLE tune_mv_actuations")
            .execute(&actuation_pool)
            .await
            .unwrap();
        let (reports, error, attempted) =
            recovery_step_reports(&actuation_pool, &run, &initial, &guard, 1, &args).await;
        assert!(
            error
                .unwrap()
                .contains("could not read MV confirmation audit")
        );
        assert!(!attempted);
        assert_eq!(reports[0].step, "mv");
    }

    fn empty_recovery_export(exported_at: DateTime<Utc>) -> RecoveryExport<'static> {
        RecoveryExport {
            version: 1,
            exported_at,
            reason: "test",
            candidate_owner_id: 7,
            run_id: None,
            run: None,
            owners: &[],
            mutation_steps: &[],
            attempts: &[],
            mv_actuations: &[],
        }
    }

    #[test]
    fn recovery_export_errors_are_reported_without_losing_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("bhtune.sqlite");
        let evidence = empty_recovery_export(Utc::now());
        fs::write(
            directory.path().join("bhtune.sqlite.recovery"),
            "not a directory",
        )
        .unwrap();
        assert!(
            write_recovery_export(&db_path, &evidence)
                .unwrap_err()
                .to_string()
                .contains("could not create recovery evidence directory")
        );

        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("bhtune.sqlite");
        let error = write_recovery_export_with_open(&db_path, &evidence, |_| {
            Err(std::io::Error::other("injected create error"))
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("could not write recovery evidence export")
        );

        let directory = tempfile::tempdir().unwrap();
        let db_path = directory.path().join("bhtune.sqlite");
        let export_directory = directory.path().join("bhtune.sqlite.recovery");
        fs::create_dir_all(&export_directory).unwrap();
        let timestamp = evidence.exported_at.format("%Y%m%dT%H%M%S%.3fZ");
        for suffix in 0..100_u8 {
            fs::write(
                export_directory.join(format!("{timestamp}-no-run-owner-7-{suffix:02}.json")),
                "preserved evidence",
            )
            .unwrap();
        }
        assert!(
            write_recovery_export(&db_path, &evidence)
                .unwrap_err()
                .to_string()
                .contains("could not choose a unique recovery evidence export path")
        );
        for suffix in 0..100_u8 {
            assert_eq!(
                fs::read_to_string(
                    export_directory.join(format!("{timestamp}-no-run-owner-7-{suffix:02}.json"))
                )
                .unwrap(),
                "preserved evidence"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_export_rejects_a_non_utf8_database_filename() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(
            b"database-\xff.sqlite".to_vec(),
        ));
        let evidence = empty_recovery_export(Utc::now());
        assert!(
            write_recovery_export(&path, &evidence)
                .unwrap_err()
                .to_string()
                .contains("database filename is not valid UTF-8")
        );
    }

    #[tokio::test]
    async fn startup_skips_a_stale_heartbeat_while_the_os_guard_is_held() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;

        let report = recover_startup_orphans(&pool).await.unwrap();

        assert!(report.skipped_for_live_owner);
        assert_eq!(report.examined, 0);
        assert_eq!(report.marked_orphaned, 0);
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Active
        );
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TuneOutcome::Running
        );
        drop(owner);
    }

    #[tokio::test]
    async fn startup_exports_evidence_before_marking_a_verified_tune_orphan() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, export_path) = mark_fixture_orphaned(&pool, owner).await;

        let export: Value = serde_json::from_slice(&fs::read(&export_path).unwrap()).unwrap();
        let persisted_run = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();

        assert_eq!(export["run_id"], run.id);
        assert_eq!(export["candidate_owner_id"], owner_id);
        assert_eq!(export["mutation_steps"].as_array().unwrap().len(), 1);
        assert_eq!(persisted_run.outcome, TuneOutcome::Failed);
        assert_eq!(
            persisted_run.recovery_state,
            Some(bhtune_db::models::TuneRecoveryState::Eligible)
        );
    }

    #[tokio::test]
    async fn startup_does_not_retire_an_owner_when_evidence_export_cannot_be_created() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let db_path = bhtune_db::database_path(&pool).await.unwrap();
        let export_directory = db_path.with_file_name(format!(
            "{}.recovery",
            db_path.file_name().unwrap().to_string_lossy()
        ));
        fs::write(&export_directory, "blocks the recovery evidence directory").unwrap();
        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("could not create recovery evidence directory")
        );
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TuneOutcome::Running
        );
        assert_eq!(
            LiveOwnershipRow::list_for_run(&pool, run.id).await.unwrap()[0].state,
            LiveOwnershipState::Active
        );
    }

    #[tokio::test]
    async fn startup_refuses_a_stale_owner_with_a_mismatched_database_identity() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;
        sqlx::query("UPDATE live_operation_owners SET database_key = 'other-db' WHERE id = ?")
            .bind(owner_id)
            .execute(&pool)
            .await
            .unwrap();
        drop(owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("mismatched database identity or claim")
        );
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Active
        );
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TuneOutcome::Running
        );
    }

    #[tokio::test]
    async fn startup_fails_closed_when_a_stale_owner_references_a_missing_run() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let mut connection = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("DELETE FROM tune_runs WHERE id = ?")
            .bind(run.id)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(error.to_string().contains("references missing run"));
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Active
        );
    }

    #[tokio::test]
    async fn startup_rolls_back_when_an_unrecoverable_run_cannot_be_failed() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            Some(run.id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &run.tags.manipulated_variable,
            Some(r#"{"kind":"pid_restore","state":"pending"}"#.to_string()),
        )
        .await
        .unwrap();
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);
        sqlx::query(
            "CREATE TRIGGER ignore_failed_run BEFORE UPDATE OF outcome ON tune_runs BEGIN SELECT RAISE(IGNORE); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed after its evidence was exported")
        );
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Active
        );
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .outcome,
            TuneOutcome::Running
        );
    }

    #[tokio::test]
    async fn startup_marks_a_tune_without_connection_provenance_unrecoverable() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;
        sqlx::query("UPDATE tune_runs SET bridge_host = NULL WHERE id = ?")
            .bind(run.id)
            .execute(&pool)
            .await
            .unwrap();
        drop(owner);

        let report = recover_startup_orphans(&pool).await.unwrap();
        let persisted_owner = LiveOwnershipRow::get(&pool, owner_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(report.marked_orphaned, 1);
        assert_eq!(report.recoverable, 0);
        assert!(!persisted_owner.orphan_eligible);
    }

    #[tokio::test]
    async fn startup_marks_a_resource_key_mismatch_unrecoverable() {
        let (_directory, pool) = file_pool().await;
        let (run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_row = owner.owner().clone();
        age_owner_heartbeat(&pool, &mut owner).await;
        let wrong_resource =
            resource_key("127.0.0.1:7602", "Mock.Kepware.Sim", "OtherLoop.MV").unwrap();
        let wrong_claim = claim_key(&owner_row.database_key, &wrong_resource).unwrap();
        sqlx::query("UPDATE live_operation_owners SET resource_key = ? WHERE id = ?")
            .bind(&wrong_resource)
            .bind(owner_row.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE live_operation_claims SET claim_key = ? WHERE owner_id = ?")
            .bind(wrong_claim)
            .bind(owner_row.id)
            .execute(&pool)
            .await
            .unwrap();
        drop(owner);

        let report = recover_startup_orphans(&pool).await.unwrap();
        let persisted_owner = LiveOwnershipRow::get(&pool, owner_row.id)
            .await
            .unwrap()
            .unwrap();
        let evidence: Value =
            serde_json::from_str(persisted_owner.orphan_evidence_json.as_deref().unwrap()).unwrap();

        assert_eq!(report.marked_orphaned, 1);
        assert_eq!(report.recoverable, 0);
        assert_eq!(evidence["resource_matches_run"], false);
        assert!(!persisted_owner.orphan_eligible);
        assert_eq!(run.id, persisted_owner.run_id.unwrap());
    }

    #[tokio::test]
    async fn startup_refuses_to_mark_an_owner_changed_after_export() {
        let (_directory, pool) = file_pool().await;
        let (_run, mut owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let owner_id = owner.owner().id;
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);
        sqlx::query(
            "CREATE TRIGGER ignore_orphan_mark BEFORE UPDATE OF state ON live_operation_owners WHEN OLD.operation_kind = 'tune' BEGIN SELECT RAISE(IGNORE); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed after its evidence was exported")
        );
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Active
        );
    }

    #[tokio::test]
    async fn startup_retires_a_stale_non_tune_owner_without_making_it_recoverable() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            Some(run.id),
            LiveOperationKind::PidWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &run.tags.manipulated_variable,
            Some(r#"{"kind":"pid_restore","state":"pending"}"#.to_string()),
        )
        .await
        .unwrap();
        let owner_id = owner.owner().id;
        add_source_mutation(
            &pool,
            &run,
            owner_id,
            &run.tags.manipulated_variable,
            json!(55.0),
            json!(45.0),
        )
        .await;
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let report = recover_startup_orphans(&pool).await.unwrap();
        let retired_owner = LiveOwnershipRow::get(&pool, owner_id)
            .await
            .unwrap()
            .unwrap();
        let failed_run = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();

        assert_eq!(report.retired_unrecoverable, 1);
        assert_eq!(report.recoverable, 0);
        assert_eq!(retired_owner.state, LiveOwnershipState::Orphaned);
        assert!(!retired_owner.orphan_eligible);
        assert_eq!(
            failed_run.recovery_state,
            Some(bhtune_db::models::TuneRecoveryState::NotRecoverable)
        );
        assert!(
            !LiveOwnershipRow::has_claim(
                &pool,
                owner_id,
                &claim_key(&retired_owner.database_key, &retired_owner.resource_key).unwrap(),
            )
            .await
            .unwrap()
        );
    }

    #[tokio::test]
    async fn startup_exports_a_stale_runless_opc_write_owner_before_retiring_it() {
        let (_directory, pool) = file_pool().await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            None,
            LiveOperationKind::OpcWrite,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            "Simulation.Examples.MV",
            Some(
                r#"{"version":1,"kind":"opc_write","tag":"Simulation.Examples.MV","state":"pending"}"#
                    .to_string(),
            ),
        )
        .await
        .unwrap();
        let owner_id = owner.owner().id;
        let step = LiveMutationStepRow::begin(
            &pool,
            NewLiveMutationStep {
                owner_id,
                run_id: None,
                step: "controller_write".to_string(),
                target_json: json!({
                    "tag": "Simulation.Examples.MV",
                    "value": 1.0,
                })
                .to_string(),
                previous_json: None,
                started_at: Utc::now(),
            },
        )
        .await
        .unwrap();
        LiveMutationStepRow::finish(
            &pool,
            step.id,
            MutationStepStatus::Confirmed,
            Utc::now(),
            None,
            Some("simulated accepted write"),
        )
        .await
        .unwrap();
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let report = recover_startup_orphans(&pool).await.unwrap();
        let retired_owner = LiveOwnershipRow::get(&pool, owner_id)
            .await
            .unwrap()
            .unwrap();
        let export: Value = serde_json::from_slice(&fs::read(&report.exports[0]).unwrap()).unwrap();

        assert_eq!(report.retired_unrecoverable, 1);
        assert_eq!(retired_owner.state, LiveOwnershipState::Orphaned);
        assert_eq!(export["run_id"], Value::Null);
        assert_eq!(export["mutation_steps"].as_array().unwrap().len(), 1);
        assert!(!retired_owner.orphan_eligible);
    }

    #[tokio::test]
    async fn restore_loop_confirms_and_persists_each_recorded_restore_step() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Good", true);
        let read_calls = Arc::clone(&service.read_calls);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, true).await;
        TuneRunRow::record_gateway_compatibility(&pool, run.id, "{}")
            .await
            .unwrap();
        let (source_owner_id, _) = mark_fixture_orphaned(&pool, owner).await;

        let report = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap();

        assert_eq!(report.status, RecoveryAttemptStatus::Confirmed);
        assert!(report.persisted);
        assert_eq!(
            report
                .steps
                .iter()
                .map(|step| step.status)
                .collect::<Vec<_>>(),
            vec![RecoveryStepStatus::Succeeded; 4]
        );
        assert!(report.steps.iter().all(|step| step.target.is_some()));
        assert!(read_calls.load(std::sync::atomic::Ordering::Relaxed) >= 1);
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 4);
        let attempts = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, RecoveryAttemptStatus::Confirmed);
        let owners = LiveOwnershipRow::list_for_run(&pool, run.id).await.unwrap();
        assert_eq!(
            owners
                .iter()
                .find(|owner| owner.id == source_owner_id)
                .unwrap()
                .state,
            LiveOwnershipState::Recovered
        );
        let recovery_owner = owners
            .iter()
            .find(|owner| owner.operation_kind == LiveOperationKind::Recovery)
            .unwrap();
        let writes = LiveMutationStepRow::list_for_owner(&pool, recovery_owner.id)
            .await
            .unwrap()
            .into_iter()
            .filter(|step| step.step == "controller_write")
            .map(|step| parse_write_target(&step.target_json).unwrap().0)
            .collect::<Vec<_>>();
        assert_eq!(
            writes,
            vec![
                run.tags.manipulated_variable.clone(),
                run.tags.controller_mode.clone().unwrap(),
                run.tags.setpoint_variable.clone().unwrap(),
                run.tags.mode_attribute.clone().unwrap(),
            ]
        );
    }

    #[tokio::test]
    async fn fixture_handles_a_tune_without_optional_mode_mutation_tags() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) =
            create_live_run_with_optional_mutations(&pool, "127.0.0.1:7602", true, false).await;

        let steps = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
            .await
            .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].run_id, Some(run.id));
        owner.release().await.unwrap();
    }

    #[tokio::test]
    async fn recovery_rejects_legacy_rows_and_provenance_mismatch_without_connecting() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Good", true);
        let read_calls = Arc::clone(&service.read_calls);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;

        let legacy = create_run(&pool, &host).await;
        let legacy_error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: legacy.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();
        assert!(
            legacy_error
                .to_string()
                .contains("legacy and active rows fail closed")
        );

        let (run, owner) = create_live_run(&pool, &host, false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        let provenance_error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: Some("127.0.0.1:7600".to_string()),
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(provenance_error.to_string().contains("contradicts"));
        assert_eq!(read_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Orphaned
        );
    }

    #[tokio::test]
    async fn restore_loop_rejects_a_non_opcda_run_before_looking_for_ownership() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        sqlx::query("UPDATE tune_runs SET driver = 'simulator' WHERE id = ?")
            .bind(run.id)
            .execute(&pool)
            .await
            .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("only a live OPC DA tune can be restored")
        );
    }

    #[tokio::test]
    async fn restore_loop_requires_exactly_one_recoverable_source_owner() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query("UPDATE live_operation_owners SET orphan_eligible = 0 WHERE id = ?")
            .bind(owner_id)
            .execute(&pool)
            .await
            .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("has 0 eligible orphan ownership records")
        );
    }

    #[tokio::test]
    async fn restore_loop_refuses_a_source_owner_for_a_different_database() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query("UPDATE live_operation_owners SET database_key = 'other-db' WHERE id = ?")
            .bind(owner_id)
            .execute(&pool)
            .await
            .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("database ownership identity"));
    }

    #[tokio::test]
    async fn restore_loop_refuses_a_source_owner_for_a_different_resource() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query(
            "UPDATE live_operation_owners SET resource_key = 'other-resource' WHERE id = ?",
        )
        .bind(owner_id)
        .execute(&pool)
        .await
        .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("controller/MV ownership identity")
        );
    }

    #[tokio::test]
    async fn restore_loop_refuses_a_source_without_its_conditional_database_claim() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query("DELETE FROM live_operation_claims WHERE owner_id = ?")
            .bind(owner_id)
            .execute(&pool)
            .await
            .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("no longer owns the canonical"));
    }

    #[tokio::test]
    async fn restore_loop_handles_losing_the_claim_during_atomic_handoff() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query(
            "CREATE TRIGGER reject_recovery_claim_handoff BEFORE UPDATE OF owner_id ON live_operation_claims BEGIN SELECT RAISE(IGNORE); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("lost its orphan recovery claim"));
        assert!(
            LiveOwnershipRow::has_claim(
                &pool,
                owner_id,
                &claim_key(
                    &LiveOwnershipRow::get(&pool, owner_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .database_key,
                    &LiveOwnershipRow::get(&pool, owner_id)
                        .await
                        .unwrap()
                        .unwrap()
                        .resource_key,
                )
                .unwrap(),
            )
            .await
            .unwrap()
        );
    }

    #[tokio::test]
    async fn recovery_setup_failure_finalizing_mv_audit_keeps_recovery_unconfirmed() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (owner_id, _) = mark_fixture_orphaned(&pool, owner).await;
        let now = Utc::now();
        TuneMvActuationRow::insert_pending(
            &pool,
            run.id,
            NewTuneMvActuation {
                sequence: 0,
                kind: MvActuationKind::Relay,
                commanded_at: now,
                target_mv: 50.0,
                previous_commanded_mv: Some(45.0),
                tolerance: 0.5,
                confirmation_due_at: now + chrono::Duration::seconds(4),
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TRIGGER reject_mv_finalization BEFORE UPDATE ON tune_mv_actuations BEGIN SELECT RAISE(FAIL, 'injected MV audit failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("could not finalize an interrupted MV audit row")
        );
        let attempts = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, RecoveryAttemptStatus::Failed);
        assert_eq!(
            LiveOwnershipRow::get(&pool, owner_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            LiveOwnershipState::Orphaned
        );
    }

    #[tokio::test]
    async fn recovery_setup_failure_connecting_uses_the_recorded_endpoint_and_is_audited() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:1", false).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("could not connect to the run's recorded")
        );
        let attempts = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, RecoveryAttemptStatus::Failed);
        assert!(
            attempts[0]
                .detail
                .as_deref()
                .unwrap()
                .contains("no restore write")
        );
    }

    #[tokio::test]
    async fn recovery_setup_failure_rejects_an_incompatible_live_gateway() {
        use opcda_bridge_proto::bridge::{
            GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
        };

        let (_directory, pool) = file_pool().await;
        let service = MockBridgeService {
            gateway_info_response: GetGatewayInfoResponse {
                application_version: "0.5.9".to_string(),
                compatibility_schema_version: 1,
                features: vec![ProtocolFeature {
                    kind: ProtocolFeatureKind::Core as i32,
                    min_version: 9,
                    max_version: 9,
                }],
            },
            ..Default::default()
        };
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, false).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed the live compatibility check")
        );
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()[0]
                .status,
            RecoveryAttemptStatus::Failed
        );
    }

    #[tokio::test]
    async fn recovery_setup_failure_persisting_compatibility_is_audited() {
        let (_directory, pool) = file_pool().await;
        let (host, _server_guard) = start_mock_server(MockBridgeService::default()).await;
        let (run, owner) = create_live_run(&pool, &host, false).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query(
            "CREATE TRIGGER reject_compatibility_snapshot BEFORE UPDATE OF gateway_compatibility_json ON tune_runs BEGIN SELECT RAISE(FAIL, 'injected compatibility persistence failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("could not persist gateway compatibility")
        );
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()[0]
                .status,
            RecoveryAttemptStatus::Failed
        );
    }

    #[tokio::test]
    async fn a_different_run_id_cannot_claim_the_same_connection_and_mv() {
        let (_directory, pool) = file_pool().await;
        let first = create_run(&pool, "127.0.0.1:7602").await;
        let second = create_run(&pool, "127.0.0.1:7602").await;
        let acquire = |run_id| {
            LiveOperationGuard::acquire(
                &pool,
                Some(run_id),
                LiveOperationKind::Tune,
                "127.0.0.1:7602",
                "Mock.Kepware.Sim",
                &first.tags.manipulated_variable,
                None,
            )
        };

        let owner = acquire(first.id).await.unwrap();
        assert!(matches!(
            acquire(second.id).await,
            Err(crate::live_ownership::LiveOwnershipAcquireError::LockBusy)
        ));
        drop(owner);
        assert!(matches!(
            acquire(second.id).await,
            Err(crate::live_ownership::LiveOwnershipAcquireError::ResourceClaimed)
        ));
    }

    #[tokio::test]
    async fn concurrent_database_claims_for_one_resource_have_one_winner() {
        let (_directory, pool) = file_pool().await;
        let first = create_run(&pool, "127.0.0.1:7602").await;
        let second = create_run(&pool, "127.0.0.1:7602").await;
        let db_path = database_path(&pool).await.unwrap();
        let database_key = fs::canonicalize(db_path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let resource_key = resource_key(
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &first.tags.manipulated_variable,
        )
        .unwrap();
        let claim_key = claim_key(&database_key, &resource_key).unwrap();
        let now = Utc::now();
        let new_owner = |run_id| NewLiveOwnership {
            run_id: Some(run_id),
            operation_kind: LiveOperationKind::Tune,
            database_key: database_key.clone(),
            resource_key: resource_key.clone(),
            owner_pid: i64::from(std::process::id()),
            acquired_at: now,
            restore_intent_json: None,
        };

        let (left, right) = tokio::join!(
            LiveOwnershipRow::create_and_claim(&pool, new_owner(first.id), &claim_key),
            LiveOwnershipRow::create_and_claim(&pool, new_owner(second.id), &claim_key),
        );
        let left = left.unwrap();
        let right = right.unwrap();

        assert_ne!(left.is_some(), right.is_some());
        let winner = left.or(right).unwrap();
        assert!(
            LiveOwnershipRow::has_claim(&pool, winner.id, &claim_key)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn competing_recovery_attempts_only_allow_one_controller_restore() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Good", true);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, false).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;
        let request = RestoreLoopRequest {
            run_id: run.id,
            yes: true,
            bridge_host: None,
            server: None,
        };

        let mut left_ctrl_c = CtrlC::never();
        let mut right_ctrl_c = CtrlC::never();
        let (left, right) = tokio::join!(
            restore_loop(&pool, request.clone(), &mut left_ctrl_c),
            restore_loop(&pool, request, &mut right_ctrl_c),
        );

        assert_ne!(left.is_ok(), right.is_ok());
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn restart_sweep_releases_a_dead_recovery_owner_back_to_its_source() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        let (source_owner_id, export_path) = mark_fixture_orphaned(&pool, owner).await;
        let (mut recovery_owner, attempt) =
            start_interrupted_recovery(&pool, run.id, source_owner_id, &export_path).await;
        let recovery_owner_id = recovery_owner.owner().id;
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        drop(recovery_owner);

        let report = recover_startup_orphans(&pool).await.unwrap();
        let source = LiveOwnershipRow::get(&pool, source_owner_id)
            .await
            .unwrap()
            .unwrap();
        let recovery = LiveOwnershipRow::get(&pool, recovery_owner_id)
            .await
            .unwrap()
            .unwrap();
        let attempt_row = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.id == attempt.id)
            .unwrap();

        assert_eq!(report.interrupted_recoveries, 1);
        assert_eq!(attempt_row.status, RecoveryAttemptStatus::Incomplete);
        assert_eq!(source.state, LiveOwnershipState::Orphaned);
        assert!(source.orphan_eligible);
        assert_eq!(recovery.state, LiveOwnershipState::Released);
        assert!(
            LiveOwnershipRow::has_claim(
                &pool,
                source_owner_id,
                &claim_key(&source.database_key, &source.resource_key).unwrap(),
            )
            .await
            .unwrap()
        );
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .recovery_state,
            Some(bhtune_db::models::TuneRecoveryState::Incomplete)
        );
    }

    #[tokio::test]
    async fn restart_sweep_refuses_a_recovery_owner_without_a_run_id() {
        let (_directory, pool) = file_pool().await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            None,
            LiveOperationKind::Recovery,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            "TestLoop.MV",
            Some(r#"{"kind":"tune_restore","state":"ready_to_restore"}"#.to_string()),
        )
        .await
        .unwrap();
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(error.to_string().contains("has no run ID"));
    }

    #[tokio::test]
    async fn restart_sweep_refuses_a_recovery_owner_with_a_missing_run_row() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            Some(run.id),
            LiveOperationKind::Recovery,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &run.tags.manipulated_variable,
            Some(r#"{"kind":"tune_restore","state":"ready_to_restore"}"#.to_string()),
        )
        .await
        .unwrap();
        age_owner_heartbeat(&pool, &mut owner).await;
        let owner_id = owner.owner().id;
        drop(owner);
        let mut connection = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *connection)
            .await
            .unwrap();
        sqlx::query("UPDATE live_operation_owners SET run_id = ? WHERE id = ?")
            .bind(run.id + 10_000)
            .bind(owner_id)
            .execute(&mut *connection)
            .await
            .unwrap();
        drop(connection);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(error.to_string().contains("references missing run"));
    }

    #[tokio::test]
    async fn restart_sweep_rejects_a_recovery_owner_without_running_recovery_state() {
        let (_directory, pool) = file_pool().await;
        let run = create_run(&pool, "127.0.0.1:7602").await;
        let mut owner = LiveOperationGuard::acquire(
            &pool,
            Some(run.id),
            LiveOperationKind::Recovery,
            "127.0.0.1:7602",
            "Mock.Kepware.Sim",
            &run.tags.manipulated_variable,
            Some(r#"{"kind":"tune_restore","state":"ready_to_restore"}"#.to_string()),
        )
        .await
        .unwrap();
        age_owner_heartbeat(&pool, &mut owner).await;
        drop(owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(error.to_string().contains("does not match a failed run"));
    }

    #[tokio::test]
    async fn restart_sweep_requires_exactly_one_eligible_source_owner() {
        let (_directory, pool) = file_pool().await;
        let (run, source_owner_id, _, mut recovery_owner, _) =
            interrupted_recovery_fixture(&pool).await;
        sqlx::query("UPDATE live_operation_owners SET orphan_eligible = 0 WHERE id = ?")
            .bind(source_owner_id)
            .execute(&pool)
            .await
            .unwrap();
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        drop(recovery_owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(error.to_string().contains("has 0 eligible source owners"));
        assert_eq!(
            TuneRunRow::get(&pool, run.id)
                .await
                .unwrap()
                .unwrap()
                .recovery_state,
            Some(bhtune_db::models::TuneRecoveryState::Running)
        );
    }

    #[tokio::test]
    async fn restart_sweep_fails_closed_without_recorded_connection_provenance() {
        let (_directory, pool) = file_pool().await;
        let (run, _, _, mut recovery_owner, _) = interrupted_recovery_fixture(&pool).await;
        sqlx::query("UPDATE tune_runs SET opc_server = NULL WHERE id = ?")
            .bind(run.id)
            .execute(&pool)
            .await
            .unwrap();
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        drop(recovery_owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("incomplete recorded connection provenance")
        );
    }

    #[tokio::test]
    async fn restart_sweep_fails_closed_when_recovery_owner_identity_differs_from_source() {
        let (_directory, pool) = file_pool().await;
        let (_run, _, _, mut recovery_owner, _) = interrupted_recovery_fixture(&pool).await;
        let owner = recovery_owner.owner().clone();
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        let wrong_resource =
            resource_key("127.0.0.1:7602", "Mock.Kepware.Sim", "OtherLoop.MV").unwrap();
        let wrong_claim = claim_key(&owner.database_key, &wrong_resource).unwrap();
        sqlx::query("UPDATE live_operation_owners SET resource_key = ? WHERE id = ?")
            .bind(&wrong_resource)
            .bind(owner.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE live_operation_claims SET claim_key = ? WHERE owner_id = ?")
            .bind(wrong_claim)
            .bind(owner.id)
            .execute(&pool)
            .await
            .unwrap();
        drop(recovery_owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("does not match its source owner")
        );
    }

    #[tokio::test]
    async fn restart_sweep_refuses_an_owner_without_a_running_attempt() {
        let (_directory, pool) = file_pool().await;
        let (_run, _, _, mut recovery_owner, attempt) = interrupted_recovery_fixture(&pool).await;
        sqlx::query("DELETE FROM tune_recovery_attempts WHERE id = ?")
            .bind(attempt.id)
            .execute(&pool)
            .await
            .unwrap();
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        drop(recovery_owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("no matching running recovery attempt")
        );
    }

    #[tokio::test]
    async fn restart_sweep_refuses_to_release_ownership_when_audit_transition_fails() {
        let (_directory, pool) = file_pool().await;
        let (_run, _, _, mut recovery_owner, _) = interrupted_recovery_fixture(&pool).await;
        sqlx::query(
            "CREATE TRIGGER ignore_restart_attempt BEFORE UPDATE OF status ON tune_recovery_attempts BEGIN SELECT RAISE(IGNORE); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        age_owner_heartbeat(&pool, &mut recovery_owner).await;
        drop(recovery_owner);

        let error = recover_startup_orphans(&pool).await.unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed while its restart evidence was being exported")
        );
    }

    #[tokio::test]
    async fn cancellation_before_the_restore_write_is_persisted_as_incomplete() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Good", true);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, true).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;
        let (mut ctrl_c, signal) = CtrlC::test_pair();
        signal.send(1).unwrap();

        let report = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut ctrl_c,
        )
        .await
        .unwrap();

        assert_eq!(report.status, RecoveryAttemptStatus::Incomplete);
        assert!(report.persisted);
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert!(
            report
                .steps
                .iter()
                .any(|step| { step.step == "mv" && step.status == RecoveryStepStatus::Failed })
        );
        let attempts = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap();
        let final_evidence: Value =
            serde_json::from_str(&attempts.last().unwrap().evidence_json).unwrap();
        assert_eq!(
            final_evidence["controller_mutation_attempted"],
            Value::Bool(false)
        );
    }

    #[tokio::test]
    async fn restore_timeout_preserves_the_full_accepted_mv_confirmation_window() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("0.0", "Good", true);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, false).await;
        TuneRunRow::record_effective_tuning(
            &pool,
            run.id,
            EffectiveTuning {
                mrft_delay_secs: 0,
                poll_interval_ms: 1000,
                timeout_secs: 3600,
                op_timeout_secs: 3,
                restore_timeout_secs: crate::config::MIN_OPC_RESTORE_TIMEOUT_SECS,
            },
        )
        .await
        .unwrap();
        let _ = mark_fixture_orphaned(&pool, owner).await;
        let started = tokio::time::Instant::now();

        let report = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap();

        assert_eq!(report.status, RecoveryAttemptStatus::Incomplete);
        assert!(report.persisted);
        assert!(started.elapsed() >= Duration::from_secs(4));
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(report.steps[0].status, RecoveryStepStatus::Failed);
    }

    #[tokio::test]
    async fn a_bad_confirmation_quality_is_incomplete_but_other_restore_steps_are_tried() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Bad", true);
        let write_calls = Arc::clone(&service.write_calls);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, true).await;
        let _ = mark_fixture_orphaned(&pool, owner).await;

        let report = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap();

        assert_eq!(report.status, RecoveryAttemptStatus::Incomplete);
        assert!(report.persisted);
        assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 3);
        assert_eq!(report.steps[0].status, RecoveryStepStatus::Failed);
        let mode = report
            .steps
            .iter()
            .find(|step| step.step == "mode")
            .unwrap();
        assert_eq!(mode.status, RecoveryStepStatus::NotNeeded);
        assert!(
            mode.detail
                .as_deref()
                .unwrap()
                .contains("automatic-mode release was intentionally withheld")
        );
        assert!(report.steps.iter().any(|step| {
            step.step == "setpoint" && step.status == RecoveryStepStatus::Succeeded
        }));
        assert!(report.steps.iter().any(|step| {
            step.step == "mode_attribute" && step.status == RecoveryStepStatus::Succeeded
        }));
    }

    #[tokio::test]
    async fn failed_final_persistence_never_claims_success_or_releases_ownership() {
        let (_directory, pool) = file_pool().await;
        let service = mock_gateway("45.0", "Good", true);
        let (host, _server_guard) = start_mock_server(service).await;
        let (run, owner) = create_live_run(&pool, &host, false).await;
        let (source_owner_id, _export_path) = mark_fixture_orphaned(&pool, owner).await;
        sqlx::query(
            "CREATE TRIGGER reject_recovery_completion BEFORE UPDATE OF status ON tune_recovery_attempts BEGIN SELECT RAISE(FAIL, 'injected completion persistence failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();

        let report = restore_loop(
            &pool,
            RestoreLoopRequest {
                run_id: run.id,
                yes: true,
                bridge_host: None,
                server: None,
            },
            &mut CtrlC::never(),
        )
        .await
        .unwrap();

        assert_eq!(report.status, RecoveryAttemptStatus::Incomplete);
        assert!(!report.persisted);
        assert!(
            report
                .detail
                .as_deref()
                .unwrap()
                .contains("failed to persist the final recovery audit")
        );
        let active_recovery = LiveOwnershipRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .into_iter()
            .find(|owner| owner.operation_kind == LiveOperationKind::Recovery)
            .unwrap();
        assert_eq!(active_recovery.state, LiveOwnershipState::Active);
        assert!(
            LiveOwnershipRow::has_claim(
                &pool,
                active_recovery.id,
                &claim_key(&active_recovery.database_key, &active_recovery.resource_key).unwrap(),
            )
            .await
            .unwrap()
        );
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .into_iter()
                .find(|attempt| attempt.id == report.attempt_id)
                .unwrap()
                .status,
            RecoveryAttemptStatus::Running
        );

        sqlx::query("DROP TRIGGER reject_recovery_completion")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE live_operation_owners SET heartbeat_at = ? WHERE id = ?")
            .bind(Utc::now() - chrono::Duration::seconds(31))
            .bind(active_recovery.id)
            .execute(&pool)
            .await
            .unwrap();
        let restarted = recover_startup_orphans(&pool).await.unwrap();
        assert_eq!(restarted.interrupted_recoveries, 1);
        assert!(
            LiveOwnershipRow::has_claim(
                &pool,
                source_owner_id,
                &claim_key(&active_recovery.database_key, &active_recovery.resource_key).unwrap(),
            )
            .await
            .unwrap()
        );
    }

    #[tokio::test]
    async fn heartbeat_persistence_failure_keeps_the_kernel_and_database_claims() {
        let (_directory, pool) = file_pool().await;
        let (run, owner) = create_live_run(&pool, "127.0.0.1:7602", false).await;
        sqlx::query(
            "CREATE TRIGGER reject_owner_heartbeat BEFORE UPDATE OF heartbeat_at ON live_operation_owners BEGIN SELECT RAISE(FAIL, 'injected heartbeat persistence failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(6)).await;

        assert!(owner.ensure_healthy().is_err());
        assert!(matches!(
            LiveOperationGuard::acquire(
                &pool,
                Some(run.id),
                LiveOperationKind::Tune,
                "127.0.0.1:7602",
                "Mock.Kepware.Sim",
                &run.tags.manipulated_variable,
                None,
            )
            .await,
            Err(crate::live_ownership::LiveOwnershipAcquireError::LockBusy)
        ));
        let owner_id = owner.owner().id;
        drop(owner);
        let persisted_owner = LiveOwnershipRow::get(&pool, owner_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(persisted_owner.state, LiveOwnershipState::Active);
        assert!(
            LiveOwnershipRow::has_claim(
                &pool,
                owner_id,
                &claim_key(&persisted_owner.database_key, &persisted_owner.resource_key).unwrap(),
            )
            .await
            .unwrap()
        );
    }
}
