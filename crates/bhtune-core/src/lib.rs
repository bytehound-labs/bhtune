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
//! Deliberately has zero non-`serde` dependencies. Anything else added here
//! must be justified by the pure domain logic itself, not by a consumer's
//! I/O or presentation needs — those belong in `bhtune-backend`,
//! `bhtune-db`, `bhtune-cli`, or `bhtune-desktop`.

pub mod constants;
pub mod controller_type;
pub mod direction;
pub mod loop_config;
pub mod mrft;
pub mod pid_config;
pub mod process_type;
pub mod tags;
pub mod template;
pub mod tuning_math;

pub use constants::{ResponseLevel, TuningConstants, lookup};
pub use controller_type::ControllerType;
pub use direction::ControllerDirection;
pub use loop_config::LoopConfig;
pub use mrft::{Action, InitialReadings, MrftCompat, MrftEngine, MrftState, Tick};
pub use pid_config::{DerivativeType, IntegralType, ProportionalType, TimeUnit};
pub use process_type::ProcessType;
pub use tags::{LoopTags, TagOrValue, derive_tag};
pub use template::{DcsTemplate, built_in_templates};
pub use tuning_math::{
    OpcWriteValues, Oscillation, PidParameters, PvRange, TuningMathCompat, TuningResult,
    calculate_all, calculate_pid_parameters, calculate_tuning_result, measure_oscillation,
    opc_write_values,
};
