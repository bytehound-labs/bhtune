---
sidebar_position: 3
---

# MRFT engine and time model

`bhtune-core` contains the MRFT algorithm as a deterministic state machine. The caller supplies
a `Tick` containing a timestamp and process-variable (PV) value to
`MrftEngine::step(Tick)`. The engine returns actions, such as a manipulated-variable (MV)
command or a completion result; the caller performs I/O and supplies the next tick.

The engine does not read a wall clock or perform network, database, or user-interface work.
Clock access is excluded from the core crate's dependency features, so time enters the
algorithm as data rather than as an implicit process-wide source. The core uses `f32` for
process values and tuning calculations, matching the single-precision analog values used at
the control-system boundary.

## Timestamp sources

| Run type | Source of the next tick time | Effect |
| --- | --- | --- |
| Simulator | A fixed step equal to the configured poll interval | Each simulator PV read advances the FOPDT process by the same interval used to advance MRFT time. Host scheduling changes how long a run takes, not the simulated time series. |
| Live OPC DA | Monotonic elapsed time projected onto the run's UTC start time | Actual scheduling and driver delays remain visible in sample gaps, while NTP or manual wall-clock changes cannot move the run's elapsed time backward or forward. |
| Recorded replay | Timestamps in the recorded trace | Replay tests feed historical ticks through the same engine without making a live connection. |

The UTC timestamp is the representation stored with samples and passed into the engine. For a
live run, the CLI pairs the run's UTC start with a monotonic instant and adds measured elapsed
time to that anchor for each successful poll. This is clock-jump-resistant timing, not
hard-real-time control: an overloaded host or a slow gateway can still delay polling.

For the relay behavior and the interpretation of its measurements, see
[MRFT concepts](../guides/mrft-concepts.md). The simulator's process model and fixed-step
behavior are described in the [simulator model guide](../guides/simulator-model.md).
