use super::helpers::*;
use super::validation::*;
use super::*;

/// List every OPC DA server registered on the bridge gateway's own host.
///
/// `GET /api/opc/servers` -- powers the GUI's server dropdown. Server discovery needs only a
/// bridge host, not a ProgID, so it cannot be a `Driver` trait method (constructing a
/// `Driver` already requires the ProgID this call exists to find); it's the free function
/// `bhtune_driver::list_opcda_servers` instead. An empty `servers` array is a normal, valid
/// answer, not an error -- only a connection failure or timeout is a 400.
#[utoipa::path(
    get,
    path = "/api/opc/servers",
    tag = "opc",
    params(OpcServersQuery),
    responses(
        (status = 200, body = OpcServersResponse),
        (status = 400, description = "The bridge gateway could not be reached in time.", body = ErrorBody),
    ),
)]
pub(crate) async fn servers(
    State(state): State<AppState>,
    Query(query): Query<OpcServersQuery>,
) -> Result<Json<OpcServersResponse>, ApiError> {
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let servers = with_timeout("list OPC DA servers", list_opcda_servers(&bridge_host)).await?;
    let gateway_compatibility = Some(
        inspect_gateway_compatibility(&bridge_host, None)
            .await
            .into(),
    );
    Ok(Json(OpcServersResponse {
        servers,
        gateway_compatibility,
    }))
}

/// Report browse capabilities for one OPC DA server.
#[utoipa::path(
    get,
    path = "/api/opc/capabilities",
    tag = "opc",
    params(OpcServerQuery),
    responses(
        (status = 200, body = OpcCapabilitiesResponse),
        (status = 400, description = "The bridge or OPC server could not be reached.", body = ErrorBody),
    ),
)]
pub(crate) async fn capabilities(
    State(state): State<AppState>,
    Query(query): Query<OpcServerQuery>,
) -> Result<Json<OpcCapabilitiesResponse>, ApiError> {
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server.clone()),
    )
    .await?;
    let report = inspect_gateway_compatibility(&bridge_host, Some(&opc_server)).await;
    let capabilities = match timed_driver_call(
        "discover OPC browse capabilities",
        Duration::from_secs(OPC_QUERY_TIMEOUT_SECS),
        driver.capabilities(),
    )
    .await
    {
        TimedDriverCall::Ready(capabilities) => {
            let mut response = OpcCapabilitiesResponse::from(capabilities);
            response.gateway_compatibility = Some((&report).into());
            response
        }
        TimedDriverCall::Incompatible { .. } => degraded_capabilities(&report),
        TimedDriverCall::Failed(error) => return Err(error),
    };
    Ok(Json(capabilities))
}

/// Return the persistent namespace-index status for one OPC DA server.
#[utoipa::path(
    get,
    path = "/api/opc/search-index/status",
    tag = "opc",
    params(OpcSearchIndexServerQuery),
    responses(
        (status = 200, body = OpcSearchIndexStatusResponse),
        (status = 400, description = "The bridge or OPC server could not be reached.", body = ErrorBody),
    ),
)]
pub(crate) async fn search_index_status(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexServerQuery>,
) -> Result<Json<OpcSearchIndexStatusResponse>, ApiError> {
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let status = with_timeout("read OPC search-index status", driver.search_index_status()).await?;
    Ok(Json(status.into()))
}

#[utoipa::path(
    get,
    path = "/api/opc/search-index/search",
    tag = "opc",
    params(OpcSearchIndexQuery),
    responses(
        (status = 200, body = OpcSearchIndexResponse),
        (status = 400, description = "The indexed-search request or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn search_index(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexQuery>,
) -> Result<Json<OpcSearchIndexResponse>, ApiError> {
    if query.query.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "a search query is required".to_string(),
        ));
    }
    let match_mode = parse_search_match_mode(&query.match_mode)?;
    let max_results = validate_positive(query.max_results, "max_results")?;
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let response = with_timeout(
        "search the OPC namespace index",
        driver.search_index(SearchIndexRequest::new(
            query.query,
            match_mode,
            max_results,
        )),
    )
    .await?;
    Ok(Json(response.into()))
}

#[utoipa::path(
    post,
    path = "/api/opc/search-index/refresh",
    tag = "opc",
    params(OpcSearchIndexRefreshQuery),
    responses(
        (status = 200, body = OpcSearchIndexStatusResponse),
        (status = 400, description = "The refresh request or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn refresh_search_index(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexRefreshQuery>,
) -> Result<Json<OpcSearchIndexStatusResponse>, ApiError> {
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let status = with_timeout(
        "refresh the OPC namespace index",
        driver.refresh_search_index(query.force.unwrap_or(false)),
    )
    .await?;
    Ok(Json(status.into()))
}

#[utoipa::path(
    post,
    path = "/api/opc/search-index/auto-refresh",
    tag = "opc",
    params(OpcSearchIndexAutoRefreshQuery),
    responses(
        (status = 200, body = OpcSearchIndexStatusResponse),
        (status = 400, description = "The auto-refresh request or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn set_search_index_auto_refresh(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexAutoRefreshQuery>,
) -> Result<Json<OpcSearchIndexStatusResponse>, ApiError> {
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let status = with_timeout(
        "set OPC namespace index auto-refresh",
        driver.set_search_index_auto_refresh(query.enabled),
    )
    .await?;
    Ok(Json(status.into()))
}

#[utoipa::path(
    delete,
    path = "/api/opc/search-index",
    tag = "opc",
    params(OpcSearchIndexServerQuery),
    responses(
        (status = 200, body = OpcSearchIndexStatusResponse),
        (status = 400, description = "The delete request or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn delete_search_index(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexServerQuery>,
) -> Result<Json<OpcSearchIndexStatusResponse>, ApiError> {
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let status = with_timeout(
        "delete the OPC namespace index",
        driver.delete_search_index(),
    )
    .await?;
    Ok(Json(status.into()))
}

#[utoipa::path(
    post,
    path = "/api/opc/search-index/control",
    tag = "opc",
    params(OpcSearchIndexControlQuery),
    responses(
        (status = 200, body = OpcSearchIndexStatusResponse),
        (status = 400, description = "The control action or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn control_search_index(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchIndexControlQuery>,
) -> Result<Json<OpcSearchIndexStatusResponse>, ApiError> {
    let action = parse_search_index_control_action(&query.action)?;
    let driver = connect_search_index_driver(&state, query.bridge_host, query.opc_server).await?;
    let status = with_timeout(
        "control the OPC namespace index",
        driver.control_search_index(action),
    )
    .await?;
    Ok(Json(status.into()))
}

/// List one bounded page of immediate children. A missing `session_id` opens a new session and
/// lists its root; all later calls round-trip the returned opaque session/node/token values.
#[utoipa::path(
    get,
    path = "/api/opc/browse",
    tag = "opc",
    params(OpcBrowseQuery),
    responses(
        (status = 200, body = OpcBrowseResponse),
        (status = 400, description = "No OPC server was specified, the browse state is invalid, or the gateway could not be reached.", body = ErrorBody),
    ),
)]
pub(crate) async fn browse(
    State(state): State<AppState>,
    Query(query): Query<OpcBrowseQuery>,
) -> Result<Json<OpcBrowseResponse>, ApiError> {
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let page_size = validate_positive(query.page_size, "page_size")?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server.clone()),
    )
    .await?;
    let request = BrowsePageRequest {
        session_id: query.session_id,
        parent_node_key: query.parent_node_key,
        page_token: query.page_token,
        page_size,
        refresh: query.refresh.unwrap_or(false),
    };
    let report = inspect_gateway_compatibility(&bridge_host, Some(&opc_server)).await;
    let page = match timed_driver_call(
        "browse OPC DA namespace",
        Duration::from_secs(OPC_QUERY_TIMEOUT_SECS),
        driver.browse(request),
    )
    .await
    {
        TimedDriverCall::Ready(page) => {
            let mut response = OpcBrowseResponse::from(page);
            response.gateway_compatibility = Some((&report).into());
            response
        }
        TimedDriverCall::Incompatible { operation } => degraded_browse(&report, operation),
        TimedDriverCall::Failed(error) => return Err(error),
    };
    Ok(Json(page))
}

#[utoipa::path(
    delete,
    path = "/api/opc/browse/sessions/{session_id}",
    tag = "opc",
    params(
        ("session_id" = String, Path, description = "Opaque bridge browse-session ID."),
        OpcServerQuery
    ),
    responses(
        (status = 200, body = OpcCloseBrowseSessionResponse),
        (status = 400, description = "The browse session could not be closed.", body = ErrorBody),
    ),
)]
pub(crate) async fn close_browse_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(query): Query<OpcServerQuery>,
) -> Result<Json<OpcCloseBrowseSessionResponse>, ApiError> {
    if session_id.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "a browse session ID is required".to_string(),
        ));
    }
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server),
    )
    .await?;
    with_timeout(
        "close OPC browse session",
        driver.close_browse_session(&session_id),
    )
    .await?;
    Ok(Json(OpcCloseBrowseSessionResponse { closed: true }))
}

#[utoipa::path(
    get,
    path = "/api/opc/search",
    tag = "opc",
    params(OpcSearchQuery),
    responses(
        (status = 200, description = "SSE stream of match, progress, and completed events."),
        (status = 400, description = "The search request or gateway connection is invalid.", body = ErrorBody),
    ),
)]
pub(crate) async fn search(
    State(state): State<AppState>,
    Query(query): Query<OpcSearchQuery>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if query.query.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "a search query is required".to_string(),
        ));
    }
    let match_mode = parse_search_match_mode(&query.match_mode)?;
    let max_results = validate_positive(query.max_results, "max_results")?;
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server.clone()),
    )
    .await?;
    let request = SearchRequest {
        query: query.query,
        match_mode,
        session_id: query.session_id,
        scope_node_key: query.scope_node_key,
        max_results,
        include_branches: query.include_branches.unwrap_or(false),
        refresh: query.refresh.unwrap_or(false),
    };
    let mut stream =
        with_timeout("start OPC namespace search", driver.search_stream(request)).await?;
    let events = async_stream::stream! {
        loop {
            match tokio::time::timeout(
                Duration::from_secs(OPC_QUERY_TIMEOUT_SECS),
                stream.next(),
            )
            .await
            {
                Ok(Ok(Some(event))) => yield Ok(search_event_to_sse(event)),
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    yield Ok(Event::default().event("error").data(
                        serde_json::json!({"error": error.to_string()}).to_string(),
                    ));
                    break;
                }
                Err(_) => {
                    yield Ok(Event::default().event("error").data(
                        serde_json::json!({
                            "error": format!(
                                "namespace search: no response within {OPC_QUERY_TIMEOUT_SECS}s"
                            )
                        }).to_string(),
                    ));
                    break;
                }
            }
        }
    };
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

/// Read one tag's current value, quality, and timestamp.
///
/// `GET /api/opc/read` -- backs the GUI's "Test connection" button (read the tag the user is
/// about to use as the loop's PV and show what comes back) and the tag-tree's live preview.
/// Deliberately does not enforce [`bhtune_driver::Quality::is_trustworthy`] the way a real
/// tune's readings must -- this is a diagnostic command, so it reports whatever quality it
/// gets rather than failing on `Uncertain`/`Bad`, matching `bhtune opc read`'s own behavior.
#[utoipa::path(
    get,
    path = "/api/opc/read",
    tag = "opc",
    params(OpcReadQuery),
    responses(
        (status = 200, body = OpcReadResponse),
        (status = 400, description = "No tag or OPC server was specified (and none is configured), or the gateway/read call could not be reached in time.", body = ErrorBody),
    ),
)]
pub(crate) async fn read(
    State(state): State<AppState>,
    Query(query): Query<OpcReadQuery>,
) -> Result<Json<OpcReadResponse>, ApiError> {
    let tag = query
        .tag
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("a tag is required".to_string()))?;
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server.clone()),
    )
    .await?;
    let gateway_compatibility = Some(
        inspect_gateway_compatibility(&bridge_host, Some(&opc_server))
            .await
            .into(),
    );
    let values = with_timeout(
        &format!("read '{tag}'"),
        driver.read(std::slice::from_ref(&tag)),
    )
    .await?;
    let value = values.into_iter().next().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!("driver returned no value for tag '{tag}'"))
    })?;
    Ok(Json(OpcReadResponse {
        tag: value.tag,
        value: value.value,
        quality: sample_quality_from_driver(value.quality),
        timestamp: value.timestamp,
        gateway_compatibility,
    }))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
