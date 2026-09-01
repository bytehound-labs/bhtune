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
        DriverCapabilities, IndexedSearchMatch, IndexedSearchProgress, NamespaceOrganization,
        Quality, SearchCompleted, SearchEvent, SearchIndexControlAction, SearchIndexRequest,
        SearchIndexResponse, SearchIndexState, SearchIndexStatus, SearchMatch, SearchMatchMode,
        SearchProgress, SearchRequest, TagId, TagValue, TagWrite, WriteOutcome,
    },
};

/// Default number of children requested for one browse page. The bridge enforces its own
/// maximum; this value keeps the browser responsive and matches the bridge library default.
pub const DEFAULT_PAGE_SIZE: u32 = opcda_bridge::DEFAULT_PAGE_SIZE;

/// Default maximum number of search matches requested by the CLI and browser.
pub const DEFAULT_SEARCH_MAX_RESULTS: u32 = opcda_bridge::DEFAULT_SEARCH_MAX_RESULTS;

/// Default maximum number of matches requested from the persistent namespace index.
pub const DEFAULT_INDEX_SEARCH_MAX_RESULTS: u32 = opcda_bridge::DEFAULT_INDEX_SEARCH_MAX_RESULTS;

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

    async fn search_index(&self, request: SearchIndexRequest) -> DriverResult<SearchIndexResponse> {
        self.search_index_query(request).await
    }
}

/// Maps `opcda_bridge`'s raw OPC quality string to [`Quality`].
///
/// Per `opc-da-client`'s documented contract (the Windows-only library the gateway wraps on
/// the other side), this is one of `"Good"`, `"Bad"`, `"Uncertain"`, or a synthesized
/// `"Unknown(0xNNNN)"` for an OPC quality code the library doesn't otherwise recognize. Any
/// string other than an exact `"Good"`/`"Uncertain"` match — including that
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
        // none), per `opc-da-client`'s documented contract. There is no reliable way to
        // convert that into a trustworthy `DateTime<Utc>` without knowing the gateway
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
        opcda_bridge::SearchIndexState::Failed => SearchIndexState::Failed,
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
        configured: status.configured,
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
                && status.code() == tonic::Code::FailedPrecondition =>
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
        // `opc-da-client` synthesizes "Unknown(0xNNNN)" for quality codes it doesn't
        // otherwise recognize; an unrecognized quality must never be silently trusted.
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
                opcda_bridge::SearchIndexState::Failed,
                SearchIndexState::Failed,
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
            configured: true,
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
            opcda_bridge::Error::Rpc(tonic::Status::failed_precondition(
                "server is not configured for namespace indexing",
            )),
            "indexed-search refresh",
        );
        assert!(matches!(
            err,
            DriverError::IndexOperationRejected { message }
                if message == "server is not configured for namespace indexing"
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
}

/// End-to-end smoke tests against a minimal mock `Bridge` gRPC service. These prove the typed
/// page/session/search API is wired together correctly without re-testing the bridge's own RPC
/// implementation.
#[cfg(test)]
mod smoke_tests {
    use super::*;
    use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
    use opcda_bridge_proto::bridge::search_event;
    use opcda_bridge_proto::bridge::{
        BrowseNode as ProtoBrowseNode, BrowseNodeKind as ProtoNodeKind,
        BrowsePage as ProtoBrowsePage, BrowseRequest, BrowseSource as ProtoBrowseSource,
        CloseBrowseSessionRequest, ControlSearchIndexRequest, GetCapabilitiesRequest,
        GetCapabilitiesResponse, GetSearchIndexStatusRequest,
        IndexedSearchMatch as ProtoIndexedSearchMatch, ListServersRequest, ListServersResponse,
        NamespaceOrganization as ProtoOrganization, ReadRequest, ReadResponse,
        RefreshSearchIndexRequest, SearchEvent as ProtoSearchEvent,
        SearchIndexResponse as ProtoSearchIndexResponse, SearchIndexState as ProtoSearchIndexState,
        SearchIndexStatus as ProtoSearchIndexStatus, SearchProgress as ProtoSearchProgress,
        TagValue as ProtoTagValue, WriteRequest, WriteResponse,
    };
    use std::net::SocketAddr;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    #[derive(Default)]
    struct MockBridgeService {
        capabilities_response: GetCapabilitiesResponse,
        browse_response: ProtoBrowsePage,
        read_response: ReadResponse,
        write_response: WriteResponse,
        write_error: Option<Status>,
        list_servers_response: ListServersResponse,
        search_events: Vec<ProtoSearchEvent>,
        search_index_status_response: ProtoSearchIndexStatus,
        search_index_response: ProtoSearchIndexResponse,
        close_error: Option<Status>,
    }

    #[tonic::async_trait]
    impl Bridge for MockBridgeService {
        async fn get_capabilities(
            &self,
            _request: Request<GetCapabilitiesRequest>,
        ) -> Result<Response<GetCapabilitiesResponse>, Status> {
            Ok(Response::new(self.capabilities_response.clone()))
        }

        async fn list_servers(
            &self,
            _request: Request<ListServersRequest>,
        ) -> Result<Response<ListServersResponse>, Status> {
            Ok(Response::new(self.list_servers_response.clone()))
        }

        async fn browse(
            &self,
            _request: Request<BrowseRequest>,
        ) -> Result<Response<ProtoBrowsePage>, Status> {
            Ok(Response::new(self.browse_response.clone()))
        }

        async fn close_browse_session(
            &self,
            _request: Request<CloseBrowseSessionRequest>,
        ) -> Result<Response<()>, Status> {
            if let Some(status) = self.close_error.clone() {
                return Err(status);
            }
            Ok(Response::new(()))
        }

        async fn get_search_index_status(
            &self,
            _request: Request<GetSearchIndexStatusRequest>,
        ) -> Result<Response<ProtoSearchIndexStatus>, Status> {
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn refresh_search_index(
            &self,
            _request: Request<RefreshSearchIndexRequest>,
        ) -> Result<Response<ProtoSearchIndexStatus>, Status> {
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn control_search_index(
            &self,
            _request: Request<ControlSearchIndexRequest>,
        ) -> Result<Response<ProtoSearchIndexStatus>, Status> {
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn search_index(
            &self,
            _request: Request<opcda_bridge_proto::bridge::SearchIndexRequest>,
        ) -> Result<Response<ProtoSearchIndexResponse>, Status> {
            Ok(Response::new(self.search_index_response.clone()))
        }

        type SearchStream = ReceiverStream<Result<ProtoSearchEvent, Status>>;

        async fn search(
            &self,
            _request: Request<opcda_bridge_proto::bridge::SearchRequest>,
        ) -> Result<Response<Self::SearchStream>, Status> {
            let (tx, rx) = mpsc::channel(4);
            let events = self.search_events.clone();
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(Ok(event)).await;
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }

        async fn read(
            &self,
            _request: Request<ReadRequest>,
        ) -> Result<Response<ReadResponse>, Status> {
            Ok(Response::new(self.read_response.clone()))
        }

        async fn write(
            &self,
            _request: Request<WriteRequest>,
        ) -> Result<Response<WriteResponse>, Status> {
            if let Some(status) = self.write_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.write_response.clone()))
        }
    }

    async fn start_mock_server(service: MockBridgeService) -> String {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(
            Server::builder()
                .add_service(BridgeServer::new(service))
                .serve_with_incoming(TcpListenerStream::new(listener)),
        );
        format!("127.0.0.1:{port}")
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
        let host = start_mock_server(MockBridgeService {
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
        let host = start_mock_server(MockBridgeService {
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
        let host = start_mock_server(MockBridgeService {
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
        let host = start_mock_server(MockBridgeService {
            search_index_status_response: ProtoSearchIndexStatus {
                server: "S1".into(),
                state: ProtoSearchIndexState::Ready as i32,
                configured: true,
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
                    configured: true,
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
        let host = start_mock_server(MockBridgeService {
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
        let host = start_mock_server(MockBridgeService {
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
    async fn driver_trait_maps_close_browse_rpc_errors() {
        let host = start_mock_server(MockBridgeService {
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
        let host = start_mock_server(MockBridgeService {
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
