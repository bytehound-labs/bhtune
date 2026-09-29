//! One mock gRPC `Bridge` for driver, CLI, and server tests.
//!
//! The field set is the union of the three copies this replaced. Defaults match the CLI and
//! server mocks: browse pages are complete, and capabilities advertise the 0.4.0/protocol-2
//! surface. Gateway info stays empty, so a caller that classifies an empty metadata response
//! as unknown still does. A read response whose single value has tag id `ignored` is expanded
//! to the requested tag ids; that is the CLI behavior, and it is a no-op unless a test sets
//! that exact tag id.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
use opcda_bridge_proto::bridge::{
    BrowsePage, BrowseRequest, CloseBrowseSessionRequest, ControlSearchIndexRequest,
    GetCapabilitiesRequest, GetCapabilitiesResponse, GetGatewayInfoRequest, GetGatewayInfoResponse,
    GetSearchIndexStatusRequest, ListServersRequest, ListServersResponse, ReadRequest,
    ReadResponse, RefreshSearchIndexRequest, SearchEvent, SearchIndexRequest, SearchIndexResponse,
    SearchIndexStatus, SearchRequest, TagValue as ProtoTagValue, WriteRequest, WriteResponse,
};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};
use tonic::transport::Server;
use tonic::{Request, Response, Status};

/// Configurable mock `Bridge` service.
///
/// Every field is public so a test can set one value and fill the rest with
/// `..Default::default()`. Counters and request logs are shared `Arc`s so a test can
/// observe them after the service has moved into the server task.
#[derive(Clone)]
pub struct MockBridgeService {
    /// Returned by `read` unless [`Self::read_error`] or [`Self::fail_read_from_call`] fires.
    /// A single value whose `tag_id` is `ignored` is cloned once per requested tag id.
    pub read_response: ReadResponse,
    /// Returned by `write` unless [`Self::write_error`] is set. `success: false` is a rejected
    /// write, not an RPC error.
    pub write_response: WriteResponse,
    /// Returned by `browse` when the request has no page token, and as the fallback when a
    /// continuation page was not configured.
    pub browse_response: BrowsePage,
    /// Returned by `browse` when the request carries a page token.
    pub browse_continuation_response: Option<BrowsePage>,
    /// Returned by `list_servers` unless [`Self::list_servers_error`] is set.
    pub list_servers_response: ListServersResponse,
    /// Returned by `get_capabilities` unless [`Self::capabilities_error`] is set.
    pub capabilities_response: GetCapabilitiesResponse,
    /// Returned by `get_gateway_info` unless [`Self::gateway_info_error`] is set.
    pub gateway_info_response: GetGatewayInfoResponse,
    /// Events forwarded, in order, by `search`.
    pub search_events: Vec<SearchEvent>,
    /// Returned by the indexed-search status, refresh, and control RPCs on success.
    pub search_index_status_response: SearchIndexStatus,
    /// Returned by `search_index` on success.
    pub search_index_response: SearchIndexResponse,
    /// RPC error for `search`, checked before any event is sent.
    pub search_error: Option<Status>,
    /// 1-based `read` call number at which later reads fail. Earlier reads still succeed.
    pub fail_read_from_call: Option<u32>,
    /// Count of `read` calls, including calls that then fail.
    pub read_calls: Arc<AtomicU32>,
    /// Count of `close_browse_session` calls that did not return [`Self::close_error`].
    pub close_browse_session_calls: Arc<AtomicU32>,
    /// RPC error for `list_servers`.
    pub list_servers_error: Option<Status>,
    /// RPC error for `browse`, checked before page-token selection.
    pub browse_error: Option<Status>,
    /// RPC error for `get_capabilities`.
    pub capabilities_error: Option<Status>,
    /// RPC error for `get_gateway_info`.
    pub gateway_info_error: Option<Status>,
    /// Recorded `get_search_index_status` requests, including calls that then fail.
    pub search_index_status_requests: Arc<Mutex<Vec<GetSearchIndexStatusRequest>>>,
    /// RPC error for `get_search_index_status`, returned after the request is recorded.
    pub search_index_status_error: Option<Status>,
    /// Recorded `search_index` requests, including calls that then fail.
    pub search_index_requests: Arc<Mutex<Vec<SearchIndexRequest>>>,
    /// RPC error for `search_index`, returned after the request is recorded.
    pub search_index_error: Option<Status>,
    /// Recorded `refresh_search_index` requests, including calls that then fail.
    pub refresh_search_index_requests: Arc<Mutex<Vec<RefreshSearchIndexRequest>>>,
    /// RPC error for `refresh_search_index`, returned after the request is recorded.
    pub refresh_search_index_error: Option<Status>,
    /// Recorded `control_search_index` requests, including calls that then fail.
    pub control_search_index_requests: Arc<Mutex<Vec<ControlSearchIndexRequest>>>,
    /// RPC error for `control_search_index`, returned after the request is recorded.
    pub control_search_index_error: Option<Status>,
    /// RPC error for `read`. Checked before [`Self::fail_read_from_call`], so both can be set.
    pub read_error: Option<Status>,
    /// RPC error for `write`. Distinct from a `success: false` [`Self::write_response`].
    pub write_error: Option<Status>,
    /// RPC error for `close_browse_session`. A failed close is not counted.
    pub close_error: Option<Status>,
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
            browse_continuation_response: None,
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
            gateway_info_response: GetGatewayInfoResponse::default(),
            search_events: Vec::new(),
            search_index_status_response: SearchIndexStatus::default(),
            search_index_response: SearchIndexResponse::default(),
            search_error: None,
            fail_read_from_call: None,
            read_calls: Arc::new(AtomicU32::new(0)),
            close_browse_session_calls: Arc::new(AtomicU32::new(0)),
            list_servers_error: None,
            browse_error: None,
            capabilities_error: None,
            gateway_info_error: None,
            search_index_status_requests: Arc::new(Mutex::new(Vec::new())),
            search_index_status_error: None,
            search_index_requests: Arc::new(Mutex::new(Vec::new())),
            search_index_error: None,
            refresh_search_index_requests: Arc::new(Mutex::new(Vec::new())),
            refresh_search_index_error: None,
            control_search_index_requests: Arc::new(Mutex::new(Vec::new())),
            control_search_index_error: None,
            read_error: None,
            write_error: None,
            close_error: None,
        }
    }
}

impl MockBridgeService {
    /// Fails `read` on the 1-based call `n` and every call after it.
    #[must_use]
    pub fn failing_read_from_call(mut self, n: u32) -> Self {
        self.fail_read_from_call = Some(n);
        self
    }
}

#[tonic::async_trait]
impl Bridge for MockBridgeService {
    async fn get_gateway_info(
        &self,
        _request: Request<GetGatewayInfoRequest>,
    ) -> Result<Response<GetGatewayInfoResponse>, Status> {
        if let Some(status) = self.gateway_info_error.clone() {
            return Err(status);
        }
        Ok(Response::new(self.gateway_info_response.clone()))
    }

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
        request: Request<BrowseRequest>,
    ) -> Result<Response<BrowsePage>, Status> {
        if let Some(status) = self.browse_error.clone() {
            return Err(status);
        }
        let response = if request.into_inner().page_token.is_some() {
            self.browse_continuation_response
                .clone()
                .unwrap_or_else(|| self.browse_response.clone())
        } else {
            self.browse_response.clone()
        };
        Ok(Response::new(response))
    }

    async fn close_browse_session(
        &self,
        _request: Request<CloseBrowseSessionRequest>,
    ) -> Result<Response<()>, Status> {
        if let Some(status) = self.close_error.clone() {
            return Err(status);
        }
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
        drop(tokio::spawn(forward_search_events(
            self.search_events.clone(),
            tx,
        )));
        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn get_search_index_status(
        &self,
        request: Request<GetSearchIndexStatusRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        record(&self.search_index_status_requests, request.into_inner());
        if let Some(status) = self.search_index_status_error.clone() {
            return Err(status);
        }
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn refresh_search_index(
        &self,
        request: Request<RefreshSearchIndexRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        record(&self.refresh_search_index_requests, request.into_inner());
        if let Some(status) = self.refresh_search_index_error.clone() {
            return Err(status);
        }
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn control_search_index(
        &self,
        request: Request<ControlSearchIndexRequest>,
    ) -> Result<Response<SearchIndexStatus>, Status> {
        record(&self.control_search_index_requests, request.into_inner());
        if let Some(status) = self.control_search_index_error.clone() {
            return Err(status);
        }
        Ok(Response::new(self.search_index_status_response.clone()))
    }

    async fn search_index(
        &self,
        request: Request<SearchIndexRequest>,
    ) -> Result<Response<SearchIndexResponse>, Status> {
        record(&self.search_index_requests, request.into_inner());
        if let Some(status) = self.search_index_error.clone() {
            return Err(status);
        }
        Ok(Response::new(self.search_index_response.clone()))
    }

    async fn read(&self, request: Request<ReadRequest>) -> Result<Response<ReadResponse>, Status> {
        let call = self.read_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(status) = self.read_error.clone() {
            return Err(status);
        }
        if let Some(fail_from) = self.fail_read_from_call
            && call >= fail_from
        {
            return Err(Status::unavailable("mock bridge: simulated read failure"));
        }
        let request = request.into_inner();
        let mut response = self.read_response.clone();
        if response.values.len() == 1 && response.values[0].tag_id == "ignored" {
            let template = response.values[0].clone();
            response.values = request
                .tag_ids
                .into_iter()
                .map(|tag_id| ProtoTagValue {
                    tag_id,
                    ..template.clone()
                })
                .collect();
        }
        Ok(Response::new(response))
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

/// A running mock server and the signal that stops it.
///
/// Keep this value alive for as long as the test needs the server. Dropping it closes the
/// shutdown channel, which stops the server, but only [`Self::shutdown`] waits for the
/// server task to finish. Binding the handle to `_` drops it immediately.
#[must_use = "dropping the handle stops the mock server"]
pub struct MockServerHandle {
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl MockServerHandle {
    /// Stops the server and waits until its task has exited.
    ///
    /// A panicked or failed server task is not re-raised here. The test that needed the
    /// server observes that failure through its own connection or assertion.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.task.await;
    }
}

/// Binds `service` to an ephemeral loopback port.
///
/// The returned address is `127.0.0.1:<port>`. The handle must stay alive until the test
/// is done with the server; call [`MockServerHandle::shutdown`] to wait for it to exit.
pub async fn start_mock_server(
    service: MockBridgeService,
) -> std::io::Result<(String, MockServerHandle)> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = Server::builder()
            .add_service(BridgeServer::new(service))
            .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Ok((
        format!("127.0.0.1:{port}"),
        MockServerHandle {
            shutdown: shutdown_tx,
            task,
        },
    ))
}

async fn forward_search_events(
    events: Vec<SearchEvent>,
    tx: mpsc::Sender<Result<SearchEvent, Status>>,
) {
    for event in events {
        if tx.send(Ok(event)).await.is_err() {
            break;
        }
    }
}

fn record<T>(recorder: &Mutex<Vec<T>>, value: T) {
    recorder
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(value);
}

#[cfg(test)]
mod tests {
    use super::{
        BrowsePage, BrowseRequest, CloseBrowseSessionRequest, ControlSearchIndexRequest,
        GetCapabilitiesRequest, GetCapabilitiesResponse, GetGatewayInfoRequest,
        GetGatewayInfoResponse, GetSearchIndexStatusRequest, ListServersRequest,
        ListServersResponse, MockBridgeService, PoisonError, ReadRequest, ReadResponse,
        RefreshSearchIndexRequest, SearchEvent, SearchIndexRequest, SearchIndexStatus,
        SearchRequest, Status, WriteRequest, WriteResponse, forward_search_events, record,
        start_mock_server,
    };
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    use tonic::Request;

    use opcda_bridge_proto::bridge::bridge_server::Bridge;

    fn configured_error() -> Status {
        Status::unavailable("configured")
    }

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
    async fn browse_returns_the_configured_page_without_a_token() {
        let service = MockBridgeService {
            browse_response: BrowsePage {
                session_id: "session".to_string(),
                complete: true,
                ..Default::default()
            },
            browse_continuation_response: Some(BrowsePage {
                session_id: "not-this".to_string(),
                ..Default::default()
            }),
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
    async fn browse_uses_the_continuation_page_when_one_is_configured() {
        let service = MockBridgeService {
            browse_continuation_response: Some(BrowsePage {
                session_id: "continued".to_string(),
                complete: false,
                ..Default::default()
            }),
            ..Default::default()
        };
        let page = service
            .browse(Request::new(BrowseRequest {
                page_token: Some("token".into()),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(page.session_id, "continued");
        assert!(!page.complete);
    }

    #[tokio::test]
    async fn browse_falls_back_when_a_token_has_no_continuation_page() {
        let service = MockBridgeService::default();
        let page = service
            .browse(Request::new(BrowseRequest {
                page_token: Some("token".into()),
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(page.complete);
    }

    #[tokio::test]
    async fn default_capabilities_match_the_cli_and_server_mocks() {
        let service = MockBridgeService::default();
        let capabilities = service
            .get_capabilities(Request::new(GetCapabilitiesRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            capabilities,
            GetCapabilitiesResponse {
                application_version: "0.4.0".to_string(),
                protocol_version: "2".to_string(),
                max_page_size: 1000,
                supports_browse_sessions: true,
                supports_search: true,
                supports_indexed_search: true,
                indexed_search_protocol_version: "1".to_string(),
                max_indexed_search_results: 50,
                ..Default::default()
            }
        );
        assert!(service.gateway_info_response.application_version.is_empty());
    }

    #[tokio::test]
    async fn gateway_info_returns_the_configured_response() {
        let service = MockBridgeService {
            gateway_info_response: GetGatewayInfoResponse {
                application_version: "test-gateway".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let response = service
            .get_gateway_info(Request::new(GetGatewayInfoRequest::default()))
            .await
            .unwrap();
        assert_eq!(response.into_inner().application_version, "test-gateway");
    }

    #[tokio::test]
    async fn configured_rpc_errors_win_over_the_success_response() {
        let error = configured_error();
        let service = MockBridgeService {
            gateway_info_error: Some(error.clone()),
            capabilities_error: Some(error.clone()),
            list_servers_error: Some(error.clone()),
            browse_error: Some(error.clone()),
            search_error: Some(error.clone()),
            read_error: Some(error.clone()),
            write_error: Some(error.clone()),
            close_error: Some(error.clone()),
            fail_read_from_call: Some(1),
            ..Default::default()
        };
        assert_eq!(
            service
                .get_gateway_info(Request::new(GetGatewayInfoRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .get_capabilities(Request::new(GetCapabilitiesRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .list_servers(Request::new(ListServersRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .browse(Request::new(BrowseRequest {
                    page_token: Some("token".into()),
                    ..Default::default()
                }))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .search(Request::new(SearchRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .read(Request::new(ReadRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(service.read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            service
                .write(Request::new(WriteRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(
            service
                .close_browse_session(Request::new(CloseBrowseSessionRequest::default()))
                .await
                .unwrap_err()
                .message(),
            "configured"
        );
        assert_eq!(service.close_browse_session_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn close_browse_session_counts_only_successful_calls() {
        let service = MockBridgeService::default();
        service
            .close_browse_session(Request::new(CloseBrowseSessionRequest::default()))
            .await
            .unwrap();
        assert_eq!(service.close_browse_session_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn read_fails_from_the_configured_call_and_expands_ignored_tag_ids() {
        let service = MockBridgeService {
            read_response: ReadResponse {
                values: vec![opcda_bridge_proto::bridge::TagValue {
                    tag_id: "ignored".to_string(),
                    value: "10.0".to_string(),
                    quality: "Good".to_string(),
                    timestamp: String::new(),
                }],
            },
            ..Default::default()
        }
        .failing_read_from_call(2);
        let first = service
            .read(Request::new(ReadRequest {
                tag_ids: vec!["PV".to_string(), "MV".to_string()],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            first
                .values
                .iter()
                .map(|value| value.tag_id.as_str())
                .collect::<Vec<_>>(),
            vec!["PV", "MV"]
        );
        assert!(
            first
                .values
                .iter()
                .all(|value| value.value == "10.0" && value.quality == "Good")
        );
        let empty = service
            .read(Request::new(ReadRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(empty.message(), "mock bridge: simulated read failure");
        let later = service
            .read(Request::new(ReadRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(later.message(), "mock bridge: simulated read failure");
    }

    #[tokio::test]
    async fn read_leaves_a_real_tag_id_unchanged() {
        let service = MockBridgeService {
            read_response: ReadResponse {
                values: vec![opcda_bridge_proto::bridge::TagValue {
                    tag_id: "Area.PV".to_string(),
                    value: "1".to_string(),
                    quality: "Good".to_string(),
                    timestamp: String::new(),
                }],
            },
            ..Default::default()
        };
        let response = service
            .read(Request::new(ReadRequest {
                tag_ids: vec!["Other".to_string()],
                ..Default::default()
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(response.values[0].tag_id, "Area.PV");
    }

    #[tokio::test]
    async fn write_returns_a_rejected_write_without_an_rpc_error() {
        let service = MockBridgeService {
            write_response: WriteResponse {
                tag_id: "Area.MV".to_string(),
                success: false,
                error: Some("tag is read-only".to_string()),
            },
            ..Default::default()
        };
        let response = service
            .write(Request::new(WriteRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert!(!response.success);
        assert_eq!(response.error.as_deref(), Some("tag is read-only"));
    }

    #[tokio::test]
    async fn indexed_search_records_the_request_before_returning_an_error() {
        let status_requests = Arc::new(Mutex::new(Vec::new()));
        let query_requests = Arc::new(Mutex::new(Vec::new()));
        let refresh_requests = Arc::new(Mutex::new(Vec::new()));
        let control_requests = Arc::new(Mutex::new(Vec::new()));
        let error = configured_error();
        let service = MockBridgeService {
            search_index_status_requests: Arc::clone(&status_requests),
            search_index_status_error: Some(error.clone()),
            search_index_requests: Arc::clone(&query_requests),
            search_index_error: Some(error.clone()),
            refresh_search_index_requests: Arc::clone(&refresh_requests),
            refresh_search_index_error: Some(error.clone()),
            control_search_index_requests: Arc::clone(&control_requests),
            control_search_index_error: Some(error),
            ..Default::default()
        };
        assert!(
            service
                .get_search_index_status(Request::new(GetSearchIndexStatusRequest::default()))
                .await
                .is_err()
        );
        assert!(
            service
                .search_index(Request::new(SearchIndexRequest::default()))
                .await
                .is_err()
        );
        assert!(
            service
                .refresh_search_index(Request::new(RefreshSearchIndexRequest::default()))
                .await
                .is_err()
        );
        assert!(
            service
                .control_search_index(Request::new(ControlSearchIndexRequest::default()))
                .await
                .is_err()
        );
        assert_eq!(status_requests.lock().unwrap().len(), 1);
        assert_eq!(query_requests.lock().unwrap().len(), 1);
        assert_eq!(refresh_requests.lock().unwrap().len(), 1);
        assert_eq!(control_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn indexed_search_returns_the_configured_success_response() {
        let service = MockBridgeService {
            search_index_status_response: SearchIndexStatus {
                server: "S1".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let status = service
            .get_search_index_status(Request::new(GetSearchIndexStatusRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.server, "S1");
        let refreshed = service
            .refresh_search_index(Request::new(RefreshSearchIndexRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(refreshed.server, "S1");
        let controlled = service
            .control_search_index(Request::new(ControlSearchIndexRequest::default()))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(controlled.server, "S1");
        assert!(
            service
                .search_index(Request::new(SearchIndexRequest::default()))
                .await
                .is_ok()
        );
        assert_eq!(
            service.search_index_status_requests.lock().unwrap().len(),
            1
        );
        assert_eq!(
            service.refresh_search_index_requests.lock().unwrap().len(),
            1
        );
        assert_eq!(
            service.control_search_index_requests.lock().unwrap().len(),
            1
        );
        assert_eq!(service.search_index_requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn search_forwards_events_and_stops_when_the_receiver_is_dropped() {
        let (tx, mut rx) = mpsc::channel(4);
        forward_search_events(vec![SearchEvent::default()], tx).await;
        assert!(rx.recv().await.unwrap().is_ok());

        let (dropped_tx, dropped_rx) = mpsc::channel(4);
        drop(dropped_rx);
        forward_search_events(
            vec![SearchEvent::default(), SearchEvent::default()],
            dropped_tx,
        )
        .await;
    }

    #[tokio::test]
    async fn search_stream_can_be_collected_from_the_rpc() {
        let service = MockBridgeService {
            search_events: vec![SearchEvent::default()],
            ..Default::default()
        };
        let mut receiver = service
            .search(Request::new(SearchRequest::default()))
            .await
            .unwrap()
            .into_inner()
            .into_inner();
        assert!(receiver.recv().await.unwrap().is_ok());
        assert!(receiver.recv().await.is_none());
    }

    #[test]
    fn record_recovers_a_poisoned_request_log() {
        let recorder = Mutex::new(Vec::<u32>::new());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = recorder.lock().unwrap();
            panic!("poison the request log");
        }));
        assert!(recorder.is_poisoned());
        record(&recorder, 7);
        let values = recorder.lock().unwrap_or_else(PoisonError::into_inner);
        assert_eq!(*values, vec![7]);
    }

    #[tokio::test]
    async fn start_mock_server_shuts_down_gracefully() {
        let (_host, server) = start_mock_server(MockBridgeService::default())
            .await
            .unwrap();
        server.shutdown().await;
    }
}
