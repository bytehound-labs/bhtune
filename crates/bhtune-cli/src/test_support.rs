//! Test-only mock gRPC `Bridge` service, shared by [`crate::driver`] and the
//! [`crate::commands::tune`]/[`crate::commands::opc`] tests that need a real (mock) OPC DA
//! bridge for [`bhtune_driver::OpcDaDriver`]/[`bhtune_driver::list_opcda_servers`] to connect
//! to — proving the CLI's own OPC DA wiring (connect success path, driver-kind bookkeeping,
//! mid-poll error propagation, the `opc` passthrough commands) actually composes, without
//! re-exercising `opcda-bridge`'s own already-tested RPC error-path matrix.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
use opcda_bridge_proto::bridge::{
    BrowsePage, BrowseRequest, CloseBrowseSessionRequest, ControlSearchIndexRequest,
    GetCapabilitiesRequest, GetCapabilitiesResponse, GetSearchIndexStatusRequest,
    ListServersRequest, ListServersResponse, ReadRequest, ReadResponse, RefreshSearchIndexRequest,
    SearchEvent, SearchIndexResponse, SearchIndexStatus, SearchRequest, WriteRequest,
    WriteResponse,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

pub(crate) struct MockBridgeService {
    pub(crate) read_response: ReadResponse,
    pub(crate) write_response: WriteResponse,
    pub(crate) browse_response: BrowsePage,
    pub(crate) list_servers_response: ListServersResponse,
    pub(crate) capabilities_response: GetCapabilitiesResponse,
    pub(crate) search_events: Vec<SearchEvent>,
    pub(crate) search_index_status_response: SearchIndexStatus,
    pub(crate) search_index_response: SearchIndexResponse,
    pub(crate) search_error: Option<Status>,
    /// Once the `read` RPC has been called this many times (1-based) or more, it fails with
    /// a gRPC error instead of returning `read_response` — lets a test simulate a bridge
    /// that works fine during setup and then drops partway through a poll loop.
    pub(crate) fail_read_from_call: Option<u32>,
    // `pub(crate)` (rather than private) purely so call sites can use `..Default::default()`
    // struct-update syntax across the module boundary; every real caller just leaves this at
    // its default and never sets it explicitly.
    pub(crate) read_calls: Arc<AtomicU32>,
    pub(crate) close_browse_session_calls: Arc<AtomicU32>,
}

impl Default for MockBridgeService {
    fn default() -> Self {
        Self {
            read_response: ReadResponse::default(),
            write_response: WriteResponse::default(),
            browse_response: BrowsePage {
                complete: true,
                ..Default::default()
            },
            list_servers_response: ListServersResponse::default(),
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
            search_events: Vec::new(),
            search_index_status_response: SearchIndexStatus::default(),
            search_index_response: SearchIndexResponse::default(),
            search_error: None,
            fail_read_from_call: None,
            read_calls: Arc::new(AtomicU32::new(0)),
            close_browse_session_calls: Arc::new(AtomicU32::new(0)),
        }
    }
}

impl MockBridgeService {
    pub(crate) fn failing_read_from_call(mut self, n: u32) -> Self {
        self.fail_read_from_call = Some(n);
        self
    }
}

#[tonic::async_trait]
impl Bridge for MockBridgeService {
    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<GetCapabilitiesResponse>, Status> {
        Ok(Response::new(self.capabilities_response.clone()))
    }

    async fn list_servers(
        &self,
        _request: Request<ListServersRequest>,
    ) -> Result<Response<ListServersResponse>, Status> {
        Ok(Response::new(self.list_servers_response.clone()))
    }

    async fn browse(
        &self,
        _request: Request<BrowseRequest>,
    ) -> Result<Response<BrowsePage>, Status> {
        Ok(Response::new(self.browse_response.clone()))
    }

    async fn close_browse_session(
        &self,
        _request: Request<CloseBrowseSessionRequest>,
    ) -> Result<Response<()>, Status> {
        self.close_browse_session_calls
            .fetch_add(1, Ordering::SeqCst);
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
        let (tx, rx) = mpsc::channel(4);
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
        _request: Request<GetSearchIndexStatusRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn refresh_search_index(
        &self,
        _request: Request<RefreshSearchIndexRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn control_search_index(
        &self,
        _request: Request<ControlSearchIndexRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn search_index(
        &self,
        _request: Request<opcda_bridge_proto::bridge::SearchIndexRequest>,
    ) -> Result<Response<SearchIndexResponse>, Status> {
        Ok(Response::new(self.search_index_response.clone()))
    }

    async fn read(&self, _request: Request<ReadRequest>) -> Result<Response<ReadResponse>, Status> {
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(fail_from) = self.fail_read_from_call
            && call >= fail_from
        {
            return Err(Status::unavailable("mock bridge: simulated read failure"));
        }
        Ok(Response::new(self.read_response.clone()))
    }

    async fn write(
        &self,
        _request: Request<WriteRequest>,
    ) -> Result<Response<WriteResponse>, Status> {
        Ok(Response::new(self.write_response.clone()))
    }
}

/// A running mock server plus the means to shut it down and wait for it to actually exit.
/// Dropping this without calling [`Self::shutdown`] also stops the server (dropping the
/// sender closes the shutdown channel the same way sending on it would), but only
/// `shutdown` proves the server task's own `.await` on `serve_with_incoming_shutdown`
/// completes and returns rather than being abandoned mid-flight at test-process exit.
pub(crate) struct MockServerHandle {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl MockServerHandle {
    /// Signals the server to stop accepting new work and waits for its task to exit.
    pub(crate) async fn shutdown(self) {
        let _ = self.shutdown.send(());
        self.task.await.unwrap();
    }
}

/// Starts `service` on an OS-assigned loopback port and returns its `host:port` address
/// alongside a handle that can gracefully stop it.
pub(crate) async fn start_mock_server(service: MockBridgeService) -> (String, MockServerHandle) {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        Server::builder()
            .add_service(BridgeServer::new(service))
            .serve_with_incoming_shutdown(
                tokio_stream::wrappers::TcpListenerStream::new(listener),
                async {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .unwrap();
    });
    (
        format!("127.0.0.1:{port}"),
        MockServerHandle {
            shutdown: shutdown_tx,
            task,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_servers_returns_an_empty_response_by_default() {
        let service = MockBridgeService::default();
        let response = service
            .list_servers(Request::new(ListServersRequest::default()))
            .await
            .unwrap();
        assert!(response.into_inner().servers.is_empty());
    }

    #[tokio::test]
    async fn list_servers_returns_the_configured_response() {
        let service = MockBridgeService {
            list_servers_response: ListServersResponse {
                servers: vec!["Matrikon.OPC.Simulation.1".to_string()],
            },
            ..Default::default()
        };
        let response = service
            .list_servers(Request::new(ListServersRequest::default()))
            .await
            .unwrap();
        assert_eq!(
            response.into_inner().servers,
            vec!["Matrikon.OPC.Simulation.1".to_string()]
        );
    }

    #[tokio::test]
    async fn browse_returns_the_configured_page() {
        let service = MockBridgeService {
            browse_response: BrowsePage {
                session_id: "session".to_string(),
                complete: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let page = service
            .browse(Request::new(BrowseRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(page.session_id, "session");
    }

    #[tokio::test]
    async fn start_mock_server_shuts_down_gracefully() {
        let (_host, server) = start_mock_server(MockBridgeService::default()).await;
        server.shutdown().await;
    }
}
