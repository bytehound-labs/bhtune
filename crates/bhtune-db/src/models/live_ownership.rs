//! Durable ownership, mutation, and interrupted-run recovery evidence for live OPC DA work.

use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};

use crate::{
    convert::{enum_to_text, text_to_enum},
    error::{DbError, DbResult},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveOperationKind {
    Tune,
    PidWrite,
    PidRevert,
    Recovery,
    OpcWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveOwnershipState {
    Active,
    Released,
    Orphaned,
    Recovered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStepStatus {
    Intent,
    Confirmed,
    Failed,
    NotNeeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAttemptStatus {
    Running,
    Confirmed,
    Incomplete,
    Failed,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NewLiveOwnership {
    pub run_id: Option<i64>,
    pub operation_kind: LiveOperationKind,
    pub database_key: String,
    pub resource_key: String,
    pub owner_pid: i64,
    pub acquired_at: DateTime<Utc>,
    pub restore_intent_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LiveOwnershipRow {
    pub id: i64,
    pub run_id: Option<i64>,
    pub operation_kind: LiveOperationKind,
    pub database_key: String,
    pub resource_key: String,
    pub owner_pid: i64,
    pub acquired_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub state: LiveOwnershipState,
    pub restore_intent_json: Option<String>,
    pub orphan_eligible: bool,
    pub orphan_evidence_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct LiveMutationStepRow {
    pub id: i64,
    pub owner_id: i64,
    pub run_id: Option<i64>,
    pub step: String,
    pub target_json: String,
    pub previous_json: Option<String>,
    pub status: MutationStepStatus,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub readback_json: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NewLiveMutationStep {
    pub owner_id: i64,
    pub run_id: Option<i64>,
    pub step: String,
    pub target_json: String,
    pub previous_json: Option<String>,
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TuneRecoveryAttemptRow {
    pub id: i64,
    pub run_id: i64,
    pub source_owner_id: i64,
    pub recovery_owner_id: i64,
    pub export_path: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: RecoveryAttemptStatus,
    pub detail: Option<String>,
    pub evidence_json: String,
}

pub struct OrphanOwnerUpdate<'a> {
    pub owner_id: i64,
    pub expected_heartbeat: DateTime<Utc>,
    pub expected_claim_key: &'a str,
    pub now: DateTime<Utc>,
    pub evidence_json: &'a str,
    pub eligible: bool,
    pub detail: &'a str,
}

pub struct UnrecoverableOwnerUpdate<'a> {
    pub owner_id: i64,
    pub expected_heartbeat: DateTime<Utc>,
    pub expected_claim_key: &'a str,
    pub now: DateTime<Utc>,
    pub evidence_json: &'a str,
    pub fail_running_run_id: Option<i64>,
    pub detail: &'a str,
}

pub struct CompleteRecoveryUpdate<'a> {
    pub attempt_id: i64,
    pub recovery_owner_id: i64,
    pub source_owner_id: i64,
    pub run_id: i64,
    pub claim_key: &'a str,
    pub status: RecoveryAttemptStatus,
    pub now: DateTime<Utc>,
    pub detail: Option<&'a str>,
    pub evidence_json: &'a str,
}

pub struct RestartRecoveryUpdate<'a> {
    pub attempt_id: i64,
    pub recovery_owner_id: i64,
    pub expected_heartbeat: DateTime<Utc>,
    pub source_owner_id: i64,
    pub run_id: i64,
    pub claim_key: &'a str,
    pub now: DateTime<Utc>,
    pub detail: &'a str,
    pub evidence_json: &'a str,
}

impl LiveOwnershipRow {
    /// Inserts an owner and conditionally claims its canonical database/resource key in one
    /// transaction. Recovery claim transfer is handled atomically by
    /// `TuneRecoveryAttemptRow::start_with_claim`.
    pub async fn create_and_claim(
        pool: &SqlitePool,
        new: NewLiveOwnership,
        claim_key: &str,
    ) -> DbResult<Option<Self>> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let row = sqlx::query(
            r"
            INSERT INTO live_operation_owners (
                run_id, operation_kind, database_key, resource_key, owner_pid,
                acquired_at, heartbeat_at, state, restore_intent_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?)
            RETURNING *
            ",
        )
        .bind(new.run_id)
        .bind(enum_to_text(&new.operation_kind)?)
        .bind(new.database_key)
        .bind(new.resource_key)
        .bind(new.owner_pid)
        .bind(new.acquired_at)
        .bind(new.acquired_at)
        .bind(new.restore_intent_json)
        .fetch_one(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        let owner = row_to_live_ownership(row)?;

        let claim_result = sqlx::query(
            r"
            INSERT INTO live_operation_claims (claim_key, owner_id, claimed_at)
            VALUES (?, ?, ?)
            ON CONFLICT(claim_key) DO NOTHING
            ",
        )
        .bind(claim_key)
        .bind(owner.id)
        .bind(new.acquired_at)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;

        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(None);
        }
        transaction.commit().await.map_err(DbError::Query)?;
        Ok(Some(owner))
    }

    pub async fn heartbeat(pool: &SqlitePool, owner_id: i64, now: DateTime<Utc>) -> DbResult<()> {
        let result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET heartbeat_at = ?
            WHERE id = ? AND state = 'active'
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims WHERE owner_id = ?
                )
            ",
        )
        .bind(now)
        .bind(owner_id)
        .bind(owner_id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        if result.rows_affected() != 1 {
            return Err(DbError::LiveOwnershipLost);
        }
        Ok(())
    }

    pub async fn release(pool: &SqlitePool, owner_id: i64, now: DateTime<Utc>) -> DbResult<()> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET state = 'released', released_at = ?
            WHERE id = ? AND state = 'active'
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims WHERE owner_id = ?
                )
            ",
        )
        .bind(now)
        .bind(owner_id)
        .bind(owner_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }
        let claim_result = sqlx::query("DELETE FROM live_operation_claims WHERE owner_id = ?")
            .bind(owner_id)
            .execute(&mut *transaction)
            .await
            .map_err(DbError::Query)?;
        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }
        transaction.commit().await.map_err(DbError::Query)
    }

    pub async fn get(pool: &SqlitePool, id: i64) -> DbResult<Option<Self>> {
        let row = sqlx::query("SELECT * FROM live_operation_owners WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .map_err(DbError::Query)?;
        row.map(row_to_live_ownership).transpose()
    }

    pub async fn has_claim(
        pool: &SqlitePool,
        owner_id: i64,
        expected_claim_key: &str,
    ) -> DbResult<bool> {
        let claimed: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM live_operation_claims WHERE owner_id = ? AND claim_key = ?)",
        )
        .bind(owner_id)
        .bind(expected_claim_key)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;
        Ok(claimed == 1)
    }

    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<Self>> {
        let rows = sqlx::query(
            "SELECT * FROM live_operation_owners WHERE run_id = ? ORDER BY acquired_at, id",
        )
        .bind(run_id)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_live_ownership).collect()
    }

    pub async fn stale_active_owners(
        pool: &SqlitePool,
        stale_before: DateTime<Utc>,
    ) -> DbResult<Vec<Self>> {
        let rows = sqlx::query(
            r"
            SELECT owners.*
            FROM live_operation_owners AS owners
            WHERE owners.state = 'active'
                AND owners.heartbeat_at < ?
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims
                    WHERE live_operation_claims.owner_id = owners.id
                )
            ORDER BY owners.heartbeat_at, owners.id
            ",
        )
        .bind(stale_before)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_live_ownership).collect()
    }

    pub async fn set_restore_intent(
        pool: &SqlitePool,
        owner_id: i64,
        intent_json: &str,
    ) -> DbResult<()> {
        let result = sqlx::query(
            r"
            UPDATE live_operation_owners SET restore_intent_json = ?
            WHERE id = ? AND state = 'active'
                AND EXISTS (SELECT 1 FROM live_operation_claims WHERE owner_id = ?)
            ",
        )
        .bind(intent_json)
        .bind(owner_id)
        .bind(owner_id)
        .execute(pool)
        .await
        .map_err(DbError::Query)?;
        if result.rows_affected() != 1 {
            return Err(DbError::LiveOwnershipLost);
        }
        Ok(())
    }

    pub async fn mark_orphaned(pool: &SqlitePool, update: OrphanOwnerUpdate<'_>) -> DbResult<bool> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET state = 'orphaned', released_at = ?, orphan_eligible = ?,
                orphan_evidence_json = ?
            WHERE id = ? AND state = 'active' AND heartbeat_at = ?
                AND operation_kind = 'tune'
                AND EXISTS (
                    SELECT 1 FROM tune_runs
                    WHERE tune_runs.id = live_operation_owners.run_id
                        AND tune_runs.driver = 'opcda'
                        AND tune_runs.outcome = 'running'
                )
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims
                    WHERE live_operation_claims.owner_id = live_operation_owners.id
                        AND live_operation_claims.claim_key = ?
                )
            ",
        )
        .bind(update.now)
        .bind(update.eligible)
        .bind(update.evidence_json)
        .bind(update.owner_id)
        .bind(update.expected_heartbeat)
        .bind(update.expected_claim_key)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        let state = if update.eligible {
            "eligible"
        } else {
            "not_recoverable"
        };
        let run_result = sqlx::query(
            r"
            UPDATE tune_runs
            SET outcome = 'failed', completed_at = ?, failure_reason = ?,
                restore_status = 'incomplete', restore_detail = ?,
                recovery_state = ?, recovery_evidence_json = ?
            WHERE id = (
                SELECT run_id FROM live_operation_owners WHERE id = ?
            ) AND outcome = 'running'
            ",
        )
        .bind(update.now)
        .bind(update.detail)
        .bind(update.detail)
        .bind(state)
        .bind(update.evidence_json)
        .bind(update.owner_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if run_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        if !update.eligible {
            sqlx::query("DELETE FROM live_operation_claims WHERE owner_id = ?")
                .bind(update.owner_id)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::Query)?;
        }
        transaction.commit().await.map_err(DbError::Query)?;
        Ok(true)
    }

    /// Retires a stale owner only after its caller has exported the exact affected rows.
    /// Any still-running row attached to the orphaned operation is failed closed rather than
    /// made automatically recoverable.
    pub async fn mark_orphaned_unrecoverable(
        pool: &SqlitePool,
        update: UnrecoverableOwnerUpdate<'_>,
    ) -> DbResult<bool> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let owner_result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET state = 'orphaned', released_at = ?, orphan_eligible = 0,
                orphan_evidence_json = ?
            WHERE id = ? AND state = 'active' AND heartbeat_at = ?
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims
                    WHERE owner_id = live_operation_owners.id AND claim_key = ?
                )
            ",
        )
        .bind(update.now)
        .bind(update.evidence_json)
        .bind(update.owner_id)
        .bind(update.expected_heartbeat)
        .bind(update.expected_claim_key)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if owner_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        if let Some(run_id) = update.fail_running_run_id {
            let run_result = sqlx::query(
                r"
                UPDATE tune_runs
                SET outcome = 'failed', completed_at = ?, failure_reason = ?,
                    restore_status = 'incomplete', restore_detail = ?,
                    recovery_state = 'not_recoverable', recovery_evidence_json = ?
                WHERE id = ? AND outcome = 'running'
                    AND EXISTS (
                        SELECT 1 FROM live_operation_owners
                        WHERE id = ? AND run_id = ? AND state = 'orphaned'
                    )
                ",
            )
            .bind(update.now)
            .bind(update.detail)
            .bind(update.detail)
            .bind(update.evidence_json)
            .bind(run_id)
            .bind(update.owner_id)
            .bind(run_id)
            .execute(&mut *transaction)
            .await
            .map_err(DbError::Query)?;
            if run_result.rows_affected() != 1 {
                transaction.rollback().await.map_err(DbError::Query)?;
                return Ok(false);
            }
        }

        let claim_result =
            sqlx::query("DELETE FROM live_operation_claims WHERE owner_id = ? AND claim_key = ?")
                .bind(update.owner_id)
                .bind(update.expected_claim_key)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::Query)?;
        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }
        transaction.commit().await.map_err(DbError::Query)?;
        Ok(true)
    }
}

impl LiveMutationStepRow {
    pub async fn begin(pool: &SqlitePool, new: NewLiveMutationStep) -> DbResult<Self> {
        let row = sqlx::query(
            r"
            INSERT INTO live_mutation_steps (
                owner_id, run_id, step, target_json, previous_json, status, started_at
            )
            SELECT ?, ?, ?, ?, ?, 'intent', ?
            WHERE EXISTS (
                SELECT 1 FROM live_operation_owners
                WHERE id = ? AND run_id IS ? AND state = 'active'
            )
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims WHERE owner_id = ?
                )
            RETURNING *
            ",
        )
        .bind(new.owner_id)
        .bind(new.run_id)
        .bind(new.step)
        .bind(new.target_json)
        .bind(new.previous_json)
        .bind(new.started_at)
        .bind(new.owner_id)
        .bind(new.run_id)
        .bind(new.owner_id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?
        .ok_or(DbError::LiveOwnershipLost)?;
        row_to_live_mutation_step(row)
    }

    pub async fn finish(
        pool: &SqlitePool,
        step_id: i64,
        status: MutationStepStatus,
        now: DateTime<Utc>,
        readback_json: Option<&str>,
        detail: Option<&str>,
    ) -> DbResult<Self> {
        if status == MutationStepStatus::Intent {
            return Err(DbError::LiveOwnershipLost);
        }
        let row = sqlx::query(
            r"
            UPDATE live_mutation_steps
            SET status = ?, completed_at = ?, readback_json = ?, detail = ?
            WHERE id = ? AND status = 'intent'
            RETURNING *
            ",
        )
        .bind(enum_to_text(&status)?)
        .bind(now)
        .bind(readback_json)
        .bind(detail)
        .bind(step_id)
        .fetch_one(pool)
        .await
        .map_err(DbError::Query)?;
        row_to_live_mutation_step(row)
    }

    pub async fn not_needed(
        pool: &SqlitePool,
        owner_id: i64,
        run_id: i64,
        step: &str,
        now: DateTime<Utc>,
        detail: &str,
    ) -> DbResult<Self> {
        let row = sqlx::query(
            r"
            INSERT INTO live_mutation_steps (
                owner_id, run_id, step, target_json, status, started_at, completed_at, detail
            )
            SELECT ?, ?, ?, '{}', 'not_needed', ?, ?, ?
            WHERE EXISTS (
                SELECT 1 FROM live_operation_owners
                WHERE id = ? AND run_id IS ? AND state = 'active'
            )
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims WHERE owner_id = ?
                )
            RETURNING *
            ",
        )
        .bind(owner_id)
        .bind(run_id)
        .bind(step)
        .bind(now)
        .bind(now)
        .bind(detail)
        .bind(owner_id)
        .bind(run_id)
        .bind(owner_id)
        .fetch_optional(pool)
        .await
        .map_err(DbError::Query)?
        .ok_or(DbError::LiveOwnershipLost)?;
        row_to_live_mutation_step(row)
    }

    pub async fn list_for_owner(pool: &SqlitePool, owner_id: i64) -> DbResult<Vec<Self>> {
        let rows = sqlx::query(
            "SELECT * FROM live_mutation_steps WHERE owner_id = ? ORDER BY started_at, id",
        )
        .bind(owner_id)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_live_mutation_step).collect()
    }

    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<Self>> {
        let rows = sqlx::query(
            "SELECT * FROM live_mutation_steps WHERE run_id = ? ORDER BY started_at, id",
        )
        .bind(run_id)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_live_mutation_step).collect()
    }
}

impl TuneRecoveryAttemptRow {
    pub async fn start_with_claim(
        pool: &SqlitePool,
        new_owner: NewLiveOwnership,
        claim_key: &str,
        source_owner_id: i64,
        run_id: i64,
        export_path: &str,
        evidence_json: &str,
    ) -> DbResult<Option<(LiveOwnershipRow, Self)>> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let owner_row = sqlx::query(
            r"
            INSERT INTO live_operation_owners (
                run_id, operation_kind, database_key, resource_key, owner_pid,
                acquired_at, heartbeat_at, state, restore_intent_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, 'active', ?)
            RETURNING *
            ",
        )
        .bind(new_owner.run_id)
        .bind(enum_to_text(&new_owner.operation_kind)?)
        .bind(&new_owner.database_key)
        .bind(&new_owner.resource_key)
        .bind(new_owner.owner_pid)
        .bind(new_owner.acquired_at)
        .bind(new_owner.acquired_at)
        .bind(&new_owner.restore_intent_json)
        .fetch_one(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        let recovery_owner = row_to_live_ownership(owner_row)?;

        let claim_result = sqlx::query(
            r"
            UPDATE live_operation_claims
            SET owner_id = ?, claimed_at = ?
            WHERE claim_key = ? AND owner_id = ?
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners
                    WHERE id = ? AND run_id = ? AND operation_kind = 'tune'
                        AND state = 'orphaned' AND orphan_eligible = 1
                        AND database_key = ? AND resource_key = ?
                )
            ",
        )
        .bind(recovery_owner.id)
        .bind(new_owner.acquired_at)
        .bind(claim_key)
        .bind(source_owner_id)
        .bind(source_owner_id)
        .bind(run_id)
        .bind(&new_owner.database_key)
        .bind(&new_owner.resource_key)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(None);
        }

        let attempt_row = sqlx::query(
            r"
            INSERT INTO tune_recovery_attempts (
                run_id, source_owner_id, recovery_owner_id, export_path, started_at,
                status, evidence_json
            )
            SELECT ?, ?, ?, ?, ?, 'running', ?
            WHERE EXISTS (
                SELECT 1 FROM live_operation_owners
                WHERE id = ? AND run_id = ? AND operation_kind = 'tune'
                    AND state = 'orphaned' AND orphan_eligible = 1
            )
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners
                    WHERE id = ? AND run_id = ? AND operation_kind = 'recovery'
                        AND state = 'active'
                )
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims
                    WHERE owner_id = ? AND claim_key = ?
                )
            RETURNING *
            ",
        )
        .bind(run_id)
        .bind(source_owner_id)
        .bind(recovery_owner.id)
        .bind(export_path)
        .bind(new_owner.acquired_at)
        .bind(evidence_json)
        .bind(source_owner_id)
        .bind(run_id)
        .bind(recovery_owner.id)
        .bind(run_id)
        .bind(recovery_owner.id)
        .bind(claim_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(DbError::Query)?
        .ok_or(DbError::LiveOwnershipLost)?;
        let attempt = row_to_recovery_attempt(attempt_row)?;

        let run_result = sqlx::query(
            r"
            UPDATE tune_runs
            SET recovery_state = 'running', recovery_evidence_json = ?
            WHERE id = ? AND outcome = 'failed'
                AND recovery_state IN ('eligible', 'incomplete')
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners
                    WHERE id = ? AND run_id = ? AND state = 'orphaned'
                        AND orphan_eligible = 1
                )
            ",
        )
        .bind(evidence_json)
        .bind(run_id)
        .bind(source_owner_id)
        .bind(run_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if run_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }

        transaction.commit().await.map_err(DbError::Query)?;
        Ok(Some((recovery_owner, attempt)))
    }

    pub async fn list_for_run(pool: &SqlitePool, run_id: i64) -> DbResult<Vec<Self>> {
        let rows = sqlx::query(
            "SELECT * FROM tune_recovery_attempts WHERE run_id = ? ORDER BY started_at, id",
        )
        .bind(run_id)
        .fetch_all(pool)
        .await
        .map_err(DbError::Query)?;
        rows.into_iter().map(row_to_recovery_attempt).collect()
    }

    pub async fn complete_with_ownership(
        pool: &SqlitePool,
        update: CompleteRecoveryUpdate<'_>,
    ) -> DbResult<()> {
        let (restore_status, recovery_state, owner_state, eligible, released_at) = match update
            .status
        {
            RecoveryAttemptStatus::Confirmed => (
                Some("confirmed"),
                "confirmed",
                "recovered",
                false,
                Some(update.now),
            ),
            RecoveryAttemptStatus::Incomplete => (
                Some("incomplete"),
                "incomplete",
                "orphaned",
                true,
                Some(update.now),
            ),
            RecoveryAttemptStatus::Failed => (None, "eligible", "orphaned", true, Some(update.now)),
            RecoveryAttemptStatus::Running => return Err(DbError::LiveOwnershipLost),
        };
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let attempt_result = sqlx::query(
            r"
            UPDATE tune_recovery_attempts
            SET status = ?, completed_at = ?, detail = ?, evidence_json = ?
            WHERE id = ? AND run_id = ? AND recovery_owner_id = ? AND status = 'running'
            ",
        )
        .bind(enum_to_text(&update.status)?)
        .bind(update.now)
        .bind(update.detail)
        .bind(update.evidence_json)
        .bind(update.attempt_id)
        .bind(update.run_id)
        .bind(update.recovery_owner_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if attempt_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }

        let run_result = sqlx::query(
            r"
            UPDATE tune_runs
            SET restore_status = COALESCE(?, restore_status),
                restore_detail = CASE WHEN ? IS NULL THEN restore_detail ELSE ? END,
                recovery_state = ?,
                recovery_evidence_json = ?
            WHERE id = ? AND outcome = 'failed'
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners
                    WHERE id = ? AND run_id = ? AND state = 'orphaned'
                        AND orphan_eligible = 1
                )
            ",
        )
        .bind(restore_status)
        .bind(restore_status)
        .bind(update.detail)
        .bind(recovery_state)
        .bind(update.evidence_json)
        .bind(update.run_id)
        .bind(update.source_owner_id)
        .bind(update.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if run_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }

        let recovery_owner_result = sqlx::query(
            "UPDATE live_operation_owners SET state = 'released', released_at = ? WHERE id = ? AND state = 'active'",
        )
        .bind(update.now)
        .bind(update.recovery_owner_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if recovery_owner_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }

        let source_owner_result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET state = ?, released_at = ?, orphan_eligible = ?
            WHERE id = ? AND run_id = ? AND state = 'orphaned' AND orphan_eligible = 1
            ",
        )
        .bind(owner_state)
        .bind(released_at)
        .bind(eligible)
        .bind(update.source_owner_id)
        .bind(update.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if source_owner_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }

        let claim_result =
            sqlx::query("DELETE FROM live_operation_claims WHERE claim_key = ? AND owner_id = ?")
                .bind(update.claim_key)
                .bind(update.recovery_owner_id)
                .execute(&mut *transaction)
                .await
                .map_err(DbError::Query)?;
        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Err(DbError::LiveOwnershipLost);
        }
        if update.status != RecoveryAttemptStatus::Confirmed {
            sqlx::query(
                "INSERT INTO live_operation_claims (claim_key, owner_id, claimed_at) VALUES (?, ?, ?)",
            )
            .bind(update.claim_key)
            .bind(update.source_owner_id)
            .bind(update.now)
            .execute(&mut *transaction)
            .await
            .map_err(DbError::Query)?;
        }
        transaction.commit().await.map_err(DbError::Query)
    }

    /// Rolls back only the database ownership handoff for a recovery process proven dead by
    /// the released OS lock and stale heartbeat. Controller state is never inspected or
    /// mutated here; the original eligible owner regains its claim for an explicit retry.
    pub async fn abandon_after_restart(
        pool: &SqlitePool,
        update: RestartRecoveryUpdate<'_>,
    ) -> DbResult<bool> {
        let mut transaction = pool.begin().await.map_err(DbError::Query)?;
        let attempt_result = sqlx::query(
            r"
            UPDATE tune_recovery_attempts
            SET status = 'incomplete', completed_at = ?, detail = ?, evidence_json = ?
            WHERE id = ? AND run_id = ? AND recovery_owner_id = ? AND status = 'running'
            ",
        )
        .bind(update.now)
        .bind(update.detail)
        .bind(update.evidence_json)
        .bind(update.attempt_id)
        .bind(update.run_id)
        .bind(update.recovery_owner_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if attempt_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        let run_result = sqlx::query(
            r"
            UPDATE tune_runs
            SET restore_status = 'incomplete', restore_detail = ?,
                recovery_state = 'incomplete', recovery_evidence_json = ?
            WHERE id = ? AND outcome = 'failed' AND recovery_state = 'running'
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners AS source
                    WHERE source.id = ? AND source.run_id = ? AND source.operation_kind = 'tune'
                        AND source.state = 'orphaned' AND source.orphan_eligible = 1
                )
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners AS recovery
                    WHERE recovery.id = ? AND recovery.run_id = ?
                        AND recovery.operation_kind = 'recovery' AND recovery.state = 'active'
                )
            ",
        )
        .bind(update.detail)
        .bind(update.evidence_json)
        .bind(update.run_id)
        .bind(update.source_owner_id)
        .bind(update.run_id)
        .bind(update.recovery_owner_id)
        .bind(update.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if run_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        let recovery_owner_result = sqlx::query(
            r"
            UPDATE live_operation_owners
            SET state = 'released', released_at = ?
            WHERE id = ? AND run_id = ? AND operation_kind = 'recovery'
                AND state = 'active' AND heartbeat_at = ?
                AND EXISTS (
                    SELECT 1 FROM live_operation_claims
                    WHERE owner_id = live_operation_owners.id AND claim_key = ?
                )
            ",
        )
        .bind(update.now)
        .bind(update.recovery_owner_id)
        .bind(update.run_id)
        .bind(update.expected_heartbeat)
        .bind(update.claim_key)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if recovery_owner_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        let claim_result = sqlx::query(
            r"
            UPDATE live_operation_claims
            SET owner_id = ?, claimed_at = ?
            WHERE claim_key = ? AND owner_id = ?
                AND EXISTS (
                    SELECT 1 FROM live_operation_owners AS source
                    WHERE source.id = ? AND source.run_id = ? AND source.operation_kind = 'tune'
                        AND source.state = 'orphaned' AND source.orphan_eligible = 1
                )
            ",
        )
        .bind(update.source_owner_id)
        .bind(update.now)
        .bind(update.claim_key)
        .bind(update.recovery_owner_id)
        .bind(update.source_owner_id)
        .bind(update.run_id)
        .execute(&mut *transaction)
        .await
        .map_err(DbError::Query)?;
        if claim_result.rows_affected() != 1 {
            transaction.rollback().await.map_err(DbError::Query)?;
            return Ok(false);
        }

        transaction.commit().await.map_err(DbError::Query)?;
        Ok(true)
    }
}

fn row_to_live_ownership(row: SqliteRow) -> DbResult<LiveOwnershipRow> {
    let operation_kind: String = row.try_get("operation_kind").map_err(DbError::Query)?;
    let state: String = row.try_get("state").map_err(DbError::Query)?;
    Ok(LiveOwnershipRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        operation_kind: text_to_enum("operation_kind", &operation_kind)?,
        database_key: row.try_get("database_key").map_err(DbError::Query)?,
        resource_key: row.try_get("resource_key").map_err(DbError::Query)?,
        owner_pid: row.try_get("owner_pid").map_err(DbError::Query)?,
        acquired_at: row.try_get("acquired_at").map_err(DbError::Query)?,
        heartbeat_at: row.try_get("heartbeat_at").map_err(DbError::Query)?,
        released_at: row.try_get("released_at").map_err(DbError::Query)?,
        state: text_to_enum("state", &state)?,
        restore_intent_json: row.try_get("restore_intent_json").map_err(DbError::Query)?,
        orphan_eligible: row.try_get("orphan_eligible").map_err(DbError::Query)?,
        orphan_evidence_json: row
            .try_get("orphan_evidence_json")
            .map_err(DbError::Query)?,
    })
}

fn row_to_live_mutation_step(row: SqliteRow) -> DbResult<LiveMutationStepRow> {
    let status: String = row.try_get("status").map_err(DbError::Query)?;
    Ok(LiveMutationStepRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        owner_id: row.try_get("owner_id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        step: row.try_get("step").map_err(DbError::Query)?,
        target_json: row.try_get("target_json").map_err(DbError::Query)?,
        previous_json: row.try_get("previous_json").map_err(DbError::Query)?,
        status: text_to_enum("status", &status)?,
        started_at: row.try_get("started_at").map_err(DbError::Query)?,
        completed_at: row.try_get("completed_at").map_err(DbError::Query)?,
        readback_json: row.try_get("readback_json").map_err(DbError::Query)?,
        detail: row.try_get("detail").map_err(DbError::Query)?,
    })
}

fn row_to_recovery_attempt(row: SqliteRow) -> DbResult<TuneRecoveryAttemptRow> {
    let status: String = row.try_get("status").map_err(DbError::Query)?;
    Ok(TuneRecoveryAttemptRow {
        id: row.try_get("id").map_err(DbError::Query)?,
        run_id: row.try_get("run_id").map_err(DbError::Query)?,
        source_owner_id: row.try_get("source_owner_id").map_err(DbError::Query)?,
        recovery_owner_id: row.try_get("recovery_owner_id").map_err(DbError::Query)?,
        export_path: row.try_get("export_path").map_err(DbError::Query)?,
        started_at: row.try_get("started_at").map_err(DbError::Query)?,
        completed_at: row.try_get("completed_at").map_err(DbError::Query)?,
        status: text_to_enum("status", &status)?,
        detail: row.try_get("detail").map_err(DbError::Query)?,
        evidence_json: row.try_get("evidence_json").map_err(DbError::Query)?,
    })
}

#[cfg(test)]
mod tests {
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
        let source_owner =
            create_owner(pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
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
            LiveOwnershipRow::stale_active_owners(
                &pool,
                Utc::now() - chrono::Duration::seconds(30)
            )
            .await
            .unwrap()
            .len(),
            1
        );
        LiveOwnershipRow::heartbeat(&pool, owner.id, Utc::now())
            .await
            .unwrap();
        assert!(
            LiveOwnershipRow::stale_active_owners(
                &pool,
                Utc::now() - chrono::Duration::seconds(30)
            )
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
            let (run, source_owner, recovery_owner, attempt) =
                orphaned_recovery_fixture(&pool).await;
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
        let source_owner =
            create_owner(&pool, Some(run.id), LiveOperationKind::Tune, CLAIM_KEY).await;
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
            let (run, source_owner, recovery_owner, attempt) =
                orphaned_recovery_fixture(&pool).await;
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
            let (run, source_owner, recovery_owner, attempt) =
                orphaned_recovery_fixture(&pool).await;
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
}
