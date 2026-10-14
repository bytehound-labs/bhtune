use super::*;
use crate::models::{TemplateOrigin, TuneDriver, TuneOutcome, TuneRecoveryState, TuneRunRow};
use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType};

const DATABASE_KEY: &str = "/test/bhtune.db";
const RESOURCE_KEY: &str = r#"["gateway:7602","Mock.Kepware.Sim","Unit1.FIC101.MV"]"#;
const CLAIM_KEY: &str = "test-database-and-loop-claim";

fn new_owner(run_id: Option<i64>, operation_kind: LiveOperationKind) -> NewLiveOwnership {
    let now = Utc::now();
    NewLiveOwnership {
        run_id,
        operation_kind,
        database_key: DATABASE_KEY.to_string(),
        resource_key: RESOURCE_KEY.to_string(),
        owner_pid: i64::from(std::process::id()),
        acquired_at: now,
        restore_intent_json: Some(r#"{"state":"ready"}"#.to_string()),
    }
}

async fn new_run(pool: &SqlitePool) -> TuneRunRow {
    let template = bhtune_core::built_in_templates().remove(0);
    let tags = LoopTags::derive_from_pv_tag("Unit1.FIC101.PV", &template);
    let config = LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 5.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    };
    TuneRunRow::start(
        pool,
        None,
        "Unit1.FIC101.PV",
        TuneDriver::Opcda,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        Utc::now(),
    )
    .await
    .unwrap()
}

async fn create_owner(
    pool: &SqlitePool,
    run_id: Option<i64>,
    operation_kind: LiveOperationKind,
    claim_key: &str,
) -> LiveOwnershipRow {
    LiveOwnershipRow::create_and_claim(pool, new_owner(run_id, operation_kind), claim_key)
        .await
        .unwrap()
        .unwrap()
}

async fn orphaned_recovery_fixture(
    pool: &SqlitePool,
) -> (
    TuneRunRow,
    LiveOwnershipRow,
    LiveOwnershipRow,
    TuneRecoveryAttemptRow,
) {
    let run = new_run(pool).await;
    let source_owner = create_owner(pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    let marked_at = Utc::now();
    assert!(
        LiveOwnershipRow::mark_orphaned(
            pool,
            OrphanOwnerUpdate {
                owner_id: source_owner.id,
                expected_heartbeat: source_owner.heartbeat_at,
                expected_claim_key: CLAIM_KEY,
                now: marked_at,
                evidence_json: r#"{"event":"startup_sweep"}"#,
                eligible: true,
                detail: "verified orphan",
            },
        )
        .await
        .unwrap()
    );
    let recovery_owner = NewLiveOwnership {
        run_id: Some(run.id),
        operation_kind: LiveOperationKind::Recovery,
        database_key: source_owner.database_key.clone(),
        resource_key: source_owner.resource_key.clone(),
        owner_pid: i64::from(std::process::id()),
        acquired_at: Utc::now(),
        restore_intent_json: source_owner.restore_intent_json.clone(),
    };
    let (recovery_owner, attempt) = TuneRecoveryAttemptRow::start_with_claim(
        pool,
        recovery_owner,
        CLAIM_KEY,
        source_owner.id,
        run.id,
        "recovery-evidence.json",
        r#"{"event":"restore_loop_attempt"}"#,
    )
    .await
    .unwrap()
    .unwrap();
    (run, source_owner, recovery_owner, attempt)
}

#[tokio::test]
async fn ownership_and_mutation_updates_require_the_active_claim() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(
        &pool,
        Some(run.id),
        LiveOperationKind::Tune,
        "owner-test-claim",
    )
    .await;
    let conflict = LiveOwnershipRow::create_and_claim(
        &pool,
        new_owner(Some(run.id), LiveOperationKind::PidWrite),
        "owner-test-claim",
    )
    .await
    .unwrap();
    assert!(conflict.is_none());
    assert_eq!(
        LiveOwnershipRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .len(),
        1
    );

    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id).await.unwrap(),
        Some(owner.clone())
    );
    assert!(
        LiveOwnershipRow::has_claim(&pool, owner.id, "owner-test-claim")
            .await
            .unwrap()
    );
    assert!(
        !LiveOwnershipRow::has_claim(&pool, owner.id, "other-claim")
            .await
            .unwrap()
    );
    sqlx::query("UPDATE live_operation_owners SET heartbeat_at = ? WHERE id = ?")
        .bind(Utc::now() - chrono::Duration::seconds(60))
        .bind(owner.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        LiveOwnershipRow::stale_active_owners(&pool, Utc::now() - chrono::Duration::seconds(30))
            .await
            .unwrap()
            .len(),
        1
    );
    LiveOwnershipRow::heartbeat(&pool, owner.id, Utc::now())
        .await
        .unwrap();
    assert!(
        LiveOwnershipRow::stale_active_owners(&pool, Utc::now() - chrono::Duration::seconds(30))
            .await
            .unwrap()
            .is_empty()
    );
    LiveOwnershipRow::set_restore_intent(&pool, owner.id, r#"{"state":"complete"}"#)
        .await
        .unwrap();

    let started_at = Utc::now();
    let intent = LiveMutationStepRow::begin(
        &pool,
        NewLiveMutationStep {
            owner_id: owner.id,
            run_id: Some(run.id),
            step: "controller_write".to_string(),
            target_json: r#"{"tag":"Unit1.FIC101.MV","value":45.0}"#.to_string(),
            previous_json: Some("40.0".to_string()),
            started_at,
        },
    )
    .await
    .unwrap();
    assert_eq!(intent.status, MutationStepStatus::Intent);
    assert!(
        LiveMutationStepRow::finish(
            &pool,
            intent.id,
            MutationStepStatus::Intent,
            Utc::now(),
            None,
            None,
        )
        .await
        .is_err()
    );
    let confirmed = LiveMutationStepRow::finish(
        &pool,
        intent.id,
        MutationStepStatus::Confirmed,
        Utc::now(),
        Some("45.0"),
        Some("confirmed"),
    )
    .await
    .unwrap();
    assert_eq!(confirmed.readback_json.as_deref(), Some("45.0"));
    assert!(
        LiveMutationStepRow::finish(
            &pool,
            intent.id,
            MutationStepStatus::Failed,
            Utc::now(),
            None,
            Some("duplicate completion"),
        )
        .await
        .is_err()
    );
    let skipped = LiveMutationStepRow::not_needed(
        &pool,
        owner.id,
        run.id,
        "mode",
        Utc::now(),
        "mode did not change",
    )
    .await
    .unwrap();
    assert_eq!(skipped.status, MutationStepStatus::NotNeeded);
    assert_eq!(
        LiveMutationStepRow::list_for_owner(&pool, owner.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        LiveMutationStepRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(
        LiveMutationStepRow::begin(
            &pool,
            NewLiveMutationStep {
                owner_id: owner.id + 1,
                run_id: Some(run.id),
                step: "controller_write".to_string(),
                target_json: "{}".to_string(),
                previous_json: None,
                started_at: Utc::now(),
            },
        )
        .await
        .is_err()
    );

    LiveOwnershipRow::release(&pool, owner.id, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Released
    );
    assert!(
        !LiveOwnershipRow::has_claim(&pool, owner.id, "owner-test-claim")
            .await
            .unwrap()
    );
    assert!(
        LiveOwnershipRow::heartbeat(&pool, owner.id, Utc::now())
            .await
            .is_err()
    );
    assert!(
        LiveOwnershipRow::set_restore_intent(&pool, owner.id, "{}")
            .await
            .is_err()
    );
    assert!(
        LiveOwnershipRow::release(&pool, owner.id, Utc::now())
            .await
            .is_err()
    );
    assert!(
        LiveMutationStepRow::not_needed(
            &pool,
            owner.id,
            run.id,
            "mode",
            Utc::now(),
            "owner released",
        )
        .await
        .is_err()
    );
    assert!(
        LiveOwnershipRow::get(&pool, owner.id + 100)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn release_rolls_back_when_its_database_claim_is_missing() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    sqlx::query("DELETE FROM live_operation_claims WHERE owner_id = ?")
        .bind(owner.id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        LiveOwnershipRow::release(&pool, owner.id, Utc::now())
            .await
            .is_err()
    );
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Active
    );
}

#[tokio::test]
async fn release_rolls_back_if_its_claim_disappears_after_owner_update() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    sqlx::query(
        "CREATE TRIGGER remove_claim_during_release \
             AFTER UPDATE OF state ON live_operation_owners WHEN NEW.state = 'released' \
             BEGIN DELETE FROM live_operation_claims WHERE owner_id = NEW.id; END",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(matches!(
        LiveOwnershipRow::release(&pool, owner.id, Utc::now()).await,
        Err(DbError::LiveOwnershipLost)
    ));
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Active
    );
    assert!(
        LiveOwnershipRow::has_claim(&pool, owner.id, CLAIM_KEY)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn orphan_mark_rolls_back_when_the_run_is_already_terminal() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    TuneRunRow::fail(&pool, run.id, Utc::now(), "already failed")
        .await
        .unwrap();

    assert!(
        !LiveOwnershipRow::mark_orphaned(
            &pool,
            OrphanOwnerUpdate {
                owner_id: owner.id,
                expected_heartbeat: owner.heartbeat_at,
                expected_claim_key: CLAIM_KEY,
                now: Utc::now(),
                evidence_json: "{}",
                eligible: true,
                detail: "verified orphan",
            },
        )
        .await
        .unwrap()
    );
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Active
    );
}

#[tokio::test]
async fn orphan_mark_rolls_back_if_the_run_changes_after_owner_update() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    sqlx::query(
        "CREATE TRIGGER terminalize_run_during_orphan_mark \
             AFTER UPDATE OF state ON live_operation_owners WHEN NEW.state = 'orphaned' \
             BEGIN UPDATE tune_runs SET outcome = 'failed', failure_reason = 'injected race' \
             WHERE id = NEW.run_id; END",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(
        !LiveOwnershipRow::mark_orphaned(
            &pool,
            OrphanOwnerUpdate {
                owner_id: owner.id,
                expected_heartbeat: owner.heartbeat_at,
                expected_claim_key: CLAIM_KEY,
                now: Utc::now(),
                evidence_json: "{}",
                eligible: true,
                detail: "verified orphan",
            },
        )
        .await
        .unwrap()
    );
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
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
async fn orphan_updates_preserve_only_explicitly_recoverable_claims() {
    let pool = crate::connect_in_memory().await.unwrap();
    let recoverable_run = new_run(&pool).await;
    let recoverable_owner = create_owner(
        &pool,
        Some(recoverable_run.id),
        LiveOperationKind::Tune,
        "recoverable-claim",
    )
    .await;
    let wrong_heartbeat = LiveOwnershipRow::mark_orphaned(
        &pool,
        OrphanOwnerUpdate {
            owner_id: recoverable_owner.id,
            expected_heartbeat: recoverable_owner.heartbeat_at + chrono::Duration::seconds(1),
            expected_claim_key: "recoverable-claim",
            now: Utc::now(),
            evidence_json: "{}",
            eligible: true,
            detail: "must not be applied",
        },
    )
    .await
    .unwrap();
    assert!(!wrong_heartbeat);
    assert!(
        LiveOwnershipRow::mark_orphaned(
            &pool,
            OrphanOwnerUpdate {
                owner_id: recoverable_owner.id,
                expected_heartbeat: recoverable_owner.heartbeat_at,
                expected_claim_key: "recoverable-claim",
                now: Utc::now(),
                evidence_json: r#"{"recovery_eligible":true}"#,
                eligible: true,
                detail: "verified restore evidence",
            },
        )
        .await
        .unwrap()
    );
    let marked_owner = LiveOwnershipRow::get(&pool, recoverable_owner.id)
        .await
        .unwrap()
        .unwrap();
    let failed_run = TuneRunRow::get(&pool, recoverable_run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marked_owner.state, LiveOwnershipState::Orphaned);
    assert!(marked_owner.orphan_eligible);
    assert_eq!(failed_run.outcome, TuneOutcome::Failed);
    assert_eq!(failed_run.recovery_state, Some(TuneRecoveryState::Eligible));
    assert!(
        LiveOwnershipRow::has_claim(&pool, recoverable_owner.id, "recoverable-claim")
            .await
            .unwrap()
    );

    let manual_run = new_run(&pool).await;
    let manual_owner = create_owner(
        &pool,
        Some(manual_run.id),
        LiveOperationKind::Tune,
        "manual-claim",
    )
    .await;
    assert!(
        LiveOwnershipRow::mark_orphaned(
            &pool,
            OrphanOwnerUpdate {
                owner_id: manual_owner.id,
                expected_heartbeat: manual_owner.heartbeat_at,
                expected_claim_key: "manual-claim",
                now: Utc::now(),
                evidence_json: r#"{"recovery_eligible":false}"#,
                eligible: false,
                detail: "restore evidence is incomplete",
            },
        )
        .await
        .unwrap()
    );
    let retired_owner = LiveOwnershipRow::get(&pool, manual_owner.id)
        .await
        .unwrap()
        .unwrap();
    let failed_run = TuneRunRow::get(&pool, manual_run.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retired_owner.state, LiveOwnershipState::Orphaned);
    assert!(!retired_owner.orphan_eligible);
    assert_eq!(
        failed_run.recovery_state,
        Some(TuneRecoveryState::NotRecoverable)
    );
    assert!(
        !LiveOwnershipRow::has_claim(&pool, manual_owner.id, "manual-claim")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unrecoverable_owner_retirement_is_transactional() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let owner = create_owner(
        &pool,
        Some(run.id),
        LiveOperationKind::PidWrite,
        "pid-write-claim",
    )
    .await;
    let mismatch = LiveOwnershipRow::mark_orphaned_unrecoverable(
        &pool,
        UnrecoverableOwnerUpdate {
            owner_id: owner.id,
            expected_heartbeat: owner.heartbeat_at + chrono::Duration::seconds(1),
            expected_claim_key: "pid-write-claim",
            now: Utc::now(),
            evidence_json: "{}",
            fail_running_run_id: Some(run.id),
            detail: "mismatched heartbeat",
        },
    )
    .await
    .unwrap();
    assert!(!mismatch);
    assert!(
        LiveOwnershipRow::has_claim(&pool, owner.id, "pid-write-claim")
            .await
            .unwrap()
    );

    let wrong_run = LiveOwnershipRow::mark_orphaned_unrecoverable(
        &pool,
        UnrecoverableOwnerUpdate {
            owner_id: owner.id,
            expected_heartbeat: owner.heartbeat_at,
            expected_claim_key: "pid-write-claim",
            now: Utc::now(),
            evidence_json: "{}",
            fail_running_run_id: Some(run.id + 1),
            detail: "run identity mismatch",
        },
    )
    .await
    .unwrap();
    assert!(!wrong_run);
    assert_eq!(
        LiveOwnershipRow::get(&pool, owner.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Active
    );

    let runless = create_owner(&pool, None, LiveOperationKind::OpcWrite, "runless-claim").await;
    assert!(
        LiveOwnershipRow::mark_orphaned_unrecoverable(
            &pool,
            UnrecoverableOwnerUpdate {
                owner_id: runless.id,
                expected_heartbeat: runless.heartbeat_at,
                expected_claim_key: "runless-claim",
                now: Utc::now(),
                evidence_json: "{}",
                fail_running_run_id: None,
                detail: "runless owner",
            },
        )
        .await
        .unwrap()
    );
    assert!(
        LiveOwnershipRow::get(&pool, runless.id)
            .await
            .unwrap()
            .unwrap()
            .orphan_evidence_json
            .is_some()
    );

    assert!(
        LiveOwnershipRow::mark_orphaned_unrecoverable(
            &pool,
            UnrecoverableOwnerUpdate {
                owner_id: owner.id,
                expected_heartbeat: owner.heartbeat_at,
                expected_claim_key: "pid-write-claim",
                now: Utc::now(),
                evidence_json: "{}",
                fail_running_run_id: Some(run.id),
                detail: "live PID operation retired",
            },
        )
        .await
        .unwrap()
    );
    let failed_run = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
    assert_eq!(failed_run.outcome, TuneOutcome::Failed);
    assert_eq!(
        failed_run.recovery_state,
        Some(TuneRecoveryState::NotRecoverable)
    );
    assert!(
        !LiveOwnershipRow::has_claim(&pool, owner.id, "pid-write-claim")
            .await
            .unwrap()
    );

    let no_claim = create_owner(&pool, None, LiveOperationKind::OpcWrite, "no-claim").await;
    sqlx::query(
            "CREATE TRIGGER preserve_test_claim BEFORE DELETE ON live_operation_claims WHEN OLD.claim_key = 'no-claim' BEGIN SELECT RAISE(IGNORE); END",
        )
            .execute(&pool)
            .await
            .unwrap();
    assert!(
        !LiveOwnershipRow::mark_orphaned_unrecoverable(
            &pool,
            UnrecoverableOwnerUpdate {
                owner_id: no_claim.id,
                expected_heartbeat: no_claim.heartbeat_at,
                expected_claim_key: "no-claim",
                now: Utc::now(),
                evidence_json: "{}",
                fail_running_run_id: None,
                detail: "claim disappeared",
            },
        )
        .await
        .unwrap()
    );
    assert_eq!(
        LiveOwnershipRow::get(&pool, no_claim.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        LiveOwnershipState::Active
    );
    sqlx::query("DROP TRIGGER preserve_test_claim")
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn recovery_attempt_completion_transfers_or_releases_ownership_by_status() {
    for (status, expected_state, expected_owner_state, claim_retained) in [
        (
            RecoveryAttemptStatus::Confirmed,
            TuneRecoveryState::Confirmed,
            LiveOwnershipState::Recovered,
            false,
        ),
        (
            RecoveryAttemptStatus::Incomplete,
            TuneRecoveryState::Incomplete,
            LiveOwnershipState::Orphaned,
            true,
        ),
        (
            RecoveryAttemptStatus::Failed,
            TuneRecoveryState::Eligible,
            LiveOwnershipState::Orphaned,
            true,
        ),
    ] {
        let pool = crate::connect_in_memory().await.unwrap();
        let (run, source_owner, recovery_owner, attempt) = orphaned_recovery_fixture(&pool).await;
        assert!(
            TuneRecoveryAttemptRow::complete_with_ownership(
                &pool,
                CompleteRecoveryUpdate {
                    attempt_id: attempt.id,
                    recovery_owner_id: recovery_owner.id,
                    source_owner_id: source_owner.id,
                    run_id: run.id,
                    claim_key: CLAIM_KEY,
                    status: RecoveryAttemptStatus::Running,
                    now: Utc::now(),
                    detail: None,
                    evidence_json: "{}",
                },
            )
            .await
            .is_err()
        );
        TuneRecoveryAttemptRow::complete_with_ownership(
            &pool,
            CompleteRecoveryUpdate {
                attempt_id: attempt.id,
                recovery_owner_id: recovery_owner.id,
                source_owner_id: source_owner.id,
                run_id: run.id,
                claim_key: CLAIM_KEY,
                status,
                now: Utc::now(),
                detail: Some("terminal test result"),
                evidence_json: r#"{"event":"restore_loop_result"}"#,
            },
        )
        .await
        .unwrap();

        let completed_attempt = TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let completed_run = TuneRunRow::get(&pool, run.id).await.unwrap().unwrap();
        let completed_source = LiveOwnershipRow::get(&pool, source_owner.id)
            .await
            .unwrap()
            .unwrap();
        let completed_recovery = LiveOwnershipRow::get(&pool, recovery_owner.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed_attempt.status, status);
        assert_eq!(completed_run.recovery_state, Some(expected_state));
        assert_eq!(completed_source.state, expected_owner_state);
        assert_eq!(completed_source.orphan_eligible, claim_retained);
        assert_eq!(completed_recovery.state, LiveOwnershipState::Released);
        assert_eq!(
            LiveOwnershipRow::has_claim(
                &pool,
                if claim_retained {
                    source_owner.id
                } else {
                    recovery_owner.id
                },
                CLAIM_KEY,
            )
            .await
            .unwrap(),
            claim_retained
        );
    }
}

#[tokio::test]
async fn recovery_handoff_and_finalization_reject_stale_database_state() {
    let pool = crate::connect_in_memory().await.unwrap();
    let run = new_run(&pool).await;
    let source_owner = create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
    assert!(
        LiveOwnershipRow::mark_orphaned(
            &pool,
            OrphanOwnerUpdate {
                owner_id: source_owner.id,
                expected_heartbeat: source_owner.heartbeat_at,
                expected_claim_key: CLAIM_KEY,
                now: Utc::now(),
                evidence_json: "{}",
                eligible: true,
                detail: "verified orphan",
            },
        )
        .await
        .unwrap()
    );
    let invalid_recovery_owner = NewLiveOwnership {
        run_id: Some(run.id),
        operation_kind: LiveOperationKind::Recovery,
        database_key: source_owner.database_key.clone(),
        resource_key: source_owner.resource_key.clone(),
        owner_pid: i64::from(std::process::id()),
        acquired_at: Utc::now(),
        restore_intent_json: None,
    };
    assert!(
        TuneRecoveryAttemptRow::start_with_claim(
            &pool,
            invalid_recovery_owner.clone(),
            "wrong-claim",
            source_owner.id,
            run.id,
            "evidence.json",
            "{}",
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        LiveOwnershipRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        LiveOwnershipRow::has_claim(&pool, source_owner.id, CLAIM_KEY)
            .await
            .unwrap()
    );

    sqlx::query(
            "CREATE TRIGGER suppress_recovery_state BEFORE UPDATE OF recovery_state ON tune_runs WHEN NEW.recovery_state = 'running' BEGIN SELECT RAISE(IGNORE); END",
        )
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        TuneRecoveryAttemptRow::start_with_claim(
            &pool,
            invalid_recovery_owner,
            CLAIM_KEY,
            source_owner.id,
            run.id,
            "evidence.json",
            "{}",
        )
        .await
        .is_err()
    );
    sqlx::query("DROP TRIGGER suppress_recovery_state")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        LiveOwnershipRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        LiveOwnershipRow::has_claim(&pool, source_owner.id, CLAIM_KEY)
            .await
            .unwrap()
    );

    let completion_pool = crate::connect_in_memory().await.unwrap();
    let (run, source_owner, recovery_owner, attempt) =
        orphaned_recovery_fixture(&completion_pool).await;
    assert!(
        TuneRecoveryAttemptRow::complete_with_ownership(
            &completion_pool,
            CompleteRecoveryUpdate {
                attempt_id: attempt.id + 1,
                recovery_owner_id: recovery_owner.id,
                source_owner_id: source_owner.id,
                run_id: run.id,
                claim_key: CLAIM_KEY,
                status: RecoveryAttemptStatus::Confirmed,
                now: Utc::now(),
                detail: None,
                evidence_json: "{}",
            },
        )
        .await
        .is_err()
    );
    assert_eq!(
        TuneRecoveryAttemptRow::list_for_run(&completion_pool, run.id)
            .await
            .unwrap()
            .first()
            .unwrap()
            .status,
        RecoveryAttemptStatus::Running
    );
    assert!(
        LiveOwnershipRow::has_claim(&completion_pool, recovery_owner.id, CLAIM_KEY)
            .await
            .unwrap()
    );

    sqlx::query("UPDATE tune_runs SET outcome = 'completed' WHERE id = ?")
        .bind(run.id)
        .execute(&completion_pool)
        .await
        .unwrap();
    assert!(
        TuneRecoveryAttemptRow::complete_with_ownership(
            &completion_pool,
            CompleteRecoveryUpdate {
                attempt_id: attempt.id,
                recovery_owner_id: recovery_owner.id,
                source_owner_id: source_owner.id,
                run_id: run.id,
                claim_key: CLAIM_KEY,
                status: RecoveryAttemptStatus::Confirmed,
                now: Utc::now(),
                detail: None,
                evidence_json: "{}",
            },
        )
        .await
        .is_err()
    );
    assert_eq!(
        TuneRecoveryAttemptRow::list_for_run(&completion_pool, run.id)
            .await
            .unwrap()
            .first()
            .unwrap()
            .status,
        RecoveryAttemptStatus::Running
    );
}

#[tokio::test]
async fn recovery_completion_rolls_back_when_any_owner_or_claim_transition_is_rejected() {
    for failure in ["recovery_owner", "source_owner", "claim"] {
        let pool = crate::connect_in_memory().await.unwrap();
        let (run, source_owner, recovery_owner, attempt) = orphaned_recovery_fixture(&pool).await;
        if failure == "recovery_owner" {
            sqlx::query(
                    "CREATE TRIGGER reject_recovery_owner_release AFTER UPDATE OF recovery_state ON tune_runs WHEN NEW.recovery_state = 'confirmed' BEGIN UPDATE live_operation_owners SET state = 'released', released_at = NEW.completed_at WHERE operation_kind = 'recovery' AND state = 'active' AND run_id = NEW.id; END",
                )
                .execute(&pool)
                .await
                .unwrap();
        }
        if failure == "source_owner" {
            sqlx::query(
                    "CREATE TRIGGER reject_source_owner_recovery AFTER UPDATE OF recovery_state ON tune_runs WHEN NEW.recovery_state = 'confirmed' BEGIN UPDATE live_operation_owners SET orphan_eligible = 0 WHERE operation_kind = 'tune' AND state = 'orphaned' AND run_id = NEW.id; END",
                )
                .execute(&pool)
                .await
                .unwrap();
        }
        if failure == "claim" {
            sqlx::query(
                    "CREATE TRIGGER reject_recovery_claim_delete BEFORE DELETE ON live_operation_claims BEGIN SELECT RAISE(IGNORE); END",
                )
                .execute(&pool)
                .await
                .unwrap();
        }

        assert!(
            TuneRecoveryAttemptRow::complete_with_ownership(
                &pool,
                CompleteRecoveryUpdate {
                    attempt_id: attempt.id,
                    recovery_owner_id: recovery_owner.id,
                    source_owner_id: source_owner.id,
                    run_id: run.id,
                    claim_key: CLAIM_KEY,
                    status: RecoveryAttemptStatus::Confirmed,
                    now: Utc::now(),
                    detail: Some("injected transition failure"),
                    evidence_json: "{}",
                },
            )
            .await
            .is_err()
        );
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            RecoveryAttemptStatus::Running
        );
    }
}

#[tokio::test]
async fn restart_recovery_rolls_back_when_run_owner_or_claim_transition_is_rejected() {
    for failure in ["run", "recovery_owner", "claim"] {
        let pool = crate::connect_in_memory().await.unwrap();
        let (run, source_owner, recovery_owner, attempt) = orphaned_recovery_fixture(&pool).await;
        if failure == "run" {
            sqlx::query("UPDATE tune_runs SET recovery_state = 'incomplete' WHERE id = ?")
                .bind(run.id)
                .execute(&pool)
                .await
                .unwrap();
        }
        if failure == "claim" {
            sqlx::query(
                    "CREATE TRIGGER reject_recovery_claim_transfer BEFORE UPDATE OF owner_id ON live_operation_claims BEGIN SELECT RAISE(IGNORE); END",
                )
                .execute(&pool)
                .await
                .unwrap();
        }
        let expected_heartbeat = if failure == "recovery_owner" {
            recovery_owner.heartbeat_at - chrono::Duration::seconds(1)
        } else {
            recovery_owner.heartbeat_at
        };

        assert!(
            !TuneRecoveryAttemptRow::abandon_after_restart(
                &pool,
                RestartRecoveryUpdate {
                    attempt_id: attempt.id,
                    recovery_owner_id: recovery_owner.id,
                    expected_heartbeat,
                    source_owner_id: source_owner.id,
                    run_id: run.id,
                    claim_key: CLAIM_KEY,
                    now: Utc::now(),
                    detail: "injected restart transition failure",
                    evidence_json: "{}",
                },
            )
            .await
            .unwrap()
        );
        assert_eq!(
            TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
                .await
                .unwrap()
                .first()
                .unwrap()
                .status,
            RecoveryAttemptStatus::Running
        );
    }
}

#[tokio::test]
async fn restart_recovery_requires_the_exact_running_attempt() {
    let pool = crate::connect_in_memory().await.unwrap();
    let (run, source_owner, recovery_owner, attempt) = orphaned_recovery_fixture(&pool).await;
    let update = |attempt_id| RestartRecoveryUpdate {
        attempt_id,
        recovery_owner_id: recovery_owner.id,
        expected_heartbeat: recovery_owner.heartbeat_at,
        source_owner_id: source_owner.id,
        run_id: run.id,
        claim_key: CLAIM_KEY,
        now: Utc::now(),
        detail: "recovery process interrupted",
        evidence_json: r#"{"event":"interrupted_recovery_restart"}"#,
    };
    assert!(
        !TuneRecoveryAttemptRow::abandon_after_restart(&pool, update(attempt.id + 1))
            .await
            .unwrap()
    );
    assert!(
        TuneRecoveryAttemptRow::abandon_after_restart(&pool, update(attempt.id))
            .await
            .unwrap()
    );
    assert_eq!(
        TuneRecoveryAttemptRow::list_for_run(&pool, run.id)
            .await
            .unwrap()
            .pop()
            .unwrap()
            .status,
        RecoveryAttemptStatus::Incomplete
    );
    assert!(
        LiveOwnershipRow::has_claim(&pool, source_owner.id, CLAIM_KEY)
            .await
            .unwrap()
    );
}
