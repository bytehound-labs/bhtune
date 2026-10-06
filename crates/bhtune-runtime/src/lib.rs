//! Shared application runtime for BHTune's command-line and HTTP adapters.
//!
//! This crate owns configuration resolution, database bootstrap, logging, history retention,
//! driver setup, tune orchestration and safety, shared OPC helpers, and sample export
//! serialization. Its source and direct dependencies do not use `clap` or an HTTP framework;
//! adapters convert their own inputs into runtime types and keep transport-specific output at
//! the boundary. The OPC DA gRPC client may bring transport crates transitively, but they are
//! not exposed through this runtime API.

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit tests may use unwrap, expect, and panic; production code must return typed errors"
    )
)]

pub mod cancel;
pub mod config;
pub mod db;
pub mod driver;
pub mod export;
pub mod gateway;
pub mod history;
pub mod live_ownership;
pub mod logging;
pub mod retention;
pub mod tune;

#[cfg(test)]
mod test_support;
mod timing;
