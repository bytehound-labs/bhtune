# bhtune

A free, open-source Rust PID control-loop auto-tuner for industrial DCS/PLC systems (Yokogawa
CentumVP, Honeywell Experion, Schneider Modicon, Allen-Bradley PlantPAx). Runs a Modified Relay
Feedback Test (MRFT) against a live loop and calculates/writes back PID constants, via a CLI or a
Tauri desktop GUI.

## Status

Early. Workspace and CI are in place. `bhtune-core`'s data model (`core-model`), MRFT
relay-switching engine (`core-mrft`), and tuning-constant math (`core-tuning-math`) are
implemented and unit-tested. `bhtune-db`'s SQLite schema (`db-schema`) is implemented and
tested — all 7 tables, migrations, and connection/pragma setup — the four built-in DCS
templates seed themselves on startup (`db-seed-templates`), the run-history repository
layer (`history-query-api`) is done: full run lifecycle (start/record-initial-readings/
complete/fail/abort), dynamic filtering and pagination over runs, and per-run sample/result/
write queries, and whole-database backup/restore (`db-backup-restore`) is done: a single
portable-file snapshot via `VACUUM INTO`, and a validated, safety-copied restore back into
place. `bhtune-backend`'s `Backend` trait and error model (`backend-trait`) are
defined and tested, and its OPC DA implementation (`backend-opcda`, `OpcDaBackend`) is also
done — the primary v1 driver, over the published `opcda-bridge` crate. `backend-simulator`/
`backend-replay`, the replay harness, CLI, and GUI are not yet. See "Phases and todos" below
for what's next.

## Design philosophy and scope discipline

Most PID auto-tuning tools for industrial DCS/PLC systems are Windows-only desktop applications
built on proprietary, license-gated toolkits and OPC SDKs — expensive to license, impossible to
audit, and impossible to run outside a hand-provisioned Windows machine. bhtune is designed from
the ground up to avoid all of that:

- **100% FOSS dependencies, machine-enforced in CI** (`cargo deny`, see `deny.toml`) — not an
  aspiration, a build gate.
- **Zero Windows/COM dependency in the application itself** — OPC DA connectivity is delegated to
  a separate network-facing gateway process (see "Key architectural decisions" below), so bhtune
  runs on Linux, macOS, and Windows identically.
- **A deterministic, replayable core engine**, validated by a golden-master regression suite:
  recorded input/output traces are replayed through the engine and the results are asserted to
  match exactly, so behavior changes are always deliberate, never silent regressions (see
  "Validation strategy" below).

Scope is deliberately bounded for v1: MRFT tuning over OPC DA only, with a CLI and a desktop GUI
sharing one engine. Resist expanding to other protocols or major new features (multi-loop batch
tuning, Step Test, OPC UA/Modbus) until v1 actually ships — those are the roadmap, not v1.

## Key architectural decisions

- **The MRFT engine is a pure, I/O-free state machine.** `bhtune-core` must expose something
  shaped like `fn step(&mut self, tick: Tick) -> Vec<Action>` — no clock reads, no network calls,
  no UI access inside the algorithm itself. This is the single decision that makes it possible to
  replay a recorded trace tick-by-tick and compare it deterministically, with no flakiness from
  timing or I/O — the golden-master regression suite depends on it.
- **No proprietary dependencies, ever, machine-enforced.** `cargo deny check` (see `deny.toml`)
  fails CI on any dependency license not on the allow-list. This is not aspirational — if it
  fails on a new dependency, find a FOSS alternative; don't widen the allow-list reflexively.
- **Zero Windows/COM dependency in this application.** All OPC DA communication is delegated to
  the sibling project [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge) over the
  network. bhtune itself builds and runs on Linux, macOS, and Windows identically.
- **The OPC DA client is a crates.io dependency, local to `bhtune-backend` only.** The
  `OpcDaBackend` implementation consumes the published `opcda-bridge` library with
  `opcda-bridge = "0.2"` pinned directly in `crates/bhtune-backend/Cargo.toml` — not promoted
  to `[workspace.dependencies]`, since `bhtune-backend` is the only crate that talks to the
  bridge directly (everything else goes through the `Backend` trait), matching this project's
  single-consumer-stays-local dependency convention. It must not use a Git dependency or a
  local path checkout. The Windows-side `opcda-bridge-gateway` remains a separate process.
- **`Backend` trait is the extensibility seam, and deliberately has zero `bhtune-core`
  dependency.** A single async trait in `bhtune-backend` abstracts all tag I/O so the tuning
  engine never knows what it's talking to:

  ```rust
  #[async_trait]
  pub trait Backend: Send + Sync {
      async fn read(&self, tags: &[TagId]) -> BackendResult<Vec<TagValue>>;
      async fn write(&self, tag: &TagId, value: TagWrite) -> BackendResult<WriteOutcome>;
      async fn browse(&self, path: &str) -> BackendResult<Vec<TagNode>>;
  }
  ```

  `TagId` is a plain `String` alias (no invariant worth a newtype). `TagValue.value` is a raw
  string, not a parsed `f32` — not every tag is numeric (mode/direction/attribute tags hold
  raw codes like `"MAN"`/`"0"` that `bhtune_core::ControllerDirection::from_raw_tag_value`
  interprets directly), so parsing is the caller's job, not this trait's. `TagWrite` is
  `Float(f32) | Raw(String)` — bhtune only ever writes numeric process values or a raw mode
  code (reverting Auto/Manual after a test). `write` returns `Ok(WriteOutcome { success,
  error_message })` even when the backend *rejects* the write (read-only tag, out of range) —
  that's a normal outcome of the call reaching the backend, not a `BackendError`; the shape
  matches `bhtune_db::models::TuneWriteRow`'s columns exactly so a caller can copy it straight
  into an audit row with no translation. `BackendError` splits `Connect` (nothing was
  attempted) from `Operation` (reached the backend, failed there) from `Unsupported` (this
  backend has no such capability, e.g. `browse` on the simulator/replay backends) so callers
  like the planned `cli-safety` guardrails can react differently to each. The trait never
  references a `bhtune-core` type: reading/writing named string tags has no domain meaning by
  itself — gluing `Backend` to `LoopTags`/`ControllerDirection`/etc. is each concrete
  backend's own job (`backend-opcda`, `backend-simulator`), not this trait's.

  `OpcDaBackend` (via `opcda-bridge`) is the primary/only driver for v1, now implemented (see
  `backend-opcda` below). `OpcUaBackend` and `ModbusBackend` are roadmap items that must slot in
  without touching `bhtune-core`. Connecting/constructing a specific backend is deliberately
  *not* part of the trait — each implementation's own inherent constructor takes whatever it
  individually needs (gateway host/port + OPC DA server name, a trace file path, simulator
  parameters), since one uniform `connect()` signature across such different backends would
  leak one implementation's parameters into the trait every other implementation would have to
  ignore.

- **AGPL-3.0-or-later + CLA.** FOSS for everyone now; the CLA (see `CLA.md`, currently a draft —
  not yet in force) is what would let ByteHound also offer separate commercial licensing terms to
  enterprise customers later, without taking anything away from AGPL users. The CLA's legal
  entity name is an open question — see "Open questions".
- **v1 adapters: CLI + Tauri desktop only.** `bhtune-server` (HTTP/REST via Axum) is a roadmap
  stub, intentionally not implemented yet.
- **Step Test is deferred**, not part of v1 (MRFT only). Step Test is an alternative, simpler
  manual tuning method that observes PV changes via an OPC DA _subscription_ rather than polling
  reads, and the bridge's protocol has no such push/subscription RPC yet — `ListServers`/`Read`/
  `Write` are unary and `Browse` is a bounded, one-shot server-streaming call (the facade drains it
  into a single `Vec` before returning). MRFT itself only needs unary polling reads, so this
  doesn't block v1 — Step Test is blocked on adding a live push/subscription RPC to
  `opcda-bridge`, distinct from `Browse`'s existing bounded stream.
- **Plain, open SQLite. No encryption, no licensing, no loop-locking, no login gate.** All tune
  history lives in a single, plain, open SQLite database anyone can inspect with any SQLite
  browser. This is a deliberate simplicity choice: a free, open-source tool has no reason to
  obfuscate its own data or gate its own usage.
- **`bhtune-core` enums are mapped to SQLite `TEXT` columns without giving `bhtune-core` a
  `sqlx` dependency.** `bhtune-core` must stay dependency-free (see below), and Rust's orphan
  rule blocks implementing the foreign `sqlx::Type` trait for a foreign enum type from
  `bhtune-db` either. `bhtune_db::convert::{enum_to_text, text_to_enum}` solves this generically,
  by round-tripping through each enum's existing, already-tested `#[serde(rename_all =
  "snake_case")]` implementation (`serde_json::Value::String`) instead of a second, hand-written,
  drift-prone string-mapping table per enum. Every enum-shaped column has a matching `CHECK (...
  IN (...))` constraint using the exact same literals, so an invalid value can never be written
  even by something other than this crate.
- **Schema rule of thumb: flatten stable/filterable data into columns, keep nested/evolving data
  as validated JSON.** `bhtune_core::loop_config::LoopConfig` (flat, stable, and something
  `history-query-api` must filter on) is flattened into real columns on `loops`/`tune_runs`.
  `bhtune_core::tags::LoopTags` (nested, template-conditional, no stated SQL-filtering need) is
  stored as one `CHECK (json_valid(...))`-constrained JSON column, reusing its `serde` impl
  verbatim. Apply this same test to any future domain type that needs a place in the schema,
  rather than deciding case-by-case.
- **`tune_results` (calculated) and `tune_writes` (actually written to the DCS) are separate
  tables.** A run can produce three calculated candidate results and zero or more writes;
  conflating "the tool suggested this" with "this went into the controller" would lose the one
  fact the legacy CSV logs never captured — see `history-writeback-audit`.
- **`f32` in the tuning engine, not `f64`.** Industrial analog tags (PV/MV/tuning constants) are
  commonly single-precision (`REAL4`/`VT_R4`) over OPC DA, and don't need more precision than
  `f32` provides. Using a fixed, narrower width consistently — rather than mixing `f32` OPC values
  into `f64` math — avoids conversion noise and keeps golden-master replay comparisons exact.
- **Seeding built-in templates is an upsert, keyed on ownership, not a one-time insert.**
  `bhtune_db::seed_builtin_templates` runs on every startup: it inserts any missing built-in
  template, overwrites existing `is_builtin = 1` rows to match the current shipped definition
  (so a suffix/unit fix in a later release reaches existing installs automatically), and never
  touches a row whose name collides with a built-in's but which isn't itself `is_builtin` — a
  user's own template is never silently overwritten just because it shares a name with a preset.
- **`history-query-api`'s repository layer covers the full run lifecycle, not just read-side
  querying.** `TuneRunRow` gained `start`/`record_initial_readings`/`complete`/`fail`/`abort`
  alongside `get`/`list`/`count` — a repository that could only read the rows it has no way to
  write would be an awkward half-feature, and building it now (rather than waiting on
  `backend-opcda`'s future orchestration glue or `cli-commands`) matches the same "do the DB
  layer ahead of time" reasoning that drove `db-schema` itself. `LoopRow` deliberately still has
  no CRUD methods yet — loop management is a separate concern from run history and stays
  deferred to whichever future todo actually needs it.
- **Dynamic run filtering uses `sqlx::QueryBuilder`, the only non-fixed SQL in `bhtune-db`.**
  `TuneRunFilter`'s seven fields (`loop_id`, `process_type`, `controller_type`, `outcome`,
  `backend`, `started_after`, `started_before`) are all optional, and the active `WHERE`
  conditions vary per call — a plain `query!`/`query` string can't express that. The shared
  `push_filter` helper always starts with `builder.push(" WHERE 1=1")` and then unconditionally
  `AND`-appends each present filter, rather than tracking a `has_condition` flag to decide
  between a `WHERE`/`AND` prefix (the flag version compiles but trips rustc's
  `unused_assignments` lint on its final write). `TuneRunRow::list` and `::count` both call the
  same `push_filter`, so pagination and the total-count-for-pagination can never disagree about
  which rows match. `Pagination { limit, offset }` defaults to 50/0.
- **Write-back readback values are a new `bhtune-db`-local `WriteReadback` type, not
  `bhtune_core::tuning_math::OpcWriteValues` reused a second time.** `OpcWriteValues` is
  documented as "the literal values to write" — a calculated, intended value, and it already
  supplies `TuneWriteRow`'s `response_level`. What a backend reads back immediately after
  issuing that write is a different kind of fact (a raw, unlabelled observation, not a
  calculation) with no natural home in `bhtune-core`, so `TuneWriteRow::insert_success` takes a
  `WriteReadback { proportional, integral, derivative }` instead. `insert_success` and
  `insert_failure` are kept as two separate functions, rather than one taking
  `Option<WriteReadback>` plus `Option<String>`, so a nonsensical combination (both present, or
  neither) is structurally unrepresentable rather than merely validated at runtime.
- **`OpcDaBackend` serializes access to one `opcda_bridge::Client` behind a `tokio::sync::Mutex`,
  never `std::sync::Mutex`.** The bridge client's methods take `&mut self`, but `Backend`'s
  methods take `&self` (required for `Arc<dyn Backend>` sharing), so the mutex guard is held
  across `.await` points — only `tokio::sync::Mutex`'s guard is `Send`, which `#[async_trait]`'s
  generated futures require by default. A single tuning session only ever has one read/write/
  browse in flight anyway, so serializing is not a real bottleneck.
- **`OpcDaBackend` always reports `TagValue::timestamp` as `None`, never a guessed value.**
  `opc-da-client`'s documented contract (the Windows-only library the gateway wraps) reports
  each tag's last-change time as a *local*, offset-less `"YYYY-MM-DD HH:MM:SS"` string (or
  `"N/A"`/`"Invalid"` for tags with none) — there is no reliable way to convert that into a
  trustworthy `DateTime<Utc>` without knowing the gateway host's timezone, which isn't part of
  the bridge protocol and can't safely be assumed to match wherever `bhtune` runs. Guessing
  (e.g. treating it as UTC, or as bhtune's own local time via `chrono::Local`) would silently
  produce a wrong-but-plausible value — exactly what this project avoids elsewhere (see
  `TagValue.value` staying an unparsed string above). This is also why `chrono`'s `clock`
  feature is never enabled anywhere in this workspace even after adding `opcda-bridge`/`tonic`:
  Cargo's feature unification would otherwise silently re-enable `Utc::now()`/`Local::now()`
  for `bhtune-core` too in any build that includes both crates (`cargo build --workspace`,
  `cargo test --workspace`, and eventually `bhtune-cli`'s own binary) — confirmed by
  temporarily adding a `Utc::now()` call to `bhtune-core` and observing it still fails to
  compile with `opcda-bridge` present in the workspace. The field is diagnostic only (e.g.
  detecting a frozen tag whose timestamp stops advancing); it is never the tick time the
  tuning engine itself runs on, which always comes from the caller's own polling clock.
- **`OpcDaBackend`'s error-mapping and quality/write/browse translation is split into small,
  pure, synchronous functions** (`quality_from_raw`, `tag_value_from_raw`,
  `opc_value_from_write`, `write_outcome_from_result`, `tag_node_from_browse`,
  `map_bridge_error`), fully unit-tested with no I/O, separate from the thin async shell that
  only locks the mutex and calls into `opcda_bridge::Client`. The shell itself is covered by a
  handful of smoke tests against a minimal mock `Bridge` gRPC service (mirroring the pattern in
  `opcda-bridge`'s own `test_support.rs`) proving the wiring composes correctly end-to-end,
  rather than re-exercising `opcda-bridge`'s own already-tested RPC error-path matrix.
- **`db-backup-restore`'s `backup_to`/`restore_from` use `VACUUM INTO` and a validate-first,
  safety-copy-first design, not a raw file copy.** `VACUUM INTO` produces a single, compacted,
  non-WAL file with no `-wal`/`-shm` sidecars to also track — the most portable on-disk form for
  "one file, take it anywhere" — and runs online (it doesn't block the source pool's other
  readers/writers). `restore_from` takes its `pool` **by value**, not `&SqlitePool`: restoring
  replaces the file underneath every existing connection, so the type system forces the caller
  to give up its old handle rather than risk it staying around and getting reused after the file
  it pointed to no longer holds the same data. Before touching anything live, the candidate
  backup file is opened **read-only** and run through `PRAGMA integrity_check` plus a check for a
  real `tune_runs` table (cheap proxy for "this is actually a bhtune database") — every way that
  check can fail (nonexistent file, unopenable file, failed or non-"ok" integrity check, wrong
  schema) maps to the same `DbError::InvalidBackup`, so callers don't need to distinguish causes
  to handle "this isn't a valid backup" correctly. Per this project's own "export before
  destructive DB operations" rule, `restore_from` unconditionally copies any existing live file
  to a timestamped `<file>.pre-restore-<UTC timestamp>.bak` sibling *before* overwriting it, and
  reports that path back via `RestoreOutcome::pre_restore_backup` (`None` only when there was no
  live file to protect, i.e. a fresh install). The actual file replacement is
  copy-to-a-same-directory-temp-file-then-`rename`, so a crash or a full disk mid-copy can never
  leave `db_path` half-overwritten (rename onto an existing path is atomic on the same
  filesystem). Stale `-wal`/`-shm` sidecars at the old live path are then explicitly removed —
  proven necessary by testing that a graceful `Pool::close()` on a database's last connection
  already deletes sidecars *it* created, so the removal loop only matters for genuinely orphaned
  ones with no backing connection (crash leftovers, or files copied in from elsewhere); the test
  covering this simulates exactly that case rather than the (already self-cleaning) graceful
  case. Finally, `db_path` is reopened via the ordinary `connect()`, so any migrations the
  backup predates are re-applied going forward — restoring an old backup transparently upgrades
  its schema, the same as opening an old database file normally would.

## OPC DA integration reference (`backend-opcda`)

`backend-opcda` is implemented: `OpcDaBackend` in `crates/bhtune-backend/src/opcda.rs`
consumes the published `opcda-bridge` facade crate from crates.io, pinned directly in
`crates/bhtune-backend/Cargo.toml` (not `[workspace.dependencies]` — see "Key architectural
decisions" above). It does not use a Git dependency, a local path dependency, or the CLI
crate `opcda-bridge-client`:

```toml
# crates/bhtune-backend/Cargo.toml
[dependencies]
opcda-bridge = "0.2"
```

The facade intentionally hides generated gRPC details and exposes the typed API
`OpcDaBackend` wraps:

```rust
use opcda_bridge::{Client, Value};

let mut client = Client::connect("192.168.1.50:7600").await?;
let servers = client.list_servers().await?;
let nodes = client
    .browse(servers[0].clone(), false, String::new(), 1_000)
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

Integration rules, as implemented in `OpcDaBackend`:

- `OpcDaBackend::connect(host, server)` passes `host:port` straight to `Client::connect`, which
  adds the plaintext `http://` scheme itself. The default gateway port is
  `opcda_bridge::DEFAULT_BRIDGE_PORT` (`7600`). `server` (the OPC DA ProgID) is stored alongside
  the client and passed to every subsequent call — `Backend`'s own trait methods don't take a
  server parameter, since that's OPC DA-specific plumbing, not something every backend has.
- One `Client` is held (behind a `tokio::sync::Mutex`, see "Key architectural decisions" above)
  and reused across every call; its methods require `&mut self` and the underlying channel is
  designed to be reused rather than reconnected per call.
- `read` returns `TagValue` fields as strings (`value`, `quality`, and `timestamp`).
  `OpcDaBackend` maps `quality` via an exact `"Good"`/`"Uncertain"` string match (anything else,
  including `opc-da-client`'s synthesized `"Unknown(0xNNNN)"`, becomes `Quality::Bad` — never
  silently trusted) and leaves `timestamp` as `None` always (see "Key architectural decisions"
  above for why). `value` itself is passed through unparsed, per the `Backend` trait's own
  contract — parsing into `f32` and surfacing a parse failure as a real error is each specific
  caller's job, not this backend's.
- `write` accepts `Value::{String, Int, Float, Bool}`; `OpcDaBackend` only ever sends
  `Value::Float` (via `f64::from(value)` for a `TagWrite::Float`) or `Value::String` (for a
  `TagWrite::Raw`, e.g. a mode-revert write) — never `Int`/`Bool`, since bhtune has no tags of
  those kinds. `WriteResult.success == false` maps to `Ok(WriteOutcome::failure(..))`, not an
  `Err` — a gateway-level rejected write (read-only tag, out of range) is a normal RPC result,
  never an RPC error.
- `opcda_bridge::Error` is boxed and wrapped, preserving its source, via one exhaustive
  `map_bridge_error` function: `Error::Connect` becomes `BackendError::Connect`, `Error::Rpc`
  becomes `BackendError::Operation`. Exhaustive (no wildcard arm) so a future new variant in
  `opcda_bridge::Error` fails this crate's build rather than silently falling into one bucket.
- `browse` hardcodes `flat: false` (one level, matching `Backend::browse`'s own contract) and a
  `max_tags` of `1000`, matching `opcda-bridge-client`'s own CLI default
  (`DEFAULT_MAX_TAGS` in that crate's `config.rs`) for consistency with the reference CLI.
  `BrowseNode.node_type` is mapped via an exact `"Branch"` string match (the gateway's own
  `NODE_TYPE_BRANCH` constant); anything else — including an unrecognized value — is treated as
  a leaf, the conservative choice (a wrongly-leaf-tagged branch just returns a clear error on
  read/write; a wrongly-branch-tagged leaf would make a real tag invisible to a tag-tree
  browser).
- `opcda-bridge-proto = "0.2"`, `tonic = "0.14"`, and `tokio-stream = "0.1"` are dev-dependencies
  only, pinned to the exact versions `opcda-bridge` itself uses internally, so this crate's own
  mock-gateway smoke tests (a minimal in-process `Bridge` gRPC service) produce wire-compatible
  types. Production code never depends on `opcda-bridge-proto` directly — only the facade.

The gateway is a separate Windows process installed with `cargo install opcda-bridge-gateway` or
downloaded from the upstream releases page. It runs beside the OPC DA server, listens on port
`7600` by default, and requires the firewall to allow the client-to-gateway connection. The
current protocol offers `ListServers`/`Read`/`Write` (unary) and `Browse` (server-streaming, but
drained into one `Vec<BrowseNode>` by this facade so callers never see the stream) — MRFT polling
only needs the unary calls, while subscription-driven Step Test remains deferred until the bridge
exposes a live push/subscription RPC (a different kind of stream than `Browse`'s bounded one-shot
listing).

## Validation strategy: golden-master replay

The engine's confidence story is golden-master replay: recorded input/output traces (tick-by-tick
PV inputs and the engine's resulting hysteresis/MV/switch-counter/calculated-constant outputs) are
replayed through the Rust engine and compared exactly. `trace-fixtures` normalizes captured traces
into a stable, versioned format under `tests/golden/`; `core-replay-harness` feeds them through
the engine and asserts per-tick and final-result equality. This is the gate for confidence that a
change didn't silently alter tuning behavior.

Reference traces are captured two ways, neither of which requires Windows:

1. **Synthetic runs against the in-Rust FOPDT simulator** (`backend-simulator`) across a coverage
   matrix of process types, controller types, action directions, and edge cases (non-zero MV range
   floor, varied skip/count cycles).
2. **Real traces recorded from field use**, once the CLI/GUI exist.

Snapshot a run as a fixture only after manually verifying the engine's output is
control-theoretically correct for that scenario — the fixture then guards against future
regressions; it is not itself the source of truth for correctness.

## Correctness-critical design details

These are easy to get subtly wrong, so they're called out explicitly. Each should have direct
unit-test coverage, not just be caught incidentally by a golden-master replay fixture.

1. **The MV boundary clamp must be dimensionally consistent on both sides.** If the relay step
   down would drive MV below its configured floor, clamp the relay amplitude to
   `MvValueIni - MvLowerRange` (the actual distance from the initial value down to the floor), not
   an expression that adds the floor back onto the initial value. Get this wrong and cascaded
   loops with a non-zero MV floor get an incorrect (usually oversized) relay amplitude — it's
   silently masked whenever the floor is 0 (the common 0–100% case), which makes it easy to miss
   in testing.
2. **The MRFT oscillation period must use full-precision elapsed time** (total seconds as a
   floating-point value), never truncated into separate hour/minute/second integer components and
   reassembled — that discards sub-second precision and wraps incorrectly past 24 hours. This
   matters most on fast loops (flow/pressure) where the whole oscillation period is only a few
   seconds, so truncation error is a large fraction of the signal, not noise.
3. **Switch timestamps must reuse the already-captured tick timestamp**, never a fresh wall-clock
   read at the moment a switch is performed — the two can differ by however long evaluation took,
   which is small but non-deterministic and breaks exact replay comparison.
4. **Lookup tables must be sized to exactly the number of process types that exist (6)** — no
   extra, unreachable rows/columns in the tuning-constant or default-cycle data.
5. **If a CSV/tabular export format is ever added**, generate the header and each data row's
   column order from the same single ordered list of field names — never maintain them as two
   independently hand-written strings; that's exactly the kind of thing that silently drifts out
   of sync.
6. **PID unit labels (Kp vs. PB; Ti vs. Ri vs. Ki; Td vs. Kd) must refresh on every relevant state
   change** — process-type change, template switch, and app startup — not only from a single
   settings-changed event handler. A partial refresh trigger is an easy way to end up with stale
   unit labels on a results screen.
7. **Tag-name derivation from a single PV tag must use the active DCS/PLC template's own
   configured suffix convention, never a hardcoded literal** — different DCS/PLC families name
   their PV item differently (e.g. a `.PV` dot-suffix convention vs. no such convention at all).
8. **Relay amplitude needs real, enforced range validation at the model/construction level** — not
   just client-side keystroke filtering plus a single "not blank" check. An unvalidated numeric
   field that only rejects blanks is exactly how a nonsensical value reaches a live control loop.
9. **Any file export feature must write to a path the user explicitly chooses, or a documented
   platform-standard data directory** — never an implicit hardcoded path or "wherever the process
   happened to start".
10. **Test/demo mode must be a first-class, explicit backend choice** (e.g. `--backend
simulator`), never triggered implicitly by a magic tag name or hidden UI state — an implicit
    trigger is surprising and easy to leave enabled accidentally.
11. **PID-type selection must be modeled as proper enums** (`ProportionalType`, `IntegralType`,
    `DerivativeType`, controller action direction, etc.), never as comparisons against magic
    display strings or sentinel values.
12. **PID is only offered for the two Temperature process types**; every other process type offers
    only P and PI. This is a deliberate domain rule (rooted in which tuning-constant columns are
    actually calibrated), not an arbitrary restriction to relax.
13. **Skip/count/noise-protection defaults are auto-populated per process type** from lookup
    tables whenever the process type changes.
14. **On the final MRFT step, MV snaps back to the initial value** rather than taking a full relay
    step.
15. **Significant-digit display formatting needs care.** Naive numeric rounding to N digits is not
    the same as significant-digit formatting (e.g. `0.00123` vs. `123000` both have 3 significant
    digits but very different rounding behavior). Decide up front whether exact significant-digit
    formatting matters for a given field or whether straightforward rounding is an acceptable,
    documented simplification for display-only purposes — don't assume the two are
    interchangeable.
16. **A live PV/MV trend chart is a core UX expectation for the desktop GUI** — plan for high-rate
    streaming updates (multiple times per second) from the start; see "Chart library" below.

## Conventions

- **Trunk-based git flow**: single long-lived `main`, short-lived PR branches
  (`<type>/<short-description>`), squash merges, no `develop`/release branches. Releases are
  tagged directly off `main`.
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/).
- **Formatting/linting**: `cargo fmt --check --all` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **No unused dependencies**: `cargo machete` runs in CI. Placeholder crates
  (`bhtune-desktop`, `bhtune-server`, and any stub not yet consuming a path dependency)
  deliberately carry **no** dependency on other workspace crates until they actually use one —
  don't add `bhtune-core` etc. back as a path dependency just to "wire up the graph"; add it when
  real code needs it.
- **`[workspace.package]` inheritance**: version/edition/license/repository are set once in the
  root `Cargo.toml` and inherited (`version.workspace = true` etc.) by every crate, rather than
  repeated per-crate.
- No umbrella root package (unlike `opcda-bridge`'s flat layout + root crate) — this workspace's
  crates live under `crates/`, and `bhtune-core` fills the "shared types" role a root crate would
  otherwise serve.
- Full contributor workflow is documented in [`CONTRIBUTING.md`](CONTRIBUTING.md); this file is
  for future coding-agent sessions, not human contributors.

### Deferred setup (deliberate, not oversights)

- **No `.envsync.yaml`/dotenv-sync (`ds`) hooks yet.** There is no real secret or test-env value
  to manage until a phase needs a live OPC test target (mirroring opcda-bridge's
  `OPC_TEST_HOST`/`OPC_TEST_SERVER`/`OPC_TEST_TAG` pattern). Add the `ds-sync` pre-commit command
  and `ds-sync-pull` post-merge command (see opcda-bridge's `.lefthook.yml` for the exact shape)
  at that point.
- **No `release-plz.yml`/`auto-merge.yml` workflows yet**, though `release-plz.toml` exists.
  These require a `RELEASE_PLZ_TOKEN` repo secret (a PAT with more permission than the default
  `GITHUB_TOKEN`, so the release PR itself can trigger further CI). Shipping the workflow without
  the secret would produce a failing Actions run on every push to `main`. Add both workflows once
  the token is provisioned.
- **No CLA-enforcement bot wired up yet.** `CLA.md` is a draft; it does not bind anyone until the
  legal-entity question is resolved, the text has had a legal review, and a CLA-assistant check is
  added to the PR checks.
- **No `frontend/` (pnpm workspace) yet.** Nothing consumes it until the `frontend-shell` phase.
- **`bhtune-desktop` has no real `tauri` dependency yet**, and `bhtune-server` has no `axum`
  dependency yet. Both are placeholder binaries only. Adding real Tauri deps prematurely would
  need system GTK/WebKit packages not assumed present and risks breaking `cargo build --workspace`
  before those phases are ready to use them.
- **A cross-project CI/CD audit against `opcda-bridge` hasn't happened yet.** See
  `cross-project-ci-audit` in "Phases and todos" — worth doing once both projects have settled a
  bit, not urgent.

## Build / Test / Lint / Coverage

- **Build**: `cargo build --workspace`
- **Test**: `cargo test --workspace`
- **Lint**: `cargo fmt --check --all` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- **Dependency hygiene**: `cargo deny check` (license/advisory allow-list) and `cargo machete`
  (unused dependencies)
- **Coverage**: `cargo llvm-cov --workspace --lcov --output-path lcov.info`

### Coverage enforcement

Coverage is tracked by Codecov and enforced at **100%** via `codecov.yml` (project and patch
targets both at 100% with a 1% threshold). Even placeholder code must be exercised by a test —
see the `main_runs_without_panicking` smoke tests in each binary crate's `main.rs` for the pattern
used to keep the gate meaningful (not vacuous) from the very first commit. Delete each one once
that binary does something real and gains its own targeted tests.

## Crate map and phase status

| Crate            | Phase                                                                   | Status                                                                       |
| ---------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------- |
| `bhtune-core`    | `core-model`/`core-mrft`/`core-tuning-math`/`core-replay-harness`       | `core-model` + `core-mrft` + `core-tuning-math` done, replay harness pending |
| `bhtune-backend` | `backend-trait`/`backend-opcda`/`backend-simulator`/`backend-replay`    | `backend-trait` + `backend-opcda` done (trait, error model, and OPC DA implementation, all tested); simulator/replay pending |
| `bhtune-db`      | `db-schema`/`db-seed-templates`/`history-query-api`/`db-backup-restore` | All done (7 tables, tested; 4 templates auto-seed on startup; run-history repository layer with lifecycle, filtering, and pagination; whole-database backup/restore via `VACUUM INTO`) |
| `bhtune-cli`     | `cli-commands`/`cli-config`/`cli-automation`/`cli-safety`/`cli-logging` | Scaffolded, prints a placeholder line only                                   |
| `bhtune-desktop` | `tauri-runner`                                                          | Placeholder binary, no Tauri dependency yet                                  |
| `bhtune-server`  | roadmap only                                                            | Placeholder binary, not part of v1                                           |

## Phases and todos (roadmap order)

0. **Behavior specification and reference traces** — the v1 feature/acceptance checklist at
   [`docs/v1-checklist.md`](docs/v1-checklist.md) (done); capture golden-master reference traces
   from the simulator and, later, real field use; build the trace fixture normalizer.
1. **Repository scaffolding** _(this commit)_ — Cargo/pnpm workspaces, license, CLA draft, CI,
   `cargo-deny` FOSS gate.
2. **`opcda-bridge` reusable client library** (published upstream) — consumed as a plain
   crates.io dependency (`opcda-bridge = "0.2"`), local to `bhtune-backend`'s own `Cargo.toml`
   (see "Key architectural decisions" for why it stays out of `[workspace.dependencies]`).
3. **`bhtune-core`** — the critical phase. Data model, MRFT state machine, and tuning math are
   done; the replay harness remains, with the correctness-critical details above baked in and
   unit-tested directly.
4. **Backends** — the `Backend` trait (`backend-trait`, done: `read`/`write`/`browse` plus
   `TagId`/`TagValue`/`TagWrite`/`WriteOutcome`/`TagNode`/`BackendError` in `crates/
   bhtune-backend`) and its OPC DA implementation (`backend-opcda`, done: `OpcDaBackend` in
   `crates/bhtune-backend/src/opcda.rs`, see "OPC DA integration reference" above); simulator
   (Rust FOPDT process model) and replay implementations remain.
5. **Persistence** — SQLite schema (`db-schema`, done: `dcs_templates`, `loops`, `tune_runs`,
   `tune_samples`, `tune_results`, `tune_writes`, `settings`, all migrated/tested in
   `crates/bhtune-db`), startup seeding of the four DCS/PLC template presets
   (`db-seed-templates`, done: `bhtune_db::seed_builtin_templates` upserts `built_in_templates()`
   on every startup), the run-history repository layer (`history-query-api`, done: full
   `TuneRunRow` lifecycle, dynamic filtering/pagination via `sqlx::QueryBuilder`, and per-run
   `TuneSampleRow`/`TuneResultRow`/`TuneWriteRow` queries), and whole-database backup/restore
   (`db-backup-restore`, done: `backup_to`/`restore_from` in `crates/bhtune-db/src/backup.rs` —
   see "Key architectural decisions" above). `db-drop-legacy` needed no work of its own: bhtune
   never had licensing/loop-locking/log-encryption to remove in the first place, since
   `db-schema` designed plain SQLite storage in from the start. Remaining: wiring up
   platform-standard data directories for the database file itself, once there's an actual
   application entry point to wire it into (part of `bhtune-cli`'s `cli-config`, not a
   `bhtune-db` concern).
6. **Headless CLI** — `tune`/`template`/`history`/`export`/`simulate` subcommands, CLI > env >
   TOML > default config precedence, non-interactive automation mode, safety guardrails
   (mandatory timeout + auto-restore, explicit opt-in for unattended PID writes), structured
   logging.
7. **Tauri desktop GUI** — Tauri v2 runner with typed command bindings; React + TS + Vite +
   Tailwind frontend; Connection/Tag-mapping/Test-parameters/Results/History/Template-editor/
   Simulator screens plus a live PV/MV trend chart; live per-tick streaming to the UI.
8. **End-to-end testing and CI** — fully automated E2E tune on Linux CI via CLI + simulator
   backend (no Windows, no external DCS dependency); golden replay suite in CI; Tauri
   build/bundle matrix for Linux/macOS/Windows.
9. **Documentation and release** — README/usage docs and a getting-started guide, published
   roadmap (OPC UA/Modbus backends, HTTP adapter + Docker image, Step Test pending the bridge
   `Subscribe` RPC, multi-loop/batch tuning), v0.1.0 with per-platform binaries.
10. **Cross-project CI/CD audit** (`cross-project-ci-audit`, not urgent/blocking) — compare
    `bhtune`'s CI/CD, lefthook, release-plz, and repo-hygiene setup against the sibling
    `opcda-bridge` project in both directions: pull over anything `opcda-bridge` has that
    `bhtune` is missing, and separately propose anything `bhtune` ended up doing differently
    (or better) that `opcda-bridge` might want to adopt too. Worth doing once both projects have
    settled a bit rather than right after initial scaffolding.

## Other notes

- **MRFT and Step Test (once implemented) share one concurrency model.** Both use one async
  polling/streaming model in `bhtune-core` — polling for MRFT, subscription-based streaming for
  Step Test — rather than inventing a different mechanism for each.
- **Safety is a first-class requirement, not polish.** Scheduled/scripted tuning against a live,
  running process removes human supervision (no operator watching the trend, able to hit Stop)
  while still stroking a real control valve. `cli-safety` guardrails (dry-run mode, mandatory
  wall-clock timeout with automatic abort-and-restore, explicit opt-in required for any run that
  writes PID constants unattended) are required for the CLI/automation surface, not optional.
- **Chart library**: `uPlot` over `Recharts` for the frontend trend chart — handles high-rate
  streaming data (multiple updates/second) far better.
- **Naming**: `bytehound` is an established Rust memory-profiler brand. `bhtune` avoids a direct
  crates.io collision, but be aware of the overlap with the ByteHound company brand in the Rust
  ecosystem when publishing.

## Open questions

- Which legal entity the CLA should ultimately name (the incorporated company vs. an individual)
  — the one item here with real legal consequence; resolve before the first outside PR is merged,
  not before this commit.
- Whether the DCS/PLC templates should remain user-editable JSON/TOML exports in addition to
  SQLite rows, so site-specific tag maps can be shared between installations.
