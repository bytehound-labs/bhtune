//! `bhtune-backend` — the extensibility seam.
//!
//! Defines a single async [`Backend`] trait abstracting all tag I/O (`read`/`write`/
//! `browse`) so `bhtune-core`'s tuning engine never knows what it is talking to. Three
//! implementations are planned:
//!
//! - [`opcda`]: the primary driver for v1 ([`OpcDaBackend`]), over the `opcda-bridge`
//!   crates.io dependency (Windows OPC DA via a network gateway — no COM/DCOM dependency in
//!   this process).
//! - [`simulator`]: an in-process FOPDT (first-order-plus-dead-time) process model
//!   ([`SimulatorBackend`]), used for fully automated E2E tests on CI (no Windows, no
//!   Kepware, no external process) and as a demo mode. Also home to [`VirtualPid`], a
//!   standalone PID controller for closed-loop validation/demos.
//! - `replay`: feeds a recorded golden-master trace back through the engine, for regression
//!   validation.
//!
//! `OpcUaBackend` and `ModbusBackend` are roadmap items (see AGENTS.md) that should slot in
//! later without requiring any changes to `bhtune-core`.
//!
//! - [`backend`] — the [`Backend`] trait itself.
//! - [`types`] — the plain data types ([`TagId`], [`TagValue`], [`TagWrite`],
//!   [`WriteOutcome`], [`TagNode`]) that cross the trait boundary.
//! - [`error`] — the crate's error type, [`BackendError`].
//! - [`opcda`] — [`OpcDaBackend`], the OPC DA implementation.
//! - [`simulator`] — [`SimulatorBackend`], [`FopdtProcess`]/[`FopdtConfig`], and
//!   [`VirtualPid`]/[`VirtualPidConfig`].
//!
//! Not yet implemented: `replay` — this crate has the trait, its supporting types, and the
//! OPC DA and simulator backends only, until `backend-replay` lands.

pub mod backend;
pub mod error;
pub mod opcda;
pub mod simulator;
pub mod types;

pub use backend::Backend;
pub use error::{BackendError, BackendResult};
pub use opcda::OpcDaBackend;
pub use simulator::{FopdtConfig, FopdtProcess, SimulatorBackend, VirtualPid, VirtualPidConfig};
pub use types::{Quality, TagId, TagNode, TagValue, TagWrite, WriteOutcome};
