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
`bhtune-db`), `template` (`list`/`show`/`import`/`export`/`delete` — see `template-cli`
below for the multi-template TOML catalog import and TOML export), `history` (`list`/
`show`), `export`
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
The Phase 6.5 live-plant safety hardening pass is done (see "Live-plant safety hardening"
below). Phase 6.6's `template-catalog` and `template-provenance` are also done: the four
built-in DCS templates moved from hardcoded Rust constructors to an embedded, contributable
TOML catalog (`crates/bhtune-core/templates/builtin.toml`) with a `DcsTemplate::validate()`
and new `versions`/`description`/`source` fields, and `dcs_templates` gained a real
three-way `origin` column (`builtin`/`catalog`/`user`, replacing a plain `is_builtin`
boolean) plus `versions_json`/`description`/`source` columns to store them. `template-
user-catalog` is also done: `bhtune-cli` auto-loads a user-supplied catalog file on every
startup (`--templates`/`BHTUNE_TEMPLATES`/a `templates` config key, resolved through the
same precedence chain as every other setting), seeding it with `TemplateOrigin::Catalog`.
`template-cli` is also done: `template import` auto-detects a single JSON template versus a
multi-template TOML catalog, `template export --format toml` emits a PR-ready
`[[template]]` block, and `template delete <name>` removes a template (with a friendly
error if a saved loop still references it) — see "Community DCS/PLC template catalog"
below. `template-docs` is also done: the README documents the `template` subcommand and
invites template contributions, `CONTRIBUTING.md` has a "Contributing a DCS/PLC template"
section, and `docs/dcs-templates.md` documents every field with a worked example — this
closes out Phase 6.6.
Phase 7's `server-http-api` is done: `bhtune-server` is a real Axum binary exposing
`/api/health`, `/api/templates` (list/get/create/delete), and `/api/runs` (filtered/paginated
list, full run detail), sharing the CLI's config precedence, database bootstrap, and tracing
setup, with graceful shutdown on Ctrl+C/`SIGTERM` — see "Key architectural decisions" above.
`openapi-contract` is also done: every route/DTO derives `utoipa::ToSchema`/`IntoParams`
(behind an optional `utoipa` Cargo feature on `bhtune-core`/`bhtune-db`, so those two crates
stay free of it unless a consumer asks — see "Key architectural decisions" below), aggregated
by a single `ApiDoc` (`crates/bhtune-server/src/openapi.rs`) that serves the raw OpenAPI 3.1
document at `GET /api/openapi.json` and an interactive Scalar UI at `/api/docs`. The document
is also checked in at the repo root (`openapi.json`) and regenerated-and-diffed in CI
(`cargo run -p bhtune-server --example gen_openapi` then `git diff --exit-code`), so it can
never silently drift from the routes that actually produce it — the first use of this
regenerate-and-diff pattern in the repo, and the template `docs-generated-cli` will reuse
later for the CLI reference/man pages/completions. Every fallible route's error responses
carry a real `body = ErrorBody` schema (`{"error": "<message>"}`) rather than an undocumented
`description`-only entry, so the generated TS client types error bodies instead of `content?:
never`. `frontend-shell` is also done: a pnpm
workspace (`pnpm-workspace.yaml`) holding `frontend/` (`bhtune-frontend`) — a React + TS +
Vite + Tailwind CSS v4 SPA using TanStack Query against a typed `openapi-fetch` client
(`frontend/src/api/client.ts`) generated by `openapi-typescript` straight from the
checked-in `openapi.json` (`frontend/src/api/schema.d.ts`, regenerated-and-diffed in CI
exactly like the Rust-side spec itself). A new `scripts/check-frontend-licenses.mjs` gate
mirrors `cargo-deny`'s license allow-list for the npm dependency tree, so "no proprietary
dependencies" stays machine-enforced on both sides of the stack — see "Key architectural
decisions" below for both. `frontend-screens` is under way: a `react-router` (declarative
mode) routing shell with an `AppLayout` (nav + the relocated health badge), a Templates
screen (list with delete, read-only detail, a create form covering all 27 `DcsTemplate`
fields — no edit, since there's no update endpoint), and a History screen (filterable/
paginated run list, full run detail with config/initial-readings/results/write-back-audit
tables, deliberately no trend chart yet — that's `history-explorer-ui`). Still blocked:
Connection, Tag mapping, Test parameters, the live PV/MV trend chart, Results with
Write-PID, and Simulator screens all need a way to actually start a tune over HTTP, which
doesn't exist yet (`bhtune-server`'s API is read-only plus template CRUD-minus-update) —
tracked as the new `server-start-tune-api` todo. `server-start-tune-api` is now done:
`POST /api/runs` and `POST /api/runs/{id}/cancel` let a tune be started and cancelled over
HTTP, reusing `bhtune-cli`'s own `prepare()`/`drive()` orchestration under the hood rather
than duplicating it — see "`server-start-tune-api`: starting and cancelling a tune over
HTTP" below for the full design, including the `Send`-trait fix this required in
`bhtune-cli` itself before a tune could be spawned as a background task at all. That
unblocked `frontend-screens`'s second slice, also now done: a combined New Run screen
(Connection, Tag mapping, Test parameters, Simulator parameters, and Write-back-on-
completion in one form, since it all feeds one `POST /api/runs` body), run cancellation,
and a polling-based live-progress banner on the run detail screen (the deliberate interim
substitute for the not-yet-built SSE stream) — manually verified against a real running
server, which caught and fixed two real bugs (a `NumberField` `step`/`min` misalignment,
and the simulator backend actually requiring five fields instead of the one originally
assumed) that typechecking alone would have missed. `server-template-update-api` is now
done: `PUT /api/templates/{name}` edits an existing `origin = "user"` template in place —
400 if the body's `name` doesn't match the path (renames aren't supported; delete and
recreate instead), 404 if no template exists at that name, 409 if the existing row isn't
`user`-owned (a `Builtin`/`Catalog` row would just be discarded by the next startup
reseed) — see `crates/bhtune-server/src/routes/templates.rs`'s `update_template` doc
comment for the full contract. That unblocked `frontend-screens`'s third slice, also now
done: a Template Edit screen (`/templates/:name/edit`), reusing the Create page's entire
27-field form via a new shared `TemplateFormFields` component (in
`frontend/src/routes/templates/TemplateFormFields.tsx`, with the constants/types/
conversion functions factored into a sibling `templateFormState.ts` so the component file
has no non-component exports — keeps `oxlint`'s `react/only-export-components` rule
genuinely clean rather than merely below CI's non-blocking `warn` threshold) with a
`nameEditable` prop that locks the Name field and explains why on the Edit page. The
Template Detail page's Edit button only renders for `origin === "user"` templates; a
direct edit-URL visit to a builtin/catalog template instead renders a warning banner and
disables Save, matching the server's 409 rather than letting the user hit it blind.
Manually verified end-to-end (create a user template, edit and save it, confirm the
change persists; visit a builtin template's edit URL directly and confirm the disabled
warning state) against a real running server, zero console errors. This closed out
`frontend-screens`'s last remaining gap.
`frontend-live-stream` is now done: `GET /api/runs/{id}/stream` (Server-Sent Events) polls
`TuneSampleRow::list_for_run_since` (a new `bhtune-db` query — `tick > after_tick`, with
`-1` as the "everything" sentinel) and `TuneRunRow::get` every 300ms inside an
`async-stream::stream!` generator, replaying every sample from tick 0 on every connection
and terminating with exactly one `done` event (`RunStreamDone { outcome }`) once the run
leaves `Running` — see `crates/bhtune-server/src/routes/stream.rs`'s module doc for why
this polls the same database every other reader uses rather than adding a broadcast
channel (zero risk to the already-tested CLI/server tick loop). On the frontend, a new
`useRunStream` hook (`frontend/src/api/runs.ts`) consumes it via a plain `EventSource`
(untyped — SSE has no typed-client support, unlike the rest of the API — with events
manually parsed against the generated `SampleResponse`/`TuneOutcome` schema types),
closing the connection and invalidating `useRun`'s query cache the instant `done` arrives.
A new reusable `TrendChart` component (`frontend/src/components/TrendChart.tsx`) renders
the resulting `samples` array with `uPlot` (PV on the left scale, MV on a right `mv`
scale), using a `useRef`-held instance and `setData` for incremental updates rather than
recreating the plot on every sample — the same component will later serve
`history-explorer-ui`, differing only in whether `samples` comes from the live stream or
a finished run's REST payload. `RunDetailPage` now switches between the two sources based
on `outcome`, and `useRun`'s own polling was relaxed from a 1s to a 5s fallback now that
the SSE stream (with its `done`-triggered invalidation) is the primary live-update
mechanism, not a once-a-second full-`samples`-refetch. Manually verified against a real
running server: the chart streamed the live relay square-wave/PV-oscillation in real
time, the SSE connection opened exactly once and closed cleanly on `done` (confirmed via
the browser's network log — no reconnect storm), and the chart handed off to the
historical `samples` array with an identical rendered trend once the run completed, zero
console errors.
`server-embed-spa` is now done: the built SPA (`frontend/dist/`) is embedded directly into
the `bhtune-server` binary via `rust-embed`, so a release build is a single self-contained
executable — no separate static file server, Node, or nginx needed on the target host — see
"`server-embed-spa`: embedding the built SPA into the binary" below for the full design.
`server-windows-service` (the last item in Phase 7) is deliberately deferred, not merely
unstarted: writing `#[cfg(windows)]` code against the `windows-service` crate in this
environment would be unverifiable — `cargo check --target x86_64-pc-windows-gnu` fails
workspace-wide because `libsqlite3-sys` needs an `x86_64-w64-mingw32-gcc` cross-compiler
that isn't installed and can't be (`sudo apt-get install mingw-w64` fails with no
passwordless sudo available). Revisit once a Windows machine or that toolchain is
available, rather than shipping untested Windows-only code on faith. Phase 8's
`e2e-simulator` is now done: a genuine, real-subprocess end-to-end test
(`crates/bhtune-cli/tests/e2e_simulator.rs`) that runs `bhtune tune` against the simulator
backend across a small process/controller-type matrix and asserts the _calculated_ PID
results, not just row presence — a gap no earlier test closed (see "Correctness-critical
design details" below, item 2, for the real `bhtune-core` bug this test caught and fixed
in the process: the MRFT oscillation period silently lost sub-second precision by default,
zeroing `ti_minutes`/`td_minutes` even for PI/PID). `e2e-playwright` is also done: a
Playwright suite (`frontend/e2e/`) drives a full tune through the real, built React SPA
served by a real `bhtune-server` binary (debug profile, which serves `frontend/dist/` live
off disk rather than needing a re-embed step — see `server-embed-spa`'s `rust-embed`
feature gating) running the in-process simulator backend, with no mocked HTTP layer and no
Vite dev server involved. `smoke.spec.ts` covers the app shell, the health badge reaching a
real backend, the seeded built-in template list, and header nav; `tune.spec.ts` drives
`/runs/new` with the same millisecond-scale simulator parameters `e2e_simulator.rs` uses and
asserts the _rendered_ Kp/Ti/Td values are sane and correctly ordered (not just that the
page didn't crash), plus a second test cancelling an in-flight run. This surfaced a genuine,
benign transient race in `bhtune-server` itself, worth documenting rather than masking:
`ActiveRun::release` only runs once a run's background task returns from `drive()` — one
`await` _after_ the same `drive()` call already persisted the outcome a client observes as
"completed" over SSE/REST — so a client that submits a new run the instant it sees the
previous one finish can occasionally still be told a run is already active; `tune.spec.ts`'s
`startTune` helper retries through this rather than papering over it with a fixed sleep. A
third TypeScript project (`tsconfig.e2e.json`, referenced from `tsconfig.json` alongside the
existing `tsconfig.app.json`/`tsconfig.node.json`) wires `e2e/`/`playwright.config.ts` into
the existing `tsc -b`/`pnpm run build` gate, so the suite's own source is genuinely
typechecked in CI, not merely executed. A new `.github/workflows/e2e.yml` job builds the
frontend, builds a debug `bhtune-server`, installs Chromium via `playwright install
--with-deps`, and runs the suite, uploading the Playwright HTML report as a CI artifact on
failure. `backend-replay` and the golden-trace replay harness are not yet — the GUI plan
reversed from a Tauri desktop app to a browser UI served by `bhtune-server` before any
Tauri code was written (see "Key architectural decisions"). Phase 9's two front-loaded,
run-now items are also done: `docs-contract` (see
"Documentation contract" above) and `docs-copilot-hook` — a paired `sessionStart`/`sessionEnd`
Copilot CLI hook (`.github/hooks/docs-drift.json`) that warns when a session changed
`crates/**` without touching any documentation surface, covering both a session's already-
committed-and-pushed changes and anything still uncommitted (see `.github/hooks/README.md`
for why it's a pair, not a single hook). See "Phases and todos" below for what's next.

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
error_message })` even when the backend _rejects_ the write (read-only tag, out of range) —
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
  _not_ part of the trait — each implementation's own inherent constructor takes whatever it
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
- **`server-http-api` is done: `bhtune-server` is a real Axum binary, not a placeholder.**
  `bhtune_server::build_router` merges `GET /api/health`, `GET`/`POST /api/templates`,
  `GET`/`DELETE /api/templates/{name}`, and `GET /api/runs` (filtered, paginated list)/
  `GET /api/runs/{id}` (full run detail: config, initial readings, samples, results, writes)
  into one `axum::Router<AppState>`, directly testable via `tower::ServiceExt::oneshot` with
  no bound socket. `main.rs` is a thin bootstrap shell calling straight into
  `bhtune_cli::{config, db, logging}` — the exact same config-precedence, database-open/
  migrate/seed, and tracing setup the CLI uses, so the two adapters can never silently
  disagree about where the database lives or how logging is configured (see the
  `bhtune-server` → `bhtune-cli` dependency note above `[dependencies]` in
  `crates/bhtune-server/Cargo.toml` for why this is a deliberate, named, temporary coupling
  rather than the intended peer relationship). Every JSON-facing DTO in `routes/*.rs` is its
  own hand-written projection of the corresponding `bhtune-db` row type (never a `Serialize`
  impl on the row type itself), mirroring `bhtune-cli`'s own `--output json` shapes
  field-for-field so the CLI and the HTTP API describe the same run the same way. Shuts down
  gracefully on Ctrl+C and, on Unix, `SIGTERM` (`axum::serve(...).with_graceful_shutdown(...)`),
  draining in-flight requests rather than dropping connections — proven by real subprocess
  integration tests (`tests/graceful_shutdown.rs`) that spawn the compiled binary, do a real
  HTTP request over a raw `TcpStream`, send a real OS signal, and assert a clean exit. The
  startup log/print line reports `TcpListener::local_addr()` (the OS-assigned address), not
  the originally-requested bind string — identical for every real deployment (a concrete port
  is always configured) but the only way a test can bind an ephemeral port (`BHTUNE_BIND=
127.0.0.1:0`) and still discover which port the OS actually chose from stdout, without
  hardcoding a port that might collide with something else already listening.
- **Cargo preserves hyphens literally in `CARGO_BIN_EXE_<name>` when a `[[bin]]` name equals
  the package name and contains a hyphen.** For `bhtune-server` (package name and `[[bin]]`
  name both `"bhtune-server"`), the correct lookup in a test is
  `env!("CARGO_BIN_EXE_bhtune-server")` — **not** the underscored
  `CARGO_BIN_EXE_bhtune_server`, which fails to compile ("environment variable not defined at
  compile time") even in a clean build. This is easy to get wrong by analogy with
  `bhtune-cli`'s own tests, which use `CARGO_BIN_EXE_bhtune` without incident only because its
  `[[bin]]` is named `bhtune` (no hyphen) while the package is `bhtune-cli` — a different name,
  so there's nothing to substitute. The underscored form only exists as a _proposed_, not yet
  implemented, Cargo enhancement (upstream issue #16438); don't trust a search result that
  describes it as already shipped. Any future same-named, hyphenated `[[bin]]` in this
  workspace will hit the same thing.
- **One API surface, described by OpenAPI, with no client-side transport abstraction.**
  `openapi-contract` is done on the Rust side: every DTO in `crates/bhtune-server/src/routes/
*.rs` derives `utoipa::ToSchema` (query structs derive `utoipa::IntoParams` instead), every
  handler carries a `#[utoipa::path(...)]` annotation, and `crates/bhtune-server/src/
openapi.rs`'s `ApiDoc` (`#[derive(utoipa::OpenApi)]`) aggregates all of it into one OpenAPI
  3.1 document — deliberately one explicit list of `paths(...)`/`components(schemas(...))`
  rather than a macro that scans `routes/**` for annotations automatically, so a route added
  without updating `ApiDoc` is a visible, reviewable omission rather than something that
  silently works but never appears in the spec. The document is served two ways: the raw JSON
  at `GET /api/openapi.json` (`axum::Json(ApiDoc::openapi())`, since `utoipa::openapi::OpenApi`
  is plain `Serialize`) and an interactive Scalar UI at `/api/docs`
  (`utoipa_scalar::Scalar::with_url` returns a state-generic `axum::Router<S>` with the UI
  route already attached, so it merges straight into `build_router` with no handwritten
  handler). It is also checked in at the repo root (`openapi.json`) and regenerated-and-diffed
  in CI (`cargo run -p bhtune-server --example gen_openapi` then `git diff --exit-code
openapi.json`) — the first use of this pattern in the repo, and the template
  `docs-generated-cli` will reuse later for the CLI reference/man pages/completions. There is
  exactly one transport — `fetch` over HTTP — so no `ApiClient`-style interface with swappable
  backends is warranted; adding one would be pure ceremony with a single implementation.
  Generating the TypeScript client itself (`openapi-typescript`) landed with `frontend-shell`
  once a `frontend/` package/`pnpm-workspace.yaml` existed for the generated client to live
  in — `openapi-contract`'s own scope stayed the Rust-side contract (annotations, aggregation,
  the two serving routes, the checked-in spec, the CI diff gate); see the next two bullets for
  the frontend-side generation and its own drift/license gates.
- **`frontend-shell` is done: a pnpm workspace, one generated client, no transport
  abstraction.** `pnpm-workspace.yaml` at the repo root declares `frontend` (package name
  `bhtune-frontend`, matching the `bhtune-*` crate-naming convention) as its sole member so
  far. `frontend/src/api/client.ts` is the one and only HTTP transport for the whole UI: an
  `openapi-fetch` client (`createClient<paths>({ baseUrl: '' })` — empty, not `/api`, because
  the generated `paths` keys already include the `/api` prefix) typed against `frontend/src/
api/schema.d.ts`, generated by `openapi-typescript` from the repo-root `openapi.json` via
  `pnpm run generate:api`. That script's own `prettier --write` step is load-bearing, not
  cosmetic: without it, `openapi-typescript`'s raw output wouldn't match what `prettier
--check` (and a human) expect of a committed file, so CI's regenerate-and-diff gate
  (`pnpm --filter bhtune-frontend run generate:api` then `git diff --exit-code -- frontend/
src/api/schema.d.ts`, mirroring the Rust `gen_openapi` pattern exactly) would show
  permanent, spurious drift. TanStack Query (`@tanstack/react-query` + the Devtools) is the
  only data-fetching/caching layer in use; the original health-check-badge placeholder in
  `frontend/src/App.tsx` proved the whole pipeline works end-to-end against a real running
  `bhtune-server` and has since been superseded by the real routes landing in
  `frontend-screens` (see the next bullet). Tailwind CSS v4's simplified setup needs no
  `tailwind.config.js` or PostCSS config — just the `@tailwindcss/vite` plugin and
  `@import 'tailwindcss';` in `index.css`. The current Vite React+TS template ships `oxlint`
  (a Rust-based linter) rather than ESLint by default; kept as scaffolded rather than
  replaced with the more common ESLint+typescript-eslint stack, since the scaffolding tool's
  own current choice is a stronger compatibility signal than a preference would be. CI
  installs pnpm via `pnpm/setup@v2` (not the older `pnpm/action-setup`, which only supports
  pnpm ≤10), which installs both pnpm 11 and a Node LTS runtime in one step.
- **`frontend-screens`'s first slice is done: declarative routing, Templates, and History
  (List/Detail only).** `frontend/src/App.tsx` is now a `react-router` (v8, declarative mode
  — `<BrowserRouter>`/`<Routes>`/`<Route>`/`<Outlet>`, no data-mode loaders, since TanStack
  Query already owns data fetching) route table with `frontend/src/layout/AppLayout.tsx` as
  the single layout route (header, nav, the health badge relocated from the old placeholder
  `App.tsx`) and a `/` → `/templates` redirect. `frontend/src/components/ui.tsx` is the one
  shared Tailwind vocabulary every screen builds from (`PageHeading`/`Button`/`Card`/`Badge`/
  `ErrorBanner`/`EmptyState`/`LoadingState` plus `Section`/`Field` for read-only displays and
  `FormSection`/`TextField`/`SelectField`/`CheckboxField` for forms), so screens stay
  consistent without a component library dependency. The Templates screens
  (`routes/templates/`) are List (table + delete via `window.confirm`), Detail (read-only,
  grouped into Identity/Behavior/Tag suffixes/Mode values), and Create (all 27 `DcsTemplate`
  fields via plain controlled `useState` — no form library, since the project has no existing
  form-library precedent and the field count doesn't yet justify adding one; `versions` is
  edited as one comma-separated text field, split/trimmed/filtered to `string[]` on submit,
  since it's typically 0-3 short tokens like `"R5, R6"`). There is deliberately no Template
  edit screen, since `bhtune-server` has no update endpoint yet. The History screens
  (`routes/history/`) are List (filterable by process type/outcome/backend, paginated via
  `limit`/`offset`) and Detail (config/initial-readings/calculated-results/write-back-audit
  tables) — deliberately no trend chart yet, which is `history-explorer-ui`'s job in Phase
  10, not this slice's. `frontend/src/api/runs.ts` (`useRuns`/`useRun`) mirrors the existing
  `templates.ts` hook shape exactly. Everything here was verified against a real running
  `bhtune-server`, not just typechecked: curl against every template CRUD status code
  (200/201/204/400/404/409) and a real `bhtune-cli simulate` run rendered through
  `chromium --headless=new --dump-dom` (an already-present system binary, not a new project
  dependency — the permanent Playwright harness is the separate, not-yet-started
  `e2e-playwright` phase) confirmed real data renders on all six routes. The remaining
  `frontend-screens` scope — Connection, Tag mapping, Test parameters, the live PV/MV trend
  chart, Results with Write-PID, and Simulator — all need a way to start a tune over HTTP,
  which does not exist yet; tracked as the new `server-start-tune-api` todo rather than
  guessed at here.
- **`frontend-screens`'s second slice — the New Run screen, run cancellation, and live
  progress polling — is done, as one combined form rather than five separate screens.**
  `routes/runs/NewRunPage.tsx` covers Connection, Tag mapping, Test parameters, Simulator
  parameters, and Write-back-on-completion in a single page, since all of it feeds one
  `POST /api/runs` body anyway; the plan's own stated principle for this phase is
  "equivalent capability plus real validation", not matching the legacy widget-for-widget
  layout. Every default (`sim_gain`, `poll_interval_ms`, `timeout_secs`, etc.) matches
  `StartRunRequest`'s server-side `#[serde(default = ...)]` values or `bhtune-cli`'s clap
  defaults exactly, and `buildRequest()` mirrors the server's own `into_tune_args()`
  pre-flight checks client-side for fast feedback (the server still re-validates
  everything regardless). `frontend/src/api/runs.ts` gained `useStartRun`/`useCancelRun`
  mutations; `useRun` gained `refetchInterval: 1000ms while outcome === "running"` as the
  deliberate interim substitute for the not-yet-built SSE stream
  (`frontend-live-stream`) — `RunDetailPage` now shows a live-progress banner (latest
  tick/PV/MV/cycles) and a "Cancel run" button while a run is active, both explicitly
  labeled in the UI as polling, not push, so nobody mistakes it for the eventual real-time
  chart. `components/ui.tsx` gained a `NumberField` (mirrors `TextField` but
  `type="number"`, `value`/`onChange` typed `number | ""` for "left blank") and extended
  `SelectField` with `required`/`hint` props matching `TextField`'s existing pattern, so an
  enum field can show the same red-asterisk-plus-explanation treatment a numeric field
  already could.

  **Manually verified against a real running server, not just typechecked** — the same
  standard the first `frontend-screens` slice was held to. A scratch `bhtune-server` +
  Vite dev server were driven through a real browser (chrome-devtools automation): filled
  and submitted the New Run form with fast-converging simulator parameters, watched a run
  complete with control-theory-consistent PID results across all three response levels,
  started a second run and clicked "Cancel run" mid-flight (confirmed `outcome` flips to
  `aborted` with `restore_status: confirmed`), and confirmed the History list reflects
  both runs correctly — zero browser console errors, every network request 2xx.

  This caught two real bugs no amount of typechecking would have, both fixed before
  landing:
  1. **`step`/`min` misalignment on two `NumberField`s.** `pollIntervalMs` used
     `min={1} step={50}` and `timeoutSecs` used `min={1} step={60}`; HTML5's step
     validation anchors at `min`, so the only valid values are `1, 51, 101, ...` and
     `1, 61, 121, ...` respectively — silently excluding the exact defaults being
     displayed (`800`, `3600`), which browsers flag as invalid (`:invalid`,
     `validationMessage` non-empty) even though the values are perfectly correct. Fixed
     by using `step={1}` for both; a numeric field's default should never itself be
     off-step.
  2. **The simulator backend actually requires five fields, not one.**
     `bhtune-cli`'s `build_loop_tags` (`commands/tune.rs`) hard-requires
     `pv_range_high`, `pv_range_low`, `mv_range_high`, `mv_range_low`, **and**
     `direction` whenever `backend: "simulator"` — the frontend had only validated and
     defaulted `pv_range_high`, so a first-time visitor's default simulator run 400'd
     immediately on submit (only caught by actually clicking "Start tune" in a browser,
     not by reading the DTO's field list). Fixed by defaulting all five to exactly
     `bhtune simulate`'s own CLI-convenience values (`100`/`0`/`100`/`0`/`"reverse"`,
     read from `SimulateArgs::into_tune_args` in `bhtune-cli/src/args.rs`), extending
     `buildRequest()`'s validation to cover all five with the server's exact wording, and
     adding a `setBackend` handler that back-fills these onto switching to the simulator
     backend without ever overwriting a value the user already set.

- **`frontend-screens`'s third slice — the Template Edit screen — is done, unblocked by
  `server-template-update-api`.** `routes/templates/TemplateEditPage.tsx` loads the
  template via `useTemplate(name)`, pre-populates local form state from it, and submits via
  a new `useUpdateTemplate()` mutation (`PUT /api/templates/{name}`) — mirroring
  `useCreateTemplate`'s shape exactly, down to invalidating both the list and single-
  template query keys on success. Building it required extracting the Create page's entire
  27-field form (previously duplicated inline) into two shared files: `templateFormState.ts`
  (constants, the `TemplateFormState` type, `blankTemplateForm`, and the two pure
  conversion functions — `templateToFormState()`/`templateFormStateToTemplate()`, inverses
  of each other) and `TemplateFormFields.tsx` (the `TemplateFormFields` component itself,
  with a `nameEditable` prop — `true` on Create, `false` on Edit, since `PUT`'s contract
  requires the body's `name` to match the path, so the Name field is disabled with an
  inline hint rather than silently allowed and then 400ing on submit). Splitting into two
  files (rather than one combined `TemplateForm.tsx`, tried first) exists specifically to
  keep `oxlint`'s `react/only-export-components` rule (configured `"warn"`, not `"error"` —
  confirmed CI's bare `oxlint` invocation wouldn't have failed either way) genuinely clean:
  a file mixing component and non-component exports breaks Vite's Fast Refresh for that
  component, which is a real development-experience cost even though it's not a CI-blocking
  one. `TemplateDetailPage.tsx` shows an "Edit" button only when `origin === "user"`; a
  direct visit to another origin's edit URL still renders (there is no route guard) but
  shows an explicit warning banner and disables Save, matching what the server would 409 on
  rather than letting a user discover that by submitting. Manually verified against a real
  running server: created a user template, edited and saved a field change, confirmed it
  persisted (including the `updated_at` timestamp changing); visited a builtin template's
  edit URL directly and confirmed the disabled-warning state; deleted the test template
  afterward to confirm delete still works alongside the new edit path. Zero browser console
  errors, every network request 2xx (aside from one pre-existing, unrelated cosmetic
  404-on-unmount when deleting from the detail page — `useDeleteTemplate` only invalidates
  the list query key, but TanStack Query's prefix-matching invalidation also refetches the
  still-mounted detail query in the instant before `navigate()` unmounts it; predates this
  slice, from the original `frontend-screens` commit, and harmless — noted here as a minor
  future cleanup opportunity, not fixed as part of this slice).
- **`frontend-live-stream` is done: SSE supersedes the interim polling banner with a real
  live-updating trend chart.** Backend: `TuneSampleRow::list_for_run_since(pool, run_id,
after_tick)` (`bhtune-db`) is a new query — `tick > after_tick`, with `-1` as the
  documented "everything" sentinel since `tick >= 0` always — polled by a new
  `GET /api/runs/{id}/stream` handler (`crates/bhtune-server/src/routes/stream.rs`) inside
  an `async-stream::stream!` generator on a 300ms interval, emitting a `sample` SSE event
  per new tick (same `SampleResponse` DTO `history.rs` already used) and exactly one final
  `done` event (`RunStreamDone { outcome }`) once the run leaves `Running`. Polls the
  database rather than adding a broadcast channel deliberately — zero risk to the
  already-tested CLI/server tick loop, and the endpoint replays every sample from tick 0
  on every connection, so it behaves identically whether a client connects mid-run or
  reconnects after a drop. Returning `Result<impl IntoResponse, ApiError>` (never naming
  `Sse<impl Stream<...>>` or boxing/pinning a `dyn Stream`) is what let this ship without
  adding `futures-core` as a direct dependency. Frontend: a new `useRunStream(id, enabled)`
  hook (`frontend/src/api/runs.ts`) opens a plain `EventSource` (untyped — SSE has no
  `openapi-fetch` support, unlike the rest of the API — parsing each event's `data` against
  the generated `SampleResponse`/`TuneOutcome` schema types by hand), accumulates `sample`
  events into its own `samples` array, and on `done` closes the connection _and_
  invalidates `useRun`'s query cache so the final `results`/`writes`/`restore_status`
  appear immediately rather than waiting on a poll. A new reusable `TrendChart` component
  (`frontend/src/components/TrendChart.tsx`) takes a plain `samples` prop and renders it
  with `uPlot` — a `useRef`-held instance created once (PV on the left scale, MV on a right
  `mv` scale, a `ResizeObserver` for responsive width) and fed via `setData` on every new
  `samples` array, uPlot's own incremental-update path, rather than recreating the plot per
  sample; the same component will serve `history-explorer-ui` later unchanged, differing
  only in whether its `samples` prop comes from the live stream or a finished run's REST
  payload. `RunDetailPage` now renders `TrendChart` fed by `useRunStream` while
  `outcome === "running"` and by `run.data.samples` once terminal — exactly one source is
  ever "live" at a time, so no de-duplication logic is needed. `useRun`'s own
  `refetchInterval` was relaxed from 1s to 5s now that it's just a safety net (in case an
  SSE connection is silently dropped by an intermediary) rather than the primary live-
  update mechanism. Manually verified against a real running `bhtune-server` + Vite dev
  server via browser automation: the chart streamed the live relay square-wave (MV) and
  resulting PV oscillation in real time as ticks arrived; the network log showed exactly
  one `GET /api/runs/{id}/stream` request for the whole run (no reconnect storm — proving
  `source.close()` in the `done` handler pre-empts the browser's default SSE auto-retry);
  and the chart handed off to the historical `samples` array with an identical rendered
  trend the moment the run reached `completed`, alongside populated results/write-back
  tables. Zero browser console errors throughout.
- **`server-embed-spa` is done: the built SPA is embedded in the binary itself, not served
  separately.** `crates/bhtune-server/src/spa.rs` defines an `Assets` struct
  (`#[derive(RustEmbed)]`, `#[folder = "$CARGO_MANIFEST_DIR/../../frontend/dist/"]`,
  `#[allow_missing = true]`) and a `static_handler(uri) -> Response`, wired in as
  `build_router`'s single whole-router `.fallback(...)` (after every other merged route, so
  `/api/*` always wins first — axum panics if two merged sub-routers each declare their own
  fallback, which is why no other route module sets one). `#[allow_missing = true]` is what
  lets `frontend/dist/` — gitignored, and never present in CI's Rust-only `check` job — be a
  clean runtime condition (empty `Assets::iter()`) instead of the crate's default hard
  compile-time error for a missing `#[folder]`. The `interpolate-folder-path` feature
  substitutes `$CARGO_MANIFEST_DIR` with an absolute path at compile time, so resolution is
  CWD-independent even in a debug build reading live from disk (rust-embed's documented
  debug-mode default is "relative to wherever the binary is run from", which would be
  fragile for a service manager starting the binary from an arbitrary directory) — confirmed
  empirically by running a debug binary from an unrelated working directory and seeing it
  still find its assets. The `mime-guess` feature gives `EmbeddedFile.metadata.mimetype()`
  directly, avoiding a redundant direct `mime_guess` dependency; `deterministic-timestamps`
  zeroes embedded files' timestamps for reproducible release builds. `static_handler` serves
  a matched path with its real MIME type and one of two cache rules — `Cache-Control:
no-cache` for `index.html` (it names the current build's content-hashed asset filenames,
  so it must always be revalidated) and `public, max-age=31536000, immutable` for every
  other embedded path (a Vite content hash means a new build always emits a new filename) —
  falls back to `index.html` for any path whose last `/`-segment has no `.` (a client-side
  route under React Router's `BrowserRouter`, which uses real HTML5 history paths, not hash
  routing, so a server-side fallback is genuinely required for direct navigation/hard-refresh
  to work), returns a real `404` for a missing dotted-extension path, and returns a `503`
  with an actionable message (`run pnpm install && pnpm run build`, or `pnpm run dev` for
  frontend development) when the SPA was never built at all. Manually verified end-to-end
  against both a debug and a `--release` binary, run from a directory unrelated to the
  crate: `/` served `index.html` with `no-cache`; a real hashed asset served with the
  long-lived immutable cache header and the correct content type; `/runs/1` (a client-side
  route) fell back to byte-identical `index.html` content; a genuinely missing asset path
  404'd; `/api/health` still resolved correctly (proving the fallback never shadows a real
  API route); and the 503 path was confirmed by temporarily moving `frontend/dist/` aside
  and back. `frontend/vite.config.ts`'s dev-mode API proxy is untouched by this — it's a
  `pnpm run dev` concern, orthogonal to how a release binary serves its own built assets.
- **Every fallible route response is now typed with a real error schema, not
  `content?: never`.** `utoipa::path`'s `responses(...)` entries for 4xx statuses previously
  gave only a `description`, so `openapi-typescript` generated `content?: never` for them —
  technically valid, since utoipa had documented no body, but wrong: `bhtune-server`'s
  `IntoResponse` for `ApiError` always writes a real `{"error": "<message>"}` JSON body at
  runtime (see `crates/bhtune-server/src/error.rs`). Fixed by making `ErrorBody` (the existing
  runtime type) `pub` and `#[derive(utoipa::ToSchema)]`, registering it in `ApiDoc`'s
  `components(schemas(...))` (utoipa does not collect schemas transitively from response
  annotations alone — every type has to be listed explicitly, matching the existing
  convention for schemas that only ever appear nested inside another struct), and adding
  `body = ErrorBody` to every error-status entry across `routes/templates.rs` and
  `routes/history.rs`. `frontend/src/api/errors.ts`'s `apiErrorMessage(error: unknown):
string` is the one shared helper every hook (`templates.ts`, `runs.ts`) uses to narrow an
  `openapi-fetch` error down to a displayable string now that the shape is real, replacing an
  earlier ad hoc `typeof error === "string"` check.
- **The repo root pins an explicit `.prettierrc.json`, matching Prettier's own defaults
  verbatim (`singleQuote: false`, `trailingComma: "all"`, etc.).** Prettier resolves config by
  searching every ancestor directory of the file being formatted, not just up to the repo
  root — so without a repo-owned config file, a contributor with a stray `.prettierrc*` (or
  `.editorconfig`) anywhere above their clone on disk silently gets different output than CI,
  which has no such ancestor files. That's exactly what happened once during `frontend-shell`
  development: a personal, outside-the-repo `~/.prettierrc.yaml` with `singleQuote: true` was
  picked up locally the whole time, so `frontend/src/api/schema.d.ts` was committed
  single-quoted while CI's clean checkout regenerated it double-quoted, failing the drift
  gate. Pinning the config explicitly (rather than just fixing the one file) makes formatting
  fully deterministic for every contributor regardless of what else lives on their machine —
  the config's values were deliberately chosen to be a no-op against true Prettier defaults,
  so this is a determinism fix, not a style change. `.prettierignore` excludes `pnpm-lock.yaml`
  (pnpm, not Prettier, owns that format) and `openapi.json` (generated by the Rust
  `gen_openapi` example with its own `git diff --exit-code` gate; utoipa's JSON serializer has
  different, equally-valid formatting conventions that Prettier would otherwise fight).
- **`scripts/check-frontend-licenses.mjs` is the npm-side counterpart to `cargo deny
check`.** A dependency-free Node script that parses `pnpm licenses list --json`'s output
  against an allow-list mirroring `deny.toml`'s Rust license allow-list (plus `Python-2.0`, a
  legitimate transitive dependency's OSI-approved PSF license), including SPDX `OR` compound
  expressions (satisfied if any single arm is on the allow-list, matching `cargo-deny`'s own
  treatment of such expressions). Wired into the root `package.json`'s `check:licenses`
  script and the `frontend` CI job, so "no proprietary dependencies" is machine-enforced for
  the npm dependency tree exactly as it already is for the Cargo one, not a second, weaker
  promise.
- **`utoipa` is an optional, feature-gated dependency on `bhtune-core`/`bhtune-db`, not a
  hard one.** Neither crate can implement `utoipa::ToSchema` for the other's types from
  `bhtune-server` directly (Rust's orphan rule: neither the trait nor the type would be local
  to `bhtune-server`), so instead `bhtune-core`/`bhtune-db` each gained
  `utoipa = { workspace = true, optional = true, ... }` plus `[features] utoipa =
["dep:utoipa"]`, and derive `#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]`
  directly on every type an HTTP-facing DTO embeds (enums like `ProcessType`/
  `ControllerType`/`TemplateOrigin`, and structs like `LoopConfig`/`DcsTemplate`/`Tick`/
  `MrftState`). `bhtune-server` enables the feature (`features = ["utoipa"]`) on both path
  dependencies; `bhtune-cli` never requests it, so a `cargo build -p bhtune-cli` in isolation
  never even fetches `utoipa` into its dependency graph — the derive costs nothing for a
  consumer that doesn't ask for it, exactly the same shape this workspace already uses for
  optional `serde`-adjacent derives elsewhere. (Cargo's feature unification means a
  `cargo build --workspace` _does_ compile `bhtune-core`/`bhtune-db` with the feature on
  everywhere once anything in the graph requests it — normal, well-understood Cargo behavior
  with no runtime effect, since the derive is compile-time-only and doesn't reopen
  `core-mrft`'s "no clock reads" guarantee, which is enforced by chrono's `clock` feature
  staying off workspace-wide, not by `utoipa` being absent.) `bhtune-core`'s existing crate-doc
  purity rule ("no I/O, no async, no clock reads") already covered why `toml` doesn't violate
  it; the same reasoning extends to `utoipa`, since deriving a schema at compile time is
  neither I/O nor an async/clock operation.
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
  makes it defensible in the meantime is that `opcda-bridge-gateway` is _already_ an
  unauthenticated network service in this exact topology, and it is strictly more dangerous than
  an unauthenticated bhtune (it can read/write any tag, whereas bhtune only ever writes the PID
  constants of one user-selected loop).
- **Nothing is paywalled, now or on the current roadmap.** The CLA exists solely to keep
  relicensing _possible_ in the future without taking anything from AGPL users today — it is not
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
  template, overwrites existing `origin = 'builtin'` rows to match the current shipped
  definition (so a suffix/unit fix in a later release reaches existing installs
  automatically), and never touches a row whose name collides with a built-in's but whose
  `origin` differs — a user's own template (or one seeded from a different catalog) is never
  silently overwritten just because it shares a name with a preset. See `template-provenance`
  below for the three-way `origin` this generalizes from a plain `is_builtin` boolean.
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
  `bhtune-core`, so both the _pre-write_ readback (`TuneWriteRow.previous`) and the
  _post-write_ confirmation readbacks reuse `WriteReadback { proportional, integral,
derivative }`. `previous` is all-or-nothing (`Option<WriteReadback>`, not three
  independently nullable fields) because `safety-writeback-rollback`'s pre-read step is a
  hard stop — either all three pre-reads succeed before anything is written, or nothing is
  written and there is no partial "previous" to record. The three `*_written`/`*_readback`
  columns, by contrast, _are_ independently nullable, since the write-and-verify loop is
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
  destructive DB operations" rule, `restore_from` copies any existing live file to a
  timestamped `<file>.pre-restore-<UTC timestamp>.bak` sibling _before_ overwriting it, and
  reports that path back via `RestoreOutcome::pre_restore_backup` (`None` only when there was no
  live file to protect, i.e. a fresh install) — via `VACUUM INTO`, not a raw `fs::copy`, and
  gated by an exclusive-access check that refuses to proceed while another connection still
  holds the live file open (`safety-db-restore`; see "Live-plant safety hardening" below for
  the exclusivity-check design and its accepted TOCTOU limitation). The actual file replacement is
  copy-to-a-same-directory-temp-file-then-`rename`, so a crash or a full disk mid-copy can never
  leave `db_path` half-overwritten (rename onto an existing path is atomic on the same
  filesystem). Stale `-wal`/`-shm` sidecars at the old live path are then explicitly removed —
  proven necessary by testing that a graceful `Pool::close()` on a database's last connection
  already deletes sidecars _it_ created, so the removal loop only matters for genuinely orphaned
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
- **`bhtune template list|show|import|export|delete`** — inspect and manage `dcs_templates`
  rows (built-in, catalog, and user-imported) via `DcsTemplateRow`. `import` accepts either a
  single JSON template or a multi-template TOML catalog (auto-detected by content, not file
  extension) — a single-template import hard-fails on a name collision, while a catalog import
  skips colliding names and reports a summary, since the expected workflow there is
  re-importing an updated shared catalog. `export --format json|toml` (default `json`) emits
  either one template as JSON or a PR-ready `[[template]]` TOML block. `delete <name>` removes
  a template, with a friendly error if a saved loop still references it (`DbError::
TemplateInUse`) and a note that a `Builtin`/`Catalog`-origin template will simply reappear on
  the next startup unless also removed from its source. See "Multi-template import, TOML
  export, and `template delete`" below for the full design.
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
the risk of raising a real process signal _inside_ `cargo test`'s own shared, multi-threaded
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

| Setting               | CLI flag        | Env var              | Config key    | Default                                                                                                                                                                                             |
| --------------------- | --------------- | -------------------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Database path         | `--db`          | `BHTUNE_DB`          | `db`          | Linux/macOS: `$XDG_DATA_HOME/bhtune/bhtune.db` (falls back to `$HOME/.local/share/bhtune/bhtune.db`); Windows: `%APPDATA%\bhtune\bhtune.db`                                                         |
| opcda-bridge gateway  | `--bridge-host` | `BHTUNE_BRIDGE_HOST` | `bridge_host` | `localhost:7600`                                                                                                                                                                                    |
| Default OPC DA server | `--server`      | —                    | `server`      | none — must be set one way or another for `tune --backend opcda` and the `opc` subcommands                                                                                                          |
| User template catalog | `--templates`   | `BHTUNE_TEMPLATES`   | `templates`   | Linux/macOS: `$XDG_CONFIG_HOME/bhtune/templates.toml` (falls back to `$HOME/.config/bhtune/templates.toml`); Windows: `%APPDATA%\bhtune\templates.toml` — missing is not an error at this tier only |

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
  `TuneRunRow::complete` runs _before_ the optional write-back attempt, so a write-back
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
  (see `safety-cancellation` below) — but that outer race only covers the _idle_ wait between
  ticks. The timeout (and Ctrl+C) also stay effective _during_ a tick — including a stalled
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

### Live-plant safety hardening (done)

A post-`cli-logging` review of the live-tuning path (`commands/tune.rs`) surfaced nine
findings before the CLI's first real trial against live plant equipment: Ctrl+C/timeout
cancellation not reaching an in-flight backend call, no guaranteed restore on every exit
path, missing input validation (e.g. `--cycles-count 0` panicked mid-run), OPC quality never
checked, PID write-back with no pre-read/rollback, `bhtune-db`'s `restore_from` unsafe under
an active WAL and wrong on Windows, `--output json` emitting prose ahead of the JSON object,
and no template/tag snapshot on a recorded run. All nine are closed, each landed as its own
commit with its own test coverage; the per-finding writeups below are the permanent record
of what changed and why, kept rather than trimmed once "done" since they're the design
rationale for code that still exists (not a changelog of the review itself):

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
  `tuning_math::measure_oscillation`'s internal `assert!` and panicked _after_ the loop had
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
    clear message before any I/O. Deliberately _not_ applied to `mrft_delay`, `cycles_skip`,
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
  - The in-flight MRFT poll loop (`run_polling_loop`) — a poor-quality PV sample here _does_
    abort the run (a new `AbortReason::PoorQuality { tag, quality }`, restored and recorded
    exactly like a Ctrl+C/timeout abort), but the triggering sample is still recorded to
    `tune_samples` (with its real, poor quality) _before_ the abort, via a new
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
  signal delivery per kind, and a `Signal` future created _after_ delivery never observes
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
    _second_ signal is a second, distinguishable resolution on the same handle, which is
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
    `tokio::select!` (covering the _idle_ wait between ticks) reuses the exact same `&mut
CtrlC` handle passed down into the tick body's `bounded_backend_call`s, which is safe
    specifically because a tokio `watch::Receiver`'s "seen this value" state advances the
    moment either `select!` observes it — there is no way for the outer and an inner
    `select!` to each separately consume the same signal.
  - `attempt_restore`/`RestoreAttempt` — wraps `restore()` in the same race, against a new
    `--restore-timeout-secs` (default 30s, independent of `--op-timeout-secs`/
    `--timeout-secs`, since a restore triggered _by_ a timeout would otherwise inherit an
    already-expired budget) and `ctrl_c.signalled()` again — a _second_ Ctrl+C during the
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
  failure _after_ a genuinely completed test — and `restore()` itself returned on its first
  failure, so a single rejected MV write pre-empted even _attempting_ to put the mode back.
  Closed in three parts, matching the design's "A + C + D" decision:
  - **`MutationGuard`** (Option A) — a plain struct of four booleans
    (`mode_attribute_written`/`mode_written`/`mv_written`/tracks whether a setpoint was
    captured), armed the instant each corresponding write actually succeeds, never
    optimistically before. `execute()`'s mutating body was split into an inner function
    returning `Result<_, (anyhow::Error, MutationGuard)>` — the guard travels _with_ the
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
    guard flag _and_ a value-based precondition (e.g. the mode-attribute step only fires if
    the read-back program value actually differs from what's already there), so a step whose
    guard flag was never armed correctly reports `NotNeeded`, distinct from an armed-but-
    failed `Failed`. `RestoreReport::failure_summary()` names every failed step by label
    (`"MV: ...; mode: ...; setpoint: ...; mode attribute: ..."`) rather than collapsing to
    "something failed", so an operator reading `bhtune history show` knows exactly what to
    check by hand.
  - **Durable restore intent** (Option D, partially done) — `TuneRunRow::record_initial_readings`
    now persists `mode_raw`/`mode_attribute_raw`/`setpoint_ini` (the loop's pre-mutation
    mode/mode-attribute/setpoint, mirroring the existing `pv_ini`/`mv_ini`/range columns)
    _before_ `transition_to_manual`'s first write, not after — so a process that dies
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
    (`rollback_state = Succeeded`); and wrote some, failed, and the rollback _itself_ failed
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
  tag's _pre-read_ in good standing while still forcing its _post-write_ readback or a later
  _rollback_ write to fail deterministically) and `distorting_write` (silently perturbs a
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
    a revert pre-reads the loop's _current_ live values first and records them as its own
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

- **`bhtune-db`'s `restore_from` is now safe under an active WAL and requires exclusive
  access before restoring** — done (`bhtune_db::backup::exclusive_pre_restore_snapshot`,
  `EXCLUSIVITY_PROBE_TIMEOUT`, `DbError::DatabaseInUse`). This finding predates any CLI
  command actually calling `restore_from`/`backup_to` (both remain library-only APIs, per
  `db-backup-restore`) — genuinely proactive hardening rather than a fix to a shipping
  path, but sequenced here rather than deferred since Phase 6.6's template catalog work
  edits the same pre-release migration findings 6 and 9 already touch. Previously the
  pre-restore safety copy used a raw `std::fs::copy`, and SQLite only auto-checkpoints a
  WAL when the _actual last connection to the file across the whole system_ closes — not
  merely the last connection in the caller's own pool — so a copy taken while a second
  process (`bhtune-server` running alongside the CLI, the exact topology this project's own
  architecture anticipates) still held the database open could silently miss committed data
  sitting only in the WAL. Separately investigated and found to be a non-issue:
  `restore_from`'s existing copy-to-temp-then-`rename` file replacement was already correct
  on Windows — `std::fs::rename` overwriting an existing destination _file_ (as opposed to a
  directory) has always worked there via `MOVEFILE_REPLACE_EXISTING`, so no
  Windows-specific fallback was needed for that part of the original finding. Closed as the
  design's "A + C":
  - **`VACUUM INTO` replaces the raw copy** (Option A) — the pre-restore safety copy is now
    taken the same way `backup_to` already takes its own snapshots: a consistent,
    WAL-content-inclusive copy that can never be silently missing committed data.
  - **An exclusivity probe gates the snapshot** (Option C) —
    `exclusive_pre_restore_snapshot` opens a dedicated, single-connection probe pool
    straight to `db_path` (deliberately not via `connect()`, which runs migrations that must
    never touch a database about to be discarded) and runs `PRAGMA wal_checkpoint(TRUNCATE)`;
    a nonzero `busy` column is SQLite's own native proof that some other connection — in this
    process or any other — is still attached, with no lock-file or advisory-lock scheme
    needed to get that answer. `busy != 0` fails the whole restore with
    `DbError::DatabaseInUse(db_path)` before the snapshot or the live file are touched at
    all. A dedicated `EXCLUSIVITY_PROBE_TIMEOUT` (200ms) backs this check rather than
    reusing `pool::connect`'s general-purpose 10-second `BUSY_TIMEOUT`: the two timeouts
    answer different questions (connect's, "let contended work finish"; this one's, "is
    anyone here right now") and reusing the longer one was measured to make every
    blocked-restore path take a real ~10 seconds, since `wal_checkpoint(TRUNCATE)`
    internally retries for the full busy-timeout duration before ever reporting `busy = 1`.
  - **The residual race is accepted and documented, not hidden.** The exclusivity probe is a
    point-in-time check, not a held lock — a different process could still open `db_path` in
    the instant between the probe succeeding and the later file replacement.
    `restore_from`'s doc comment calls this out explicitly as "the honest fix for the
    multi-process case," matching the design's own framing of Option C versus the fuller,
    explicitly deferred Option D (a logical/online restore that needs no exclusivity at
    all).

  **Testing approach.** Two new tests prove the exclusivity check itself: one opens a second
  real connection to the live database, begins a transaction, and executes an actual
  `SELECT` (establishing a genuine WAL read snapshot — a bare `BEGIN` alone wouldn't hold the
  file open the same way), then asserts `restore_from` returns `DbError::DatabaseInUse` with
  the live database left completely untouched; the other confirms a retry succeeds once that
  blocking connection is dropped and closed. A third test targets the one line the new
  exclusivity step's own side effect made harder to reach: because
  `exclusive_pre_restore_snapshot`'s own open-checkpoint-close sequence against an
  _existing_ `db_path` already tidies up any stale `-wal`/`-shm` sidecars itself, the
  pre-existing orphaned-sidecar test no longer exercises the later post-rename cleanup
  loop's own `remove_file` call (caught by `cargo llvm-cov`'s line-level report, not by a
  failing test — its own assertions, checking only final restored data, still passed). The
  new test constructs the one scenario that _can_ only be cleaned up by that loop: `db_path`
  itself never existing (so the exclusivity/snapshot step is skipped entirely, per its own
  existence gate) while stale sidecar files exist anyway at the paths it would use.

- **`--output json` now emits exactly one parseable JSON value on stdout on every `tune`
  path** — done (`commands::tune::maybe_write_back`, `RunOutcome::Completed`'s new
  `write_back_detail` field). Previously `maybe_write_back` `println!`ed its interactive
  listing/prompt and every status/result line unconditionally, regardless of `--output` —
  confirmed by hand: a completed simulator run (which never has PID constant tags
  configured, see `build_tags`'s `BackendKindArg::Simulator` arm) printed "No PID constant
  tags configured for this run's backend/template; skipping write-back." on stdout _before_
  the run's final JSON object, so `serde_json::from_str`/`json.loads` on stdout failed for
  every scripted/scheduled caller using `--output json` — the exact audience that flag
  exists to serve. Closed as the design's Option B ("format-aware reporting"), chosen over
  Option A (prose to stderr only) because a scripted caller ends up with strictly more
  information than today — the reason a write-back was skipped or failed is now a real,
  parseable field — rather than merely relocating prose out of the way; Option C (buffer
  the whole run and render once at the end) was rejected as a far larger refactor for a
  finding scoped to one function, with the interactive prompt still needing to print before
  reading stdin regardless.
  - **`maybe_write_back` gained an `output: OutputFormat` parameter and now returns
    `(WriteBackOutcome, Option<String>)`** instead of bare `WriteBackOutcome` — the second
    element is a human-readable detail string explaining _why_ the outcome is what it is,
    populated on every `Skipped`/`Failed` return path (no PID constant tags configured; no
    calculated results recorded; the named `--write-pid` response level has no result;
    pre-read failure; a rejected write, with or without a successful/failed rollback) and
    left `None` only for `Written` (self-explanatory) and the Table-mode "everything
    succeeded" cases exercised elsewhere. `RunOutcome::Completed`'s new `write_back_detail`
    field carries this through to `print_summary`, which folds it into the JSON object as
    `"write_back_detail"` — `Table` mode ignores it entirely (every match arm gained a
    trailing `..` to accommodate the new field without caring about its value), since the
    equivalent information is already in the `println!`ed prose there.
  - **Every remaining `println!` in `maybe_write_back` is now gated on `output ==
OutputFormat::Table`**, so `Json` mode prints nothing at all from this function — the
    caller's single final JSON object is the only thing that reaches stdout.
  - **The interactive listing/menu/prompt moved to `eprintln!` unconditionally**, in both
    output formats. A prompt has no business on stdout in _any_ format: a caller piping
    stdout elsewhere (exactly the scripted use `--output json` exists for, but just as true
    of `--output table | tee run.log`) should never see "Write which response level..."
    interleaved with the actual result.
  - **`--output json` without `--write-pid` now skips the interactive prompt outright**
    rather than attempting to read a response level from stdin — added as a new early-return
    arm (`None if output == OutputFormat::Json`) checked _before_ `reader` is touched at all,
    returning `WriteBackOutcome::Skipped` with a detail string naming the reason. There is no
    human present to answer an interactive prompt in a scripted/scheduled JSON run, and the
    prior behavior (read a line from real `stdin`, block indefinitely if none arrives) is
    exactly the kind of silent hang this project's automation posture (`cli-automation`) is
    designed to avoid. Combining `--output json` with `--write-pid <level>` still writes
    non-interactively exactly as before — this new arm only fires when no level was named.
  - **`--dry-run`'s removal (finding 1) and this finding compose cleanly**: with `--dry-run`
    gone, `--write-pid` already requires `--yes` unconditionally, so the JSON-mode
    early-exit above is reached only when a caller deliberately chose `--output json`
    without also naming a `--write-pid` level — an unusual but valid combination (e.g.
    "run the test and report the calculated constants, but never write") that now degrades
    to a clean, documented skip instead of a stdin hang.

  **Testing approach.** Six of the thirteen existing `maybe_write_back` unit tests were
  extended (rather than duplicated) with an assertion on the returned detail string, proving
  the plumbing end-to-end for each distinct skip/failure shape: no PID constant tags
  configured, no results recorded, the pre-read-failure case (asserts the detail _starts
  with_ `"pre-read failed:"`, since the underlying transport error's own message is
  interpolated), a rolled-back failure (asserts the detail _ends with_ `"(rolled back)"`), a
  failed rollback (asserts it mentions both `"rollback also failed"` and `"history
revert"`), and a named `--write-pid` level with no recorded result. One new unit test
  (`maybe_write_back_skips_the_interactive_prompt_without_touching_stdin_when_json_output_is_set_without_write_pid`)
  uses a named `Cursor` (rather than an inline temporary) specifically so `reader.position()`
  can be asserted as `0` _after_ the call — direct proof that the JSON-mode early-exit never
  reads a single byte from stdin, not just that it returns the right value. None of this,
  however, can prove the actual stdout _contract_ — `print_summary` calls `println!` directly
  and returns only a label enum, so a unit test has no way to observe the rendered JSON
  string. That gap is closed by a new subprocess-level integration test,
  `crates/bhtune-cli/tests/json_output_contract.rs`, modeled on `ctrlc_abort.rs`'s pattern of
  spawning the real compiled `bhtune` binary (`env!("CARGO_BIN_EXE_bhtune")`) rather than
  calling anything in-process: `tune_output_json_emits_exactly_one_parseable_json_value_on_stdout`
  runs a fast-completing simulator tune with `--output json`, asserts a clean exit code, and
  — the load-bearing assertion — runs `serde_json::from_str` on the _entire, trimmed_ stdout
  and asserts it succeeds, catching both "prose printed before the object" (this finding's
  original bug) and "prose printed after it"; it further asserts `write_back_detail` is a
  string containing "no PID constant tags configured" and that the old suppressed prose
  string never appears anywhere in stdout. A second test,
  `tune_output_table_is_plain_text_not_json`, is a sanity check that the default `Table`
  format for the identical run is _not_ parseable as JSON, proving the format flag actually
  branches rather than the two tests coincidentally passing the same way.

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
always go to the rotating file (`tracing_appender::rolling`, non-blocking); they _also_
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

## Community DCS/PLC template catalog (`template-catalog`)

Turns the four built-in DCS/PLC templates from hardcoded Rust constructor functions into a
contributable data file, so adding a new control-system family becomes a TOML pull request
rather than a Rust change — motivated by the goal of eventually shipping a full library of
DCS/PLC systems, which doesn't scale if every contributor has to learn the workspace layout
and get a Rust PR reviewed.

- **Format: TOML, not JSON or YAML.** The legacy app's `SettingsTemplates.json` had the right
  _content_ but the wrong _shape_ for a community catalog. JSON has no comments, and a shared
  catalog needs inline provenance (which manual a suffix came from, why a field is blank) —
  that alone rules it out as an authoring format. YAML's implicit typing is an active footgun
  here: `mode_manual_value`/`mode_auto_value`/`controller_action_direct_value` are, for some
  templates, literally the _strings_ `"true"`/`"false"`/`"0"` — YAML would silently coerce
  unquoted forms of these to bool/int on exactly the fields that decide whether a loop gets
  put into Manual. `toml` was already a dependency (`bhtune-cli`'s single-template import/
  export); the mainstream YAML crates are unusable under this project's `cargo deny` gate
  (`serde_yaml` is deprecated/archived, its `serde_yml` fork carries RUSTSEC-2025-0068). JSON
  import/export stays supported for interop; a `toml` export format is done (`template-cli`,
  see below) so the contribution loop is export → annotate → PR.
- **Embedded, not read from disk.** `crates/bhtune-core/templates/builtin.toml` is
  `include_str!`-compiled into the binary, so a shipped binary can never be broken by a
  missing or hand-edited data file, while contributors still edit a plain text file, not
  Rust. `built_in_templates()` is now `parse_catalog(BUILTIN_CATALOG).expect(...)`; a unit
  test parses and validates the embedded file, so a malformed contribution fails CI rather
  than shipping.
- **`parse_catalog(&str) -> Result<Vec<DcsTemplate>, TemplateError>`** is the pure parsing
  entry point — deserializes a private `Catalog { #[serde(rename = "template")] templates:
Vec<DcsTemplate> }` wrapper (TOML's array-of-tables idiom, one `[[template]]` block per
  entry) and then calls `.validate()` on every template, so a syntactically valid but
  semantically incomplete contribution (e.g. a mode suffix with no manual/auto value) is
  rejected at parse time, not mid-tune. Parsing a `&'static str` embedded at compile time is
  not I/O, so `bhtune-core`'s "no I/O, no clock, no async" purity rule is preserved — all
  _file_ reading stays in `bhtune-cli`, which reuses this exact function to load a user
  catalog from disk (`template-user-catalog`, done — see below).
- **`DcsTemplate::validate()`** mirrors the `LoopConfig::validate` precedent from
  `cli-safety`: non-empty `name` (trimmed); non-empty `process_variable_suffix`; non-empty
  `manipulated_variable_suffix`; if `controller_mode_suffix` is set, both
  `mode_manual_value` and `mode_auto_value` must be non-empty; if `mode_attribute_suffix` is
  set, `mode_attribute_program_value` must be `Some`. Runs on every catalog template (built-
  in, contributed, or user-catalog — `template-user-catalog`'s `load_user_templates` reuses
  `parse_catalog` directly, so the same validation applies with no separate code path) and,
  since `template-cli`, on `template import`'s single-JSON-template path too (`import_one`
  calls it explicitly, since that path parses with plain `serde_json::from_str` rather than
  going through `parse_catalog`) — a garbage template can no longer be imported and only
  fail much later, mid-tune.
- **`TemplateError`** is a hand-rolled `Display`/`std::error::Error` enum (no `thiserror`,
  matching `bhtune-core`'s existing `RangeError`/`LoopConfigError` convention): `Toml(toml::
de::Error)` (`source()` delegates to the wrapped error), `EmptyName`, `EmptyField { name,
field }`, `MissingModeValue { name, field }`, `MissingModeAttributeProgramValue { name }`.
  `toml::de::Error` (the resolved `toml 1.1.4+spec-1.1.0`) already derives `Clone`/`PartialEq`
  and implements `Display`/`Error`, so it's stored directly rather than flattened into a
  `String` — no information is lost converting a parse error into this crate's own error type.
- **New `DcsTemplate` fields, all `#[serde(default)]`** so both the TOML catalog and any
  existing JSON import/export stay backward-compatible: `versions: Vec<String>` (the DCS/PLC
  releases a template's tag conventions are known to apply to — see "Per-version templates"
  below), `description: Option<String>`, `source: Option<String>` (a documentation citation
  for where the tag mapping came from). Deliberately **no "verified" trust field** — an
  earlier draft proposed a hardware/documentation/unverified enum, dropped because it would
  need someone to adjudicate and maintain it per template, and a stale "verified" badge is
  worse than none; everything accepted into the catalog is treated as verified, and real
  errors get fixed as bugs when they surface.
- **Per-version templates, not per-vendor.** DCS vendors change tag conventions across major
  releases, so a single "Yokogawa CentumVP" entry can silently be wrong for a newer release.
  Each template carries a `versions` list of the releases it's known to apply to (e.g.
  `["R5", "R6"]`); when a release changes conventions, the contribution pattern is a **new
  template entry** with its own name and `versions` list, never editing an existing one in
  place, since sites on the older release still depend on that exact mapping. `name` stays
  the single unique key, so no lookup code has to change. The four seeded templates'
  `versions` reflect when each mapping was actually authored (~2015–2016), not an exhaustive
  tested matrix — recorded with a "current as of authoring" comment in `builtin.toml` so a
  later reader doesn't over-read the list as a coverage guarantee: Yokogawa CentumVP `["R5",
"R6"]` (field-confirmed), Honeywell Experion `["R400", "R410", "R430"]`, Schneider Modicon
  `["Unity Pro V8.0", "Unity Pro V8.1", "Unity Pro V11.0"]`, Allen-Bradley PlantPAx `["3.0",
"3.5", "4.0"]`.
- **`toml` promoted to `[workspace.dependencies]`** now that both `bhtune-cli` (single-
  template JSON/TOML import/export) and `bhtune-core` (the embedded catalog) consume it, per
  the root `Cargo.toml`'s own documented convention of promoting on a second consumer.
- **`bhtune-db` fallout, resolved by `template-provenance`.** `DcsTemplate` is `bhtune-db`'s
  own row type for `dcs_templates` (no separate DTO — see `db-schema`'s design note), so
  `row_to_dcs_template` had to start constructing the three new fields the moment
  `template-catalog` added them to `DcsTemplate` itself. They were read back as empty/`None`
  placeholders for one commit, documented in place as a stopgap; `template-provenance` (below)
  closed the gap immediately after by adding real `versions_json`/`description`/`source`
  columns, so every field now round-trips through actual storage rather than a placeholder.

**Testing approach.** 24 tests in `bhtune-core/src/template.rs` (18 new): the embedded
catalog parses and every built-in validates; each built-in's `versions`/`description`/
`source` match the researched seed values above; a minimal valid TOML template parses;
malformed TOML is rejected as `TemplateError::Toml`; every `validate()` branch is exercised
individually via targeted `str::replace` edits on a shared minimal-valid-TOML fixture (empty
PV suffix, empty MV suffix, mode suffix missing manual value, mode suffix missing auto value,
mode-attribute suffix missing program value); `TemplateError`'s `Display`/`std::error::Error`/
`source()` behavior is covered for every variant, not just the ones a validation branch
happens to construct. `cargo llvm-cov` confirms 100% line coverage of the new code.

### `dcs_templates.origin` replaces `is_builtin` (`template-provenance`)

`template-catalog` (above) taught `DcsTemplate` that a template can come from more than one
place, but `dcs_templates` itself still only had a two-state `is_builtin BOOLEAN` — no room
for a third state, which `template-user-catalog` (see "Auto-loading a user template catalog"
below) needs: a row seeded from a site's own `templates.toml` is neither a shipped built-in
nor a hand-imported user template, and treating it as either would break one of the two.
Done now, pre-release, specifically to avoid a later `ALTER TABLE` migration once real
installs exist.

- **`origin TEXT CHECK (origin IN ('builtin', 'catalog', 'user'))`** replaces `is_builtin
INTEGER`. `bhtune_db::models::TemplateOrigin` (`Builtin`/`Catalog`/`User`) — previously
  defined only for `tune_runs.template_origin` (`safety-run-snapshot`, with a temporary
  `from_is_builtin` bridge method) — moved to be `dcs_templates`' own type, since that's now
  its primary use; `tune_runs.template_origin` reuses the same enum for its run-start
  snapshot, and `from_is_builtin` was deleted as dead code now that `dcs_templates.origin` is
  real. `builtin` and `catalog` rows are re-upserted from their respective files on every
  startup; `user` rows (hand-imported or, eventually, GUI-created) are never auto-touched.
- **New `versions_json`/`description`/`source` columns** replace the placeholder empty/`None`
  values `template-catalog` had to read back (see the "resolved by `template-provenance`"
  bullet above) with the template's real, already-authored data — `versions_json` follows the
  same `CHECK (json_valid(...))`-plus-`serde_json`-round-trip idiom as `tags_json`/
  `template_snapshot_json`, needing no new `DbError` variant (`InvalidJsonShape`'s doc comment
  was broadened to mention it).
- **`seed_builtin_templates` generalized into `seed_templates(pool, templates, origin, now)`**,
  with `seed_builtin_templates` kept as a thin wrapper (`seed_templates(pool,
built_in_templates(), TemplateOrigin::Builtin, now)`) rather than renamed, since `bhtune-cli`
  already has established callers of the original name. `template-user-catalog` (see below)
  is the first real caller of `seed_templates` directly, with `TemplateOrigin::Catalog`. The
  `SkippedUserOwned` outcome generalizes the same way the boolean did: a row exists but its
  `origin` differs from the one being seeded, so it belongs to a different catalog/seed pass
  and is left untouched.
- **Three new tests in `tests/schema.rs`** cover what the schema alone had never proven: that
  the `origin` `CHECK` constraint actually rejects an invalid value (via a raw `UPDATE`, since
  `dcs_templates` has too many non-defaulted `NOT NULL` columns to hand-write a full raw
  `INSERT` the way the simpler tables' precedent tests do), that all three `origin`
  variants — including `Catalog`, which no production code path produces yet — round-trip
  through `DcsTemplateRow::get`, and that `row_to_dcs_template`'s `versions_json` decode-error
  path actually fires for JSON that is syntactically valid (satisfying the `CHECK`) but the
  wrong shape (`"123"`, a bare JSON number, isn't a `Vec<String>`) — the `CHECK` constraint
  alone only proves bad _syntax_ is rejected at the SQL layer, not that the Rust-level shape
  mismatch is handled once past it.

### Auto-loading a user template catalog (`template-user-catalog`)

`template-catalog`/`template-provenance` (above) made the catalog format and the database's
three-way `origin` real, but nothing yet populated `TemplateOrigin::Catalog` with real data —
a site's own `templates.toml` was still not read by anything. `bhtune-cli` now auto-loads one
on every startup, mirroring `bhtune.toml`'s own `cli-config` precedence chain exactly rather
than inventing a new pattern.

- **Resolution order: `--templates` > `BHTUNE_TEMPLATES` > `templates` config key > platform
  default.** `config::load_user_templates(cli_templates, config, xdg_config_home, home,
appdata, is_windows)` in `crates/bhtune-cli/src/config.rs` mirrors `load_config`'s own
  split between an _explicit_ path (CLI flag, already folded in by clap's `env =
"BHTUNE_TEMPLATES"`, or the config-file key) and the _auto-discovered default_ path — but
  it is a 4-tier chain, one tier deeper than `load_config`'s own 2-tier bootstrapping case,
  because `bhtune.toml`'s own path obviously can't be configured from inside itself, whereas
  the templates path _can_ have a config-file-key tier since `bhtune.toml` is already loaded
  by the time templates are resolved.
- **`templates_path_from(...)` mirrors `config_path_from` (the config directory), not
  `default_db_path_from`/`default_log_dir_from` (the data directory).** `templates.toml`
  lives in the same directory as `bhtune.toml` — `$XDG_CONFIG_HOME/bhtune/templates.toml` on
  Linux/macOS (falling back to `$HOME/.config/bhtune/` if unset), `%APPDATA%\bhtune\
templates.toml` on Windows — since both are per-user hand-edited settings files, not
  persistent application data.
- **Missing-file semantics depend on how the path was resolved, matching `bhtune.toml`'s own
  rule.** An auto-discovered default path that doesn't exist is `Ok(None)` — not an error, and
  the common case, since most installs never create `templates.toml` at all. An _explicit_
  path — from `--templates`/`BHTUNE_TEMPLATES` or the config file's `templates` key — that
  doesn't exist is a hard error naming the path, exactly like an explicit `--config` path that
  doesn't exist. A file that exists but fails to parse (malformed TOML) or fails
  `DcsTemplate::validate()` (e.g. a mode suffix with no manual/auto value) is always a hard
  error regardless of how the path was resolved, naming the file and the problem.
- **Reuses `bhtune_core::template::parse_catalog` directly** — the same function
  `template-catalog` built for the embedded built-in catalog already parses the `[[template]]`
  TOML shape _and_ calls `.validate()` on every template, so a single call handles both
  "shape" and "content" validation with no new parsing code in `bhtune-cli` at all.
- **`db::open`'s signature grew a `user_templates: Option<Vec<DcsTemplate>>` parameter.** It
  seeds the built-ins first (as before), then — only if `Some` — seeds the user catalog via
  `bhtune_db::seed_templates(&pool, templates, TemplateOrigin::Catalog, now)`, the first real
  production caller of the `Catalog` origin (previously exercised only by one round-trip
  test in `template-provenance`). `lib.rs`'s `run_with_cli_and_ctrl_c` calls
  `config::load_user_templates(...)` right after resolving the database path and before
  `db::open`, propagating a load/parse/validation failure through the same `fail()` exit path
  every other startup error uses.
- **`--templates <PATH>` global CLI flag**, placed alongside `--db`/`--config` in `args.rs`,
  with `env = "BHTUNE_TEMPLATES"` matching the other global flags' `BHTUNE_*` convention.

**Testing approach.** 2 new tests in `db.rs` (seeding a user catalog tags rows with
`TemplateOrigin::Catalog`; reseeding the same catalog is idempotent), 2 extended tests in
`args.rs` (the new flag/env var/default), and 16 new tests in `config.rs`: 5 for
`templates_path_from` (Windows with/without `%APPDATA%`, Unix via `$XDG_CONFIG_HOME`, Unix
via `$HOME` fallback, Unix with neither set) and 11 for `load_user_templates` (nothing
resolves → `None`; an auto-discovered path that's missing → `Ok(None)`, not an error; an
explicit CLI-flag path that's missing → error; an explicit config-key path that's missing →
error; a valid file parses; malformed TOML → error; a template failing `validate()` → error;
the generic-I/O-error branch, e.g. reading a directory as if it were a file → error; the CLI
flag winning over the config key; the config key being used when there's no CLI flag). Plus
one `lib.rs` integration test proving `run_with_cli` itself (not just the lower-level
`config::load_user_templates` unit) surfaces a broken `--templates` path as exit failure
before ever calling `db::open`. `cargo llvm-cov` confirms 100% line coverage of every line
this todo added or touched.

### Multi-template import, TOML export, and `template delete` (`template-cli`)

`template-catalog`/`template-provenance`/`template-user-catalog` (above) made TOML catalogs a
real, auto-loaded data source, but `bhtune template import`/`export` still only understood a
single JSON template, and there was no way at all to remove a template once imported or
auto-seeded — a real dead end now that startup auto-loads a user catalog. This closes both
gaps in `crates/bhtune-cli/src/commands/template.rs`.

- **`template import` auto-detects JSON vs. TOML by sniffing content, not extension or a
  try-then-fallback.** `looks_like_json_object` checks whether the file's content, ignoring
  leading whitespace, starts with `{`; if so it's parsed as a single JSON template (the
  existing `import_one`, unchanged hard-fail-on-name-collision behavior); otherwise it's
  parsed as a TOML catalog via `bhtune_core::template::parse_catalog` (the new
  `import_catalog`). Chosen over "try JSON, fall back to TOML on failure" specifically so a
  malformed file of either format gets a format-specific, useful parse error rather than
  always surfacing the TOML parser's complaint about content the user actually meant as
  JSON — legitimate TOML catalog files can never start with a bare `{` at the document root
  (not valid top-level TOML), so the heuristic never misclassifies real content of either
  format.
- **`import_catalog` is best-effort, deliberately unlike `import_one`.** A single-JSON-
  template import still hard-fails on a name collision (unchanged — it's one deliberate
  template, so a collision is a mistake to fix). A multi-template TOML catalog import
  instead skips any colliding name and reports both what was imported and what was skipped,
  because the expected workflow is re-importing an updated community catalog file that
  overlaps with templates already present — the useful outcome is "add what's new," not
  "fail because some of this was already here." An empty catalog (`template = []`) is its
  own message, not an error.
- **`template export --format <json|toml>`** (new `TemplateFileFormat` `ValueEnum` in
  `args.rs`, default `json`, so the existing default behavior is unchanged for anyone not
  passing the flag). The TOML path is `bhtune_core::template::to_catalog_toml(vec![row.
template])` — a single-template file is just a one-entry catalog, so it round-trips
  through the exact same `parse_catalog` used everywhere else, and the output is a
  `[[template]]` block ready to paste into a contribution PR (the export → annotate → PR
  loop `template-catalog`'s design section had planned for since before this todo existed).
- **`template delete <name>`** — there was previously no way to remove a template at all.
  Looks the template up by name (the existing "no template named" error if missing), then
  calls `bhtune_db::models::DcsTemplateRow::delete`: `Ok(true)` deletes and prints an
  origin-specific note for `Builtin`/`Catalog` templates (both will silently reappear on the
  next startup unless also removed from their source — the embedded catalog for `Builtin`,
  which only a new release can change, or the user's `templates.toml` for `Catalog` — so the
  CLI says so up front rather than letting a confused re-appearance be discovered later);
  `Ok(false)` (a same-process TOCTOU race — something else deleted the row between the
  lookup and the delete call) is reported as "already deleted" rather than treated as a bug;
  `Err(DbError::TemplateInUse)` becomes a friendly "still referenced by one or more saved
  loops" message instead of a raw SQL error, deleting nothing.
- **`template list` gained a VERSIONS column** (between ORIGIN and PROPORTIONAL),
  formatting `versions.join(", ")` or `"-"` for a template with none recorded.
- **`import_one` (the single-JSON-template path) now calls `DcsTemplate::validate()`
  itself**, before the name-collision check. It parses with plain `serde_json::from_str`
  rather than going through `parse_catalog`, so — unlike `import_catalog`, which inherits
  validation for free from `parse_catalog` — it never got the validation `template-catalog`
  added to `DcsTemplate` in the first place; this closes that gap explicitly.
- **`bhtune-cli` gained a `sqlx` dev-dependency** (not a production one — this crate's
  non-test code still only ever talks to `bhtune-db`'s repository API) purely so the
  `delete`-still-referenced test can insert a raw `loops` row via SQL, mirroring the exact
  pattern `bhtune-db`'s own `tests/schema.rs` already uses for the identical FK check; there
  is no `LoopRow::insert` yet (loop-saving is `cli-commands` scope, not this todo's).

**Testing approach.** 22 tests in `commands/template.rs` (up from 10) cover: TOML single-
template export round-tripping through import under a renamed identity; a multi-template
TOML catalog import adding new entries while skipping ones that already exist by name (and
the all-new/all-skipped edge cases separately); an empty-catalog import being a no-op;
invalid-TOML-catalog import producing a TOML-specific error (proving the content-sniffing
heuristic routes correctly, alongside the pre-existing invalid-JSON-import test); a JSON
template that parses but fails `validate()` being rejected without ever reaching the
database; `delete` succeeding for `User`/`Builtin`/`Catalog`-origin templates (each
exercising its own note branch); `delete` failing cleanly on an unknown name and on a
template still referenced by a
loop (the latter inserting a real `loops` row, per the `sqlx` dev-dependency note above); and
`list` formatting a template with no recorded `versions` as `"-"`. One further test added to
`bhtune-db/tests/schema.rs` (`dcs_template_delete_reports_a_non_database_error_as_query_not_
template_in_use`, using a closed pool to force a non-`Database` `sqlx::Error`) closes a
coverage gap in `DcsTemplateRow::delete`'s own classification logic that predates this todo.
Two branches remain deliberately untested and documented in place, consistent with this
project's existing accepted-gap precedent (e.g. `safety-db-restore`'s exclusivity-check
residual race): `delete`'s own same-process TOCTOU branch, and the CLI-level passthrough for
a non-`TemplateInUse` `DbError` from `DcsTemplateRow::delete` — both require a database error
to occur between two calls that share one connection pool with no `.await` yield point
between them, which is not deterministically constructible without fault-injection seams
this project has no other use for. `cargo llvm-cov` confirms these are the only three lines
(across those two branches) left uncovered in the entire file.

## `server-start-tune-api`: starting and cancelling a tune over HTTP

`crates/bhtune-server/src/routes/runs.rs` adds `POST /api/runs` (start) and
`POST /api/runs/{id}/cancel` (cancel), closing the gap `frontend-screens` surfaced: every
remaining GUI screen needs a way to actually start a tune, and until now `bhtune-server`'s API
was read-only plus template CRUD-minus-update.

**Reuses `bhtune-cli`'s orchestration; does not reimplement it.** `start_run` calls
`bhtune_cli::commands::tune::prepare()` inline (template lookup, tag derivation, a real
backend connect attempt, the `tune_runs` insert) and, once that succeeds, `tokio::spawn`s
`bhtune_cli::commands::tune::drive()` (the polling/tuning phase itself) as a background task
tracked by a new `crate::active_run::ActiveRun` (an `Arc<Mutex<Option<ActiveRunEntry>>>`
shared via `AppState`). `POST /api/runs` returns `201 Created` with the same
`RunDetailResponse` shape `GET /api/runs/{id}` would show for this run at this instant
(almost always still `outcome: "running"`) as soon as `prepare()` succeeds — it does not wait
for the tune to finish. `POST /api/runs/{id}/cancel` signals the background task's `CtrlC`
handle and awaits it reaching a terminal outcome, then returns `204 No Content`; cancelling
an already-finished or unknown run is not an error (`204`/`404` respectively, matching the
CLI's own idempotent-cancel precedent). v1 allows only one active run at a time, enforced by
`ActiveRun` itself, not by any per-loop locking.

**`StartRunRequest` mirrors `TuneArgs` field-for-field**, with `#[serde(default = "...")]`
helpers reproducing the CLI's own clap defaults exactly (`sim_gain`/`sim_tau`/
`sim_dead_time`/`poll_interval_ms`/etc.), so a client that only cares about a few fields gets
the same behavior `bhtune tune`'s bare flags would. `into_tune_args()` is where a real,
previously-invisible gap gets closed: every value clap's `value_parser`s would normally
validate (finite floats, positive integers) arrives here with **no** such validation, because
constructing a `TuneArgs` directly in Rust code bypasses clap entirely. `require_finite`/
`require_finite_if_some`/`require_positive` close that gap explicitly, each producing a `400`
naming the offending field. Fields already covered by `LoopConfig::validate()` inside
`prepare()` itself (`relay_amp`, `cycles_count` after defaulting, `mrft_delay`) are
deliberately _not_ re-checked here, to avoid two divergent copies of the same rule.

**Two-tier conflict detection, and why both tiers are real.** `start_run` first does an
optimistic pre-check (`state.active_run.active_run_id().await`) purely to avoid a wasted
`prepare()` call (a real backend connection attempt, a DB insert) in the common case where a
run is obviously already active. This is _not_ authoritative: `prepare()` awaits real
database I/O, which is exactly the kind of gap that lets two near-simultaneous
`POST /api/runs` requests both pass the pre-check before either reaches the actual
`state.active_run.start(...)` call — the real, authoritative check. Losing that second,
deeper race is handled distinctly from losing the shallow one: since `prepare()` already
succeeded (a `tune_runs` row exists, but no backend I/O beyond the connect attempt has
happened), the just-inserted row is explicitly marked `failed` via `TuneRunRow::fail(...)`
rather than left forever showing `outcome: "running"` for a run that will never actually
progress. The two rejection messages are worded differently on purpose (the shallow one says
"cancel it first via ..."; the deep one says "no backend I/O was performed") so a caller —
and this phase's own tests — can tell which check actually fired.

**The `Send` fix in `bhtune-cli` this required.** Spawning `drive()` as a `tokio::spawn`
background task requires its future to be `Send + 'static`. The first compile attempt failed:
`drive()` calls `execute()`, which constructed `std::io::stdin().lock()` (a `StdinLock`,
`!Send` because it wraps a `std::sync::MutexGuard`) inline as an argument to an internal
`.await`ed call inside the `RestoreAttempt::Confirmed` write-back branch. Because
`async fn` desugars to one monolithic generated future type per function, _any_ `!Send` local
live across _any_ `.await` point — even in a branch never taken at runtime — makes the whole
generated future `!Send`, and `execute()` was a single non-generic function, so its one
compiled future type was permanently unsendable regardless of which runtime branch actually
touched the reader. This was harmless for the CLI's own use (`run_with_ctrl_c`'s future is
only ever `.await`ed directly inside `#[tokio::main]`, never spawned) but fatal for
`bhtune-server`. The fix: made `execute()` **generic over the reader type**
(`async fn execute<R: std::io::BufRead>(..., reader: &mut R)`), with **no explicit `Send`
bound on `R`** — Rust's monomorphization then produces a _separate_ concrete future type per
instantiation, each independently checked. `run_with_ctrl_c()` instantiates it with
`&mut std::io::stdin().lock()` (`!Send`, fine — never spawned); `drive()` instantiates it with
`&mut std::io::empty()` (`std::io::Empty` is `Send + Sync + Clone + Copy` and behaves as
immediate EOF, exactly the right semantic for "no human present to answer an interactive
write-back prompt" — `maybe_write_back`'s existing EOF/blank-input-skips-write-back logic
already handles it gracefully). A `spawn_local`/`LocalSet` architecture change was considered
and rejected as disproportionate — it would force the entire axum server onto a
single-threaded runtime flavor to accommodate one `!Send` value in one rarely-hit branch.
**This is a reusable pattern, not a one-off:** any future function that is sometimes spawned
and sometimes not, and that holds a genuinely-optional `!Send` resource only on one branch,
should reach for "make the resource type generic" before reaching for `spawn_local`.

**Test coverage, including a genuinely reliable concurrency test.**
`cargo llvm-cov -p bhtune-server` reports 99.35%→99.59% line coverage on `routes/runs.rs`
(97.77% region, 100% function) after 14 tests (up from the initial 10), with only two lines
left uncovered — both defensive `panic!` message-format arguments on assertions that never
fail in a passing suite (`wait_for_outcome`'s 10-second-timeout guard, and the race test's own
`else` branch), matching this project's existing accepted-gap precedent
(`core-tuning-math`/`backend-simulator`'s "passing-assert's message-format argument"). Of the
four new tests, the most interesting is
`a_genuine_race_between_two_starts_marks_the_losing_row_failed`: it calls the `start_run`
handler function _directly_ (bypassing the router/tower/hyper stack entirely — `State(state)`
and `Json(request)` are plain public tuple-struct constructors, not just `FromRequest`
extractors) and races two invocations with `tokio::join!`. This reliably lands in the deep
"authoritative race lost" branch — verified empirically across 45+ repeated runs with zero
failures — because `#[tokio::test]` defaults to a single-threaded runtime, where
`tokio::join!` polls both futures on the same task and genuinely interleaves at each
`prepare()` `.await` point (real, if in-memory, SQLite I/O), giving both requests a fair
chance to pass the optimistic pre-check before either reaches the authoritative check. This
is a deterministic, non-flaky test, not the "accept the gap" fallback that was the working
assumption before it was attempted.

## `server-embed-spa`: embedding the built SPA into the binary

`crates/bhtune-server/src/spa.rs` embeds the built React SPA (`frontend/dist/`) directly into
the `bhtune-server` binary, so a release build is one self-contained executable that needs
nothing else — no separate static file server, no Node/nginx on the target host — matching the
Windows-installer/single-binary deployment shape this project has targeted since the Tauri
reversal (see "Key architectural decisions").

**`rust-embed`, not a hand-rolled static file server.** `Assets` is a
`#[derive(RustEmbed)]` struct:

```rust
#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../frontend/dist/"]
#[allow_missing = true]
struct Assets;
```

Three feature choices, each verified empirically against the crate's actual behavior in an
isolated scratch project rather than assumed from the README alone:

- **`interpolate-folder-path`** substitutes `$CARGO_MANIFEST_DIR` with an absolute path at
  compile time. Without it, rust-embed's documented debug-mode default resolves `#[folder]`
  _relative to wherever the binary is run from_ (reading live from disk on every request) —
  fine for `cargo run` from the repo root, fragile for a systemd unit or Windows Service
  starting the binary from an arbitrary working directory. With the absolute path baked in,
  resolution is CWD-independent in debug mode too — confirmed by building a debug binary and
  running it from `/tmp`, and it still found its assets.
- **`mime-guess`** exposes `EmbeddedFile.metadata.mimetype() -> &str` directly, so
  `static_handler` never needs a redundant direct `mime_guess` dependency (the crate's own
  official `axum-spa` example depends on `mime_guess` directly instead — read as a design
  reference, not used as a dependency here).
- **`deterministic-timestamps`** zeroes embedded files' timestamps, so a release binary built
  twice from the same source is byte-reproducible.
- **`#[allow_missing = true]`** (a struct attribute, not a Cargo feature) is what makes a
  missing `frontend/dist/` a clean runtime condition — `Assets::iter()` empty,
  `Assets::get(...)` always `None` — instead of rust-embed's default hard compile-time error.
  This matters concretely: `frontend/dist/` is gitignored and CI's Rust-only `check` job never
  runs `pnpm run build` first, so without this attribute the workspace simply would not
  compile there.

**`static_handler` is the whole router's single `.fallback(...)`**, appended in
`build_router` after every other merged route module:

- A path that matches an embedded file is served with its real MIME type and one of two
  cache rules: `Cache-Control: no-cache` for `index.html` (it names the _current_ build's
  content-hashed asset filenames, so it must always be revalidated) and
  `public, max-age=31536000, immutable` for every other embedded path (a Vite content hash
  means a new build always emits a new filename, so caching indefinitely is safe).
- A path with no `.` in its last `/`-segment falls back to `index.html` — this is the SPA
  route (React Router's `BrowserRouter` uses real HTML5 history paths, not hash routing, so a
  server-side fallback is genuinely required for a direct load or hard refresh of, say,
  `/runs/42` to work at all).
- A path that _does_ look like a real static-asset request (has a dotted extension) but
  doesn't match any embedded file is a real `404`, not a silent SPA-fallback — otherwise a
  typo'd asset URL would return an HTML page with a `200`.
- If the SPA was never built at all (`Assets::iter()` empty), every request gets a `503`
  naming the fix (`run pnpm install && pnpm run build` in `frontend/`, or `pnpm run dev`
  there against this server for local frontend development with hot-reload) instead of a
  confusing generic 404.
- Since this is the router's _only_ fallback, it never collides with another sub-router's
  own fallback — axum panics at router-build time if two merged routers each declare one,
  which is why no other route module in this crate sets one.

**A subtle bug caught by writing a standalone verification script instead of trusting
intuition.** The "does this path look like a real static asset" check needs the _last_
`/`-segment. The first draft used `path.rsplit('/').next_back()`, which reads as "reverse-split,
then take from the back" — but `rsplit`'s iterator already yields segments back-to-front, so
`.next_back()` un-reverses that back to _front_-to-back order and returns the _first_ segment,
not the last. A tiny standalone Rust script proved this empirically for `"assets/foo.js"`
before the fix (`path.rsplit('/').next()`, which correctly returns `"foo.js"`) was trusted.

**5 tests in `spa.rs`**, all gracefully degrading based on whether `frontend/dist/` actually
exists locally (checked via a small `frontend_is_built()` helper) — they assert real file
serving, correct cache headers, and SPA-fallback content when the SPA is built, and always
assert the `503` path regardless, so the suite passes both in CI's Rust-only `check` job
(where `frontend/dist/` never exists) and in a fully-built local dev environment. Manually
verified end-to-end against both a debug and a `--release` binary, run from a directory
unrelated to the crate (proving no accidental CWD dependency survived): `/` served
`index.html` with `no-cache`; a real hashed asset served with the long-lived immutable cache
header and the correct content type; `/runs/1` (a client-side route) fell back to
byte-identical `index.html` content; a genuinely missing asset path 404'd; `/api/health`
still resolved correctly (proving the fallback never shadows a real API route); and the 503
path was confirmed twice — once via the unit tests, once by starting a real server with
`frontend/dist/` temporarily moved aside and curling `/` directly.

`frontend/vite.config.ts`'s dev-mode API proxy is unaffected by any of this — it is a
`pnpm run dev` concern (hot-reload against a running `bhtune-server` for its API only), fully
orthogonal to how a release binary serves its own already-built assets.

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
   **Cautionary note:** `core-tuning-math`'s first implementation stated this rule correctly but
   didn't actually follow it — `measure_oscillation` computed elapsed time via
   `chrono::Duration::num_seconds()` (whole-second-truncating) _unconditionally_, with
   `TuningMathCompat.replicate_period_truncation_bug` only gating an additional 24-hour wrap on
   top of the already-truncated value. Every existing unit test used whole-second switch-time
   offsets, so this was lossless in every test and went unnoticed until `e2e-simulator`'s real,
   millisecond-spaced subprocess timing hit it directly, silently zeroing `ti_minutes`/
   `td_minutes` even for PI/PID. Fixed by switching to `num_milliseconds()` for the default path;
   see the `measure_oscillation_keeps_sub_second_precision_by_default` regression test in
   `tuning_math.rs` and `e2e_simulator.rs`'s module doc for the full story. The lesson: writing
   the rule down is not sufficient on its own — it needs test coverage with genuinely
   sub-second-precision inputs, not just whole-second ones, to actually enforce it.
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

## Documentation contract (`docs-contract`)

A documentation update is part of the definition-of-done for any change that alters
user-visible behavior — a new CLI flag or subcommand, a config key, an HTTP endpoint, a
default value, an error message a user would act on, a template/catalog field, or a safety
rule. There is no dedicated "catch up on docs" phase later; drift that isn't fixed in the same
change tends to never get fixed.

What to update, in order of how much it costs to get wrong:

1. **Generated references** (CLI help text via `clap`, the OpenAPI spec, the generated TS
   client) are never hand-edited and never go stale by definition — the build regenerates them
   and CI's `git diff --exit-code` gates fail if a commit forgets to include the regenerated
   output. Nothing to remember here beyond running the generator before committing.
2. **This file (`AGENTS.md`)** — update the relevant phase's roadmap bullet under "Phases and
   todos", the Status paragraph if the change is significant enough to shift what's next, and
   (for anything correctness-critical or easy to silently regress) a new numbered item under
   "Correctness-critical design details" or an addition to an existing one. This file is
   written for future coding-agent sessions rather than end users, but it is exactly as
   load-bearing as user-facing docs: a session that trusts a stale "not yet implemented" line
   redoes already-finished work, and one that trusts a stale design-rule description can
   reintroduce a bug that was already fixed once (see item 2 under "Correctness-critical design
   details" for a real example of exactly that risk, just in the other direction — the rule was
   documented correctly but the code didn't follow it).
3. **`README.md`** — anything a new user or contributor reads first: setup, CLI usage,
   architecture, the published roadmap. Keep it accurate to what's actually shipped, not to
   what's planned.
4. **`docs/`** (prose guides, `docs/dcs-templates.md`, `docs/v1-checklist.md`) and
   `CONTRIBUTING.md` — update whichever of these describes the area being changed.

Write documentation as state-of-the-world facts, not a changelog of what just happened.
"The CLI rejects `--cycles-count 0`" is documentation; "Added validation for `--cycles-count`"
is process narrative that belongs in the commit message and PR description, not in a file a
reader opens to learn how the software behaves today.

**Backstop, not a substitute.** `docs-copilot-hook` (a `sessionEnd` Copilot CLI hook, see
`.github/hooks/README.md`) prints a cheap, non-blocking warning for the single most common
miss — a session that changed `crates/**` without touching any documentation surface — but it
is a safety net for an honest oversight, not a license to skip this step and let the hook catch
it. It cannot judge whether documentation is actually _good_, and it has no way to catch drift
in behavior that never touched `crates/**` at all (a `frontend/`-only or CI-workflow-only
change with real user-visible impact, for instance).

## Conventions

- **Trunk-based git flow**: single long-lived `main`, short-lived PR branches
  (`<type>/<short-description>`), squash merges, no `develop`/release branches. Releases are
  tagged directly off `main`.
- **Commits**: [Conventional Commits](https://www.conventionalcommits.org/).
- **Formatting/linting**: `cargo fmt --check --all` and
  `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- **No unused dependencies**: `cargo machete` runs in CI. Placeholder crates (any stub not yet
  consuming a path dependency) deliberately carry **no** dependency on other workspace crates
  until they actually use one — don't add `bhtune-core` etc. back as a path dependency just to
  "wire up the graph"; add it when real code needs it. `bhtune-server` graduated out of this
  category with `server-http-api` (it now depends on `bhtune-core`/`bhtune-db`/`bhtune-cli`).
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
- **Frontend build**: `pnpm --filter bhtune-frontend run build` (`tsc -b` typecheck + Vite
  production bundle)
- **Frontend lint/format**: `pnpm --filter bhtune-frontend run lint` (oxlint) and
  `pnpm --filter bhtune-frontend run format:check` (Prettier, configured by the repo-root
  `.prettierrc.json`/`.prettierignore` — see "Key architectural decisions" above)
- **Frontend API client**: `pnpm --filter bhtune-frontend run generate:api` regenerates
  `frontend/src/api/schema.d.ts` from the repo-root `openapi.json` — run this after any
  `bhtune-server` route/DTO change and commit the result; CI fails on drift
- **Frontend dependency hygiene**: `pnpm run check:licenses` (from the repo root) — the
  npm-side counterpart to `cargo deny check`, see "Key architectural decisions" above

### Coverage enforcement

Coverage is tracked by Codecov and enforced at **100%** via `codecov.yml` (project and patch
targets both at 100% with a 1% threshold). Even placeholder code must be exercised by a test —
see the `main_runs_without_panicking` smoke tests in each binary crate's `main.rs` for the pattern
used to keep the gate meaningful (not vacuous) from the very first commit. Delete each one once
that binary does something real and gains its own targeted tests.

## Crate map and phase status

| Crate              | Phase                                                                                                                                 | Status                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bhtune-core`      | `core-model`/`core-mrft`/`core-tuning-math`/`template-catalog`/`core-replay-harness`                                                  | `core-model` + `core-mrft` + `core-tuning-math` + `template-catalog` done (the four built-in DCS templates now parse from an embedded, contributable TOML catalog — see "Community DCS/PLC template catalog" below); replay harness pending                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `bhtune-backend`   | `backend-trait`/`backend-opcda`/`backend-simulator`/`backend-replay`                                                                  | `backend-trait` + `backend-opcda` + `backend-simulator` done (trait, error model, OPC DA implementation, and FOPDT simulator, all tested); replay pending                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `bhtune-db`        | `db-schema`/`db-seed-templates`/`history-query-api`/`db-backup-restore`/`template-provenance`                                         | All done (7 tables, tested; 4 templates auto-seed on startup; run-history repository layer with lifecycle, filtering, and pagination; whole-database backup/restore via `VACUUM INTO`, hardened with an exclusive-access requirement by `safety-db-restore`; `dcs_templates` gained a real three-way `origin` column plus `versions_json`/`description`/`source` — see "Live-plant safety hardening" and "Community DCS/PLC template catalog" below)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `bhtune-cli`       | `cli-commands`/`cli-config`/`cli-automation`/`cli-safety`/`cli-logging`/`template-user-catalog`/`template-cli`                        | All five sub-phases done (subcommands, see "CLI reference" above; `CLI > env > TOML > default` config precedence, see "Config precedence" above; `--yes`/`--write-pid`/`--output json` and distinguished exit codes, see "Automation" above; relay-amp validation and mandatory `--timeout-secs`, see "Safety" above; `tracing` file+stderr logging, see "Logging" above) — a fully headless, scriptable CLI, no server required. The Phase 6.5 live-plant safety hardening pass following a post-`cli-logging` review is also done; see "Live-plant safety hardening" below. `template-user-catalog` (Phase 6.6) is also done: auto-loads a user catalog file on startup via the same config precedence chain — see "Auto-loading a user template catalog" above. `template-cli` is also done: multi-template TOML import/export and `template delete` — see "Multi-template import, TOML export, and `template delete`" above                                                                                                                                                                                                                          |
| `bhtune-server`    | `server-http-api`/`openapi-contract`/`server-start-tune-api`/`server-template-update-api`/`server-embed-spa`/`server-windows-service` | `server-http-api` + `openapi-contract` + `server-start-tune-api` + `server-template-update-api` + `server-embed-spa` done — real Axum binary (health/templates full CRUD/history/runs routes, graceful shutdown, shares the CLI's config/db/logging bootstrap), full OpenAPI 3.1 contract (`utoipa` annotations, `ApiDoc` aggregator, `/api/openapi.json`, Scalar UI at `/api/docs`, checked-in spec with a CI diff gate — see "Key architectural decisions" above), `POST /api/runs`/`POST /api/runs/{id}/cancel` starting and cancelling a real tune over HTTP by reusing `bhtune-cli`'s own `prepare()`/`drive()` orchestration — see "`server-start-tune-api`: starting and cancelling a tune over HTTP" below — `PUT /api/templates/{name}` editing an existing `user`-origin template in place (400 on a name mismatch, 404 if unknown, 409 if not user-owned), and the built SPA embedded directly into the binary via `rust-embed` with an SPA-fallback route, correct MIME types, and long-lived cache headers on hashed assets — see "`server-embed-spa`: embedding the built SPA into the binary" below; only Windows service support pending |
| `frontend/` (pnpm) | `frontend-shell`/`frontend-screens`/`frontend-live-stream`                                                                            | All three done — React + TS + Vite + Tailwind CSS v4 SPA (`bhtune-frontend`), TanStack Query, a typed `openapi-fetch` client generated from `openapi.json` with its own CI drift gate, and an npm license-allowlist gate mirroring `cargo-deny` — see "Key architectural decisions" above. Routing shell, Templates (List/Detail/Create/Edit), History (List/Detail), a combined New Run screen (Connection/Tag-mapping/Test-parameters/Simulator/Write-back in one form, plus run cancellation), and a live PV/MV trend chart (`TrendChart`, uPlot-based, fed by a new SSE `useRunStream` hook while a run is active and by `useRun`'s `samples` once terminal) are all done and manually verified against a real running server                                                                                                                                                                                                                                                                                                                                                                                                                        |

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
   own, with no server required. The Phase 6.5 live-plant safety hardening pass is also done —
   see "Live-plant safety hardening" above. Phase 6.6, turning the built-in DCS/PLC templates
   into a community-contributable catalog, is done: `template-catalog` (`bhtune-core`),
   `template-provenance` (`bhtune-db` schema: a real three-way `origin` column plus
   `versions_json`/`description`/`source`), `template-user-catalog` (`bhtune-cli` auto-loads
   a user-supplied catalog file on startup, resolved through the same config precedence chain as
   every other setting), `template-cli` (multi-template TOML import/export, `template
delete`, and validating a single-JSON-template import too), and `template-docs` (README/
   `CONTRIBUTING.md`/`docs/dcs-templates.md` documenting the catalog and inviting
   contributions) are all done — see "Community DCS/PLC template catalog", "Auto-loading a
   user template catalog", and "Multi-template import, TOML export, and `template delete`"
   above.
7. **Web GUI (`bhtune-server` + React SPA)** — `server-http-api`, `openapi-contract`, and
   `frontend-shell` are all done: `bhtune-server` promoted from stub to a real Axum server
   exposing `/api/health`, `/api/templates` (list/get/create/delete), and `/api/runs`
   (filtered/paginated list, full run detail) over the tuning engine, sharing the CLI's config
   precedence and database bootstrap, with graceful shutdown on Ctrl+C/`SIGTERM`; every
   route/DTO is annotated with `utoipa`, aggregated into one OpenAPI 3.1 document served at
   `/api/openapi.json` and as an interactive Scalar UI at `/api/docs`, checked in at the repo
   root and drift-gated in CI, with every fallible route's error responses now typed with a
   real `ErrorBody` schema instead of `content?: never`; `frontend/` (`bhtune-frontend`) is a
   pnpm-workspace React + TS + Vite + Tailwind CSS v4 SPA using TanStack Query against a typed
   `openapi-fetch` client generated from that same spec, with its own CI-enforced
   regenerate-and-diff gate and a new npm license-allowlist gate mirroring `cargo-deny` — see
   "Key architectural decisions" above for all of this. `frontend-screens` is now fully
   done: a `react-router` routing shell (`AppLayout` nav + health badge), the Templates
   screens (List/Detail/Create/Edit), and the History screens (List/Detail) are done and
   verified against a real running server. `server-start-tune-api` is now done:
   `POST /api/runs`/`POST /api/runs/{id}/cancel` start and cancel a real tune over HTTP,
   reusing `bhtune-cli`'s own `prepare()`/`drive()` orchestration rather than duplicating it —
   see "`server-start-tune-api`: starting and cancelling a tune over HTTP" above, including
   the `Send`-trait fix this required in `bhtune-cli` before a tune could be spawned as a
   background task at all. That unblocked `frontend-screens`'s second slice, also done
   and manually verified against a real running server: a combined New Run screen
   (Connection, Tag mapping, Test parameters, Simulator parameters, and Write-back-on-
   completion in one form), run cancellation, and (at the time) a polling-based
   live-progress banner on the run detail screen — see the dedicated bullet above for the
   two real bugs this manual verification caught and fixed. `server-template-update-api` is
   done too: `PUT /api/templates/{name}` edits an existing `user`-origin template in place,
   which unblocked `frontend-screens`'s third slice, the Template Edit screen — see the
   dedicated bullet above. `frontend-live-stream` is now also done: `GET /api/runs/{id}/
stream` (SSE, polling a new `TuneSampleRow::list_for_run_since` query) plus a
   `useRunStream` hook and a reusable `uPlot`-based `TrendChart` component replace that
   polling banner with a real live-updating PV/MV trend chart, handing off cleanly to the
   historical `samples` array once a run completes — see the dedicated bullet above for
   the full design and its manual browser verification. `server-embed-spa` is now also
   done: the built SPA is embedded directly into the `bhtune-server` binary via
   `rust-embed` (an `Assets` struct over `frontend/dist/`, an SPA-fallback route, correct
   MIME types via the `mime-guess` feature, and long-lived immutable cache headers on
   Vite's content-hashed assets, versus `no-cache` on `index.html` itself), so a release
   build is one self-contained executable with no separate static file server needed — see
   "`server-embed-spa`: embedding the built SPA into the binary" above for the full design
   and its manual end-to-end verification. Remaining: running as a proper platform service
   (`server-windows-service`). Replaces the earlier Tauri desktop GUI
   phase — see "Key architectural decisions" above for the reversal.
8. **End-to-end testing and CI** — `e2e-simulator` is done: a genuine subprocess-level test
   (`crates/bhtune-cli/tests/e2e_simulator.rs`) spawns the real `bhtune tune` binary against the
   simulator backend across a small process/controller-type matrix (all `direction=reverse`, the
   direction empirically confirmed to actually oscillate against this simulator's fixed FOPDT
   parameters), then opens the resulting SQLite database directly and asserts the _calculated_
   PID results are sane — positive, correctly-ordered `kp` across all three response levels,
   response-level-invariant `ti_minutes`/`td_minutes`, and a non-empty sample trail — closing a
   real gap no earlier test covered (existing subprocess tests only checked the JSON summary's
   shape/exit code, and existing in-process tests only checked row presence/counts, never actual
   values). Writing it surfaced and fixed a real `bhtune-core` bug in the process — see
   "Correctness-critical design details" above, item 2. `e2e-playwright` is also done: a
   Playwright suite (`frontend/e2e/`) drives a full tune through the real, built React SPA
   served by a real `bhtune-server` binary (debug profile -- serves `frontend/dist/` live off
   disk, no re-embed step needed between runs) over the in-process simulator backend --
   `smoke.spec.ts` (app shell, health badge, seeded template list, header nav) and
   `tune.spec.ts` (a full tune through `/runs/new` with `e2e_simulator.rs`'s own
   millisecond-scale simulator parameters, asserting sane/ordered rendered Kp/Ti/Td values,
   plus cancelling an in-flight run). `.github/workflows/e2e.yml` builds the frontend and a
   debug `bhtune-server`, installs Chromium, and runs the suite in CI, uploading the HTML
   report on failure. A direct dividend of dropping Tauri: `tauri-driver`/WebDriver would
   have been markedly more fragile in CI than plain Playwright against a real browser.
   Remaining: golden replay suite in CI (`e2e-golden-ci`, blocked on `trace-fixtures`/
   `capture-traces`); release build matrix for Linux/macOS/
   Windows (`build-matrix`, via `cargo-dist`, embedding the built SPA — no Tauri bundler or
   WebView runtime to manage).
9. **Documentation and release** — two prerequisites are already done, front-loaded ahead of
   the rest of this phase since they're cheap and are what actually prevents drift: a
   documentation contract in this file (`docs-contract`, see "Documentation contract" above)
   and a paired `sessionStart`/`sessionEnd` Copilot CLI hook warning when a session changes
   `crates/**` without touching any documentation surface (`docs-copilot-hook`, see
   `.github/hooks/README.md`). Remaining: README/usage docs and a getting-started guide,
   published roadmap (OPC UA/Modbus backends, free remote/multi-user access, Step Test pending
   the bridge `Subscribe` RPC, multi-loop/batch tuning), v0.1.0 with per-platform binaries, a
   Windows MSI installer (`pkg-windows-installer`, the primary distribution artifact), and a
   secondary Docker image (`pkg-docker`).
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
  abort-and-restore; none of it is optional polish. A follow-up live-plant safety hardening pass
  (Phase 6.5, done — see "Live-plant safety hardening" above) closed nine more findings from a
  further review, including making Ctrl+C/timeout cancellation reach an in-flight backend call,
  guaranteeing a restore on every exit path, and enforcing OPC quality.
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
