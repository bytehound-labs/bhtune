use super::super::helpers::{
    browse_node_kind_name, degraded_warning, json_search_match, organization_name, source_name,
};
use super::super::{
    GatewayFeatureCompatibilityResponse, GatewayProtocolRangeResponse, OpcBrowseNodeKind,
};
use super::*;
use crate::test_support::mock_bridge::{MockBridgeService, start_mock_server};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use bhtune_driver::{
    BrowseNode, BrowseNodeKind, BrowseSource, DriverResult, NamespaceOrganization, SearchEvent,
    SearchMatch,
};
use opcda_bridge_proto::bridge::{
    BrowseNode as ProtoBrowseNode, BrowseNodeKind as ProtoBrowseNodeKind, BrowsePage,
    BrowseSource as ProtoBrowseSource, IndexSchedulerDiagnostics as ProtoIndexSchedulerDiagnostics,
    IndexedSearchMatch as ProtoIndexedSearchMatch,
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

async fn delete_request(app: axum::Router, path: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::delete(path).body(Body::empty()).unwrap())
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
        effective_limits: None,
        controller_state: 0,
        pause_reason: None,
        recovery_deadline: None,
        foreground: Default::default(),
        host: Default::default(),
        storage: Default::default(),
        scheduler: Some(ProtoIndexSchedulerDiagnostics {
            next_refresh_at: Some("2026-08-23T10:05:00Z".to_string()),
            last_attempt_at: Some("2026-08-16T10:05:00Z".to_string()),
            last_success_at: Some("2026-08-16T10:05:00Z".to_string()),
            last_success_duration_ms: Some(300_000),
            retry_after: None,
            consecutive_failures: 0,
            circuit_open: false,
        }),
        health: Default::default(),
        promoting: false,
        pause_reason_detail: None,
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

fn expect_bad_request(error: ApiError) -> String {
    match error {
        ApiError::BadRequest(message) => message,
        other => panic!("expected BadRequest, got {other:?}"),
    }
}

#[test]
fn opc_conversion_helpers_cover_all_enum_variants_and_sse_events() {
    for (wire, expected) in [
        (BrowseNodeKind::Unspecified, "unspecified"),
        (BrowseNodeKind::Branch, "branch"),
        (BrowseNodeKind::Item, "item"),
        (BrowseNodeKind::BranchAndItem, "branch_and_item"),
    ] {
        assert_eq!(browse_node_kind_name(wire), expected);
        let _ = OpcBrowseNodeKind::from(wire);
    }
    for (value, expected) in [
        (NamespaceOrganization::Unspecified, "unspecified"),
        (NamespaceOrganization::Flat, "flat"),
        (NamespaceOrganization::Hierarchical, "hierarchical"),
    ] {
        assert_eq!(organization_name(value), expected);
    }
    for (value, expected) in [
        (BrowseSource::Unspecified, "unspecified"),
        (BrowseSource::Da3, "da3"),
        (BrowseSource::Da2, "da2"),
        (BrowseSource::Flat, "flat"),
        (BrowseSource::Derived, "derived"),
    ] {
        assert_eq!(source_name(value), expected);
    }
    for value in ["exact", "prefix", "contains"] {
        assert!(parse_search_match_mode(value).is_ok());
    }
    let events = [
        SearchEvent::Match(SearchMatch {
            node: BrowseNode {
                node_key: "n".into(),
                display_name: "PV".into(),
                kind: BrowseNodeKind::Item,
                item_id: Some("Area.PV".into()),
            },
            breadcrumbs: vec![],
        }),
        SearchEvent::Progress(bhtune_driver::SearchProgress {
            visited_nodes: 2,
            matches: 1,
            partial: true,
        }),
        SearchEvent::Completed(bhtune_driver::SearchCompleted {
            complete: true,
            cancelled: false,
            truncated: false,
            warning: None,
        }),
    ];
    for event in events {
        let rendered = search_event_to_sse(event);
        assert!(!format!("{rendered:?}").is_empty());
    }
    let json = json_search_match(&SearchMatch {
        node: BrowseNode {
            node_key: "n".into(),
            display_name: "PV".into(),
            kind: BrowseNodeKind::BranchAndItem,
            item_id: Some("Area.PV".into()),
        },
        breadcrumbs: vec![bhtune_driver::BrowseBreadcrumb {
            node_key: "area".into(),
            display_name: "Area".into(),
        }],
    });
    assert_eq!(json["breadcrumbs"][0]["display_name"], "Area");
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let (host, _host_server) = start_mock_server(MockBridgeService::default()).await;
    let app = crate::build_router(state_with(Some(&host), None).await);

    let response = get(app, "/api/opc/servers").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["servers"], serde_json::json!([]));
    assert_eq!(body["gateway_compatibility"]["status"], "unknown");
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
    let (host, _host_server) = start_mock_server(MockBridgeService::default()).await;
    // Config points at an unreachable host; the query param must win.
    let app = crate::build_router(state_with(Some("127.0.0.1:1"), None).await);

    let response = get(app, &format!("/api/opc/servers?bridge_host={host}")).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn capabilities_reports_the_indexed_search_contract() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
            search_index_promoting: false,
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
        let (host, _host_server) = start_mock_server(MockBridgeService {
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
        assert_eq!(body["auto_refresh_enabled"], true);
        assert_eq!(body["active_generation"], 7);
        assert_eq!(body["entry_count"], 12_345);
        assert_eq!(body["unique_item_count"], 9_876);
        assert_eq!(body["started_at"], "2026-08-16T10:00:00Z");
        assert_eq!(body["completed_at"], "2026-08-16T10:05:00Z");
        assert_eq!(body["database_bytes"], 65_536);
        assert_eq!(body["organization"], "hierarchical");
        assert_eq!(body["source"], "da3");
        assert_eq!(body["scheduler"]["next_refresh_at"], "2026-08-23T10:05:00Z");
        assert_eq!(body["scheduler"]["last_attempt_at"], "2026-08-16T10:05:00Z");
        assert_eq!(body["scheduler"]["last_success_at"], "2026-08-16T10:05:00Z");
        assert_eq!(body["scheduler"]["last_success_duration_ms"], 300_000);
        assert_eq!(body["scheduler"]["consecutive_failures"], 0);
        assert_eq!(body["scheduler"]["circuit_open"], false);
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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

    let auto_refresh_app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
    let response = post(
        auto_refresh_app,
        "/api/opc/search-index/auto-refresh?enabled=false",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["state"], "refreshing");

    let delete_app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
    let response = delete_request(delete_app, "/api/opc/search-index?opc_server=Sim.Server").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["state"], "refreshing");

    assert_eq!(
        control_requests.lock().unwrap().as_slice(),
        &[
            opcda_bridge_proto::bridge::ControlSearchIndexRequest {
                server: "Sim.Server".to_string(),
                action: opcda_bridge_proto::bridge::SearchIndexControlAction::Resume as i32,
            },
            opcda_bridge_proto::bridge::ControlSearchIndexRequest {
                server: "Sim.Server".to_string(),
                action: opcda_bridge_proto::bridge::SearchIndexControlAction::DisableAutoRefresh
                    as i32,
            },
            opcda_bridge_proto::bridge::ControlSearchIndexRequest {
                server: "Sim.Server".to_string(),
                action: opcda_bridge_proto::bridge::SearchIndexControlAction::Delete as i32,
            },
        ]
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
        (
            "/api/opc/search-index/auto-refresh?enabled=false&opc_server=Sim.Server",
            "set OPC namespace index auto-refresh",
            true,
        ),
        (
            "/api/opc/search-index?opc_server=Sim.Server",
            "delete the OPC namespace index",
            false,
        ),
    ];

    for (path, operation, is_post) in paths {
        let app = crate::build_router(state_with(Some(&host), None).await);
        let response = if path.starts_with("/api/opc/search-index?") {
            delete_request(app, path).await
        } else if is_post {
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let (host, _host_server) = start_mock_server(MockBridgeService::default()).await;
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
async fn close_browse_session_forwards_the_opaque_session_id() {
    let (host, _host_server) = start_mock_server(MockBridgeService::default()).await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
    let response = get(
        app,
        "/api/opc/browse/sessions/session-42?opc_server=Sim.Server",
    )
    .await;
    // A GET is intentionally rejected by the route method; exercise the real DELETE below.
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
    let response = app
        .oneshot(
            Request::delete("/api/opc/browse/sessions/session-42?opc_server=Sim.Server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["closed"], true);
}

#[tokio::test]
async fn search_stream_returns_match_progress_and_completed_events() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        search_events: vec![
            opcda_bridge_proto::bridge::SearchEvent {
                event: Some(opcda_bridge_proto::bridge::search_event::Event::Match(
                    opcda_bridge_proto::bridge::SearchMatch {
                        node: Some(ProtoBrowseNode {
                            node_key: "pv".into(),
                            display_name: "PV".into(),
                            kind: ProtoBrowseNodeKind::Item as i32,
                            item_id: Some("Area.PV".into()),
                        }),
                        breadcrumbs: vec![opcda_bridge_proto::bridge::BrowseBreadcrumb {
                            node_key: "area".into(),
                            display_name: "Area".into(),
                        }],
                    },
                )),
            },
            opcda_bridge_proto::bridge::SearchEvent {
                event: Some(opcda_bridge_proto::bridge::search_event::Event::Progress(
                    opcda_bridge_proto::bridge::SearchProgress {
                        visited_nodes: 3,
                        matches: 1,
                        partial: false,
                    },
                )),
            },
            opcda_bridge_proto::bridge::SearchEvent {
                event: Some(opcda_bridge_proto::bridge::search_event::Event::Completed(
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
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);
    let response = get(
        app,
        "/api/opc/search?opc_server=Sim.Server&query=PV&match_mode=exact&max_results=4",
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("event: match"));
    assert!(text.contains("event: progress"));
    assert!(text.contains("event: completed"));
    assert!(text.contains("Area.PV"));
}

#[tokio::test]
async fn read_returns_the_value_quality_and_timestamp_from_a_mock_gateway() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
async fn close_browse_session_validates_empty_ids_before_connecting() {
    let app = crate::build_router(state_with(None, Some("Sim.Server")).await);
    let response = app
        .oneshot(
            Request::delete("/api/opc/browse/sessions/%20?opc_server=Sim.Server")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("session ID")
    );
}

#[tokio::test]
async fn search_validates_empty_query_before_connecting() {
    let app = crate::build_router(state_with(None, Some("Sim.Server")).await);
    let response = get(app, "/api/opc/search?query=%20").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("search query")
    );
}

#[tokio::test]
async fn read_returns_400_when_the_connected_gateway_rejects_the_read() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        read_error: Some(tonic::Status::unavailable("read unavailable")),
        ..Default::default()
    })
    .await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

    let response = get(app, "/api/opc/read?tag=Unit1.LIC101.PV").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        body_json(response).await["error"]
            .as_str()
            .unwrap()
            .contains("read 'Unit1.LIC101.PV'")
    );
}

#[tokio::test]
async fn read_surfaces_uncertain_and_bad_quality_without_failing() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
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
    let message = expect_bad_request(err);
    assert!(message.contains("read 'X'"));
    assert!(message.contains("no response within 30s"));
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
    let message = expect_bad_request(err);
    assert!(message.contains("read 'X'"));
    assert!(message.contains("not supported"));
}

#[test]
fn bad_request_assertion_fails_clearly_for_another_api_error() {
    let panic =
        std::panic::catch_unwind(|| expect_bad_request(ApiError::NotFound("missing".to_string())))
            .unwrap_err();
    assert!(
        panic
            .downcast_ref::<String>()
            .is_some_and(|message| message.contains("BadRequest"))
    );
}

#[tokio::test]
async fn with_timeout_preserves_index_enrollment_diagnostic() {
    let err = with_timeout("refresh the OPC namespace index", async {
        Err::<(), _>(bhtune_driver::DriverError::IndexOperationRejected {
            message: "server is not enrolled for namespace indexing".to_string(),
        })
    })
    .await
    .unwrap_err();
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("refresh the OPC namespace index"));
            assert!(message.contains("server is not enrolled for namespace indexing"));
        }
        other => panic!("expected BadRequest, got {other:?}"),
    }
}

#[tokio::test]
async fn with_timeout_maps_an_incompatible_gateway_without_waiting() {
    let err = with_timeout(
        "discover OPC browse capabilities",
        std::future::ready(Err::<(), _>(
            bhtune_driver::DriverError::IncompatibleGateway {
                operation: "capability discovery",
            },
        )),
    )
    .await
    .unwrap_err();
    let message = expect_bad_request(err);
    assert!(message.contains("capability discovery"), "{message}");
    assert!(
        message.contains("upgrade the OPC DA bridge gateway"),
        "{message}"
    );
}

#[tokio::test]
async fn inspect_falls_back_to_unknown_when_the_gateway_is_unreachable() {
    let report = inspect_gateway_compatibility("127.0.0.1:1", None).await;
    assert_eq!(
        report.status,
        bhtune_driver::OpcDaCompatibilityStatus::Unknown
    );
    assert!(
        report
            .warning_message()
            .is_some_and(|warning| warning.contains("did not report enough"))
    );
}

#[test]
fn degraded_warning_covers_refusal_warning_and_display_fallback() {
    let full = sample_report(bhtune_driver::OpcDaCompatibilityStatus::Full);
    let partial = sample_report(bhtune_driver::OpcDaCompatibilityStatus::Partial);
    let unknown = sample_report(bhtune_driver::OpcDaCompatibilityStatus::Unknown);
    let incompatible = sample_report(bhtune_driver::OpcDaCompatibilityStatus::Incompatible);

    let fallback = degraded_warning(&full, "paged browse");
    assert!(fallback.contains("paged browse"), "{fallback}");
    assert!(
        fallback.contains("upgrade the OPC DA bridge gateway"),
        "{fallback}"
    );
    assert!(degraded_warning(&partial, "paged browse").contains("partially compatible"));
    assert!(degraded_warning(&unknown, "paged browse").contains("did not report enough"));
    assert!(
        degraded_warning(&incompatible, "paged browse").contains("is incompatible with bhtune")
    );
}

#[test]
fn feature_compatibility_response_copies_reported_gateway_versions() {
    use bhtune_driver::{
        OpcDaFeatureCompatibility, OpcDaFeatureCompatibilityStatus, OpcDaGatewayFeature,
        OpcDaProtocolRange,
    };

    let reported = OpcDaFeatureCompatibility {
        feature: OpcDaGatewayFeature::Namespace,
        status: OpcDaFeatureCompatibilityStatus::Compatible,
        client_versions: OpcDaProtocolRange { min: 2, max: 2 },
        gateway_versions: Some(OpcDaProtocolRange { min: 2, max: 3 }),
        negotiated_version: Some(2),
        reason: "namespace overlap".to_string(),
    };
    let response = GatewayFeatureCompatibilityResponse::from(&reported);
    assert_eq!(response.feature, "namespace");
    assert_eq!(response.status, "compatible");
    assert_eq!(response.client_versions.min, 2);
    assert_eq!(response.client_versions.max, 2);
    assert_eq!(
        response.gateway_versions,
        Some(GatewayProtocolRangeResponse { min: 2, max: 3 })
    );
    assert_eq!(response.negotiated_version, Some(2));
    assert_eq!(response.reason, "namespace overlap");

    let unreported = OpcDaFeatureCompatibility {
        gateway_versions: None,
        negotiated_version: None,
        status: OpcDaFeatureCompatibilityStatus::Unknown,
        reason: "not reported".to_string(),
        ..reported
    };
    let missing = GatewayFeatureCompatibilityResponse::from(&unreported);
    assert_eq!(missing.gateway_versions, None);
    assert_eq!(missing.negotiated_version, None);
}

fn sample_report(
    status: bhtune_driver::OpcDaCompatibilityStatus,
) -> bhtune_driver::OpcDaGatewayCompatibility {
    bhtune_driver::OpcDaGatewayCompatibility {
        client_version: "0.1.0".to_string(),
        gateway_version: Some("0.5.0".to_string()),
        source: bhtune_driver::OpcDaCompatibilitySource::GatewayInfo,
        status,
        features: Vec::new(),
    }
}

#[tokio::test]
async fn capabilities_degrades_when_the_gateway_predates_capability_discovery() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        capabilities_error: Some(tonic::Status::unimplemented("capability discovery")),
        ..Default::default()
    })
    .await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

    let response = get(app, "/api/opc/capabilities").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["application_version"], "");
    assert_eq!(body["max_page_size"], 0);
    assert_eq!(body["supports_browse_sessions"], false);
    assert_eq!(body["organization"], "unspecified");
    assert_eq!(body["gateway_compatibility"]["status"], "unknown");
}

#[tokio::test]
async fn capabilities_returns_400_when_capability_discovery_fails() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        capabilities_error: Some(tonic::Status::internal("boom")),
        ..Default::default()
    })
    .await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

    let response = get(app, "/api/opc/capabilities").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = body_json(response).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("discover OPC browse capabilities"),
        "{message}"
    );
}

#[tokio::test]
async fn browse_degrades_when_the_gateway_predates_paged_browse() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        browse_error: Some(tonic::Status::unimplemented("paged browse")),
        ..Default::default()
    })
    .await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

    let response = get(app, "/api/opc/browse").await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["nodes"], serde_json::json!([]));
    assert_eq!(body["complete"], true);
    assert_eq!(body["organization"], "unspecified");
    assert_eq!(body["gateway_compatibility"]["status"], "unknown");
    let warning = body["warning"].as_str().unwrap();
    assert!(warning.contains("did not report enough"), "{warning}");
}

#[tokio::test]
async fn browse_returns_400_when_paged_browse_fails() {
    let (host, _host_server) = start_mock_server(MockBridgeService {
        browse_error: Some(tonic::Status::internal("boom")),
        ..Default::default()
    })
    .await;
    let app = crate::build_router(state_with(Some(&host), Some("Sim.Server")).await);

    let response = get(app, "/api/opc/browse").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let message = body_json(response).await["error"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(message.contains("browse OPC DA namespace"), "{message}");
}
