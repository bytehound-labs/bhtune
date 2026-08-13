//! Test-only mock gRPC `Bridge` service, shared by [`crate::backend`] and
//! [`crate::commands::tune`]'s tests that need a real (mock) OPC DA bridge for
//! [`bhtune_backend::OpcDaBackend`] to connect to — proving the CLI's own OPC DA wiring
//! (connect success path, backend-kind bookkeeping, mid-poll error propagation) actually
//! composes, without re-exercising `opcda-bridge`'s own already-tested RPC error-path matrix.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use opcda_bridge_proto::bridge::bridge_server::{Bridge, BridgeServer};
use opcda_bridge_proto::bridge::{
    BrowseRequest, BrowseResponse, ListServersRequest, ListServersResponse, ReadRequest,
    ReadResponse, WriteRequest, WriteResponse,
};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

#[derive(Default)]
pub(crate) struct MockBridgeService {
    pub(crate) read_response: ReadResponse,
    pub(crate) write_response: WriteResponse,
    /// Once the `read` RPC has been called this many times (1-based) or more, it fails with
    /// a gRPC error instead of returning `read_response` — lets a test simulate a bridge
    /// that works fine during setup and then drops partway through a poll loop.
    pub(crate) fail_read_from_call: Option<u32>,
    // `pub(crate)` (rather than private) purely so call sites can use `..Default::default()`
    // struct-update syntax across the module boundary; every real caller just leaves this at
    // its default and never sets it explicitly.
    pub(crate) read_calls: Arc<AtomicU32>,
}

impl MockBridgeService {
    pub(crate) fn failing_read_from_call(mut self, n: u32) -> Self {
        self.fail_read_from_call = Some(n);
        self
    }
}

#[tonic::async_trait]
impl Bridge for MockBridgeService {
    // `list_servers`/`browse` are never called by any test that uses this shared mock —
    // `bhtune-cli`'s OPC DA support only exercises `Read`/`Write` (setup reads, poll-loop
    // reads, and the mode-revert write). Kept minimal rather than mirroring `opc.rs`'s own
    // fuller mock, which does need real (streaming) browse support for its own tests.
    async fn list_servers(
        &self,
        _request: Request<ListServersRequest>,
    ) -> Result<Response<ListServersResponse>, Status> {
        Ok(Response::new(ListServersResponse::default()))
    }

    type BrowseStream = ReceiverStream<Result<BrowseResponse, Status>>;

    async fn browse(
        &self,
        _request: Request<BrowseRequest>,
    ) -> Result<Response<Self::BrowseStream>, Status> {
        Err(Status::unimplemented(
            "mock bridge: browse is not used by bhtune-cli's tests",
        ))
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

/// Starts `service` on an OS-assigned loopback port and returns its `host:port` address.
pub(crate) async fn start_mock_server(service: MockBridgeService) -> String {
    let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        Server::builder()
            .add_service(BridgeServer::new(service))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("127.0.0.1:{port}")
}
