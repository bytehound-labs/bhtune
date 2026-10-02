//! OPC DA diagnostic routes: `GET /api/opc/servers`, `GET /api/opc/capabilities`,
//! `GET /api/opc/browse`, `DELETE /api/opc/browse/sessions/:id`, `GET /api/opc/search`,
//! `GET /api/opc/read`, and the indexed-search status/refresh/control routes -- back the GUI's
//! server dropdown, typed tag-tree browser, namespace search, index management, and connection
//! test.
//!
//! Read-only diagnostics remain independent of [`crate::state::AppState::active_run`], and every
//! OPC operation is bounded by an explicit `OPC_QUERY_TIMEOUT_SECS` timeout (see that constant's
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
use bhtune_driver::{
    BrowseNodeKind, BrowsePageRequest, BrowseSource, Driver, DriverResult, NamespaceOrganization,
    OpcDaDriver, OpcDaGatewayCompatibility, SearchEvent, SearchIndexControlAction,
    SearchIndexRequest, SearchMatch, SearchMatchMode, SearchRequest, check_gateway_compatibility,
    list_opcda_servers,
};
use bhtune_runtime::tune::sample_quality_from_driver;

use crate::error::{ApiError, ErrorBody};
use crate::state::AppState;

/// Bounds every OPC DA call this module makes. `opcda_bridge::Client::connect` has no
/// connect timeout of its own (plain `tonic`, no `connect_timeout` configured), so a
/// firewalled or black-holed gateway host would otherwise hang a request for however long
/// the OS's own TCP-connect timeout is -- potentially minutes. 30s matches the runtime's
/// built-in operation and restore timeout defaults in `config::tuning`, rather than
/// inventing a new number. Each connect/browse/read call
/// gets its own separate budget via [`with_timeout`], not one combined timeout spanning
/// connect *and* the operation together.
const OPC_QUERY_TIMEOUT_SECS: u64 = 30;

/// Runs `fut` (one OPC DA driver call) under an [`OPC_QUERY_TIMEOUT_SECS`] deadline, mapping
/// both a [`bhtune_driver::DriverError`] and an elapsed deadline to [`ApiError::BadRequest`]
/// -- every failure this module can produce is "the gateway/tag couldn't be reached in
/// time", a client-actionable diagnostic outcome, never an [`ApiError::Internal`] bug in this
/// server. `what` names the attempted operation (e.g. `"connect to OPC server 'X'"`) so the
/// error message identifies which step failed.
enum TimedDriverCall<T> {
    Ready(T),
    Incompatible { operation: &'static str },
    Failed(ApiError),
}

async fn timed_driver_call<T>(
    what: &str,
    timeout: Duration,
    fut: impl Future<Output = DriverResult<T>>,
) -> TimedDriverCall<T> {
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(value)) => TimedDriverCall::Ready(value),
        Ok(Err(bhtune_driver::DriverError::IncompatibleGateway { operation })) => {
            TimedDriverCall::Incompatible { operation }
        }
        Ok(Err(err)) => TimedDriverCall::Failed(ApiError::BadRequest(format!("{what}: {err}"))),
        Err(_) => TimedDriverCall::Failed(ApiError::BadRequest(format!(
            "{what}: no response within {}s",
            timeout.as_secs()
        ))),
    }
}

async fn with_timeout<T>(
    what: &str,
    fut: impl Future<Output = DriverResult<T>>,
) -> Result<T, ApiError> {
    match timed_driver_call(what, Duration::from_secs(OPC_QUERY_TIMEOUT_SECS), fut).await {
        TimedDriverCall::Ready(value) => Ok(value),
        TimedDriverCall::Incompatible { operation } => Err(ApiError::BadRequest(format!(
            "{what}: {}",
            bhtune_driver::DriverError::IncompatibleGateway { operation }
        ))),
        TimedDriverCall::Failed(error) => Err(error),
    }
}

mod dto;
mod handlers;
mod helpers;
mod validation;

pub use dto::*;
pub(crate) use handlers::{
    __path_browse, __path_capabilities, __path_close_browse_session, __path_control_search_index,
    __path_delete_search_index, __path_read, __path_refresh_search_index, __path_search,
    __path_search_index, __path_search_index_status, __path_servers,
    __path_set_search_index_auto_refresh,
};
pub(crate) use handlers::{
    browse, capabilities, close_browse_session, control_search_index, delete_search_index, read,
    refresh_search_index, search, search_index, search_index_status, servers,
    set_search_index_auto_refresh,
};

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
        .route(
            "/api/opc/search-index/auto-refresh",
            post(set_search_index_auto_refresh),
        )
        .route("/api/opc/search-index/control", post(control_search_index))
        .route("/api/opc/search-index", delete(delete_search_index))
        .route("/api/opc/read", get(read))
}
