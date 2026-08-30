//! OPC DA diagnostic routes: `GET /api/opc/servers`, `GET /api/opc/capabilities`,
//! `GET /api/opc/browse`, `DELETE /api/opc/browse/sessions/:id`, `GET /api/opc/search`,
//! `GET /api/opc/read`, and the indexed-search status/refresh/control routes -- back the GUI's
//! server dropdown, typed tag-tree browser, namespace search, index management, and connection
//! test.
//!
//! Read-only diagnostics remain independent of [`crate::state::AppState::active_run`], and every
//! OPC operation is bounded by an explicit [`OPC_QUERY_TIMEOUT_SECS`] timeout (see that constant's
//! doc comment for why one is needed at all). Index management does not acquire the active-tune
//! lock either: starting or controlling a gateway inventory must not block, or be blocked by, an
//! in-flight tune.

use std::convert::Infallible;
use std::future::Future;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bhtune_cli::commands::tune::sample_quality_from_driver;
use bhtune_db::models::SampleQuality;
use bhtune_driver::{
    BrowseNode, BrowseNodeKind, BrowsePage, BrowsePageRequest, BrowseSource, Driver,
    DriverCapabilities, DriverResult, IndexedSearchMatch, IndexedSearchProgress,
    NamespaceOrganization, OpcDaDriver, SearchEvent, SearchIndexControlAction, SearchIndexRequest,
    SearchIndexResponse, SearchIndexStatus, SearchMatch, SearchMatchMode, SearchRequest,
    list_opcda_servers,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;

/// Bounds every OPC DA call this module makes. `opcda_bridge::Client::connect` has no
/// connect timeout of its own (plain `tonic`, no `connect_timeout` configured), so a
/// firewalled or black-holed gateway host would otherwise hang a request for however long
/// the OS's own TCP-connect timeout is -- potentially minutes. 30s matches
/// `bhtune-cli`'s `default_op_or_restore_timeout_secs()` (also 30) for consistency with the
/// rest of the codebase, rather than inventing a new number. Each connect/browse/read call
/// gets its own separate budget via [`with_timeout`], not one combined timeout spanning
/// connect *and* the operation together.
const OPC_QUERY_TIMEOUT_SECS: u64 = 30;

/// Runs `fut` (one OPC DA driver call) under an [`OPC_QUERY_TIMEOUT_SECS`] deadline, mapping
/// both a [`bhtune_driver::DriverError`] and an elapsed deadline to [`ApiError::BadRequest`]
/// -- every failure this module can produce is "the gateway/tag couldn't be reached in
/// time", a client-actionable diagnostic outcome, never an [`ApiError::Internal`] bug in this
/// server. `what` names the attempted operation (e.g. `"connect to OPC server 'X'"`) so the
/// error message identifies which step failed.
async fn with_timeout<T>(
    what: &str,
    fut: impl Future<Output = DriverResult<T>>,
) -> Result<T, ApiError> {
    match tokio::time::timeout(Duration::from_secs(OPC_QUERY_TIMEOUT_SECS), fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(ApiError::BadRequest(format!("{what}: {err}"))),
        Err(_) => Err(ApiError::BadRequest(format!(
            "{what}: no response within {OPC_QUERY_TIMEOUT_SECS}s"
        ))),
    }
}

/// Query parameters for `GET /api/opc/servers`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcServersQuery {
    /// Overrides the configured/default bridge host for this one request, matching `bhtune
    /// opc servers --bridge-host`.
    pub bridge_host: Option<String>,
}

/// Response body of `GET /api/opc/servers`.
#[derive(Debug, Serialize, ToSchema)]
pub struct OpcServersResponse {
    pub servers: Vec<String>,
}

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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let servers = with_timeout("list OPC DA servers", list_opcda_servers(&bridge_host)).await?;
    Ok(Json(OpcServersResponse { servers }))
}

/// Query parameters for `GET /api/opc/capabilities`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcServerQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcCapabilitiesResponse {
    pub application_version: String,
    pub protocol_version: String,
    pub max_page_size: u32,
    pub supports_browse_sessions: bool,
    pub supports_search: bool,
    pub organization: String,
    pub source: String,
    pub supports_indexed_search: bool,
    pub indexed_search_protocol_version: String,
    pub max_indexed_search_results: u32,
    pub search_index_state: String,
}

impl From<DriverCapabilities> for OpcCapabilitiesResponse {
    fn from(capabilities: DriverCapabilities) -> Self {
        Self {
            application_version: capabilities.application_version,
            protocol_version: capabilities.protocol_version,
            max_page_size: capabilities.max_page_size,
            supports_browse_sessions: capabilities.supports_browse_sessions,
            supports_search: capabilities.supports_search,
            organization: organization_name(capabilities.organization).to_string(),
            source: source_name(capabilities.source).to_string(),
            supports_indexed_search: capabilities.supports_indexed_search,
            indexed_search_protocol_version: capabilities.indexed_search_protocol_version,
            max_indexed_search_results: capabilities.max_indexed_search_results,
            search_index_state: capabilities.search_index_state.to_string(),
        }
    }
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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server),
    )
    .await?;
    let capabilities =
        with_timeout("discover OPC browse capabilities", driver.capabilities()).await?;
    Ok(Json(capabilities.into()))
}

/// Query parameters shared by indexed-search status and search-index actions.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexServerQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcIndexedSearchProgressResponse {
    pub branches_visited: u64,
    pub entries_seen: u64,
    pub unique_items: u64,
    pub active_time_ms: u64,
    pub paused_time_ms: u64,
    pub items_per_second: f64,
    pub estimated_remaining_ms: Option<u64>,
}

impl From<IndexedSearchProgress> for OpcIndexedSearchProgressResponse {
    fn from(progress: IndexedSearchProgress) -> Self {
        Self {
            branches_visited: progress.branches_visited,
            entries_seen: progress.entries_seen,
            unique_items: progress.unique_items,
            active_time_ms: progress.active_time_ms,
            paused_time_ms: progress.paused_time_ms,
            items_per_second: progress.items_per_second,
            estimated_remaining_ms: progress.estimated_remaining_ms,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcSearchIndexStatusResponse {
    pub server: String,
    pub state: String,
    pub configured: bool,
    pub active_generation: u64,
    pub entry_count: u64,
    pub unique_item_count: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub last_error: Option<String>,
    pub database_bytes: u64,
    pub organization: String,
    pub source: String,
    pub progress: Option<OpcIndexedSearchProgressResponse>,
}

impl From<SearchIndexStatus> for OpcSearchIndexStatusResponse {
    fn from(status: SearchIndexStatus) -> Self {
        Self {
            server: status.server,
            state: status.state.to_string(),
            configured: status.configured,
            active_generation: status.active_generation,
            entry_count: status.entry_count,
            unique_item_count: status.unique_item_count,
            started_at: status.started_at,
            completed_at: status.completed_at,
            last_error: status.last_error,
            database_bytes: status.database_bytes,
            organization: organization_name(status.organization).to_string(),
            source: source_name(status.source).to_string(),
            progress: status.progress.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcIndexedSearchMatchResponse {
    pub item_id: String,
    pub display_name: String,
    pub kind: OpcBrowseNodeKind,
    pub breadcrumbs: Vec<String>,
}

impl From<IndexedSearchMatch> for OpcIndexedSearchMatchResponse {
    fn from(found: IndexedSearchMatch) -> Self {
        Self {
            item_id: found.item_id,
            display_name: found.display_name,
            kind: found.kind.into(),
            breadcrumbs: found.breadcrumbs,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcSearchIndexResponse {
    pub matches: Vec<OpcIndexedSearchMatchResponse>,
    pub has_more: bool,
    pub status: OpcSearchIndexStatusResponse,
}

impl From<SearchIndexResponse> for OpcSearchIndexResponse {
    fn from(response: SearchIndexResponse) -> Self {
        Self {
            matches: response.matches.into_iter().map(Into::into).collect(),
            has_more: response.has_more,
            status: response.status.into(),
        }
    }
}

async fn connect_search_index_driver(
    state: &AppState,
    bridge_host: Option<String>,
    opc_server: Option<String>,
) -> Result<OpcDaDriver, ApiError> {
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_cli::config::resolve_bridge_host(bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server),
    )
    .await
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

/// Query the gateway-owned persistent namespace index. This is a bounded unary request and
/// never falls back to the legacy live traversal search.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub query: String,
    #[serde(default = "default_search_match_mode")]
    pub match_mode: String,
    #[serde(default = "default_index_search_max_results")]
    #[param(minimum = 1)]
    pub max_results: u32,
}

fn default_index_search_max_results() -> u32 {
    bhtune_driver::DEFAULT_INDEX_SEARCH_MAX_RESULTS
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

/// Query parameters for `POST /api/opc/search-index/refresh`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexRefreshQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub force: Option<bool>,
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

/// Query parameters for `POST /api/opc/search-index/control`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexControlQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub action: String,
}

fn parse_search_index_control_action(value: &str) -> Result<SearchIndexControlAction, ApiError> {
    match value {
        "pause" => Ok(SearchIndexControlAction::Pause),
        "resume" => Ok(SearchIndexControlAction::Resume),
        "cancel" => Ok(SearchIndexControlAction::Cancel),
        _ => Err(ApiError::BadRequest(
            "action must be one of: pause, resume, cancel".to_string(),
        )),
    }
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

/// Query parameters for `GET /api/opc/browse`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcBrowseQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub session_id: Option<String>,
    pub parent_node_key: Option<String>,
    pub page_token: Option<String>,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    pub refresh: Option<bool>,
}

fn default_page_size() -> u32 {
    bhtune_driver::DEFAULT_PAGE_SIZE
}

fn validate_positive(value: u32, field: &str) -> Result<u32, ApiError> {
    if value == 0 {
        return Err(ApiError::BadRequest(format!(
            "{field} must be greater than zero"
        )));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpcBrowseNodeKind {
    Unspecified,
    Branch,
    Item,
    BranchAndItem,
}

impl From<BrowseNodeKind> for OpcBrowseNodeKind {
    fn from(kind: BrowseNodeKind) -> Self {
        match kind {
            BrowseNodeKind::Unspecified => Self::Unspecified,
            BrowseNodeKind::Branch => Self::Branch,
            BrowseNodeKind::Item => Self::Item,
            BrowseNodeKind::BranchAndItem => Self::BranchAndItem,
        }
    }
}

/// One node returned by `GET /api/opc/browse`. `node_key` and `item_id` must remain separate:
/// the former is an opaque navigation key, while the latter is the exact selectable OPC DA
/// ItemID and may contain namespace punctuation with no relationship to hierarchy.
#[derive(Debug, Serialize, ToSchema)]
pub struct OpcBrowseNodeResponse {
    pub node_key: String,
    pub display_name: String,
    pub kind: OpcBrowseNodeKind,
    pub item_id: Option<String>,
}

impl From<BrowseNode> for OpcBrowseNodeResponse {
    fn from(node: BrowseNode) -> Self {
        Self {
            node_key: node.node_key,
            display_name: node.display_name,
            kind: node.kind.into(),
            item_id: node.item_id,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcBrowseResponse {
    pub session_id: String,
    pub nodes: Vec<OpcBrowseNodeResponse>,
    pub next_page_token: Option<String>,
    pub complete: bool,
    pub organization: String,
    pub source: String,
    pub warning: Option<String>,
}

impl From<BrowsePage> for OpcBrowseResponse {
    fn from(page: BrowsePage) -> Self {
        Self {
            session_id: page.session_id,
            nodes: page.nodes.into_iter().map(Into::into).collect(),
            next_page_token: page.next_page_token,
            complete: page.complete,
            organization: organization_name(page.organization).to_string(),
            source: source_name(page.source).to_string(),
            warning: page.warning,
        }
    }
}

fn organization_name(value: NamespaceOrganization) -> &'static str {
    match value {
        NamespaceOrganization::Unspecified => "unspecified",
        NamespaceOrganization::Flat => "flat",
        NamespaceOrganization::Hierarchical => "hierarchical",
    }
}

fn source_name(value: BrowseSource) -> &'static str {
    match value {
        BrowseSource::Unspecified => "unspecified",
        BrowseSource::Da3 => "da3",
        BrowseSource::Da2 => "da2",
        BrowseSource::Flat => "flat",
        BrowseSource::Derived => "derived",
    }
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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(query.opc_server, &config)
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
    let page = with_timeout("browse OPC DA namespace", driver.browse(request)).await?;
    Ok(Json(page.into()))
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcCloseBrowseSessionResponse {
    pub closed: bool,
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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(query.opc_server, &config)
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

/// Query parameters for the progressive `GET /api/opc/search` SSE endpoint.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub query: String,
    #[serde(default = "default_search_match_mode")]
    pub match_mode: String,
    pub session_id: Option<String>,
    pub scope_node_key: Option<String>,
    #[serde(default = "default_search_max_results")]
    #[param(minimum = 1)]
    pub max_results: u32,
    pub include_branches: Option<bool>,
    pub refresh: Option<bool>,
}

fn default_search_match_mode() -> String {
    "contains".to_string()
}

fn default_search_max_results() -> u32 {
    bhtune_driver::DEFAULT_SEARCH_MAX_RESULTS
}

fn parse_search_match_mode(value: &str) -> Result<SearchMatchMode, ApiError> {
    match value {
        "exact" => Ok(SearchMatchMode::Exact),
        "prefix" => Ok(SearchMatchMode::Prefix),
        "contains" => Ok(SearchMatchMode::Contains),
        _ => Err(ApiError::BadRequest(
            "match_mode must be one of: exact, prefix, contains".to_string(),
        )),
    }
}

fn search_event_to_sse(event: SearchEvent) -> Event {
    let (kind, payload) = match event {
        SearchEvent::Match(found) => ("match", json_search_match(&found)),
        SearchEvent::Progress(progress) => (
            "progress",
            serde_json::json!({
                "visited_nodes": progress.visited_nodes,
                "matches": progress.matches,
                "partial": progress.partial,
            }),
        ),
        SearchEvent::Completed(completed) => (
            "completed",
            serde_json::json!({
                "complete": completed.complete,
                "cancelled": completed.cancelled,
                "truncated": completed.truncated,
                "warning": completed.warning,
            }),
        ),
    };
    Event::default().event(kind).data(payload.to_string())
}

fn json_search_match(found: &SearchMatch) -> serde_json::Value {
    serde_json::json!({
        "node": {
            "node_key": found.node.node_key,
            "display_name": found.node.display_name,
            "kind": browse_node_kind_name(found.node.kind),
            "item_id": found.node.item_id,
        },
        "breadcrumbs": found.breadcrumbs.iter().map(|part| {
            serde_json::json!({
                "node_key": part.node_key,
                "display_name": part.display_name,
            })
        }).collect::<Vec<_>>(),
    })
}

fn browse_node_kind_name(kind: BrowseNodeKind) -> &'static str {
    match kind {
        BrowseNodeKind::Unspecified => "unspecified",
        BrowseNodeKind::Branch => "branch",
        BrowseNodeKind::Item => "item",
        BrowseNodeKind::BranchAndItem => "branch_and_item",
    }
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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(query.opc_server, &config)
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

/// Query parameters for `GET /api/opc/read`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcReadQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    /// The fully qualified tag to read (e.g. `"Unit1.LIC101.PV"`). Required -- unlike the
    /// other two fields, there is no configured default for "which tag".
    pub tag: Option<String>,
}

/// Response body of `GET /api/opc/read`.
///
/// `quality` reuses [`SampleQuality`] rather than a third quality representation --
/// `bhtune-db`'s `SampleQuality` (mapped from the driver's live [`bhtune_driver::Quality`] by
/// [`sample_quality_from_driver`]) is already exposed directly over HTTP in
/// `GET /api/runs/{id}`'s `SampleResponse::pv_quality` (see `routes::history`), so this
/// follows that same precedent instead of inventing a parallel `OpcQualityResponse` enum.
#[derive(Debug, Serialize, ToSchema)]
pub struct OpcReadResponse {
    pub tag: String,
    pub value: String,
    pub quality: SampleQuality,
    /// Always `null` for the OPC DA driver today: the gateway's last-change time is a
    /// *local*, offset-less string with no reliable way to convert it to a trustworthy
    /// `DateTime<Utc>` (see `bhtune_driver::opcda::tag_value_from_raw`'s doc comment) -- kept
    /// as a field rather than dropped entirely so a future driver that *can* supply a
    /// trustworthy instant (or a bridge protocol revision that reports the gateway's own
    /// timezone) doesn't need an API shape change to start populating it.
    pub timestamp: Option<DateTime<Utc>>,
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
    let bridge_host = bhtune_cli::config::resolve_bridge_host(query.bridge_host, &config);
    let opc_server = bhtune_cli::config::resolve_server(query.opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    let driver = with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server.clone()),
    )
    .await?;
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
    }))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/opc/servers", get(servers))
        .route("/api/opc/capabilities", get(capabilities))
        .route("/api/opc/browse", get(browse))
        .route(
            "/api/opc/browse/sessions/{session_id}",
            delete(close_browse_session),
        )
        .route("/api/opc/search", get(search))
        .route("/api/opc/search-index/status", get(search_index_status))
        .route("/api/opc/search-index/search", get(search_index))
        .route("/api/opc/search-index/refresh", post(refresh_search_index))
        .route("/api/opc/search-index/control", post(control_search_index))
        .route("/api/opc/read", get(read))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::mock_bridge::{MockBridgeService, start_mock_server};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use opcda_bridge_proto::bridge::{
        BrowseNode as ProtoBrowseNode, BrowseNodeKind as ProtoBrowseNodeKind, BrowsePage,
        BrowseSource as ProtoBrowseSource, IndexedSearchMatch as ProtoIndexedSearchMatch,
        IndexedSearchProgress as ProtoIndexedSearchProgress, ListServersResponse,
        NamespaceOrganization as ProtoNamespaceOrganization, ReadResponse,
        SearchIndexResponse as ProtoSearchIndexResponse, SearchIndexState as ProtoSearchIndexState,
        SearchIndexStatus as ProtoSearchIndexStatus, SearchMatchMode as ProtoSearchMatchMode,
        TagValue as ProtoTagValue,
    };
    use tonic::Code;
    use tower::ServiceExt;

    async fn body_json(response: axum::http::Response<Body>) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn get(app: axum::Router, path: &str) -> axum::http::Response<Body> {
        app.oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn post(app: axum::Router, path: &str) -> axum::http::Response<Body> {
        app.oneshot(Request::post(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn proto_index_status(state: ProtoSearchIndexState) -> ProtoSearchIndexStatus {
        ProtoSearchIndexStatus {
            server: "Sim.Server".to_string(),
            state: state as i32,
            configured: true,
            active_generation: 7,
            entry_count: 12_345,
            unique_item_count: 9_876,
            started_at: Some("2026-08-16T10:00:00Z".to_string()),
            completed_at: Some("2026-08-16T10:05:00Z".to_string()),
            last_error: None,
            database_bytes: 65_536,
            organization: ProtoNamespaceOrganization::Hierarchical as i32,
            source: ProtoBrowseSource::Da3 as i32,
            progress: Some(ProtoIndexedSearchProgress {
                branches_visited: 321,
                entries_seen: 12_345,
                unique_items: 9_876,
                active_time_ms: 240_000,
                paused_time_ms: 60_000,
                items_per_second: 250.5,
                estimated_remaining_ms: Some(30_000),
            }),
        }
    }
    /// A fresh in-memory [`AppState`] with `bridge_host`/`opc_server` overridden -- every
    /// test in this module needs the config resolution to pick up a specific mock gateway
    /// (or a deliberately unreachable/unset one), never the four seeded built-in templates.
    async fn state_with(bridge_host: Option<&str>, opc_server: Option<&str>) -> AppState {
        let state = crate::test_support::in_memory_state().await;
        let mut store = state.config_store.write().unwrap();
        store.config.bridge_host = bridge_host.map(str::to_string);
        store.config.server = opc_server.map(str::to_string);
        drop(store);
        state
    }

    #[tokio::test]
    async fn servers_returns_every_registered_server_from_a_mock_gateway() {
        let host = start_mock_server(MockBridgeService {
            list_servers_response: ListServersResponse {
                servers: vec![
                    "Matrikon.OPC.Simulation.1".to_string(),
                    "Kepware.KEPServerEX.V6".to_string(),
                ],
            },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), None).await);

        let response = get(app, "/api/opc/servers").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["servers"],
            serde_json::json!(["Matrikon.OPC.Simulation.1", "Kepware.KEPServerEX.V6"])
        );
    }

    #[tokio::test]
    async fn servers_handles_an_empty_result() {
        let host = start_mock_server(MockBridgeService::default()).await;
        let app = crate::build_router(state_with(Some(&host), None).await);

        let response = get(app, "/api/opc/servers").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            body_json(response).await,
            serde_json::json!({"servers": []})
        );
    }

    #[tokio::test]
    async fn servers_returns_400_when_the_gateway_is_unreachable() {
        let app = crate::build_router(state_with(Some("127.0.0.1:1"), None).await);

        let response = get(app, "/api/opc/servers").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("list OPC DA servers")
        );
    }

    #[tokio::test]
    async fn servers_query_param_overrides_the_configured_bridge_host() {
        let host = start_mock_server(MockBridgeService::default()).await;
        // Config points at an unreachable host; the query param must win.
        let app = crate::build_router(state_with(Some("127.0.0.1:1"), None).await);

        let response = get(app, &format!("/api/opc/servers?bridge_host={host}")).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn capabilities_reports_the_indexed_search_contract() {
        let host = start_mock_server(MockBridgeService {
            capabilities_response: opcda_bridge_proto::bridge::GetCapabilitiesResponse {
                application_version: "0.4.0".to_string(),
                protocol_version: "2".to_string(),
                max_page_size: 1000,
                supports_browse_sessions: true,
                supports_search: true,
                organization: ProtoNamespaceOrganization::Hierarchical as i32,
                source: ProtoBrowseSource::Da3 as i32,
                supports_indexed_search: true,
                indexed_search_protocol_version: "1".to_string(),
                max_indexed_search_results: 50,
                search_index_state: ProtoSearchIndexState::Ready as i32,
            },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/capabilities").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["application_version"], "0.4.0");
        assert_eq!(body["protocol_version"], "2");
        assert_eq!(body["supports_indexed_search"], true);
        assert_eq!(body["indexed_search_protocol_version"], "1");
        assert_eq!(body["max_indexed_search_results"], 50);
        assert_eq!(body["search_index_state"], "ready");
    }

    #[tokio::test]
    async fn search_index_status_maps_every_state_and_progress() {
        let states = [
            (ProtoSearchIndexState::Unspecified, "unspecified"),
            (ProtoSearchIndexState::NotIndexed, "not_indexed"),
            (ProtoSearchIndexState::Partial, "partial"),
            (ProtoSearchIndexState::Ready, "ready"),
            (ProtoSearchIndexState::Stale, "stale"),
            (ProtoSearchIndexState::Refreshing, "refreshing"),
            (ProtoSearchIndexState::Failed, "failed"),
        ];

        for (state, expected_state) in states {
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let host = start_mock_server(MockBridgeService {
                search_index_status_response: proto_index_status(state),
                search_index_status_requests: requests.clone(),
                ..Default::default()
            })
            .await;
            let app = crate::build_router(state_with(Some(&host), None).await);

            let response = get(app, "/api/opc/search-index/status?opc_server=Sim.Server").await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_json(response).await;
            assert_eq!(body["server"], "Sim.Server");
            assert_eq!(body["state"], expected_state);
            assert_eq!(body["configured"], true);
            assert_eq!(body["active_generation"], 7);
            assert_eq!(body["entry_count"], 12_345);
            assert_eq!(body["unique_item_count"], 9_876);
            assert_eq!(body["started_at"], "2026-08-16T10:00:00Z");
            assert_eq!(body["completed_at"], "2026-08-16T10:05:00Z");
            assert_eq!(body["database_bytes"], 65_536);
            assert_eq!(body["organization"], "hierarchical");
            assert_eq!(body["source"], "da3");
            assert_eq!(body["progress"]["branches_visited"], 321);
            assert_eq!(body["progress"]["entries_seen"], 12_345);
            assert_eq!(body["progress"]["unique_items"], 9_876);
            assert_eq!(body["progress"]["active_time_ms"], 240_000);
            assert_eq!(body["progress"]["paused_time_ms"], 60_000);
            assert_eq!(body["progress"]["items_per_second"], 250.5);
            assert_eq!(body["progress"]["estimated_remaining_ms"], 30_000);
            assert_eq!(
                requests.lock().unwrap().as_slice(),
                &[opcda_bridge_proto::bridge::GetSearchIndexStatusRequest {
                    server: "Sim.Server".to_string(),
                }]
            );
        }
    }

    #[tokio::test]
    async fn search_index_returns_exact_matches_status_and_has_more() {
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = start_mock_server(MockBridgeService {
            search_index_response: ProtoSearchIndexResponse {
                matches: vec![ProtoIndexedSearchMatch {
                    item_id: "FCS0201!204FI00510.PV".to_string(),
                    display_name: "PV".to_string(),
                    kind: ProtoBrowseNodeKind::BranchAndItem as i32,
                    breadcrumbs: vec!["FCS0201".to_string(), "204FI00510".to_string()],
                }],
                has_more: true,
                status: Some(proto_index_status(ProtoSearchIndexState::Ready)),
            },
            search_index_requests: requests.clone(),
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), None).await);

        let response = get(
            app,
            "/api/opc/search-index/search?opc_server=Sim.Server&query=204FI00510&match_mode=prefix&max_results=7",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(
            body["matches"],
            serde_json::json!([{
                "item_id": "FCS0201!204FI00510.PV",
                "display_name": "PV",
                "kind": "branch_and_item",
                "breadcrumbs": ["FCS0201", "204FI00510"],
            }])
        );
        assert_eq!(body["has_more"], true);
        assert_eq!(body["status"]["state"], "ready");

        let request = &requests.lock().unwrap()[0];
        assert_eq!(request.server, "Sim.Server");
        assert_eq!(request.query, "204FI00510");
        assert_eq!(request.match_mode, ProtoSearchMatchMode::Prefix as i32);
        assert_eq!(request.max_results, 7);
    }

    #[tokio::test]
    async fn search_index_refresh_and_control_forward_actions() {
        let refresh_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let control_requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = start_mock_server(MockBridgeService {
            search_index_status_response: proto_index_status(ProtoSearchIndexState::Refreshing),
            refresh_search_index_requests: refresh_requests.clone(),
            control_search_index_requests: control_requests.clone(),
            ..Default::default()
        })
        .await;

        let refresh_app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
        let response = post(refresh_app, "/api/opc/search-index/refresh?force=true").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["state"], "refreshing");
        assert_eq!(
            refresh_requests.lock().unwrap().as_slice(),
            &[opcda_bridge_proto::bridge::RefreshSearchIndexRequest {
                server: "Sim.Server".to_string(),
                force: true,
            }]
        );

        let control_app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
        let response = post(control_app, "/api/opc/search-index/control?action=resume").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_json(response).await["state"], "refreshing");
        assert_eq!(
            control_requests.lock().unwrap().as_slice(),
            &[opcda_bridge_proto::bridge::ControlSearchIndexRequest {
                server: "Sim.Server".to_string(),
                action: opcda_bridge_proto::bridge::SearchIndexControlAction::Resume as i32,
            }]
        );
    }

    #[tokio::test]
    async fn indexed_search_routes_reject_invalid_input_before_connecting() {
        let cases = [
            (
                "/api/opc/search-index/search?query=%20&opc_server=Sim.Server",
                "search query is required",
            ),
            (
                "/api/opc/search-index/search?query=PV&match_mode=wildcard&opc_server=Sim.Server",
                "match_mode must be one of",
            ),
            (
                "/api/opc/search-index/search?query=PV&max_results=0&opc_server=Sim.Server",
                "max_results must be greater than zero",
            ),
            (
                "/api/opc/search-index/control?action=stop&opc_server=Sim.Server",
                "action must be one of",
            ),
        ];

        for (path, expected_error) in cases {
            let app = crate::build_router(state_with(None, None).await);
            let response = if path.contains("/control") {
                post(app, path).await
            } else {
                get(app, path).await
            };
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = body_json(response).await;
            assert!(body["error"].as_str().unwrap().contains(expected_error));
        }
    }

    #[tokio::test]
    async fn indexed_search_routes_surface_gateway_errors() {
        let host = start_mock_server(MockBridgeService {
            search_index_status_error: Some(tonic::Status::internal("status failed")),
            search_index_error: Some(tonic::Status::internal("search failed")),
            refresh_search_index_error: Some(tonic::Status::internal("refresh failed")),
            control_search_index_error: Some(tonic::Status::internal("control failed")),
            ..Default::default()
        })
        .await;
        let paths = [
            (
                "/api/opc/search-index/status?opc_server=Sim.Server",
                "read OPC search-index status",
                false,
            ),
            (
                "/api/opc/search-index/search?query=PV&opc_server=Sim.Server",
                "search the OPC namespace index",
                false,
            ),
            (
                "/api/opc/search-index/refresh?opc_server=Sim.Server",
                "refresh the OPC namespace index",
                true,
            ),
            (
                "/api/opc/search-index/control?action=pause&opc_server=Sim.Server",
                "control the OPC namespace index",
                true,
            ),
        ];

        for (path, operation, is_post) in paths {
            let app = crate::build_router(state_with(Some(&host), None).await);
            let response = if is_post {
                post(app, path).await
            } else {
                get(app, path).await
            };
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let error = body_json(response).await["error"]
                .as_str()
                .unwrap()
                .to_string();
            assert!(error.contains(operation), "{error}");
            assert!(error.contains("failed"), "{error}");
        }
    }

    #[tokio::test]
    async fn indexed_search_status_reports_an_unsupported_gateway() {
        let host = start_mock_server(MockBridgeService {
            search_index_status_error: Some(tonic::Status::new(
                Code::Unimplemented,
                "indexed search is unavailable",
            )),
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), None).await);

        let response = get(app, "/api/opc/search-index/status?opc_server=Sim.Server").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error = body_json(response).await["error"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(error.contains("does not support indexed-search status"));
        assert!(
            error.contains("upgrade the OPC DA bridge gateway"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn browse_returns_nodes_from_a_mock_gateway() {
        let host = start_mock_server(MockBridgeService {
            browse_response: BrowsePage {
                session_id: "session".to_string(),
                nodes: vec![ProtoBrowseNode {
                    node_key: "unit1".to_string(),
                    display_name: "Unit1".to_string(),
                    kind: ProtoBrowseNodeKind::Branch as i32,
                    item_id: None,
                }],
                complete: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/browse").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["session_id"], "session");
        assert_eq!(body["nodes"][0]["display_name"], "Unit1");
        assert_eq!(body["nodes"][0]["kind"], "branch");
    }

    #[tokio::test]
    async fn browse_handles_an_empty_result() {
        let host = start_mock_server(MockBridgeService::default()).await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(
            app,
            "/api/opc/browse?session_id=session&parent_node_key=unit1",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["nodes"], serde_json::json!([]));
        assert_eq!(body["complete"], true);
    }

    #[tokio::test]
    async fn browse_returns_400_when_no_server_is_configured() {
        let app = crate::build_router(state_with(None, None).await);

        let response = get(app, "/api/opc/browse").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("no OPC server specified")
        );
    }

    #[tokio::test]
    async fn browse_returns_400_when_the_gateway_is_unreachable() {
        let app = crate::build_router(state_with(Some("127.0.0.1:1"), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/browse").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("connect to OPC server")
        );
    }

    #[tokio::test]
    async fn read_returns_the_value_quality_and_timestamp_from_a_mock_gateway() {
        let host = start_mock_server(MockBridgeService {
            // Constructed directly (rather than via `good_reading`, which hardcodes an
            // `"ignored"` tag_id -- fine for `runs.rs`'s tests, which don't echo it back, but
            // this handler does) so the mocked response's tag_id matches what was requested,
            // matching `bhtune-cli`'s own `read_prints_values_from_a_mock_gateway` precedent.
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Unit1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["tag"], "Unit1.LIC101.PV");
        assert_eq!(body["value"], "42.5");
        assert_eq!(body["quality"], "good");
        // Always `null` today -- the OPC DA driver never trusts the gateway's local,
        // offset-less timestamp string enough to convert it (see `OpcReadResponse::timestamp`'s
        // doc comment); asserting `is_null()` here (not `is_string()`) documents that as
        // deliberate rather than a bug were someone to "fix" it later without reading why.
        assert!(body["timestamp"].is_null());
    }

    #[tokio::test]
    async fn read_returns_500_when_the_driver_reports_success_but_no_value() {
        // A well-behaved driver never does this for a single requested tag -- but the mock
        // gateway's `read` RPC ignores the actual request and returns whatever
        // `read_response` is configured with (see `test_support::mock_bridge`'s `read`
        // implementation), which makes this defensive `ApiError::Internal` branch (a bug in
        // the driver, not a client mistake) reachable in a test without needing a real
        // misbehaving gateway.
        let host = start_mock_server(MockBridgeService {
            read_response: ReadResponse { values: vec![] },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = body_json(response).await;
        // The tag name and "driver returned no value" detail are logged (`tracing::error!`
        // in `error.rs`'s `IntoResponse` impl), never sent to the client -- matching every
        // other `ApiError::Internal` response in this codebase.
        assert_eq!(body["error"], "internal server error");
    }

    #[tokio::test]
    async fn read_requires_a_tag() {
        let app = crate::build_router(state_with(None, Some("Sim.Server")).await);

        let response = get(app, "/api/opc/read").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(body["error"].as_str().unwrap().contains("tag is required"));
    }

    #[tokio::test]
    async fn read_returns_400_when_no_server_is_configured() {
        let app = crate::build_router(state_with(None, None).await);

        let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("no OPC server specified")
        );
    }

    #[tokio::test]
    async fn read_returns_400_when_the_gateway_is_unreachable() {
        let app = crate::build_router(state_with(Some("127.0.0.1:1"), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn read_surfaces_uncertain_and_bad_quality_without_failing() {
        let host = start_mock_server(MockBridgeService {
            read_response: opcda_bridge_proto::bridge::ReadResponse {
                values: vec![opcda_bridge_proto::bridge::TagValue {
                    tag_id: "ignored".to_string(),
                    value: "0".to_string(),
                    quality: "Bad".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;
        let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

        let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_json(response).await;
        assert_eq!(body["quality"], "bad");
    }

    /// Direct unit test of the shared timeout wrapper (rather than driving a full HTTP
    /// handler through a real stalled gateway call) -- mirrors `bhtune-cli`'s own
    /// `bounded_driver_call_returns_timed_out_when_the_driver_call_stalls` test precedent:
    /// `start_paused = true` lets `tokio::time::timeout`'s deadline elapse in virtual rather
    /// than real time, so this proves the elapsed-deadline branch without an actual 30s wait.
    #[tokio::test(start_paused = true)]
    async fn with_timeout_maps_an_elapsed_deadline_to_a_bad_request() {
        let err = with_timeout("read 'X'", std::future::pending::<DriverResult<()>>())
            .await
            .unwrap_err();
        match err {
            ApiError::BadRequest(message) => {
                assert!(message.contains("read 'X'"));
                assert!(message.contains("no response within 30s"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn with_timeout_passes_through_a_successful_result() {
        let value = with_timeout("op", async { Ok::<_, bhtune_driver::DriverError>(7) })
            .await
            .unwrap();
        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn with_timeout_maps_a_driver_error_to_a_bad_request() {
        let err = with_timeout("read 'X'", async {
            Err::<(), _>(bhtune_driver::DriverError::Unsupported {
                operation: "browse",
            })
        })
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(message) => {
                assert!(message.contains("read 'X'"));
                assert!(message.contains("not supported"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn with_timeout_preserves_index_configuration_diagnostic() {
        let err = with_timeout("refresh the OPC namespace index", async {
            Err::<(), _>(bhtune_driver::DriverError::IndexOperationRejected {
                message: "server is not configured for namespace indexing".to_string(),
            })
        })
        .await
        .unwrap_err();
        match err {
            ApiError::BadRequest(message) => {
                assert!(message.contains("refresh the OPC namespace index"));
                assert!(message.contains("server is not configured for namespace indexing"));
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
