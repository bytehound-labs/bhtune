use bhtune_db::models::SampleQuality;
use bhtune_driver::{
    BrowseNode, BrowseNodeKind, BrowsePage, DriverCapabilities, IndexedSearchMatch,
    IndexedSearchProgress, OpcDaFeatureCompatibility, OpcDaGatewayCompatibility,
    SearchIndexResponse, SearchIndexStatus,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::helpers::{organization_name, source_name};
use super::validation::{
    default_index_search_max_results, default_page_size, default_search_match_mode,
    default_search_max_results,
};

/// Inclusive protocol range stored with a gateway compatibility snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct GatewayProtocolRangeResponse {
    pub min: u32,
    pub max: u32,
}

/// One protocol feature in a gateway compatibility snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct GatewayFeatureCompatibilityResponse {
    pub feature: String,
    pub status: String,
    pub client_versions: GatewayProtocolRangeResponse,
    pub gateway_versions: Option<GatewayProtocolRangeResponse>,
    pub negotiated_version: Option<u32>,
    pub reason: String,
}

impl From<&OpcDaFeatureCompatibility> for GatewayFeatureCompatibilityResponse {
    fn from(feature: &OpcDaFeatureCompatibility) -> Self {
        Self {
            feature: feature.feature.as_str().to_string(),
            status: feature.status.as_str().to_string(),
            client_versions: GatewayProtocolRangeResponse {
                min: feature.client_versions.min,
                max: feature.client_versions.max,
            },
            gateway_versions: feature.gateway_versions.as_ref().map(|range| {
                GatewayProtocolRangeResponse {
                    min: range.min,
                    max: range.max,
                }
            }),
            negotiated_version: feature.negotiated_version,
            reason: feature.reason.clone(),
        }
    }
}

/// Gateway protocol compatibility attached to inspection responses and run detail.
///
/// String fields match the snake_case snapshot stored in
/// `tune_runs.gateway_compatibility_json`. Discovery, browse, and read attach this even when
/// the gateway is only partially compatible or predates a requested operation; live mutations
/// refuse an incompatible core instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct GatewayCompatibilityResponse {
    pub client_version: String,
    pub gateway_version: Option<String>,
    pub source: String,
    pub status: String,
    pub features: Vec<GatewayFeatureCompatibilityResponse>,
}

impl From<&OpcDaGatewayCompatibility> for GatewayCompatibilityResponse {
    fn from(report: &OpcDaGatewayCompatibility) -> Self {
        Self {
            client_version: report.client_version.clone(),
            gateway_version: report.gateway_version.clone(),
            source: report.source.as_str().to_string(),
            status: report.status.as_str().to_string(),
            features: report
                .features
                .iter()
                .map(GatewayFeatureCompatibilityResponse::from)
                .collect(),
        }
    }
}

impl From<OpcDaGatewayCompatibility> for GatewayCompatibilityResponse {
    fn from(report: OpcDaGatewayCompatibility) -> Self {
        Self::from(&report)
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
    /// Compatibility of the gateway that answered this discovery call. Discovery itself is
    /// not refused when the gateway is partial, unknown, or incompatible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_compatibility: Option<GatewayCompatibilityResponse>,
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
    /// Compatibility observed while discovering capabilities. Present even when capability
    /// discovery itself is unsupported and the rest of this response is degraded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_compatibility: Option<GatewayCompatibilityResponse>,
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
            gateway_compatibility: None,
        }
    }
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
    pub auto_refresh_enabled: bool,
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
    pub scheduler: OpcIndexSchedulerResponse,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcIndexSchedulerResponse {
    pub next_refresh_at: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_success_duration_ms: Option<u64>,
    pub retry_after: Option<String>,
    pub consecutive_failures: u32,
    pub circuit_open: bool,
}

impl From<SearchIndexStatus> for OpcSearchIndexStatusResponse {
    fn from(status: SearchIndexStatus) -> Self {
        Self {
            server: status.server,
            state: status.state.to_string(),
            auto_refresh_enabled: status.auto_refresh_enabled,
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
            scheduler: OpcIndexSchedulerResponse {
                next_refresh_at: status.scheduler.next_refresh_at,
                last_attempt_at: status.scheduler.last_attempt_at,
                last_success_at: status.scheduler.last_success_at,
                last_success_duration_ms: status.scheduler.last_success_duration_ms,
                retry_after: status.scheduler.retry_after,
                consecutive_failures: status.scheduler.consecutive_failures,
                circuit_open: status.scheduler.circuit_open,
            },
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

/// Query parameters for `POST /api/opc/search-index/refresh`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexRefreshQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub force: Option<bool>,
}

/// Query parameters for `POST /api/opc/search-index/auto-refresh`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexAutoRefreshQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub enabled: bool,
}

/// Query parameters for `POST /api/opc/search-index/control`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct OpcSearchIndexControlQuery {
    pub bridge_host: Option<String>,
    pub opc_server: Option<String>,
    pub action: String,
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
    /// Compatibility observed for this browse. An unsupported page still returns this field
    /// with an empty, explicitly degraded page instead of failing the request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_compatibility: Option<GatewayCompatibilityResponse>,
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
            gateway_compatibility: None,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OpcCloseBrowseSessionResponse {
    pub closed: bool,
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
    /// Compatibility of the gateway that served this diagnostic read. A partial or unknown
    /// gateway still returns the tag value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_compatibility: Option<GatewayCompatibilityResponse>,
}
