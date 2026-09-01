//! Shared test-only helpers for building an [`AppState`] backed by a seeded, in-memory
//! database -- every route test module needs one, so it lives here rather than being
//! copy-pasted per module.

#![cfg(test)]

use chrono::Utc;
use std::sync::{Arc, RwLock};

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
    let mut config_store =
        bhtune_cli::config::load_config_store_from(None, None, None, None, false)
            .expect("default test config store should load");
    // Keep route tests fast now that HTTP requests correctly inherit global timing settings
    // instead of carrying obsolete per-run timing fields.
    config_store.config.tuning = bhtune_cli::config::TuningConfig {
        mrft_delay_secs: Some(0),
        poll_interval_ms: Some(5),
        timeout_secs: Some(5),
        op_timeout_secs: Some(5),
        restore_timeout_secs: Some(5),
    };
    config_store.toml_tuning = config_store.config.tuning;
    config_store.tuning_sources =
        bhtune_cli::config::tuning_config_sources(&config_store.toml_tuning);
    AppState {
        pool,
        active_run: ActiveRun::default(),
        config_store: Arc::new(RwLock::new(config_store)),
    }
}

/// A minimal mock `Bridge` gRPC service for tests that need a real
/// [`bhtune_driver::OpcDaDriver`] connect/read/write/browse/list-servers round trip rather
/// than stopping at a route's eligibility checks -- originally `routes::runs`'s own private
/// `mod mock_bridge` (for `write_run`/`revert_run`, with `list_servers`/`browse` hardcoded to
/// empty since neither handler ever called them), promoted here for the OPC diagnostic routes.
/// The mock mirrors the released typed browse/session/search protobuf contract.
pub(crate) mod mock_bridge {
    use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
    use opcda_bridge_proto::bridge::{
        BrowsePage, BrowseRequest, CloseBrowseSessionRequest, ControlSearchIndexRequest,
        GetCapabilitiesRequest, GetCapabilitiesResponse, GetSearchIndexStatusRequest,
        ListServersRequest, ListServersResponse, ReadRequest, ReadResponse,
        RefreshSearchIndexRequest, SearchEvent, SearchIndexResponse, SearchIndexStatus,
        SearchRequest, WriteRequest, WriteResponse,
    };
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;
    use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    pub(crate) struct MockBridgeService {
        pub(crate) list_servers_response: ListServersResponse,
        pub(crate) list_servers_error: Option<Status>,
        pub(crate) browse_response: BrowsePage,
        pub(crate) browse_error: Option<Status>,
        pub(crate) capabilities_response: GetCapabilitiesResponse,
        pub(crate) capabilities_error: Option<Status>,
        pub(crate) search_events: Vec<SearchEvent>,
        pub(crate) search_error: Option<Status>,
        pub(crate) search_index_status_response: SearchIndexStatus,
        pub(crate) search_index_status_error: Option<Status>,
        pub(crate) search_index_status_requests: Arc<Mutex<Vec<GetSearchIndexStatusRequest>>>,
        pub(crate) search_index_response: SearchIndexResponse,
        pub(crate) search_index_error: Option<Status>,
        pub(crate) search_index_requests:
            Arc<Mutex<Vec<opcda_bridge_proto::bridge::SearchIndexRequest>>>,
        pub(crate) refresh_search_index_error: Option<Status>,
        pub(crate) refresh_search_index_requests: Arc<Mutex<Vec<RefreshSearchIndexRequest>>>,
        pub(crate) control_search_index_error: Option<Status>,
        pub(crate) control_search_index_requests: Arc<Mutex<Vec<ControlSearchIndexRequest>>>,
        pub(crate) read_response: ReadResponse,
        pub(crate) read_error: Option<Status>,
        pub(crate) write_response: WriteResponse,
        pub(crate) write_error: Option<Status>,
    }

    impl Default for MockBridgeService {
        fn default() -> Self {
            Self {
                list_servers_response: ListServersResponse::default(),
                list_servers_error: None,
                browse_response: BrowsePage {
                    complete: true,
                    ..Default::default()
                },
                browse_error: None,
                capabilities_response: GetCapabilitiesResponse {
                    application_version: "0.4.0".to_string(),
                    protocol_version: "2".to_string(),
                    max_page_size: 1000,
                    supports_browse_sessions: true,
                    supports_search: true,
                    supports_indexed_search: true,
                    indexed_search_protocol_version: "1".to_string(),
                    max_indexed_search_results: 50,
                    ..Default::default()
                },
                capabilities_error: None,
                search_events: Vec::new(),
                search_error: None,
                search_index_status_response: SearchIndexStatus::default(),
                search_index_status_error: None,
                search_index_status_requests: Arc::new(Mutex::new(Vec::new())),
                search_index_response: SearchIndexResponse::default(),
                search_index_error: None,
                search_index_requests: Arc::new(Mutex::new(Vec::new())),
                refresh_search_index_error: None,
                refresh_search_index_requests: Arc::new(Mutex::new(Vec::new())),
                control_search_index_error: None,
                control_search_index_requests: Arc::new(Mutex::new(Vec::new())),
                read_response: ReadResponse::default(),
                read_error: None,
                write_response: WriteResponse::default(),
                write_error: None,
            }
        }
    }

    #[tonic::async_trait]
    impl Bridge for MockBridgeService {
        async fn get_capabilities(
            &self,
            _request: Request<GetCapabilitiesRequest>,
        ) -> Result<Response<GetCapabilitiesResponse>, Status> {
            if let Some(status) = self.capabilities_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.capabilities_response.clone()))
        }

        async fn list_servers(
            &self,
            _request: Request<ListServersRequest>,
        ) -> Result<Response<ListServersResponse>, Status> {
            if let Some(status) = self.list_servers_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.list_servers_response.clone()))
        }

        async fn browse(
            &self,
            _request: Request<BrowseRequest>,
        ) -> Result<Response<BrowsePage>, Status> {
            if let Some(status) = self.browse_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.browse_response.clone()))
        }

        async fn close_browse_session(
            &self,
            _request: Request<CloseBrowseSessionRequest>,
        ) -> Result<Response<()>, Status> {
            Ok(Response::new(()))
        }

        type SearchStream = ReceiverStream<Result<SearchEvent, Status>>;

        async fn search(
            &self,
            _request: Request<SearchRequest>,
        ) -> Result<Response<Self::SearchStream>, Status> {
            if let Some(status) = self.search_error.clone() {
                return Err(status);
            }
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let events = self.search_events.clone();
            tokio::spawn(async move {
                for event in events {
                    if tx.send(Ok(event)).await.is_err() {
                        break;
                    }
                }
            });
            Ok(Response::new(ReceiverStream::new(rx)))
        }

        async fn get_search_index_status(
            &self,
            request: Request<GetSearchIndexStatusRequest>,
        ) -> Result<Response<SearchIndexStatus>, Status> {
            self.search_index_status_requests
                .lock()
                .unwrap()
                .push(request.into_inner());
            if let Some(status) = self.search_index_status_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn refresh_search_index(
            &self,
            request: Request<RefreshSearchIndexRequest>,
        ) -> Result<Response<SearchIndexStatus>, Status> {
            self.refresh_search_index_requests
                .lock()
                .unwrap()
                .push(request.into_inner());
            if let Some(status) = self.refresh_search_index_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn control_search_index(
            &self,
            request: Request<ControlSearchIndexRequest>,
        ) -> Result<Response<SearchIndexStatus>, Status> {
            self.control_search_index_requests
                .lock()
                .unwrap()
                .push(request.into_inner());
            if let Some(status) = self.control_search_index_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.search_index_status_response.clone()))
        }

        async fn search_index(
            &self,
            request: Request<opcda_bridge_proto::bridge::SearchIndexRequest>,
        ) -> Result<Response<SearchIndexResponse>, Status> {
            self.search_index_requests
                .lock()
                .unwrap()
                .push(request.into_inner());
            if let Some(status) = self.search_index_error.clone() {
                return Err(status);
            }
            Ok(Response::new(self.search_index_response.clone()))
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
    /// address, ready to be recorded as a run's `bridge_host`. Tests that need explicit
    /// cleanup should use [`start_mock_server_with_handle`] instead.
    pub(crate) async fn start_mock_server(service: MockBridgeService) -> String {
        let (host, handle) = start_mock_server_with_handle(service).await;
        std::mem::forget(handle);
        host
    }

    pub(crate) struct MockServerHandle {
        shutdown: Option<oneshot::Sender<()>>,
        task: JoinHandle<()>,
    }

    impl MockServerHandle {
        pub(crate) async fn shutdown(mut self) {
            let _ = self.shutdown.take().unwrap().send(());
            self.task.await.unwrap();
        }
    }

    pub(crate) async fn start_mock_server_with_handle(
        service: MockBridgeService,
    ) -> (String, MockServerHandle) {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (shutdown, shutdown_signal) = oneshot::channel();
        let task = tokio::spawn(async move {
            Server::builder()
                .add_service(BridgeServer::new(service))
                .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                    let _ = shutdown_signal.await;
                })
                .await
                .unwrap();
        });
        (
            format!("127.0.0.1:{port}"),
            MockServerHandle {
                shutdown: Some(shutdown),
                task,
            },
        )
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use opcda_bridge_proto::bridge::BrowseRequest;

        #[tokio::test]
        async fn browse_returns_the_configured_page() {
            let service = MockBridgeService {
                browse_response: BrowsePage {
                    complete: false,
                    ..Default::default()
                },
                ..Default::default()
            };
            let response = service
                .browse(Request::new(BrowseRequest::default()))
                .await
                .unwrap();

            assert!(!response.into_inner().complete);
        }

        #[tokio::test]
        async fn mock_server_can_be_shutdown_and_joined() {
            let (_host, handle) = start_mock_server_with_handle(MockBridgeService::default()).await;
            handle.shutdown().await;
        }
    }
}
