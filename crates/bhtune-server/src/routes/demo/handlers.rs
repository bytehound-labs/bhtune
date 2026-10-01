use super::dto::DemoStreamDone;
use super::helpers::{
    bad_request, build_owned_run_detail, client_ip, delete_existing_demo_run,
    discard_unscheduled_demo_run, ensure_demo, ensure_global_run_capacity, global_capacity,
    identify, identify_without_lookup, ok_event, owner_id, prepare_error, quota_error, too_many,
    trim_owned_history,
};
use super::validation::parse_demo_request;
use super::{
    ApiError, AppState, DemoSessionRow, Duration, Event, HeaderMap, HeaderValue,
    InitialReadingsResponse, IntoResponse, Json, KeepAlive, Pagination, Path, PeerAddress, Query,
    Response, RunDetailResponse, RunExportFormat, RunExportQuery, RunListQuery, RunListResponse,
    RunSummaryResponse, SSE_POLL_INTERVAL, SampleResponse, Sse, StartRunRequest, State, StatusCode,
    StdDuration, TemplateOrigin, TemplateResponse, TuneOutcome, TuneRunRow, TuneSampleRow, Utc,
    drive, filter_from_query, header, ordinary_request_permit, parse_stored_request, prepare_owned,
    require_present, stream,
};

pub(crate) async fn list_runs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<RunListQuery>,
) -> Result<Json<RunListResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let Some(owner_id) = identity.persisted.map(|session| session.id) else {
        return Ok(Json(RunListResponse {
            runs: Vec::new(),
            returned: 0,
            total: 0,
        }));
    };
    if query.offset.is_some_and(|offset| offset < 0) || query.limit.is_some_and(|limit| limit < 1) {
        return Err(bad_request(
            "demo run pagination requires limit >= 1 and offset >= 0",
        ));
    }
    let max_page = i64::from(
        state.demo_policy.retained_runs_per_visitor + state.demo_policy.max_active_runs_per_visitor,
    );
    let pagination = Pagination::new(
        query.limit.unwrap_or(max_page).min(max_page),
        query.offset.unwrap_or(0),
    );
    let filter = filter_from_query(&query).with_demo_session_id(owner_id);
    let runs = TuneRunRow::list(&state.pool, &filter, pagination).await?;
    let total = TuneRunRow::count(&state.pool, &filter).await?;
    Ok(Json(RunListResponse {
        returned: runs.len(),
        runs: runs.iter().map(RunSummaryResponse::from).collect(),
        total,
    }))
}

pub(crate) async fn last_request(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<StartRunRequest>>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let Some(owner_id) = identity.persisted.map(|session| session.id) else {
        return Ok(Json(None));
    };
    let request = TuneRunRow::newest_for_demo_session(&state.pool, owner_id)
        .await?
        .and_then(|run| parse_stored_request(run.id, &run.request_json));
    Ok(Json(request))
}

pub(crate) async fn get_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<Json<RunDetailResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    build_owned_run_detail(&state, run_id, owner_id)
        .await?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))
}

pub(crate) async fn stream_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_demo(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;

    let global_permit = state.demo_runtime.try_acquire_global_sse().map_err(|_| {
        global_capacity(
            "demo SSE capacity is temporarily exhausted",
            state.demo_policy.sse_lifetime_secs,
        )
    })?;
    let visitor_permit = state
        .demo_runtime
        .try_acquire_visitor_sse(&identity.token_hash, state.demo_policy.max_sse_per_visitor)
        .await
        .map_err(|_| {
            too_many(
                "demo SSE connection limit exceeded for this visitor",
                state.demo_policy.sse_lifetime_secs,
            )
        })?;

    let pool = state.pool.clone();
    let lifetime = StdDuration::from_secs(state.demo_policy.sse_lifetime_secs);
    let events = stream! {
        let _global_permit = global_permit;
        let _visitor_permit = visitor_permit;
        if lifetime.is_zero() {
            yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
        } else {
            let deadline = tokio::time::Instant::now() + lifetime;
            let mut last_tick = -1;
            let mut sent_initial = false;
            loop {
            let run = match tokio::time::timeout_at(
                deadline,
                TuneRunRow::get_for_demo_session(&pool, run_id, owner_id),
            ).await {
                Ok(Ok(Some(run))) => run,
                Ok(Ok(None)) => {
                    yield ok_event(Event::default().event("error").data("demo run is no longer available"));
                    break;
                }
                Ok(Err(error)) => {
                    tracing::error!(run_id, owner_id, %error, "failed to poll owned demo run");
                    yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    break;
                }
                Err(_) => {
                    yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                    break;
                }
            };
            if !sent_initial
                && let Some(initial) = run.initial_readings.clone()
            {
                match Event::default()
                    .event("initial")
                    .json_data(InitialReadingsResponse::from(initial))
                {
                    Ok(event) => {
                        sent_initial = true;
                        yield ok_event(event);
                    }
                    Err(error) => {
                        tracing::error!(run_id, owner_id, %error, "failed to encode owned demo initial readings");
                        yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                        break;
                    }
                }
            }
            match tokio::time::timeout_at(
                deadline,
                TuneSampleRow::list_for_run_since(&pool, run_id, last_tick),
            ).await {
                Ok(Ok(samples)) => {
                    let mut encoding_failed = false;
                    for sample in &samples {
                        last_tick = sample.tick_index;
                        match Event::default()
                            .event("sample")
                            .json_data(SampleResponse::from(sample))
                        {
                            Ok(event) => yield ok_event(event),
                            Err(error) => {
                                tracing::error!(run_id, owner_id, %error, "failed to encode owned demo sample");
                                yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                                encoding_failed = true;
                                break;
                            }
                        }
                    }
                    if encoding_failed {
                        break;
                    }
                }
                Ok(Err(error)) => {
                    tracing::error!(run_id, owner_id, %error, "failed to poll owned demo samples");
                    yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    break;
                }
                Err(_) => {
                    yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                    break;
                }
            }
            if run.outcome != TuneOutcome::Running {
                match Event::default()
                    .event("done")
                    .json_data(DemoStreamDone { outcome: run.outcome })
                {
                    Ok(event) => yield ok_event(event),
                    Err(error) => {
                        tracing::error!(run_id, owner_id, %error, "failed to encode owned demo terminal event");
                        yield ok_event(Event::default().event("error").data("demo stream unavailable"));
                    }
                }
                break;
            }
            if tokio::time::timeout_at(deadline, tokio::time::sleep(SSE_POLL_INTERVAL))
                .await
                .is_err()
            {
                yield ok_event(Event::default().event("error").data("demo stream lifetime exceeded"));
                break;
            }
        }
        }
    };
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

pub(crate) async fn cancel_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    let run = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    let _cancelled_or_already_terminal =
        run.outcome != TuneOutcome::Running || state.active_run.cancel(run_id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    let run = TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    if run.outcome == TuneOutcome::Running {
        return Err(ApiError::Conflict(
            "cancel the demo run before deleting it".into(),
        ));
    }
    delete_existing_demo_run(&state, run_id, owner_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn export_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(run_id): Path<i64>,
    Query(query): Query<RunExportQuery>,
) -> Result<Response, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let identity = identify(&state, &headers).await?;
    let owner_id = owner_id(&identity, run_id)?;
    TuneRunRow::get_for_demo_session(&state.pool, run_id, owner_id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("no demo run with id {run_id}")))?;
    let samples = TuneSampleRow::list_for_run(&state.pool, run_id).await?;
    if samples.is_empty() {
        return Err(ApiError::NotFound(format!(
            "demo run {run_id} has no samples"
        )));
    }
    let format = query.format.unwrap_or(RunExportFormat::Csv);
    let bytes = bhtune_runtime::export::samples_to_bytes(&samples, format.into())?;
    let (content_type, extension) = match format {
        RunExportFormat::Csv => ("text/csv", "csv"),
        RunExportFormat::Json => ("application/json", "json"),
    };
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"demo-run-{run_id}.{extension}\""
        ))
        .map_err(|error| ApiError::Internal(error.into()))?,
    );
    Ok(response)
}

pub(crate) async fn list_templates(
    State(state): State<AppState>,
) -> Result<Json<Vec<TemplateResponse>>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let rows = bhtune_db::models::DcsTemplateRow::list(&state.pool).await?;
    Ok(Json(
        rows.into_iter()
            .filter(|row| row.origin == TemplateOrigin::Builtin)
            .map(TemplateResponse::from)
            .collect(),
    ))
}

pub(crate) async fn get_template(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<TemplateResponse>, ApiError> {
    ensure_demo(&state)?;
    let _request_permit = ordinary_request_permit(&state)?;
    let row = bhtune_db::models::DcsTemplateRow::get_by_name(&state.pool, &name)
        .await?
        .filter(|row| row.origin == TemplateOrigin::Builtin)
        .ok_or_else(|| ApiError::NotFound(format!("no built-in template named '{name}'")))?;
    Ok(Json(row.into()))
}

pub(crate) async fn start_run(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddress,
    Json(value): Json<serde_json::Value>,
) -> Result<(StatusCode, Json<RunDetailResponse>), ApiError> {
    ensure_demo(&state)?;
    // The raw object allow-list is intentionally checked before acquiring a permit, touching
    // session/history storage, reading configuration, or constructing a driver. Deserializing
    // directly to `StartRunRequest` would lose the distinction between an omitted forbidden
    // field and an explicitly supplied `null`/`false` field.
    let request = parse_demo_request(value)?;
    let _request_permit = ordinary_request_permit(&state)?;

    let identity = identify_without_lookup(&headers)?;
    let client_ip = client_ip(&headers, peer, state.trusted_proxy.as_deref());
    let _start_admission = state.demo_runtime.lock_start_admission().await;
    let global_permit = state.demo_runtime.try_acquire_global_run().map_err(|_| {
        global_capacity(
            "demo run capacity is temporarily exhausted",
            state.demo_policy.run_timeout_secs,
        )
    })?;
    let visitor_permit = state
        .demo_runtime
        .try_acquire_visitor_run(
            &identity.token_hash,
            state.demo_policy.max_active_runs_per_visitor,
        )
        .await
        .map_err(|_| {
            too_many(
                "a demo tune is already active for this visitor",
                state.demo_policy.run_timeout_secs,
            )
        })?;

    let now = Utc::now();
    let accepted_start = state
        .demo_runtime
        .reserve_accepted_start(&identity.token_hash, &client_ip, now, state.demo_policy)
        .await
        .map_err(quota_error)?;
    let preparation =
        async {
            // On-demand cleanup prevents an expired visitor or over-retained history from causing a
            // friendly capacity rejection until the next periodic sweep.
            DemoSessionRow::cleanup_expired(&state.pool, now).await?;
            bhtune_db::models::TuneRunRow::prune_terminal_demo_owned(
                &state.pool,
                state.demo_policy.retained_runs_per_visitor,
            )
            .await?;
            ensure_global_run_capacity(&state).await?;
            let session =
                match DemoSessionRow::get_by_token_hash(&state.pool, &identity.token_hash, now)
                    .await?
                {
                    Some(session) => session,
                    None => {
                        DemoSessionRow::get_or_create(
                            &state.pool,
                            &identity.token_hash,
                            now,
                            now + Duration::seconds(state.demo_policy.session_ttl_secs as i64),
                        )
                        .await?
                    }
                };
            trim_owned_history(&state, session.id).await;
            let accepted_runs = TuneRunRow::count_for_demo_session(&state.pool, session.id).await?;
            if accepted_runs >= i64::from(state.demo_policy.max_runs_per_session) {
                return Err(too_many(
                    "maximum demo runs for this session has been reached",
                    state.demo_policy.accepted_start_window_secs,
                ));
            }

            let args = request.into_tune_request()?;
            let mut config = state.config_snapshot()?;
            config.tuning.mrft_delay_secs = Some(0);
            config.tuning.poll_interval_ms = Some(state.demo_policy.poll_interval_ms);
            config.tuning.timeout_secs = Some(state.demo_policy.run_timeout_secs);
            let prepared = prepare_owned(&state.pool, args, &config, session.id)
                .await
                .map_err(|error| prepare_error(error, &state))?;
            Ok::<_, ApiError>((session.id, prepared))
        }
        .await;
    let (session_id, prepared) = match preparation {
        Ok(prepared) => prepared,
        Err(error) => {
            state
                .demo_runtime
                .release_accepted_start(accepted_start)
                .await;
            return Err(error);
        }
    };
    let run_id = prepared.run_id();

    let (mut ctrl_c, cancel_handle) = bhtune_runtime::cancel::CtrlC::manual();
    let pool = state.pool.clone();
    let state_for_task = state.clone();
    let task = async move {
        let _global_permit = global_permit;
        let _visitor_permit = visitor_permit;
        let _ = drive(&pool, prepared, &mut ctrl_c).await;
        trim_owned_history(&state_for_task, session_id).await;
    };
    if state
        .active_run
        .start(run_id, cancel_handle, task)
        .await
        .is_err()
    {
        state
            .demo_runtime
            .release_accepted_start(accepted_start)
            .await;
        discard_unscheduled_demo_run(&state, run_id, session_id).await;
        return Err(global_capacity(
            "demo run could not be scheduled; retry shortly",
            state.demo_policy.run_timeout_secs,
        ));
    }

    let built = build_owned_run_detail(&state, run_id, session_id).await?;
    const DEMO_RUN_PRESENT: &str = "prepare_owned inserted this owner-scoped demo run";
    let detail = require_present(built, DEMO_RUN_PRESENT)?;
    Ok((StatusCode::CREATED, Json(detail)))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
