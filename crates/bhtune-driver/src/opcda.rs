//! `OpcDaDriver`: the primary [`Driver`] implementation, talking to a DCS/PLC's OPC DA
//! server through the `opcda-bridge` gateway's gRPC API.
//!
//! This module intentionally splits into two halves: a thin async shell (`OpcDaDriver`
//! itself) that only locks the client and calls into `opcda_bridge`, and small mapping
//! functions that translate the bridge's typed pages, nodes, capabilities, and search events
//! into the driver crate's protocol-neutral types. The mapping functions carry the real risk
//! of a subtle bug and are unit-testable with no I/O; the shell is exercised end-to-end by
//! mock-gateway smoke tests rather than re-testing the bridge's own gRPC matrix.

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    driver::Driver,
    error::{DriverError, DriverResult},
    types::{
        BrowseBreadcrumb, BrowseNode, BrowseNodeKind, BrowsePage, BrowsePageRequest, BrowseSource,
        DriverCapabilities, IndexSchedulerDiagnostics, IndexedSearchMatch, IndexedSearchProgress,
        NamespaceOrganization, Quality, SearchCompleted, SearchEvent, SearchIndexControlAction,
        SearchIndexRequest, SearchIndexResponse, SearchIndexState, SearchIndexStatus, SearchMatch,
        SearchMatchMode, SearchProgress, SearchRequest, TagId, TagValue, TagWrite, WriteOutcome,
    },
};

/// Default number of children requested for one browse page. The bridge enforces its own
/// maximum; this value keeps the browser responsive and matches the bridge library default.
pub const DEFAULT_PAGE_SIZE: u32 = opcda_bridge::DEFAULT_PAGE_SIZE;

/// Default maximum number of search matches requested by the CLI and browser.
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = opcda_bridge::DEFAULT_SEARCH_MAX_RESULTS;

/// Default maximum number of matches requested from the persistent namespace index.
pub const DEFAULT_INDEX_SEARCH_MAX_RESULTS: u32 = opcda_bridge::DEFAULT_INDEX_SEARCH_MAX_RESULTS;

/// A gateway-wide protocol feature reported by `opcda-bridge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpcDaGatewayFeature {
    Core,
    Namespace,
    IndexedSearch,
}

impl OpcDaGatewayFeature {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Namespace => "namespace",
            Self::IndexedSearch => "indexed_search",
        }
    }
}

/// One gateway-wide protocol feature and the inclusive versions it supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcDaGatewayFeatureSupport {
    pub feature: OpcDaGatewayFeature,
    pub min_version: u32,
    pub max_version: u32,
}

/// Gateway-wide metadata available without contacting an OPC DA server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpcDaGatewayInfo {
    pub application_version: String,
    pub compatibility_schema_version: u32,
    pub features: Vec<OpcDaGatewayFeatureSupport>,
}

/// An inclusive protocol-version range supported by bhtune or reported by a gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpcDaProtocolRange {
    /// Lowest supported protocol version.
    pub min: u32,
    /// Highest supported protocol version.
    pub max: u32,
}

impl std::fmt::Display for OpcDaProtocolRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.min, self.max)
    }
}

/// Overall protocol compatibility between this bhtune build and an `opcda-bridge` gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpcDaCompatibilityStatus {
    /// Every protocol feature bhtune uses overlaps with the gateway.
    Full,
    /// The core protocol overlaps, but at least one optional feature is degraded.
    Partial,
    /// The core protocol does not overlap; live operations must be refused.
    Incompatible,
    /// The gateway did not report enough metadata to verify compatibility.
    Unknown,
}

impl OpcDaCompatibilityStatus {
    /// Stable `snake_case` identifier used in logs and stored run provenance.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Partial => "partial",
            Self::Incompatible => "incompatible",
            Self::Unknown => "unknown",
        }
    }
}

/// Where the gateway protocol metadata behind a compatibility result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpcDaCompatibilitySource {
    /// The gateway-wide version and protocol metadata RPC.
    GatewayInfo,
    /// Per-server capability metadata from a gateway that predates gateway-wide metadata.
    LegacyCapabilities,
    /// The gateway reported no usable protocol metadata.
    Unknown,
}

impl OpcDaCompatibilitySource {
    /// Stable `snake_case` identifier used in logs and stored run provenance.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GatewayInfo => "gateway_info",
            Self::LegacyCapabilities => "legacy_capabilities",
            Self::Unknown => "unknown",
        }
    }
}

/// Compatibility of one gateway protocol feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpcDaFeatureCompatibilityStatus {
    /// bhtune and the gateway share at least one protocol version for this feature.
    Compatible,
    /// The gateway does not offer this optional feature.
    Unsupported,
    /// The gateway offers this feature, but no protocol version overlaps.
    Incompatible,
    /// The gateway did not report this feature.
    Unknown,
}

impl OpcDaFeatureCompatibilityStatus {
    /// Stable `snake_case` identifier used in logs and stored run provenance.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "compatible",
            Self::Unsupported => "unsupported",
            Self::Incompatible => "incompatible",
            Self::Unknown => "unknown",
        }
    }
}

/// The compatibility evaluation of one gateway protocol feature.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpcDaFeatureCompatibility {
    /// The protocol feature evaluated.
    pub feature: OpcDaGatewayFeature,
    /// Whether this bhtune build can use the feature with the gateway.
    pub status: OpcDaFeatureCompatibilityStatus,
    /// The protocol versions this bhtune build supports for the feature.
    pub client_versions: OpcDaProtocolRange,
    /// The protocol versions the gateway reported, if any.
    pub gateway_versions: Option<OpcDaProtocolRange>,
    /// The highest protocol version both sides support, if any.
    pub negotiated_version: Option<u32>,
    /// A human-readable explanation from the compatibility evaluation.
    pub reason: String,
}

impl OpcDaFeatureCompatibility {
    /// Describes the feature's status and both protocol ranges in one line.
    #[must_use]
    pub fn describe(&self) -> String {
        let gateway = self
            .gateway_versions
            .map_or_else(|| "none".to_string(), |range| range.to_string());
        format!(
            "{} protocol {}: bhtune supports {}, gateway reports {gateway}",
            self.feature.as_str(),
            self.status.as_str(),
            self.client_versions,
        )
    }
}

/// The result of checking whether an `opcda-bridge` gateway can safely serve this bhtune
/// build.
///
/// Live mutations must be refused when [`Self::is_incompatible`] is true. `Partial` and
/// `Unknown` results allow live operations but should surface [`Self::warning_message`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpcDaGatewayCompatibility {
    /// The bhtune version whose protocol profile was evaluated.
    pub client_version: String,
    /// The gateway application version, when the gateway reported one.
    pub gateway_version: Option<String>,
    /// Where the gateway protocol metadata came from.
    pub source: OpcDaCompatibilitySource,
    /// The overall compatibility result.
    pub status: OpcDaCompatibilityStatus,
    /// The per-feature evaluations behind [`Self::status`].
    pub features: Vec<OpcDaFeatureCompatibility>,
}

impl OpcDaGatewayCompatibility {
    /// Returns true when live operations against this gateway must be refused.
    #[must_use]
    pub const fn is_incompatible(&self) -> bool {
        matches!(self.status, OpcDaCompatibilityStatus::Incompatible)
    }

    /// The refusal text for an incompatible gateway, or `None` when live operations may proceed.
    #[must_use]
    pub fn live_mutation_refusal(&self) -> Option<String> {
        self.is_incompatible()
            .then(|| self.incompatibility_message())
    }

    /// A synthetic `Unknown` result used when inspection cannot classify the gateway.
    #[must_use]
    pub fn unverified() -> Self {
        gateway_compatibility_from_report(opcda_bridge::unknown_compatibility_report(
            CLIENT_VERSION,
        ))
    }

    fn gateway_label(&self) -> String {
        self.gateway_version.as_deref().map_or_else(
            || "opcda-bridge gateway (version not reported)".to_string(),
            |version| format!("opcda-bridge gateway {version}"),
        )
    }

    fn details(&self) -> String {
        let issues: Vec<String> = self
            .features
            .iter()
            .filter(|feature| feature.status != OpcDaFeatureCompatibilityStatus::Compatible)
            .map(OpcDaFeatureCompatibility::describe)
            .collect();
        if issues.is_empty() {
            String::new()
        } else {
            format!(" ({})", issues.join("; "))
        }
    }

    /// An actionable refusal message naming the gateway version, both protocol ranges for
    /// every affected feature, and the fix.
    #[must_use]
    pub fn incompatibility_message(&self) -> String {
        format!(
            "{} is incompatible with bhtune {}{}; upgrade the opcda-bridge gateway or bhtune so \
             their core protocol versions overlap",
            self.gateway_label(),
            self.client_version,
            self.details(),
        )
    }

    /// An operator warning for a `Partial` or `Unknown` result, or `None` when the result
    /// needs no warning (`Full`) or must be refused instead (`Incompatible`).
    #[must_use]
    pub fn warning_message(&self) -> Option<String> {
        match self.status {
            OpcDaCompatibilityStatus::Full | OpcDaCompatibilityStatus::Incompatible => None,
            OpcDaCompatibilityStatus::Partial => Some(format!(
                "{} is partially compatible with bhtune {}; affected optional features are \
                 degraded{}",
                self.gateway_label(),
                self.client_version,
                self.details(),
            )),
            OpcDaCompatibilityStatus::Unknown => Some(format!(
                "{} did not report enough protocol metadata to verify compatibility with bhtune \
                 {}; proceeding without verification{}",
                self.gateway_label(),
                self.client_version,
                self.details(),
            )),
        }
    }
}

const fn gateway_feature_from_bridge(
    feature: opcda_bridge::CompatibilityFeature,
) -> OpcDaGatewayFeature {
    match feature {
        opcda_bridge::CompatibilityFeature::Core => OpcDaGatewayFeature::Core,
        opcda_bridge::CompatibilityFeature::Namespace => OpcDaGatewayFeature::Namespace,
        opcda_bridge::CompatibilityFeature::IndexedSearch => OpcDaGatewayFeature::IndexedSearch,
    }
}

const fn compatibility_status_from_bridge(
    status: opcda_bridge::CompatibilityStatus,
) -> OpcDaCompatibilityStatus {
    match status {
        opcda_bridge::CompatibilityStatus::Full => OpcDaCompatibilityStatus::Full,
        opcda_bridge::CompatibilityStatus::Partial => OpcDaCompatibilityStatus::Partial,
        opcda_bridge::CompatibilityStatus::Incompatible => OpcDaCompatibilityStatus::Incompatible,
        opcda_bridge::CompatibilityStatus::Unknown => OpcDaCompatibilityStatus::Unknown,
    }
}

const fn compatibility_source_from_bridge(
    source: opcda_bridge::CompatibilitySource,
) -> OpcDaCompatibilitySource {
    match source {
        opcda_bridge::CompatibilitySource::GatewayInfo => OpcDaCompatibilitySource::GatewayInfo,
        opcda_bridge::CompatibilitySource::LegacyCapabilities => {
            OpcDaCompatibilitySource::LegacyCapabilities
        }
        opcda_bridge::CompatibilitySource::Unknown => OpcDaCompatibilitySource::Unknown,
    }
}

const fn feature_status_from_bridge(
    status: opcda_bridge::FeatureCompatibilityStatus,
) -> OpcDaFeatureCompatibilityStatus {
    match status {
        opcda_bridge::FeatureCompatibilityStatus::Compatible => {
            OpcDaFeatureCompatibilityStatus::Compatible
        }
        opcda_bridge::FeatureCompatibilityStatus::Unsupported => {
            OpcDaFeatureCompatibilityStatus::Unsupported
        }
        opcda_bridge::FeatureCompatibilityStatus::Incompatible => {
            OpcDaFeatureCompatibilityStatus::Incompatible
        }
        opcda_bridge::FeatureCompatibilityStatus::Unknown => {
            OpcDaFeatureCompatibilityStatus::Unknown
        }
    }
}

const fn protocol_range_from_bridge(
    range: opcda_bridge::ProtocolVersionRange,
) -> OpcDaProtocolRange {
    OpcDaProtocolRange {
        min: range.min,
        max: range.max,
    }
}

fn gateway_compatibility_from_report(
    report: opcda_bridge::CompatibilityReport,
) -> OpcDaGatewayCompatibility {
    OpcDaGatewayCompatibility {
        client_version: report.client_version,
        gateway_version: report
            .gateway_version
            .filter(|version| !version.trim().is_empty()),
        source: compatibility_source_from_bridge(report.source),
        status: compatibility_status_from_bridge(report.status),
        features: report
            .features
            .into_iter()
            .map(|feature| OpcDaFeatureCompatibility {
                feature: gateway_feature_from_bridge(feature.feature),
                status: feature_status_from_bridge(feature.status),
                client_versions: protocol_range_from_bridge(feature.client_versions),
                gateway_versions: feature.gateway_versions.map(protocol_range_from_bridge),
                negotiated_version: feature.negotiated_version,
                reason: feature.reason,
            })
            .collect(),
    }
}

/// A cancellable stream of typed namespace-search events.
#[derive(Debug)]
pub struct DriverSearchStream {
    inner: opcda_bridge::SearchStream,
}

impl DriverSearchStream {
    /// Waits for the next event. Dropping the stream cancels the gateway-side search.
    pub async fn next(&mut self) -> DriverResult<Option<SearchEvent>> {
        let event = self
            .inner
            .message()
            .await
            .map_err(|err| map_bridge_error_for(err, "namespace search"))?;
        event.map(search_event_from_bridge).transpose()
    }
}

/// The primary [`Driver`] for v1: reads, writes, and browses OPC DA tags through an
/// `opcda-bridge` gateway's gRPC API.
///
/// Holds its `opcda_bridge::Client` behind a `tokio::sync::Mutex` because the client's own
/// methods take `&mut self` (it buffers per-call gRPC codec state) while [`Driver`]'s
/// methods take `&self` (required so a driver can be shared behind `Arc<dyn Driver>`) —
/// serializing calls through the one connection is a reasonable tradeoff, since a single
/// tuning session only ever has one read/write/browse in flight at a time, and the
/// underlying HTTP/2 channel stays cheaply multiplexed regardless of how many logical
/// callers there are.
#[derive(Debug)]
pub struct OpcDaDriver {
    client: Mutex<opcda_bridge::Client>,
    server: String,
}

impl OpcDaDriver {
    /// Connects to an `opcda-bridge` gateway at `host` (e.g. `"localhost:7600"`) and binds
    /// to `server` — the OPC DA server's ProgID (e.g. `"Matrikon.OPC.Simulation.1"`) that
    /// every subsequent `read`/`write`/`browse` call is scoped to.
    pub async fn connect(host: &str, server: impl Into<String>) -> DriverResult<Self> {
        let client = opcda_bridge::Client::connect(host)
            .await
            .map_err(|err| map_bridge_error_for(err, "connect to OPC DA bridge"))?;
        Ok(Self {
            client: Mutex::new(client),
            server: server.into(),
        })
    }

    /// Reports the bridge and OPC server's browse/search capabilities.
    pub async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
        let mut client = self.client.lock().await;
        client
            .capabilities(self.server.clone())
            .await
            .map_err(|err| map_bridge_error_for(err, "capability discovery"))
            .and_then(capabilities_from_bridge)
    }

    /// Requests one bounded browse page without following its continuation token.
    pub async fn browse_page(&self, request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        let mut client = self.client.lock().await;
        client
            .browse_page(opcda_bridge::BrowsePageRequest {
                server: self.server.clone(),
                session_id: request.session_id,
                parent_node_key: request.parent_node_key,
                page_token: request.page_token,
                page_size: request.page_size,
                refresh: request.refresh,
            })
            .await
            .map_err(|err| map_bridge_error_for(err, "paged browse"))
            .and_then(browse_page_from_bridge)
    }

    /// Starts a progressive namespace search. Dropping the returned stream cancels the search.
    pub async fn search_stream(&self, request: SearchRequest) -> DriverResult<DriverSearchStream> {
        let mut client = self.client.lock().await;
        let bridge_request = opcda_bridge::SearchRequest {
            server: self.server.clone(),
            query: request.query,
            match_mode: match_search_mode(request.match_mode),
            session_id: request.session_id,
            scope_node_key: request.scope_node_key,
            max_results: request.max_results,
            include_branches: request.include_branches,
            refresh: request.refresh,
        };
        let inner = client
            .search_stream(bridge_request)
            .await
            .map_err(|err| map_bridge_error_for(err, "namespace search"))?;
        Ok(DriverSearchStream { inner })
    }

    /// Collects a complete search stream for callers that do not need progressive delivery.
    pub async fn search_events(&self, request: SearchRequest) -> DriverResult<Vec<SearchEvent>> {
        let mut stream = self.search_stream(request).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await? {
            events.push(event);
        }
        Ok(events)
    }

    /// Returns the gateway-owned persistent namespace-index status for this OPC server.
    pub async fn search_index_status(&self) -> DriverResult<SearchIndexStatus> {
        let mut client = self.client.lock().await;
        client
            .search_index_status(self.server.clone())
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed-search status"))
            .map(search_index_status_from_bridge)
    }

    /// Enables or disables future automatic refreshes for this OPC server's index.
    pub async fn set_search_index_auto_refresh(
        &self,
        enabled: bool,
    ) -> DriverResult<SearchIndexStatus> {
        let mut client = self.client.lock().await;
        client
            .set_search_index_auto_refresh(self.server.clone(), enabled)
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed-search auto-refresh"))
            .map(search_index_status_from_bridge)
    }

    /// Deletes this OPC server's persistent namespace index and enrollment.
    pub async fn delete_search_index(&self) -> DriverResult<SearchIndexStatus> {
        let mut client = self.client.lock().await;
        client
            .delete_search_index(self.server.clone())
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed-search delete"))
            .map(search_index_status_from_bridge)
    }

    /// Starts or coalesces a persistent namespace-index refresh for this OPC server.
    pub async fn refresh_search_index(&self, force: bool) -> DriverResult<SearchIndexStatus> {
        let mut client = self.client.lock().await;
        client
            .refresh_search_index(self.server.clone(), force)
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed-search refresh"))
            .map(search_index_status_from_bridge)
    }

    /// Pauses, resumes, or cancels a persistent namespace-index build.
    pub async fn control_search_index(
        &self,
        action: SearchIndexControlAction,
    ) -> DriverResult<SearchIndexStatus> {
        let mut client = self.client.lock().await;
        client
            .control_search_index(self.server.clone(), action.into())
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed-search control"))
            .map(search_index_status_from_bridge)
    }

    /// Queries the gateway-owned persistent namespace index without falling back to live
    /// namespace traversal.
    pub async fn search_index_query(
        &self,
        request: SearchIndexRequest,
    ) -> DriverResult<SearchIndexResponse> {
        let mut client = self.client.lock().await;
        client
            .search_index(opcda_bridge::SearchIndexRequest {
                server: self.server.clone(),
                query: request.query,
                match_mode: match_search_mode(request.match_mode),
                max_results: request.max_results,
            })
            .await
            .map_err(|err| map_bridge_error_for(err, "indexed search"))
            .map(search_index_response_from_bridge)
    }
}

/// Queries gateway-wide version and protocol metadata without contacting an OPC DA server.
///
/// This is the appropriate connectivity probe for a newly installed gateway because it works
/// before an OPC DA server or OPCEnum is installed on the gateway host.
pub async fn get_opcda_gateway_info(bridge_host: &str) -> DriverResult<OpcDaGatewayInfo> {
    let mut client = opcda_bridge::Client::connect(bridge_host)
        .await
        .map_err(|err| map_bridge_error_for(err, "connect to OPC DA bridge"))?;
    let info = client
        .gateway_info()
        .await
        .map_err(|err| map_bridge_error_for(err, "gateway information"))?;
    Ok(OpcDaGatewayInfo {
        application_version: info.application_version,
        compatibility_schema_version: info.compatibility_schema_version,
        features: info
            .features
            .into_iter()
            .map(|support| OpcDaGatewayFeatureSupport {
                feature: gateway_feature_from_bridge(support.feature),
                min_version: support.versions.min,
                max_version: support.versions.max,
            })
            .collect(),
    })
}

/// The bhtune version whose protocol profile is evaluated against a gateway. Every workspace
/// crate inherits the same version, so this is bhtune's own release version.
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Builds an `Incompatible` report when the gateway rejects a metadata RPC outright.
///
/// The published facade reports that rejection as an error rather than a compatibility
/// status. Inspection still needs a structured result, and live mutations need a refusal, so
/// the core feature is marked incompatible and the rejected operation is named in each
/// feature reason.
fn incompatible_gateway_report(operation: &str) -> OpcDaGatewayCompatibility {
    let profile = opcda_bridge::current_client_profile(CLIENT_VERSION);
    let reason = format!("gateway rejected {operation} as incompatible with this client");
    let features = profile
        .features
        .into_iter()
        .map(|support| {
            let feature = gateway_feature_from_bridge(support.feature);
            let status = if matches!(feature, OpcDaGatewayFeature::Core) {
                OpcDaFeatureCompatibilityStatus::Incompatible
            } else {
                OpcDaFeatureCompatibilityStatus::Unknown
            };
            OpcDaFeatureCompatibility {
                feature,
                status,
                client_versions: OpcDaProtocolRange {
                    min: support.versions.min,
                    max: support.versions.max,
                },
                gateway_versions: None,
                negotiated_version: None,
                reason: reason.clone(),
            }
        })
        .collect();
    OpcDaGatewayCompatibility {
        client_version: CLIENT_VERSION.to_string(),
        gateway_version: None,
        source: OpcDaCompatibilitySource::Unknown,
        status: OpcDaCompatibilityStatus::Incompatible,
        features,
    }
}

/// Checks whether the `opcda-bridge` gateway at `bridge_host` speaks a protocol compatible
/// with this bhtune build, without reading or writing any OPC DA tag.
///
/// Uses the gateway-wide metadata RPC first. A gateway that predates that RPC is evaluated
/// from the named `server`'s legacy capability metadata when a server is supplied. When that
/// fallback is also rejected as incompatible, the result is an `Incompatible` report rather
/// than an error, so inspection can show the refusal and live mutations can refuse it. A
/// gateway that cannot be classified, and does not reject the client, is reported as
/// `Unknown`.
///
/// # Errors
///
/// Returns [`DriverError::Connect`] when the gateway cannot be reached, and
/// [`DriverError::Operation`] when a metadata RPC fails for a reason other than an
/// incompatible-gateway rejection.
pub async fn check_gateway_compatibility(
    bridge_host: &str,
    server: Option<&str>,
) -> DriverResult<OpcDaGatewayCompatibility> {
    let mut client = opcda_bridge::Client::connect(bridge_host)
        .await
        .map_err(|err| map_bridge_error_for(err, "connect to OPC DA bridge"))?;
    match client
        .compatibility_with_client_version(server, CLIENT_VERSION)
        .await
    {
        Ok(report) => Ok(gateway_compatibility_from_report(report)),
        Err(opcda_bridge::Error::IncompatibleGateway { operation }) => {
            Ok(incompatible_gateway_report(operation))
        }
        Err(err) => Err(map_bridge_error_for(err, "gateway compatibility")),
    }
}

/// Lists the OPC DA servers registered on the `opcda-bridge` gateway's own host at
/// `bridge_host` (e.g. `"localhost:7600"`).
///
/// A standalone free function rather than a [`Driver`] method or an [`OpcDaDriver`]
/// associated function: server discovery is a *pre-connection* operation — it needs only a
/// bridge host, not the OPC DA server ProgID that [`OpcDaDriver::connect`] requires and that
/// discovery exists to help a caller find in the first place. Connects for the one call and
/// drops the connection immediately afterward; unlike `OpcDaDriver`, there is no ongoing
/// session to hold open here.
///
/// Note: `opcda_bridge::Client::list_servers` always sends `host: "localhost"` in its
/// request — i.e. it lists servers registered on *the gateway's own* machine, not on
/// whatever machine bhtune itself happens to run on. That is exactly right for this
/// topology (the gateway runs next to the OPC DA server; bhtune runs wherever the
/// engineer's browser or scheduler is) and is called out here so it is never mistaken for a
/// bug.
pub async fn list_opcda_servers(bridge_host: &str) -> DriverResult<Vec<String>> {
    let mut client = opcda_bridge::Client::connect(bridge_host)
        .await
        .map_err(|err| map_bridge_error_for(err, "connect to OPC DA bridge"))?;
    client
        .list_servers()
        .await
        .map_err(|err| map_bridge_error_for(err, "list OPC DA servers"))
}

/// Explicitly releases one gateway-side browse session without requiring an OPC server
/// ProgID. The session ID is the only value the bridge close RPC needs, so this is separate
/// from [`OpcDaDriver`] for callers such as the CLI's `opc close` command that may no longer
/// have the server name handy.
pub async fn close_opcda_browse_session(bridge_host: &str, session_id: &str) -> DriverResult<()> {
    let mut client = opcda_bridge::Client::connect(bridge_host)
        .await
        .map_err(|err| map_bridge_error_for(err, "connect to OPC DA bridge"))?;
    client
        .close_browse_session(session_id)
        .await
        .map_err(|err| map_bridge_error_for(err, "browse-session close"))
}

#[async_trait]
impl Driver for OpcDaDriver {
    async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>> {
        let mut client = self.client.lock().await;
        let raw = client
            .read(self.server.clone(), tags.to_vec())
            .await
            .map_err(|err| map_bridge_error_for(err, "read OPC DA tags"))?;
        Ok(raw.into_iter().map(tag_value_from_raw).collect())
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome> {
        let mut client = self.client.lock().await;
        let result = client
            .write(
                self.server.clone(),
                tag.clone(),
                opc_value_from_write(value),
            )
            .await
            .map_err(|err| map_bridge_error_for(err, "write OPC DA tag"))?;
        Ok(write_outcome_from_result(result))
    }

    async fn capabilities(&self) -> DriverResult<DriverCapabilities> {
        self.capabilities().await
    }

    async fn browse(&self, request: BrowsePageRequest) -> DriverResult<BrowsePage> {
        self.browse_page(request).await
    }

    async fn close_browse_session(&self, session_id: &str) -> DriverResult<()> {
        let mut client = self.client.lock().await;
        client
            .close_browse_session(session_id)
            .await
            .map_err(|err| map_bridge_error_for(err, "browse-session close"))
    }

    async fn search(&self, request: SearchRequest) -> DriverResult<Vec<SearchEvent>> {
        self.search_events(request).await
    }

    async fn search_index_status(&self) -> DriverResult<SearchIndexStatus> {
        self.search_index_status().await
    }

    async fn refresh_search_index(&self, force: bool) -> DriverResult<SearchIndexStatus> {
        self.refresh_search_index(force).await
    }

    async fn control_search_index(
        &self,
        action: SearchIndexControlAction,
    ) -> DriverResult<SearchIndexStatus> {
        self.control_search_index(action).await
    }

    async fn set_search_index_auto_refresh(
        &self,
        enabled: bool,
    ) -> DriverResult<SearchIndexStatus> {
        self.set_search_index_auto_refresh(enabled).await
    }

    async fn delete_search_index(&self) -> DriverResult<SearchIndexStatus> {
        self.delete_search_index().await
    }

    async fn search_index(&self, request: SearchIndexRequest) -> DriverResult<SearchIndexResponse> {
        self.search_index_query(request).await
    }
}

/// Maps `opcda_bridge`'s raw OPC quality string to [`Quality`].
///
/// The gateway reports one of `"Good"`, `"Bad"`, `"Uncertain"`, or a synthesized
/// `"Unknown(0xNNNN)"` for an OPC quality code it doesn't otherwise recognize. Any string
/// other than an exact `"Good"`/`"Uncertain"` match — including that
/// `"Unknown(...)"` case — is treated as [`Quality::Bad`]: an unrecognized quality is
/// exactly the situation where guessing "trustworthy" would be the wrong default.
pub fn quality_from_raw(raw: &str) -> Quality {
    match raw {
        "Good" => Quality::Good,
        "Uncertain" => Quality::Uncertain,
        _ => Quality::Bad,
    }
}

/// Maps one `opcda_bridge::TagValue` (a single tag's raw read result) to this crate's
/// [`TagValue`].
pub fn tag_value_from_raw(raw: opcda_bridge::TagValue) -> TagValue {
    TagValue {
        tag: raw.tag_id,
        value: raw.value,
        quality: quality_from_raw(&raw.quality),
        // The gateway reports each tag's last-change time as a *local*, offset-less
        // "YYYY-MM-DD HH:MM:SS" string (or a "N/A"/"Invalid" sentinel for tags that have
        // none). There is no reliable way to convert that into a trustworthy `DateTime<Utc>`
        // without knowing the gateway
        // host's timezone, which isn't part of the bridge protocol and can't safely be
        // assumed to match wherever `bhtune` itself runs — so this is always `None` rather
        // than a guess. Purely diagnostic regardless (see `TagValue::timestamp`'s doc
        // comment): never the tick time the tuning engine itself runs on.
        timestamp: None,
    }
}

/// Maps a [`TagWrite`] to the `opcda_bridge::Value` its `Client::write` expects.
pub fn opc_value_from_write(write: TagWrite) -> opcda_bridge::Value {
    match write {
        TagWrite::Float(f) => opcda_bridge::Value::Float(f64::from(f)),
        TagWrite::Raw(s) => opcda_bridge::Value::String(s),
    }
}

/// Maps an `opcda_bridge::WriteResult` (the RPC-level outcome of one write) to
/// [`WriteOutcome`].
pub fn write_outcome_from_result(result: opcda_bridge::WriteResult) -> WriteOutcome {
    if result.success {
        WriteOutcome::success()
    } else {
        WriteOutcome::failure(
            result
                .error
                .unwrap_or_else(|| "gateway rejected the write".to_string()),
        )
    }
}

/// Maps the bridge's capabilities into the protocol-neutral driver model.
pub fn capabilities_from_bridge(
    capabilities: opcda_bridge::Capabilities,
) -> DriverResult<DriverCapabilities> {
    Ok(DriverCapabilities {
        application_version: capabilities.application_version,
        protocol_version: capabilities.protocol_version,
        max_page_size: capabilities.max_page_size,
        supports_browse_sessions: capabilities.supports_browse_sessions,
        supports_search: capabilities.supports_search,
        organization: namespace_organization_from_bridge(capabilities.organization),
        source: browse_source_from_bridge(capabilities.source),
        supports_indexed_search: capabilities.supports_indexed_search,
        indexed_search_protocol_version: capabilities.indexed_search_protocol_version,
        max_indexed_search_results: capabilities.max_indexed_search_results,
        search_index_state: search_index_state_from_bridge(capabilities.search_index_state),
    })
}

pub fn search_index_state_from_bridge(state: opcda_bridge::SearchIndexState) -> SearchIndexState {
    match state {
        opcda_bridge::SearchIndexState::Unspecified => SearchIndexState::Unspecified,
        opcda_bridge::SearchIndexState::NotIndexed => SearchIndexState::NotIndexed,
        opcda_bridge::SearchIndexState::Partial => SearchIndexState::Partial,
        opcda_bridge::SearchIndexState::Ready => SearchIndexState::Ready,
        opcda_bridge::SearchIndexState::Stale => SearchIndexState::Stale,
        opcda_bridge::SearchIndexState::Refreshing => SearchIndexState::Refreshing,
        opcda_bridge::SearchIndexState::Promoting => SearchIndexState::Promoting,
        opcda_bridge::SearchIndexState::Failed => SearchIndexState::Failed,
        opcda_bridge::SearchIndexState::Deleting => SearchIndexState::Deleting,
    }
}

pub fn indexed_search_progress_from_bridge(
    progress: opcda_bridge::IndexedSearchProgress,
) -> IndexedSearchProgress {
    IndexedSearchProgress {
        branches_visited: progress.branches_visited,
        entries_seen: progress.entries_seen,
        unique_items: progress.unique_items,
        active_time_ms: progress.active_time_ms,
        paused_time_ms: progress.paused_time_ms,
        items_per_second: progress.items_per_second,
        estimated_remaining_ms: progress.estimated_remaining_ms,
    }
}

pub fn search_index_status_from_bridge(
    status: opcda_bridge::SearchIndexStatus,
) -> SearchIndexStatus {
    SearchIndexStatus {
        server: status.server,
        state: search_index_state_from_bridge(status.state),
        auto_refresh_enabled: status.auto_refresh_enabled,
        active_generation: status.active_generation,
        entry_count: status.entry_count,
        unique_item_count: status.unique_item_count,
        started_at: status.started_at,
        completed_at: status.completed_at,
        last_error: status.last_error,
        database_bytes: status.database_bytes,
        organization: namespace_organization_from_bridge(status.organization),
        source: browse_source_from_bridge(status.source),
        progress: status.progress.map(indexed_search_progress_from_bridge),
        scheduler: IndexSchedulerDiagnostics {
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

pub fn indexed_search_match_from_bridge(
    found: opcda_bridge::IndexedSearchMatch,
) -> IndexedSearchMatch {
    IndexedSearchMatch {
        item_id: found.item_id,
        display_name: found.display_name,
        kind: match found.kind {
            opcda_bridge::BrowseNodeKind::Unspecified => BrowseNodeKind::Unspecified,
            opcda_bridge::BrowseNodeKind::Branch => BrowseNodeKind::Branch,
            opcda_bridge::BrowseNodeKind::Item => BrowseNodeKind::Item,
            opcda_bridge::BrowseNodeKind::BranchAndItem => BrowseNodeKind::BranchAndItem,
        },
        breadcrumbs: found.breadcrumbs,
    }
}

pub fn search_index_response_from_bridge(
    response: opcda_bridge::SearchIndexResponse,
) -> SearchIndexResponse {
    SearchIndexResponse {
        matches: response
            .matches
            .into_iter()
            .map(indexed_search_match_from_bridge)
            .collect(),
        has_more: response.has_more,
        status: search_index_status_from_bridge(response.status),
    }
}

impl From<SearchIndexControlAction> for opcda_bridge::SearchIndexControlAction {
    fn from(action: SearchIndexControlAction) -> Self {
        match action {
            SearchIndexControlAction::Pause => Self::Pause,
            SearchIndexControlAction::Resume => Self::Resume,
            SearchIndexControlAction::Cancel => Self::Cancel,
        }
    }
}

pub fn namespace_organization_from_bridge(
    organization: opcda_bridge::NamespaceOrganization,
) -> NamespaceOrganization {
    match organization {
        opcda_bridge::NamespaceOrganization::Unspecified => NamespaceOrganization::Unspecified,
        opcda_bridge::NamespaceOrganization::Flat => NamespaceOrganization::Flat,
        opcda_bridge::NamespaceOrganization::Hierarchical => NamespaceOrganization::Hierarchical,
    }
}

pub fn browse_source_from_bridge(source: opcda_bridge::BrowseSource) -> BrowseSource {
    match source {
        opcda_bridge::BrowseSource::Unspecified => BrowseSource::Unspecified,
        opcda_bridge::BrowseSource::Da3 => BrowseSource::Da3,
        opcda_bridge::BrowseSource::Da2 => BrowseSource::Da2,
        opcda_bridge::BrowseSource::Flat => BrowseSource::Flat,
        opcda_bridge::BrowseSource::Derived => BrowseSource::Derived,
    }
}

pub fn browse_node_from_bridge(node: opcda_bridge::BrowseNode) -> BrowseNode {
    BrowseNode {
        node_key: node.node_key,
        display_name: node.display_name,
        kind: match node.kind {
            opcda_bridge::BrowseNodeKind::Unspecified => BrowseNodeKind::Unspecified,
            opcda_bridge::BrowseNodeKind::Branch => BrowseNodeKind::Branch,
            opcda_bridge::BrowseNodeKind::Item => BrowseNodeKind::Item,
            opcda_bridge::BrowseNodeKind::BranchAndItem => BrowseNodeKind::BranchAndItem,
        },
        item_id: node.item_id,
    }
}

pub fn browse_page_from_bridge(page: opcda_bridge::BrowsePage) -> DriverResult<BrowsePage> {
    Ok(BrowsePage {
        session_id: page.session_id,
        nodes: page
            .nodes
            .into_iter()
            .map(browse_node_from_bridge)
            .collect(),
        next_page_token: page.next_page_token,
        complete: page.complete,
        organization: namespace_organization_from_bridge(page.organization),
        source: browse_source_from_bridge(page.source),
        warning: page.warning,
    })
}

pub fn search_event_from_bridge(event: opcda_bridge::SearchEvent) -> DriverResult<SearchEvent> {
    match event {
        opcda_bridge::SearchEvent::Match(found) => Ok(SearchEvent::Match(SearchMatch {
            node: browse_node_from_bridge(found.node),
            breadcrumbs: found
                .breadcrumbs
                .into_iter()
                .map(|part| BrowseBreadcrumb {
                    node_key: part.node_key,
                    display_name: part.display_name,
                })
                .collect(),
        })),
        opcda_bridge::SearchEvent::Progress(progress) => {
            Ok(SearchEvent::Progress(SearchProgress {
                visited_nodes: progress.visited_nodes,
                matches: progress.matches,
                partial: progress.partial,
            }))
        }
        opcda_bridge::SearchEvent::Completed(completed) => {
            Ok(SearchEvent::Completed(SearchCompleted {
                complete: completed.complete,
                cancelled: completed.cancelled,
                truncated: completed.truncated,
                warning: completed.warning,
            }))
        }
    }
}

fn match_search_mode(mode: SearchMatchMode) -> opcda_bridge::SearchMatchMode {
    match mode {
        SearchMatchMode::Exact => opcda_bridge::SearchMatchMode::Exact,
        SearchMatchMode::Prefix => opcda_bridge::SearchMatchMode::Prefix,
        SearchMatchMode::Contains => opcda_bridge::SearchMatchMode::Contains,
    }
}

/// Maps every `opcda_bridge::Error` this driver can encounter — at connect time or during
/// any RPC — to the matching [`DriverError`] variant: `opcda_bridge::Error::Connect` (the
/// gRPC channel itself couldn't be established) becomes [`DriverError::Connect`], and
/// `opcda_bridge::Error::Rpc` (the channel is fine, but the gateway returned a gRPC error
/// for this specific call) becomes [`DriverError::Operation`], except for an indexed-search
/// `FailedPrecondition`, which becomes [`DriverError::IndexOperationRejected`] so gateway
/// configuration and concurrency diagnostics remain actionable. An exhaustive match rather
/// than a wildcard arm, deliberately: if `opcda_bridge::Error` ever gains a new variant,
/// this should fail to compile and force a real decision about where it belongs, not
/// silently fall into one bucket.
fn map_bridge_error_for(err: opcda_bridge::Error, operation: &'static str) -> DriverError {
    match &err {
        opcda_bridge::Error::Connect(_) => DriverError::Connect(Box::new(err)),
        opcda_bridge::Error::Rpc(status)
            if is_indexed_search_operation(operation)
                && matches!(
                    status.code(),
                    tonic::Code::InvalidArgument
                        | tonic::Code::NotFound
                        | tonic::Code::AlreadyExists
                        | tonic::Code::FailedPrecondition
                ) =>
        {
            DriverError::IndexOperationRejected {
                message: status.message().to_string(),
            }
        }
        opcda_bridge::Error::Rpc(status)
            if (operation == "paged browse" || operation == "browse-session close")
                && matches!(
                    status.code(),
                    tonic::Code::NotFound | tonic::Code::FailedPrecondition
                ) =>
        {
            DriverError::BrowseStateInvalid
        }
        opcda_bridge::Error::UnknownIndexServer { .. }
        | opcda_bridge::Error::IndexNotEnrolled { .. }
        | opcda_bridge::Error::IndexDeleting { .. } => DriverError::IndexOperationRejected {
            message: err.to_string(),
        },
        opcda_bridge::Error::IncompatibleGateway { .. } => {
            DriverError::IncompatibleGateway { operation }
        }
        opcda_bridge::Error::Rpc(_) | opcda_bridge::Error::Protocol(_) => {
            DriverError::Operation(Box::new(err))
        }
    }
}

fn is_indexed_search_operation(operation: &str) -> bool {
    matches!(
        operation,
        "indexed-search status"
            | "indexed-search refresh"
            | "indexed-search control"
            | "indexed-search auto-refresh"
            | "indexed-search delete"
            | "indexed search"
    )
}

#[cfg(test)]
mod tests;

/// End-to-end smoke tests against a minimal mock `Bridge` gRPC service. These prove the typed
/// page/session/search API is wired together correctly without re-testing the bridge's own RPC
/// implementation.
#[cfg(test)]
mod smoke_tests;
