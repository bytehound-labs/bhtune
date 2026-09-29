# Golden-master replay validation

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Validation strategy: golden-master replay

The engine's confidence story is golden-master replay: recorded input/output traces (tick-by-tick
PV inputs and the engine's resulting hysteresis/MV/switch-counter/calculated-constant outputs) are
replayed through the Rust engine and compared exactly. `trace-fixtures` normalizes captured traces
into a stable, versioned format under `tests/golden/`; `core-replay-harness` feeds them through
the engine and asserts per-tick and final-result equality. This is the gate for confidence that a
change didn't silently alter tuning behavior.

Both are now done. `scripts/convert_golden_trace.py` normalizes a captured `--log --decryptedLog`
CSV pair into `tests/golden/fixtures/<name>.json` (parameterized by process/controller type,
direction, and template — see its module docstring for the full contribution workflow, including
how to independently derive `direction` and the peaks/troughs array lengths rather than guessing
them). `crates/bhtune-core/tests/golden_replay.rs` is the harness itself: it deserializes a
fixture, drives a real `MrftEngine::step` once per tick asserting `state()` against every tick's
recorded fields, captures the `Action::Complete` payload, and asserts it plus `calculate_all`'s
three response-level results against the fixture's `expected_final`. The first fixture
(`flow_pi_direct`, Flow/PI/Reverse, from the first real hp-VM capture) passes in full — the Rust
port reproduces the legacy app's tuning behavior tick-for-tick and result-for-result.

The production CLI's numeric simulator regression is deliberately separate from this
deterministic replay oracle. `crates/bhtune-cli/tests/e2e_simulator.rs` launches the real
subprocess and persists a Flow/PI, Temperature (Heat Exchange)/PID, and Level/P matrix through
the full CLI path, then compares Kp/Ti/Td and template-converted P/I/D values with reviewed
baselines. It uses a serial 5 ms fixed-step simulator configuration: each PV read advances both
the FOPDT process and MRFT time by exactly 5 ms, so all nonzero fields use tight
absolute-plus-relative tolerances with no scheduler-jitter allowance. The browser E2E checks
server/UI delivery; this CLI test is the numeric envelope check.

Getting there surfaced two genuine data-precision limits of the legacy CSV logger itself (not
engine defects — confirmed in both cases by reading the actual C# source, not by loosening
tolerances to make a test pass):

- **The raw CSV logs `TimeCurrent`/`MvSwitchTimesList_N` at whole-second precision only**, despite
  the true ~800 ms polling cadence. This creates an exact tie at any threshold comparison whose
  true sub-second offset happens to straddle it — confirmed once, at a noise-protection-boundary
  tick, and resolved with a single, evidence-based timestamp nudge (documented inline in the
  fixture's `description` and in `scripts/convert_golden_trace.py`'s `--nudge-tick` flag), not by
  changing the engine, since the engine's own `<=` comparison was verified byte-for-byte identical
  to `OPCClass.cs`'s.
- **`CalculatedMRFTperiodMinutes` (`OPCClass.cs`, ~line 743) computes elapsed time via
  `TimeSpan.Seconds` — an integer, truncating — rather than total elapsed seconds**: exactly the
  bug `TuningMathCompat::replicate_period_truncation_bug` already exists to optionally reproduce
  (see "Correctness-critical design details" below, item 2). Combined with the whole-second
  logging ceiling above, the fixture's reconstructed elapsed time between the first and last
  recorded switch can differ from the legacy app's own truncated-integer computation by up to one
  second — fully and numerically explaining the one place the replay harness needs a dedicated,
  narrow tolerance (`ti_minutes`/`integral`, ~0.003 minutes observed against a documented ~0.0028
  theoretical bound for this trace's cycle count) rather than the general tolerance used
  everywhere else. This is a property of this one whole-second-logged trace, not the engine: the
  harness deliberately drives `calculate_all` with the _default_ (bug-fixed) `TuningMathCompat`,
  since that is the behavior bhtune ships.

Reference traces are captured two ways, neither of which requires Windows:

1. **Synthetic runs against the in-Rust FOPDT simulator** (`driver-simulator`, done — see
   "Simulator driver reference" above) across a coverage matrix of process types, controller
   types, action directions, and edge cases (non-zero MV range floor, varied skip/count cycles).
   `bhtune-driver`'s own test suite already includes one such run (a full `MrftEngine` driven
   through `SimulatorDriver` to completion).
2. **Real traces recorded from field use** (`capture-traces`, done) — one trace was captured and
   replayed (`flow_pi_direct`, Flow/PI/Reverse). Deliberately closed here rather than continuing
   through the other 5 process types, PID/temperature controllers, reverse action, and cascade:
   the one trace already fully proves the capture-to-fixture-to-harness pipeline works and that
   the Rust engine matches the legacy app exactly for a real field recording, and the marginal
   parity evidence 5 more captures would add wasn't judged worth the recurring `hp` Windows-VM
   time against higher-priority phases (server/frontend/packaging). `scripts/convert_golden_trace.py`
   remains ready to normalize further captures if one is ever recorded opportunistically, but no
   more are planned.

Snapshot a run as a fixture only after manually verifying the engine's output is
control-theoretically correct for that scenario — the fixture then guards against future
regressions; it is not itself the source of truth for correctness.

`cleanup-golden-traces` is also done: the raw `flow_pi_direct_*.csv` captures under
`tests/golden/raw/` were deleted once `core-replay-harness` went green, since the normalized
`tests/golden/fixtures/flow_pi_direct.json` is what the harness actually reads at runtime and is
fully self-contained — the raw CSVs had no remaining purpose beyond provenance, which git history
(commit `0301538`) already preserves permanently. `scripts/convert_golden_trace.py`'s own
docstring records how the fixture was produced, so the provenance is documented even with the raw
files gone.
