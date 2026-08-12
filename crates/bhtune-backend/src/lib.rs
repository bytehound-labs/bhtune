//! `bhtune-backend` — the extensibility seam.
//!
//! Defines a single async [`Backend`] trait abstracting all tag I/O (`read`/`write`/
//! `browse`) so `bhtune-core`'s tuning engine never knows what it is talking to. Three
//! implementations are planned:
//!
//! - `opcda`: the primary driver for v1, over a reusable client library published as the
//!   `opcda-bridge` crates.io dependency (Windows OPC DA via a network gateway — no
//!   COM/DCOM dependency in this process).
//! - `simulator`: an in-process FOPDT (first-order-plus-dead-time) process model with a
//!   virtual PID controller, used for fully automated E2E tests on CI (no Windows, no
//!   Kepware, no external process) and as a demo mode.
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
//!
//! Not yet implemented: `opcda`/`simulator`/`replay` themselves — this crate has the trait
//! and its supporting types only, until `backend-opcda`/`backend-simulator`/`backend-replay`
//! land.

pub mod backend;
pub mod error;
pub mod types;

pub use backend::Backend;
pub use error::{BackendError, BackendResult};
pub use types::{Quality, TagId, TagNode, TagValue, TagWrite, WriteOutcome};
