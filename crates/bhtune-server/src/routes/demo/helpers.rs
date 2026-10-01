use super::dto::DemoIdentity;
use super::{
    ApiError, AppState, DEMO_COOKIE_NAME, DemoPolicy, DemoQuotaExceeded, DemoSessionRow, Digest,
    Event, FORWARDED_CLIENT_IP_HEADER, FromRequestParts, Future, HeaderMap, HeaderValue,
    Infallible, InitialReadingsResponse, IpAddr, Ipv6Addr, MvActuationResponse, Parts,
    PidConstantTagsResponse, PidParameterLabelsResponse, ResultResponse, RunDetailResponse,
    SampleResponse, ServerMode, Sha256, SocketAddr, TuneMvActuationRow, TuneResultRow, TuneRunRow,
    TuneSampleRow, TuneWriteRow, Utc, WriteResponse, header, random,
};

pub(crate) struct PeerAddress(pub(super) SocketAddr);

impl<S> FromRequestParts<S> for PeerAddress
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        std::future::ready(
            parts
                .extensions
                .get::<axum::extract::ConnectInfo<SocketAddr>>()
                .map(|value| Self(value.0))
                .ok_or_else(|| {
                    ApiError::Internal(anyhow::anyhow!("Demo request is missing peer ConnectInfo"))
                }),
        )
    }
}

pub(super) fn ensure_demo(state: &AppState) -> Result<(), ApiError> {
    (state.mode == ServerMode::Demo)
        .then_some(())
        .ok_or_else(|| ApiError::NotFound("demo mode is not enabled".into()))
}

pub(super) fn bad_request(message: impl Into<String>) -> ApiError {
    ApiError::BadRequest(message.into())
}

pub(super) fn too_many(message: impl Into<String>, retry_after_secs: u64) -> ApiError {
    ApiError::TooManyRequests {
        message: message.into(),
        retry_after_secs,
    }
}

pub(super) fn global_capacity(message: impl Into<String>, retry_after_secs: u64) -> ApiError {
    ApiError::GlobalCapacity {
        message: message.into(),
        retry_after_secs,
    }
}

pub(crate) fn ordinary_request_permit(
    state: &AppState,
) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    state
        .demo_runtime
        .try_acquire_ordinary_request()
        .map_err(|_| {
            global_capacity(
                "demo request capacity is temporarily exhausted",
                state.demo_policy.ordinary_request_timeout_secs,
            )
        })
}

pub(super) fn quota_error(error: DemoQuotaExceeded) -> ApiError {
    too_many(
        "demo accepted-start quota exceeded; wait before starting another tune",
        error.retry_after_secs,
    )
}

pub(super) fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex_encode(&hasher.finalize())
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn trusted_proxy(peer: SocketAddr, configured: Option<&str>) -> bool {
    let Some(configured) = configured else {
        return false;
    };
    let configured = configured.trim();
    if let Ok(address) = configured.parse::<IpAddr>() {
        return address == peer.ip();
    }
    let Some((network, bits)) = configured.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(bits)) = (network.parse::<IpAddr>(), bits.parse::<u32>()) else {
        return false;
    };
    match (peer.ip(), network) {
        (IpAddr::V4(peer), IpAddr::V4(network)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(peer) & mask == u32::from(network) & mask
        }
        (IpAddr::V6(peer), IpAddr::V6(network)) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(peer) & mask == u128::from(network) & mask
        }
        _ => false,
    }
}

pub(super) fn quota_ip(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => {
            let network = Ipv6Addr::from(u128::from(address) & (u128::MAX << 64));
            format!("{network}/64")
        }
    }
}

pub(super) fn client_ip(
    headers: &HeaderMap,
    peer: PeerAddress,
    configured_proxy: Option<&str>,
) -> String {
    if trusted_proxy(peer.0, configured_proxy) {
        let mut values = headers.get_all(FORWARDED_CLIENT_IP_HEADER).iter();
        if let (Some(value), None) = (values.next(), values.next())
            && let Ok(value) = value.to_str()
            && value.trim() == value
            && !value.contains(',')
            && let Ok(address) = value.parse::<IpAddr>()
        {
            return quota_ip(address);
        }
    }
    quota_ip(peer.0.ip())
}

pub(super) fn valid_token(token: &str) -> bool {
    token.len() == 64
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(super) fn cookie_token(headers: &HeaderMap) -> Result<Option<String>, ()> {
    let mut found = None;
    for value in headers.get_all(header::COOKIE) {
        let value = value.to_str().map_err(|_| ())?;
        for part in value.split(';') {
            let Some((name, value)) = part.trim().split_once('=') else {
                continue;
            };
            if name != DEMO_COOKIE_NAME {
                continue;
            }
            if found.is_some() || !valid_token(value) {
                return Err(());
            }
            found = Some(value.to_owned());
        }
    }
    Ok(found)
}

pub(super) fn parsed_token_hash(headers: &HeaderMap) -> Result<String, ApiError> {
    let token = cookie_token(headers)
        .map_err(|()| ApiError::Unauthorized("invalid demo session cookie".into()))?
        .ok_or_else(|| ApiError::Unauthorized("a demo session cookie is required".into()))?;
    Ok(token_hash(&token))
}

pub(crate) fn session_cookie_header(
    headers: &HeaderMap,
    policy: DemoPolicy,
) -> Result<Option<HeaderValue>, ApiError> {
    if matches!(cookie_token(headers), Ok(Some(_))) {
        return Ok(None);
    }
    let token: [u8; 32] = random();
    let cookie = format!(
        "{DEMO_COOKIE_NAME}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict; Secure",
        hex_encode(&token),
        policy.session_ttl_secs
    );
    HeaderValue::from_str(&cookie)
        .map(Some)
        .map_err(|error| ApiError::Internal(error.into()))
}

pub(super) async fn identify(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<DemoIdentity, ApiError> {
    let token_hash = parsed_token_hash(headers)?;
    let persisted = DemoSessionRow::get_by_token_hash(&state.pool, &token_hash, Utc::now()).await?;
    Ok(DemoIdentity {
        token_hash,
        persisted,
    })
}

pub(super) fn identify_without_lookup(headers: &HeaderMap) -> Result<DemoIdentity, ApiError> {
    Ok(DemoIdentity {
        token_hash: parsed_token_hash(headers)?,
        persisted: None,
    })
}

pub(super) fn owner_id(identity: &DemoIdentity, run_id: i64) -> Result<i64, ApiError> {
    identity
        .persisted
        .as_ref()
        .map(|session| session.id)
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))
}

pub(super) async fn build_owned_run_detail(
    state: &AppState,
    run_id: i64,
    owner_id: i64,
) -> Result<Option<RunDetailResponse>, ApiError> {
    let Some(run) = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id).await? else {
        return Ok(None);
    };
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    let results = TuneResultRow::list_for_run(&state.pool, run_id).await?;
    let writes = TuneWriteRow::list_for_run(&state.pool, run_id).await?;
    let mv_actuations = TuneMvActuationRow::list_for_run(&state.pool, run_id).await?;
    let pid_constant_tags = match (
        &run.tags.proportional_constant,
        &run.tags.integral_constant,
        &run.tags.derivative_constant,
    ) {
        (Some(proportional), Some(integral), Some(derivative)) => Some(PidConstantTagsResponse {
            proportional: proportional.clone(),
            integral: integral.clone(),
            derivative: derivative.clone(),
        }),
        _ => None,
    };
    let original_request = serde_json::from_str(&run.request_json).ok();
    let pid_parameter_labels = PidParameterLabelsResponse::from(&run.template);
    Ok(Some(RunDetailResponse {
        id: run.id,
        tag_name: run.loop_name,
        notes: run.notes,
        driver: run.driver,
        outcome: run.outcome,
        failure_reason: run.failure_reason,
        started_at: run.started_at,
        completed_at: run.completed_at,
        template_name: run.template.name,
        template_origin: run.template_origin,
        allow_uncertain_quality: run.allow_uncertain_quality,
        config: run.config,
        effective_tuning: run.effective_tuning,
        opc_server: run.opc_server,
        bridge_host: run.bridge_host,
        pid_constant_tags,
        pid_parameter_labels,
        initial_readings: run.initial_readings.map(InitialReadingsResponse::from),
        timing_metrics: run.timing_metrics,
        samples: samples.iter().map(SampleResponse::from).collect(),
        results: results.iter().map(ResultResponse::from).collect(),
        writes: writes.iter().map(WriteResponse::from).collect(),
        mv_actuations: mv_actuations
            .iter()
            .map(MvActuationResponse::from)
            .collect(),
        restore_status: run.restore_status,
        restore_detail: run.restore_detail,
        original_request,
        gateway_compatibility: None,
    }))
}

pub(super) fn ok_event(event: Event) -> Result<Event, Infallible> {
    Ok(event)
}

pub(super) async fn delete_existing_demo_run(
    state: &AppState,
    run_id: i64,
    owner_id: i64,
) -> Result<(), ApiError> {
    if !TuneRunRow::delete_for_demo_session(&state.pool, run_id, owner_id).await? {
        return Err(ApiError::NotFound(format!("no demo run with id {run_id}")));
    }
    Ok(())
}

pub(super) async fn trim_owned_history(state: &AppState, owner_id: i64) {
    if let Err(error) = TuneRunRow::prune_terminal_for_demo_session(
        &state.pool,
        owner_id,
        state.demo_policy.retained_runs_per_visitor,
    )
    .await
    {
        tracing::warn!(owner_id, %error, "failed to prune excess demo history");
    }
}

pub(super) async fn ensure_global_run_capacity(state: &AppState) -> Result<(), ApiError> {
    let current = TuneRunRow::count_demo_owned(&state.pool).await?;
    if current >= i64::from(state.demo_policy.max_tune_run_rows_global) {
        return Err(ApiError::GlobalCapacity {
            message: "demo history capacity is temporarily exhausted".into(),
            retry_after_secs: state.demo_policy.cleanup_interval_secs,
        });
    }
    Ok(())
}

pub(super) fn prepare_error(error: anyhow::Error, state: &AppState) -> ApiError {
    if error.chain().any(|cause| {
        cause
            .to_string()
            .contains("demo tune run row limit reached")
    }) {
        ApiError::GlobalCapacity {
            message: "demo history capacity is temporarily exhausted".into(),
            retry_after_secs: state.demo_policy.cleanup_interval_secs,
        }
    } else {
        ApiError::Internal(error)
    }
}

pub(super) async fn discard_unscheduled_demo_run(state: &AppState, run_id: i64, session_id: i64) {
    if let Err(error) = TuneRunRow::delete_for_demo_session(&state.pool, run_id, session_id).await {
        tracing::error!(run_id, session_id, %error, "failed to discard unscheduled demo run");
    }
}

pub(super) async fn api_not_found() -> ApiError {
    ApiError::NotFound("API route is not available in Demo mode".into())
}
