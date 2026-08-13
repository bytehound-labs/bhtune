//! `bhtune opc read/write/browse`: thin passthrough diagnostics directly over
//! [`bhtune_backend::OpcDaBackend`], independent of running a full tune. Useful for checking
//! gateway connectivity and confirming tag names before starting a real test.

use bhtune_backend::{Backend, OpcDaBackend, TagWrite};

use crate::args::OpcCommand;

pub async fn run(command: OpcCommand, config: &crate::config::BhtuneConfig) -> anyhow::Result<()> {
    match command {
        OpcCommand::Read {
            bridge_host,
            server,
            tags,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            read(&bridge_host, &server, &tags).await
        }
        OpcCommand::Write {
            bridge_host,
            server,
            tag,
            value,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            write(&bridge_host, &server, &tag, &value).await
        }
        OpcCommand::Browse {
            bridge_host,
            server,
            path,
        } => {
            let bridge_host = crate::config::resolve_bridge_host(bridge_host, config);
            let server = crate::config::resolve_server(server, config)?;
            browse(&bridge_host, &server, &path).await
        }
    }
}

async fn read(bridge_host: &str, server: &str, tags: &[String]) -> anyhow::Result<()> {
    if tags.is_empty() {
        anyhow::bail!("at least one tag is required");
    }
    let backend = OpcDaBackend::connect(bridge_host, server).await?;
    let values = backend.read(tags).await?;
    println!(
        "{:<40} {:<15} {:<10} {:<20}",
        "TAG", "VALUE", "QUALITY", "TIMESTAMP"
    );
    for v in values {
        println!(
            "{:<40} {:<15} {:<10} {:<20}",
            v.tag,
            v.value,
            format!("{:?}", v.quality),
            v.timestamp
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|| "-".to_string()),
        );
    }
    Ok(())
}

async fn write(bridge_host: &str, server: &str, tag: &str, value: &str) -> anyhow::Result<()> {
    let backend = OpcDaBackend::connect(bridge_host, server).await?;
    // Numeric-looking values are written as floats (matching a live process value or PID
    // constant write); anything else is written raw (e.g. a mode code like "MAN").
    let write_value = match value.parse::<f32>() {
        Ok(f) => TagWrite::Float(f),
        Err(_) => TagWrite::Raw(value.to_string()),
    };
    let outcome = backend.write(&tag.to_string(), write_value).await?;
    if outcome.success {
        println!("Wrote '{value}' to '{tag}'.");
        Ok(())
    } else {
        anyhow::bail!(
            "backend rejected the write: {}",
            outcome
                .error_message
                .unwrap_or_else(|| "unknown reason".to_string())
        )
    }
}

async fn browse(bridge_host: &str, server: &str, path: &str) -> anyhow::Result<()> {
    let backend = OpcDaBackend::connect(bridge_host, server).await?;
    let nodes = backend.browse(path).await?;
    if nodes.is_empty() {
        println!("No tags found under '{path}'.");
        return Ok(());
    }
    for node in nodes {
        println!(
            "{} {}",
            if node.is_branch { "[+]" } else { "   " },
            node.tag
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockBridgeService, start_mock_server};
    use opcda_bridge_proto::bridge::{
        BrowseResponse, ReadResponse, TagValue as ProtoTagValue, WriteResponse,
    };

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
        .await;

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
        let (host, server) = start_mock_server(MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Unit1.LIC101.OP".to_string(),
                success: true,
                error: None,
            },
            ..Default::default()
        })
        .await;

        write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
            .await
            .unwrap();

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
        .await;

        let err = write(&host, "Sim.Server", "Unit1.LIC101.OP", "55.0")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("read-only"));

        server.shutdown().await;
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
        .await;

        write(&host, "Sim.Server", "Unit1.LIC101.MODE", "MAN")
            .await
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_prints_nodes_from_a_mock_gateway() {
        let (host, server) = start_mock_server(MockBridgeService {
            browse_responses: vec![BrowseResponse {
                tag_id: "Unit1".to_string(),
                node_type: "Branch".to_string(),
            }],
            ..Default::default()
        })
        .await;

        browse(&host, "Sim.Server", "").await.unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn browse_handles_an_empty_result() {
        let (host, server) = start_mock_server(MockBridgeService::default()).await;
        browse(&host, "Sim.Server", "Unit1").await.unwrap();
        server.shutdown().await;
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
    async fn run_dispatches_read_write_and_browse() {
        let (host, server) = start_mock_server(MockBridgeService {
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
            browse_responses: vec![BrowseResponse {
                tag_id: "Unit1".to_string(),
                node_type: "Branch".to_string(),
            }],
            ..Default::default()
        })
        .await;
        let config = crate::config::BhtuneConfig::default();

        run(
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
            OpcCommand::Browse {
                bridge_host: Some(host),
                server: Some("Sim.Server".to_string()),
                path: String::new(),
            },
            &config,
        )
        .await
        .unwrap();

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
        .await;
        let config = crate::config::BhtuneConfig {
            bridge_host: Some(host),
            server: Some("Sim.Server".to_string()),
            ..Default::default()
        };

        run(
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
    async fn run_errors_when_server_is_unset_in_both_cli_and_config() {
        let err = run(
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
}
