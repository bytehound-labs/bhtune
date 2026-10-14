use super::*;
use crate::args::SearchIndexControlActionArg;
use bhtune_test_support::{MockBridgeService, start_mock_server};
use opcda_bridge_proto::bridge::{
    BrowseNode, BrowseNodeKind, BrowsePage, GetGatewayInfoResponse, ListServersResponse,
    ProtocolFeature, ProtocolFeatureKind, ReadResponse, SearchCompleted,
    SearchEvent as ProtoSearchEvent, SearchIndexResponse as ProtoSearchIndexResponse,
    SearchIndexState as ProtoSearchIndexState, SearchIndexStatus as ProtoSearchIndexStatus,
    SearchProgress, TagValue as ProtoTagValue, WriteResponse, search_event,
};

#[tokio::test]
async fn gateway_info_reports_gateway_wide_metadata_in_both_formats() {
    let (host, server) = start_mock_server(MockBridgeService {
        gateway_info_response: GetGatewayInfoResponse {
            application_version: "0.5.9".into(),
            compatibility_schema_version: 1,
            features: vec![ProtocolFeature {
                kind: ProtocolFeatureKind::Core as i32,
                min_version: 1,
                max_version: 2,
            }],
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    gateway_info(&host).await.unwrap();
    gateway_info_with_output(&host, OutputFormat::Json)
        .await
        .unwrap();
    assert_eq!(
        gateway_info_json(&get_opcda_gateway_info(&host).await.unwrap()),
        serde_json::json!({
            "application_version": "0.5.9",
            "compatibility_schema_version": 1,
            "features": [{
                "feature": "core",
                "min_version": 1,
                "max_version": 2,
            }],
        })
    );

    server.shutdown().await;
}

#[tokio::test]
async fn gateway_info_prints_a_full_result_without_a_warning() {
    let (host, server) = start_mock_server(MockBridgeService {
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
    .await
    .expect("bind mock bridge");

    let report = check_gateway_compatibility(&host, None).await.unwrap();
    assert_eq!(report.status.as_str(), "full");
    assert!(report.warning_message().is_none());
    gateway_info(&host).await.unwrap();
    gateway_info_with_output(&host, OutputFormat::Json)
        .await
        .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn gateway_info_connect_failure_surfaces_as_an_error() {
    let err = gateway_info("127.0.0.1:1").await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn servers_prints_every_registered_server_from_a_mock_gateway() {
    let (host, server) = start_mock_server(MockBridgeService {
        list_servers_response: ListServersResponse {
            servers: vec![
                "Matrikon.OPC.Simulation.1".to_string(),
                "Kepware.KEPServerEX.V6".to_string(),
            ],
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    servers(&host).await.unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn servers_handles_an_empty_result() {
    let (host, server) = start_mock_server(MockBridgeService::default())
        .await
        .expect("bind mock bridge");
    servers(&host).await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn servers_connect_failure_surfaces_as_an_error() {
    let err = servers("127.0.0.1:1").await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn read_prints_values_from_a_mock_gateway() {
    let (host, server) = start_mock_server(MockBridgeService {
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
    .await
    .expect("bind mock bridge");

    read(&host, "Sim.Server", &["Unit1.LIC101.PV".to_string()])
        .await
        .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn read_requires_at_least_one_tag() {
    let err = read("127.0.0.1:1", "Sim.Server", &[]).await.unwrap_err();
    assert!(err.to_string().contains("at least one tag"));
}

#[tokio::test]
async fn write_reports_success_from_a_mock_gateway() {
    let service = MockBridgeService {
        write_response: WriteResponse {
            tag_id: "Unit1.LIC101.OP".to_string(),
            success: true,
            error: None,
        },
        ..Default::default()
    };
    let write_calls = std::sync::Arc::clone(&service.write_calls);
    let (host, server) = start_mock_server(service).await.expect("bind mock bridge");
    let directory = tempfile::tempdir().unwrap();
    let pool = bhtune_db::connect(&directory.path().join("opc-write-audit.db"))
        .await
        .unwrap();

    write_with_output(
        &pool,
        &host,
        "Sim.Server",
        "Unit1.LIC101.OP",
        "55.0",
        OutputFormat::Json,
    )
    .await
    .unwrap();
    let owner_id = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM live_operation_owners ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner = bhtune_db::models::LiveOwnershipRow::get(&pool, owner_id)
        .await
        .unwrap()
        .unwrap();
    let steps = bhtune_db::models::LiveMutationStepRow::list_for_owner(&pool, owner_id)
        .await
        .unwrap();

    assert_eq!(write_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(owner.state, bhtune_db::models::LiveOwnershipState::Released);
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].status,
        bhtune_db::models::MutationStepStatus::Confirmed
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&steps[0].target_json).unwrap()["tag"],
        "Unit1.LIC101.OP"
    );
    server.shutdown().await;
}

#[tokio::test]
async fn write_surfaces_a_rejected_write_as_an_error() {
    let (host, server) = start_mock_server(MockBridgeService {
        write_response: WriteResponse {
            tag_id: "Unit1.LIC101.OP".to_string(),
            success: false,
            error: Some("tag is read-only".to_string()),
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    let err = write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("read-only"));

    server.shutdown().await;
}

#[tokio::test]
async fn write_reports_a_gateway_rejection_when_it_omits_a_reason() {
    let (host, server) = start_mock_server(MockBridgeService {
        write_response: WriteResponse {
            tag_id: "Unit1.LIC101.OP".to_string(),
            success: false,
            error: None,
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    let err = write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("driver rejected the write"));

    server.shutdown().await;
}

#[test]
fn diagnostic_formatters_cover_timestamp_and_reason_fallbacks() {
    assert_eq!(format_timestamp(None), "-");
    assert_eq!(
        format_timestamp(Some(
            chrono::DateTime::parse_from_rfc3339("2024-01-15T10:23:45Z")
                .unwrap()
                .into()
        )),
        "2024-01-15T10:23:45+00:00"
    );
    assert_eq!(write_rejection_reason(None), "unknown reason");
    assert_eq!(
        write_rejection_reason(Some("read-only".to_string())),
        "read-only"
    );
}

#[test]
fn write_and_release_results_preserve_operation_and_persistence_errors() {
    let success = combine_write_and_release(
        Ok(WriteOutcome {
            success: true,
            error_message: None,
        }),
        Ok(()),
    )
    .unwrap();
    assert!(success.success);

    let operation_only =
        combine_write_and_release(Err(anyhow::anyhow!("write failed")), Ok(())).unwrap_err();
    assert!(operation_only.to_string().contains("write failed"));

    let release_only = combine_write_and_release(
        Ok(WriteOutcome {
            success: true,
            error_message: None,
        }),
        Err(anyhow::anyhow!("release failed")),
    )
    .unwrap_err();
    assert!(release_only.to_string().contains("release failed"));

    let both = combine_write_and_release(
        Err(anyhow::anyhow!("write failed")),
        Err(anyhow::anyhow!("release failed")),
    )
    .unwrap_err();
    assert!(both.to_string().contains("write failed"));
    assert!(both.to_string().contains("release failed"));
}

#[tokio::test]
async fn write_accepts_a_raw_non_numeric_value() {
    let (host, server) = start_mock_server(MockBridgeService {
        write_response: WriteResponse {
            tag_id: "Unit1.LIC101.OP".to_string(),
            success: true,
            error: None,
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    write(&host, "Sim.Server", "Unit1.LIC101.MODE", "MAN")
        .await
        .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn browse_prints_nodes_from_a_mock_gateway() {
    let (host, server) = start_mock_server(MockBridgeService {
        browse_response: BrowsePage {
            session_id: "session".to_string(),
            nodes: vec![BrowseNode {
                node_key: "unit1".to_string(),
                display_name: "Unit1".to_string(),
                kind: BrowseNodeKind::Branch as i32,
                item_id: None,
            }],
            complete: true,
            ..Default::default()
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");

    browse(&host, "Sim.Server", "").await.unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn browse_handles_an_empty_result() {
    let (host, server) = start_mock_server(MockBridgeService::default())
        .await
        .expect("bind mock bridge");
    browse(&host, "Sim.Server", "Unit1").await.unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn browse_connect_failure_surfaces_as_an_error() {
    let err = browse("127.0.0.1:1", "Sim.Server", "").await.unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn connect_failure_surfaces_as_an_error() {
    // Port 1 is a privileged/unlikely-bound port; connecting should fail promptly.
    let err = read(
        "127.0.0.1:1",
        "Sim.Server",
        &["Unit1.LIC101.PV".to_string()],
    )
    .await
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[tokio::test]
async fn run_dispatches_gateway_info_servers_read_write_and_browse() {
    let close_browse_session_calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let directory = tempfile::tempdir().unwrap();
    let pool = bhtune_db::connect(&directory.path().join("opc-dispatch-test.db"))
        .await
        .unwrap();
    let (host, server) = start_mock_server(MockBridgeService {
        list_servers_response: ListServersResponse {
            servers: vec!["Matrikon.OPC.Simulation.1".to_string()],
        },
        read_response: ReadResponse {
            values: vec![ProtoTagValue {
                tag_id: "Unit1.LIC101.PV".to_string(),
                value: "42.5".to_string(),
                quality: "Good".to_string(),
                timestamp: "2024-01-15 10:23:45".to_string(),
            }],
        },
        write_response: WriteResponse {
            tag_id: "Unit1.LIC101.OP".to_string(),
            success: true,
            error: None,
        },
        browse_response: BrowsePage {
            session_id: "session".to_string(),
            complete: true,
            ..Default::default()
        },
        close_browse_session_calls: close_browse_session_calls.clone(),
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");
    let config = crate::config::BhtuneConfig::default();

    run(
        &pool,
        OpcCommand::GatewayInfo {
            bridge_host: Some(host.clone()),
        },
        &config,
    )
    .await
    .unwrap();

    run(
        &pool,
        OpcCommand::Servers {
            bridge_host: Some(host.clone()),
        },
        &config,
    )
    .await
    .unwrap();

    run(
        &pool,
        OpcCommand::Read {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".to_string()),
            tags: vec!["Unit1.LIC101.PV".to_string()],
        },
        &config,
    )
    .await
    .unwrap();

    run(
        &pool,
        OpcCommand::Write {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".to_string()),
            tag: "Unit1.LIC101.OP".to_string(),
            value: "55.0".to_string(),
        },
        &config,
    )
    .await
    .unwrap();

    run(
        &pool,
        OpcCommand::Browse {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".to_string()),
            session_id: None,
            parent_node_key: None,
            page_token: None,
            page_size: bhtune_driver::DEFAULT_PAGE_SIZE,
            all: false,
            refresh: false,
        },
        &config,
    )
    .await
    .unwrap();
    assert_eq!(
        close_browse_session_calls.load(std::sync::atomic::Ordering::SeqCst),
        0
    );

    run(
        &pool,
        OpcCommand::Close {
            bridge_host: Some(host),
            session_id: "session".to_string(),
        },
        &config,
    )
    .await
    .unwrap();
    assert_eq!(
        close_browse_session_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    server.shutdown().await;
}

#[tokio::test]
async fn run_resolves_bridge_host_and_server_from_config_when_cli_flags_are_unset() {
    let (host, server) = start_mock_server(MockBridgeService {
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
    .await
    .expect("bind mock bridge");
    let config = crate::config::BhtuneConfig {
        bridge_host: Some(host),
        server: Some("Sim.Server".to_string()),
        ..Default::default()
    };
    let pool = bhtune_db::connect_in_memory().await.unwrap();

    run(
        &pool,
        OpcCommand::Read {
            bridge_host: None,
            server: None,
            tags: vec!["Unit1.LIC101.PV".to_string()],
        },
        &config,
    )
    .await
    .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn run_resolves_bridge_host_from_config_for_servers_when_cli_flag_is_unset() {
    let (host, server) = start_mock_server(MockBridgeService::default())
        .await
        .expect("bind mock bridge");
    let config = crate::config::BhtuneConfig {
        bridge_host: Some(host),
        ..Default::default()
    };
    let pool = bhtune_db::connect_in_memory().await.unwrap();

    run(&pool, OpcCommand::Servers { bridge_host: None }, &config)
        .await
        .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn run_errors_when_server_is_unset_in_both_cli_and_config() {
    let pool = bhtune_db::connect_in_memory().await.unwrap();
    let err = run(
        &pool,
        OpcCommand::Read {
            bridge_host: None,
            server: None,
            tags: vec!["Unit1.LIC101.PV".to_string()],
        },
        &crate::config::BhtuneConfig::default(),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no OPC server specified"));
}

#[tokio::test]
async fn search_and_index_commands_cover_table_json_and_control_paths() {
    let status = ProtoSearchIndexStatus {
        server: "Sim.Server".into(),
        state: ProtoSearchIndexState::Partial as i32,
        configured: true,
        active_generation: 2,
        entry_count: 3,
        unique_item_count: 2,
        started_at: Some("2026-01-01T00:00:00Z".into()),
        completed_at: None,
        last_error: Some("inventory incomplete".into()),
        database_bytes: 42,
        progress: Some(opcda_bridge_proto::bridge::IndexedSearchProgress {
            branches_visited: 1,
            entries_seen: 3,
            unique_items: 2,
            active_time_ms: 10,
            paused_time_ms: 2,
            items_per_second: 1.5,
            estimated_remaining_ms: Some(5),
        }),
        ..Default::default()
    };
    let (host, server) = start_mock_server(MockBridgeService {
        search_events: vec![
            ProtoSearchEvent {
                event: Some(search_event::Event::Progress(SearchProgress {
                    visited_nodes: 4,
                    matches: 1,
                    partial: true,
                })),
            },
            ProtoSearchEvent {
                event: Some(search_event::Event::Match(
                    opcda_bridge_proto::bridge::SearchMatch {
                        node: Some(BrowseNode {
                            node_key: "pv".into(),
                            display_name: "PV".into(),
                            kind: BrowseNodeKind::Item as i32,
                            item_id: Some("Area.PV".into()),
                        }),
                        breadcrumbs: vec![opcda_bridge_proto::bridge::BrowseBreadcrumb {
                            node_key: "area".into(),
                            display_name: "Area".into(),
                        }],
                    },
                )),
            },
            ProtoSearchEvent {
                event: Some(search_event::Event::Completed(SearchCompleted {
                    complete: false,
                    cancelled: false,
                    truncated: true,
                    warning: Some("partial result".into()),
                })),
            },
        ],
        search_index_status_response: status.clone(),
        search_index_response: ProtoSearchIndexResponse {
            matches: vec![],
            has_more: true,
            status: Some(status),
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");
    let config = crate::config::BhtuneConfig::default();
    let pool = bhtune_db::connect_in_memory().await.unwrap();

    run_with_output(
        &pool,
        OpcCommand::Search {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".into()),
            query: "PV".into(),
            match_mode: OpcSearchMatchModeArg::Contains,
            max_results: 10,
            session_id: Some("session".into()),
            scope_node_key: Some("area".into()),
            include_branches: true,
            refresh: true,
        },
        &config,
        OutputFormat::Table,
    )
    .await
    .unwrap();
    run_with_output(
        &pool,
        OpcCommand::Search {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".into()),
            query: "PV".into(),
            match_mode: OpcSearchMatchModeArg::Exact,
            max_results: 10,
            session_id: None,
            scope_node_key: None,
            include_branches: false,
            refresh: false,
        },
        &config,
        OutputFormat::Json,
    )
    .await
    .unwrap();

    for command in [
        SearchIndexCommand::Status {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".into()),
        },
        SearchIndexCommand::Refresh {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".into()),
            force: true,
        },
        SearchIndexCommand::Control {
            bridge_host: Some(host.clone()),
            server: Some("Sim.Server".into()),
            action: SearchIndexControlActionArg::Pause,
        },
    ] {
        run_with_output(
            &pool,
            OpcCommand::SearchIndex { command },
            &config,
            OutputFormat::Table,
        )
        .await
        .unwrap();
    }
    run_with_output(
        &pool,
        OpcCommand::SearchIndex {
            command: SearchIndexCommand::Search {
                bridge_host: Some(host.clone()),
                server: Some("Sim.Server".into()),
                query: "PV".into(),
                match_mode: OpcSearchMatchModeArg::Prefix,
                max_results: 5,
            },
        },
        &config,
        OutputFormat::Json,
    )
    .await
    .unwrap();

    server.shutdown().await;
}

#[tokio::test]
async fn diagnostic_commands_cover_json_and_validation_branches() {
    let status = bhtune_driver::SearchIndexStatus {
        server: "Sim.Server".into(),
        state: bhtune_driver::SearchIndexState::Failed,
        auto_refresh_enabled: true,
        active_generation: 1,
        entry_count: 2,
        unique_item_count: 1,
        started_at: None,
        completed_at: None,
        last_error: Some("failed".into()),
        database_bytes: 10,
        organization: bhtune_driver::NamespaceOrganization::Flat,
        source: bhtune_driver::BrowseSource::Flat,
        progress: Some(bhtune_driver::IndexedSearchProgress {
            branches_visited: 1,
            entries_seen: 2,
            unique_items: 1,
            active_time_ms: 1,
            paused_time_ms: 0,
            items_per_second: 2.0,
            estimated_remaining_ms: None,
        }),
        scheduler: bhtune_driver::IndexSchedulerDiagnostics::default(),
    };
    let indexed = bhtune_driver::SearchIndexResponse {
        matches: vec![bhtune_driver::IndexedSearchMatch {
            item_id: "Area.PV".into(),
            display_name: "PV".into(),
            kind: bhtune_driver::BrowseNodeKind::BranchAndItem,
            breadcrumbs: vec!["Area".into()],
        }],
        has_more: true,
        status: status.clone(),
    };
    assert_eq!(quality_name(Quality::Good), "good");
    assert_eq!(quality_name(Quality::Uncertain), "uncertain");
    assert_eq!(quality_name(Quality::Bad), "bad");
    print_search_index_status(&status, OutputFormat::Table).unwrap();
    print_search_index_status(&status, OutputFormat::Json).unwrap();
    print_search_index_results(&indexed, OutputFormat::Table).unwrap();
    print_search_index_results(&indexed, OutputFormat::Json).unwrap();
    print_search_index_results(
        &bhtune_driver::SearchIndexResponse {
            matches: vec![],
            has_more: false,
            status,
        },
        OutputFormat::Table,
    )
    .unwrap();

    let host_service = MockBridgeService {
        read_response: ReadResponse {
            values: vec![ProtoTagValue {
                tag_id: "Area.PV".into(),
                value: "1.5".into(),
                quality: "Uncertain".into(),
                timestamp: "N/A".into(),
            }],
        },
        write_response: WriteResponse {
            tag_id: "Area.MV".into(),
            success: false,
            error: None,
        },
        browse_response: BrowsePage {
            session_id: "s".into(),
            nodes: vec![
                BrowseNode {
                    node_key: "u".into(),
                    display_name: "Unspecified".into(),
                    kind: BrowseNodeKind::Unspecified as i32,
                    item_id: None,
                },
                BrowseNode {
                    node_key: "i".into(),
                    display_name: "Item".into(),
                    kind: BrowseNodeKind::Item as i32,
                    item_id: Some("Area.PV".into()),
                },
                BrowseNode {
                    node_key: "b".into(),
                    display_name: "Branch".into(),
                    kind: BrowseNodeKind::Branch as i32,
                    item_id: None,
                },
                BrowseNode {
                    node_key: "both".into(),
                    display_name: "Both".into(),
                    kind: BrowseNodeKind::BranchAndItem as i32,
                    item_id: Some("Area.Both".into()),
                },
            ],
            next_page_token: Some("next".into()),
            complete: false,
            ..Default::default()
        },
        browse_continuation_response: Some(BrowsePage {
            session_id: "s".into(),
            complete: true,
            ..Default::default()
        }),
        list_servers_response: ListServersResponse {
            servers: vec!["Sim.Server".into()],
        },
        ..Default::default()
    };
    let (host, server) = start_mock_server(host_service)
        .await
        .expect("bind mock bridge");
    let directory = tempfile::tempdir().unwrap();
    let pool = bhtune_db::connect(&directory.path().join("opc-diagnostic-test.db"))
        .await
        .unwrap();
    servers_with_output(&host, OutputFormat::Json)
        .await
        .unwrap();
    read_with_output(&host, "Sim.Server", &["Area.PV".into()], OutputFormat::Json)
        .await
        .unwrap();
    write_with_output(
        &pool,
        &host,
        "Sim.Server",
        "Area.MV",
        "1.0",
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    let err = write_with_output(
        &pool,
        &host,
        "Sim.Server",
        "Area.MV",
        "not-numeric",
        OutputFormat::Json,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("driver rejected"));
    browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            page_size: 2,
            all: true,
            ..Default::default()
        },
        OutputFormat::Json,
    )
    .await
    .unwrap();
    browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            page_size: 2,
            all: false,
            ..Default::default()
        },
        OutputFormat::Json,
    )
    .await
    .unwrap();
    browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            page_size: 2,
            all: true,
            ..Default::default()
        },
        OutputFormat::Table,
    )
    .await
    .unwrap();
    browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            page_size: 2,
            all: false,
            ..Default::default()
        },
        OutputFormat::Table,
    )
    .await
    .unwrap();
    let err = browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            parent_node_key: Some("parent".into()),
            ..Default::default()
        },
        OutputFormat::Table,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("--session-id"));
    let err = close_with_output(&host, " ", OutputFormat::Table)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("session ID"));
    close_with_output(&host, "s", OutputFormat::Json)
        .await
        .unwrap();
    server.shutdown().await;

    let (host, server) = start_mock_server(MockBridgeService {
        write_response: WriteResponse {
            tag_id: "Area.MV".into(),
            success: true,
            error: None,
        },
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");
    write_with_output(
        &pool,
        &host,
        "Sim.Server",
        "Area.MV",
        "1.0",
        OutputFormat::Json,
    )
    .await
    .unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn table_search_reports_empty_results_and_completion_diagnostics() {
    let (host, server) = start_mock_server(MockBridgeService {
        search_events: vec![ProtoSearchEvent {
            event: Some(search_event::Event::Completed(SearchCompleted {
                complete: false,
                cancelled: false,
                truncated: true,
                warning: Some("partial".into()),
            })),
        }],
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");
    search_with_output(
        &host,
        "Sim.Server",
        SearchOptions {
            query: "PV".into(),
            match_mode: OpcSearchMatchModeArg::Contains,
            max_results: 1,
            session_id: None,
            scope_node_key: None,
            include_branches: false,
            refresh: false,
        },
        OutputFormat::Table,
    )
    .await
    .unwrap();
    server.shutdown().await;
}

#[tokio::test]
async fn browse_all_stops_at_the_safety_page_limit() {
    let (host, server) = start_mock_server(MockBridgeService {
        browse_response: BrowsePage {
            session_id: "s".into(),
            next_page_token: Some("next".into()),
            complete: false,
            ..Default::default()
        },
        browse_continuation_response: Some(BrowsePage {
            session_id: "s".into(),
            next_page_token: Some("next".into()),
            complete: false,
            ..Default::default()
        }),
        ..Default::default()
    })
    .await
    .expect("bind mock bridge");
    let err = browse_with_output(
        &host,
        "Sim.Server",
        BrowseOptions {
            all: true,
            ..Default::default()
        },
        OutputFormat::Table,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("safety limit"));
    server.shutdown().await;
}
