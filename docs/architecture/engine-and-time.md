---
sidebar_position: 3
---

# MRFT engine and time model

`bhtune-core` contains the MRFT algorithm as a deterministic state machine. The caller supplies
a `Tick` containing a timestamp and process-variable (PV) value to
`MrftEngine::step(Tick)`. The engine returns actions, such as a manipulated-variable (MV)
command or a completion result; the caller performs I/O and supplies the next tick.

The engine does not read a wall clock or perform network, database, or user-interface work.
Switch decisions and recorded switch times reuse the supplied tick timestamp; evaluation does
not take another clock reading. The core uses `f32` for process values and tuning calculations,
matching the single-precision analog values used at the control-system boundary.

The core's standalone default dependency graph does not request chrono's `clock`/`now` features.
Application dependencies such as SQLx enable them through Cargo feature unification in a
workspace build, however. Timestamp injection is the engine's source/API contract, not a
workspace-wide compile-time clock barrier.

## Timestamp sources

| Run type        | Source of the next tick time                                   | Effect                                                                                                                                                                         |
| --------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Simulator       | A fixed step equal to the configured poll interval             | Each simulator PV read advances the FOPDT process by the same interval used to advance MRFT time. Host scheduling changes how long a run takes, not the simulated time series. |
| Live OPC DA     | Monotonic elapsed time projected onto the run's UTC start time | Actual scheduling and driver delays remain visible in sample gaps, while NTP or manual wall-clock changes cannot move the run's elapsed time backward or forward.              |
| Recorded replay | Timestamps in the recorded trace                               | Replay tests feed historical ticks through the same engine without making a live connection.                                                                                   |

The UTC timestamp is the representation stored with samples and passed into the engine. For a
live run, the shared runtime pairs the run's UTC start with a monotonic instant and captures
elapsed time after each successful PV read. CLI and HTTP-started tunes use the same anchor.
An OPC DA tag's timestamp is not engine time; that driver reports `None` for it.

This is clock-jump-resistant timing, not hard-real-time control: an overloaded host or a slow
gateway can still delay polling. Live scheduling delays the next poll rather than issuing a
burst of catch-up operations. Oscillation periods use full elapsed milliseconds, without
whole-second truncation or a 24-hour wrap. Wall-clock run and operation budgets still apply
to simulator runs even though their model time advances in fixed steps.

## Switching and measurement

Hysteresis extrema decide when to switch the relay. Separate result extrema measure the
oscillation used for tuning. A switch sample belongs to the measurement interval it closes
and seeds the next result interval; resetting that result tracker to the initial PV would
lose a shared endpoint when polling is sparse.

The hysteresis trackers retain their established reset to the initial PV. Separating the
trackers corrects measurement without changing relay switching. Changing the hysteresis reset
as well, called Variant B in the research record, is not production behavior: it requires
closed-loop simulation and an explicitly approved noncritical trial, not just replay of an
unchanged input trace.

`calculate_all_checked` is the production result boundary. Invalid measurements or calculated
values retain an explicit reason and null numeric fields; they cannot become PID write
targets. Template unit conversion and
[snapshotted PID precision](../dcs-templates.md#pid-precision) determine controller-ready targets
separately from raw calculation validity.

## Advisory timing evidence

History records observed sample gaps, measured period, approximate samples per period, and
successful operation-latency summaries. Sampling is `adequate` at six or more observed samples
per period, `marginal` below six, and `not_assessed` without a usable period. These categories
do not abort a run, invalidate a finite result, or authorize a write.

A shared PV/MV request can populate both PV-read and MV-verification latency categories.
They describe overlapping evidence and must not be added as independent network durations.
Commanded MV remains the sample/trend series; measured MV is separate actuation evidence.
See [timing and host responsiveness](../guides/safety.md#timing-and-host-responsiveness).

For the relay behavior and the interpretation of its measurements, see
[MRFT concepts](../guides/mrft-concepts.md). The simulator's process model and fixed-step
behavior are described in the [simulator model guide](../guides/simulator-model.md).
