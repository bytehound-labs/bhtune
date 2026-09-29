# Architecture decisions

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Design philosophy and scope discipline

Most PID auto-tuning tools for industrial DCS/PLC systems are Windows-only desktop applications
built on proprietary toolkits and OPC SDKs, limiting portability and auditability. bhtune is
designed from the ground up to avoid all of that:

- **100% open-source dependencies, machine-enforced in CI** (`cargo deny`, see `deny.toml`) — not an
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
  fails on a new dependency, find an open-source alternative; don't widen the allow-list
  reflexively.
- **Zero Windows/COM dependency in this application.** All OPC DA communication is delegated to
  the sibling project [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge) over the
  network. bhtune itself builds and runs on Linux, macOS, and Windows identically.
- **The OPC DA client is a crates.io dependency, local to `bhtune-driver` only.** The
  `OpcDaDriver` implementation consumes the published `opcda-bridge` library with
  `opcda-bridge = "0.5"` pinned directly in `crates/bhtune-driver/Cargo.toml` (the
  0.5.0 release) — not promoted
  to `[workspace.dependencies]`, since `bhtune-driver` is the only crate that talks to the
  bridge directly (everything else goes through the `Driver` trait), matching this project's
  single-consumer-stays-local dependency convention. It must not use a Git dependency or a
  local path checkout. The Windows-side `opcda-bridge-gateway` remains a separate process.
- **`Driver` trait is the extensibility seam, and deliberately has zero `bhtune-core`
  dependency.** A single async trait in `bhtune-driver` abstracts all tag I/O so the tuning
  engine never knows what it's talking to:

  ```rust
  #[async_trait]
  pub trait Driver: Send + Sync {
      async fn read(&self, tags: &[TagId]) -> DriverResult<Vec<TagValue>>;
      async fn write(&self, tag: &TagId, value: TagWrite) -> DriverResult<WriteOutcome>;
      async fn capabilities(&self) -> DriverResult<DriverCapabilities>;
      async fn browse(&self, request: BrowsePageRequest) -> DriverResult<BrowsePage>;
      async fn close_browse_session(&self, session_id: &str) -> DriverResult<()>;
      async fn search(&self, request: SearchRequest) -> DriverResult<Vec<SearchEvent>>;
  }
  ```

  `TagId` is a plain `String` alias (no invariant worth a newtype). `TagValue.value` is a raw
  string, not a parsed `f32` — not every tag is numeric (mode/direction/attribute tags hold
  raw codes like `"MAN"`/`"0"` that `bhtune_core::ControllerDirection::from_raw_tag_value`
  interprets directly), so parsing is the caller's job, not this trait's. `TagWrite` is
  `Float(f32) | Raw(String)` — bhtune only ever writes numeric process values or a raw mode
  code (reverting Auto/Manual after a test). `write` returns `Ok(WriteOutcome { success,
error_message })` even when the driver _rejects_ the write (read-only tag, out of range) —
  that's a normal outcome of the call reaching the driver, not a `DriverError`; the shape
  matches `bhtune_db::models::TuneWriteRow`'s columns exactly so a caller can copy it straight
  into an audit row with no translation. `DriverError` splits `Connect` (nothing was
  attempted) from `Operation` (reached the driver, failed there) from `Unsupported` (this
  driver has no such capability, e.g. `browse` on the simulator/replay drivers) so callers
  like the `cli-safety` guardrails can react differently to each. The trait never
  references a `bhtune-core` type: reading/writing named string tags has no domain meaning by
  itself — gluing `Driver` to `LoopTags`/`ControllerDirection`/etc. is each concrete
  driver's own job (`driver-opcda`, `driver-simulator`), not this trait's.

  `OpcDaDriver` (via `opcda-bridge`) is the primary/only driver for v1, now implemented (see
  `driver-opcda` below). `OpcUaDriver` and `ModbusDriver` are roadmap items that must slot in
  without touching `bhtune-core`. Connecting/constructing a specific driver is deliberately
  _not_ part of the trait — each implementation's own inherent constructor takes whatever it
  individually needs (gateway host/port + OPC DA server name, a trace file path, simulator
  parameters), since one uniform `connect()` signature across such different drivers would
  leak one implementation's parameters into the trait every other implementation would have to
  ignore.

- **AGPL-3.0-or-later + CLA.** BHTune is distributed under the AGPL. The CLA (see `CLA.md`,
  version 1.0, in force) records the rights needed to accept and maintain contributions, naming
  ByteHound Corp. as the entity. Its copyright grant is deliberately broad enough to sublicense
  contributions under terms other than the AGPL, which is what keeps a paid enterprise offering
  possible; `CLA.md` states that to contributors plainly rather than burying it in legal prose.
  Enforcement runs on every pull request through `.github/workflows/cla.yml` (`cla-tooling`). The
  text has not had a formal legal review — worth arranging before a large corporate contributor
  signs.
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
  a plain NSIS installer (`pkg-windows-installer`) is the primary distribution artifact precisely
  because Docker is frequently banned or unavailable on OT networks; the Docker image
  (`pkg-docker`, done — see "`pkg-docker`: the Docker image" below) is a secondary channel for
  IT-managed Linux hosts, not the deployment path this decision was optimized for.
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
  opt-in is chosen. Authentication, TLS, and audit logging are planned post-v1 remote-access
  features (`server-remote-auth`, `server-tls`, `server-audit-log`, `server-oidc`) rather than
  blocking v1. This is a judgement call worth
  re-examining before that host is ever reachable off a trusted OT network: the precedent that
  makes it defensible in the meantime is that `opcda-bridge-gateway` is _already_ an
  unauthenticated network service in this exact topology, and it is strictly more dangerous than
  an unauthenticated bhtune (it can read/write any tag, whereas bhtune only ever writes the PID
  constants of one user-selected loop).
- **Full-mode browser mutations use automatic same-host CSRF validation by default.** When neither
  `BHTUNE_ORIGIN` nor the TOML `origin` key is set, a state-changing request with an `Origin`
  header is accepted only when its HTTP(S) origin authority matches the request `Host`
  case-insensitively, including effective ports; requests without `Origin` remain accepted for
  CLI/curl compatibility, and explicit `cross-site` Fetch Metadata is rejected. An explicit
  origin remains a strict pin for reverse proxies that rewrite `Host` or deployments that need
  one public origin. This is not authentication, and Full mode remains unsafe for untrusted
  networks. Demo mode intentionally keeps exact configured-origin validation.
- **Step Test is deferred**, not part of v1 (MRFT only). Step Test is an alternative, simpler
  manual tuning method that observes PV changes via an OPC DA _subscription_ rather than polling
  reads, and the bridge's protocol has no such push/subscription RPC yet — `ListServers`/`Read`/
  `Write` are unary and `Browse` is a bounded, one-shot server-streaming call (the facade drains it
  into a single `Vec` before returning). MRFT itself only needs unary polling reads, so this
  doesn't block v1 — Step Test is blocked on adding a live push/subscription RPC to
  `opcda-bridge`, distinct from `Browse`'s existing bounded stream.
- **`f32` in the tuning engine, not `f64`.** Industrial analog tags (PV/MV/tuning constants) are
  commonly single-precision (`REAL4`/`VT_R4`) over OPC DA, and don't need more precision than
  `f32` provides. Using a fixed, narrower width consistently — rather than mixing `f32` OPC values
  into `f64` math — avoids conversion noise and keeps golden-master replay comparisons exact.
- **Simulator MRFT timestamps use the same fixed step as the FOPDT process.** Each simulator PV
  read advances `FopdtProcess` by exactly the configured `[tuning].poll_interval_ms`;
  `bhtune-cli` advances the
  corresponding `Tick.time` by that same exact duration, with the first sample one step after the
  logical run start. Host scheduling still controls how quickly the CLI subprocess gets CPU time
  and the mandatory `[tuning].timeout_secs` remains real elapsed time, but scheduler jitter cannot make
  the synthetic process evolve in one time domain while tuning math measures it in another.
  Persisted simulator samples use this logical time too, so their trend and calculated period
  describe the same simulated process.
- **Live OPC DA MRFT timestamps use monotonic elapsed time projected onto UTC.** `prepare()`
  captures one `RunTimeAnchor` (`Utc::now()` paired with `tokio::time::Instant::now()`) at the
  same point `tune_runs.started_at` is recorded. `run_polling_loop` timestamps each successful PV
  observation after the driver read as `utc_anchor + monotonic_elapsed`, so a slow read is
  represented in the sample that actually experienced it and subsequent engine switch timestamps
  reuse that same value. NTP or a manual calendar-clock correction after the anchor is captured
  has no input path into MRFT timing. Real scheduling, OPC read/write, and SQLite processing
  delays remain part of the measured timeline; `MissedTickBehavior::Delay` remains deliberate so
  a late tick never causes catch-up I/O bursts. This makes the algorithm clock-jump-safe, not
  hard-real-time: an overloaded host can still undersample a live oscillation.
- **Polling timing diagnostics are persisted facts, not a PID-validity policy.**
  `PollTimingAccumulator` observes only timestamps for successful PV poll samples (not startup
  readings or presentation-only trend boundaries), records adjacent gap count/mean/maximum, and
  counts a missed polling opportunity whenever a gap is at least `2 * requested_interval`.
  Completed runs additionally reuse `measure_oscillation()` for the measured period and divide it
  by the mean observed gap for an approximate samples-per-period value. The typed
  `TimingMetrics` snapshot is stored as nullable, `json_valid`-checked
  `tune_runs.timing_metrics_json`; old rows and attempts with no successful poll remain `NULL`.
  The database write is deferred until after the safety-critical restore attempt, so a locked
  SQLite file can never leave the live loop waiting at its relay-test value merely to save
  diagnostic metadata. Normal completion/abort uses one atomic repository update for the
  terminal outcome and timing snapshot, so the SSE `done` event can never make the frontend stop
  polling on a terminal row before its timing metrics are durable for API and CLI consumers.
  Completed-only period fields are included only in that same `completed` update; a failure while
  saving calculated results records cadence metrics without a period. Standalone failure-path
  persistence is best-effort so diagnostic metadata can never replace a tune's real
  completion/abort/error outcome. Only live runs with a nonzero missed-opportunity count log a
  warning; the web run-detail UI does not display it, and no run abort, result-validity label, or
  PID write restriction exists until field evidence supports a defensible threshold.

## Other notes

- **MRFT and Step Test (once implemented) share one concurrency model.** Both use one async
  polling/streaming model in `bhtune-core` — polling for MRFT, subscription-based streaming for
  Step Test — rather than inventing a different mechanism for each.
- **Safety is a first-class requirement, not polish.** Scheduled/scripted tuning against a live,
  running process removes human supervision (no operator watching the trend, able to hit Stop)
  while still stroking a real control valve. `cli-safety` (done — see "Safety" above) ships
  real relay-amp range validation and a mandatory wall-clock timeout with automatic
  abort-and-restore; none of it is optional polish. A follow-up live-plant safety hardening pass
  (Phase 6.5, done — see "Live-plant safety hardening" above) closed nine more findings from a
  further review, including making Ctrl+C/timeout cancellation reach an in-flight driver call,
  guaranteeing a restore on every exit path, and enforcing OPC quality.
- **Chart library**: `uPlot` over `Recharts` for the frontend trend chart — handles high-rate
  streaming data (multiple updates/second) far better.
- **Naming**: `bytehound` is an established Rust memory-profiler brand. `bhtune` avoids a direct
  crates.io collision, but be aware of the overlap with the ByteHound company brand in the Rust
  ecosystem when publishing.

## Open questions

- Whether the DCS/PLC templates should remain user-editable JSON/TOML exports in addition to
  SQLite rows, so site-specific tag maps can be shared between installations.
