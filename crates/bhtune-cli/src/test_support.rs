//! Shared mock gRPC `Bridge` used by driver, history, OPC, and tune tests.
//!
//! The implementation lives in `bhtune-test-support`. This module keeps the
//! existing `crate::test_support` import path and turns a bind failure into a
//! test panic so callers can keep destructuring `(host, server)`.

pub(crate) use bhtune_test_support::{MockBridgeService, MockServerHandle};

/// Binds `service` on an ephemeral localhost port and returns the address plus
/// a handle that shuts the server down.
pub(crate) async fn start_mock_server(service: MockBridgeService) -> (String, MockServerHandle) {
    bhtune_test_support::start_mock_server(service)
        .await
        .expect("bind mock bridge")
}
