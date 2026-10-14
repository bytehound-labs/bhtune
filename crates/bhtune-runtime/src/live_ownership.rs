//! Process-shared and database-backed ownership for live controller mutations.

use std::{
    collections::{HashMap, VecDeque},
    fs::{File, OpenOptions},
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bhtune_db::{
    DbError, SqlitePool, database_path,
    models::{
        LiveMutationStepRow, LiveOperationKind, LiveOwnershipRow, MutationStepStatus,
        NewLiveMutationStep, NewLiveOwnership, TuneDriver, TuneRunRow,
    },
};
use bhtune_driver::{
    BrowsePage, BrowsePageRequest, Driver, DriverError, DriverResult, TagId, TagValue, TagWrite,
    WriteOutcome,
};
use chrono::Utc;
use fs2::FileExt;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum LiveOwnershipAcquireError {
    #[error("another live controller operation holds this database's OS ownership lock")]
    LockBusy,
    #[error("the canonical live controller resource is already claimed")]
    ResourceClaimed,
    #[error(transparent)]
    Persistence(#[from] anyhow::Error),
}

/// An OS lock for one canonical SQLite database. The lock file's existence is not evidence
/// of ownership; only the held kernel lock prevents another process from acquiring it.
#[derive(Debug)]
pub struct DatabaseFileGuard {
    _file: File,
    database_key: String,
    #[cfg(test)]
    memory_lock_file: Option<Arc<tempfile::NamedTempFile>>,
}

impl DatabaseFileGuard {
    /// Attempts to acquire the database's nonblocking, process-shared exclusive lock.
    pub async fn try_acquire(pool: &SqlitePool) -> anyhow::Result<Option<Self>> {
        let path = database_path(pool).await;
        #[cfg(test)]
        let pool_identity = std::ptr::from_ref(pool) as usize;
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            let mut memory_lock_file = None;
            let (lock_path, database_key) = match path {
                Ok(path) => {
                    let canonical_path = canonicalize_database_path(&path)?;
                    let database_key = database_key_from_canonical_path(&canonical_path)?;
                    let mut lock_path = canonical_path.as_os_str().to_os_string();
                    lock_path.push(".live-owner.lock");
                    (PathBuf::from(lock_path), database_key)
                }
                #[cfg(test)]
                Err(DbError::DatabasePathUnavailable) => {
                    let file = tests::memory_database_lock_file(pool_identity)?;
                    let path = file.path().to_path_buf();
                    memory_lock_file = Some(file);
                    (path, format!("test-memory:{pool_identity:x}"))
                }
                Err(error) => return Err(error.into()),
            };
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_path)
                .map_err(|error| {
                    anyhow::anyhow!("cannot open the live-operation lock file: {error}")
                })?;
            match classify_lock_result(FileExt::try_lock_exclusive(&file))? {
                true => Ok(Some(Self {
                    _file: file,
                    database_key,
                    #[cfg(test)]
                    memory_lock_file,
                })),
                false => Ok(None),
            }
        })
        .await?
    }

    pub fn database_key(&self) -> &str {
        &self.database_key
    }
}

/// A held OS lock paired with the conditional SQLite owner/claim and an independent
/// five-second heartbeat. Dropping it releases the OS lock; successful completion must call
/// [`Self::release`] so the database owner and claim are also durably retired.
pub struct LiveOperationGuard {
    file_guard: DatabaseFileGuard,
    owner: LiveOwnershipRow,
    claim_key: String,
    pool: SqlitePool,
    heartbeat_failed: Arc<AtomicBool>,
    previous_write_values: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    heartbeat_task: Option<JoinHandle<()>>,
}

/// A `Driver` decorator that commits a durable per-write intent before forwarding any live
/// mutation and records whether the transport accepted or rejected it afterward.
pub struct AuditedDriver<'a> {
    inner: &'a dyn Driver,
    pool: &'a SqlitePool,
    guard: &'a LiveOperationGuard,
    run_id: Option<i64>,
}

impl<'a> AuditedDriver<'a> {
    pub fn new(
        inner: &'a dyn Driver,
        pool: &'a SqlitePool,
        guard: &'a LiveOperationGuard,
        run_id: Option<i64>,
    ) -> Self {
        Self {
            inner,
            pool,
            guard,
            run_id,
        }
    }

    pub fn run_id(&self) -> Option<i64> {
        self.run_id
    }

    pub fn owner(&self) -> &LiveOperationGuard {
        self.guard
    }
}

#[async_trait::async_trait]
impl Driver for AuditedDriver<'_> {
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        self.inner.read(tags).await
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
        self.guard.ensure_healthy().map_err(|error| {
            DriverError::Operation(Box::new(io::Error::other(error.to_string())))
        })?;
        let target_json = write_target_json(tag, &value).map_err(|error| {
            DriverError::Operation(Box::new(io::Error::other(error.to_string())))
        })?;
        let step = LiveMutationStepRow::begin(
            self.pool,
            NewLiveMutationStep {
                owner_id: self.guard.owner.id,
                run_id: self.run_id,
                step: "controller_write".to_string(),
                target_json,
                previous_json: self.guard.take_previous_write_value(tag).await,
                started_at: Utc::now(),
            },
        )
        .await
        .map_err(db_driver_error)?;

        let result = self.inner.write(tag, value).await;
        let (status, detail) = match &result {
            Ok(outcome) if outcome.success => (
                MutationStepStatus::Confirmed,
                Some(
                    "the driver accepted the write; value confirmation is recorded separately"
                        .to_string(),
                ),
            ),
            Ok(outcome) => (
                MutationStepStatus::Failed,
                Some(
                    outcome
                        .error_message
                        .clone()
                        .unwrap_or_else(|| "the driver rejected the write".to_string()),
                ),
            ),
            Err(error) => (MutationStepStatus::Failed, Some(error.to_string())),
        };
        let audit_result = LiveMutationStepRow::finish(
            self.pool,
            step.id,
            status,
            Utc::now(),
            None,
            detail.as_deref(),
        )
        .await;
        if let Err(error) = audit_result {
            return Err(db_driver_error(error));
        }
        result
    }

    async fn browse(&self, request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        self.inner.browse(request).await
    }
}

fn write_target_json(tag: &str, value: &TagWrite) -> anyhow::Result<String> {
    let value = match value {
        TagWrite::Float(value) => {
            let number = serde_json::Number::from_f64(f64::from(*value))
                .ok_or_else(|| anyhow::anyhow!("controller write target is not finite"))?;
            Value::Number(number)
        }
        TagWrite::Raw(value) => Value::String(value.clone()),
    };
    serde_json::to_string(&json!({"tag": tag, "value": value}))
        .map_err(|error| anyhow::anyhow!("failed to serialize controller write target: {error}"))
}

fn db_driver_error(error: DbError) -> DriverError {
    DriverError::Operation(Box::new(error))
}

impl LiveOperationGuard {
    /// Acquires the OS lock first, then creates the owner and atomically claims its canonical
    /// database/controller/tag key. Recovery uses a separate transaction that also starts
    /// the recovery attempt and records its evidence.
    #[allow(clippy::too_many_arguments)]
    pub async fn acquire(
        pool: &SqlitePool,
        run_id: Option<i64>,
        operation_kind: LiveOperationKind,
        bridge_host: &str,
        server: &str,
        resource_tag: &str,
        restore_intent_json: Option<String>,
    ) -> Result<Self, LiveOwnershipAcquireError> {
        let file_guard = Self::try_acquire_database_file(pool)
            .await
            .map_err(LiveOwnershipAcquireError::Persistence)?
            .ok_or(LiveOwnershipAcquireError::LockBusy)?;
        let database_key = file_guard.database_key().to_owned();
        let resource_key = resource_key(bridge_host, server, resource_tag)
            .map_err(LiveOwnershipAcquireError::Persistence)?;
        let claim_key = claim_key(&database_key, &resource_key)
            .map_err(LiveOwnershipAcquireError::Persistence)?;
        let now = Utc::now();
        let owner = LiveOwnershipRow::create_and_claim(
            pool,
            NewLiveOwnership {
                run_id,
                operation_kind,
                database_key,
                resource_key,
                owner_pid: i64::from(std::process::id()),
                acquired_at: now,
                restore_intent_json,
            },
            &claim_key,
        )
        .await
        .map_err(|error| LiveOwnershipAcquireError::Persistence(error.into()))?
        .ok_or(LiveOwnershipAcquireError::ResourceClaimed)?;
        Ok(Self::start_heartbeat(pool, file_guard, owner, claim_key))
    }

    /// Acquires ownership for a post-run write or revert using only the connection and MV
    /// recorded on `run`. Explicit caller values must be cross-checked before this function;
    /// they are never used to select a different controller.
    pub async fn acquire_for_recorded_run(
        pool: &SqlitePool,
        run: &TuneRunRow,
        operation_kind: LiveOperationKind,
        restore_intent_json: String,
    ) -> Result<Self, LiveOwnershipAcquireError> {
        if run.driver != TuneDriver::Opcda {
            return Err(LiveOwnershipAcquireError::Persistence(anyhow::anyhow!(
                "run {} used the {:?} driver and has no live OPC DA operation to own",
                run.id,
                run.driver
            )));
        }
        let bridge_host = run.bridge_host.as_deref().ok_or_else(|| {
            LiveOwnershipAcquireError::Persistence(anyhow::anyhow!(
                "run {} has no recorded bridge host; refusing to guess which gateway to use",
                run.id
            ))
        })?;
        let server = run.opc_server.as_deref().ok_or_else(|| {
            LiveOwnershipAcquireError::Persistence(anyhow::anyhow!(
                "run {} has no recorded OPC server; refusing to guess which controller to use",
                run.id
            ))
        })?;
        let resource_tag = run.tags.manipulated_variable.as_str();
        Self::acquire(
            pool,
            Some(run.id),
            operation_kind,
            bridge_host,
            server,
            resource_tag,
            Some(restore_intent_json),
        )
        .await
    }

    /// Returns a database-file guard without creating a database owner. Full-mode startup
    /// uses this to distinguish a stale heartbeat from a still-running, possibly paused owner.
    pub async fn try_acquire_database_file(
        pool: &SqlitePool,
    ) -> anyhow::Result<Option<DatabaseFileGuard>> {
        DatabaseFileGuard::try_acquire(pool).await
    }

    pub(crate) fn from_database_owner(
        pool: &SqlitePool,
        file_guard: DatabaseFileGuard,
        owner: LiveOwnershipRow,
        claim_key: String,
    ) -> Self {
        Self::start_heartbeat(pool, file_guard, owner, claim_key)
    }

    fn start_heartbeat(
        pool: &SqlitePool,
        file_guard: DatabaseFileGuard,
        owner: LiveOwnershipRow,
        claim_key: String,
    ) -> Self {
        let heartbeat_failed = Arc::new(AtomicBool::new(false));
        let previous_write_values = Arc::new(Mutex::new(HashMap::new()));
        let task_failed = Arc::clone(&heartbeat_failed);
        let task_pool = pool.clone();
        let owner_id = owner.id;
        let heartbeat_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(error) =
                    LiveOwnershipRow::heartbeat(&task_pool, owner_id, Utc::now()).await
                {
                    task_failed.store(true, Ordering::Release);
                    tracing::error!(
                        owner_id,
                        error = %error,
                        "live controller owner heartbeat failed"
                    );
                    break;
                }
            }
        });
        Self {
            file_guard,
            owner,
            claim_key,
            pool: pool.clone(),
            heartbeat_failed,
            previous_write_values,
            heartbeat_task: Some(heartbeat_task),
        }
    }

    pub fn owner(&self) -> &LiveOwnershipRow {
        &self.owner
    }

    pub fn claim_key(&self) -> &str {
        &self.claim_key
    }

    pub fn database_key(&self) -> &str {
        self.file_guard.database_key()
    }

    /// Prevents a caller from starting another controller mutation after an asynchronous
    /// database-heartbeat failure. An already-issued driver call remains protected by the OS
    /// lock until its owning future finishes or the process exits.
    pub fn ensure_healthy(&self) -> anyhow::Result<()> {
        if self.heartbeat_failed.load(Ordering::Acquire) {
            anyhow::bail!(
                "live controller ownership heartbeat failed; refusing further mutations and \
                 retaining the pre-write audit intent"
            );
        }
        Ok(())
    }

    /// Queues the value observed before the next write to `tag`. The audited driver consumes
    /// it when it records the write intent, preserving correct pre-write data even for
    /// write-and-rollback sequences on the same tag.
    pub async fn queue_previous_write_value(&self, tag: &str, value_json: String) {
        self.previous_write_values
            .lock()
            .await
            .entry(tag.to_owned())
            .or_default()
            .push_back(value_json);
    }

    pub async fn clear_previous_write_values(&self) {
        self.previous_write_values.lock().await.clear();
    }

    async fn take_previous_write_value(&self, tag: &str) -> Option<String> {
        let mut values = self.previous_write_values.lock().await;
        let by_tag = values.get_mut(tag)?;
        let value = by_tag.pop_front();
        if by_tag.is_empty() {
            values.remove(tag);
        }
        value
    }

    /// Replaces the pre-mutation recovery intent with a more complete, durably persisted
    /// snapshot. Failure prevents the caller from issuing its next live write.
    pub async fn persist_restore_intent(&self, intent_json: &str) -> anyhow::Result<()> {
        self.ensure_healthy()?;
        LiveOwnershipRow::set_restore_intent(&self.pool, self.owner.id, intent_json).await?;
        Ok(())
    }

    /// Retires the persisted owner/claim before releasing the OS lock. A database persistence
    /// failure is returned to the caller and must never be reported as successful completion.
    pub async fn release(mut self) -> anyhow::Result<()> {
        self.stop_heartbeat().await;
        self.ensure_healthy()?;
        LiveOwnershipRow::release(&self.pool, self.owner.id, Utc::now()).await?;
        Ok(())
    }

    /// Stops heartbeat updates while a recovery attempt atomically finalizes both owners and
    /// the claim. The caller must keep this guard alive until that transaction succeeds or
    /// fails.
    pub async fn stop_heartbeat(&mut self) {
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for LiveOperationGuard {
    fn drop(&mut self) {
        if let Some(task) = self.heartbeat_task.take() {
            task.abort();
        }
    }
}

/// The canonical resource key is independent of the run ID. Host and ProgID comparisons are
/// case-insensitive; the OPC ItemID remains exact because OPC namespaces may be case-sensitive.
pub fn resource_key(bridge_host: &str, server: &str, resource_tag: &str) -> anyhow::Result<String> {
    serde_json::to_string(&(
        bridge_host.to_ascii_lowercase(),
        server.to_ascii_lowercase(),
        resource_tag,
    ))
    .map_err(|error| anyhow::anyhow!("failed to serialize the live resource identity: {error}"))
}

pub fn claim_key(database_key: &str, resource_key: &str) -> anyhow::Result<String> {
    serde_json::to_string(&(database_key, resource_key))
        .map_err(|error| anyhow::anyhow!("failed to serialize the live claim identity: {error}"))
}

fn lock_is_contended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || error
            .raw_os_error()
            .is_some_and(|code| fs2::lock_contended_error().raw_os_error() == Some(code))
}

fn classify_lock_result(result: io::Result<()>) -> anyhow::Result<bool> {
    match result {
        Ok(()) => Ok(true),
        Err(error) if lock_is_contended(&error) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn database_key_from_canonical_path(path: &std::path::Path) -> anyhow::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("database path is not valid UTF-8"))
}

fn canonicalize_database_path(path: &std::path::Path) -> anyhow::Result<PathBuf> {
    std::fs::canonicalize(path).map_err(|error| {
        anyhow::anyhow!(
            "cannot canonicalize the live-operation database path {}: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests;
