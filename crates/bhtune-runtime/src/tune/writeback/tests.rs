use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use bhtune_core::{
    ControllerType, LoopConfig, LoopTags, ProcessType, TuningResultInvalidReason,
    TuningResultStatus,
};
use bhtune_db::models::{
    LiveMutationStepRow, LiveOperationKind, TemplateOrigin, TuneDriver, TuneRunRow,
};
use bhtune_driver::{
    BrowsePage, BrowsePageRequest, Driver, DriverError, DriverResult, Quality, TagId, TagValue,
    TagWrite, WriteOutcome,
};
use chrono::Utc;

use super::*;
use crate::live_ownership::AuditedDriver;

#[test]
fn pid_write_ownership_is_operation_specific() {
    assert!(operation_kind_authorizes_pid_write(
        WriteKind::Write,
        LiveOperationKind::Tune
    ));
    assert!(operation_kind_authorizes_pid_write(
        WriteKind::Write,
        LiveOperationKind::PidWrite
    ));
    assert!(operation_kind_authorizes_pid_write(
        WriteKind::Revert,
        LiveOperationKind::PidRevert
    ));
    assert!(!operation_kind_authorizes_pid_write(
        WriteKind::Write,
        LiveOperationKind::OpcWrite
    ));
    assert!(!operation_kind_authorizes_pid_write(
        WriteKind::Revert,
        LiveOperationKind::Tune
    ));
}

struct FailingDriver;

#[async_trait]
impl Driver for FailingDriver {
    async fn read(&self, _tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        Err(DriverError::Unsupported { operation: "test" })
    }

    async fn write(&self, _tag: &TagId, _value: TagWrite) -> DriverResult<WriteOutcome> {
        Err(DriverError::Unsupported { operation: "test" })
    }

    async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        Err(DriverError::Unsupported { operation: "test" })
    }
}

struct PidDriver {
    values: std::sync::Mutex<HashMap<String, String>>,
    write_counts: std::sync::Mutex<HashMap<String, usize>>,
    failure_calls: HashMap<String, Vec<usize>>,
    read_calls: AtomicUsize,
    write_calls: AtomicUsize,
}

impl PidDriver {
    fn new(tags: &LoopTags, failure_calls: HashMap<String, Vec<usize>>) -> Self {
        let mut values = HashMap::new();
        for (tag, value) in [
            (tags.proportional_constant.as_ref(), "1.0"),
            (tags.integral_constant.as_ref(), "2.0"),
            (tags.derivative_constant.as_ref(), "0.0"),
        ] {
            if let Some(tag) = tag {
                values.insert(tag.clone(), value.to_string());
            }
        }
        Self {
            values: std::sync::Mutex::new(values),
            write_counts: std::sync::Mutex::new(HashMap::new()),
            failure_calls,
            read_calls: AtomicUsize::new(0),
            write_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl Driver for PidDriver {
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        self.read_calls.fetch_add(1, Ordering::Relaxed);
        let values = self.values.lock().unwrap();
        tags.iter()
            .map(|tag| {
                let value = values.get(tag).cloned().ok_or_else(|| {
                    DriverError::Operation(Box::new(std::io::Error::other(
                        "test PID tag has no value",
                    )))
                })?;
                Ok(TagValue {
                    tag: tag.clone(),
                    value,
                    quality: Quality::Good,
                    timestamp: None,
                })
            })
            .collect()
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
        self.write_calls.fetch_add(1, Ordering::Relaxed);
        let call = {
            let mut counts = self.write_counts.lock().unwrap();
            let count = counts.entry(tag.clone()).or_default();
            *count += 1;
            *count
        };
        if self
            .failure_calls
            .get(tag)
            .is_some_and(|calls| calls.contains(&call))
        {
            return Ok(WriteOutcome::failure("simulated write rejection"));
        }
        let value = match value {
            TagWrite::Float(value) => value.to_string(),
            TagWrite::Raw(value) => value,
        };
        self.values.lock().unwrap().insert(tag.clone(), value);
        Ok(WriteOutcome::success())
    }

    async fn browse(&self, _request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        Err(DriverError::Unsupported { operation: "test" })
    }
}

struct RecordingHandler {
    selection: WriteBackSelection,
    events: Vec<String>,
}

impl RecordingHandler {
    fn new(selection: WriteBackSelection) -> Self {
        Self {
            selection,
            events: Vec::new(),
        }
    }
}

impl WriteBackHandler for RecordingHandler {
    fn select_response_level(&mut self, _previews: &[PidWritePreview]) -> WriteBackSelection {
        self.events.push("select".to_string());
        self.selection.clone()
    }

    fn write_back_selected(&mut self, values: &ControllerPidValues, requested: bool) {
        let level = values.response_level;
        self.events.push(format!("selected:{level:?}:{requested}"));
    }

    fn write_back_skipped(&mut self, reason: WriteBackSkipReason) {
        self.events.push(format!("skipped:{reason:?}"));
    }

    fn write_back_failed(&mut self, detail: &str) {
        self.events.push(format!("failed:{detail}"));
    }

    fn write_back_finished(&mut self, level: ResponseLevel, outcome: &PidWriteOutcome) {
        self.events.push(format!("finished:{level:?}:{outcome:?}"));
    }
}

struct DefaultHandler;

impl WriteBackHandler for DefaultHandler {
    fn select_response_level(&mut self, _previews: &[PidWritePreview]) -> WriteBackSelection {
        WriteBackSelection::Skipped("no selection".to_string())
    }
}

async fn fixture() -> (SqlitePool, i64, DcsTemplate, LoopTags, LoopConfig) {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    bhtune_db::seed_builtin_templates(&pool, Utc::now())
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
        Utc::now(),
    )
    .await
    .unwrap();
    (pool, run.id, template, tags, config)
}

async fn recorded_fixture() -> (SqlitePool, i64, DcsTemplate, LoopTags, LoopConfig) {
    let (pool, run_id, template, tags, config) = fixture().await;
    TuneRunRow::record_connection(
        &pool,
        run_id,
        Some("Mock.Kepware.Sim"),
        Some("127.0.0.1:7602"),
        "{}",
    )
    .await
    .unwrap();
    (pool, run_id, template, tags, config)
}

fn result_row(run_id: i64, response_level: ResponseLevel) -> TuneResultRow {
    TuneResultRow {
        id: 0,
        run_id,
        response_level,
        kp: Some(2.0),
        ti_minutes: Some(4.0),
        td_minutes: Some(0.5),
        proportional: Some(2.0),
        integral: Some(4.0),
        derivative: Some(0.5),
        status: TuningResultStatus::Valid,
        invalid_reason: None,
    }
}

#[test]
fn controller_preview_preserves_raw_values_and_reports_precision_failures() {
    let mut template = bhtune_core::built_in_templates().remove(0);
    let mut result = result_row(1, ResponseLevel::Moderate);
    result.proportional = Some(155.21378);
    result.integral = Some(2.482169);
    let preview = pid_write_preview(&result, ControllerType::Pi, &template);
    let values = preview.values.unwrap();
    assert_eq!(values.proportional.value, 155.2);
    assert_eq!(values.proportional.display, "155.2");
    assert_eq!(values.integral.value, 2.5);
    assert_eq!(values.integral.display, "2.5");
    assert_eq!(values.derivative.value, 0.0);
    assert_eq!(values.derivative.display, "0.0");
    assert!(preview.unavailable_reason.is_none());
    assert_eq!(result.proportional, Some(155.21378));
    assert_eq!(result.integral, Some(2.482169));
    assert_eq!(result.status, TuningResultStatus::Valid);

    result.integral = Some(0.049);
    let preview = pid_write_preview(&result, ControllerType::Pi, &template);
    assert!(preview.values.is_none());
    assert!(preview.unavailable_reason.unwrap().contains("integral"));
    assert_eq!(result.status, TuningResultStatus::Valid);

    template.pid_rounding = bhtune_core::PidRounding::default();
    let values = controller_pid_for_result(&result, ControllerType::P, &template).unwrap();
    assert_eq!(values.integral.value, 9999.0);
    assert_eq!(values.integral.display, "9999");
    assert_eq!(values.derivative.value, 0.0);
}

#[tokio::test]
async fn rounding_to_zero_rejects_automated_and_interactive_writes_before_pid_io() {
    let (pool, run_id, template, tags, config) = fixture().await;
    let mut result = result_row(run_id, ResponseLevel::Moderate);
    result.integral = Some(0.049);
    TuneResultRow::insert(&pool, &result).await.unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());

    for requested in [Some(ResponseLevel::Moderate), None] {
        let mut handler =
            RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));
        let (outcome, detail) = maybe_write_back(
            &pool,
            run_id,
            &tags,
            &template,
            &driver,
            config,
            requested,
            true,
            Some(&mut handler),
        )
        .await
        .unwrap();
        assert_eq!(outcome, WriteBackOutcome::Failed);
        assert!(detail.unwrap().contains("erase an active term to zero"));
        assert!(handler.events.last().unwrap().starts_with("failed:"));
    }
    assert_eq!(driver.read_calls.load(Ordering::Relaxed), 0);
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
    assert!(
        TuneWriteRow::list_for_run(&pool, run_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn rounded_write_matches_preview_and_audit_but_revert_preserves_original_precision() {
    let (pool, run_id, template, tags, config) = recorded_fixture().await;
    let mut result = result_row(run_id, ResponseLevel::Moderate);
    result.proportional = Some(155.21378);
    result.integral = Some(2.482169);
    TuneResultRow::insert(&pool, &result).await.unwrap();
    let expected = controller_pid_for_result(&result, config.controller_type, &template).unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());
    let original = WriteReadback {
        proportional: 7.123456,
        integral: 8.765432,
        derivative: 0.123456,
    };
    for (tag, value) in [
        (
            tags.proportional_constant.as_ref().unwrap(),
            original.proportional,
        ),
        (tags.integral_constant.as_ref().unwrap(), original.integral),
        (
            tags.derivative_constant.as_ref().unwrap(),
            original.derivative,
        ),
    ] {
        driver
            .values
            .lock()
            .unwrap()
            .insert(tag.clone(), value.to_string());
    }
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
    let (outcome, detail) = maybe_write_back_with_owner(
        &pool,
        run_id,
        &tags,
        &template,
        &audited,
        config,
        Some(ResponseLevel::Moderate),
        false,
        None,
        Some(&owner),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, WriteBackOutcome::Written { .. }));
    assert!(detail.is_none());
    let written = TuneWriteRow::list_for_run(&pool, run_id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(written.previous, Some(original));
    assert_eq!(
        written.proportional_written,
        Some(expected.proportional.value)
    );
    assert_eq!(written.integral_written, Some(expected.integral.value));
    assert_eq!(written.derivative_written, Some(expected.derivative.value));
    assert_eq!(written.proportional_readback, written.proportional_written);
    assert_eq!(written.integral_readback, written.integral_written);
    assert_eq!(written.derivative_readback, written.derivative_written);
    owner.release().await.unwrap();

    let outcome = write_pid_values(
        &pool,
        run_id,
        &driver,
        tags.proportional_constant.as_deref().unwrap(),
        tags.integral_constant.as_deref().unwrap(),
        tags.derivative_constant.as_deref().unwrap(),
        ResponseLevel::Moderate,
        original,
        WriteKind::Revert,
        false,
    )
    .await
    .unwrap();
    assert_eq!(outcome, PidWriteOutcome::Written);
    let reverted = TuneWriteRow::list_for_run(&pool, run_id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(reverted.kind, WriteKind::Revert);
    assert_eq!(reverted.proportional_written, Some(original.proportional));
    assert_eq!(reverted.integral_written, Some(original.integral));
    assert_eq!(reverted.derivative_written, Some(original.derivative));
}

#[test]
fn optional_handler_defaults_are_no_ops() {
    let mut handler = DefaultHandler;
    handler.pid_results_ready(&[]);
    assert!(matches!(
        handler.select_response_level(&[]),
        WriteBackSelection::Skipped(reason) if reason == "no selection"
    ));
    let values = controller_pid_for_result(
        &result_row(1, ResponseLevel::Moderate),
        ControllerType::Pi,
        &bhtune_core::built_in_templates().remove(0),
    )
    .unwrap();
    handler.write_back_selected(&values, true);
    handler.write_back_skipped(WriteBackSkipReason::NoPidTags);
    handler.write_back_failed("ignored");
    handler.write_back_finished(ResponseLevel::Moderate, &PidWriteOutcome::Written);
}

#[tokio::test]
async fn failing_driver_reports_unsupported_write_and_browse() {
    let driver = FailingDriver;
    assert!(matches!(
        driver.write(&"tag".to_string(), TagWrite::Float(1.0)).await,
        Err(DriverError::Unsupported { operation: "test" })
    ));
    assert!(matches!(
        driver.browse(BrowsePageRequest::root(1)).await,
        Err(DriverError::Unsupported { operation: "test" })
    ));
}

#[tokio::test]
async fn pid_driver_reports_missing_value_and_unsupported_browse() {
    let (_pool, _run_id, _template, tags, _config) = fixture().await;
    let driver = PidDriver::new(&tags, HashMap::new());

    assert!(matches!(
        driver.read(&["missing".to_string()]).await,
        Err(DriverError::Operation(_))
    ));
    assert!(matches!(
        driver.browse(BrowsePageRequest::root(1)).await,
        Err(DriverError::Unsupported { operation: "test" })
    ));
}

#[tokio::test]
async fn pid_writeback_notifies_handler_after_successful_write() {
    let (pool, run_id, template, tags, config) = fixture().await;
    TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
        .await
        .unwrap();
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
    let mut handler = RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));

    let (outcome, detail) = maybe_write_back_with_owner(
        &pool,
        run_id,
        &tags,
        &template,
        &audited,
        config,
        Some(ResponseLevel::Moderate),
        false,
        Some(&mut handler),
        Some(&owner),
    )
    .await
    .unwrap();

    assert!(matches!(
        outcome,
        WriteBackOutcome::Written {
            response_level: ResponseLevel::Moderate
        }
    ));
    assert!(detail.is_none());
    assert_eq!(
        handler.events,
        ["selected:Moderate:true", "finished:Moderate:Written",]
    );
    owner.release().await.unwrap();
}

#[tokio::test]
async fn pid_rollback_rejects_non_finite_audit_values_before_writing() {
    let (pool, run_id, _template, tags, _config) = fixture().await;
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());

    let error = rollback_pid_writes(
        &driver,
        &[("Proportional", "Unit1.LIC101.P", f32::NAN, 1.0)],
        &owner,
    )
    .await
    .unwrap_err();

    assert!(error.contains("non-finite write value"));
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 0);
    owner.release().await.unwrap();
}

#[tokio::test]
async fn owned_writeback_audits_all_pid_mutations_before_reporting_success() {
    let (pool, run_id, template, tags, config) = fixture().await;
    TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
        .await
        .unwrap();
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));

    let (outcome, detail) = maybe_write_back_with_owner(
        &pool,
        run_id,
        &tags,
        &template,
        &audited,
        config,
        Some(ResponseLevel::Moderate),
        false,
        None,
        Some(&owner),
    )
    .await
    .unwrap();

    assert!(matches!(
        outcome,
        WriteBackOutcome::Written {
            response_level: ResponseLevel::Moderate
        }
    ));
    assert!(detail.is_none());
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);
    assert!(
        TuneWriteRow::list_for_run(&pool, run_id)
            .await
            .unwrap()
            .last()
            .unwrap()
            .success
    );
    let audit = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
        .await
        .unwrap();
    assert_eq!(audit.len(), 3);
    assert!(
        audit
            .iter()
            .all(|step| step.status == bhtune_db::models::MutationStepStatus::Confirmed)
    );
    owner.release().await.unwrap();
}

#[tokio::test]
async fn owned_pid_write_accepts_tune_ownership() {
    let (pool, run_id, _template, tags, _config) = fixture().await;
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::Tune,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));

    let outcome = write_pid_values_with_owner(
        &pool,
        run_id,
        &audited,
        tags.proportional_constant.as_deref().unwrap(),
        tags.integral_constant.as_deref().unwrap(),
        tags.derivative_constant.as_deref().unwrap(),
        ResponseLevel::Moderate,
        WriteReadback {
            proportional: 5.0,
            integral: 6.0,
            derivative: 7.0,
        },
        WriteKind::Write,
        false,
    )
    .await
    .unwrap();

    assert!(matches!(outcome, PidWriteOutcome::Written));
    owner.release().await.unwrap();
}

#[tokio::test]
async fn pid_write_with_owner_reverts_only_constants_confirmed_before_a_later_failure() {
    let (pool, run_id, _template, tags, _config) = fixture().await;
    let integral = tags.integral_constant.as_ref().unwrap().clone();
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::from([(integral, vec![1])]));
    let original_proportional = 1.234567_f32;
    driver.values.lock().unwrap().insert(
        tags.proportional_constant.as_ref().unwrap().clone(),
        original_proportional.to_string(),
    );
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
    let target = WriteReadback {
        proportional: 5.0,
        integral: 6.0,
        derivative: 7.0,
    };

    let outcome = write_pid_values_with_owner(
        &pool,
        run_id,
        &audited,
        tags.proportional_constant.as_deref().unwrap(),
        tags.integral_constant.as_deref().unwrap(),
        tags.derivative_constant.as_deref().unwrap(),
        ResponseLevel::Moderate,
        target,
        WriteKind::Write,
        false,
    )
    .await
    .unwrap();

    assert!(matches!(
        outcome,
        PidWriteOutcome::Failed { ref detail } if detail.contains("rolled back")
    ));
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);
    assert_eq!(
        driver
            .values
            .lock()
            .unwrap()
            .get(tags.proportional_constant.as_deref().unwrap())
            .map(String::as_str),
        Some(original_proportional.to_string().as_str())
    );
    let persisted = TuneWriteRow::list_for_run(&pool, run_id)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(persisted.rollback_state, Some(RollbackState::Succeeded));
    let audit = LiveMutationStepRow::list_for_owner(&pool, owner.owner().id)
        .await
        .unwrap();
    assert_eq!(audit.len(), 3);
    owner.release().await.unwrap();
}

#[tokio::test]
async fn pid_write_wrapper_owns_success_and_revert_pre_read_failure_paths() {
    let (pool, run_id, _template, tags, _config) = recorded_fixture().await;
    let driver = PidDriver::new(&tags, HashMap::new());
    let target = WriteReadback {
        proportional: 2.0,
        integral: 3.0,
        derivative: 0.5,
    };
    assert_eq!(
        write_pid_values(
            &pool,
            run_id,
            &driver,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            target,
            WriteKind::Write,
            false,
        )
        .await
        .unwrap(),
        PidWriteOutcome::Written
    );
    assert_eq!(driver.write_calls.load(Ordering::Relaxed), 3);

    assert!(
        write_pid_values(
            &pool,
            run_id,
            &FailingDriver,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            target,
            WriteKind::Revert,
            false,
        )
        .await
        .is_ok_and(|outcome| matches!(outcome, PidWriteOutcome::Failed { .. }))
    );
    let owners = bhtune_db::models::LiveOwnershipRow::list_for_run(&pool, run_id)
        .await
        .unwrap();
    assert_eq!(owners.len(), 2);
    assert!(
        owners
            .iter()
            .all(|owner| owner.state == bhtune_db::models::LiveOwnershipState::Released)
    );
    assert!(
        write_pid_values(
            &pool,
            run_id + 100,
            &FailingDriver,
            "P",
            "I",
            "D",
            ResponseLevel::Moderate,
            target,
            WriteKind::Write,
            false,
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn pid_write_wrapper_never_hides_write_or_release_persistence_failures() {
    let (pool, run_id, _template, tags, _config) = recorded_fixture().await;
    sqlx::query(
            "CREATE TRIGGER reject_pid_write_insert BEFORE INSERT ON tune_writes BEGIN SELECT RAISE(FAIL, 'injected PID write insert failure'); END",
        )
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        write_pid_values(
            &pool,
            run_id,
            &FailingDriver,
            tags.proportional_constant.as_deref().unwrap(),
            tags.integral_constant.as_deref().unwrap(),
            tags.derivative_constant.as_deref().unwrap(),
            ResponseLevel::Moderate,
            WriteReadback {
                proportional: 2.0,
                integral: 3.0,
                derivative: 0.5,
            },
            WriteKind::Write,
            false,
        )
        .await
        .is_err()
    );
    sqlx::query("DROP TRIGGER reject_pid_write_insert")
        .execute(&pool)
        .await
        .unwrap();

    let (release_pool, release_run_id, _template, release_tags, _config) = recorded_fixture().await;
    sqlx::query(
            "CREATE TRIGGER reject_pid_owner_release BEFORE UPDATE OF state ON live_operation_owners WHEN NEW.state = 'released' BEGIN SELECT RAISE(FAIL, 'injected owner release failure'); END",
        )
        .execute(&release_pool)
        .await
        .unwrap();
    let release_error = write_pid_values(
        &release_pool,
        release_run_id,
        &FailingDriver,
        release_tags.proportional_constant.as_deref().unwrap(),
        release_tags.integral_constant.as_deref().unwrap(),
        release_tags.derivative_constant.as_deref().unwrap(),
        ResponseLevel::Moderate,
        WriteReadback {
            proportional: 2.0,
            integral: 3.0,
            derivative: 0.5,
        },
        WriteKind::Revert,
        false,
    )
    .await
    .unwrap_err();
    assert!(release_error.to_string().contains("owner release failure"));
    sqlx::query("DROP TRIGGER reject_pid_owner_release")
        .execute(&release_pool)
        .await
        .unwrap();

    let (combined_pool, combined_run_id, _template, combined_tags, _config) =
        recorded_fixture().await;
    sqlx::query(
            "CREATE TRIGGER reject_pid_restore_intent BEFORE UPDATE OF restore_intent_json ON live_operation_owners BEGIN SELECT RAISE(FAIL, 'injected restore intent failure'); END",
        )
        .execute(&combined_pool)
        .await
        .unwrap();
    sqlx::query(
            "CREATE TRIGGER reject_pid_owner_release_again BEFORE UPDATE OF state ON live_operation_owners WHEN NEW.state = 'released' BEGIN SELECT RAISE(FAIL, 'injected owner release failure'); END",
        )
        .execute(&combined_pool)
        .await
        .unwrap();
    let combined_error = write_pid_values(
        &combined_pool,
        combined_run_id,
        &PidDriver::new(&combined_tags, HashMap::new()),
        combined_tags.proportional_constant.as_deref().unwrap(),
        combined_tags.integral_constant.as_deref().unwrap(),
        combined_tags.derivative_constant.as_deref().unwrap(),
        ResponseLevel::Moderate,
        WriteReadback {
            proportional: 2.0,
            integral: 3.0,
            derivative: 0.5,
        },
        WriteKind::Write,
        false,
    )
    .await
    .unwrap_err();
    assert!(
        combined_error
            .to_string()
            .contains("additionally failed to persist live ownership release"),
        "{combined_error:#}"
    );
}

#[tokio::test]
async fn owned_pid_write_rejects_mismatched_run_and_operation_kind() {
    let (pool, run_id, _template, tags, _config) = fixture().await;
    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    let driver = PidDriver::new(&tags, HashMap::new());
    let audited = AuditedDriver::new(&driver, &pool, &owner, Some(run_id));
    let target = WriteReadback {
        proportional: 2.0,
        integral: 3.0,
        derivative: 0.5,
    };
    assert!(
        write_pid_values_with_owner(
            &pool,
            run_id + 1,
            &audited,
            "P",
            "I",
            "D",
            ResponseLevel::Moderate,
            target,
            WriteKind::Write,
            false,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("does not match PID write run")
    );
    assert!(
        write_pid_values_with_owner(
            &pool,
            run_id,
            &audited,
            "P",
            "I",
            "D",
            ResponseLevel::Moderate,
            target,
            WriteKind::Revert,
            false,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("does not authorize")
    );
    assert!(
        write_pid_values_inner(
            &pool,
            run_id + 1,
            &audited,
            "P",
            "I",
            "D",
            ResponseLevel::Moderate,
            target,
            WriteKind::Write,
            false,
            &owner,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("does not match PID write run")
    );
    owner.release().await.unwrap();
}

#[tokio::test]
async fn pid_restore_intent_requires_a_present_json_object_and_persists_target_values() {
    let (pool, run_id, _template, tags, _config) = fixture().await;
    let target = WriteReadback {
        proportional: 2.0,
        integral: 3.0,
        derivative: 0.5,
    };
    let previous = WriteReadback {
        proportional: 1.0,
        integral: 2.0,
        derivative: 0.0,
    };

    let owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        None,
    )
    .await
    .unwrap();
    persist_pid_restore_intent(
        &pool,
        &owner,
        run_id,
        WriteKind::Write,
        ResponseLevel::Moderate,
        previous,
        target,
    )
    .await
    .unwrap();
    let persisted = bhtune_db::models::LiveOwnershipRow::get(&pool, owner.owner().id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(
            persisted.restore_intent_json.as_deref().unwrap()
        )
        .unwrap()["pid_restore"]["target"]["proportional"]
            == 2.0
    );
    owner.release().await.unwrap();

    let non_object_owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("[]".to_string()),
    )
    .await
    .unwrap();
    assert!(
        persist_pid_restore_intent(
            &pool,
            &non_object_owner,
            run_id,
            WriteKind::Write,
            ResponseLevel::Moderate,
            previous,
            target,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("not a JSON object")
    );
    non_object_owner.release().await.unwrap();

    let missing_owner = LiveOperationGuard::acquire(
        &pool,
        Some(run_id),
        LiveOperationKind::PidWrite,
        "127.0.0.1:7602",
        "Mock.Kepware.Sim",
        &tags.manipulated_variable,
        Some("{}".to_string()),
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM live_operation_owners WHERE id = ?")
        .bind(missing_owner.owner().id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        persist_pid_restore_intent(
            &pool,
            &missing_owner,
            run_id,
            WriteKind::Write,
            ResponseLevel::Moderate,
            previous,
            target,
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("disappeared")
    );
    drop(missing_owner);
}

#[tokio::test]
async fn maybe_write_back_reports_skips_and_selection_failures() {
    let (pool, run_id, template, tags, config) = fixture().await;
    let driver = FailingDriver;

    let mut no_tags = tags.clone();
    no_tags.proportional_constant = None;
    let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
    let result = maybe_write_back(
        &pool,
        run_id,
        &no_tags,
        &template,
        &driver,
        config,
        None,
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Skipped);
    assert_eq!(handler.events, ["skipped:NoPidTags"]);

    let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
    let result = maybe_write_back(
        &pool,
        run_id + 1,
        &tags,
        &template,
        &driver,
        config,
        None,
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Skipped);
    assert_eq!(handler.events, ["skipped:NoResults"]);

    TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
        .await
        .unwrap();

    let mut handler =
        RecordingHandler::new(WriteBackSelection::Failed("selection failed".to_string()));
    let result = maybe_write_back(
        &pool,
        run_id,
        &tags,
        &template,
        &driver,
        config,
        None,
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Failed);
    assert_eq!(handler.events, ["select", "failed:selection failed"]);

    let mut handler =
        RecordingHandler::new(WriteBackSelection::Skipped("operator skipped".to_string()));
    let result = maybe_write_back(
        &pool,
        run_id,
        &tags,
        &template,
        &driver,
        config,
        None,
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Skipped);
    assert_eq!(handler.events, ["select"]);
}

#[tokio::test]
async fn maybe_write_back_reports_missing_invalid_and_live_write_results() {
    let (pool, run_id, template, tags, config) = fixture().await;
    let driver = FailingDriver;
    TuneResultRow::insert(&pool, &result_row(run_id, ResponseLevel::Moderate))
        .await
        .unwrap();
    TuneResultRow::insert(
        &pool,
        &TuneResultRow {
            status: TuningResultStatus::Invalid,
            invalid_reason: Some(TuningResultInvalidReason::NonPositivePvAmplitude),
            kp: None,
            ti_minutes: None,
            td_minutes: None,
            proportional: None,
            integral: None,
            derivative: None,
            ..result_row(run_id, ResponseLevel::Aggressive)
        },
    )
    .await
    .unwrap();

    let mut handler = RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Moderate));
    let result = maybe_write_back(
        &pool,
        run_id,
        &tags,
        &template,
        &driver,
        config,
        Some(ResponseLevel::Sluggish),
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Failed);
    assert!(handler.events[0].starts_with("failed:no calculated result"));

    let mut handler =
        RecordingHandler::new(WriteBackSelection::Selected(ResponseLevel::Aggressive));
    let result = maybe_write_back(
        &pool,
        run_id,
        &tags,
        &template,
        &driver,
        config,
        None,
        true,
        Some(&mut handler),
    )
    .await
    .unwrap();
    assert_eq!(result.0, WriteBackOutcome::Failed);
    assert_eq!(handler.events.len(), 2);
    assert_eq!(handler.events[0], "select");
    assert!(handler.events[1].starts_with("failed:"));

    let mut handler = RecordingHandler::new(WriteBackSelection::Skipped("unused".into()));
    let error = maybe_write_back(
        &pool,
        run_id,
        &tags,
        &template,
        &driver,
        config,
        Some(ResponseLevel::Moderate),
        true,
        Some(&mut handler),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("requires a live ownership guard")
    );
    assert_eq!(handler.events, ["selected:Moderate:true"]);
}
