//! `bhtune-core` — the pure, I/O-free domain crate.
//!
//! This crate holds (once implemented — see `AGENTS.md` for phase status):
//! - The MRFT (Modified Relay Feedback Test) tuning algorithm, as a pure
//!   state machine: `fn step(&mut self, tick: Tick) -> Vec<Action>`. No
//!   clock reads, no network calls, no UI access — that constraint is what
//!   makes it possible to replay a recorded trace and assert the engine
//!   produces bit-identical results across changes.
//! - The tuning-constant lookup matrices and PID unit conversion math.
//! - The domain data model: tag maps, loop configuration, DCS/PLC template
//!   semantics, and the enums that model PID parameter types and controller
//!   direction.
//!
//! Deliberately has zero dependencies today. Anything added here must be
//! justified by the pure domain logic itself, not by a consumer's I/O or
//! presentation needs — those belong in `bhtune-backend`, `bhtune-db`,
//! `bhtune-cli`, or `bhtune-desktop`.
