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
mod tests;
