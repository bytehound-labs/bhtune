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
mod tests;
