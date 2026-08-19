//! `bhtune-driver` — the extensibility seam.
//!
//! Defines a single async [`Driver`] trait abstracting all tag I/O (`read`/`write`/
//! `browse`) so `bhtune-core`'s tuning engine never knows what it is talking to. Three
//! implementations exist:
//!
//! - [`opcda`]: the primary driver for v1 ([`OpcDaDriver`]), over the `opcda-bridge`
//!   crates.io dependency (Windows OPC DA via a network gateway — no COM/DCOM dependency in
//!   this process). Also home to [`list_opcda_servers`], a standalone pre-connection
//!   function for OPC DA server discovery (see that function's doc comment for why it
//!   isn't a `Driver`/`OpcDaDriver` method).
//! - [`simulator`]: an in-process FOPDT (first-order-plus-dead-time) process model
//!   ([`SimulatorDriver`]), used for fully automated E2E tests on CI (no Windows, no
//!   Kepware, no external process) and as a demo mode. Also home to [`VirtualPid`], a
//!   standalone PID controller for closed-loop validation/demos.
//! - [`replay`]: feeds a recorded golden-master trace ([`ReplayDriver`]) back through the
//!   engine, for regression validation — see that module's doc comment for why this
//!   complements, rather than duplicates, `core-replay-harness`'s pure-engine parity proof.
//!
//! `OpcUaDriver` and `ModbusDriver` are roadmap items (see AGENTS.md) that should slot in
//! later without requiring any changes to `bhtune-core`.
//!
//! - [`driver`] — the [`Driver`] trait itself.
//! - [`types`] — the plain data types ([`TagId`], [`TagValue`], [`TagWrite`],
//!   [`WriteOutcome`], [`TagNode`]) that cross the trait boundary.
//! - [`error`] — the crate's error type, [`DriverError`].
//! - [`opcda`] — [`OpcDaDriver`], the OPC DA implementation, and [`list_opcda_servers`].
//! - [`simulator`] — [`SimulatorDriver`], [`FopdtProcess`]/[`FopdtConfig`], and
//!   [`VirtualPid`]/[`VirtualPidConfig`].
//! - [`replay`] — [`ReplayDriver`], [`ReplaySample`], and [`RecordedWrite`].

pub mod driver;
pub mod error;
pub mod opcda;
pub mod replay;
pub mod simulator;
pub mod types;

pub use driver::Driver;
pub use error::{DriverError, DriverResult};
pub use opcda::{OpcDaDriver, list_opcda_servers};
pub use replay::{RecordedWrite, ReplayDriver, ReplaySample, ReplayTraceExhausted};
pub use simulator::{FopdtConfig, FopdtProcess, SimulatorDriver, VirtualPid, VirtualPidConfig};
pub use types::{Quality, TagId, TagNode, TagValue, TagWrite, WriteOutcome};
