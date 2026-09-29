# MRFT measurement and result validity

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## MRFT measurement correction and result validity

The production MRFT path uses a measurement-only boundary correction. `MrftEngine` keeps
`max_pv_cycle`/`min_pv_cycle` for hysteresis and relay-switch decisions, while
`max_pv_result`/`min_pv_result` measure the response amplitude used by the tuning formulas.
The result extrema update on every PV sample; when a switch closes one interval, its PV sample
is recorded for that interval and seeds the newly opened interval. The hysteresis extrema still
reset to `pv_value_ini`, so this correction cannot change switch timing, MV commands, actuation
verification, or the recorded sample trail.

`MrftCompat::replicate_extrema_reset_bug` retains the legacy reset-to-`pv_value_ini` behavior for
parity experiments only. It is not enabled by production callers. The committed
`flow_pi_direct` golden fixture remains a regression oracle for the legacy observable state, but
it cannot distinguish this correction from the legacy behavior because its switches occur before
the discarded boundary extrema matter. A recorded trace also cannot validate the coupled
hysteresis change: changing the switch schedule would change the plant response that the trace
contains.

Production result persistence uses `calculate_all_checked`, not the legacy numeric
`calculate_all` entry point used by parity/replay callers. A result is `Valid` only when its PV
amplitude, period, frequency, calculated Kp/Ti/Td, and template-converted P/I/D values are finite
and usable. Zero or negative amplitudes/periods and non-finite intermediates become an `Invalid`
result with an explicit reason; its numeric columns remain `NULL`. SQLite constraints enforce the
valid/invalid shape, and CLI/HTTP calculated-result write paths reject invalid rows before driver
connection. This prevents run 7's zero proportional-band value from being treated as a real PID
constant while retaining the run, samples, trend, and diagnostic evidence needed to investigate it.

Sampling quality is reported separately from mathematical result validity. A run is `adequate`
when it has at least 6.0 observed samples per measured oscillation period, `marginal` when it has
a finite positive value below that threshold, and `not_assessed` when no usable period exists.
This classification is advisory: it does not currently invalidate a result, abort a tune, or
block a valid PID write. `TimingMetrics` also records successful PV-read, MV-write, MV-verification-read,
sample-persistence, and total-tick-work latency summaries so a requested poll interval can be
compared with the actual cadence before changing defaults. A pending relay's batched PV/MV
request may populate both the PV-read and MV-verification categories; those measurements describe
one overlapping operation, not two durations to add. Failed, cancelled, and timed-out operations
are excluded from these summaries.

Variant B — resetting hysteresis extrema to the switch sample as well as result extrema — is
not shipped. It requires closed-loop FOPDT simulation because a changed switch schedule cannot be
replayed against a fixed historical PV trace. It remains research-only until simulation evidence
and an explicitly approved, operator-attended noncritical-loop trial exist; no live-plant trial
is part of the current implementation.
