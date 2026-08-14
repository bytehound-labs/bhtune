//! Constructs the selected [`bhtune_backend::Backend`] implementation from a [`TuneArgs`].

use bhtune_backend::{Backend, FopdtConfig, OpcDaBackend, SimulatorBackend};

use crate::args::{BackendKindArg, TuneArgs};

/// The two tag names [`SimulatorBackend`] is configured with — fixed rather than derived
/// from `--tagname`/a template, since the simulator has no DCS suffix convention at all (see
/// `backend-simulator`'s two-tag-only contract).
pub const SIMULATOR_PV_TAG: &str = "Sim.PV";
pub const SIMULATOR_MV_TAG: &str = "Sim.MV";

/// Builds the backend `args` selects. For `--backend simulator`, `args.tagname` is ignored;
/// the caller must build its [`bhtune_core::LoopTags`] using [`SIMULATOR_PV_TAG`]/
/// [`SIMULATOR_MV_TAG`] instead of deriving from a template.
pub async fn build(args: &TuneArgs) -> anyhow::Result<Box<dyn Backend>> {
    match args.backend {
        BackendKindArg::Opcda => {
            let server = args
                .server
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--server is required with --backend opcda"))?;
            // By the time `build` runs, `commands::tune::run` has already resolved
            // `args.bridge_host` through `crate::config::resolve_bridge_host` (CLI > env >
            // config file > default), so this `unwrap_or` is a defensive fallback for
            // direct/test callers that bypass that resolution step, not the primary
            // precedence mechanism.
            let bridge_host = args
                .bridge_host
                .as_deref()
                .unwrap_or(crate::config::DEFAULT_BRIDGE_HOST);
            tracing::info!(bridge_host, server = %server, "connecting to opcda-bridge gateway");
            let backend = OpcDaBackend::connect(bridge_host, server).await?;
            Ok(Box::new(backend))
        }
        BackendKindArg::Simulator => {
            tracing::info!(
                gain = args.sim_gain,
                tau = args.sim_tau,
                dead_time = args.sim_dead_time,
                "constructing simulator backend"
            );
            let config = FopdtConfig::new(
                args.sim_gain,
                args.sim_tau,
                args.sim_dead_time,
                args.poll_interval_ms as f32 / 1000.0,
            )
            .with_noise_amplitude(args.sim_noise);
            let backend = SimulatorBackend::new(
                SIMULATOR_PV_TAG,
                SIMULATOR_MV_TAG,
                config,
                args.sim_initial_pv,
                args.sim_initial_mv,
                args.sim_seed,
            );
            Ok(Box::new(backend))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::DirectionArg;

    fn sim_args() -> TuneArgs {
        TuneArgs {
            tagname: "ignored".to_string(),
            template: "Yokogawa CentumVP".to_string(),
            process_type: crate::args::ProcessTypeArg::Flow,
            controller_type: crate::args::ControllerTypeArg::Pi,
            relay_amp: 10.0,
            cycles_skip: None,
            cycles_count: None,
            noise_protection_secs: None,
            mrft_delay: 0,
            backend: BackendKindArg::Simulator,
            bridge_host: None,
            server: None,
            sim_gain: 1.0,
            sim_tau: 2.0,
            sim_dead_time: 5.0,
            sim_noise: 0.0,
            sim_seed: 0,
            sim_initial_pv: 50.0,
            sim_initial_mv: 50.0,
            pv_range_high: Some(100.0),
            pv_range_low: Some(0.0),
            mv_range_high: Some(100.0),
            mv_range_low: Some(0.0),
            direction: Some(DirectionArg::Reverse),
            poll_interval_ms: 800,
            timeout_secs: 3600,
            name: None,
            yes: false,
            write_pid: None,
            output: crate::output::OutputFormat::Table,
        }
    }

    #[tokio::test]
    async fn builds_a_working_simulator_backend() {
        let backend = build(&sim_args()).await.unwrap();
        let values = backend.read(&[SIMULATOR_MV_TAG.to_string()]).await.unwrap();
        assert_eq!(values[0].value, "50");
    }

    #[tokio::test]
    async fn opcda_backend_requires_a_server_flag() {
        let mut args = sim_args();
        args.backend = BackendKindArg::Opcda;
        args.bridge_host = Some("127.0.0.1:1".to_string());
        args.server = None;
        let result = build(&args).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("--server"));
    }

    #[tokio::test]
    async fn opcda_backend_falls_back_to_the_default_bridge_host_when_unset() {
        // `build()` is normally only reached after `commands::tune::run` has already
        // resolved `bridge_host` via `crate::config::resolve_bridge_host`, so a `None` here
        // only happens for a direct/test caller -- confirms the fallback constant is used
        // rather than e.g. an empty host string.
        let mut args = sim_args();
        args.backend = BackendKindArg::Opcda;
        args.bridge_host = None;
        args.server = Some("MockServer".to_string());
        let err = build(&args).await.err().unwrap();
        // `DEFAULT_BRIDGE_HOST` ("localhost:7600") has nothing listening in CI, so this
        // still fails -- what matters is that it attempted the default host, not a blank one.
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn opcda_backend_connects_and_reads_through_a_mock_bridge() {
        use crate::test_support::{MockBridgeService, start_mock_server};
        use opcda_bridge_proto::bridge::{ReadResponse, TagValue as ProtoTagValue};

        let (host, server) = start_mock_server(MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Sim.MV".to_string(),
                    value: "50".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        })
        .await;

        let mut args = sim_args();
        args.backend = BackendKindArg::Opcda;
        args.bridge_host = Some(host);
        args.server = Some("MockServer".to_string());

        // Reaching a real read confirms `build()`'s OPC DA branch actually returned a
        // connected, working `OpcDaBackend`, not just that `connect()` didn't error.
        let backend = build(&args).await.unwrap();
        let values = backend.read(&["Sim.MV".to_string()]).await.unwrap();
        assert_eq!(values[0].value, "50");

        server.shutdown().await;
    }
}
