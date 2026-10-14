# Golden-master replay validation

This record preserves trace provenance and the replay oracle's evidence limits. Code and tests
are authoritative; see the [correctness register](correctness-register.md) for fix decisions
and the [simulator model](../../guides/simulator-model.md) for plant-free numeric validation.

## Validation strategy: golden-master replay

Recorded tick-by-tick PV inputs drive a real `MrftEngine::step`. The harness compares hysteresis,
MV, switch counters, final peaks/troughs/timestamps, and all three response-level calculations.
Discrete state and switch timestamps compare exactly; numeric fields use the harness's explicit
absolute-plus-relative tolerances, not bitwise floating-point equality.

`scripts/convert_golden_trace.py` normalizes a captured `--log --decryptedLog` CSV pair into
`tests/golden/fixtures/<name>.json`. Its module docstring describes parameterization and independent
derivation of controller direction and peak/trough array lengths; do not infer these from the
fixture name. `crates/bhtune-core/tests/golden_replay.rs` is the executable oracle.
The field fixture `flow_pi_direct` is **Flow/PI/Reverse**, despite its historical filename.

The production CLI's numeric simulator regression is deliberately separate from this
deterministic replay oracle. `crates/bhtune-cli/tests/e2e_simulator.rs` launches the real
subprocess and persists a Flow/PI, Temperature (Heat Exchange)/PID, and Level/P matrix through
the full CLI path, then compares Kp/Ti/Td and template-converted P/I/D values with reviewed
baselines. It uses a serial 5 ms fixed-step simulator configuration: each PV read advances both
the FOPDT process and MRFT time by exactly 5 ms, so all nonzero fields use tight
absolute-plus-relative tolerances with no scheduler-jitter allowance. The browser E2E checks
server/UI delivery; this CLI test is the numeric envelope check.

## Recorded precision limits

Two source-verified properties of the legacy logger/calculation limit this trace's precision:

- **The raw CSV logs `TimeCurrent`/`MvSwitchTimesList_N` at whole-second precision only**, despite
  the true ~800 ms polling cadence. This creates an exact tie at any threshold comparison whose
  true sub-second offset happens to straddle it — confirmed once, at a noise-protection-boundary
  tick, and resolved with a single, evidence-based timestamp nudge (documented inline in the
  fixture's `description` and in `scripts/convert_golden_trace.py`'s `--nudge-tick` flag), not by
  changing the engine, since the engine's own `<=` comparison was verified byte-for-byte identical
  to `OPCClass.cs`'s.
- **`CalculatedMRFTperiodMinutes` (`OPCClass.cs`, ~line 743) computes elapsed time via
  `TimeSpan.Seconds` — an integer, truncating — rather than total elapsed seconds**. The Rust
  engine uses full elapsed milliseconds, without truncation or a 24-hour wrap (see
  [correctness register, item 2](correctness-register.md#correctness-critical-design-details-also-the-legacy-bug-register-core-bug-register)).
  Combined with the whole-second logging
  ceiling above, the fixture's reconstructed elapsed time between the first and last recorded
  switch can differ from the legacy app's truncated calculation by up to one second — fully and
  numerically explaining the one place the replay harness needs a dedicated, narrow tolerance
  (`PERIOD_TOLERANCE_MINUTES = 0.01` for `ti_minutes`, converted to seconds for `integral`;
  about 0.003 minutes observed against the documented 0.0028 theoretical bound for this trace).
  This is a property of this one whole-second-logged trace, not the engine: the harness uses the
  production `calculate_all` behavior.

## Evidence boundaries and provenance

Synthetic FOPDT tests cover process/controller/direction combinations and boundaries such as
nonzero MV floors and varied skip/count cycles; they do not supply independent legacy-field
parity. The recorded fixture validates one Flow/PI/Reverse trace and its capture-to-conversion-to-
replay pipeline, not every process type, plant, firmware, or closed-loop PID response.
Additional independently reviewed field traces can use the same converter/harness.

Snapshot a run as a fixture only after manually verifying the engine's output is
control-theoretically correct for that scenario — the fixture then guards against future
regressions; it is not itself the source of truth for correctness.

The normalized `tests/golden/fixtures/flow_pi_direct.json` is self-contained. Raw
`flow_pi_direct_*.csv` capture provenance remains in Git history at commit `0301538`, not as
required runtime files. The converter's docstring records fixture construction and the
evidence-backed timestamp adjustment.
