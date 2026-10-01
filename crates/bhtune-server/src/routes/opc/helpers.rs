use super::*;

pub(super) async fn inspect_gateway_compatibility(
    bridge_host: &str,
    server: Option<&str>,
) -> OpcDaGatewayCompatibility {
    match timed_driver_call(
        "check OPC DA gateway compatibility",
        Duration::from_secs(OPC_QUERY_TIMEOUT_SECS),
        check_gateway_compatibility(bridge_host, server),
    )
    .await
    {
        TimedDriverCall::Ready(report) => report,
        TimedDriverCall::Incompatible { .. } | TimedDriverCall::Failed(_) => {
            OpcDaGatewayCompatibility::unverified()
        }
    }
}

pub(super) fn degraded_warning(
    report: &OpcDaGatewayCompatibility,
    operation: &'static str,
) -> String {
    report
        .live_mutation_refusal()
        .or_else(|| report.warning_message())
        .unwrap_or_else(|| {
            bhtune_driver::DriverError::IncompatibleGateway { operation }.to_string()
        })
}

pub(super) fn degraded_capabilities(report: &OpcDaGatewayCompatibility) -> OpcCapabilitiesResponse {
    OpcCapabilitiesResponse {
        application_version: String::new(),
        protocol_version: String::new(),
        max_page_size: 0,
        supports_browse_sessions: false,
        supports_search: false,
        organization: "unspecified".to_string(),
        source: "unspecified".to_string(),
        supports_indexed_search: false,
        indexed_search_protocol_version: String::new(),
        max_indexed_search_results: 0,
        search_index_state: "unspecified".to_string(),
        gateway_compatibility: Some(report.into()),
    }
}

pub(super) fn degraded_browse(
    report: &OpcDaGatewayCompatibility,
    operation: &'static str,
) -> OpcBrowseResponse {
    OpcBrowseResponse {
        session_id: String::new(),
        nodes: Vec::new(),
        next_page_token: None,
        complete: true,
        organization: "unspecified".to_string(),
        source: "unspecified".to_string(),
        warning: Some(degraded_warning(report, operation)),
        gateway_compatibility: Some(report.into()),
    }
}

pub(super) async fn connect_search_index_driver(
    state: &AppState,
    bridge_host: Option<String>,
    opc_server: Option<String>,
) -> Result<OpcDaDriver, ApiError> {
    let config = state.config_snapshot()?;
    let bridge_host = bhtune_runtime::config::resolve_bridge_host(bridge_host, &config);
    let opc_server = bhtune_runtime::config::resolve_server(opc_server, &config)
        .map_err(|err| ApiError::BadRequest(err.to_string()))?;
    with_timeout(
        &format!("connect to OPC server '{opc_server}' via bridge '{bridge_host}'"),
        OpcDaDriver::connect(&bridge_host, opc_server),
    )
    .await
}

pub(super) fn organization_name(value: NamespaceOrganization) -> &'static str {
    match value {
        NamespaceOrganization::Unspecified => "unspecified",
        NamespaceOrganization::Flat => "flat",
        NamespaceOrganization::Hierarchical => "hierarchical",
    }
}

pub(super) fn source_name(value: BrowseSource) -> &'static str {
    match value {
        BrowseSource::Unspecified => "unspecified",
        BrowseSource::Da3 => "da3",
        BrowseSource::Da2 => "da2",
        BrowseSource::Flat => "flat",
        BrowseSource::Derived => "derived",
    }
}

pub(super) fn search_event_to_sse(event: SearchEvent) -> Event {
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

pub(super) fn json_search_match(found: &SearchMatch) -> serde_json::Value {
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

pub(super) fn browse_node_kind_name(kind: BrowseNodeKind) -> &'static str {
    match kind {
        BrowseNodeKind::Unspecified => "unspecified",
        BrowseNodeKind::Branch => "branch",
        BrowseNodeKind::Item => "item",
        BrowseNodeKind::BranchAndItem => "branch_and_item",
    }
}
