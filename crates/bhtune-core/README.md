# bhtune-core

Pure domain logic for BHTune: the Modified Relay Feedback Test (MRFT) state machine,
PID tuning math, controller and process types, tag mappings, and DCS/PLC templates.

`MrftEngine` is driven by caller-provided ticks. The core crate does not read a clock,
perform network or filesystem I/O, or run an asynchronous executor. Its deterministic
engine and checked tuning-result calculations are suitable for replay and validation.

API documentation: <https://docs.rs/bhtune-core>
