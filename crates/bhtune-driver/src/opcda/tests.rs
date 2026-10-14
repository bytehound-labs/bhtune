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
        auto_refresh_enabled: true,
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
