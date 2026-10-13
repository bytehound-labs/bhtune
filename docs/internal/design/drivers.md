# Drivers

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Driver implementation decisions

- **`OpcDaDriver` serializes access to one `opcda_bridge::Client` behind a `tokio::sync::Mutex`,
  never `std::sync::Mutex`.** The bridge client's methods take `&mut self`, but `Driver`'s
  methods take `&self` (required for `Arc<dyn Driver>` sharing), so the mutex guard is held
  across `.await` points — only `tokio::sync::Mutex`'s guard is `Send`, which `#[async_trait]`'s
  generated futures require by default. A single tuning session only ever has one read/write/
  browse in flight anyway, so serializing is not a real bottleneck.
- **`SimulatorDriver` uses `std::sync::Mutex`, not `tokio::sync::Mutex` like `OpcDaDriver`.**
  Its `read`/`write` bodies contain no `.await` points at all — they're `async fn` only because
  the `Driver` trait requires it — so nothing ever holds the guard across a suspension point,
  making the simpler std mutex both sufficient and correct. This is a genuine difference from
  `OpcDaDriver`, not an inconsistency: the tokio mutex there is load-bearing because its guard
  really is held across `.await`.
- **The FOPDT process model uses an exact closed-form discretization, not a ported ODE solver.**
  For the first-order lag `tau*dy/dt = -(y-y0) + Kp*(u-u0)` driven by a zero-order-hold input over
  one tick, the update `pv_new = pv*decay + (1-decay)*(bias + gain*mv_effective)` (`decay =
exp(-dt/tau)`) is the exact analytical solution, not an approximation — verified by comparing
  it against the legacy Python reference's own `scipy.integrate.odeint` integration across 5
  varied gain/tau/dt combinations (agreement to ~1e-5, `odeint`'s own tolerance). This avoids
  taking on a numerical-ODE-solver dependency for a model simple enough to solve in closed form.
  Dead time is a `VecDeque<f32>` delay line seeded with `ceil(dead_time_s / tick_interval_s)`
  copies of the initial MV, push-then-pop each tick.
- **`VirtualPid` (the standalone PID controller used to closed-loop-validate the simulator) is
  deliberately not wired into `SimulatorDriver`/`Driver`.** `SimulatorDriver` exists so a real
  `MrftEngine` can drive a synthetic process through the actual `Driver` trait; `VirtualPid` is a
  separate demo/validation utility proving the FOPDT model behaves like a real control loop under
  simple feedback (proportional-only exact-formula check, anti-windup, no derivative kick,
  full closed-loop convergence — the convergence gains were numerically pre-verified against a
  disposable Python script before being hardcoded, the same discipline used for `core-mrft`/
  `core-tuning-math`'s expected values). Wiring it into `Driver` would give `Driver` two
  unrelated jobs (being a `MrftEngine`'s tag I/O source, and running its own independent
  controller) for no real benefit.
- **`rand` 0.10 is configured `default-features = false, features = ["std", "std_rng"]`, and
  `StdRng` is seeded explicitly rather than using a thread-local RNG.** Every RNG in
  `bhtune-driver` is constructed via `StdRng::seed_from_u64`, so `thread_rng`/OS-entropy features
  are never used and stay disabled. `StdRng` was chosen over `SmallRng` specifically because
  `SmallRng`'s own documentation states its algorithm depends on the target's pointer size — a
  real cross-platform reproducibility risk for a Windows/macOS/Linux project — whereas `StdRng`'s
  only non-portability caveat is across `rand` crate versions, which is acceptable since CI only
  runs on `ubuntu-latest` (see `.github/workflows/checks.yml`/`coverage.yml`). No test hardcodes
  an exact noise value for this reason; tests only assert bounds and same-seed/different-seed
  equality/inequality, so a future `rand` upgrade changing `StdRng`'s internals can't break them.
- **`OpcDaDriver` always reports `TagValue::timestamp` as `None`, never a guessed value.**
  `opc-da-client`'s documented contract (the Windows-only library the gateway wraps) reports
  each tag's last-change time as a _local_, offset-less `"YYYY-MM-DD HH:MM:SS"` string (or
  `"N/A"`/`"Invalid"` for tags with none) — there is no reliable way to convert that into a
  trustworthy `DateTime<Utc>` without knowing the gateway host's timezone, which isn't part of
  the bridge protocol and can't safely be assumed to match wherever `bhtune` runs. Guessing
  (e.g. treating it as UTC, or as bhtune's own local time via `chrono::Local`) would silently
  produce a wrong-but-plausible value — exactly what this project avoids elsewhere (see
  `TagValue.value` staying an unparsed string above). This is also why `chrono`'s `clock`
  feature is never enabled anywhere in this workspace even after adding `opcda-bridge`/`tonic`:
  Cargo's feature unification would otherwise silently re-enable `Utc::now()`/`Local::now()`
  for `bhtune-core` too in any build that includes both crates (`cargo build --workspace`,
  `cargo test --workspace`, and eventually the `bhtune` binary) — confirmed by
  temporarily adding a `Utc::now()` call to `bhtune-core` and observing it still fails to
  compile with `opcda-bridge` present in the workspace. The field is diagnostic only (e.g.
  detecting a frozen tag whose timestamp stops advancing); it is never the tick time the
  tuning engine itself runs on, which always comes from the caller's own polling clock.
- **`OpcDaDriver`'s error-mapping and quality/write/browse translation is split into small,
  pure, synchronous functions** (`quality_from_raw`, `tag_value_from_raw`,
  `opc_value_from_write`, `write_outcome_from_result`, `tag_node_from_browse`,
  `map_bridge_error`), fully unit-tested with no I/O, separate from the thin async shell that
  only locks the mutex and calls into `opcda_bridge::Client`. The shell itself is covered by a
  handful of smoke tests against a minimal mock `Bridge` gRPC service (mirroring the pattern in
  `opcda-bridge`'s own `test_support.rs`) proving the wiring composes correctly end-to-end,
  rather than re-exercising `opcda-bridge`'s own already-tested RPC error-path matrix.

## OPC DA integration reference (`driver-opcda`)

`driver-opcda` is implemented: `OpcDaDriver` in `crates/bhtune-driver/src/opcda.rs`
consumes the published `opcda-bridge` facade crate from crates.io, pinned directly in
`crates/bhtune-driver/Cargo.toml` (not `[workspace.dependencies]` — see "Key architectural
decisions" above). It does not use a Git dependency, a local path dependency, or the CLI
crate `opcda-bridge-client`:

```toml
# crates/bhtune-driver/Cargo.toml
[dependencies]
opcda-bridge = "0.6"
```

The facade intentionally hides generated gRPC details and exposes typed capabilities,
session-aware browse pages, search streams, and read/write operations:

```rust
use opcda_bridge::{Client, Value};

let mut client = Client::connect("192.168.1.50:7600").await?;
let servers = client.list_servers().await?;
let capabilities = client.capabilities(servers[0].clone()).await?;
let page = client
    .browse_page(opcda_bridge::BrowsePageRequest::root(
        servers[0].clone(),
        capabilities.max_page_size.min(1_000),
    ))
    .await?;
let values = client
    .read(servers[0].clone(), vec!["Area.Loop.PV".into()])
    .await?;
let result = client
    .write(
        servers[0].clone(),
        "Area.Loop.MV".into(),
        Value::Float(f64::from(42.0_f32)),
    )
    .await?;
```

Integration rules, as implemented in `OpcDaDriver`:

- `OpcDaDriver::connect(host, server)` passes `host:port` straight to `Client::connect`, which
  adds the plaintext `http://` scheme itself. The default gateway port is
  `opcda_bridge::DEFAULT_BRIDGE_PORT` (`7600`). `server` (the OPC DA ProgID) is stored alongside
  the client and passed to every subsequent call — `Driver`'s own trait methods don't take a
  server parameter, since that's OPC DA-specific plumbing, not something every driver has.
- One `Client` is held (behind a `tokio::sync::Mutex`, see "Key architectural decisions" above)
  and reused across every call; its methods require `&mut self` and the underlying channel is
  designed to be reused rather than reconnected per call.
- `read` returns `TagValue` fields as strings (`value`, `quality`, and `timestamp`).
  `OpcDaDriver` maps `quality` via an exact `"Good"`/`"Uncertain"` string match (anything else,
  including `opc-da-client`'s synthesized `"Unknown(0xNNNN)"`, becomes `Quality::Bad` — never
  silently trusted) and leaves `timestamp` as `None` always (see "Key architectural decisions"
  above for why). `value` itself is passed through unparsed, per the `Driver` trait's own
  contract — parsing into `f32` and surfacing a parse failure as a real error is each specific
  caller's job, not this driver's.
- `write` accepts `Value::{String, Int, Float, Bool}`; `OpcDaDriver` only ever sends
  `Value::Float` (via `f64::from(value)` for a `TagWrite::Float`) or `Value::String` (for a
  `TagWrite::Raw`, e.g. a mode-revert write) — never `Int`/`Bool`, since bhtune has no tags of
  those kinds. `WriteResult.success == false` maps to `Ok(WriteOutcome::failure(..))`, not an
  `Err` — a gateway-level rejected write (read-only tag, out of range) is a normal RPC result,
  never an RPC error.
- `opcda_bridge::Error` is boxed and wrapped, preserving its source, via one exhaustive
  `map_bridge_error` function: `Error::Connect` becomes `DriverError::Connect`, ordinary
  `Error::Rpc` becomes `DriverError::Operation`, and indexed-search `FailedPrecondition`
  responses become `DriverError::IndexOperationRejected` with the gateway's actionable reason.
  Exhaustive (no wildcard arm) so a future new variant in `opcda_bridge::Error` fails this
  crate's build rather than silently falling into one bucket.
- `browse_page` requests one bounded page of immediate children. The gateway owns the browse
  session, node key, and continuation token; BHTune round-trips those values unchanged and
  never infers hierarchy from `.`, `!`, or `/`. A `BrowseNode` carries a display label, an
  opaque navigation key, an exact optional ItemID, and a kind distinguishing branches, items,
  and nodes that are both (`branch_and_item`).
- `search_index_status` reports the gateway-owned persistent index state, generation, counts,
  timestamps, error, source, and build progress. `search_index` performs a bounded unary query
  against that index and returns ranked matches with exact ItemIDs, breadcrumbs, and `has_more`.
  `refresh_search_index` and `control_search_index` expose explicit refresh and pause/resume/
  cancel controls. The browser's global search remains index-backed, while reopening a saved tag first
  uses a server-returned root node's opaque key to scope one bounded exact live traversal search
  in the active browse session when the persistent index cannot resolve its path. It may use
  unscoped live search only when no matching root scope is available; it never invents hierarchy
  from ItemID punctuation.
- `close_browse_session` explicitly releases gateway-side browse state. The HTTP browser calls
  it during modal cleanup; the CLI leaves sessions open so printed continuation tokens remain
  usable and exposes `bhtune opc close <session-id>` for explicit cleanup.
- `opcda-bridge-proto = "0.6"`, `tonic = "0.14"`, and `tokio-stream = "0.1"` are
  dev-dependencies only, pinned to the exact versions `opcda-bridge` itself uses internally,
  so this crate's mock-gateway smoke tests produce wire-compatible types. Production code
  never depends on `opcda-bridge-proto` directly — only the facade.

The gateway is a separate Windows process installed with `cargo install opcda-bridge-gateway` or
downloaded from the upstream releases page. It runs beside the OPC DA server, listens on port
`7600` by default, and requires the firewall to allow the client-to-gateway connection. The
0.5.0 protocol offers `ListServers`, `GetCapabilities`, paged `Browse`, `CloseBrowseSession`,
streaming live `Search`, persistent indexed-search status/query/refresh/control/delete operations,
per-server automatic-refresh controls, and unary `Read`/`Write`. MRFT polling only needs the unary calls, while subscription-driven Step
Test remains deferred until the bridge exposes a live push/subscription RPC.

## Simulator driver reference (`driver-simulator`)

`driver-simulator` is implemented in `crates/bhtune-driver/src/simulator.rs`: an in-process
FOPDT (first-order-plus-dead-time) process model plus a standalone virtual PID controller, served
through the real `Driver` trait as `SimulatorDriver`. No external process, no Windows, no
network I/O — every tick advances an internal virtual clock rather than sleeping on the wall
clock. The CLI's MRFT timestamp advances by the same configured fixed step, which is what makes
numeric simulator results reproducible across differently scheduled hosts.

- **`FopdtConfig`/`FopdtProcess`** — the process model: `gain`, `time_constant_s`, `dead_time_s`,
  `tick_interval_s`, and an optional noise amplitude. `step()` advances the model by exactly one
  tick using the exact closed-form discretization (see "Key architectural decisions" above), with
  dead time modeled as a `VecDeque<f32>` delay line. `mv()` reads the last-written MV without
  advancing anything; `write_mv()` sets it.
- **`VirtualPidConfig`/`VirtualPid`** — a standalone position-form PID controller (`Kc`, `Ti`,
  `Td`), derivative-on-measurement (matching the legacy Python reference, avoids derivative kick
  on a setpoint step), with anti-reset-windup (an integral increment is only committed if the
  resulting output didn't need clamping). Not wired into `SimulatorDriver`/`Driver` — see "Key
  architectural decisions" above for why it's kept as a separate demo/validation utility.
- **`SimulatorDriver`** — the `Driver` impl. Constructed with a PV tag name, an MV tag name, a
  `FopdtConfig`, initial PV/MV, and an RNG seed; wraps one `FopdtProcess` behind a
  `std::sync::Mutex` (see "Key architectural decisions" above for why not `tokio::sync::Mutex`).
  Reading the configured PV tag calls `FopdtProcess::step` (advances the simulated clock one
  tick); reading the MV tag returns `mv()` without advancing. Writing the MV tag accepts either a
  `TagWrite::Float` or a `TagWrite::Raw` that parses as `f32`; a non-numeric raw write is a
  rejected `WriteOutcome`, not a `DriverError`. Any other tag name is `DriverError::
InvalidTagValue` on both read and write. `browse` is always `DriverError::Unsupported` — a
  synthetic two-tag process has no real tag tree to browse.

The FOPDT physics were ported from the legacy `Model` repo's `ProcessModelOPC.py` (the script the
legacy C# app's hidden `OPCClass.Python` debug branch actually shells out to), not reimplemented
from a textbook formula — see "Key architectural decisions" above for the closed-form
discretization and its numerical cross-check against that reference.

## Replay driver reference (`driver-replay`)

`driver-replay` is implemented in `crates/bhtune-driver/src/replay.rs`: `ReplayDriver` feeds a
recorded `(time, pv)` trace through the real `Driver` trait. Unlike `driver-opcda`/
`driver-simulator`, it is **not a live driver and has no CLI-selectable driver kind** —
The `bhtune` CLI's `DriverKindArg` enum deliberately has no `Replay` variant, and
`TryFrom<bhtune_db::models::TuneDriver>` errors for `TuneDriver::Replay` on purpose. Its entire
purpose is validation: proving the `Driver` trait abstraction itself introduces no bugs on top
of the already-proven-correct `MrftEngine`, by replaying a trace through the real trait rather
than calling the engine directly (which is what `core-replay-harness` already does, at the
pure-engine level).

- **`ReplaySample { time, pv }`** — the minimal per-tick data the driver needs. Deliberately not
  `bhtune-core`'s `Tick`, and not the full golden-fixture schema — keeps this crate's production
  code free of a `bhtune-core` dependency, matching `driver-trait`/`driver-opcda`/
  `driver-simulator`.
- **`RecordedWrite { tag, value }`** — every MV write observed, in call order, exposed via
  `ReplayDriver::writes()` so a validation test can inspect what the engine actually wrote
  without needing its own mock/spy driver.
- **`ReplayDriver`** — constructed either directly from a `Vec<ReplaySample>` (`new`, for
  synthetic tests) or by parsing a real golden-fixture JSON file (`from_fixture_json`, for the
  E2E test below). Reading the configured PV tag returns the next unconsumed sample and advances
  an internal cursor, with `timestamp: Some(sample.time)` — see the timestamp exception below.
  Reading the MV tag returns the last-written value (`timestamp: None`) without advancing the PV
  cursor. Writing the MV tag mirrors `SimulatorDriver`'s numeric-parse/rejection convention
  (a non-numeric raw write is a rejected `WriteOutcome`, not a `DriverError`) and records every
  accepted write. Any other tag name is `DriverError::InvalidTagValue` on both read and write.
  `browse` is always `DriverError::Unsupported`, same rationale as the simulator. Reading a PV
  past the last recorded sample is `DriverError::Operation`, boxing a small `ReplayTraceExhausted
{ recorded, attempted }` — a genuine "this trace doesn't cover what was asked of it" condition,
  not a panic.
- **`from_fixture_json`** — parses the same golden-fixture JSON `core-replay-harness` consumes,
  via a private, deliberately minimal `FixtureFile { ticks: Vec<FixtureTick { time, pv }> }`
  `serde::Deserialize` subset. Serde's default unknown-field-ignoring behavior (no
  `#[serde(deny_unknown_fields)]`) silently skips every field this driver doesn't need —
  `config`, `direction`, `initial`, `pv_range`, `template_name`, per-tick `expected`,
  `expected_final` — so the full fixture schema is never duplicated in this crate. A malformed
  document or a missing `ticks` field is `DriverError::Operation`, boxing the underlying
  `serde_json::Error`.

**The `TagValue.timestamp` exception.** `types.rs`'s doc comment states a live driver's
timestamp must never become "the tick time the tuning engine itself runs on" — true and
load-bearing for `OpcDaDriver`/`SimulatorDriver`, whose timestamps are absent/untrustworthy by
construction. `ReplayDriver` is not a live driver: replaying exact historical `(time, pv)`
pairs _is_ its entire purpose, so its E2E test legitimately reads `TagValue.timestamp` to
reconstruct each `Tick.time` rather than maintaining a second, separately-synchronized time
source. This is a deliberate, narrow exception to the general rule, not a violation of it.

**End-to-end validation.** The crate's test suite drives a real `MrftEngine` through
`ReplayDriver` fed from the actual `tests/golden/fixtures/flow_pi_direct.json` file (the same
fixture `core-replay-harness` uses) and asserts it reaches the same final aggressive-response
proportional band (`pid.proportional` ≈ 157.7088) `core-replay-harness` already validates at the
pure-engine level — proof that going through the real `Driver` trait, rather than calling
`MrftEngine::step` directly, changes nothing. The golden trace has trailing padding ticks after
MRFT completion (`core-replay-harness`'s own documented behavior: `MrftEngine::step` is a no-op
once `Action::Complete` is returned, and the legacy app kept polling/logging during its
`MrftDelayTimerStart`/`MrftDelayComplete` shutdown sequence), so the test asserts
`driver.remaining() < total_samples` after the loop rather than full consumption — proving real
consumption happened without wrongly demanding the entire trace be drained.
