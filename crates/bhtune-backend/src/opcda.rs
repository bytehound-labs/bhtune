//! `OpcDaBackend`: the primary [`Backend`] implementation, talking to a DCS/PLC's OPC DA
//! server through the `opcda-bridge` gateway's gRPC API.
//!
//! This module intentionally splits into two halves: a thin async shell (`OpcDaBackend`
//! itself) that only locks the client and calls into `opcda_bridge`, and a set of small,
//! pure, synchronous mapping functions (`quality_from_raw`, `tag_value_from_raw`,
//! `opc_value_from_write`, `write_outcome_from_result`, `tag_node_from_browse`,
//! `map_bridge_error`) that do all the actual translation between `opcda_bridge`'s types and
//! this crate's. The mapping functions carry the real risk of a subtle bug (a quality string
//! typo, a swapped success/failure branch) and are fully unit-testable with no I/O; the shell
//! is exercised end-to-end by one mock-gateway smoke test rather than a full error-path
//! matrix, since `opcda-bridge`'s own test suite already covers the gRPC plumbing itself.

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    backend::Backend,
    error::{BackendError, BackendResult},
    types::{Quality, TagId, TagNode, TagValue, TagWrite, WriteOutcome},
};

/// A generous per-call cap on how many tags a single `browse` call returns, matching
/// `opcda-bridge-client`'s own CLI default (`DEFAULT_MAX_TAGS` in that crate's
/// `config.rs`) so bhtune's browsing behavior is consistent with the reference CLI rather
/// than picking an unrelated number.
const DEFAULT_MAX_TAGS: u32 = 1000;

/// The primary [`Backend`] for v1: reads, writes, and browses OPC DA tags through an
/// `opcda-bridge` gateway's gRPC API.
///
/// Holds its `opcda_bridge::Client` behind a `tokio::sync::Mutex` because the client's own
/// methods take `&mut self` (it buffers per-call gRPC codec state) while [`Backend`]'s
/// methods take `&self` (required so a backend can be shared behind `Arc<dyn Backend>`) —
/// serializing calls through the one connection is a reasonable tradeoff, since a single
/// tuning session only ever has one read/write/browse in flight at a time, and the
/// underlying HTTP/2 channel stays cheaply multiplexed regardless of how many logical
/// callers there are.
#[derive(Debug)]
pub struct OpcDaBackend {
    client: Mutex<opcda_bridge::Client>,
    server: String,
}

impl OpcDaBackend {
    /// Connects to an `opcda-bridge` gateway at `host` (e.g. `"localhost:7600"`) and binds
    /// to `server` — the OPC DA server's ProgID (e.g. `"Matrikon.OPC.Simulation.1"`) that
    /// every subsequent `read`/`write`/`browse` call is scoped to.
    pub async fn connect(host: &str, server: impl Into<String>) -> BackendResult<Self> {
        let client = opcda_bridge::Client::connect(host)
            .await
            .map_err(map_bridge_error)?;
        Ok(Self {
            client: Mutex::new(client),
            server: server.into(),
        })
    }
}

#[async_trait]
impl Backend for OpcDaBackend {
    async fn read(&self, tags: &[TagId]) -> BackendResult<Vec<TagValue>> {
        let mut client = self.client.lock().await;
        let raw = client
            .read(self.server.clone(), tags.to_vec())
            .await
            .map_err(map_bridge_error)?;
        Ok(raw.into_iter().map(tag_value_from_raw).collect())
    }

    async fn write(&self, tag: &TagId, value: TagWrite) -> BackendResult<WriteOutcome> {
        let mut client = self.client.lock().await;
        let result = client
            .write(
                self.server.clone(),
                tag.clone(),
                opc_value_from_write(value),
            )
            .await
            .map_err(map_bridge_error)?;
        Ok(write_outcome_from_result(result))
    }

    async fn browse(&self, path: &str) -> BackendResult<Vec<TagNode>> {
        let mut client = self.client.lock().await;
        let nodes = client
            .browse(
                self.server.clone(),
                false,
                path.to_string(),
                DEFAULT_MAX_TAGS,
            )
            .await
            .map_err(map_bridge_error)?;
        Ok(nodes.into_iter().map(tag_node_from_browse).collect())
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
fn quality_from_raw(raw: &str) -> Quality {
    match raw {
        "Good" => Quality::Good,
        "Uncertain" => Quality::Uncertain,
        _ => Quality::Bad,
    }
}

/// Maps one `opcda_bridge::TagValue` (a single tag's raw read result) to this crate's
/// [`TagValue`].
fn tag_value_from_raw(raw: opcda_bridge::TagValue) -> TagValue {
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
fn opc_value_from_write(write: TagWrite) -> opcda_bridge::Value {
    match write {
        TagWrite::Float(f) => opcda_bridge::Value::Float(f64::from(f)),
        TagWrite::Raw(s) => opcda_bridge::Value::String(s),
    }
}

/// Maps an `opcda_bridge::WriteResult` (the RPC-level outcome of one write) to
/// [`WriteOutcome`].
fn write_outcome_from_result(result: opcda_bridge::WriteResult) -> WriteOutcome {
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

/// Maps an `opcda_bridge::BrowseNode` to [`TagNode`]. The gateway's own `"Leaf"`/`"Branch"`
/// constants (see that crate's server implementation) are the only two values it ever
/// sends; anything else is treated as a leaf — the conservative choice, since a caller that
/// wrongly assumes a real branch is a leaf will at worst attempt to read/write it and get a
/// clear error back, whereas wrongly assuming a real leaf is a branch would make a genuine
/// tag silently invisible to a tag-tree browser.
fn tag_node_from_browse(node: opcda_bridge::BrowseNode) -> TagNode {
    TagNode {
        tag: node.tag_id,
        is_branch: node.node_type == "Branch",
    }
}

/// Maps every `opcda_bridge::Error` this backend can encounter — at connect time or during
/// any RPC — to the matching [`BackendError`] variant: `opcda_bridge::Error::Connect` (the
/// gRPC channel itself couldn't be established) becomes [`BackendError::Connect`], and
/// `opcda_bridge::Error::Rpc` (the channel is fine, but the gateway returned a gRPC error
/// for this specific call) becomes [`BackendError::Operation`]. An exhaustive match rather
/// than a wildcard arm, deliberately: if `opcda_bridge::Error` ever gains a new variant,
/// this should fail to compile and force a real decision about where it belongs, not
/// silently fall into one bucket.
fn map_bridge_error(err: opcda_bridge::Error) -> BackendError {
    match &err {
        opcda_bridge::Error::Connect(_) => BackendError::Connect(Box::new(err)),
        opcda_bridge::Error::Rpc(_) => BackendError::Operation(Box::new(err)),
    }
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
    fn tag_node_from_browse_maps_leaf_and_branch() {
        let leaf = tag_node_from_browse(opcda_bridge::BrowseNode {
            tag_id: "Area1.LIC101.PV".to_string(),
            node_type: "Leaf".to_string(),
        });
        assert!(!leaf.is_branch);

        let branch = tag_node_from_browse(opcda_bridge::BrowseNode {
            tag_id: "Area1".to_string(),
            node_type: "Branch".to_string(),
        });
        assert!(branch.is_branch);
    }

    #[test]
    fn tag_node_from_browse_treats_unrecognized_node_type_as_a_leaf() {
        let node = tag_node_from_browse(opcda_bridge::BrowseNode {
            tag_id: "t".to_string(),
            node_type: "Mystery".to_string(),
        });
        assert!(!node.is_branch);
    }

    #[tokio::test]
    async fn connect_failure_maps_to_backend_error_connect() {
        // Nothing is listening on this port, so `Client::connect` fails at the transport
        // level before any RPC is attempted -- exactly the `BackendError::Connect` case.
        let err = OpcDaBackend::connect("127.0.0.1:1", "AnyServer")
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::Connect(_)));
    }
}

/// End-to-end smoke tests against a minimal mock `Bridge` gRPC service: these exist to
/// prove `OpcDaBackend` is wired together correctly (locking, field mapping, error
/// propagation all actually compose through a real `tonic` round trip), not to
/// re-exercise `opcda-bridge`'s own already-tested RPC error-path matrix.
#[cfg(test)]
mod smoke_tests {
    use super::*;
    use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
    use opcda_bridge_proto::bridge::{
        BrowseRequest, BrowseResponse, ListServersRequest, ListServersResponse, ReadRequest,
        ReadResponse, TagValue as ProtoTagValue, WriteRequest, WriteResponse,
    };
    use std::net::SocketAddr;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    #[derive(Default)]
    struct MockBridgeService {
        browse_responses: Vec<BrowseResponse>,
        read_response: ReadResponse,
        write_response: WriteResponse,
        write_error: Option<Status>,
    }

    #[tonic::async_trait]
    impl Bridge for MockBridgeService {
        async fn list_servers(
            &self,
            _request: Request<ListServersRequest>,
        ) -> Result<Response<ListServersResponse>, Status> {
            Ok(Response::new(ListServersResponse { servers: vec![] }))
        }

        type BrowseStream = ReceiverStream<Result<BrowseResponse, Status>>;

        async fn browse(
            &self,
            _request: Request<BrowseRequest>,
        ) -> Result<Response<Self::BrowseStream>, Status> {
            let (tx, rx) = mpsc::channel(4);
            let items = self.browse_responses.clone();
            tokio::spawn(async move {
                for item in items {
                    let _ = tx.send(Ok(item)).await;
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

    /// Starts `service` on an ephemeral localhost port and returns its `host:port` address,
    /// ready to be passed to [`OpcDaBackend::connect`]. Mirrors the pattern
    /// `opcda-bridge`'s own `test_support.rs` uses for its equivalent mock server, trimmed
    /// to only what these smoke tests exercise (no graceful shutdown -- each test's server
    /// simply runs for the rest of the test process, on its own ephemeral port).
    async fn start_mock_server(service: MockBridgeService) -> String {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            Server::builder()
                .add_service(BridgeServer::new(service))
                .serve_with_incoming(TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        format!("127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn read_round_trips_through_a_real_gateway_connection() {
        let service = MockBridgeService {
            read_response: ReadResponse {
                values: vec![ProtoTagValue {
                    tag_id: "Area1.LIC101.PV".to_string(),
                    value: "42.5".to_string(),
                    quality: "Good".to_string(),
                    timestamp: "2024-01-15 10:23:45".to_string(),
                }],
            },
            ..Default::default()
        };
        let host = start_mock_server(service).await;
        let backend = OpcDaBackend::connect(&host, "S1").await.unwrap();

        let values = backend
            .read(&["Area1.LIC101.PV".to_string()])
            .await
            .unwrap();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].tag, "Area1.LIC101.PV");
        assert_eq!(values[0].value, "42.5");
        assert_eq!(values[0].quality, Quality::Good);
    }

    #[tokio::test]
    async fn write_round_trips_a_rejected_write_as_an_ok_outcome_not_an_error() {
        let service = MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Area1.LIC101.MV".to_string(),
                success: false,
                error: Some("tag is read-only".to_string()),
            },
            ..Default::default()
        };
        let host = start_mock_server(service).await;
        let backend = OpcDaBackend::connect(&host, "S1").await.unwrap();

        let outcome = backend
            .write(&"Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))
            .await
            .unwrap();
        assert!(!outcome.success);
        assert_eq!(outcome.error_message.as_deref(), Some("tag is read-only"));
    }

    #[tokio::test]
    async fn write_rpc_error_maps_to_backend_error_operation() {
        let service = MockBridgeService {
            write_error: Some(Status::internal("boom")),
            ..Default::default()
        };
        let host = start_mock_server(service).await;
        let backend = OpcDaBackend::connect(&host, "S1").await.unwrap();

        let err = backend
            .write(&"Area1.LIC101.MV".to_string(), TagWrite::Float(55.0))
            .await
            .unwrap_err();
        assert!(matches!(err, BackendError::Operation(_)));
    }

    #[tokio::test]
    async fn browse_maps_leaf_and_branch_nodes() {
        let service = MockBridgeService {
            browse_responses: vec![
                BrowseResponse {
                    tag_id: "Area1".to_string(),
                    node_type: "Branch".to_string(),
                },
                BrowseResponse {
                    tag_id: "Area1.LIC101.PV".to_string(),
                    node_type: "Leaf".to_string(),
                },
            ],
            ..Default::default()
        };
        let host = start_mock_server(service).await;
        let backend = OpcDaBackend::connect(&host, "S1").await.unwrap();

        let nodes = backend.browse("").await.unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_branch);
        assert!(!nodes[1].is_branch);
    }
}
