//! Unpublished shared test doubles for BHTune.
//!
//! This crate is not a product crate and is not published or packaged as a release
//! artifact. The mock bridge depends only on the OPC DA protobuf contract, so the
//! driver, CLI, and server tests can share one implementation without a dependency cycle.
//!
//! The `mock-driver` feature is an empty cycle guard. The in-process CLI and driver
//! `MockDriver` doubles stay in the crates that use them.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests may use unwrap, expect, and panic; production code must return typed errors"
    )
)]

mod mock_bridge;

pub use mock_bridge::{MockBridgeService, MockServerHandle, start_mock_server};
