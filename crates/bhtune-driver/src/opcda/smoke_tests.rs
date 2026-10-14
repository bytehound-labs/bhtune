use super::*;
use bhtune_test_support::MockBridgeService;
use opcda_bridge_proto::bridge::search_event;
use opcda_bridge_proto::bridge::{
    BrowseNode as ProtoBrowseNode, BrowseNodeKind as ProtoNodeKind, BrowsePage as ProtoBrowsePage,
    BrowseSource as ProtoBrowseSource, GetCapabilitiesResponse, GetGatewayInfoResponse,
    IndexedSearchMatch as ProtoIndexedSearchMatch, ListServersResponse,
    NamespaceOrganization as ProtoOrganization, ProtocolFeature, ProtocolFeatureKind, ReadResponse,
    SearchEvent as ProtoSearchEvent, SearchIndexResponse as ProtoSearchIndexResponse,
    SearchIndexState as ProtoSearchIndexState, SearchIndexStatus as ProtoSearchIndexStatus,
    SearchProgress as ProtoSearchProgress, TagValue as ProtoTagValue, WriteResponse,
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
    driver.set_search_index_auto_refresh(false).await.unwrap();
    driver.set_search_index_auto_refresh(true).await.unwrap();
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
        <OpcDaDriver as Driver>::set_search_index_auto_refresh(&driver, false)
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
            protocol_feature(ProtocolFeatureKind::IndexedSearch, 2, 2),
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
