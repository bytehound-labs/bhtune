//! `bhtune-backend` — the extensibility seam.
//!
//! Defines a single async `Backend` trait abstracting all tag I/O
//! (`read`/`write`/`browse`) so `bhtune-core`'s tuning engine never knows
//! what it is talking to. Three implementations are planned:
//!
//! - `opcda`: the primary driver for v1, over a reusable client library
//!   extracted from the sibling `opcda-bridge` project (Windows OPC DA via a
//!   network gateway — no COM/DCOM dependency in this process).
//! - `simulator`: an in-process FOPDT (first-order-plus-dead-time) process
//!   model with a virtual PID controller, used for fully automated E2E tests
//!   on CI (no Windows, no Kepware, no external process) and as a demo mode.
//! - `replay`: feeds a recorded golden-master trace back through the
//!   engine, for regression validation.
//!
//! `OpcUaBackend` and `ModbusBackend` are roadmap items (see AGENTS.md) that
//! should slot in later without requiring changes to `bhtune-core`.
//!
//! Not yet implemented — this crate is scaffolding only until the
//! `backend-trait` phase.
