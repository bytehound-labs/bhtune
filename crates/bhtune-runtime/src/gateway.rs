//! Shared opcda-bridge compatibility checks for live mutations and inspection surfaces.
//!
//! Live mutations refuse an incompatible core. Read-only discovery, browse, and read surfaces
//! must not call [`require_live_gateway_compatible`]: they degrade instead of failing.

use anyhow::Context as _;
use bhtune_driver::{OpcDaGatewayCompatibility, check_gateway_compatibility};

/// Checks the gateway and refuses only an incompatible core protocol range.
///
/// `Partial` and `Unknown` results are logged and returned so the caller can proceed and
/// persist the snapshot. A transport or RPC failure is an error: a live mutation must not
/// start when compatibility cannot be classified.
pub async fn require_live_gateway_compatible(
    bridge_host: &str,
    server: Option<&str>,
) -> anyhow::Result<OpcDaGatewayCompatibility> {
    let report = check_gateway_compatibility(bridge_host, server)
        .await
        .with_context(|| format!("failed to check opcda-bridge compatibility at {bridge_host}"))?;
    if let Some(refusal) = report.live_mutation_refusal() {
        anyhow::bail!(refusal);
    }
    if let Some(warning) = report.warning_message() {
        tracing::warn!("{warning}");
    }
    Ok(report)
}

/// Serializes a compatibility snapshot for `tune_runs.gateway_compatibility_json`.
#[must_use]
pub fn compatibility_json(report: &OpcDaGatewayCompatibility) -> String {
    serde_json::to_string(report).unwrap_or_default()
}

/// Parses a stored snapshot, returning `None` when the historical JSON cannot be read.
#[must_use]
pub fn parse_stored_compatibility(json: &str) -> Option<OpcDaGatewayCompatibility> {
    match serde_json::from_str(json) {
        Ok(report) => Some(report),
        Err(error) => {
            tracing::warn!("stored gateway compatibility snapshot could not be parsed: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use opcda_bridge_proto::bridge::{
        GetGatewayInfoResponse, ProtocolFeature, ProtocolFeatureKind,
    };

    use super::{compatibility_json, parse_stored_compatibility, require_live_gateway_compatible};
    use crate::test_support::{MockBridgeService, start_mock_server};
    use bhtune_driver::OpcDaCompatibilityStatus;

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

    fn gateway_info(features: Vec<ProtocolFeature>) -> GetGatewayInfoResponse {
        GetGatewayInfoResponse {
            application_version: "0.6.0".into(),
            compatibility_schema_version: 1,
            features,
        }
    }

    #[tokio::test]
    async fn require_live_gateway_compatible_refuses_only_an_incompatible_core() {
        let incompatible = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info(vec![protocol_feature(
                ProtocolFeatureKind::Core,
                9,
                9,
            )]),
            ..Default::default()
        })
        .await;
        let refused = require_live_gateway_compatible(&incompatible.0, Some("Plant.Server"))
            .await
            .expect_err("an incompatible core must refuse the live mutation");
        assert!(refused.to_string().contains("incompatible"));

        let partial = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info(vec![protocol_feature(
                ProtocolFeatureKind::Core,
                1,
                1,
            )]),
            ..Default::default()
        })
        .await;
        let partial_report = require_live_gateway_compatible(&partial.0, None)
            .await
            .unwrap();
        assert_eq!(partial_report.status, OpcDaCompatibilityStatus::Partial);

        let unknown = start_mock_server(MockBridgeService::default()).await;
        let unknown_report = require_live_gateway_compatible(&unknown.0, None)
            .await
            .unwrap();
        assert_eq!(unknown_report.status, OpcDaCompatibilityStatus::Unknown);

        let full = start_mock_server(MockBridgeService {
            gateway_info_response: gateway_info(vec![
                protocol_feature(ProtocolFeatureKind::Core, 1, 1),
                protocol_feature(ProtocolFeatureKind::Namespace, 2, 3),
                protocol_feature(ProtocolFeatureKind::IndexedSearch, 3, 3),
            ]),
            ..Default::default()
        })
        .await;
        let full_report = require_live_gateway_compatible(&full.0, None)
            .await
            .unwrap();
        assert_eq!(full_report.status, OpcDaCompatibilityStatus::Full);
        assert!(compatibility_json(&full_report).contains("\"status\":\"full\""));
    }

    #[test]
    fn parse_stored_compatibility_returns_none_for_an_unreadable_snapshot() {
        assert!(parse_stored_compatibility("not-a-report").is_none());
        assert!(parse_stored_compatibility(r#"{"status":"unknown"}"#).is_none());
    }
}
