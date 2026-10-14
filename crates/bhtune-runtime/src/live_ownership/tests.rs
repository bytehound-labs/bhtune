use super::*;
use async_trait::async_trait;
use std::sync::atomic::AtomicUsize;

pub(super) fn memory_database_lock_file(
    pool_identity: usize,
) -> io::Result<Arc<tempfile::NamedTempFile>> {
    type LockFiles = HashMap<usize, std::sync::Weak<tempfile::NamedTempFile>>;
    static FILES: std::sync::OnceLock<std::sync::Mutex<LockFiles>> = std::sync::OnceLock::new();
    let mut files = FILES.get_or_init(Default::default).lock().unwrap();
    files.retain(|_, file| file.strong_count() > 0);
    if let Some(file) = files.get(&pool_identity).and_then(std::sync::Weak::upgrade) {
        return Ok(file);
    }
    let file = Arc::new(
        tempfile::Builder::new()
            .prefix("bhtune-memory-owner-")
            .tempfile()?,
    );
    files.insert(pool_identity, Arc::downgrade(&file));
    Ok(file)
}

#[derive(Clone, Copy)]
enum WriteBehavior {
    Accepted,
    RejectedWithoutMessage,
    RejectedWithMessage,
    TransportError,
}

struct TestDriver {
    behavior: std::sync::Mutex<WriteBehavior>,
    write_calls: AtomicUsize,
}

impl TestDriver {
    fn new(behavior: WriteBehavior) -> Self {
        Self {
            behavior: std::sync::Mutex::new(behavior),
            write_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Driver for TestDriver {
    async fn read(&self, _tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        Err(DriverError::Unsupported { operation: "test" })
    }

    async fn write(&self, _tag: &TagId, _value: TagWrite) -> DriverResult<WriteOutcome> {
        self.write_calls.fetch_add(1, Ordering::Relaxed);
        match *self.behavior.lock().unwrap() {
            WriteBehavior::Accepted => Ok(WriteOutcome::success()),
            WriteBehavior::RejectedWithoutMessage => Ok(WriteOutcome {
                success: false,
                error_message: None,
            }),
            WriteBehavior::RejectedWithMessage => Ok(WriteOutcome::failure("simulated rejection")),
            WriteBehavior::TransportError => Err(DriverError::Operation(Box::new(
                io::Error::other("simulated transport failure"),
            ))),
        }
    }

    async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        Err(DriverError::Unsupported { operation: "test" })
    }
}

async fn test_owner(pool: &SqlitePool) -> LiveOperationGuard {
    LiveOperationGuard::acquire(
        pool,
        None,
        LiveOperationKind::OpcWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        "Simulation.Examples.MV",
        Some(r#"{"kind":"opc_write","state":"pending"}"#.to_string()),
    )
    .await
    .unwrap()
}

#[test]
fn resource_key_normalizes_host_and_server_but_preserves_item_id() {
    assert_eq!(
        resource_key("Gateway.EXAMPLE:7600", "Kepware.Server", "Area1.LIC101.MV").unwrap(),
        resource_key("gateway.example:7600", "kepware.server", "Area1.LIC101.MV").unwrap()
    );
    assert_ne!(
        resource_key("gateway.example:7600", "kepware.server", "Area1.LIC101.MV").unwrap(),
        resource_key("gateway.example:7600", "kepware.server", "area1.lic101.mv").unwrap()
    );
}

#[test]
fn claim_key_includes_the_canonical_database_identity() {
    assert_ne!(
        claim_key("/db/one.sqlite", "resource").unwrap(),
        claim_key("/db/two.sqlite", "resource").unwrap()
    );
}

#[test]
fn only_native_lock_contention_means_an_owned_lock_is_busy() {
    assert!(lock_is_contended(&io::Error::from(
        io::ErrorKind::WouldBlock
    )));
    assert!(!lock_is_contended(&io::Error::from(
        io::ErrorKind::PermissionDenied
    )));
    assert!(lock_is_contended(&fs2::lock_contended_error()));
    assert!(!lock_is_contended(&io::Error::from_raw_os_error(0)));
    assert!(classify_lock_result(Ok(())).unwrap());
    assert!(!classify_lock_result(Err(io::Error::from(io::ErrorKind::WouldBlock))).unwrap());
    assert!(classify_lock_result(Err(io::Error::from(io::ErrorKind::PermissionDenied))).is_err());
}

#[cfg(unix)]
#[test]
fn non_utf8_database_paths_fail_closed() {
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
    assert!(database_key_from_canonical_path(&path).is_err());
}

#[tokio::test]
async fn database_lock_reports_missing_database_and_unopenable_lock_file() {
    let directory = tempfile::tempdir().unwrap();
    let missing_path = directory.path().join("missing.db");
    let canonicalize_error = canonicalize_database_path(&missing_path).unwrap_err();
    assert!(
        canonicalize_error
            .to_string()
            .contains("cannot canonicalize")
    );

    let blocked_path = directory.path().join("blocked.db");
    let blocked_pool = bhtune_db::connect(&blocked_path).await.unwrap();
    let mut lock_path = blocked_path.as_os_str().to_os_string();
    lock_path.push(".live-owner.lock");
    std::fs::create_dir(PathBuf::from(lock_path)).unwrap();
    let open_error = DatabaseFileGuard::try_acquire(&blocked_pool)
        .await
        .unwrap_err();
    assert!(
        open_error
            .to_string()
            .contains("cannot open the live-operation lock file")
    );
    blocked_pool.close().await;
}

#[tokio::test]
async fn memory_database_guard_preserves_exclusion_and_reacquisition() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let first = DatabaseFileGuard::try_acquire(&pool)
        .await
        .unwrap()
        .unwrap();
    let database_key = first.database_key().to_owned();
    assert!(
        DatabaseFileGuard::try_acquire(&pool)
            .await
            .unwrap()
            .is_none()
    );
    drop(first);
    let second = DatabaseFileGuard::try_acquire(&pool)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.database_key(), database_key);
    drop(second);
    pool.close().await;
}

#[tokio::test]
async fn memory_database_guard_removes_its_private_lock_file() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let guard = DatabaseFileGuard::try_acquire(&pool)
        .await
        .unwrap()
        .unwrap();
    let path = guard
        .memory_lock_file
        .as_ref()
        .unwrap()
        .path()
        .to_path_buf();
    assert!(path.is_file());
    drop(guard);
    assert!(!path.exists());
    pool.close().await;
}

#[tokio::test]
async fn audited_driver_persists_intent_outcomes_and_prewrite_values() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let mut owner = test_owner(&pool).await;
    let driver = TestDriver::new(WriteBehavior::RejectedWithoutMessage);
    let audited = AuditedDriver::new(&driver, &pool, &owner, None);
    assert!(audited.read(&[]).await.is_err());
    assert!(
        audited
            .write(
                &"Simulation.Examples.MV".to_string(),
                TagWrite::Float(f32::NAN),
            )
            .await
            .is_err()
    );
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
    owner
        .queue_previous_write_value("Simulation.Examples.MV", "45".to_string())
        .await;
    owner.clear_previous_write_values().await;
    owner
        .queue_previous_write_value("Simulation.Examples.MV", "46".to_string())
        .await;

    let outcome = audited
        .write(
            &"Simulation.Examples.MV".to_string(),
            TagWrite::Raw("47".to_string()),
        )
        .await
        .unwrap();
    assert!(!outcome.success);
    assert_eq!(audited.run_id(), None);
    assert_eq!(audited.owner().database_key(), owner.database_key());
    let rows = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, MutationStepStatus::Failed);
    assert_eq!(rows[0].previous_json.as_deref(), Some("46"));
    assert_eq!(
        serde_json::from_str::<Value>(&rows[0].target_json).unwrap()["value"],
        "47"
    );
    assert!(
        rows[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("driver rejected the write")
    );

    *driver.behavior.lock().unwrap() = WriteBehavior::RejectedWithMessage;
    assert!(
        audited
            .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(48.0),)
            .await
            .is_ok()
    );
    *driver.behavior.lock().unwrap() = WriteBehavior::Accepted;
    audited
        .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(49.0))
        .await
        .unwrap();
    assert_eq!(
        LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
            .await
            .unwrap()
            .len(),
        3
    );
    *driver.behavior.lock().unwrap() = WriteBehavior::TransportError;
    assert!(
        audited
            .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(50.0),)
            .await
            .is_err()
    );
    assert_eq!(
        LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
            .await
            .unwrap()
            .len(),
        4
    );
    assert!(matches!(
        audited.browse(BrowsePageRequest::root(10)).await,
        Err(DriverError::Unsupported { operation: "test" })
    ));
    assert!(write_target_json("tag", &TagWrite::Float(f32::NAN)).is_err());
    assert!(write_target_json("tag", &TagWrite::Raw("value".to_string())).is_ok());
    owner.stop_heartbeat().await;
    owner.stop_heartbeat().await;
    drop(owner);
}

#[tokio::test]
async fn acquiring_post_run_ownership_requires_recorded_opc_provenance() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    use bhtune_core::{ControllerType, LoopConfig, LoopTags, ProcessType};
    use bhtune_db::models::TemplateOrigin;

    let template = bhtune_core::built_in_templates().remove(0);
    let tags = LoopTags::derive_from_pv_tag("Unit1.LIC101.PV", &template);
    let config = LoopConfig {
        process_type: ProcessType::Flow,
        controller_type: ControllerType::Pi,
        relay_amp_percent: 5.0,
        num_cycles_skip: 1,
        num_cycles_count: 2,
        noise_protection_secs: 3,
        mrft_delay_secs: 0,
    };
    let mut run = TuneRunRow::start(
        &pool,
        None,
        "Unit1.LIC101.PV",
        TuneDriver::Opcda,
        config,
        TemplateOrigin::Builtin,
        &template,
        &tags,
        Utc::now(),
    )
    .await
    .unwrap();

    run.driver = TuneDriver::Simulator;
    assert!(
        LiveOperationGuard::acquire_for_recorded_run(
            &pool,
            &run,
            LiveOperationKind::PidWrite,
            "{}".to_string(),
        )
        .await
        .err()
        .expect("simulator ownership must be rejected")
        .to_string()
        .contains("has no live OPC DA operation")
    );

    run.driver = TuneDriver::Opcda;
    run.bridge_host = None;
    assert!(
        LiveOperationGuard::acquire_for_recorded_run(
            &pool,
            &run,
            LiveOperationKind::PidWrite,
            "{}".to_string(),
        )
        .await
        .err()
        .expect("missing bridge provenance must be rejected")
        .to_string()
        .contains("no recorded bridge host")
    );

    run.bridge_host = Some("127.0.0.1:7602".to_string());
    run.opc_server = None;
    assert!(
        LiveOperationGuard::acquire_for_recorded_run(
            &pool,
            &run,
            LiveOperationKind::PidWrite,
            "{}".to_string(),
        )
        .await
        .err()
        .expect("missing server provenance must be rejected")
        .to_string()
        .contains("no recorded OPC server")
    );
}

#[tokio::test]
async fn owner_heartbeat_refreshes_without_a_driver_call() {
    let directory = tempfile::tempdir().unwrap();
    let pool = bhtune_db::connect(&directory.path().join("heartbeat.sqlite"))
        .await
        .unwrap();
    let owner = test_owner(&pool).await;
    let old_heartbeat = Utc::now() - chrono::Duration::hours(1);
    LiveOwnershipRow::heartbeat(&pool, owner.owner().id, old_heartbeat)
        .await
        .unwrap();

    tokio::time::sleep(HEARTBEAT_INTERVAL + Duration::from_secs(1)).await;

    let refreshed = LiveOwnershipRow::get(&pool, owner.owner().id)
        .await
        .unwrap()
        .unwrap();
    assert!(refreshed.heartbeat_at > old_heartbeat);
    owner.release().await.unwrap();
    pool.close().await;
}

#[tokio::test]
async fn audited_driver_stops_on_unhealthy_ownership_and_failed_audit_persistence() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let owner = test_owner(&pool).await;
    let driver = TestDriver::new(WriteBehavior::Accepted);
    let audited = AuditedDriver::new(&driver, &pool, &owner, None);

    owner.heartbeat_failed.store(true, Ordering::Release);
    assert!(
        audited
            .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(48.0),)
            .await
            .is_err()
    );
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
    owner.heartbeat_failed.store(false, Ordering::Release);

    sqlx::query(
            "CREATE TRIGGER reject_mutation_intent BEFORE INSERT ON live_mutation_steps BEGIN SELECT RAISE(FAIL, 'injected intent persistence failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        audited
            .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(49.0),)
            .await
            .is_err()
    );
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
    sqlx::query("DROP TRIGGER reject_mutation_intent")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
            "CREATE TRIGGER reject_mutation_completion BEFORE UPDATE ON live_mutation_steps BEGIN SELECT RAISE(FAIL, 'injected completion persistence failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        audited
            .write(&"Simulation.Examples.MV".to_string(), TagWrite::Float(50.0),)
            .await
            .is_err()
    );
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 1);
    let rows = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, MutationStepStatus::Intent);
    drop(owner);
}
