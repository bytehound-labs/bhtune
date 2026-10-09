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
            auto_refresh_policy: status
                .scheduler
                .auto_refresh_policy
                .map(|policy| match policy {
                    opcda_bridge::IndexAutoRefreshPolicy::Allowed => {
                        crate::IndexAutoRefreshPolicy::Allowed
                    }
                    opcda_bridge::IndexAutoRefreshPolicy::Disabled => {
                        crate::IndexAutoRefreshPolicy::Disabled
                    }
                    opcda_bridge::IndexAutoRefreshPolicy::Paused => {
                        crate::IndexAutoRefreshPolicy::Paused
                    }
                }),
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
mod tests {
    use super::*;

    #[test]
    fn quality_from_raw_matches_good_and_uncertain_exactly() {
        assert_eq!(quality_from_raw("Good"), Quality::Good);
        assert_eq!(quality_from_raw("Uncertain"), Quality::Uncertain);
    }

    #[test]
    fn quality_from_raw_treats_bad_as_bad() {
        assert_eq!(quality_from_raw("Bad"), Quality::Bad);
    }

    #[test]
    fn quality_from_raw_treats_unrecognized_codes_as_bad() {
        // The gateway synthesizes "Unknown(0xNNNN)" for quality codes it doesn't otherwise
        // recognize; an unrecognized quality must never be silently trusted.
        assert_eq!(quality_from_raw("Unknown(0x1234)"), Quality::Bad);
        assert_eq!(quality_from_raw(""), Quality::Bad);
    }

    #[test]
    fn tag_value_from_raw_maps_fields_and_drops_the_unreliable_timestamp() {
        let raw = opcda_bridge::TagValue {
            tag_id: "Area1.LIC101.PV".to_string(),
            value: "42.5".to_string(),
            quality: "Good".to_string(),
            timestamp: "2024-01-15 10:23:45".to_string(),
        };
        let value = tag_value_from_raw(raw);
        assert_eq!(value.tag, "Area1.LIC101.PV");
        assert_eq!(value.value, "42.5");
        assert_eq!(value.quality, Quality::Good);
        assert_eq!(value.timestamp, None);
    }

    #[test]
    fn tag_value_from_raw_drops_timestamp_even_for_na_sentinel() {
        let raw = opcda_bridge::TagValue {
            tag_id: "t".to_string(),
            value: "0".to_string(),
            quality: "Bad".to_string(),
            timestamp: "N/A".to_string(),
        };
        assert_eq!(tag_value_from_raw(raw).timestamp, None);
    }

    #[test]
    fn opc_value_from_write_maps_float() {
        assert_eq!(
            opc_value_from_write(TagWrite::Float(55.5)),
            opcda_bridge::Value::Float(55.5)
        );
    }

    #[test]
    fn opc_value_from_write_maps_raw_string() {
        assert_eq!(
            opc_value_from_write(TagWrite::Raw("AUT".into())),
            opcda_bridge::Value::String("AUT".to_string())
        );
    }

    #[test]
    fn write_outcome_from_result_maps_success() {
        let result = opcda_bridge::WriteResult {
            tag_id: "t".to_string(),
            success: true,
            error: None,
        };
        let outcome = write_outcome_from_result(result);
        assert!(outcome.success);
        assert_eq!(outcome.error_message, None);
    }

    #[test]
    fn write_outcome_from_result_maps_failure_with_gateway_message() {
        let result = opcda_bridge::WriteResult {
            tag_id: "t".to_string(),
            success: false,
            error: Some("access denied".to_string()),
        };
        let outcome = write_outcome_from_result(result);
        assert!(!outcome.success);
        assert_eq!(outcome.error_message.as_deref(), Some("access denied"));
    }

    #[test]
    fn write_outcome_from_result_synthesizes_a_message_when_the_gateway_gave_none() {
        let result = opcda_bridge::WriteResult {
            tag_id: "t".to_string(),
            success: false,
            error: None,
        };
        let outcome = write_outcome_from_result(result);
        assert!(!outcome.success);
        assert!(outcome.error_message.is_some());
    }

    #[test]
    fn browse_node_from_bridge_preserves_opaque_identity_and_exact_item_id() {
        let node = browse_node_from_bridge(opcda_bridge::BrowseNode {
            node_key: "opaque-node".to_string(),
            display_name: "PV".to_string(),
            kind: opcda_bridge::BrowseNodeKind::BranchAndItem,
            item_id: Some("FCS0201!204FI00510.PV".to_string()),
        });
        assert_eq!(node.node_key, "opaque-node");
        assert_eq!(node.display_name, "PV");
        assert!(node.kind.is_branch());
        assert!(node.kind.is_item());
        assert_eq!(node.item_id.as_deref(), Some("FCS0201!204FI00510.PV"));
    }

    #[test]
    fn browse_page_from_bridge_maps_nodes_and_continuation_metadata() {
        let page = browse_page_from_bridge(opcda_bridge::BrowsePage {
            session_id: "session".to_string(),
            nodes: vec![opcda_bridge::BrowseNode {
                node_key: "node".to_string(),
                display_name: "PV".to_string(),
                kind: opcda_bridge::BrowseNodeKind::Item,
                item_id: Some("Area1.LIC101.PV".to_string()),
            }],
            next_page_token: Some("next".to_string()),
            complete: false,
            organization: opcda_bridge::NamespaceOrganization::Hierarchical,
            source: opcda_bridge::BrowseSource::Da2,
            warning: Some("partial".to_string()),
        })
        .unwrap();
        assert_eq!(page.session_id, "session");
        assert_eq!(page.nodes[0].item_id.as_deref(), Some("Area1.LIC101.PV"));
        assert_eq!(page.next_page_token.as_deref(), Some("next"));
        assert!(!page.complete);
        assert_eq!(page.warning.as_deref(), Some("partial"));
    }

    #[test]
    fn protocol_enum_mappers_cover_every_wire_variant() {
        for (wire, expected) in [
            (
                opcda_bridge::NamespaceOrganization::Unspecified,
                NamespaceOrganization::Unspecified,
            ),
            (
                opcda_bridge::NamespaceOrganization::Flat,
                NamespaceOrganization::Flat,
            ),
            (
                opcda_bridge::NamespaceOrganization::Hierarchical,
                NamespaceOrganization::Hierarchical,
            ),
        ] {
            assert_eq!(namespace_organization_from_bridge(wire), expected);
        }
        for (wire, expected) in [
            (
                opcda_bridge::BrowseSource::Unspecified,
                BrowseSource::Unspecified,
            ),
            (opcda_bridge::BrowseSource::Da3, BrowseSource::Da3),
            (opcda_bridge::BrowseSource::Da2, BrowseSource::Da2),
            (opcda_bridge::BrowseSource::Flat, BrowseSource::Flat),
            (opcda_bridge::BrowseSource::Derived, BrowseSource::Derived),
        ] {
            assert_eq!(browse_source_from_bridge(wire), expected);
        }
        for (wire, expected) in [
            (
                opcda_bridge::SearchIndexState::Unspecified,
                SearchIndexState::Unspecified,
            ),
            (
                opcda_bridge::SearchIndexState::NotIndexed,
                SearchIndexState::NotIndexed,
            ),
            (
                opcda_bridge::SearchIndexState::Partial,
                SearchIndexState::Partial,
            ),
            (
                opcda_bridge::SearchIndexState::Ready,
                SearchIndexState::Ready,
            ),
            (
                opcda_bridge::SearchIndexState::Stale,
                SearchIndexState::Stale,
            ),
            (
                opcda_bridge::SearchIndexState::Refreshing,
                SearchIndexState::Refreshing,
            ),
            (
                opcda_bridge::SearchIndexState::Promoting,
                SearchIndexState::Promoting,
            ),
            (
                opcda_bridge::SearchIndexState::Failed,
                SearchIndexState::Failed,
            ),
            (
                opcda_bridge::SearchIndexState::Deleting,
                SearchIndexState::Deleting,
            ),
        ] {
            assert_eq!(search_index_state_from_bridge(wire), expected);
        }
        for (wire, expected) in [
            (SearchMatchMode::Exact, opcda_bridge::SearchMatchMode::Exact),
            (
                SearchMatchMode::Prefix,
                opcda_bridge::SearchMatchMode::Prefix,
            ),
            (
                SearchMatchMode::Contains,
                opcda_bridge::SearchMatchMode::Contains,
            ),
        ] {
            assert_eq!(match_search_mode(wire), expected);
        }
        for (wire, expected) in [
            (
                SearchIndexControlAction::Pause,
                opcda_bridge::SearchIndexControlAction::Pause,
            ),
            (
                SearchIndexControlAction::Resume,
                opcda_bridge::SearchIndexControlAction::Resume,
            ),
            (
                SearchIndexControlAction::Cancel,
                opcda_bridge::SearchIndexControlAction::Cancel,
            ),
        ] {
            assert_eq!(
                <opcda_bridge::SearchIndexControlAction as From<_>>::from(wire),
                expected
            );
        }
    }

    #[test]
    fn auto_refresh_policy_labels_preserve_gateway_configuration_meaning() {
        for (policy, label) in [
            (crate::IndexAutoRefreshPolicy::Allowed, "allowed"),
            (crate::IndexAutoRefreshPolicy::Disabled, "disabled"),
            (crate::IndexAutoRefreshPolicy::Paused, "paused"),
        ] {
            assert_eq!(policy.to_string(), label);
        }
    }

    #[test]
    fn indexed_search_mappers_cover_all_node_kinds_and_optional_progress() {
        let kinds = [
            opcda_bridge::BrowseNodeKind::Unspecified,
            opcda_bridge::BrowseNodeKind::Branch,
            opcda_bridge::BrowseNodeKind::Item,
            opcda_bridge::BrowseNodeKind::BranchAndItem,
        ];
        for kind in kinds {
            let found = indexed_search_match_from_bridge(opcda_bridge::IndexedSearchMatch {
                item_id: "item".into(),
                display_name: "Item".into(),
                kind,
                breadcrumbs: vec!["Area".into()],
            });
            let expected = match kind {
                opcda_bridge::BrowseNodeKind::Unspecified => BrowseNodeKind::Unspecified,
                opcda_bridge::BrowseNodeKind::Branch => BrowseNodeKind::Branch,
                opcda_bridge::BrowseNodeKind::Item => BrowseNodeKind::Item,
                opcda_bridge::BrowseNodeKind::BranchAndItem => BrowseNodeKind::BranchAndItem,
            };
            assert_eq!(found.kind, expected);
        }
        let status = search_index_status_from_bridge(opcda_bridge::SearchIndexStatus {
            server: "S".into(),
            state: opcda_bridge::SearchIndexState::Partial,
            active_generation: 2,
            entry_count: 3,
            unique_item_count: 4,
            started_at: None,
            completed_at: None,
            last_error: Some("warning".into()),
            database_bytes: 5,
            organization: opcda_bridge::NamespaceOrganization::Flat,
            source: opcda_bridge::BrowseSource::Derived,
            progress: None,
            effective_limits: None,
            controller_state: opcda_bridge::IndexControllerState::Unspecified,
            pause_reason: None,
            recovery_deadline: None,
            pause_reason_detail: None,
            foreground: opcda_bridge::IndexForegroundDiagnostics {
                active_count: 0,
                operations: 0,
                errors: 0,
                bad_quality: 0,
                latency_p50_ms: None,
                latency_p95_ms: None,
                latency_max_ms: None,
                last_error: false,
                last_bad_quality: false,
            },
            host: opcda_bridge::IndexHostDiagnostics::default(),
            storage: opcda_bridge::IndexStorageDiagnostics::default(),
            scheduler: opcda_bridge::IndexSchedulerDiagnostics::default(),
            health: opcda_bridge::IndexHealthDiagnostics::default(),
            promoting: false,
        });
        assert_eq!(status.state, SearchIndexState::Partial);
        assert!(status.progress.is_none());
    }

    #[test]
    fn search_event_mapper_handles_match_progress_and_completion() {
        let events = [
            opcda_bridge::SearchEvent::Match(opcda_bridge::SearchMatch {
                node: opcda_bridge::BrowseNode {
                    node_key: "n".into(),
                    display_name: "PV".into(),
                    kind: opcda_bridge::BrowseNodeKind::Item,
                    item_id: Some("PV".into()),
                },
                breadcrumbs: vec![opcda_bridge::BrowseBreadcrumb {
                    node_key: "root".into(),
                    display_name: "Root".into(),
                }],
            }),
            opcda_bridge::SearchEvent::Progress(opcda_bridge::SearchProgress {
                visited_nodes: 2,
                matches: 1,
                partial: true,
            }),
            opcda_bridge::SearchEvent::Completed(opcda_bridge::SearchCompleted {
                complete: false,
                cancelled: true,
                truncated: true,
                warning: Some("partial".into()),
            }),
        ];
        assert!(matches!(
            search_event_from_bridge(events[0].clone()).unwrap(),
            SearchEvent::Match(_)
        ));
        assert!(matches!(
            search_event_from_bridge(events[1].clone()).unwrap(),
            SearchEvent::Progress(SearchProgress {
                visited_nodes: 2,
                matches: 1,
                partial: true
            })
        ));
        assert!(matches!(
            search_event_from_bridge(events[2].clone()).unwrap(),
            SearchEvent::Completed(SearchCompleted {
                cancelled: true,
                truncated: true,
                ..
            })
        ));
    }

    #[test]
    fn indexed_search_precondition_preserves_gateway_reason() {
        let err = map_bridge_error_for(
            opcda_bridge::Error::Rpc(tonic::Status::not_found(
                "server is not enrolled for namespace indexing",
            )),
            "indexed-search refresh",
        );
        assert!(matches!(
            err,
            DriverError::IndexOperationRejected { message }
                if message == "server is not enrolled for namespace indexing"
        ));
    }

    #[test]
    fn indexed_search_invalid_argument_is_a_rejected_operation() {
        let err = map_bridge_error_for(
            opcda_bridge::Error::Rpc(tonic::Status::invalid_argument(
                "OPC server is not registered on the gateway",
            )),
            "indexed-search refresh",
        );
        assert!(matches!(
            err,
            DriverError::IndexOperationRejected { message }
                if message == "OPC server is not registered on the gateway"
        ));
    }

    #[test]
    fn bridge_error_mapping_distinguishes_browse_state_and_gateway_compatibility() {
        assert!(matches!(
            map_bridge_error_for(
                opcda_bridge::Error::Rpc(tonic::Status::not_found("gone")),
                "paged browse",
            ),
            DriverError::BrowseStateInvalid
        ));
        assert!(matches!(
            map_bridge_error_for(
                opcda_bridge::Error::Rpc(tonic::Status::failed_precondition("gone")),
                "browse-session close",
            ),
            DriverError::BrowseStateInvalid
        ));
        assert!(matches!(
            map_bridge_error_for(
                opcda_bridge::Error::Rpc(tonic::Status::internal("boom")),
                "read OPC DA tags",
            ),
            DriverError::Operation(_)
        ));
        assert!(matches!(
            map_bridge_error_for(
                opcda_bridge::Error::IncompatibleGateway {
                    operation: "capability discovery",
                },
                "capability discovery",
            ),
            DriverError::IncompatibleGateway {
                operation: "capability discovery"
            }
        ));
        assert!(matches!(
            map_bridge_error_for(
                opcda_bridge::Error::Protocol("bad payload".into()),
                "read OPC DA tags",
            ),
            DriverError::Operation(_)
        ));
    }

    #[test]
    fn bridge_error_mapping_preserves_typed_index_enrollment_errors() {
        for error in [
            opcda_bridge::Error::UnknownIndexServer {
                server: "Unknown.Server".into(),
            },
            opcda_bridge::Error::IndexNotEnrolled {
                server: "Known.Server".into(),
            },
        ] {
            assert!(matches!(
                map_bridge_error_for(error, "indexed-search refresh"),
                DriverError::IndexOperationRejected { message }
                    if message.contains("Server")
            ));
        }
    }

    #[tokio::test]
    async fn connect_failure_maps_to_driver_error_connect() {
        // Nothing is listening on this port, so `Client::connect` fails at the transport
        // level before any RPC is attempted -- exactly the `DriverError::Connect` case.
        let err = OpcDaDriver::connect("127.0.0.1:1", "AnyServer")
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::Connect(_)));
    }

    #[tokio::test]
    async fn list_opcda_servers_connect_failure_maps_to_driver_error_connect() {
        let err = list_opcda_servers("127.0.0.1:1").await.unwrap_err();
        assert!(matches!(err, DriverError::Connect(_)));
    }

    #[tokio::test]
    async fn get_opcda_gateway_info_connect_failure_maps_to_driver_error_connect() {
        let err = get_opcda_gateway_info("127.0.0.1:1").await.unwrap_err();
        assert!(matches!(err, DriverError::Connect(_)));
    }

    #[tokio::test]
    async fn check_gateway_compatibility_connect_failure_maps_to_driver_error_connect() {
        let err = check_gateway_compatibility("127.0.0.1:1", None)
            .await
            .unwrap_err();
        assert!(matches!(err, DriverError::Connect(_)));
    }

    fn sample_feature(
        feature: OpcDaGatewayFeature,
        status: OpcDaFeatureCompatibilityStatus,
        gateway_versions: Option<OpcDaProtocolRange>,
    ) -> OpcDaFeatureCompatibility {
        OpcDaFeatureCompatibility {
            feature,
            status,
            client_versions: OpcDaProtocolRange { min: 1, max: 1 },
            gateway_versions,
            negotiated_version: None,
            reason: String::new(),
        }
    }

    fn sample_compatibility(
        status: OpcDaCompatibilityStatus,
        gateway_version: Option<&str>,
        features: Vec<OpcDaFeatureCompatibility>,
    ) -> OpcDaGatewayCompatibility {
        OpcDaGatewayCompatibility {
            client_version: "0.1.0".into(),
            gateway_version: gateway_version.map(str::to_string),
            source: OpcDaCompatibilitySource::GatewayInfo,
            status,
            features,
        }
    }

    #[test]
    fn compatibility_identifiers_are_stable_snake_case() {
        assert_eq!(OpcDaProtocolRange { min: 1, max: 2 }.to_string(), "1-2");
        assert_eq!(
            [
                OpcDaCompatibilityStatus::Full,
                OpcDaCompatibilityStatus::Partial,
                OpcDaCompatibilityStatus::Incompatible,
                OpcDaCompatibilityStatus::Unknown,
            ]
            .map(OpcDaCompatibilityStatus::as_str),
            ["full", "partial", "incompatible", "unknown"]
        );
        assert_eq!(
            [
                OpcDaCompatibilitySource::GatewayInfo,
                OpcDaCompatibilitySource::LegacyCapabilities,
                OpcDaCompatibilitySource::Unknown,
            ]
            .map(OpcDaCompatibilitySource::as_str),
            ["gateway_info", "legacy_capabilities", "unknown"]
        );
        assert_eq!(
            [
                OpcDaFeatureCompatibilityStatus::Compatible,
                OpcDaFeatureCompatibilityStatus::Unsupported,
                OpcDaFeatureCompatibilityStatus::Incompatible,
                OpcDaFeatureCompatibilityStatus::Unknown,
            ]
            .map(OpcDaFeatureCompatibilityStatus::as_str),
            ["compatible", "unsupported", "incompatible", "unknown"]
        );
    }

    #[test]
    fn feature_description_names_both_protocol_ranges() {
        let reported = sample_feature(
            OpcDaGatewayFeature::Core,
            OpcDaFeatureCompatibilityStatus::Incompatible,
            Some(OpcDaProtocolRange { min: 3, max: 3 }),
        );
        assert_eq!(
            reported.describe(),
            "core protocol incompatible: bhtune supports 1-1, gateway reports 3-3"
        );
        let missing = sample_feature(
            OpcDaGatewayFeature::IndexedSearch,
            OpcDaFeatureCompatibilityStatus::Unsupported,
            None,
        );
        assert_eq!(
            missing.describe(),
            "indexed_search protocol unsupported: bhtune supports 1-1, gateway reports none"
        );
    }

    #[test]
    fn an_incompatible_gateway_gets_an_actionable_refusal_and_no_warning() {
        let compatibility = sample_compatibility(
            OpcDaCompatibilityStatus::Incompatible,
            Some("0.9.0"),
            vec![sample_feature(
                OpcDaGatewayFeature::Core,
                OpcDaFeatureCompatibilityStatus::Incompatible,
                Some(OpcDaProtocolRange { min: 3, max: 3 }),
            )],
        );
        assert!(compatibility.is_incompatible());
        assert_eq!(compatibility.warning_message(), None);
        assert_eq!(
            compatibility.live_mutation_refusal().as_deref(),
            Some(compatibility.incompatibility_message().as_str())
        );
        assert_eq!(
            compatibility.incompatibility_message(),
            "opcda-bridge gateway 0.9.0 is incompatible with bhtune 0.1.0 (core protocol \
             incompatible: bhtune supports 1-1, gateway reports 3-3); upgrade the opcda-bridge \
             gateway or bhtune so their core protocol versions overlap"
        );
    }

    #[test]
    fn a_partially_compatible_gateway_warns_about_only_the_degraded_features() {
        let compatibility = sample_compatibility(
            OpcDaCompatibilityStatus::Partial,
            Some("0.3.2"),
            vec![
                sample_feature(
                    OpcDaGatewayFeature::Core,
                    OpcDaFeatureCompatibilityStatus::Compatible,
                    Some(OpcDaProtocolRange { min: 1, max: 1 }),
                ),
                sample_feature(
                    OpcDaGatewayFeature::IndexedSearch,
                    OpcDaFeatureCompatibilityStatus::Unsupported,
                    None,
                ),
            ],
        );
        assert!(!compatibility.is_incompatible());
        assert_eq!(
            compatibility.warning_message().as_deref(),
            Some(
                "opcda-bridge gateway 0.3.2 is partially compatible with bhtune 0.1.0; affected \
                 optional features are degraded (indexed_search protocol unsupported: bhtune \
                 supports 1-1, gateway reports none)"
            )
        );
    }

    #[test]
    fn an_unverifiable_gateway_warns_without_a_version_or_feature_details() {
        let compatibility = sample_compatibility(OpcDaCompatibilityStatus::Unknown, None, vec![]);
        assert!(!compatibility.is_incompatible());
        assert_eq!(
            compatibility.warning_message().as_deref(),
            Some(
                "opcda-bridge gateway (version not reported) did not report enough protocol \
                 metadata to verify compatibility with bhtune 0.1.0; proceeding without \
                 verification"
            )
        );
    }

    #[test]
    fn a_fully_compatible_gateway_needs_no_warning() {
        let compatibility = sample_compatibility(
            OpcDaCompatibilityStatus::Full,
            Some("0.5.9"),
            vec![sample_feature(
                OpcDaGatewayFeature::Core,
                OpcDaFeatureCompatibilityStatus::Compatible,
                Some(OpcDaProtocolRange { min: 1, max: 1 }),
            )],
        );
        assert!(!compatibility.is_incompatible());
        assert_eq!(compatibility.warning_message(), None);
        assert_eq!(compatibility.live_mutation_refusal(), None);
    }

    #[test]
    fn an_unverified_report_is_unknown_and_does_not_refuse_live_mutations() {
        let compatibility = OpcDaGatewayCompatibility::unverified();
        assert_eq!(compatibility.status, OpcDaCompatibilityStatus::Unknown);
        assert_eq!(compatibility.client_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(compatibility.live_mutation_refusal(), None);
        assert!(compatibility.warning_message().is_some());
    }

    #[test]
    fn bridge_compatibility_enums_map_exhaustively() {
        assert_eq!(
            [
                opcda_bridge::CompatibilityFeature::Core,
                opcda_bridge::CompatibilityFeature::Namespace,
                opcda_bridge::CompatibilityFeature::IndexedSearch,
            ]
            .map(gateway_feature_from_bridge),
            [
                OpcDaGatewayFeature::Core,
                OpcDaGatewayFeature::Namespace,
                OpcDaGatewayFeature::IndexedSearch,
            ]
        );
        assert_eq!(
            [
                opcda_bridge::CompatibilityStatus::Full,
                opcda_bridge::CompatibilityStatus::Partial,
                opcda_bridge::CompatibilityStatus::Incompatible,
                opcda_bridge::CompatibilityStatus::Unknown,
            ]
            .map(compatibility_status_from_bridge),
            [
                OpcDaCompatibilityStatus::Full,
                OpcDaCompatibilityStatus::Partial,
                OpcDaCompatibilityStatus::Incompatible,
                OpcDaCompatibilityStatus::Unknown,
            ]
        );
        assert_eq!(
            [
                opcda_bridge::CompatibilitySource::GatewayInfo,
                opcda_bridge::CompatibilitySource::LegacyCapabilities,
                opcda_bridge::CompatibilitySource::Unknown,
            ]
            .map(compatibility_source_from_bridge),
            [
                OpcDaCompatibilitySource::GatewayInfo,
                OpcDaCompatibilitySource::LegacyCapabilities,
                OpcDaCompatibilitySource::Unknown,
            ]
        );
        assert_eq!(
            [
                opcda_bridge::FeatureCompatibilityStatus::Compatible,
                opcda_bridge::FeatureCompatibilityStatus::Unsupported,
                opcda_bridge::FeatureCompatibilityStatus::Incompatible,
                opcda_bridge::FeatureCompatibilityStatus::Unknown,
            ]
            .map(feature_status_from_bridge),
            [
                OpcDaFeatureCompatibilityStatus::Compatible,
                OpcDaFeatureCompatibilityStatus::Unsupported,
                OpcDaFeatureCompatibilityStatus::Incompatible,
                OpcDaFeatureCompatibilityStatus::Unknown,
            ]
        );
    }

    fn bridge_report(gateway_version: Option<&str>) -> opcda_bridge::CompatibilityReport {
        opcda_bridge::CompatibilityReport {
            client_version: "0.1.0".into(),
            library_version: "0.5.0".into(),
            gateway_version: gateway_version.map(str::to_string),
            source: opcda_bridge::CompatibilitySource::GatewayInfo,
            status: opcda_bridge::CompatibilityStatus::Partial,
            evidence: opcda_bridge::CompatibilityEvidence::Unverified,
            features: vec![opcda_bridge::FeatureCompatibility {
                feature: opcda_bridge::CompatibilityFeature::Namespace,
                status: opcda_bridge::FeatureCompatibilityStatus::Compatible,
                client_versions: opcda_bridge::ProtocolVersionRange { min: 2, max: 2 },
                gateway_versions: Some(opcda_bridge::ProtocolVersionRange { min: 1, max: 2 }),
                negotiated_version: Some(2),
                reason: "overlap".into(),
            }],
        }
    }

    #[test]
    fn a_bridge_report_maps_every_field_and_drops_a_blank_gateway_version() {
        let mapped = gateway_compatibility_from_report(bridge_report(Some("0.5.9")));
        assert_eq!(
            mapped,
            OpcDaGatewayCompatibility {
                client_version: "0.1.0".into(),
                gateway_version: Some("0.5.9".into()),
                source: OpcDaCompatibilitySource::GatewayInfo,
                status: OpcDaCompatibilityStatus::Partial,
                features: vec![OpcDaFeatureCompatibility {
                    feature: OpcDaGatewayFeature::Namespace,
                    status: OpcDaFeatureCompatibilityStatus::Compatible,
                    client_versions: OpcDaProtocolRange { min: 2, max: 2 },
                    gateway_versions: Some(OpcDaProtocolRange { min: 1, max: 2 }),
                    negotiated_version: Some(2),
                    reason: "overlap".into(),
                }],
            }
        );
        assert_eq!(
            gateway_compatibility_from_report(bridge_report(Some("  "))).gateway_version,
            None
        );
        assert_eq!(
            gateway_compatibility_from_report(bridge_report(None)).gateway_version,
            None
        );
    }
}

/// End-to-end smoke tests against a minimal mock `Bridge` gRPC service. These prove the typed
/// page/session/search API is wired together correctly without re-testing the bridge's own RPC
/// implementation.
#[cfg(test)]
mod smoke_tests {
    use super::*;
    use bhtune_test_support::MockBridgeService;
    use opcda_bridge_proto::bridge::search_event;
    use opcda_bridge_proto::bridge::{
        BrowseNode as ProtoBrowseNode, BrowseNodeKind as ProtoNodeKind,
        BrowsePage as ProtoBrowsePage, BrowseSource as ProtoBrowseSource, GetCapabilitiesResponse,
        GetGatewayInfoResponse, IndexedSearchMatch as ProtoIndexedSearchMatch, ListServersResponse,
        NamespaceOrganization as ProtoOrganization, ProtocolFeature, ProtocolFeatureKind,
        ReadResponse, SearchEvent as ProtoSearchEvent,
        SearchIndexResponse as ProtoSearchIndexResponse, SearchIndexState as ProtoSearchIndexState,
        SearchIndexStatus as ProtoSearchIndexStatus, SearchProgress as ProtoSearchProgress,
        TagValue as ProtoTagValue, WriteResponse,
    };
    use tonic::Status;

    async fn start_mock_server(
        service: MockBridgeService,
    ) -> (String, bhtune_test_support::MockServerHandle) {
        bhtune_test_support::start_mock_server(service)
            .await
            .expect("bind mock bridge")
    }

    fn browse_page() -> ProtoBrowsePage {
        ProtoBrowsePage {
            session_id: "session".into(),
            nodes: vec![
                ProtoBrowseNode {
                    node_key: "area".into(),
                    display_name: "Area1".into(),
                    kind: ProtoNodeKind::Branch as i32,
                    item_id: None,
                },
                ProtoBrowseNode {
                    node_key: "pv".into(),
                    display_name: "PV".into(),
                    kind: ProtoNodeKind::BranchAndItem as i32,
                    item_id: Some("FCS0201!204FI00510.PV".into()),
                },
            ],
            complete: true,
            organization: ProtoOrganization::Hierarchical as i32,
            source: ProtoBrowseSource::Da2 as i32,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn read_round_trips_through_a_real_gateway_connection() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Area1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        let values = driver.read(&["Area1.LIC101.PV".to_string()]).await.unwrap();
        assert_eq!(values[0].quality, Quality::Good);
        assert_eq!(values[0].value, "42.5");
    }

    #[tokio::test]
    async fn write_round_trips_a_rejected_write() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Area1.LIC101.MV".to_string(),
                success: false,
                error: Some("tag is read-only".to_string()),
            },
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        let outcome = driver
            .write(&"Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))
            .await
            .unwrap();
        assert_eq!(outcome.error_message.as_deref(), Some("tag is read-only"));
    }

    #[tokio::test]
    async fn capabilities_and_browse_preserve_typed_namespace_metadata() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            capabilities_response: GetCapabilitiesResponse {
                application_version: "0.4.0".into(),
                protocol_version: "2".into(),
                max_page_size: 1000,
                supports_browse_sessions: true,
                supports_search: true,
                organization: ProtoOrganization::Hierarchical as i32,
                source: ProtoBrowseSource::Da2 as i32,
                supports_indexed_search: true,
                indexed_search_protocol_version: "1".into(),
                max_indexed_search_results: 50,
                search_index_state: ProtoSearchIndexState::Ready as i32,
                search_index_promoting: false,
            },
            browse_response: browse_page(),
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        let capabilities = driver.capabilities().await.unwrap();
        assert_eq!(capabilities.application_version, "0.4.0");
        assert!(capabilities.supports_browse_sessions);
        assert!(capabilities.supports_indexed_search);
        assert_eq!(capabilities.indexed_search_protocol_version, "1");
        let page = driver.browse(BrowsePageRequest::root(200)).await.unwrap();
        assert_eq!(page.session_id, "session");
        assert!(page.nodes[0].kind.is_branch());
        assert!(page.nodes[1].kind.is_item());
        assert_eq!(
            page.nodes[1].item_id.as_deref(),
            Some("FCS0201!204FI00510.PV")
        );
        driver.close_browse_session("session").await.unwrap();
    }

    #[tokio::test]
    async fn indexed_search_round_trips_exact_item_id_and_status() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            search_index_status_response: ProtoSearchIndexStatus {
                server: "S1".into(),
                state: ProtoSearchIndexState::Ready as i32,
                active_generation: 7,
                entry_count: 2,
                unique_item_count: 2,
                organization: ProtoOrganization::Hierarchical as i32,
                source: ProtoBrowseSource::Da2 as i32,
                ..Default::default()
            },
            search_index_response: ProtoSearchIndexResponse {
                matches: vec![ProtoIndexedSearchMatch {
                    item_id: "FCS0201!204FI00510.PV".into(),
                    display_name: "PV".into(),
                    kind: ProtoNodeKind::Item as i32,
                    breadcrumbs: vec!["FCS0201".into(), "204FI00510".into()],
                }],
                has_more: false,
                status: Some(ProtoSearchIndexStatus {
                    server: "S1".into(),
                    state: ProtoSearchIndexState::Ready as i32,
                    active_generation: 7,
                    entry_count: 2,
                    unique_item_count: 2,
                    organization: ProtoOrganization::Hierarchical as i32,
                    source: ProtoBrowseSource::Da2 as i32,
                    ..Default::default()
                }),
            },
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        let status = driver.search_index_status().await.unwrap();
        assert_eq!(status.state, SearchIndexState::Ready);
        assert_eq!(status.active_generation, 7);
        driver.delete_search_index().await.unwrap();
        let response = driver
            .search_index_query(SearchIndexRequest::new(
                "FCS0201!204FI00510",
                SearchMatchMode::Prefix,
                50,
            ))
            .await
            .unwrap();
        assert_eq!(response.matches[0].item_id, "FCS0201!204FI00510.PV");
        assert!(!response.has_more);
    }

    #[tokio::test]
    async fn search_stream_preserves_progress_and_completion() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            search_events: vec![
                ProtoSearchEvent {
                    event: Some(search_event::Event::Progress(ProtoSearchProgress {
                        visited_nodes: 4,
                        matches: 0,
                        partial: true,
                    })),
                },
                ProtoSearchEvent {
                    event: Some(search_event::Event::Completed(
                        opcda_bridge_proto::bridge::SearchCompleted {
                            complete: true,
                            cancelled: false,
                            truncated: false,
                            warning: None,
                        },
                    )),
                },
            ],
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        let events = driver
            .search_events(SearchRequest::new("PV", SearchMatchMode::Contains, 20))
            .await
            .unwrap();
        assert!(matches!(events[0], SearchEvent::Progress(_)));
        assert!(matches!(events[1], SearchEvent::Completed(_)));
    }

    #[tokio::test]
    async fn driver_trait_delegates_all_opcda_operations() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            capabilities_response: GetCapabilitiesResponse {
                protocol_version: "2".into(),
                ..Default::default()
            },
            browse_response: ProtoBrowsePage {
                complete: true,
                ..Default::default()
            },
            search_index_response: ProtoSearchIndexResponse {
                status: Some(ProtoSearchIndexStatus::default()),
                ..Default::default()
            },
            search_events: vec![ProtoSearchEvent {
                event: Some(search_event::Event::Completed(
                    opcda_bridge_proto::bridge::SearchCompleted {
                        complete: true,
                        ..Default::default()
                    },
                )),
            }],
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        assert_eq!(
            <OpcDaDriver as Driver>::capabilities(&driver)
                .await
                .unwrap()
                .protocol_version,
            "2"
        );
        <OpcDaDriver as Driver>::browse(&driver, BrowsePageRequest::root(1))
            .await
            .unwrap();
        <OpcDaDriver as Driver>::close_browse_session(&driver, "session")
            .await
            .unwrap();
        let events = <OpcDaDriver as Driver>::search(
            &driver,
            SearchRequest::new("PV", SearchMatchMode::Contains, 10),
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 1);
        assert!(
            <OpcDaDriver as Driver>::search_index_status(&driver)
                .await
                .is_ok()
        );
        assert!(
            <OpcDaDriver as Driver>::refresh_search_index(&driver, false)
                .await
                .is_ok()
        );
        assert!(
            <OpcDaDriver as Driver>::delete_search_index(&driver)
                .await
                .is_ok()
        );
        assert!(
            <OpcDaDriver as Driver>::control_search_index(&driver, SearchIndexControlAction::Pause)
                .await
                .is_ok()
        );
        assert!(
            <OpcDaDriver as Driver>::search_index(
                &driver,
                SearchIndexRequest::new("PV", SearchMatchMode::Exact, 1)
            )
            .await
            .is_ok()
        );
    }

    #[tokio::test]
    async fn gateway_info_reports_gateway_wide_protocols_without_an_opc_server() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            gateway_info_response: GetGatewayInfoResponse {
                application_version: "0.5.9".into(),
                compatibility_schema_version: 1,
                features: vec![
                    ProtocolFeature {
                        kind: ProtocolFeatureKind::Core as i32,
                        min_version: 1,
                        max_version: 1,
                    },
                    ProtocolFeature {
                        kind: ProtocolFeatureKind::Namespace as i32,
                        min_version: 2,
                        max_version: 3,
                    },
                    ProtocolFeature {
                        kind: ProtocolFeatureKind::IndexedSearch as i32,
                        min_version: 2,
                        max_version: 2,
                    },
                ],
            },
            ..Default::default()
        })
        .await;

        assert_eq!(
            get_opcda_gateway_info(&host).await.unwrap(),
            OpcDaGatewayInfo {
                application_version: "0.5.9".into(),
                compatibility_schema_version: 1,
                features: vec![
                    OpcDaGatewayFeatureSupport {
                        feature: OpcDaGatewayFeature::Core,
                        min_version: 1,
                        max_version: 1,
                    },
                    OpcDaGatewayFeatureSupport {
                        feature: OpcDaGatewayFeature::Namespace,
                        min_version: 2,
                        max_version: 3,
                    },
                    OpcDaGatewayFeatureSupport {
                        feature: OpcDaGatewayFeature::IndexedSearch,
                        min_version: 2,
                        max_version: 2,
                    },
                ],
            }
        );
        assert_eq!(
            get_opcda_gateway_info(&host)
                .await
                .unwrap()
                .features
                .into_iter()
                .map(|support| support.feature.as_str())
                .collect::<Vec<_>>(),
            vec!["core", "namespace", "indexed_search"]
        );
    }

    fn protocol_feature(
        kind: ProtocolFeatureKind,
        min_version: u32,
        max_version: u32,
    ) -> ProtocolFeature {
        ProtocolFeature {
            kind: kind as i32,
            min_version,
            max_version,
        }
    }

    fn gateway_info_with_features(features: Vec<ProtocolFeature>) -> GetGatewayInfoResponse {
        GetGatewayInfoResponse {
            application_version: "0.5.9".into(),
            compatibility_schema_version: 1,
            features,
        }
    }

    #[tokio::test]
    async fn check_gateway_compatibility_classifies_full_partial_unknown_and_incompatible() {
        let (full, _full_server) = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info_with_features(vec![
                protocol_feature(ProtocolFeatureKind::Core, 1, 1),
                protocol_feature(ProtocolFeatureKind::Namespace, 2, 3),
                protocol_feature(ProtocolFeatureKind::IndexedSearch, 3, 3),
            ]),
            ..Default::default()
        })
        .await;
        let full_report = check_gateway_compatibility(&full, None).await.unwrap();
        assert_eq!(full_report.status, OpcDaCompatibilityStatus::Full);
        assert_eq!(full_report.live_mutation_refusal(), None);

        let (partial, _partial_server) = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info_with_features(vec![protocol_feature(
                ProtocolFeatureKind::Core,
                1,
                1,
            )]),
            ..Default::default()
        })
        .await;
        let partial_report = check_gateway_compatibility(&partial, None).await.unwrap();
        assert_eq!(partial_report.status, OpcDaCompatibilityStatus::Partial);
        assert!(partial_report.warning_message().is_some());
        assert_eq!(partial_report.live_mutation_refusal(), None);

        let (unknown, _unknown_server) = start_mock_server(MockBridgeService::default()).await;
        let unknown_report = check_gateway_compatibility(&unknown, None).await.unwrap();
        assert_eq!(unknown_report.status, OpcDaCompatibilityStatus::Unknown);
        assert!(unknown_report.warning_message().is_some());
        assert_eq!(unknown_report.live_mutation_refusal(), None);

        let (incompatible, _incompatible_server) = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info_with_features(vec![protocol_feature(
                ProtocolFeatureKind::Core,
                9,
                9,
            )]),
            ..Default::default()
        })
        .await;
        let incompatible_report = check_gateway_compatibility(&incompatible, Some("Plant.Server"))
            .await
            .unwrap();
        assert!(incompatible_report.is_incompatible());
        assert!(incompatible_report.live_mutation_refusal().is_some());
    }

    #[tokio::test]
    async fn unimplemented_metadata_with_a_server_is_an_incompatible_report() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            gateway_info_error: Some(Status::unimplemented("no gateway info")),
            capabilities_error: Some(Status::unimplemented("no capabilities")),
            ..Default::default()
        })
        .await;
        let report = check_gateway_compatibility(&host, Some("Plant.Server"))
            .await
            .unwrap();
        assert!(report.is_incompatible());
        assert!(report.features.iter().any(|feature| {
            feature.reason.contains("capability discovery")
                && matches!(feature.feature, OpcDaGatewayFeature::Core)
                && matches!(
                    feature.status,
                    OpcDaFeatureCompatibilityStatus::Incompatible
                )
        }));
    }

    #[tokio::test]
    async fn unimplemented_metadata_without_a_server_is_unknown() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            gateway_info_error: Some(Status::unimplemented("no gateway info")),
            ..Default::default()
        })
        .await;
        let report = check_gateway_compatibility(&host, None).await.unwrap();
        assert_eq!(report.status, OpcDaCompatibilityStatus::Unknown);
        assert_eq!(report.live_mutation_refusal(), None);
    }

    #[tokio::test]
    async fn other_metadata_failures_remain_operation_errors() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            gateway_info_error: Some(Status::internal("metadata failed")),
            ..Default::default()
        })
        .await;
        let err = check_gateway_compatibility(&host, None).await.unwrap_err();
        assert!(matches!(err, DriverError::Operation(_)));
    }

    #[tokio::test]
    async fn driver_trait_maps_close_browse_rpc_errors() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            close_error: Some(Status::internal("close failed")),
            ..Default::default()
        })
        .await;
        let driver = OpcDaDriver::connect(&host, "S1").await.unwrap();
        assert!(matches!(
            <OpcDaDriver as Driver>::close_browse_session(&driver, "session").await,
            Err(DriverError::Operation(_))
        ));
    }

    #[tokio::test]
    async fn list_opcda_servers_returns_the_gateways_registered_servers() {
        let (host, _host_server) = start_mock_server(MockBridgeService {
            list_servers_response: ListServersResponse {
                servers: vec!["Matrikon.OPC.Simulation.1".into()],
            },
            ..Default::default()
        })
        .await;
        assert_eq!(
            list_opcda_servers(&host).await.unwrap(),
            vec!["Matrikon.OPC.Simulation.1".to_string()]
        );
    }

    proptest::proptest! {
        #[test]
        fn arbitrary_protocol_payloads_preserve_safe_fields(
            tag in proptest::prelude::any::<String>(),
            value in proptest::prelude::any::<String>(),
            quality in proptest::prelude::any::<String>(),
            timestamp in proptest::prelude::any::<String>(),
            write_error in proptest::prelude::prop::option::of(proptest::prelude::any::<String>()),
            success in proptest::prelude::any::<bool>(),
        ) {
            let mapped = tag_value_from_raw(opcda_bridge::TagValue {
                tag_id: tag.clone(),
                value: value.clone(),
                quality,
                timestamp,
            });
            proptest::prop_assert_eq!(mapped.tag, tag.clone());
            proptest::prop_assert_eq!(mapped.value, value.clone());
            proptest::prop_assert_eq!(mapped.timestamp, None);

            let node = browse_node_from_bridge(opcda_bridge::BrowseNode {
                node_key: tag.clone(),
                display_name: value.clone(),
                kind: opcda_bridge::BrowseNodeKind::Item,
                item_id: Some(tag.clone()),
            });
            proptest::prop_assert_eq!(node.node_key, tag);
            proptest::prop_assert!(node.kind.is_item());

            let outcome = write_outcome_from_result(opcda_bridge::WriteResult {
                tag_id: String::new(),
                success,
                error: write_error.clone(),
            });
            proptest::prop_assert_eq!(outcome.success, success);
            if success {
                proptest::prop_assert_eq!(outcome.error_message, None);
            } else {
                proptest::prop_assert_eq!(
                    outcome.error_message,
                    Some(write_error.unwrap_or_else(|| "gateway rejected the write".to_string()))
                );
            }
        }

        #[test]
        fn arbitrary_numeric_writes_keep_the_f32_value(value in -1_000_000.0f32..1_000_000.0f32) {
            proptest::prop_assert_eq!(
                opc_value_from_write(TagWrite::Float(value)),
                opcda_bridge::Value::Float(f64::from(value))
            );
        }
    }
}
