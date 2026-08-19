//! Shared test-only helpers for building an [`AppState`] backed by a seeded, in-memory
//! database -- every route test module needs one, so it lives here rather than being
//! copy-pasted per module.

#![cfg(test)]

use chrono::Utc;

use crate::active_run::ActiveRun;
use crate::state::AppState;

/// An in-memory SQLite pool, migrated and seeded with the four built-in DCS/PLC templates
/// (Yokogawa CentumVP, Honeywell Experion, Schneider Modicon, Allen-Bradley PlantPAx) --
/// matching what any real bhtune install has from its first startup, so route tests can
/// exercise the "list/show an existing template" paths without each test seeding its own
/// fixture data.
pub(crate) async fn in_memory_state() -> AppState {
    let pool = bhtune_db::connect_in_memory()
        .await
        .expect("in-memory pool should always connect and migrate cleanly");
    bhtune_db::seed_builtin_templates(&pool, Utc::now())
        .await
        .expect("seeding the built-in templates into a fresh in-memory db should never fail");
    AppState {
        pool,
        active_run: ActiveRun::default(),
        app_config: bhtune_cli::config::BhtuneConfig::default(),
    }
}

/// A minimal mock `Bridge` gRPC service for tests that need a real
/// [`bhtune_driver::OpcDaDriver`] connect/read/write/browse/list-servers round trip rather
/// than stopping at a route's eligibility checks -- originally `routes::runs`'s own private
/// `mod mock_bridge` (for `write_run`/`revert_run`, with `list_servers`/`browse` hardcoded to
/// empty since neither handler ever called them), promoted here and given configurable
/// `list_servers_response`/`browse_responses` once `routes::opc`'s tests needed to actually
/// exercise those two RPCs. Mirrors `bhtune_driver::opcda`'s own `smoke_tests` module (itself
/// mirroring `bhtune-cli`'s `test_support`), field-for-field where the shape overlaps.
pub(crate) mod mock_bridge {
    use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
    use opcda_bridge_proto::bridge::{
        BrowseRequest, BrowseResponse, ListServersRequest, ListServersResponse, ReadRequest,
        ReadResponse, WriteRequest, WriteResponse,
    };
    use std::net::SocketAddr;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    #[derive(Default)]
    pub(crate) struct MockBridgeService {
        pub(crate) list_servers_response: ListServersResponse,
        pub(crate) list_servers_error: Option<Status>,
        pub(crate) browse_responses: Vec<BrowseResponse>,
        pub(crate) browse_error: Option<Status>,
        pub(crate) read_response: ReadResponse,
        pub(crate) read_error: Option<Status>,
        pub(crate) write_response: WriteResponse,
        pub(crate) write_error: Option<Status>,
    }

    #[tonic::async_trait]
    impl Bridge for MockBridgeService {
        async fn list_servers(
            &self,
            _request: Request<ListServersRequest>,
        ) -> Result<Response<ListServersResponse>, Status> {
            if let Some(status) = self.list_servers_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.list_servers_response.clone()))
        }

        type BrowseStream = ReceiverStream<Result<BrowseResponse, Status>>;

        async fn browse(
            &self,
            _request: Request<BrowseRequest>,
        ) -> Result<Response<Self::BrowseStream>, Status> {
            if let Some(status) = self.browse_error.clone() {
                return Err(status);
            }
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let items = self.browse_responses.clone();
            tokio::spawn(async move {
                for item in items {
                    if tx.send(Ok(item)).await.is_err() {
                        // The caller dropped the stream before consuming every item --
                        // stop forwarding rather than sending into a closed channel.
                        break;
                    }
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }

        async fn read(
            &self,
            _request: Request<ReadRequest>,
        ) -> Result<Response<ReadResponse>, Status> {
            if let Some(status) = self.read_error.clone() {
                return Err(status);
            }
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

    /// Starts `service` on an ephemeral localhost port and returns its `host:port`
    /// address, ready to be recorded as a run's `bridge_host`. No graceful shutdown --
    /// each test's server simply runs for the rest of the test process on its own
    /// ephemeral port, matching the upstream pattern this mirrors.
    pub(crate) async fn start_mock_server(service: MockBridgeService) -> String {
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

    /// A "Good"-quality `"10.0"` reading, regardless of which tag was requested --
    /// matching `bhtune-cli`'s own `history::revert` test fixtures' rationale: every
    /// pre-read and every write's confirmation readback returns this same value, so a
    /// fixture that also writes/reverts to `10.0` always sees a matching readback no
    /// matter which of the three PID constants is being processed.
    pub(crate) fn good_reading(value: &str) -> ReadResponse {
        ReadResponse {
            values: vec![opcda_bridge_proto::bridge::TagValue {
                tag_id: "ignored".to_string(),
                value: value.to_string(),
                quality: "Good".to_string(),
                timestamp: "2024-01-15 10:23:45".to_string(),
            }],
        }
    }
}
