use super::{
    ApiError, AppState, OpcDaDriver, PidWriteOutcome, ResponseLevel, RunAlreadyActive,
    RunDetailResponse, SqlitePool, TuneRunRow, WriteKind, WriteReadback, build_run_detail,
    require_present,
};
use bhtune_runtime::tune::write_pid_values_with_owner;

pub(super) fn normalized_notes(notes: String) -> Option<String> {
    let trimmed = notes.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub(super) const INSERTED_RUN_PRESENT: &str =
    "the tune_runs row this handler just inserted via prepare() must exist immediately afterward";
pub(super) const P_TAG_PRESENT: &str =
    "require_writable_run already checked proportional_constant is Some";
pub(super) const I_TAG_PRESENT: &str =
    "require_writable_run already checked integral_constant is Some";
pub(super) const D_TAG_PRESENT: &str =
    "require_writable_run already checked derivative_constant is Some";

/// Connects an [`OpcDaDriver`] using `run`'s own recorded `opc_server`/`bridge_host` --
/// never re-resolved from this process's own config/flags, for exactly the reason
/// `bhtune_runtime::history::resolve_revert_connection` enforces: a value
/// re-resolved at write/revert time could silently point at a different gateway than the
/// run itself actually used. [`require_writable_run`] must already have confirmed both
/// fields are present.
pub(super) async fn connect_to_runs_recorded_driver(
    pool: &SqlitePool,
    run: &TuneRunRow,
) -> Result<OpcDaDriver, ApiError> {
    const OPC_SERVER_PRESENT: &str = "require_writable_run already checked opc_server is Some";
    const BRIDGE_HOST_PRESENT: &str = "require_writable_run already checked bridge_host is Some";
    let opc_server = require_present(run.opc_server.as_deref(), OPC_SERVER_PRESENT)?;
    let bridge_host = require_present(run.bridge_host.as_deref(), BRIDGE_HOST_PRESENT)?;
    let driver = OpcDaDriver::connect(bridge_host, opc_server)
        .await
        .map_err(|e| {
            ApiError::BadRequest(format!(
                "failed to connect to OPC server '{opc_server}' via bridge '{bridge_host}': {e}"
            ))
        })?;
    let report =
        bhtune_runtime::gateway::require_live_gateway_compatible(bridge_host, Some(opc_server))
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    if run.gateway_compatibility_json.is_none() {
        let snapshot = bhtune_runtime::gateway::compatibility_json(&report);
        TuneRunRow::record_gateway_compatibility(pool, run.id, &snapshot).await?;
    }
    Ok(driver)
}

/// Reserves the [`crate::active_run::ActiveRun`] exclusive write/revert reservation for
/// `run.id`, connects, and calls [`write_pid_values`] -- releasing the reservation on every
/// exit path (this project's established "no `Drop`-based cleanup, `Drop` cannot await" rule;
/// see
/// `crate::active_run::ActiveRun::reserve`'s own doc comment) -- then rebuilds and returns
/// the run's fresh [`RunDetailResponse`] regardless of whether the write/revert itself
/// succeeded. A [`PidWriteOutcome::Failed`] is not an HTTP error: the request was processed
/// successfully and its result -- including the failure -- is recorded in the returned
/// `writes[]` array's `success`/`error_message` fields, exactly how a client already reads a
/// write-back outcome from `GET /api/runs/{id}`. Only [`ApiError::Conflict`] (a tune or another
/// write/revert operation holds the exclusive reservation), [`ApiError::BadRequest`] (the driver connection itself
/// failed), or [`ApiError::Internal`] (an unexpected database failure inside
/// [`write_pid_values`]) short-circuit this into an actual error response.
#[allow(clippy::too_many_arguments)]
pub(super) async fn reserve_connect_and_write(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
) -> Result<RunDetailResponse, ApiError> {
    reserve_connect_and_write_with_hook(
        state,
        run_id,
        run,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain_quality,
        |_| async {},
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn reserve_connect_and_write_with_hook<F, Fut>(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
    after_release: F,
) -> Result<RunDetailResponse, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    reserve_connect_and_write_with_hooks(
        state,
        run_id,
        run,
        p_tag,
        i_tag,
        d_tag,
        response_level,
        target,
        kind,
        allow_uncertain_quality,
        |_| async {},
        after_release,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn reserve_connect_and_write_with_hooks<F, Fut, G, Gut>(
    state: &AppState,
    run_id: i64,
    run: &TuneRunRow,
    p_tag: &str,
    i_tag: &str,
    d_tag: &str,
    response_level: ResponseLevel,
    target: WriteReadback,
    kind: WriteKind,
    allow_uncertain_quality: bool,
    before_write: F,
    after_release: G,
) -> Result<RunDetailResponse, ApiError>
where
    F: FnOnce(&AppState) -> Fut,
    Fut: std::future::Future<Output = ()>,
    G: FnOnce(&AppState) -> Gut,
    Gut: std::future::Future<Output = ()>,
{
    state
        .active_run
        .reserve(run_id)
        .await
        .map_err(|RunAlreadyActive { run_id: existing }| {
            ApiError::Conflict(format!(
                "run {existing} or another PID write/revert is active; try again once it finishes"
            ))
        })?;

    let operation_kind = pid_operation_kind(kind);
    let restore_intent = serde_json::json!({
        "version": 1,
        "kind": "pid_restore",
        "run_id": run_id,
        "write_kind": kind,
        "state": "awaiting_prewrite_readings",
    })
    .to_string();
    let ownership = bhtune_runtime::live_ownership::LiveOperationGuard::acquire_for_recorded_run(
        &state.pool,
        run,
        operation_kind,
        restore_intent,
    )
    .await
    .map_err(map_live_ownership_acquire_error);
    let result = match ownership {
        Err(error) => Err(error),
        Ok(ownership) => {
            let operation_result: Result<PidWriteOutcome, ApiError> = async {
                ownership.ensure_healthy().map_err(ApiError::Internal)?;
                let driver = connect_to_runs_recorded_driver(&state.pool, run).await?;
                before_write(state).await;
                let audited = bhtune_runtime::live_ownership::AuditedDriver::new(
                    &driver,
                    &state.pool,
                    &ownership,
                    Some(run_id),
                );
                write_pid_values_with_owner(
                    &state.pool,
                    run_id,
                    &audited,
                    p_tag,
                    i_tag,
                    d_tag,
                    response_level,
                    target,
                    kind,
                    allow_uncertain_quality,
                )
                .await
                .map_err(ApiError::Internal)
            }
            .await;
            let release_result = ownership.release().await;
            combine_pid_write_and_release(operation_result, release_result)
        }
    };

    state.active_run.release(run_id).await;
    after_release(state).await;
    result?;

    refreshed_run_detail(&state.pool, run_id).await
}

fn pid_operation_kind(kind: WriteKind) -> bhtune_db::models::LiveOperationKind {
    match kind {
        WriteKind::Write => bhtune_db::models::LiveOperationKind::PidWrite,
        WriteKind::Revert => bhtune_db::models::LiveOperationKind::PidRevert,
    }
}

fn map_live_ownership_acquire_error(
    error: bhtune_runtime::live_ownership::LiveOwnershipAcquireError,
) -> ApiError {
    match error {
        bhtune_runtime::live_ownership::LiveOwnershipAcquireError::LockBusy
        | bhtune_runtime::live_ownership::LiveOwnershipAcquireError::ResourceClaimed => {
            ApiError::Conflict(error.to_string())
        }
        bhtune_runtime::live_ownership::LiveOwnershipAcquireError::Persistence(error) => {
            ApiError::Internal(error)
        }
    }
}

fn combine_pid_write_and_release(
    operation_result: Result<PidWriteOutcome, ApiError>,
    release_result: anyhow::Result<()>,
) -> Result<PidWriteOutcome, ApiError> {
    match (operation_result, release_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(ApiError::Internal(error)),
        (Err(error), Err(release_error)) => Err(ApiError::Internal(anyhow::anyhow!(
            "{error:?}; additionally failed to persist live ownership release: {release_error}"
        ))),
    }
}

pub(super) async fn refreshed_run_detail(
    pool: &SqlitePool,
    run_id: i64,
) -> Result<RunDetailResponse, ApiError> {
    build_run_detail(pool, run_id).await?.ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "run {run_id} vanished while its write/revert was being processed"
        ))
    })
}

pub(super) fn map_revert_error(error: bhtune_runtime::history::RevertRunError) -> ApiError {
    match error {
        bhtune_runtime::history::RevertRunError::Invalid(error)
        | bhtune_runtime::history::RevertRunError::Connection(error)
        | bhtune_runtime::history::RevertRunError::Gateway(error) => {
            ApiError::BadRequest(error.to_string())
        }
        bhtune_runtime::history::RevertRunError::Persistence(error) => ApiError::Internal(error),
    }
}

pub(super) async fn reserve_and_revert(
    state: &AppState,
    run_id: i64,
    allow_uncertain_quality: bool,
) -> Result<RunDetailResponse, ApiError> {
    state
        .active_run
        .reserve(run_id)
        .await
        .map_err(|RunAlreadyActive { run_id: existing }| {
            ApiError::Conflict(format!(
                "run {existing} or another PID write/revert is active; try again once it finishes"
            ))
        })?;

    let result = bhtune_runtime::history::revert_run(
        &state.pool,
        bhtune_runtime::history::RevertRequest {
            run_id,
            bridge_host: None,
            server: None,
            confirmed: true,
        },
        allow_uncertain_quality,
    )
    .await;

    state.active_run.release(run_id).await;
    result.map_err(map_revert_error)?;

    refreshed_run_detail(&state.pool, run_id).await
}

#[cfg(test)]
mod tests {
    use super::{
        ApiError, PidWriteOutcome, WriteKind, combine_pid_write_and_release,
        map_live_ownership_acquire_error, pid_operation_kind,
    };
    use bhtune_db::models::LiveOperationKind;
    use bhtune_runtime::live_ownership::LiveOwnershipAcquireError;

    #[test]
    fn pid_operation_kind_matches_write_and_revert() {
        assert!(matches!(
            pid_operation_kind(WriteKind::Write),
            LiveOperationKind::PidWrite
        ));
        assert!(matches!(
            pid_operation_kind(WriteKind::Revert),
            LiveOperationKind::PidRevert
        ));
    }

    #[test]
    fn ownership_acquisition_failures_map_to_explicit_http_errors() {
        assert!(matches!(
            map_live_ownership_acquire_error(LiveOwnershipAcquireError::LockBusy),
            ApiError::Conflict(_)
        ));
        assert!(matches!(
            map_live_ownership_acquire_error(LiveOwnershipAcquireError::ResourceClaimed),
            ApiError::Conflict(_)
        ));
        assert!(matches!(
            map_live_ownership_acquire_error(LiveOwnershipAcquireError::Persistence(
                anyhow::anyhow!("database unavailable"),
            )),
            ApiError::Internal(_)
        ));
    }

    #[test]
    fn pid_write_and_release_results_preserve_operation_and_persistence_errors() {
        assert!(matches!(
            combine_pid_write_and_release(Ok(PidWriteOutcome::Written), Ok(())),
            Ok(PidWriteOutcome::Written)
        ));
        assert!(matches!(
            combine_pid_write_and_release(
                Err(ApiError::Conflict("operation failed".to_string())),
                Ok(()),
            ),
            Err(ApiError::Conflict(_))
        ));
        assert!(matches!(
            combine_pid_write_and_release(
                Ok(PidWriteOutcome::Written),
                Err(anyhow::anyhow!("release failed")),
            ),
            Err(ApiError::Internal(_))
        ));
        assert!(matches!(
            combine_pid_write_and_release(
                Err(ApiError::Conflict("operation failed".to_string())),
                Err(anyhow::anyhow!("release failed")),
            ),
            Err(ApiError::Internal(_))
        ));
    }
}
