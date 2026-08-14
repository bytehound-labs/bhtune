# bhtune

A free, open-source Rust PID control-loop auto-tuner for industrial DCS/PLC systems (Yokogawa
CentumVP, Honeywell Experion, Schneider Modicon, Allen-Bradley PlantPAx). Runs a Modified Relay
Feedback Test (MRFT) against a live loop and calculates/writes back PID constants, via a CLI or a
browser-based web GUI.

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
defined and tested, its OPC DA implementation (`backend-opcda`, `OpcDaBackend`) is
done — the primary v1 driver, over the published `opcda-bridge` crate — and its in-Rust
FOPDT process simulator (`backend-simulator`, `SimulatorBackend`) is done, giving CI a
fully synthetic, wall-clock-free way to drive a real `MrftEngine` end to end. `bhtune-cli`'s
core subcommand set (`cli-commands`) is done: `tune`/`simulate` (drive a real MRFT run against
either `OpcDaBackend` or `SimulatorBackend`, persisting the full lifecycle through
`bhtune-db`), `template` (`list`/`show`/`import`/`export`), `history` (`list`/`show`), `export`
(CSV/JSON of one run's samples), and `opc` (low-level `read`/`write`/`browse` passthrough to
the bridge, bypassing the tuning engine). `cli-config` is done: `CLI flag > env var > TOML
config file > platform default` precedence for the database path, opcda-bridge gateway
address, and default OPC server, mirroring `opcda-bridge-client`'s own config conventions
(see "Config precedence" below). `cli-automation` is done: `--yes`/`--write-pid`/
`--output json` on `tune`/`simulate`, `--output json` on `history list`/`show`/`revert`, and
distinguished process exit codes (`EXIT_ABORTED`, `EXIT_WRITE_BACK_FAILED`) so scheduled/
scripted callers can tell a Ctrl+C abort or a failed PID write-back apart from a clean
completion without parsing stdout (see "Automation" below). `cli-safety` is done: real
range validation on `--relay-amp` at the `LoopConfig` model/construction level (not just a
"not blank" check), a mandatory `--timeout-secs` wall-clock limit racing the poll loop
(auto-abort-and-restore, its own distinguished `EXIT_TIMED_OUT` exit code) — later hardened
by `safety-cancellation` below to actually reach an in-flight backend call rather than only
the idle wait between ticks — and an unconditional `--write-pid`-requires-`--yes` gate (there
is no way to write PID constants to a live loop, interactively or scripted, without
confirming it) — see "Safety" below. `cli-logging` is done: `tracing`/`tracing-subscriber` structured logging to a rotating
file, resolved through the same `CLI > env > TOML > default` precedence as every other
setting, with console mirroring confined to stderr so it can never corrupt the `--output
json` stdout contract — see "Logging" below. This completes all five `bhtune-cli` sub-phases;
the CLI is a fully headless, scriptable adapter on its own, no server required.
`backend-replay`, the replay harness, and the web GUI are not yet — the GUI plan reversed
from a Tauri desktop app to a browser UI served by `bhtune-server` before any Tauri code was
written (see "Key architectural decisions"). See "Phases and todos" below for what's next.

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

Scope is deliberately bounded for v1: MRFT tuning over OPC DA only, with a CLI and a web GUI
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
  like the `cli-safety` guardrails can react differently to each. The trait never
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
- **v1 adapters: CLI + browser-based web GUI, served by `bhtune-server`.** There is no desktop
  app. The original plan called for a Tauri v2 desktop shell (see the deleted `bhtune-desktop`
  placeholder crate in git history) with a Dockerized web server as a possible future add-on;
  that was reversed before any Tauri code was written; `bhtune-desktop` had zero dependencies and
  was never built against, so nothing was lost in the reversal. `bhtune-server` (Axum) is
  promoted from roadmap stub to the primary v1 GUI adapter instead, serving both the HTTP API and
  the built React SPA (embedded via `rust-embed`) from one binary. Reasons: the intended
  deployment shape is a shared, always-on host near the OPC DA gateway that engineers connect
  to — a desktop app fundamentally can't serve that; WebView2 is genuinely missing on air-gapped/
  imaged OT hosts and stale WebKitGTK on LTS Linux breaks charting libraries, which matters
  because a live PV/MV trend chart is a headline feature (see below); and Playwright E2E against
  a real browser is markedly more reliable in CI than `tauri-driver`/WebDriver for a project with
  a 100%-coverage, golden-master validation posture. Nothing here reduces to "just add Docker" —
  a plain installer (`pkg-windows-installer`) is the primary distribution artifact precisely
  because Docker is frequently banned or unavailable on OT networks; the Docker image
  (`pkg-docker`) is a secondary channel for IT-managed Linux hosts, not the deployment path this
  decision was optimized for.
- **One API surface, described by OpenAPI, with no client-side transport abstraction.** Handlers/
  DTOs are annotated with `utoipa` to emit an OpenAPI 3.1 spec; `openapi-typescript` generates the
  TypeScript client consumed by the frontend, gated by a `git diff --exit-code` CI check so
  spec/client drift is impossible (`openapi-contract`). There is exactly one transport — `fetch`
  over HTTP — so no `ApiClient`-style interface with swappable backends is warranted; adding one
  would be pure ceremony with a single implementation. The same OpenAPI spec renders interactive
  docs (Scalar) at `/api/docs`, doubling as the reference for third-party scripting against the
  HTTP API.
- **Live tick streaming uses Server-Sent Events, not WebSocket.** The flow is strictly
  server→client (engine state out, never commands in over the same channel), and SSE
  auto-reconnects natively, survives ordinary HTTP proxies, and is trivially inspectable with
  `curl` — all wins over WebSocket for a stream with no client→server traffic.
- **No built-in scheduler, permanently — this is a deliberate, settled decision, not a gap.**
  Scheduled/unattended tuning is driven by external schedulers (cron, Windows Task Scheduler)
  invoking the CLI directly; the CLI never requires `bhtune-server` to be running. Building a
  scheduler into the product would duplicate what every target OS already provides reliably.
- **v1 binds to `127.0.0.1` by default; no authentication ships in v1.** Binding off-loopback
  (e.g. to a LAN interface so multiple engineers can reach a shared host) is an explicit,
  loud opt-in, not a default, and the Windows installer never opens a firewall port unless that
  opt-in is chosen. Authentication, TLS, and audit logging are real, planned, **free** features
  (not paywalled) deferred to post-v1 remote-access work (`server-remote-auth`, `server-tls`,
  `server-audit-log`, `server-oidc`) rather than blocking v1. This is a judgement call worth
  re-examining before that host is ever reachable off a trusted OT network: the precedent that
  makes it defensible in the meantime is that `opcda-bridge-gateway` is *already* an
  unauthenticated network service in this exact topology, and it is strictly more dangerous than
  an unauthenticated bhtune (it can read/write any tag, whereas bhtune only ever writes the PID
  constants of one user-selected loop).
- **Nothing is paywalled, now or on the current roadmap.** The CLA exists solely to keep
  relicensing *possible* in the future without taking anything from AGPL users today — it is not
  evidence of a planned paid tier, and no roadmap item (including the post-v1 remote-access work
  above) is scoped as enterprise-only.
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
  rather than deciding case-by-case. `tune_runs`'s own `template_name`/`template_origin`
  (flat) plus `template_snapshot_json`/`tags_json` (JSON) columns (`safety-run-snapshot`, see
  "Live-plant safety hardening" below) are a second application of the exact same rule.
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
  supplies `TuneWriteRow`'s `response_level`. What a backend reads back is a different kind
  of fact (a raw, unlabelled observation, not a calculation) with no natural home in
  `bhtune-core`, so both the *pre-write* readback (`TuneWriteRow.previous`) and the
  *post-write* confirmation readbacks reuse `WriteReadback { proportional, integral,
  derivative }`. `previous` is all-or-nothing (`Option<WriteReadback>`, not three
  independently nullable fields) because `safety-writeback-rollback`'s pre-read step is a
  hard stop — either all three pre-reads succeed before anything is written, or nothing is
  written and there is no partial "previous" to record. The three `*_written`/`*_readback`
  columns, by contrast, *are* independently nullable, since the write-and-verify loop is
  sequential and stops at the first failure (P can succeed while I fails and D is never
  attempted). A single `NewTuneWrite` struct (all fields `pub`, built incrementally via
  `NewTuneWrite::new(response_level, written_at)` and one `TuneWriteRow::insert`) replaced an
  earlier `insert_success`/`insert_failure` two-function split once partial writes and
  rollback outcomes needed representing — two constructors can't express "wrote P, failed on
  I, rolled P back successfully" without one of them degenerating into the other's superset.
- **`OpcDaBackend` serializes access to one `opcda_bridge::Client` behind a `tokio::sync::Mutex`,
  never `std::sync::Mutex`.** The bridge client's methods take `&mut self`, but `Backend`'s
  methods take `&self` (required for `Arc<dyn Backend>` sharing), so the mutex guard is held
  across `.await` points — only `tokio::sync::Mutex`'s guard is `Send`, which `#[async_trait]`'s
  generated futures require by default. A single tuning session only ever has one read/write/
  browse in flight anyway, so serializing is not a real bottleneck.
- **`SimulatorBackend` uses `std::sync::Mutex`, not `tokio::sync::Mutex` like `OpcDaBackend`.**
  Its `read`/`write` bodies contain no `.await` points at all — they're `async fn` only because
  the `Backend` trait requires it — so nothing ever holds the guard across a suspension point,
  making the simpler std mutex both sufficient and correct. This is a genuine difference from
  `OpcDaBackend`, not an inconsistency: the tokio mutex there is load-bearing because its guard
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
  deliberately not wired into `SimulatorBackend`/`Backend`.** `SimulatorBackend` exists so a real
  `MrftEngine` can drive a synthetic process through the actual `Backend` trait; `VirtualPid` is a
  separate demo/validation utility proving the FOPDT model behaves like a real control loop under
  simple feedback (proportional-only exact-formula check, anti-windup, no derivative kick,
  full closed-loop convergence — the convergence gains were numerically pre-verified against a
  disposable Python script before being hardcoded, the same discipline used for `core-mrft`/
  `core-tuning-math`'s expected values). Wiring it into `Backend` would give `Backend` two
  unrelated jobs (being a `MrftEngine`'s tag I/O source, and running its own independent
  controller) for no real benefit.
- **`rand` 0.10 is configured `default-features = false, features = ["std", "std_rng"]`, and
  `StdRng` is seeded explicitly rather than using a thread-local RNG.** Every RNG in
  `bhtune-backend` is constructed via `StdRng::seed_from_u64`, so `thread_rng`/OS-entropy features
  are never used and stay disabled. `StdRng` was chosen over `SmallRng` specifically because
  `SmallRng`'s own documentation states its algorithm depends on the target's pointer size — a
  real cross-platform reproducibility risk for a Windows/macOS/Linux project — whereas `StdRng`'s
  only non-portability caveat is across `rand` crate versions, which is acceptable since CI only
  runs on `ubuntu-latest` (see `.github/workflows/checks.yml`/`coverage.yml`). No test hardcodes
  an exact noise value for this reason; tests only assert bounds and same-seed/different-seed
  equality/inequality, so a future `rand` upgrade changing `StdRng`'s internals can't break them.
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

## Simulator backend reference (`backend-simulator`)

`backend-simulator` is implemented in `crates/bhtune-backend/src/simulator.rs`: an in-process
FOPDT (first-order-plus-dead-time) process model plus a standalone virtual PID controller, served
through the real `Backend` trait as `SimulatorBackend`. No external process, no Windows, no
network I/O — every tick advances an internal virtual clock rather than sleeping on the wall
clock, which is what makes it usable for fast CI E2E runs.

- **`FopdtConfig`/`FopdtProcess`** — the process model: `gain`, `time_constant_s`, `dead_time_s`,
  `tick_interval_s`, and an optional noise amplitude. `step()` advances the model by exactly one
  tick using the exact closed-form discretization (see "Key architectural decisions" above), with
  dead time modeled as a `VecDeque<f32>` delay line. `mv()` reads the last-written MV without
  advancing anything; `write_mv()` sets it.
- **`VirtualPidConfig`/`VirtualPid`** — a standalone position-form PID controller (`Kc`, `Ti`,
  `Td`), derivative-on-measurement (matching the legacy Python reference, avoids derivative kick
  on a setpoint step), with anti-reset-windup (an integral increment is only committed if the
  resulting output didn't need clamping). Not wired into `SimulatorBackend`/`Backend` — see "Key
  architectural decisions" above for why it's kept as a separate demo/validation utility.
- **`SimulatorBackend`** — the `Backend` impl. Constructed with a PV tag name, an MV tag name, a
  `FopdtConfig`, initial PV/MV, and an RNG seed; wraps one `FopdtProcess` behind a
  `std::sync::Mutex` (see "Key architectural decisions" above for why not `tokio::sync::Mutex`).
  Reading the configured PV tag calls `FopdtProcess::step` (advances the simulated clock one
  tick); reading the MV tag returns `mv()` without advancing. Writing the MV tag accepts either a
  `TagWrite::Float` or a `TagWrite::Raw` that parses as `f32`; a non-numeric raw write is a
  rejected `WriteOutcome`, not a `BackendError`. Any other tag name is `BackendError::
  InvalidTagValue` on both read and write. `browse` is always `BackendError::Unsupported` — a
  synthetic two-tag process has no real tag tree to browse.

The FOPDT physics were ported from the legacy `Model` repo's `ProcessModelOPC.py` (the script the
legacy C# app's hidden `OPCClass.Python` debug branch actually shells out to), not reimplemented
from a textbook formula — see "Key architectural decisions" above for the closed-form
discretization and its numerical cross-check against that reference.

## CLI reference (`cli-commands`)

`bhtune-cli` (binary name `bhtune`) is a thin `clap`-derive orchestration layer over
`bhtune-core`/`bhtune-db`/`bhtune-backend` — every subcommand opens the same SQLite database
(`crate::db::open`, which also seeds the four built-in templates) and shares one dispatcher in
`lib.rs::run_with_cli`.

- **`bhtune tune`** — runs a full MRFT test against a named template: resolves the template,
  derives the tag set (`build_loop_tags`, in `commands/tune.rs`), selects a backend
  (`crate::backend::build`, `--backend opcda|simulator`), transitions the loop to Manual, polls
  at `--poll-interval-ms` driving a real `MrftEngine`, persists every tick
  (`TuneSampleRow::insert`) and the final per-response-level results
  (`TuneResultRow::insert`), restores the loop's original mode, and optionally writes back one
  response level's PID constants with a stdin confirmation prompt (`maybe_write_back`) —
  audited via `TuneWriteRow`. `--mrft-delay <seconds>` pads the run with pre-/post-test
  recording-only ticks (PV still read and logged; no switch evaluation), mirroring the legacy
  `--mrftDelayTime` flag. `--timeout-secs` adds a mandatory-unattended-operation guardrail —
  see "Safety" below. A run's outcome (`Completed`/`Aborted` on Ctrl+C or
  `--timeout-secs`/`Failed` on any setup or mid-poll error) is always recorded in `tune_runs`
  before the process returns, even on failure.
- **`bhtune simulate`** — a zero-configuration wrapper around `tune` that forces
  `--backend simulator` against a synthetic FOPDT process (`SIMULATOR_PV_TAG`/
  `SIMULATOR_MV_TAG`), for a demo/smoke-test run with no real DCS/PLC needed.
  `SimulateArgs::into_tune_args` converts to the same `TuneArgs` `tune` uses, so the two share
  every code path below template resolution.
- **`bhtune template list|show|import|export`** — inspect and manage `dcs_templates` rows
  (built-in and user-imported) as JSON, via `DcsTemplateRow`.
- **`bhtune history list|show`** — list past runs (optional `--outcome` filter, `--limit`/
  `--offset` pagination) and show one run's full detail (config, initial readings, calculated
  results, write-back audit rows), via `history-query-api`'s `TuneRunRow`/`TuneResultRow`/
  `TuneWriteRow` queries.
- **`bhtune history revert <run-id>`** — undoes a run's last PID write-back by writing its
  recorded pre-write values back to the live loop, under the same `--yes` confirmation gate
  as the original write-back; see the "`bhtune history revert <run-id>` — done" bullet under
  "Live-plant safety hardening" below for the full validation/behavior design
  (`safety-writeback-rollback`).
- **`bhtune export <run_id>`** — exports one run's recorded samples as CSV or JSON
  (`--format`), to stdout or `--output <path>`.
- **`bhtune opc read|write|browse`** — low-level passthrough straight to the `opcda-bridge`
  gateway (via `opcda_bridge::Client`, bypassing the tuning engine entirely) for diagnostics —
  the CLI equivalent of the legacy app's ad-hoc tag testing.

**What `cli-commands` deliberately does not cover** — each shipped as its own later phase,
not an oversight: `tracing`-based structured logging shipped separately as `cli-logging` —
see "Logging" below. Non-interactive automation flags (`--yes`/`--write-pid`/`--output json`)
and distinguished exit codes shipped separately as `cli-automation` — see "Automation" below.
Unattended-run safety guardrails (`--timeout-secs`/`LoopConfig::validate`) shipped
separately as `cli-safety` — see "Safety" below. Platform-standard config file/data-directory
precedence shipped separately as `cli-config` — see "Config precedence" below.

**Testing approach.** `commands/tune.rs`'s tests use a `MockBackend` (an in-memory
`Backend` impl with canned/erroring responses) for setup-and-validation-error paths, a real
`SimulatorBackend` for full happy-path runs (including the `--mrft-delay` padding test, which
necessarily costs a couple of real wall-clock seconds — `chrono::Utc::now()`, which
`pre_delay_end`/`post_delay_end` are computed from, is unaffected by tokio's pausable test
clock), and a shared test-only mock gRPC `Bridge` service (`crate::test_support`, used by
`backend.rs`, `tune.rs`, and `commands/opc.rs`) to prove the OPC DA path — connect, initial
reads, a mid-poll failure, and the `opc` passthrough commands — actually works end-to-end
without a real gateway or OPC DA server. A single canned mock read response satisfies every
setup read regardless of which tag was requested (see `OpcDaBackend::read`'s positional, not
tag-matched, mapping), which is what makes it possible to calibrate exactly which call number
a mock failure should start on. `test_support::MockBridgeService` supports configurable
streaming `browse` responses (a spawned per-request forwarder over an `mpsc` channel, mirroring
`opcda-bridge`'s own real streaming shape) so `commands/opc.rs` doesn't need — and no longer
has — its own separate mock implementation. `test_support::start_mock_server` returns a
`MockServerHandle` alongside the listening address; calling `.shutdown().await` signals the
server via a oneshot channel and awaits its task, so a mock server's lifetime is explicit and
bounded rather than only ever abandoned at test-process exit.

`run_polling_loop`'s `tokio::signal::ctrl_c()` select arm (and the `Aborted`-outcome branches
downstream of it in `run`/`execute`) is covered by `tests/ctrlc_abort.rs`, a black-box
integration test that spawns the real compiled `bhtune` binary as a child OS process (via
`Command::new(env!("CARGO_BIN_EXE_bhtune"))`, only available to `tests/*.rs` integration
targets, not `#[cfg(test)]` unit tests) and sends it a genuine `SIGINT` mid-poll — sidestepping
the risk of raising a real process signal *inside* `cargo test`'s own shared, multi-threaded
test binary (where a race between signal delivery and tokio's handler registration could
terminate the entire test process, not just one test). `cargo-llvm-cov` merges the spawned
child's coverage data automatically (its `%p`-templated `LLVM_PROFILE_FILE` is inherited and
resolved per-process at runtime), so this one test also closes `lib.rs::run()` and `main.rs`,
both of which require the real binary entry point to actually execute. The same technique is
reusable for any future OS-signal-dependent or entry-point-only code path.

`args.rs`'s tests share one `expect_variant!` macro (downcasting a parsed `Cli::command` to
the specific `Command`/`HistoryCommand` variant a test expects, panicking clearly otherwise)
instead of each of the four call sites carrying its own near-identical, individually-uncovered
`let-else { panic!(...) }` — collapsing four never-taken branches into one, which
`expect_variant_panics_on_a_mismatch` then exercises directly via `std::panic::catch_unwind`.

Coverage is genuinely 100% line-covered for this phase (verified directly against `lcov`
`DA:` records, not just the region-based `--summary-only` view, which has a known — and
harmless — aggregation quirk of reporting a small nonzero "Missed Lines" count for a handful
of files that `--show-missing-lines`'s per-line annotations and `lcov` both agree are fully
hit); no gap is accepted or left permanently unaddressed in this phase.
`args.rs`'s let-else panic branches — genuinely hard-to-test lines are named and accepted
rather than either skipped silently or chased at disproportionate risk.

## Config precedence (`cli-config`)

`crates/bhtune-cli/src/config.rs` resolves every global setting with `CLI flag > env var >
TOML config file > built-in default` precedence, deliberately mirroring
`opcda-bridge-client`'s own `config.rs` so both projects' configuration surfaces stay
recognizable to the same user. `bhtune --config <path>` loads an explicit TOML file (a
missing explicit path is a hard error); omitting `--config` auto-discovers one from a
platform-standard location, where a missing file silently resolves to all-defaults rather
than erroring (it may simply not have been created yet). A file that exists but fails to
parse as TOML is always a hard error in either case — a config typo should never be
silently ignored. See
[`crates/bhtune-cli/bhtune.example.toml`](crates/bhtune-cli/bhtune.example.toml) for every
available key.

Auto-discovered config file location (first one found wins):

- Linux/macOS: `$XDG_CONFIG_HOME/bhtune/bhtune.toml`, falling back to
  `$HOME/.config/bhtune/bhtune.toml`.
- Windows: `%APPDATA%\bhtune\bhtune.toml`.

| Setting               | CLI flag        | Env var             | Config key    | Default                                                                                                                                    |
| --------------------- | --------------- | -------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Database path         | `--db`          | `BHTUNE_DB`          | `db`           | Linux/macOS: `$XDG_DATA_HOME/bhtune/bhtune.db` (falls back to `$HOME/.local/share/bhtune/bhtune.db`); Windows: `%APPDATA%\bhtune\bhtune.db` |
| opcda-bridge gateway  | `--bridge-host` | `BHTUNE_BRIDGE_HOST` | `bridge_host`  | `localhost:7600`                                                                                                                            |
| Default OPC DA server | `--server`      | —                    | `server`       | none — must be set one way or another for `tune --backend opcda` and the `opc` subcommands                                                 |

`resolve_db_path`/`resolve_bridge_host` fold the env var into the CLI value already (via
clap's `env` attribute on `Cli::db`/`TuneArgs::bridge_host`/`OpcCommand`'s per-variant
`bridge_host`), so each `resolve_*` function itself only has two tiers left to arbitrate:
the (already env-merged) CLI value versus the config file. `resolve_server` errors if
neither the CLI nor the config file supplies a value — there's no sensible default OPC
server to fall back to — and is applied only for the `Opcda` backend inside
`commands::tune::run` (never for `simulate`, which has no OPC server concept at all; a
config-file `server` key is simply not consulted for a simulator run rather than causing an
unrelated error).

`db::open` gained `ensure_parent_dir`, creating the database path's parent directory tree
(`std::fs::create_dir_all`) before connecting — needed once the default database path could
be a nested, not-yet-existing platform directory (e.g. a fresh install's
`~/.local/share/bhtune/`) rather than always a path the caller already ensured existed.
`Path::parent()` returns `Some("")` for a bare filename with no directory component, and
`create_dir_all("")` is a documented no-op success, so no special-casing is needed for that
degenerate input.

**Coverage note.** `db.rs`'s `ensure_parent_dir` shows one line as "missed" in
`--summary-only` (the closing `}` of its `if let Some(parent) = ...` block) — cross-checked
directly against the annotated per-line report and confirmed as the same harmless
`cargo-llvm-cov` line-attribution quirk noted elsewhere in this file (a bare closing brace
with no executable content of its own, reported separately from the block's own hit count).
The block's actual branches are both genuinely exercised: the success path 13 times and the
`map_err`/`?` failure path exactly once, via the existing
`run_with_cli_config_load_failure_is_exit_failure` test's unwritable `/nonexistent-dir/`
database path — not a real gap.

## Automation (`cli-automation`)

`bhtune tune`/`bhtune simulate` support fully non-interactive operation for scheduled/scripted
use (`cron`, Windows Task Scheduler, CI), and `bhtune history list`/`show`/`revert` support
machine-readable output for the same callers:

- **`--yes`** — required before `--write-pid` is honored at all; see below.
- **`--write-pid <aggressive|moderate|sluggish>`** — writes that response level's calculated
  PID constants back to the DCS without the interactive stdin confirmation prompt
  `maybe_write_back` otherwise uses. Requires `--yes`; `run()` rejects the combination with a
  hard `Err` as its very first statement, before any backend connection or database write —
  an unattended write-back must be an explicit, deliberate choice, not a stray flag. If the
  named response level has no recorded calculated result (defensive; not reachable through
  normal CLI validation), the write-back is reported as failed rather than attempted, exactly
  as an invalid interactive selection already was.
- **`--output <table|json>`** — on `tune`/`simulate`, the final summary line; on
  `history list`/`show`, the whole listing/detail; on `history revert`, the pre-attempt
  status line and the final outcome (a `RevertJson` object). `table` is the default and
  preserves the original plain-text shape exactly. `json` prints one
  `serde_json::to_string_pretty` object (or array, for `history list`) to stdout — never a
  mix of the two on one invocation. Local DTOs (`RunSummaryJson`/`RunListJson`/
  `InitialReadingsJson`/`ResultJson`/`WriteJson`/`RunDetailJson`/`RevertJson`/
  `RevertedTargetJson` in `commands/history.rs`) project the `bhtune-db` row types that don't
  themselves derive `Serialize` (DB row shape stays deliberately decoupled from any API/CLI
  JSON shape); `bhtune-core` enums and `LoopConfig`/`TuneBackend`/`TuneOutcome` already derive
  `Serialize` and are reused directly.
- **Exit codes** — `lib.rs` defines `EXIT_SUCCESS = 0`, `EXIT_FAILURE = 1` (a setup error:
  unknown template, invalid flag combination, database/backend connection failure — anything
  `run()` returns as `Err`), `EXIT_ABORTED = 2` (Ctrl+C), `EXIT_WRITE_BACK_FAILED = 3`
  (the test itself completed, but the requested PID write-back failed — rejected write,
  failed confirmation readback, or the defensive missing-result case above), `EXIT_TIMED_OUT
  = 4` (`--timeout-secs` elapsed before the test finished), `EXIT_POOR_QUALITY = 5` (a
  non-`Good` OPC sample aborted the run — see the OPC-quality bullet under "Live-plant safety
  hardening" above), and `EXIT_RESTORE_INCOMPLETE = 6` (the post-run restore could not be
  confirmed within `--restore-timeout-secs`, or was cut short by a second Ctrl+C — see
  `safety-cancellation` above; kept distinct from `EXIT_ABORTED` since "aborted and restored"
  and "aborted, restore abandoned — go check the loop by hand" are very different outcomes
  for a scheduler to alert on). `tune_outcome_exit_code` maps `commands::tune::TuneOutcome`
  (`Completed`/`Aborted`/`TimedOut`/`WriteBackFailed`/`PoorQuality`/`RestoreIncomplete`,
  returned by `run()` on the `Ok` path) to the process's actual
  `ExitCode`; `fail()` handles the `Err` path and always prints the error in the format
  `--output` requested before returning `EXIT_FAILURE`. **The database's own
  `tune_runs.outcome` column only ever records `Completed`/`Aborted`/`Failed`** —
  `TuneRunRow::complete` runs *before* the optional write-back attempt, so a write-back
  failure changes the process's exit code and the printed summary but never retroactively
  rewrites an already-`Completed` run's DB outcome to look like the whole test failed.
- **`--write-pid`/`--yes` on `bhtune simulate`** are accepted (for a uniform flag surface with
  `tune`) but always a no-op: the built-in simulator has no PID constant tags configured at
  all (`build_loop_tags` leaves them all `None` for `BackendKindArg::Simulator`), so write-back
  is unconditionally `WriteBackOutcome::Skipped` regardless of these flags.

**Testing approach.** `tune_outcome_for_run`/`print_summary` are pure/near-pure functions
(the latter's only side effect is the `println!` itself) tested directly against every
`RunOutcome` x `OutputFormat` combination, rather than only through a full `run()`. A genuine
end-to-end test of `run()` reaching a real `WriteBackOutcome::Written`/`Failed` through the
actual polling loop is structurally impossible with current test infrastructure: the mock OPC
DA bridge only ever returns static PV values (can never trigger a real relay switch), and
`SimulatorBackend` structurally has no PID tags at all (see above) — so
`a_full_simulator_tune_with_write_pid_and_yes_still_skips_write_back` proves the flag
combination is a harmless no-op against the simulator, while `maybe_write_back`'s own
non-interactive `--write-pid` branch (including the "requested level has no recorded result"
case) is tested directly via the `run_with_recorded_results()` fixture instead of chasing a
full E2E. `tests/ctrlc_abort.rs` (see below) asserts the real subprocess exits with
`EXIT_ABORTED`, not `0`, closing the loop on the one `TuneOutcome` variant `print_summary`'s
own unit tests can't reach through a real `run_polling_loop` execution.

## Safety (`cli-safety`)

Guardrails for unattended operation against live plant equipment — the legacy app is always
human-attended (an operator watches the trend and can hit Stop); scheduled/scripted tuning
removes that supervision while still stroking a live valve, so these are not optional polish:

- **`LoopConfig::validate`** (`bhtune-core`) — real range validation on `relay_amp_percent` at
  the model/construction level, not just a client-side keystroke filter or a single "not
  blank" check (the legacy predecessor's bug: a leftover 2014/2015/2016 debug code left in the
  Relay Amplitude box passed its only check). `RELAY_AMP_PERCENT_MIN = 0.1`/
  `RELAY_AMP_PERCENT_MAX = 50.0` (both `pub const f32` on `LoopConfig`) reject non-finite
  values and anything outside that range; `LoopConfigError` is a hand-rolled `Display`/
  `std::error::Error` enum (no `thiserror` — `bhtune-core` stays dependency-minimal) — the
  crate's first fallible-construction pattern (previously only `Option`-returning functions
  existed, e.g. `derive_tag`). `build_loop_config` (`commands/tune.rs`) calls `.validate()?`
  immediately after constructing the `LoopConfig`, before any backend connection or database
  write, mirroring the `--write-pid`-requires-`--yes` fail-fast precedent below.
- **`--timeout-secs <seconds>`** (default `3600`) — a mandatory wall-clock limit on the whole
  test, with no disable/unlimited option; a value of `0` just means an (essentially unusable)
  instant timeout, preserving genuine "mandatory" semantics. Implemented in
  `run_polling_loop` as a `tokio::time::sleep` created once before the loop and raced via a
  `tokio::select!` arm alongside `interval.tick()` and a single process-wide `CtrlC` handle
  (see `safety-cancellation` below) — but that outer race only covers the *idle* wait between
  ticks. The timeout (and Ctrl+C) also stay effective *during* a tick — including a stalled
  backend read or write, e.g. a wedged DCOM call or a black-holed network — because every
  backend call inside the tick body is separately raced against the same `CtrlC` handle and a
  `--op-timeout-secs` cap via `bounded_backend_call` (see `safety-cancellation` for why this
  two-layer design, rather than one that only checked after each completed tick, was needed).
  On firing, the loop is restored to its pre-test mode via the exact same path as a Ctrl+C
  abort (`restore` + `TuneRunRow::abort`, recording plain `Aborted` in `tune_runs.outcome` —
  no new DB state) and reported to the caller as the distinct `TuneOutcome::TimedOut` /
  `EXIT_TIMED_OUT = 4`, so a scheduler's alerting can tell "this run had to be forcibly killed
  for running too long" (possibly a stuck relay, a misconfigured tag mapping, or a stalled
  backend read — worth investigating) apart from "an operator stopped it on purpose"
  (`EXIT_ABORTED`, routine).
- **`--write-pid <level>` unconditionally requires `--yes`** — in `run()`
  (`args.write_pid.is_some() && !args.yes`), checked before any backend connection or
  database write. There is no rehearsal mode that lifts this gate: a `--write-pid` run either
  has explicit confirmation or is rejected outright. (An earlier `--dry-run` flag did lift
  this gate, but it was removed since it did not actually avoid touching the live loop: it
  still forced the mode transition and stroked the MV through a full relay test, only
  skipping the final PID write.)

**Testing approach.** `run_times_out_and_aborts_when_timeout_secs_elapses_before_completion`
exercises the real timeout end-to-end (`poll_interval_ms: 3`/`timeout_secs: 1`, deliberately
not a multiple of each other so the deadline never lands exactly on a tick boundary, plus a
`cycles_count` far too high to legitimately complete in the ~333 ticks available) — this pays
a real ~1s wall-clock cost rather than using `#[tokio::test(start_paused = true)]`, because
pausing tokio's clock also fast-forwards the real `SqlitePool`'s own internal connection-
acquire timeout, turning every query into a spurious `PoolTimedOut` error; `tests/
ctrlc_abort.rs` already accepts a similar real-time cost for the same underlying reason (an
actual signal/timeout has to actually elapse). `build_loop_config_rejects_an_out_of_range_
relay_amp_before_any_backend_or_db_io` mirrors the same "no I/O before the fail-fast check"
pattern now also proven for `--write-pid`/`--yes` (`run_rejects_write_pid_without_yes_before_
starting_the_tune`).

### Live-plant safety hardening (in progress)

A post-`cli-logging` review of the live-tuning path (`commands/tune.rs`) surfaced nine
further findings before the CLI's first real trial against live plant equipment: Ctrl+C/
timeout cancellation not reaching an in-flight backend call, no guaranteed restore on every
exit path, missing input validation (e.g. `--cycles-count 0` panics mid-run), OPC quality
never checked, PID write-back with no pre-read/rollback, `bhtune-db`'s `restore_from` unsafe
under an active WAL and wrong on Windows, `--output json` emitting prose ahead of the JSON
object, and no template/tag snapshot on a recorded run. Being remediated one finding at a
time; this section is updated as each lands, with a full pass once all are done:

- **`--dry-run` removed entirely** — done. It was documented as never touching the DCS, but
  actually forced the full mode transition and stroked the MV through a complete relay test,
  skipping only the final PID write — indistinguishable from a real test for every purpose
  except the last write. No rescoped/renamed replacement was added: "runs a real relay test
  but skips one write" is already exactly what omitting `--write-pid` does in non-interactive
  mode. A genuinely non-mutating rehearsal (validate tags/template/ranges/connectivity with
  no loop I/O at all) remains on the roadmap as a separate future command, not a flag on a
  live tune.
- **No externally supplied number reaches the engine unvalidated** — done
  (`bhtune-core::range`, `LoopConfig::validate`, `bhtune-cli::args` value parsers,
  `commands::tune::validate_initial_state`). Previously `--cycles-count 0` reached
  `tuning_math::measure_oscillation`'s internal `assert!` and panicked *after* the loop had
  already been switched to manual and stroked through a full relay test, with no restore on
  the panic path; ranges read from the backend or passed as flags were never checked for
  finiteness or ordering, so a `NaN` parsed by `f32::from_str` (which silently accepts the
  literal strings `"nan"`/`"inf"`) could flow into the tuning math and, ultimately, a PID
  write. Closed in four layers:
  - `bhtune_core::range` — new `PvRange`/`MvRange` validated newtypes, each with a
    `::new(high, low) -> Result<Self, RangeError>` constructor. `PvRange` only requires the
    bounds be distinct (it is used purely as a span magnitude); `MvRange` requires strict
    `low < high`, since `mrft::clamp_relay_amplitude`'s boundary math assumes that
    orientation. Both types keep their fields `pub` (unvalidated construction via a struct
    literal is still possible in-crate) — validation is enforced only at the
    untrusted-input boundary (`::new()`), not universally.
  - `LoopConfig::validate()` — extended to reject `cycles_count < 1` and
    `mrft_delay_secs > MRFT_DELAY_SECS_MAX` (3,600s, matching the default `--timeout-secs`),
    alongside the existing relay-amplitude check.
  - `bhtune-cli::args` — `finite_f32`/`positive_u32`/`positive_u64` clap `value_parser`
    functions applied to every numeric flag on `TuneArgs`/`SimulateArgs` that reaches the
    engine (relay amp, cycles count, the PV/MV range bounds, the simulator's process
    parameters, poll interval, timeout). Rejects `NaN`/infinite/zero/negative input with a
    clear message before any I/O. Deliberately *not* applied to `mrft_delay`, `cycles_skip`,
    `noise_protection_secs`, or `sim_seed` — each is either bounded only at the model level
    or has no invalid range at the CLI layer (`0` is a legitimate RNG seed).
  - `commands::tune::validate_initial_state` — a new checkpoint between
    `read_initial_values` and `transition_to_manual` (the single choke point before any
    mutation of the live loop) that validates the resolved `InitialState` uniformly,
    regardless of whether each value came from a CLI flag or a backend tag: constructs
    `PvRange`/`MvRange` from the read ranges and confirms the initial MV falls inside the
    validated MV range. `read_f32`/`resolve_f32` additionally reject non-finite parsed
    values directly, closing the `"nan"`/`"inf"` string-parsing gap before a value is even
    assembled into `InitialState`.

  An `execute()`-level integration test proves the actual safety property end-to-end: a
  backend reporting an inverted MV range (`low >= high`) causes `execute` to fail with no
  entries at all in the backend's write log — i.e. `transition_to_manual` never runs, not
  merely "the tuning math never runs".
- **Every run now snapshots the template it was configured against, not just its name** —
  done. `tune_runs` recorded no template or tag information at all, so a historical run
  could not be reinterpreted once the template catalog underneath it changed — a real
  concern given Phase 6.6 makes catalog edits routine. `tune_runs` gained four columns:
  `template_name`/`template_origin` (flat, filterable — the same denormalized-for-`WHERE`
  precedent as the existing `loop_name` column) plus `template_snapshot_json`/`tags_json`
  (the full serialized `DcsTemplate`/`LoopTags`, `CHECK (json_valid(...))`-constrained, for
  exact reproduction even after the template type itself gains fields). No foreign key to
  `dcs_templates`: a run must stay interpretable even if that row is later renamed or
  deleted. `bhtune_db::models::TemplateOrigin` (`Builtin`/`Catalog`/`User`) captures where a
  template came from; `TuneRunRow::start` now takes `template_origin`/`template: &DcsTemplate`/
  `tags: &LoopTags` alongside the existing `config`, serializing the latter two with
  `.expect(...)` on the same "infallible because upstream validation already guarantees
  every `f32` is finite" basis as `enum_to_text`. A new `DbError::InvalidJsonShape` variant
  covers the case where a stored blob is syntactically valid JSON (guaranteed by the schema)
  but no longer deserializes into the current `DcsTemplate`/`LoopTags` shape. `bhtune history
  show` (not `list`, to keep the list view narrow) prints the snapshotted template name and
  origin alongside the run's other identity fields, from `RunDetailJson`/the plain-text
  table.
- **OPC quality now enforced on every tuning-critical read** — done
  (`commands::tune::check_quality`, `bhtune_db::models::SampleQuality`). Previously
  `bhtune_backend::Quality`/`is_trustworthy()` existed but nothing in the tune path ever
  called it — a tag reporting `Uncertain` (a stale held-last-value during a comms hiccup) or
  outright `Bad` quality flowed into the MRFT engine and a PID write-back exactly like a
  trustworthy `Good` reading. `check_quality` is now the single choke point: `Good` always
  passes; `Bad` is never accepted, `--allow-uncertain-quality` or not; `Uncertain` is
  accepted only with that flag set, logging a loud `tracing::warn!` every time so a run
  executed under relaxed rules is never silently indistinguishable from a normal one. Wired
  through every read that feeds a tuning decision:
  - `read_initial_values`/`transition_to_manual`'s setpoint read — a poor-quality reading
    before any mutation of the loop is a hard failure (a plain `anyhow::Error`), since
    nothing has been mutated yet and there is no loop state to restore.
  - The in-flight MRFT poll loop (`run_polling_loop`) — a poor-quality PV sample here *does*
    abort the run (a new `AbortReason::PoorQuality { tag, quality }`, restored and recorded
    exactly like a Ctrl+C/timeout abort), but the triggering sample is still recorded to
    `tune_samples` (with its real, poor quality) *before* the abort, via a new
    `read_pv_sample` helper that returns quality without hard-failing on it, so the future
    history explorer can show exactly what was seen when the run gave up.
  - The PID write-back confirmation readback (`maybe_write_back`) — a poor-quality readback
    is classified as a write-back failure (`WriteBackOutcome::Failed`, audited via
    `TuneWriteRow`), never silently accepted as proof the write landed. A poor-quality
    readback also drives finding 6's rollback of whatever was already confirmed, below.

  `tune_samples` gained a `pv_quality` column (`SampleQuality`: `Good`/`Uncertain`/`Bad`, the
  DB-side mirror of `bhtune_backend::Quality` — two separate enums since `bhtune-backend` and
  `bhtune-db` are sibling crates, neither depending on the other) and `tune_runs` gained
  `allow_uncertain_quality`, so a run's quality posture is part of its permanent history.
  `bhtune tune --allow-uncertain-quality` is the CLI flag; a poor-quality abort exits with
  `EXIT_POOR_QUALITY` (5), distinct from a Ctrl+C/timeout abort, and `--output json` carries
  nullable `poor_quality_tag`/`poor_quality` fields alongside the existing `timeout_secs`.
- **Ctrl+C and `--timeout-secs` now reach an in-flight backend call, and the restore itself
  is bounded** — done (`bhtune-cli::cancel`, `commands::tune::{bounded_backend_call,
  attempt_restore}`). Previously the signal listener and the timeout sleep were both
  reconstructed fresh on every polling-loop iteration, inline in a `tokio::select!` — so for
  the entire duration of a tick's body (the PV read, the relay MV write, the sample insert)
  neither existed, and a Ctrl+C delivered in that window was silently lost (tokio coalesces
  signal delivery per kind, and a `Signal` future created *after* delivery never observes
  it), with no fallback to the OS's default terminate-on-SIGINT behavior either (tokio
  replaces it process-wide the first time `ctrl_c()` is ever polled, and never reverts it). A
  hung backend read made the loop uninterruptible outright — exactly the scenario
  `--timeout-secs` was introduced to prevent, and the very claim ("fires even mid-hung-read")
  that this fix makes true rather than aspirational. Closed in three parts:
  - `bhtune_cli::cancel::CtrlC` — one process-wide Ctrl+C listener, installed exactly once at
    real startup (`CtrlC::install`, called only from `crate::run`, never from a function unit
    tests exercise) and threaded explicitly through `execute`/`run_polling_loop`/
    `attempt_restore` as `&mut CtrlC` rather than each calling `tokio::signal::ctrl_c()`
    itself. Built on `tokio::sync::watch` (not `tokio_util::sync::CancellationToken`, which
    would add a dependency) specifically for its per-clone "have I observed this value yet"
    semantics: `CtrlC::signalled()` resolves immediately for a signal that arrived at any
    point before that call — including before the handle's first call at all — and a
    *second* signal is a second, distinguishable resolution on the same handle, which is
    exactly the "first Ctrl+C aborts, second forces a hard stop" distinction below needs.
    `CtrlC::never()`/`CtrlC::test_pair()` back the test-only `run`/direct-call entry points,
    so the many unit tests never install a real process-wide signal handler (which would
    otherwise risk swallowing a developer's own Ctrl+C to a hung `cargo test`).
  - `bounded_backend_call`/`TickOperation` — races one backend call (the tick's PV read, or
    its MV write) against `ctrl_c.signalled()` and a fresh `--op-timeout-secs` sleep (new
    flag, default 30s, capping a single operation rather than the whole run), returning
    `Completed(T)`/`Cancelled`/`TimedOut`; a genuine `Err` from the call itself still
    propagates via `?` rather than being folded into this enum, since a rejected write or a
    transport error is a real failure, not "gave up waiting". `run_polling_loop`'s outer
    `tokio::select!` (covering the *idle* wait between ticks) reuses the exact same `&mut
    CtrlC` handle passed down into the tick body's `bounded_backend_call`s, which is safe
    specifically because a tokio `watch::Receiver`'s "seen this value" state advances the
    moment either `select!` observes it — there is no way for the outer and an inner
    `select!` to each separately consume the same signal.
  - `attempt_restore`/`RestoreAttempt` — wraps `restore()` in the same race, against a new
    `--restore-timeout-secs` (default 30s, independent of `--op-timeout-secs`/
    `--timeout-secs`, since a restore triggered *by* a timeout would otherwise inherit an
    already-expired budget) and `ctrl_c.signalled()` again — a *second* Ctrl+C during the
    restore is what "forces a hard stop" means in practice, since the restore is the one
    thing that keeps running after the first signal aborts polling.
    `RestoreAttempt::Incomplete { reason }` (timeout or second-Ctrl+C, distinguished only by
    `reason`'s text) prints an operator-facing `eprintln!` naming the MV tag and its
    pre-test value plus a structured `tracing::error!`, and becomes a new
    `RunOutcome::RestoreIncomplete`/`TuneOutcome::RestoreIncomplete`, exiting
    `EXIT_RESTORE_INCOMPLETE` (6) — distinct from `EXIT_ABORTED` (2), since "aborted and
    restored" and "aborted, restore abandoned, go check the loop by hand" are very different
    outcomes for a scheduler to alert on.

  **Testing approach.** A `MockBackend.hanging_read`/`hanging_write` (awaits
  `std::future::pending::<()>()` before ever reaching its own bookkeeping, so a hung call is
  provably never recorded even though the abandoned future is only dropped, not signalled)
  backs two new `run_polling_loop` integration tests: a stalled PV read aborting via
  `--op-timeout-secs` with no sample recorded (no valid tick exists yet), and a stalled MV
  write being cancelled by a `CtrlC::test_pair()`-driven background task (standing in for a
  human pressing Ctrl+C mid-write) while still recording the sample from that tick's earlier,
  already-completed PV read. `bounded_backend_call`/`attempt_restore` also each have direct
  unit tests exercising all of their outcomes in isolation (completed/cancelled/timed-out/a
  genuine error propagating; confirmed/incomplete-via-timeout/incomplete-via-second-Ctrl+C),
  and one `run_with_ctrl_c` test exercises the real (non-test-only) entry point end-to-end
  with a simulated signal, rather than only through the `CtrlC::never()`-backed `run` every
  other test in the module uses.
- **Every exit path now funnels through one best-effort, all-steps-attempted restore** — done
  (`commands::tune::{MutationGuard, RestoreReport, RestoreStepOutcome, restore, execute}`,
  `bhtune_db::models::{RestoreStatus, TuneRunRow::record_restore_status}`). Previously
  `execute()` could transition a loop to manual and then return without ever calling
  `restore()` at all — any `?` between the transition and the polling loop (the
  `record_initial_readings` DB write, engine construction), or a `persist_results`/`complete`
  failure *after* a genuinely completed test — and `restore()` itself returned on its first
  failure, so a single rejected MV write pre-empted even *attempting* to put the mode back.
  Closed in three parts, matching the design's "A + C + D" decision:
  - **`MutationGuard`** (Option A) — a plain struct of four booleans
    (`mode_attribute_written`/`mode_written`/`mv_written`/tracks whether a setpoint was
    captured), armed the instant each corresponding write actually succeeds, never
    optimistically before. `execute()`'s mutating body was split into an inner function
    returning `Result<_, (anyhow::Error, MutationGuard)>` — the guard travels *with* the
    error on every failure path — so the outer function can unconditionally consume
    whatever guard state exists (fully armed, partially armed, or the zero value from a
    failure before any write) and call `restore()` accordingly on every single exit, with no
    path that skips it. Nothing is ever "restored" that the guard doesn't say was actually
    changed.
  - **`RestoreReport`/`RestoreStepOutcome`** (Option C) — `restore()` now attempts all four
    steps (MV, mode, setpoint, mode attribute) unconditionally rather than returning on the
    first `Err`, collecting each step's own `RestoreStepOutcome`
    (`NotNeeded`/`Succeeded`/`Failed(String)`). The MV step is never gated by the guard (a
    relay-stroked MV always gets written back, since nothing else in the guard implies it
    wasn't touched); the mode/setpoint/mode-attribute steps are each gated by their own
    guard flag *and* a value-based precondition (e.g. the mode-attribute step only fires if
    the read-back program value actually differs from what's already there), so a step whose
    guard flag was never armed correctly reports `NotNeeded`, distinct from an armed-but-
    failed `Failed`. `RestoreReport::failure_summary()` names every failed step by label
    (`"MV: ...; mode: ...; setpoint: ...; mode attribute: ..."`) rather than collapsing to
    "something failed", so an operator reading `bhtune history show` knows exactly what to
    check by hand.
  - **Durable restore intent** (Option D, partially done) — `TuneRunRow::record_initial_readings`
    now persists `mode_raw`/`mode_attribute_raw`/`setpoint_ini` (the loop's pre-mutation
    mode/mode-attribute/setpoint, mirroring the existing `pv_ini`/`mv_ini`/range columns)
    *before* `transition_to_manual`'s first write, not after — so a process that dies
    outright (SIGKILL, power loss, a second Ctrl+C during an already-incomplete restore) still
    leaves a durable, reconstructable record of what needs to be put back, not just an
    in-memory `MutationGuard` that dies with the process. New `restore_status`
    (`RestoreStatus::Confirmed`/`Incomplete`) and `restore_detail` columns on `tune_runs`
    record the outcome of the post-run restore attempt itself (`None` means no restore was
    ever attempted — either nothing was mutated, or the run is still in progress), surfaced
    in `bhtune history show`'s table and JSON output. **Not yet done:** the
    `bhtune restore-loop --run <id>` replay command the design calls for, to actually act on
    that persisted intent later. Deliberately deferred — finding 6's own "read historical
    values, write them back under a confirmation gate" command,
    `bhtune history revert <run-id>`, is now implemented (see below), and shares enough
    shape with a future `restore-loop` that it is worth revisiting whether the two should
    share code once `restore-loop` is actually built, rather than assuming up front.

  **Testing approach.** A direct unit test on `restore()` (bypassing `execute()` entirely,
  via a hand-constructed fully-armed `MutationGuard` and a backend where all four writes
  fail) proves every step is attempted independently and the summary names all four. Three
  `execute()`-level integration tests cover the guard's actual exit paths:
  `transition_to_manual` failing on its very first write (before the mode-revert path is
  ever armed) still runs the unconditional MV restore step, leaves `MODE` untouched, and
  records `Incomplete` with a "mode attribute" detail; a `persist_results`/`complete`
  failure after a genuinely completed simulator test still attempts the restore and records
  `Confirmed`; and a poor-quality abort partway through polling (via a new
  `MockBackend::degrade_quality_after` test-harness extension, returning a tag's quality as
  `Good` for the first N reads and a chosen `Quality` after) still runs the restore end to
  end and records `Incomplete`.
- **PID write-back now pre-reads, verifies against tolerance, and rolls back a partial
  write** — done, core rewrite (`commands::tune::{read_previous_pid_values,
  pid_value_within_tolerance, write_and_verify_pid_value, rollback_pid_writes,
  maybe_write_back}`, `bhtune_db::models::{NewTuneWrite, RollbackState}`). Previously the
  three constants were written in sequence with no pre-read at all: if P succeeded and I was
  rejected, the loop was left with a mismatched, half-updated set and no way to know what P
  used to be. A transport error during the confirmation readback propagated via `?` and
  skipped the audit row entirely — the single most alarming failure mode was the one least
  likely to be recorded. "Confirmation" itself only checked that three values parsed as
  numbers, without checking quality or how close they were to what was requested, so a
  clamped or stale readback was indistinguishable from genuine confirmation. Closed as:
  - **Pre-read is a hard stop.** `read_previous_pid_values` reads P, then I, then D
    (subject to finding 5's quality rule), failing on the first bad read before anything is
    written — the run's `previous` values are always fully known or the write never starts,
    so a rollback target always exists once anything has been written.
  - **Write, verify, and check tolerance, one constant at a time.** `write_and_verify_pid_value`
    reuses `write_value` (so a transport error and a rejected write both surface the same
    way, rather than a raw `?` skipping the audit row) and `read_f32` (so a poor-quality
    confirmation read is never mistaken for success), then checks the readback against
    `pid_value_within_tolerance` — a combined absolute (`1e-3`) and relative (1%) tolerance,
    rather than exact equality, since a DCS's own unit conversion means a just-written float
    is not guaranteed to read back bit-identical, and a purely relative tolerance breaks
    down for a requested value at or near zero (e.g. `D = 0` on a PI controller).
    `maybe_write_back` calls this once per constant, in P/I/D order, stopping at the first
    failure — implemented as a loop over a fixed 3-element array of `(label, tag, requested,
    previous)` tuples with index-based `[Option<f32>; 3]` temporaries for the written/readback
    values, rather than string-matching on `label`, then unpacked into `NewTuneWrite`'s named
    fields once the loop ends.
  - **Roll back only what was actually confirmed.** A constant is only added to
    `rollback_targets` after its own write-and-verify succeeds, so if P succeeds and I fails,
    only P is rolled back — D, never attempted, needs no rollback and I, never confirmed,
    has nothing to put back either. `rollback_pid_writes` mirrors `restore()`'s "attempt
    every step independently" philosophy from the previous bullet rather than stopping at
    the first rollback failure, collecting every failure so a rollback that only partially
    succeeds is still fully reported.
  - **Four distinguishable outcomes**, not just success/failure: wrote nothing (pre-read
    failed, `previous = None`, `rollback_state = None`); wrote and confirmed everything
    (`success = true`); wrote some, failed, rolled back successfully
    (`rollback_state = Succeeded`); and wrote some, failed, and the rollback *itself* failed
    (`rollback_state = Failed`, `rollback_error` set) — printing a message pointing the
    operator at `bhtune history revert <run-id>` for the last case, since the loop may now
    hold a mismatched set of constants with no automated way left to fix it.
  - **`tune_writes` gained five columns**: `proportional_previous`/`integral_previous`/
    `derivative_previous` (nullable — the pre-read itself can fail) and `rollback_state`
    (`CHECK` constrained to `succeeded`/`failed`, `NULL` meaning rollback was never needed)/
    `rollback_error`. The existing `proportional_written`/`integral_written`/
    `derivative_written` columns were relaxed from `NOT NULL` to nullable, since a partial
    write can now leave a later constant's `written`/`readback` genuinely absent rather than
    forced to some placeholder value — added to the one pre-release migration in place,
    since nothing has shipped yet.

  **Testing approach.** `MockBackend` gained three more builders alongside the existing
  `degrade_quality_after`: `erroring_read_after`/`rejecting_write_after` (a tag's first N
  reads/writes succeed normally, then every one after that fails — letting a test put a
  tag's *pre-read* in good standing while still forcing its *post-write* readback or a later
  *rollback* write to fail deterministically) and `distorting_write` (silently perturbs a
  written float by a fixed offset before storing it, so a readback that parses fine and
  reports `Good` quality can still be exercised as an out-of-tolerance rejection — a failure
  mode distinct from an erroring or poor-quality readback that no prior mock capability could
  produce). Dedicated tests cover all nine `maybe_write_back` outcomes: a full success
  (asserting `previous`/all three `*_written`/`*_readback` fields and `rollback_state = None`
  together, not just the top-level `WriteBackOutcome`), a pre-read failure (`previous = None`,
  nothing on the backend's write log at all), a rejected write, a readback that errors after
  the pre-read has already succeeded, a poor-quality readback (distinguished by message
  prefix from both the read-error case and a pre-read failure), an out-of-tolerance readback,
  a successful rollback (confirming the backend's live value was actually restored, not just
  the audit row), a rollback that itself fails (`rollback_state = Failed` with
  `rollback_error` naming the constant), and an `Uncertain` readback accepted without
  incident under `--allow-uncertain-quality` (proving finding 5's escape hatch and finding
  6's tolerance check compose correctly rather than the latter accidentally re-imposing a
  `Good`-only rule of its own). `pid_value_within_tolerance` also has direct unit
  tests pinning down its exact-match, relative-band, absolute-floor-near-zero, and
  negative-value behavior.

- **`bhtune history revert <run-id>` — done** (`commands::history::revert`,
  `bhtune_db::models::WriteKind`). Undoes a past PID write-back by writing the run's
  recorded pre-write values back to the live loop, so a write-back that turns out to have
  been wrong can be corrected days later without anyone having written the old numbers down
  by hand.
  - **`tune_writes` gained a `kind` discriminant** (`WriteKind::Write`/`Revert`, `CHECK`
    constrained, defaulting to `Write` in `NewTuneWrite::new`) rather than a new table — a
    revert is structurally identical to a write-back (same pre-read/write/verify/audit
    shape), just run against a historical target instead of a freshly calculated one, and
    `tune_writes` has no `UNIQUE (run_id, response_level)` constraint blocking a second row
    at the same response level. `history show`'s write-back audit listing now prints each
    row's kind alongside its response level (`Write (Moderate level)` /
    `Revert (Moderate level)`) so a revert is never mistaken for the original write.
  - **Validates before ever connecting to the backend.** In order: the run exists; the run
    used the `Opcda` backend (a `Simulator`/`Replay` run has no live loop to revert against);
    the run has at least one recorded `Write`-kind row (nothing to revert otherwise); that
    row's `previous` is `Some` (a write whose own pre-read failed has nothing recorded to
    revert to); `--yes` was passed (reverting writes to a live loop, same confirmation gate
    as the original write-back); the run's snapshotted tags have all three PID constant tags
    configured. Only after all six checks pass does it resolve `--bridge-host`/`--server`
    and call `OpcDaBackend::connect` — so four of these checks are exercised in tests with no
    mock backend running at all, and even the connection-failure path itself is a genuine
    test (an unreachable host, proving every earlier check passed).
  - **Reuses `commands::tune`'s own pre-read/write-and-verify helpers directly**
    (`read_previous_pid_values`, `write_and_verify_pid_value`, promoted from private to
    `pub(crate)` for this purpose) rather than re-implementing them, so a revert's pre-read,
    tolerance check, and per-constant failure semantics are identical to the original
    write-back's by construction, not by parallel maintenance. Like the original write-back,
    a revert pre-reads the loop's *current* live values first and records them as its own
    `previous` — so a revert that turns out to be wrong can itself be undone by reverting
    again — then writes and verifies Proportional, Integral, and Derivative in order,
    stopping at the first failure. A revert never chains a nested rollback of itself
    (`rollback_state` stays `None` on every revert row); a partially-failed revert is
    reported and audited, matching a partially-failed original write-back's "roll back only
    what was confirmed" philosophy being a deliberately separate concern from "undo an old
    write-back on request".
  - **Format-aware reporting**, matching the "Option B" design used for the original
    write-back: a `Table`-mode status line before attempting the revert and a plain-text
    summary after; a `RevertJson`/`RevertedTargetJson` object (run id, response level, the
    target P/I/D values, success, and an error message) on `--output json`, with no prose
    printed ahead of it.

  **Testing approach.** Eleven dedicated tests. Six need no mock backend at all, since
  `revert`'s validation runs before it ever connects: no such run; the run used a
  non-`Opcda` backend; no `Write`-kind row recorded; the recorded write's `previous` is
  `None`; `--yes` not passed; the run's tags have no PID constant tags configured; and a
  genuine connection failure (an unreachable host, proving every check above passed). Two
  use the shared mock gRPC `Bridge` service from `crate::test_support`: a full success
  (asserting the resulting row's `kind = Revert`, all three written/readback values, and
  `rollback_state = None`), and a partial failure using the mock's existing
  `failing_read_from_call(n)` builder to fail exactly Integral's post-write verification
  readback (call 5 of 6: three pre-reads plus Proportional's own verification succeed
  first), proving Derivative is never attempted and the failure is still fully audited. A
  final test exercises the `--output json` success path directly (`bhtune-cli`'s own
  subprocess-level "stdout is exactly one JSON object" contract remains
  `safety-json-contract`'s responsibility, not re-proven per command here).

## Logging (`cli-logging`)

Structured `tracing`/`tracing-subscriber` logging, matching `opcda-bridge-gateway`'s own
stack and `log.*` config conventions (level/directory/format/rotation, resolved through the
same `CLI flag > env var > TOML config file > default` precedence as every other setting —
see "Config precedence" above), adapted for one hard constraint: it must never be able to
corrupt `--output json`'s single-object stdout contract (see "Automation" above).

- **`--log-level`** (env `RUST_LOG`) — an `EnvFilter` directive spec, e.g. `"debug"` or
  `"bhtune_cli=debug,sqlx=warn"`; defaults to `info`, and falls back to `info` on a spec that
  fails to parse rather than erroring — a config typo shouldn't stop a tune from running.
- **`--log-dir`** — defaults to a platform-standard data directory
  (`config::default_log_dir_from`, the same precedence machinery `cli-config`'s DB path
  already uses, not a directory next to the binary).
- **`--log-format`** — `pretty` (human-readable, ANSI-free — log files aren't a terminal) or
  `json` (newline-delimited, for log shippers). Defaults to `pretty`.
- **`--log-rotation`** — `hourly`, `daily`, or `never`. Defaults to `daily`.
- **`[log]` in `bhtune.toml`** — `level`/`dir`/`format`/`rotation` keys underneath config-file
  precedence, mirrored 1:1 with the flags above via `LogConfig` in `config.rs`.

**Deliberately never writes to stdout — the single load-bearing design decision.** Log lines
always go to the rotating file (`tracing_appender::rolling`, non-blocking); they *also*
mirror to **stderr**, and only when a console is actually attached (`std::io::stderr().
is_terminal()` — false for a `cron`/Task-Scheduler invocation), never to stdout.
`opcda-bridge-gateway`'s equivalent mirrors to stdout safely, because it owns stdout outright;
bhtune's CLI does not, since `--output json` documents stdout as a single machine-readable
object. Verified end-to-end, not just by inspection: `tests/ctrlc_abort.rs` asserts the real
spawned subprocess's stderr never contains the product-output string, and a manual run of the
compiled binary with `--log-level debug` against the simulator backend confirmed the log file
captured every instrumented line while stderr stayed silent (no attached console).

**Wired into `run()`, not `run_with_cli`.** `lib.rs::run()` loads the config, resolves
`default_log_dir`, calls `logging::resolve_log_settings`/`logging::init_tracing`, holds the
returned `WorkerGuard` for the rest of the process's life (dropping it early would silently
truncate buffered lines not yet flushed on exit), then delegates to `run_with_cli`. This
keeps logging setup fully decoupled from `run_with_cli`'s own large, injection-based test
suite (zero existing tests call `run()` directly) and means `cargo test` never touches a real
platform log directory. `init_tracing`'s result is soft-failed (`let _log_guard = ...`, no
`?`) — an unwritable log directory shouldn't prevent a user from getting their tune's actual
result, a deliberate deviation from the gateway's hard-error approach.

**Instrumentation added at meaningful points**, not exhaustively: database open and template
seed count (`db.rs`), backend construction for both the OPC DA and simulator branches
(`backend.rs`), and in `commands/tune.rs` — run start/finish, the `Err` path, both abort
branches (Ctrl+C and `--timeout-secs`), MRFT engine completion, a per-tick trace event, and
write-back outcomes (success/readback-failure/rejected) in `maybe_write_back`. Bare
`tracing::*!` calls are always safe to sprinkle through already-tested code with no dedicated
new tests, because `tracing`'s global-subscriber single-assignment semantics mean events are
silently dropped whenever no subscriber is installed (as in every other test in the suite) —
they only do anything once a real `init_tracing` call succeeds, which only happens in
`tests/ctrlc_abort.rs`'s one real subprocess.

**Testing approach.** `logging.rs`'s 18 unit tests cover `parse_log_format`/`parse_rotation`
(including the graceful-degradation defaults), `build_env_filter`, and `resolve_log_settings`'s
full CLI/config/default precedence directly; two tests exercise `init_tracing`/
`init_tracing_with_stderr` themselves (both stderr-attached and stderr-detached layer wiring)
using `level: Some("off")` rather than a real level — deliberately, since `tracing_subscriber`'s
global-subscriber install only succeeds once per shared test-binary process, so whichever test
wins that race stays installed (and its filter level applies) for every other, unrelated test
in the same `cargo test` invocation; an earlier `Some("debug")` version of these tests was
found leaking unrelated `sqlx` DEBUG query noise into other tests' output for exactly this
reason. `tests/ctrlc_abort.rs` is the one real, conflict-free, end-to-end exercise of
`init_tracing` in a fresh process — it passes its own `--log-dir` tempdir and asserts the
directory is non-empty after the run, alongside the stderr-never-contains-product-output
assertion above.

## Validation strategy: golden-master replay

The engine's confidence story is golden-master replay: recorded input/output traces (tick-by-tick
PV inputs and the engine's resulting hysteresis/MV/switch-counter/calculated-constant outputs) are
replayed through the Rust engine and compared exactly. `trace-fixtures` normalizes captured traces
into a stable, versioned format under `tests/golden/`; `core-replay-harness` feeds them through
the engine and asserts per-tick and final-result equality. This is the gate for confidence that a
change didn't silently alter tuning behavior.

Reference traces are captured two ways, neither of which requires Windows:

1. **Synthetic runs against the in-Rust FOPDT simulator** (`backend-simulator`, done — see
   "Simulator backend reference" above) across a coverage matrix of process types, controller
   types, action directions, and edge cases (non-zero MV range floor, varied skip/count cycles).
   `bhtune-backend`'s own test suite already includes one such run (a full `MrftEngine` driven
   through `SimulatorBackend` to completion); `core-replay-harness` will need more, spanning the
   full matrix, once it's built.
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
16. **A live PV/MV trend chart is a core UX expectation for the web GUI** — plan for high-rate
    streaming updates (multiple times per second) from the start; see "Chart library" below.

## Conventions

- **Trunk-based git flow**: single long-lived `main`, short-lived PR branches
  (`<type>/<short-description>`), squash merges, no `develop`/release branches. Releases are
  tagged directly off `main`.
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/).
- **Formatting/linting**: `cargo fmt --check --all` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **No unused dependencies**: `cargo machete` runs in CI. Placeholder crates
  (`bhtune-server` and any stub not yet consuming a path dependency)
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
- **`bhtune-server` has no `axum`, `utoipa`, or `rust-embed` dependency yet.** It remains a
  placeholder binary until `server-http-api` starts. Adding real deps prematurely risks breaking
  `cargo build --workspace` before that phase is ready to use them, for no benefit.
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
| `bhtune-backend` | `backend-trait`/`backend-opcda`/`backend-simulator`/`backend-replay`    | `backend-trait` + `backend-opcda` + `backend-simulator` done (trait, error model, OPC DA implementation, and FOPDT simulator, all tested); replay pending |
| `bhtune-db`      | `db-schema`/`db-seed-templates`/`history-query-api`/`db-backup-restore` | All done (7 tables, tested; 4 templates auto-seed on startup; run-history repository layer with lifecycle, filtering, and pagination; whole-database backup/restore via `VACUUM INTO`) |
| `bhtune-cli`     | `cli-commands`/`cli-config`/`cli-automation`/`cli-safety`/`cli-logging` | All five sub-phases done (subcommands, see "CLI reference" above; `CLI > env > TOML > default` config precedence, see "Config precedence" above; `--yes`/`--write-pid`/`--output json` and distinguished exit codes, see "Automation" above; relay-amp validation and mandatory `--timeout-secs`, see "Safety" above; `tracing` file+stderr logging, see "Logging" above) — a fully headless, scriptable CLI, no server required. Undergoing a live-plant safety hardening pass (Phase 6.5) after a post-`cli-logging` review; see "Live-plant safety hardening" below |
| `bhtune-server`  | `server-http-api`/`openapi-contract`/`server-embed-spa`/`server-windows-service` | Placeholder binary; primary v1 GUI adapter, no `axum` dependency yet         |

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
   bhtune-backend`), its OPC DA implementation (`backend-opcda`, done: `OpcDaBackend` in
   `crates/bhtune-backend/src/opcda.rs`, see "OPC DA integration reference" above), and its
   in-Rust FOPDT simulator (`backend-simulator`, done: `SimulatorBackend`/`FopdtProcess`/
   `VirtualPid` in `crates/bhtune-backend/src/simulator.rs`, see "Simulator backend reference"
   above); the replay implementation remains.
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
   `db-schema` designed plain SQLite storage in from the start. Platform-standard data
   directories for the database file are wired up in `bhtune-cli`'s `cli-config`
   (`resolve_db_path`/`default_db_path_from`, see "Config precedence" above), not a
   `bhtune-db` concern.
6. **Headless CLI** — `tune`/`simulate`/`template`/`history`/`export`/`opc` subcommands
   (`cli-commands`, done — see "CLI reference" above), `CLI > env > TOML > default` config
   precedence (`cli-config`, done — see "Config precedence" above), non-interactive automation
   mode (`cli-automation`, done: `--yes`/`--write-pid`/`--output json` and distinguished exit
   codes — see "Automation" above), safety guardrails (`cli-safety`, done: relay-amp range
   validation and mandatory `--timeout-secs` with auto-abort-and-restore — see "Safety"
   above), and structured logging (`cli-logging`, done: `tracing`/`tracing-subscriber`
   to a rotating file plus stderr-only console mirroring — see "Logging" above). All five
   sub-phases are done — `bhtune-cli` is a complete, fully headless, scriptable adapter on its
   own, with no server required. Now undergoing a live-plant safety hardening pass (Phase
   6.5) — see "Live-plant safety hardening" above.
7. **Web GUI (`bhtune-server` + React SPA)** — `bhtune-server` promoted from stub to an Axum
   server exposing the tuning engine over an OpenAPI-described HTTP API (`server-http-api`,
   `openapi-contract`), embedding the built SPA into the binary (`server-embed-spa`); React + TS
   + Vite + Tailwind frontend using TanStack Query against the generated client
   (`frontend-shell`); Connection/Tag-mapping/Test-parameters/Results/History/Template-editor/
   Simulator screens plus a live PV/MV trend chart (`frontend-screens`); live per-tick streaming
   to the UI over SSE (`frontend-live-stream`); running as a proper platform service
   (`server-windows-service`). Replaces the earlier Tauri desktop GUI phase — see "Key
   architectural decisions" above for the reversal.
8. **End-to-end testing and CI** — fully automated E2E tune on Linux CI via CLI + simulator
   backend (no Windows, no external DCS dependency); Playwright E2E against the real web UI
   (`e2e-playwright`); golden replay suite in CI; release build matrix for Linux/macOS/Windows
   (`build-matrix`, via `cargo-dist`, embedding the built SPA — no Tauri bundler or WebView
   runtime to manage).
9. **Documentation and release** — README/usage docs and a getting-started guide, published
   roadmap (OPC UA/Modbus backends, free remote/multi-user access, Step Test pending the bridge
   `Subscribe` RPC, multi-loop/batch tuning), v0.1.0 with per-platform binaries, a Windows MSI
   installer (`pkg-windows-installer`, the primary distribution artifact), and a secondary Docker
   image (`pkg-docker`).
10. **History explorer** (low priority, post-v1) — a filterable/sortable run list and PV/MV
    trend view over already-recorded history (`history-explorer-ui`), age-based retention
    disabled by default (`history-retention`), and headless parity via `bhtune history
    list`/`show`/`prune` (`history-cli`). A reader of data earlier phases already write, so
    deliberately scheduled after v1.
11. **Remote and multi-user access** (post-v1, free like everything else) — local accounts with
    session cookies and revocable API tokens (`server-remote-auth`), TLS (`server-tls`), an
    audit log of who ran/wrote what (`server-audit-log`), and OIDC for SSO-managed orgs
    (`server-oidc`). Deferred, not blocking v1's `127.0.0.1`-by-default posture — see "Key
    architectural decisions" above.
12. **Cross-project CI/CD audit** (`cross-project-ci-audit`, not urgent/blocking) — compare
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
  while still stroking a real control valve. `cli-safety` (done — see "Safety" above) ships
  real relay-amp range validation and a mandatory wall-clock timeout with automatic
  abort-and-restore; none of it is optional polish. A further live-plant safety hardening pass
  (Phase 6.5, in progress — see "Live-plant safety hardening" above) is closing nine more
  findings from a follow-up review, including making Ctrl+C/timeout cancellation reach an
  in-flight backend call, guaranteeing a restore on every exit path, and enforcing OPC quality.
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
