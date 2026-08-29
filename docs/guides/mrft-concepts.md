# MRFT concepts

BHTune's tuning engine runs an **MRFT** (Modified Relay Feedback Test) against a loop instead of
asking you to guess PID constants or step the process open-loop. This page explains what that
means in practice — enough to interpret a run's results and choose sensible parameters, not a
full control-theory derivation.

## The relay feedback idea

Classic relay feedback (Åström & Hägglund) forces a process into a small, controlled
oscillation by switching the manipulated variable (MV) between two fixed values instead of
using a normal PID controller. Where the process crosses its setpoint, the relay flips; the
resulting oscillation's **period** and **amplitude** are enough to estimate the process's
ultimate gain and ultimate period — the same two numbers classic Ziegler-Nichols closed-loop
tuning is built on — without ever pushing the process to the edge of instability the way a true
Ziegler-Nichols closed-loop test does.

**"Modified"** refers to the specific relay-switching refinements BHTune's engine implements:
hysteresis around the switch point (so measurement noise near the setpoint doesn't cause
spurious extra switches), a configurable number of initial cycles to _skip_ before any
switches are counted (letting initial transients settle out), and noise-protection delays
around each switch. These are exactly the same refinements the legacy tool implemented, ported
unchanged.

## What actually happens during a test

1. **BHTune reads the loop's current state** — PV, MV, ranges, mode, and controller direction —
   and validates all of it (see [Safety](safety.md#input-validation)) before touching anything.
2. **The loop is switched to manual.** Nothing is calculated yet; a test is a real, physical
   experiment on a real, running process.
3. **The MV is stepped by the relay amplitude** (a percentage of the MV range you choose) every
   time the PV crosses the switch threshold, in the direction that opposes the process's own
   moves — this is what "closes the loop" around the relay instead of a PID block.
4. **Every PV sample is polled and logged** (by default every 800 ms, matching the legacy tool's
   timer), building up a picture of the resulting oscillation: its peaks, its troughs, and the
   exact times each switch happened.
5. **The first few cycles are skipped** (`--cycles-skip`, defaulted per process type) before any
   switch is counted, and then a fixed number of cycles are counted (`--cycles-count`) to
   measure the steady-state oscillation.
6. **On the final step, the MV snaps back to its starting value** rather than taking one more
   full relay step — so the loop is left close to where it started, not mid-swing.
7. **The loop is restored** to its original mode (and setpoint, if it was changed) — see
   [Safety](safety.md#restoration) for exactly what "restored" guarantees.

For OPC DA runs, each accepted MV relay command is also read back and checked against its
commanded target before another relay can replace it. This verification uses a fixed internal
four-second confirmation window; a command that remains outside tolerance, or whose matching
readback arrives after the deadline, aborts the test and starts restoration. Simulator and replay
runs do not add this live-I/O verification step.

The MV values shown in the trend, persisted samples, and sample exports are the **commanded**
values produced by the MRFT engine. The actual MV values returned by OPC DA readbacks are
separate actuation-audit records shown in run history; keeping these series separate preserves
the engine's timing and export semantics while making physical actuation evidence available.

Nothing here writes a PID constant. That only happens if you explicitly ask for it
(`--write-pid <level>` on the CLI, or the Automatic PID settings section of the New tune form) — see
[PID write-back](safety.md#pid-write-back).

## From oscillation to PID constants

Once the counted cycles are captured, BHTune measures the oscillation's average period and
amplitude, and combines them with the relay amplitude to estimate the process's ultimate gain
and period. Three sets of PID constants are then calculated from that estimate — **Aggressive**,
**Moderate**, and **Sluggish** — using per-process-type tuning-constant matrices (the same
approach, and the same underlying constants, as the legacy tool). Which one to actually use
depends on how much overshoot/oscillation the process can tolerate:

- **Aggressive** — fastest disturbance rejection, least tolerant of model error; best for
  well-behaved, fast loops (flow, pressure) where a little overshoot is harmless.
- **Moderate** — a reasonable default for most loops.
- **Sluggish** — slowest, most conservative; best where overshoot is expensive or dangerous
  (level loops feeding a downstream process, some temperature loops).

`bhtune history show <run-id>` (or the run detail screen in the web GUI) always reports all
three, in the units your DCS/PLC template expects (Proportional Band % or Gain, Reset Time or
Reset Rate, Derivative Time or Derivative Gain — see
[DCS/PLC templates](../dcs-templates.md)), so the choice is made after seeing the numbers, not
before.

## Controller type and process type

- **P and PI** are offered for every process type; **PID** is only offered for the two
  Temperature process types, matching the tag conventions the built-in templates were authored
  against (a temperature loop's derivative term is usually meaningful; a flow loop's usually
  isn't, and adding one to a fast, noisy loop tends to do more harm than good).
- **Process type** (flow, pressure — line or vessel, level, temperature — mixing or heat
  exchange) drives the default cycles-skip/cycles-count/noise-protection values and which row of
  the tuning-constant matrices is used. These defaults come from the same lookup tables the
  legacy tool used; override them explicitly if your process needs something different.

## Next steps

- [Safety](safety.md) — what BHTune does (and refuses to do) around a live, running process,
  including cancellation, quality enforcement, and write-back rollback.
- [DCS/PLC templates](../dcs-templates.md) — how a template's suffixes and units turn a bare tag
  name into the full set of reads/writes a tune needs.
- [CLI quickstart](../getting-started/cli-quickstart.md) / [Web GUI quickstart](../getting-started/web-gui-quickstart.md)
  — run a test yourself against the built-in simulator.
