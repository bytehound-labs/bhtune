# bhtune

An open-source Rust PID control-loop auto-tuner for industrial DCS/PLC systems (Yokogawa
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
place. `bhtune-driver`'s `Driver` trait and error model (`driver-trait`) are
defined and tested, its OPC DA implementation (`driver-opcda`, `OpcDaDriver`) is
done — the primary v1 driver, over the published `opcda-bridge` crate — and its in-Rust
FOPDT process simulator (`driver-simulator`, `SimulatorDriver`) is done, giving CI a
fully synthetic, wall-clock-free way to drive a real `MrftEngine` end to end. `bhtune-cli`'s
core subcommand set (`cli-commands`) is done: `tune`/`simulate` (drive a real MRFT run against
either `OpcDaDriver` or `SimulatorDriver`, persisting the full lifecycle through
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
by `safety-cancellation` below to actually reach an in-flight driver call rather than only
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
regenerate-and-diff pattern in the repo, later reused by `docs-generated-cli` for the CLI
reference/man pages/completions/config schema. Every fallible route's error responses
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
mode) routing shell with an `AppLayout` (nav + the relocated health indicator), a Templates
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
and the simulator driver actually requiring five fields instead of the one originally
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
`server-windows-service` (the last item in Phase 7) is now implemented: a platform-neutral
`ServiceDefinition`/`ServiceLifecycle` in the new `crates/bhtune-server/src/service.rs`, a
`#[cfg(target_os = "windows")]` module wrapping the `windows-service` crate for real SCM
`install`/`uninstall`/`start`/`stop`/`status`, and — since `bhtune-server` (unlike the
Windows-only `opcda-bridge-gateway` it borrows this pattern from) is genuinely cross-platform
— real, informative, non-panicking stub functions on every other OS that explain the actual
platform equivalent (systemd on Linux, launchd on macOS) and point at the new packaging
files instead of silently doing nothing. `crates/bhtune-server/src/cli.rs` gained the five
subcommands plus a global `--config <path>` flag, captured into the service's own registered
launch arguments at install time so a service-launched process always resolves the same
config file regardless of which account the SCM runs it as (a real gotcha — see the
installation guide's callout). `main.rs` is now a thin platform-split dispatcher: a
synchronous `fn main()` on Windows tries SCM dispatch first, falling back to building its own
Tokio runtime for interactive use when run outside the SCM; every other platform still runs
the async server directly, unchanged. New `packaging/systemd/bhtune-server.service` (validated
with `systemd-analyze verify`) and `packaging/launchd/com.bytehound-labs.bhtune-server.plist`
(validated with Python's `plistlib`) supply the Linux/macOS equivalents. This Linux sandbox
still cannot compile `#[cfg(windows)]` code directly (`libsqlite3-sys` needs an
`x86_64-w64-mingw32-gcc` cross-compiler that isn't installed and can't be — no passwordless
sudo to install `mingw-w64`), so the Windows-specific SCM glue was verified by careful
line-by-line comparison against the already-CI-proven `opcda-bridge-gateway` reference
implementation and against the `windows-service` 0.8 API surface on docs.rs, plus the real
`windows-latest` CI job, rather than compiled locally — and then manually verified against a
live Service Control Manager on the `hp` Windows host, exercising every subcommand against
a real, freshly-cloned build: `install` (confirmed via `sc qc` — exact expected
`SERVICE_NAME`/`DISPLAY_NAME`/`AUTO_START`/`LocalSystem`, and, with `--config <path>`, the
path correctly baked into `BINARY_PATH_NAME`), `start` (confirmed via `sc query` showing
`RUNNING`, the process actually listening in the `Services` session, and a real HTTP
`/api/health` request succeeding), `stop` (clean `WIN32_EXIT_CODE 0`, process actually gone
from `tasklist`), and `uninstall` (service fully deregistered, `status` correctly returning
to the pre-install "does not exist" error). Also confirmed the interactive/foreground
fallback (`is_run_outside_scm`) genuinely serves requests when run outside the SCM, and that
a `--config` pointing at a custom `db`/`log.dir` is actually honored by a `LocalSystem`-run
service rather than falling back to that account's own profile directory — the exact gotcha
the installation guide documents a mitigation for. `protoc` turned out to be a previously
undocumented build prerequisite on Windows (transitively via `opcda-bridge-proto`'s gRPC
codegen; `choco install protoc` resolves it) and has been added to the installation guide.
No code defects were found; two apparent anomalies during testing (an interactive run
started via `cmd /c start /b` over SSH leaving no process behind, and a log file that `dir`
reported as 0 bytes while running) were both root-caused to test-methodology artifacts, not
real bugs — `start /b` doesn't survive the invoking SSH channel closing, and `dir` shows a
stale cached size for a file another process still has open for writing (`type` confirmed
the real-time content was correct and matched Linux's own output exactly). This item is now
fully closed out. Phase 8's
`e2e-simulator` is now done: a genuine, real-subprocess end-to-end test
(`crates/bhtune-cli/tests/e2e_simulator.rs`) that runs `bhtune tune` against the simulator
driver across a small process/controller-type matrix and asserts the _calculated_ PID
results, not just row presence — a gap no earlier test closed (see "Correctness-critical
design details" below, item 2, for the real `bhtune-core` bug this test caught and fixed
in the process: the MRFT oscillation period silently lost sub-second precision by default,
zeroing `ti_minutes`/`td_minutes` even for PI/PID). `e2e-playwright` is also done: a
Playwright suite (`frontend/e2e/`) drives a full tune through the real, built React SPA
served by a real `bhtune-server` binary (debug profile, which serves `frontend/dist/` live
off disk rather than needing a re-embed step — see `server-embed-spa`'s `rust-embed`
feature gating) running the in-process simulator driver, with no mocked HTTP layer and no
Vite dev server involved. `smoke.spec.ts` covers the app shell, the health indicator reaching a
real driver, the seeded built-in template list, and header nav; `tune.spec.ts` drives
`/runs/new` with the same millisecond-scale simulator parameters `e2e_simulator.rs` uses and
asserts the _rendered_ Kp/Ti/Td values are sane and correctly ordered (not just that the
page didn't crash), plus a second test cancelling an in-flight run. The server now tracks
multiple tune tasks independently, so separate browser requests can start concurrently;
only post-hoc PID writes/reverts remain mutually exclusive with tunes. A third TypeScript
project (`tsconfig.e2e.json`, referenced from `tsconfig.json` alongside the
existing `tsconfig.app.json`/`tsconfig.node.json`) wires `e2e/`/`playwright.config.ts` into
the existing `tsc -b`/`pnpm run build` gate, so the suite's own source is genuinely
typechecked in CI, not merely executed. A new `.github/workflows/e2e.yml` job builds a debug
`bhtune-server`, builds the frontend, installs Chromium via `playwright install
--with-deps`, and runs the suite, uploading the Playwright HTML report as a CI artifact on
failure. This workflow's first real run caught a genuine, previously-undiscovered production
bug on its very first execution — `bhtune-server`'s new `build.rs` now fixes a `rust-embed`
compile-time build-order trap where compiling the crate before `frontend/dist/` exists
permanently breaks asset serving for that build, regardless of build order afterward; see
"`server-embed-spa`: embedding the built SPA into the binary" below for the full mechanism
and fix. `build-matrix` (the last item in Phase 8) is now done too:
`.github/workflows/release.yml` builds and packages the `bhtune`+`bhtune-server` binaries
for Linux/macOS/Windows, building the frontend first so the release build's `rust-embed`
step captures real SPA assets, via `taiki-e/create-gh-release-action` +
`taiki-e/upload-rust-binary-action` — opcda-bridge's own already-proven tooling, adopted
in place of the originally-planned `cargo-dist` once that sibling project's simpler setup
was reviewed — see "`build-matrix`: the release binary matrix" below for the full design,
including why this does not by itself mean `release-v1` should happen yet.
`driver-replay` is now done too, completing Phase 4: `ReplayDriver`
(`crates/bhtune-driver/src/replay.rs`) feeds a recorded `(time, pv)` trace through the real
`Driver` trait — a validation-only construct, deliberately not wired into
`bhtune tune --driver` (there is no CLI-selectable replay driver; see "Replay driver
reference" below), proving the trait abstraction itself introduces no bugs on top of the
already-proven-correct `MrftEngine`. `from_fixture_json` parses the same golden-fixture JSON
`core-replay-harness` consumes, tolerating every field it doesn't need via serde's
default unknown-field-ignoring behavior rather than duplicating that fixture schema in this
crate. Its end-to-end test replays the real `tests/golden/fixtures/flow_pi_direct.json`
trace through a genuine `MrftEngine` via the `Driver` trait and reaches the same
aggressive-response PB≈157.7088 result `core-replay-harness` already validates at the
pure-engine level — see "Replay driver reference (`driver-replay`)" below for the full
design, including why its `TagValue.timestamp` is the one deliberate, narrow exception to
the rule that a driver's timestamp must never become the tuning engine's own tick time.
Phase 9's two front-loaded,
run-now items are also done: `docs-contract` (see
"Documentation contract" above) and `docs-copilot-hook` — a paired `sessionStart`/`sessionEnd`
Copilot CLI hook (`.github/hooks/docs-drift.json`) that warns when a session changed
`crates/**` without touching any documentation surface, covering both a session's already-
committed-and-pushed changes and anything still uncommitted (see `.github/hooks/README.md`
for why it's a pair, not a single hook). `docs-generated-cli` is also done: a new
`crates/bhtune-cli/examples/gen_docs.rs` regenerates the full CLI reference
(`docs/reference/cli.md`, `clap-markdown`), one git/cargo-style man page per command and
subcommand (`man/*.1`, `clap_mangen`, recursing `Command::get_subcommands()` rather than a
single flat page), bash/zsh/fish completions (`completions/`, `clap_complete`), and a JSON
Schema reference for both `bhtune.toml` and the DCS/PLC template catalog shape
(`docs/reference/config.md`, `schemars`) — reusing `gen_openapi`'s exact regenerate-and-diff
idiom, now gated in CI by a new `checks.yml` step. See "`docs-generated-cli`: generating the
CLI reference, man pages, completions, and config schema" below for the full design.
`docs-readme` is also done: a getting-started guide (installation, CLI/web GUI quickstarts,
MRFT concepts, safety) under `docs/getting-started/`+`docs/guides/`, linked from the README.
`docs-site-scaffold` is also done: a Docusaurus 3 site (`bhtune-website`, `website/`)
publishing that same `docs/` folder as a browsable, searchable site, with its own CI job.
`docs-site-deploy` is also done: that site is live at
[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/) — see
"`docs-site-scaffold`: the Docusaurus documentation site" below. `docs-roadmap` is also done:
[`docs/roadmap.md`](docs/roadmap.md) covers the fuller reasoning behind each item in the
README's roadmap section. `docs-api-rustdoc` is also done: `cargo doc` output for all six
crate/binary targets is published under `/api/` on that same site, indexed from a
hand-written `docs/reference/api.md` — see "`docs-api-rustdoc`: publishing the Rust API
reference" below.
Phase 10's `history-retention` is now done: age-based deletion of `tune_runs` (and their
cascaded samples/results/write-back audit rows) older than a configurable number of days,
off by default (retain forever). `resolve_retention_days` (`bhtune-cli`'s `config.rs`)
resolves the policy through the usual `CLI --retention-days > BHTUNE_RETENTION_DAYS env >
retention_days in bhtune.toml > (no default)` precedence, and a new shared
`crate::retention` module (`cutoff_for`, `sweep_retention`) is the single place that turns
"N days" into an actual delete — used identically by `db::open`'s startup sweep (both
binaries), `bhtune-server`'s periodic 24-hour ticker while it keeps running, and `history
prune`'s real-deletion path, so a preview and an actual sweep can never disagree about which
runs are in scope. The startup sweep is fatal on failure (propagates via `?`, matching the
existing template-seeding precedent — a one-shot CLI/server-startup invocation should fail
fast rather than silently proceed against a possibly-broken database); the server's
periodic sweep instead logs a warning and continues, since a background maintenance hiccup
must never crash a long-running server out from under an in-flight HTTP connection or tune.
`history-cli`'s remaining scope is also done: `bhtune history prune` (`--older-than-days` to
override the configured policy for one invocation, required if no policy is configured at
all; `--dry-run` to report a count and cutoff without deleting anything, via
`TuneRunRow::count` against the identical filter shape the real sweep uses; `--output json`)
completes the four-subcommand `history` surface (`list`/`show`/`revert`/`prune`) started
under `cli-commands`/`safety-writeback-rollback`. `history-explorer-ui` is now done, closing
out Phase 10: the filterable/sortable run list, full run detail, and the PV/MV trend chart
were already in place from `frontend-screens`/`frontend-live-stream`; the remaining piece —
export and delete actions on the run detail screen — is now shipped too. `GET
/api/runs/{id}/export?format=csv|json` (`export_run`, reusing `bhtune-cli`'s own
`samples_to_bytes`, so the HTTP and CLI export paths can never disagree on the CSV/JSON
shape) and `DELETE /api/runs/{id}` (`delete_run`, cascading through `tune_samples`/
`tune_results`/`tune_writes` via the schema's existing `ON DELETE CASCADE`) are both new
`bhtune-server` routes; the frontend adds Export CSV/Export JSON download links (plain
`<a download>` tags, deliberately not a fetch-then-blob dance, so the browser's native
download handling does the work) and a Delete run button (`window.confirm` then navigate
back to the run list) to `RunDetailPage`. `delete_run`'s conflict check deliberately reads
the run's own DB `outcome` column rather than `ActiveRun`'s in-memory registry: `drive()`
persists a run's terminal outcome to the database _before_ returning, and `ActiveRun::release`
only runs strictly after `drive()` returns (see `routes::runs::start_run`), so there is a real
— if brief — window where a run is already durably `completed` but its registry entry has not
been released yet. Checking the DB's own outcome instead of the best-effort in-memory tracker
closes that race outright, with the registry reserved for live tune/write coordination and the
database outcome remaining authoritative for deletion — found and fixed by writing a real
Playwright E2E test for delete
(`tune.spec.ts`) that first failed against the naive `ActiveRun`-based guard. That same
Playwright run also surfaced a second, unrelated pre-existing bug in TanStack Query's
setup: `queryClient` had no `retry` policy at all, so a genuine 404 (like the deleted run's
own detail page) retried 3 times with exponential backoff before the UI's error banner ever
appeared, leaving the page stuck on "Loading run…" for several seconds. Fixed with a new
`ApiError` class (`frontend/src/api/errors.ts`) carrying the HTTP status code, threaded
through every `queryFn`/`mutationFn` in `runs.ts`/`templates.ts`/`AppLayout.tsx`, and a
`queryClient` default `retry` that skips retrying any 4xx response — permanent failures —
while keeping the default 3-retry behavior for genuinely transient ones (network drops,
5xx). `pkg-docker` is now done too: a multi-stage `Dockerfile` (pnpm frontend build → `cargo
build --release` for `bhtune-cli`+`bhtune-server` → a slim `debian:bookworm-slim` runtime)
builds both binaries into a single ~110 MB image, published to
`ghcr.io/bytehound-labs/bhtune` by a new `.github/workflows/docker-publish.yml` — tagged
`edge` on every push to `main`, additionally under the version and `latest` once a release
tag exists, and build-only (no push, no registry credentials touched) on every PR, so a
broken Dockerfile fails the PR that broke it rather than surfacing at release time. See
"`pkg-docker`: the Docker image" below for the full design, including the two build-time-only
system dependencies the Rust builder stage needs beyond `protoc` (`build-essential`, for
`bhtune-db`'s bundled-SQLite `cc` compile step — easy to miss since the plain, non-`slim`
`rust` image includes it by default and only `slim` doesn't) and the container-specific
`BHTUNE_BIND=0.0.0.0:8787` default's rationale.
`trace-fixtures`/`core-replay-harness` (Phase 0/3) are now also done: `flow_pi_direct` (the
first real trace captured on the `hp` Windows host) is normalized by
`scripts/convert_golden_trace.py` and replayed tick-by-tick through a real `MrftEngine` plus
`calculate_all` in `crates/bhtune-core/tests/golden_replay.rs` — the Rust port is now proven,
not just argued, to reproduce the legacy C# app's tuning behavior exactly. See "Validation
strategy: golden-master replay" below for the two genuine legacy-CSV-logger precision limits
this surfaced (both independently confirmed against the real C# source, not worked around by
loosening tolerances blindly). `capture-traces` is deliberately closed at this one trace (no
more are planned) and `cleanup-golden-traces`/`e2e-golden-ci`/`core-bug-register` are also done
— see "Phases and todos" below for what's next.

`pkg-evaluate-others` is now done too: `.deb` and `.rpm` packages (via `cargo-deb` and
`cargo-generate-rpm`, sharing the same asset set as the Docker image), `cargo-binstall`
metadata on `bhtune-cli`, and a prepared-but-inert Homebrew formula awaiting a real tap
repo and release checksums. `release.yml` gained a new `package-deb-rpm` job, deliberately
separate from the existing per-platform `build` matrix rather than extra steps on its Linux
leg, because `upload-rust-binary-action` always builds with an explicit `--target`, leaving
binaries in a target-triple subdirectory the packaging asset paths don't expect. Both new
package formats, and the job itself, were validated by actually dispatching `release.yml`
in GitHub Actions rather than trusting local testing alone — which caught a real bug
(`cargo generate-rpm` doesn't create its own missing output directory, unlike `cargo-deb`)
invisible to local runs because the local test directory always happened to pre-exist. See
"`pkg-evaluate-others`: the remaining distribution channels" below for the full design,
including a `-p` flag that is a path for one tool and a crate name for the other despite
identical `--help` wording, and why winget stays out of scope for now.

Phase 7.5 (pre-v1 UX and terminology hardening) is under way. `rename-driver` is done: the
`backend` → `driver` rename landed across the entire workspace (crate, trait, error types,
every concrete driver, the `--driver` CLI flag, the HTTP/OpenAPI `driver` field, and the
frontend), with migration `0001` edited in place — the crate map and every other section
below already use `driver`/`bhtune-driver` terminology throughout as a result. Its second
todo, `db-run-request-snapshot`, is now also done. `tune_runs` gained
`opc_server`/`bridge_host` (flat, nullable columns — `NULL`/`NULL` for a non-opcda run) and
`request_json` (the complete run request exactly as submitted, before any config-driven
defaulting), added to the same in-place `0001` migration `rename-driver` had just edited.
`TuneRunRow::record_connection` populates all three via a follow-up `UPDATE` right after
`start()`, matching `record_initial_readings`/`record_allow_uncertain_quality`'s existing
precedent, and `bhtune-cli`'s `prepare()` calls it immediately, before any driver I/O. This
closes a real latent safety bug in `bhtune history revert`, which used to re-resolve the OPC
server/bridge host from `--server`/`--bridge-host`/config _at revert time_ — silently able
to write a run's old PID constants into a different plant's controller than the one it
actually tuned. `resolve_revert_connection` now always trusts the run's own recorded
connection, treating an explicit flag as a cross-check (a hard error on contradiction) rather
than an override — see the `bhtune history revert <run-id>` entry under "Live-plant safety
hardening" below for the full design and the three new tests
(`revert_errors_when_the_run_has_no_recorded_connection`,
`revert_errors_when_an_explicit_server_flag_contradicts_the_recorded_one`,
`revert_errors_when_an_explicit_bridge_host_flag_contradicts_the_recorded_one`).
`bhtune-server`'s `/api/runs` list gained matching `opc_server`/`bridge_host` query filters,
and the run-detail response gained the same two fields (deliberately _not_ the list/summary
rows, matching `history list`'s table having no connection column either); `bhtune-cli`'s
`history show` gained a "Connection:" line in `Table` mode and the same two fields in its
`RunDetailJson`, so the CLI and HTTP API stay in JSON-shape parity. Unblocks
`api-post-run-write` and `ui-prefill-last-run`, both of which need a stored, trustworthy
connection/request to act on. Its own next todo, `ui-simulator-greyout`, is also done:
`components/ui.tsx`'s `NumberField`/`SelectField`/`CheckboxField` gained a `disabled` prop,
matching `TextField`'s pre-existing `disabled:cursor-not-allowed disabled:opacity-50`
pattern, and `NewRunPage.tsx` now disables — rather than hides — every field the simulator
driver genuinely ignores: the OPC DA server ProgID and bridge host (previously conditionally
hidden outright; now always rendered, so switching drivers no longer reflows the form), the
tag name (the simulator hardcodes its own PV/MV tags), the write-back level and its
confirmation checkbox (the simulator has no PID constant tags to write to), the
`--allow-uncertain-quality` checkbox (the simulator always reports `Good`), and the
op/restore timeouts (no out-of-process I/O to time out) — each with a one-line hint
explaining why. The template,
PV/MV ranges, controller direction, process type, controller type, relay amplitude, cycles,
poll interval, run timeout, and MRFT delay padding fields all stay enabled, since they are
genuinely used regardless of driver — the template's unit conversions apply to every run,
and the simulator's lack of its own range/direction tags makes those four fields _more_
required, not less. `buildRequest()`'s tag-name check is skipped whenever the field is
disabled, matching the rule that a disabled field must be excluded from client-side
validation. Manually verified against a real running server via browser automation (not
just typechecked): confirmed the disabled state, hint text, and enabled/disabled field list
match exactly in both driver modes, and that the existing Playwright E2E suite (which never
exercises the opcda-only fields) still passes unmodified.

Phase 7.5's next todo, `ui-friendly-process-names`, is also done. Every enum that leaked into
the UI as its raw snake_case wire value (`process_type`, `controller_type`,
`controller_direction`, `response_level`, `driver`, `outcome`) now renders a friendly display
label instead, via new `frontend/src/lib/enumLabels.ts` — six `Record<EnumType, string>` maps
(`PROCESS_TYPE_LABELS`, `CONTROLLER_TYPE_LABELS`, `DIRECTION_LABELS`,
`RESPONSE_LEVEL_LABELS`, `DRIVER_LABELS`, `OUTCOME_LABELS`) typed directly against the
generated OpenAPI schema types, so a new enum member fails `tsc` rather than silently falling
back to raw text. `ProcessType`/`ControllerType`/`ControllerDirection`/`ResponseLevel` reuse
the legacy app's own dropdown/results-tab strings verbatim (e.g. `pressure_line` → "Pressure
(Line)", `temperature_heat_exchange` → "Temperature (Heat Exchange)"), since control
engineers already know that vocabulary; `TuneDriver`/`TuneOutcome` have no legacy precedent,
so they use plain title case ("OPC DA", "Simulator", "Replay"; "Running", "Completed",
"Failed", "Aborted").
`components/ui.tsx`'s `SelectField` gained an optional `displayLabel` prop and was
re-parameterized from one generic to two (`SelectField<Value extends string, Option extends
Value = Value>`) — `Value` types the field's own state (which may include the `""`
placeholder sentinel for an optional field), `Option` types the rendered choices and
`displayLabel`'s parameter, and letting them differ is what lets a label map keyed only on
real enum members type-check against an optional-enum field without also having to cover the
empty sentinel. `NewRunPage.tsx`, `RunListPage.tsx`, and `RunDetailPage.tsx` all wire the
relevant label maps into every `SelectField`/filter/table-cell/badge that previously rendered
a raw enum value (the Template dropdown's free-form names are deliberately left alone — no
label map applies to them); the two `capitalize` CSS-class workarounds on the response-level
table cells are removed now that the label text itself is already properly cased. Fixed a
resulting Playwright ambiguity in `tune.spec.ts`: the outcome badge's text changed from raw
lowercase (`"completed"`) to the capitalized label (`"Completed"`), which collided with
`RunDetailPage`'s pre-existing, unrelated "Completed" field _label_ (the completion-timestamp
field's `<dt>`) once both rendered the identical string — resolved with a shared `outcomeBadge()`
helper that scopes the locator to `<dd>` (value) elements only, since that field's own `<dd>`
holds a formatted timestamp, never the literal word "Completed".

Phase 7.5's next todo, `ui-tune-nav`, is also done. `layout/AppLayout.tsx` gained a "Tune"
header nav item pointing at `/runs/new`, placed first (ahead of Templates and History), and
`App.tsx`'s index route now redirects to `/runs/new` instead of `/templates` — starting a
tune is the app's default landing page. Adding a nav item for a route that is a path
segment of History's own route (`/runs/new` under `/runs`) surfaced a real, if minor, nav
bug: `NavLink`'s default active-matching is prefix-based, so History's `to="/runs"` would
have highlighted alongside Tune on `/runs/new` with no way to tell the two apart. Fixed with
an explicit `isHistoryActive` override computed from `useLocation()` (active for `/runs` and
any `/runs/:id` detail page, deliberately excluding `/runs/new`), verified with a throwaway
browser-automation script confirming exactly one nav item is ever highlighted, across
`/runs/new`, `/templates`, `/runs`, and a real `/runs/:id` detail page. Updated
`e2e/smoke.spec.ts`'s landing-page assertion and split its combined
landing/templates-list test into two, since the seeded-templates list is no longer visible
on first load.

Phase 7.5's `api-post-run-write` is also done. `POST /api/runs/{id}/write` (body: a
`response_level`) and `POST /api/runs/{id}/revert` (no body) let PID constants be written to
a live loop _after_ a run finishes, rather than only via the CLI's pre-run `--write-pid`
flow — an engineer can compare Sluggish/Moderate/Aggressive on the run detail screen and act
on whichever one looks right. Neither endpoint reimplements the write path:
`read_previous_pid_values`/`write_and_verify_pid_value` (already shared between the in-run
write and `bhtune history revert`) are promoted from `pub(crate)` to `pub`, and a new shared
orchestrating function, `write_pid_values`, wraps pre-read → write-and-verify-each-constant →
roll-back-on-partial-failure → audit-row-insert exactly once, called by both the CLI's
existing write-back path and these two new HTTP handlers — the same reuse pattern
`server-start-tune-api` established for `prepare()`/`drive()`. `require_writable_run`
enforces run eligibility in a fixed order (still-running → wrong driver → missing PID
constant tags → no recorded connection) before either handler does anything else, and
`revert_run` additionally requires the most recent `write`-kind row to have recorded
pre-write values (a write whose own pre-read failed records `previous = None` and cannot be
reverted from). Both handlers take the single `ActiveRun` slot for the duration of the
operation — a post-hoc write strokes the same live loop a tune does, so the two must never
overlap — via a new `ActiveRun::reserve`/`release` pair alongside a new `ActiveRunKind`
distinguishing a short, directly-awaited "exclusive" reservation (a write/revert) from a
spawned tune task (`start`'s existing kind, now `ActiveRunKind::Task`); `cancel`/
`cancel_and_wait` handle both kinds correctly (an exclusive reservation has nothing to
cancel or wait for — axum's own graceful-shutdown request drain already covers it). A
physical write/revert failure is reported as an ordinary `200` with the failure visible in
the returned `writes[]` audit row, never a `4xx`/`5xx` — matching how a failed write already
behaved during an in-run write-back, and confirmed directly by a dedicated test. Test
coverage (12 new tests landed with the implementation, `bhtune-server` unit tests 93 → 105;
3 more added afterward while auditing coverage, → 108: two close real branches the initial
12 didn't reach — `require_writable_run`'s missing-PID-tags check and `revert_run`'s
missing-previous-values check — and a third exercises a failed pre-read, symmetric with the
existing failed-write test) uses a third, crate-local minimal mock gRPC `Bridge` service
(`routes::runs::tests::mock_bridge`), deliberately mirroring — not sharing — the same
pattern already used by `bhtune-cli::test_support` and `driver-opcda`'s own `smoke_tests`,
since three internal, already-thorough consumers didn't justify a shared test-support crate.
Both new routes were initially missing from `openapi.rs`'s explicit `paths(...)`/
`components(schemas(...))` lists — that module's own doc comment warns this fails silently
(the route works; it's just absent from the spec) rather than loudly, and this was exactly
the omission it warned about; fixed before `openapi.json`/`frontend/src/api/schema.d.ts`
were regenerated. Unblocks `ui-post-run-write` (the Write/Revert buttons) and completes the
connection/audit-trail groundwork `ui-prefill-last-run` also depends on.

Phase 7.5's `ui-post-run-write` is also done: the run detail screen now has a "Write" button
per response-level row in the "Calculated results" table and a "Revert" button on the
newest successful row of the existing "Write-back audit" table, both calling the two new
`api-post-run-write` endpoints via new `useWriteRun`/`useRevertRun` hooks (`frontend/src/
api/runs.ts`) that seed the `run` query cache from the returned `RunDetailResponse` on
success, matching `useStartRun`'s existing cache-seeding pattern rather than invalidating
and refetching. A client-side `writeEligibility()` helper mirrors `require_writable_run`'s
checks exactly (not-still-running, `opcda` driver, `pid_constant_tags` present, `opc_server`/
`bridge_host` present) so a disabled button always carries a `title` tooltip and a persistent
"Write/revert disabled: ..." note explaining why, rather than a mystery grey control — this
needed a small, deliberate addition to the shared `Button` component (`ui.tsx`), which had no
`title` prop before. Both actions are gated behind a `window.confirm` naming the loop, the
exact tag names, and the exact P/I/D values that will be written or restored, sourced
directly from the same row being acted on. The Revert button's visibility is intentionally
narrow: it renders only on the single newest `writes[]` row, and only when that row is a
successful write (`kind === "write" && success`) — once superseded by a later write or
revert, the button disappears, matching the server's own "revert always targets the most
recent write" rule. Verified with real browser automation (`@playwright/test`'s `chromium`
launcher driven from standalone Node scripts, since the `chrome-devtools-*` MCP tools
timed out repeatedly in this environment) against a real running `bhtune-server` and Vite
dev server: an eligible opcda run (hand-crafted via direct SQLite edits, since no real OPC
DA gateway was available) shows all three Write buttons enabled with correct confirm-dialog
text, and clicking Write against an unreachable bridge host correctly surfaces the
connection failure as an `ErrorBanner` — confirming the deliberate `api-post-run-write`
distinction between a driver **connection** failure (HTTP 400, no audit row) and a **write**
failure after a successful connection (HTTP 200, a `success: false` audit row rendered by
the existing Write-back audit table); an ineligible simulator run shows all three Write
buttons disabled with the correct tooltip and note; and the Revert button was confirmed to
appear on a seeded successful write row and correctly disappear once a newer row supersedes
it. Completes Phase 7.5's GUI gap list alongside `ui-simulator-greyout`/
`ui-friendly-process-names`/`ui-tune-nav`; remaining Phase 7.5 work is
`driver-list-servers`, `api-opc-browse`, `ui-opc-browser`, and `phase75-docs`.

Phase 7.5's `ui-prefill-last-run` is also done. `GET /api/runs/last-request`
(`routes/history.rs`) returns the newest run's own stored request as a `StartRunRequest`, or
`null` on a fresh install with no runs yet — deliberately a graceful `null` rather than a
`500` if that row's `request_json` fails to parse (a new `parse_stored_request` helper logs a
`tracing::warn!` and returns `None` instead of erroring), since a malformed historical row
is reachable in practice (a hand-poked SQLite row, or a `seed_one_run`-style test fixture)
even though the real `prepare()`-driven CLI/HTTP paths can never produce one, and failing the
entire New Run page over one bad row's data quality would be worse than treating it as
"nothing to prefill from". The same graceful helper now also powers a new
`RunDetailResponse.original_request` field (_this specific_ run's own request, independent of
whether it's the newest one), added specifically to power "Duplicate this run" without a
second network round-trip. On the frontend, `NewRunPage.tsx` seeds its form once from
`useLastRunRequest()` on a plain visit (a `formFromRequest` inverse of the existing
`buildRequest`, informed by tracing `bhtune-cli`'s `RequestSnapshot`: fields with a CLI/serde
default are always concrete in a real stored request and are copied straight across, while
the genuinely-optional fields — cycles, ranges, direction, connection overrides, name — are
shown _blank_ when absent rather than substituting today's hardcoded default, since an
absence there specifically means "the engineer relied on a default last time"), gains a
"Reset to defaults" button that restores the hardcoded defaults, and shows an explanatory
note ("Prefilled from the most
recent run's settings" / "Prefilled from run #N's settings") whenever the form isn't showing
blank defaults. `RunDetailPage.tsx` gained a "Duplicate this run" button (disabled with a
`title` when the run has no usable `original_request`) that navigates to `/runs/new` with a
new exported `DuplicateRunState` in router state, which the New Run page's lazy `useState`
initializer seeds from synchronously — taking priority over the async last-run prefill,
which must not fire in that case. This historical prefill is now a fallback only: the separate
New Tune draft store takes precedence and is described below.

Manual browser verification (Playwright driven from a standalone throwaway spec, per the
same `chrome-devtools-*` MCP timeout noted under `ui-post-run-write` above) caught a real
race-condition bug before it shipped: the pre-existing "auto-select the first available
template once the list loads" effect and the new last-run-prefill effect can both resolve in
the _same_ React batch (when `templates` and `last-request` both finish fetching together),
landing both effects in the same commit. Both then closed over that render's _stale_,
pre-update `form.template` (still `""`), so the template-defaulting effect's `setForm` call —
queued _after_ the prefill's own `setForm` in the same commit, since it's declared later in
the component — clobbered the just-prefilled template with the alphabetically-first one the
instant afterward. Fixed by moving the "is there already a template?" check from the effect's
own closure into the _functional_ `setForm` updater passed to it, so the check runs at
_application_ time against whatever the prior queued update (the prefill) already produced,
rather than against a stale snapshot — a general pattern worth remembering for any pair of
effects in this app that read-then-conditionally-write the same piece of state from two
independent async sources. Re-verified 4 consecutive green runs of the full scenario (fresh
defaults → run a tune → reload shows the prefill note and correct values → "Reset to defaults"
resets to defaults → "Duplicate this run" from the run detail page prefills from that
specific run) after the fix, with no flakiness. Completes every Phase 7.5 GUI/API todo except
`driver-list-servers`/`api-opc-browse`/`ui-opc-browser` (the OPC browser trio) and the
`phase75-docs` wrap-up. The later draft-persistence work is complete: it stores all editable
fields except Notes in SQLite and restores them across reloads. The frontend treats 400/404/405
responses from a server without the draft route as an empty draft during upgrades, while
unexpected storage failures remain visible.

Phase 7.5's `driver-list-servers` is also done: a new `bhtune_driver::opcda::list_opcda_servers
(bridge_host)` free function, re-exported at the crate root. It is deliberately **not** a
`Driver`/`OpcDaDriver` method — server discovery is a _pre-connection_ operation (it needs
only a bridge host, not the OPC DA server ProgID that `OpcDaDriver::connect` requires, and
that discovery exists to help a caller find in the first place), so it connects for the one
`list_servers` RPC and drops the connection immediately afterward rather than reusing
`OpcDaDriver`'s held session. Note for anyone reading the wire calls: `opcda_bridge::
Client::list_servers` always sends `host: "localhost"`, i.e. it lists servers registered on
_the gateway's own_ machine, not on whatever machine bhtune itself runs on — exactly right
for this topology (the gateway runs next to the OPC DA server), and worth knowing so it's
never mistaken for a bug. On the CLI side, `bhtune opc servers [--bridge-host <HOST>]` fills
the one gap in the existing `opc read`/`write`/`browse` diagnostic family — it was previously
impossible to discover a server's ProgID from bhtune at all, forcing a round-trip to
`opcda-bridge-client`'s own CLI just to find out what to pass to `--server`. Both the shared
smoke-test mock gateway (`bhtune-driver::opcda`'s own `smoke_tests` module) and
`bhtune-cli`'s separate `test_support::MockBridgeService` gained a settable
`list_servers_response` field to cover this — the latter's mock previously hardcoded an
empty response with a comment noting `list_servers` was never actually exercised by any CLI
test, which is no longer true. 3 new tests in `bhtune-driver` (70 total) and 6 new tests in
`bhtune-cli` (313 total). `docs/reference/cli.md`, `man/bhtune-opc-servers.1`, and the three
shell completion files were regenerated via the existing `gen_docs` example and are clean
under CI's drift gate. Full quality gate green: `fmt`, `clippy -D warnings`,
`cargo test --workspace`, `deny check`, `machete`.

Phase 7.5's `api-opc-browse` is also done: three new read-only `bhtune-server` routes —
`GET /api/opc/servers`, `GET /api/opc/browse` (one tree level at a time, matching
`Driver::browse`'s own contract), and `GET /api/opc/read` (single tag: value, quality,
timestamp) — backing the not-yet-built GUI server dropdown/tag-tree browser/"Test
connection" button (`ui-opc-browser`, next) independently of ever having run a tune. None
of the three touches `AppState::active_run`: a diagnostic browse/read must not be blocked
by, or block, an in-flight tune. Every OPC DA call is wrapped in a new `with_timeout`
helper enforcing a 30-second deadline (`OPC_QUERY_TIMEOUT_SECS`, matching `bhtune-cli`'s own
`default_op_or_restore_timeout_secs()`) via `tokio::time::timeout`, since
`opcda_bridge::Client::connect` has no connect timeout of its own and a firewalled/
black-holed gateway would otherwise hang a request indefinitely. `GET /api/opc/read`'s
`quality` field reuses `bhtune_db::models::SampleQuality` — already exposed directly over
HTTP by `GET /api/runs/{id}`'s `SampleResponse::pv_quality` — via a `sample_quality_from_driver`
conversion function promoted from `bhtune-cli::commands::tune` (`private fn` to `pub fn`,
its second consumer now) rather than inventing a third quality DTO enum.
`OpcReadResponse::timestamp` is `Option<DateTime<Utc>>` and is always `null` today: the OPC
DA driver never converts the gateway's local, offset-less last-change-time string into a
trustworthy UTC instant (see `driver-opcda`'s existing design in the Status section above) —
kept as a field anyway so a future driver or bridge-protocol revision that _can_ supply a
trustworthy instant doesn't need an API shape change to start populating it. Query-parameter
resolution (`bridge_host`/`opc_server`) reuses `bhtune_cli::config::resolve_bridge_host`/
`resolve_server` verbatim, matching every other OPC-touching route. The mock gRPC `Bridge`
test double that used to live privately inside `routes/runs.rs` (hardcoded-empty
`list_servers`/`browse`) was promoted into `test_support::mock_bridge` and extended with
configurable `list_servers_response`/`browse_responses` fields (mirroring `bhtune-cli`'s own
separate, richer mock — crates don't share test infrastructure in this codebase, but a
second consumer _within_ a crate does warrant promoting a private test helper), so
`routes::runs`'s 29 existing tests and the new `routes::opc` tests now share one mock rather
than each hand-rolling it. 17 new tests in `bhtune-server` (130 total, up from 113): handler-
level tests for all three routes (happy path, empty result, connect-failure/unreachable
host, missing-server-config, missing-tag, a query-param bridge-host override, `Uncertain`/
`Bad` quality passthrough, and a driver-misbehaves-with-zero-values 500 case) plus three
direct unit tests of `with_timeout` itself (success passthrough, driver-error mapping, and —
via `#[tokio::test(start_paused = true)]` + `std::future::pending()`, the same technique
`bhtune-cli`'s own `bounded_driver_call` tests already use — the 30-second elapsed-deadline
branch, proven without an actual 30-second wait). `openapi.json` and
`frontend/src/api/schema.d.ts` were regenerated (three new paths, four new schemas —
`OpcServersResponse`/`OpcBrowseResponse`/`OpcTagNodeResponse`/`OpcReadResponse` — and a new
`"opc"` OpenAPI tag). Full quality gate green: `fmt`, `clippy -D warnings`,
`cargo test --workspace` (770 tests), `deny check`, `machete`. Remaining Phase 7.5 work:
`phase75-docs`.

Phase 7.5's `ui-opc-browser` is also done — the last GUI/API todo of the eleven, leaving
only the `phase75-docs` wrap-up. Two new pieces wire `api-opc-browse`'s three routes into
the New Run form, both rendered only when `form.driver === "opcda"`: `OpcServerDiscovery`
(`components/OpcServerDiscovery.tsx`), a "Browse servers" button next to the ProgID field
that opens an on-demand modal rather than rendering every discovered server inline (server
discovery is itself a live network call), with clickable ProgIDs that fill the field directly,
or a clean error/empty state; and `OpcTagBrowserModal`
(`components/OpcTagBrowserModal.tsx`), opened by a new "Browse tags" button next to the Tag
name field (disabled, with an explanatory `title`, until a ProgID is entered) — a
lazily-expanding tree (one `GET /api/opc/browse` level per node, cached per server/path so
re-expanding an already-open node doesn't refetch) whose leaf selection renders a **derived
tag set preview**: the exact tags the active template would derive from that selection, via
a new pure `frontend/src/lib/opcTags.ts::deriveTag` — a client-side mirror of
`bhtune_core::tags::derive_from_pv_tag`'s "replace everything after the last `.`/`!`/`/` with
the suffix" algorithm, used only for this preview; the server remains the actual source of
truth once a run starts — plus a "Test read" button (`GET /api/opc/read`, showing value and
quality) and "Select tag", which replaces the selected node's final component with the active
template's process-variable suffix before writing it into the Tag name field. Double-clicking
a leaf performs the same selection, while double-clicking a branch expands or collapses it.
The selection panel is rendered before a node is clicked, and the first loaded node is selected
automatically. Its detailed template replacement list is inside a native `details` element
that is collapsed by default.
This is deliberately not "strip the suffix and use the base name": since
`deriveTag`/`derive_from_pv_tag` both work by replacing everything after the last separator,
a full leaf tag (e.g. `FIC101.PV`) is already exactly the right input — the preview panel and
a real tune's tag derivation agree because they run the identical algorithm, which manual
verification confirmed end-to-end (below). Template changes reuse the same separator-aware
logic to replace a matching previous PV suffix while preserving the tag path. A new shared
`Modal` component (`components/
ui.tsx`) backs the tag browser and is reusable for future modals: closes on Escape, a
backdrop click, or an explicit close button.

Manually verified against a real running `bhtune-server` plus a temporary, deliberately
not-committed mock gRPC gateway — a path-aware fake `Bridge` service bound to
`127.0.0.1:7600`, since the crate's existing `smoke_tests::MockBridgeService` ignores
request contents and can't demonstrate real recursive tree expansion or a template-specific
derived-tag preview. Confirmed: server discovery returning real ProgIDs; recursive branch
expansion (`FIC101` → `FIC101.PV`/`.MODE`/`.OUT`); the derived tag preview rendering the
correct Allen-Bradley PlantPAx suffixes for a selected leaf; "Test read" showing a live
value and `Good` quality; and "Select tag" writing the selected tag back into the Tag name
field exactly as the preview showed — proving the client-side preview and the server's real
tag derivation agree, not merely that they're intended to. Also verified structurally before
the mock gateway existed: driver-switch show/hide of both new affordances, disabled states
with explanatory titles, and clean error rendering against an unreachable gateway. The mock
gateway and its harness were removed after verification and never reached a commit. No
standalone "Test connection" button was added elsewhere on the form — the modal's own "Test
read" already covers that need, and a second, redundant affordance would just be one more
thing to keep in sync. `pnpm run build`/`lint`/`format:check` all clean; the Rust workspace
(`fmt`, `clippy -D warnings`, `cargo test --workspace`, `deny check`, `machete`) is
unaffected and still fully green. Completes every Phase 7.5 GUI/API todo; only
`phase75-docs` remains.

The manual mock-gateway verification above is deliberately not permanent, but a lighter,
permanent regression test is: `frontend/e2e/opc-browser.spec.ts` (4 new Playwright tests, 11
total in the suite) exercises the OPC DA path against the suite's real, already-running
`bhtune-server` — with no gateway started at its default `localhost:7600` bridge host, every
action fails at the connection step, which resolves in single-digit milliseconds
(`ECONNREFUSED`, empirically confirmed, well inside `with_timeout`'s 30s budget) rather than
hanging. That failure is still real coverage no other spec touches: the driver-switch
visibility of "Browse servers"/"Browse tags", the "Browse tags" button's disabled-until-
a-ProgID-is-entered state, both buttons' real HTTP request wiring, the modal opening and
rendering a visible connection-error message rather than silently swallowing it, and the
modal closing via both its "Close" button and Escape. It does not attempt to re-prove the
populated-tree happy path — that stays the mock-gateway pass's job, since standing up a
second, permanent mock gRPC service just for this suite would cost more than it would
additionally prove.

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
  `opcda-bridge = "0.2"` pinned directly in `crates/bhtune-driver/Cargo.toml` — not promoted
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
      async fn browse(&self, path: &str) -> DriverResult<Vec<TagNode>>;
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
  currently a draft — not yet in force) records the rights needed to accept and maintain
  contributions, naming ByteHound Corp. as the entity. Still outstanding before it's binding: a
  legal review of the text, and wiring up a CLA-signing check (`cla-tooling`).
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
  (`pkg-docker`, done — see "`pkg-docker`: the Docker image" below) is a secondary channel for
  IT-managed Linux hosts, not the deployment path this decision was optimized for.
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
openapi.json`) — the first use of this pattern in the repo, later reused by
  `docs-generated-cli` for the CLI reference/man pages/completions/config schema. There is
  exactly one transport — `fetch` over HTTP — so no `ApiClient`-style interface with swappable
  drivers is warranted; adding one would be pure ceremony with a single implementation.
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
  the single layout route (header, nav, the health indicator relocated from the old placeholder
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
  (`routes/history/`) are List (filterable by process type/outcome/driver, paginated via
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
  2. **The simulator driver actually requires five fields, not one.**
     `bhtune-cli`'s `build_loop_tags` (`commands/tune.rs`) hard-requires
     `pv_range_high`, `pv_range_low`, `mv_range_high`, `mv_range_low`, **and**
     `direction` whenever `driver: "simulator"` — the frontend had only validated and
     defaulted `pv_range_high`, so a first-time visitor's default simulator run 400'd
     immediately on submit (only caught by actually clicking "Start tune" in a browser,
     not by reading the DTO's field list). Fixed by defaulting all five to exactly
     `bhtune simulate`'s own CLI-convenience values (`100`/`0`/`100`/`0`/`"reverse"`,
     read from `SimulateArgs::into_tune_args` in `bhtune-cli/src/args.rs`), extending
     `buildRequest()`'s validation to cover all five with the server's exact wording, and
     adding a `setDriver` handler that back-fills these onto switching to the simulator
     driver without ever overwriting a value the user already set.

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
  live-updating trend chart.** Driver: `TuneSampleRow::list_for_run_since(pool, run_id,
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
  opt-in is chosen. Authentication, TLS, and audit logging are planned post-v1 remote-access
  features (`server-remote-auth`, `server-tls`, `server-audit-log`, `server-oidc`) rather than
  blocking v1. This is a judgement call worth
  re-examining before that host is ever reachable off a trusted OT network: the precedent that
  makes it defensible in the meantime is that `opcda-bridge-gateway` is _already_ an
  unauthenticated network service in this exact topology, and it is strictly more dangerous than
  an unauthenticated bhtune (it can read/write any tag, whereas bhtune only ever writes the PID
  constants of one user-selected loop).
- **Step Test is deferred**, not part of v1 (MRFT only). Step Test is an alternative, simpler
  manual tuning method that observes PV changes via an OPC DA _subscription_ rather than polling
  reads, and the bridge's protocol has no such push/subscription RPC yet — `ListServers`/`Read`/
  `Write` are unary and `Browse` is a bounded, one-shot server-streaming call (the facade drains it
  into a single `Vec` before returning). MRFT itself only needs unary polling reads, so this
  doesn't block v1 — Step Test is blocked on adding a live push/subscription RPC to
  `opcda-bridge`, distinct from `Browse`'s existing bounded stream.
- **Plain, open SQLite. No encryption, no loop-locking, no login gate.** All tune
  history lives in a single, plain, open SQLite database anyone can inspect with any SQLite
  browser. This is a deliberate simplicity choice: an open-source tool has no reason to
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
  `driver-opcda`'s future orchestration glue or `cli-commands`) matches the same "do the DB
  layer ahead of time" reasoning that drove `db-schema` itself. `LoopRow` deliberately still has
  no CRUD methods yet — loop management is a separate concern from run history and stays
  deferred to whichever future todo actually needs it.
- **Dynamic run filtering uses `sqlx::QueryBuilder`, the only non-fixed SQL in `bhtune-db`.**
  `TuneRunFilter`'s seven fields (`loop_id`, `process_type`, `controller_type`, `outcome`,
  `driver`, `started_after`, `started_before`) are all optional, and the active `WHERE`
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
  supplies `TuneWriteRow`'s `response_level`. What a driver reads back is a different kind
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
  `cargo test --workspace`, and eventually `bhtune-cli`'s own binary) — confirmed by
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

## OPC DA integration reference (`driver-opcda`)

`driver-opcda` is implemented: `OpcDaDriver` in `crates/bhtune-driver/src/opcda.rs`
consumes the published `opcda-bridge` facade crate from crates.io, pinned directly in
`crates/bhtune-driver/Cargo.toml` (not `[workspace.dependencies]` — see "Key architectural
decisions" above). It does not use a Git dependency, a local path dependency, or the CLI
crate `opcda-bridge-client`:

```toml
# crates/bhtune-driver/Cargo.toml
[dependencies]
opcda-bridge = "0.2"
```

The facade intentionally hides generated gRPC details and exposes the typed API
`OpcDaDriver` wraps:

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
  `map_bridge_error` function: `Error::Connect` becomes `DriverError::Connect`, `Error::Rpc`
  becomes `DriverError::Operation`. Exhaustive (no wildcard arm) so a future new variant in
  `opcda_bridge::Error` fails this crate's build rather than silently falling into one bucket.
- `browse` hardcodes `flat: false` (one level, matching `Driver::browse`'s own contract) and a
  `max_tags` of `1000`, matching `opcda-bridge-client`'s own CLI default
  (`DEFAULT_MAX_TAGS` in that crate's `config.rs`) for consistency with the reference CLI.
  The gateway must use recursive hierarchical browsing for servers whose `OPC_FLAT` response
  contains only top-level branches (for example, Yokogawa CSHIS); its tree adapter accepts both
  dotted and slash-separated fully-qualified item IDs.
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

## Simulator driver reference (`driver-simulator`)

`driver-simulator` is implemented in `crates/bhtune-driver/src/simulator.rs`: an in-process
FOPDT (first-order-plus-dead-time) process model plus a standalone virtual PID controller, served
through the real `Driver` trait as `SimulatorDriver`. No external process, no Windows, no
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
`bhtune-cli`'s `DriverKindArg` enum deliberately has no `Replay` variant, and
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

## CLI reference (`cli-commands`)

`bhtune-cli` (binary name `bhtune`) is a thin `clap`-derive orchestration layer over
`bhtune-core`/`bhtune-db`/`bhtune-driver` — every subcommand opens the same SQLite database
(`crate::db::open`, which also seeds the four built-in templates) and shares one dispatcher in
`lib.rs::run_with_cli`.

- **`bhtune tune`** — runs a full MRFT test against a named template: resolves the template,
  derives the tag set (`build_loop_tags`, in `commands/tune.rs`), selects a driver
  (`crate::driver::build`, `--driver opcda|simulator`), transitions the loop to Manual, polls
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
  `--driver simulator` against a synthetic FOPDT process (`SIMULATOR_PV_TAG`/
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

**Testing approach.** `commands/tune.rs`'s tests use a `MockDriver` (an in-memory
`Driver` impl with canned/erroring responses) for setup-and-validation-error paths, a real
`SimulatorDriver` for full happy-path runs (including the `--mrft-delay` padding test, which
necessarily costs a couple of real wall-clock seconds — `chrono::Utc::now()`, which
`pre_delay_end`/`post_delay_end` are computed from, is unaffected by tokio's pausable test
clock), and a shared test-only mock gRPC `Bridge` service (`crate::test_support`, used by
`driver.rs`, `tune.rs`, and `commands/opc.rs`) to prove the OPC DA path — connect, initial
reads, a mid-poll failure, and the `opc` passthrough commands — actually works end-to-end
without a real gateway or OPC DA server. A single canned mock read response satisfies every
setup read regardless of which tag was requested (see `OpcDaDriver::read`'s positional, not
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

| Setting               | CLI flag           | Env var                 | Config key       | Default                                                                                                                                                                                             |
| --------------------- | ------------------ | ----------------------- | ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Database path         | `--db`             | `BHTUNE_DB`             | `db`             | Linux/macOS: `$XDG_DATA_HOME/bhtune/bhtune.db` (falls back to `$HOME/.local/share/bhtune/bhtune.db`); Windows: `%APPDATA%\bhtune\bhtune.db`                                                         |
| opcda-bridge gateway  | `--bridge-host`    | `BHTUNE_BRIDGE_HOST`    | `bridge_host`    | `localhost:7600`                                                                                                                                                                                    |
| Default OPC DA server | `--server`         | —                       | `server`         | none — must be set one way or another for `tune --driver opcda` and the `opc` subcommands                                                                                                           |
| User template catalog | `--templates`      | `BHTUNE_TEMPLATES`      | `templates`      | Linux/macOS: `$XDG_CONFIG_HOME/bhtune/templates.toml` (falls back to `$HOME/.config/bhtune/templates.toml`); Windows: `%APPDATA%\bhtune\templates.toml` — missing is not an error at this tier only |
| History retention     | `--retention-days` | `BHTUNE_RETENTION_DAYS` | `retention_days` | none — retain forever (see "Status" above for the retention sweep design)                                                                                                                           |

`resolve_db_path`/`resolve_bridge_host`/`resolve_retention_days` fold the env var into the
CLI value already (via clap's `env` attribute on `Cli::db`/`TuneArgs::bridge_host`/
`Cli::retention_days`/`OpcCommand`'s per-variant `bridge_host`), so each `resolve_*`
function itself only has two tiers left to arbitrate: the (already env-merged) CLI value
versus the config file. `resolve_server` errors if
neither the CLI nor the config file supplies a value — there's no sensible default OPC
server to fall back to — and is applied only for the `Opcda` driver inside
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
use (`cron`, Windows Task Scheduler, CI), and `bhtune history list`/`show`/`revert`/`prune`
support machine-readable output for the same callers:

- **`--yes`** — required before `--write-pid` is honored at all; see below.
- **`--write-pid <aggressive|moderate|sluggish>`** — writes that response level's calculated
  PID constants back to the DCS without the interactive stdin confirmation prompt
  `maybe_write_back` otherwise uses. Requires `--yes`; `run()` rejects the combination with a
  hard `Err` as its very first statement, before any driver connection or database write —
  an unattended write-back must be an explicit, deliberate choice, not a stray flag. If the
  named response level has no recorded calculated result (defensive; not reachable through
  normal CLI validation), the write-back is reported as failed rather than attempted, exactly
  as an invalid interactive selection already was.
- **`--output <table|json>`** — on `tune`/`simulate`, the final summary line; on
  `history list`/`show`, the whole listing/detail; on `history revert`, the pre-attempt
  status line and the final outcome (a `RevertJson` object); on `history prune`, the
  deleted-or-would-delete count and cutoff (a `PruneJson` object, via the same shared
  `crate::retention` module the automatic startup/periodic sweeps use, so a `--dry-run`
  preview and a real prune can never disagree about which runs are in scope). `table` is the
  default and preserves the original plain-text shape exactly. `json` prints one
  `serde_json::to_string_pretty` object (or array, for `history list`) to stdout — never a
  mix of the two on one invocation. Local DTOs (`RunSummaryJson`/`RunListJson`/
  `InitialReadingsJson`/`ResultJson`/`WriteJson`/`RunDetailJson`/`RevertJson`/
  `RevertedTargetJson`/`PruneJson` in `commands/history.rs`) project the `bhtune-db` row types
  that don't themselves derive `Serialize` (DB row shape stays deliberately decoupled from any
  API/CLI JSON shape); `bhtune-core` enums and `LoopConfig`/`TuneDriver`/`TuneOutcome` already
  derive `Serialize` and are reused directly.
- **Exit codes** — `lib.rs` defines `EXIT_SUCCESS = 0`, `EXIT_FAILURE = 1` (a setup error:
  unknown template, invalid flag combination, database/driver connection failure — anything
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
  all (`build_loop_tags` leaves them all `None` for `DriverKindArg::Simulator`), so write-back
  is unconditionally `WriteBackOutcome::Skipped` regardless of these flags.

**Testing approach.** `tune_outcome_for_run`/`print_summary` are pure/near-pure functions
(the latter's only side effect is the `println!` itself) tested directly against every
`RunOutcome` x `OutputFormat` combination, rather than only through a full `run()`. A genuine
end-to-end test of `run()` reaching a real `WriteBackOutcome::Written`/`Failed` through the
actual polling loop is structurally impossible with current test infrastructure: the mock OPC
DA bridge only ever returns static PV values (can never trigger a real relay switch), and
`SimulatorDriver` structurally has no PID tags at all (see above) — so
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
  immediately after constructing the `LoopConfig`, before any driver connection or database
  write, mirroring the `--write-pid`-requires-`--yes` fail-fast precedent below.
- **`--timeout-secs <seconds>`** (default `3600`) — a mandatory wall-clock limit on the whole
  test, with no disable/unlimited option; a value of `0` just means an (essentially unusable)
  instant timeout, preserving genuine "mandatory" semantics. Implemented in
  `run_polling_loop` as a `tokio::time::sleep` created once before the loop and raced via a
  `tokio::select!` arm alongside `interval.tick()` and a single process-wide `CtrlC` handle
  (see `safety-cancellation` below) — but that outer race only covers the _idle_ wait between
  ticks. The timeout (and Ctrl+C) also stay effective _during_ a tick — including a stalled
  driver read or write, e.g. a wedged DCOM call or a black-holed network — because every
  driver call inside the tick body is separately raced against the same `CtrlC` handle and a
  `--op-timeout-secs` cap via `bounded_driver_call` (see `safety-cancellation` for why this
  two-layer design, rather than one that only checked after each completed tick, was needed).
  On firing, the loop is restored to its pre-test mode via the exact same path as a Ctrl+C
  abort (`restore` + `TuneRunRow::abort`, recording plain `Aborted` in `tune_runs.outcome` —
  no new DB state) and reported to the caller as the distinct `TuneOutcome::TimedOut` /
  `EXIT_TIMED_OUT = 4`, so a scheduler's alerting can tell "this run had to be forcibly killed
  for running too long" (possibly a stuck relay, a misconfigured tag mapping, or a stalled
  driver read — worth investigating) apart from "an operator stopped it on purpose"
  (`EXIT_ABORTED`, routine).
- **`--write-pid <level>` unconditionally requires `--yes`** — in `run()`
  (`args.write_pid.is_some() && !args.yes`), checked before any driver connection or
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
relay_amp_before_any_driver_or_db_io` mirrors the same "no I/O before the fail-fast check"
pattern now also proven for `--write-pid`/`--yes` (`run_rejects_write_pid_without_yes_before_
starting_the_tune`).

### Live-plant safety hardening (done)

A post-`cli-logging` review of the live-tuning path (`commands/tune.rs`) surfaced nine
findings before the CLI's first real trial against live plant equipment: Ctrl+C/timeout
cancellation not reaching an in-flight driver call, no guaranteed restore on every exit
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
  the panic path; ranges read from the driver or passed as flags were never checked for
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
    regardless of whether each value came from a CLI flag or a driver tag: constructs
    `PvRange`/`MvRange` from the read ranges and confirms the initial MV falls inside the
    validated MV range. `read_f32`/`resolve_f32` additionally reject non-finite parsed
    values directly, closing the `"nan"`/`"inf"` string-parsing gap before a value is even
    assembled into `InitialState`.

  An `execute()`-level integration test proves the actual safety property end-to-end: a
  driver reporting an inverted MV range (`low >= high`) causes `execute` to fail with no
  entries at all in the driver's write log — i.e. `transition_to_manual` never runs, not
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
  `bhtune_driver::Quality`/`is_trustworthy()` existed but nothing in the tune path ever
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
  DB-side mirror of `bhtune_driver::Quality` — two separate enums since `bhtune-driver` and
  `bhtune-db` are sibling crates, neither depending on the other) and `tune_runs` gained
  `allow_uncertain_quality`, so a run's quality posture is part of its permanent history.
  `bhtune tune --allow-uncertain-quality` is the CLI flag; a poor-quality abort exits with
  `EXIT_POOR_QUALITY` (5), distinct from a Ctrl+C/timeout abort, and `--output json` carries
  nullable `poor_quality_tag`/`poor_quality` fields alongside the existing `timeout_secs`.

- **Ctrl+C and `--timeout-secs` now reach an in-flight driver call, and the restore itself
  is bounded** — done (`bhtune-cli::cancel`, `commands::tune::{bounded_driver_call,
attempt_restore}`). Previously the signal listener and the timeout sleep were both
  reconstructed fresh on every polling-loop iteration, inline in a `tokio::select!` — so for
  the entire duration of a tick's body (the PV read, the relay MV write, the sample insert)
  neither existed, and a Ctrl+C delivered in that window was silently lost (tokio coalesces
  signal delivery per kind, and a `Signal` future created _after_ delivery never observes
  it), with no fallback to the OS's default terminate-on-SIGINT behavior either (tokio
  replaces it process-wide the first time `ctrl_c()` is ever polled, and never reverts it). A
  hung driver read made the loop uninterruptible outright — exactly the scenario
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
  - `bounded_driver_call`/`TickOperation` — races one driver call (the tick's PV read, or
    its MV write) against `ctrl_c.signalled()` and a fresh `--op-timeout-secs` sleep (new
    flag, default 30s, capping a single operation rather than the whole run), returning
    `Completed(T)`/`Cancelled`/`TimedOut`; a genuine `Err` from the call itself still
    propagates via `?` rather than being folded into this enum, since a rejected write or a
    transport error is a real failure, not "gave up waiting". `run_polling_loop`'s outer
    `tokio::select!` (covering the _idle_ wait between ticks) reuses the exact same `&mut
CtrlC` handle passed down into the tick body's `bounded_driver_call`s, which is safe
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

  **Testing approach.** A `MockDriver.hanging_read`/`hanging_write` (awaits
  `std::future::pending::<()>()` before ever reaching its own bookkeeping, so a hung call is
  provably never recorded even though the abandoned future is only dropped, not signalled)
  backs two new `run_polling_loop` integration tests: a stalled PV read aborting via
  `--op-timeout-secs` with no sample recorded (no valid tick exists yet), and a stalled MV
  write being cancelled by a `CtrlC::test_pair()`-driven background task (standing in for a
  human pressing Ctrl+C mid-write) while still recording the sample from that tick's earlier,
  already-completed PV read. `bounded_driver_call`/`attempt_restore` also each have direct
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
  via a hand-constructed fully-armed `MutationGuard` and a driver where all four writes
  fail) proves every step is attempted independently and the summary names all four. Three
  `execute()`-level integration tests cover the guard's actual exit paths:
  `transition_to_manual` failing on its very first write (before the mode-revert path is
  ever armed) still runs the unconditional MV restore step, leaves `MODE` untouched, and
  records `Incomplete` with a "mode attribute" detail; a `persist_results`/`complete`
  failure after a genuinely completed simulator test still attempts the restore and records
  `Confirmed`; and a poor-quality abort partway through polling (via a new
  `MockDriver::degrade_quality_after` test-harness extension, returning a tag's quality as
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

  **Testing approach.** `MockDriver` gained three more builders alongside the existing
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
  nothing on the driver's write log at all), a rejected write, a readback that errors after
  the pre-read has already succeeded, a poor-quality readback (distinguished by message
  prefix from both the read-error case and a pre-read failure), an out-of-tolerance readback,
  a successful rollback (confirming the driver's live value was actually restored, not just
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
  - **Validates before ever connecting to the driver.** In order: the run exists; the run
    used the `Opcda` driver (a `Simulator`/`Replay` run has no live loop to revert against);
    the run has at least one recorded `Write`-kind row (nothing to revert otherwise); that
    row's `previous` is `Some` (a write whose own pre-read failed has nothing recorded to
    revert to); `--yes` was passed (reverting writes to a live loop, same confirmation gate
    as the original write-back); the run's snapshotted tags have all three PID constant tags
    configured. Only after all six checks pass does it call `OpcDaDriver::connect` — so five
    of these checks are exercised in tests with no mock driver running at all, and even the
    connection-failure path itself is a genuine test (an unreachable host, proving every
    earlier check passed).
  - **Uses the run's own recorded connection — never re-resolves one** (`db-run-request-snapshot`,
    `resolve_revert_connection`). This closes a real latent safety bug: reverting used to
    resolve `--bridge-host`/`--server` from the flag/config precedence chain _at revert
    time_, so a tune run against `Kepware.KEPServerEX.V6` on gateway A could be reverted
    from a shell whose config pointed at gateway B — silently writing the first loop's old
    PID constants into a _different plant's_ controller, using tag names that may well exist
    on both. Now `resolve_revert_connection` always trusts `run.opc_server`/`run.bridge_host`
    (populated by `TuneRunRow::record_connection` when the original run started, and a hard
    error if either is missing — an old run predating this feature, or a `Simulator`/`Replay`
    run that never recorded one); an explicit `--server`/`--bridge-host` flag is only a
    cross-check, and a hard error if it contradicts the stored value rather than silently
    overriding it. `--bridge-host` deliberately has no `BHTUNE_BRIDGE_HOST` env fallback the
    way every other command's `--bridge-host` does, precisely so an unrelated ambient env var
    can never itself trigger a false "contradicts the recorded one" error.
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

  **Testing approach.** Thirteen dedicated tests (`db-run-request-snapshot` added three:
  `revert_errors_when_the_run_has_no_recorded_connection`,
  `revert_errors_when_an_explicit_server_flag_contradicts_the_recorded_one`, and
  `revert_errors_when_an_explicit_bridge_host_flag_contradicts_the_recorded_one`). Ten need
  no mock driver at all, since `revert`'s validation runs before it ever connects: no such
  run; the run used a non-`Opcda` driver; no `Write`-kind row recorded; the recorded write's
  `previous` is `None`; `--yes` not passed; the run's tags have no PID constant tags
  configured; the run has no recorded connection at all; an explicit `--server`/
  `--bridge-host` that contradicts the recorded one (one test each); and a genuine connection
  failure using the run's own recorded (but unreachable) connection, proving every check
  above passed. Two use the shared mock gRPC `Bridge` service from `crate::test_support`: a
  full success
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
  configured, see `build_tags`'s `DriverKindArg::Simulator` arm) printed "No PID constant
  tags configured for this run's driver/template; skipping write-back." on stdout _before_
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
compiled binary with `--log-level debug` against the simulator driver confirmed the log file
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
seed count (`db.rs`), driver construction for both the OPC DA and simulator branches
(`driver.rs`), and in `commands/tune.rs` — run start/finish, the `Err` path, both abort
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
driver connect attempt, the `tune_runs` insert) and, once that succeeds, `tokio::spawn`s
`bhtune_cli::commands::tune::drive()` (the polling/tuning phase itself) as a background task
tracked by a new `crate::active_run::ActiveRun` (an `Arc<Mutex<BTreeMap<i64, ActiveTask>>>`
plus an exclusive post-hoc write/revert reservation, shared via `AppState`). `POST /api/runs`
returns `201 Created` with the same
`RunDetailResponse` shape `GET /api/runs/{id}` would show for this run at this instant
(almost always still `outcome: "running"`) as soon as `prepare()` succeeds — it does not wait
for the tune to finish. `POST /api/runs/{id}/cancel` signals the background task's `CtrlC`
handle and awaits it reaching a terminal outcome, then returns `204 No Content`; cancelling
an already-finished or unknown run is not an error (`204`/`404` respectively, matching the
CLI's own idempotent-cancel precedent). Tune tasks may run concurrently; PID write/revert
operations reserve the registry exclusively so they cannot overlap a tune or another write.

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

**Exclusive-operation conflict detection.** `start_run` performs an optimistic pre-check for
an exclusive PID write/revert reservation to avoid a wasted `prepare()` call (a real driver
connection attempt and DB insert). The authoritative `ActiveRun::start` check repeats that
reservation check under the same mutex, so a reservation beginning between the pre-check and
task registration fails cleanly: the inserted row is marked `failed` with a reason that no
tune task was started. Independent tune starts do not conflict and are both registered.

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
(`core-tuning-math`/`driver-simulator`'s "passing-assert's message-format argument"). Of the
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

**`crates/bhtune-server/build.rs`: neutralizing a rust-embed compile-time build-order trap.**
`e2e-playwright`'s first real CI run failed every test with every route returning a literal
`404 not found` body, even though the server started cleanly and logged nothing alarming — the
plain job log showed only the failing Playwright assertions, and the actual cause only surfaced
by downloading the run's `playwright-report` artifact and reading its page-snapshot error
context, which showed `404 not found` as the entire rendered page for `/`. Root cause, traced
into `rust-embed`'s own macro-expansion source (`rust-embed-impl`/`rust-embed-utils` 8.12.0):
without `debug-embed`, the generated `get()` reads files from disk at runtime, but the
path-traversal guard's reference path (`canonical_folder_path`) is computed via
`Path::canonicalize()` **once, during the derive macro's expansion** — i.e. at `bhtune-server`'s
own compile time, not at request time. If `frontend/dist/` doesn't exist at that exact instant,
`canonicalize()` fails and the macro's fallback silently bakes in the raw, non-canonical folder
path (with `../../` segments left uncollapsed) instead of erroring. At runtime, every
`Assets::get(path)` call canonicalizes the _requested_ file's path — which resolves cleanly once
`frontend/dist/` exists — and checks it `starts_with()` that bad compile-time-baked path; a clean
canonical path can never `starts_with` an unclean one containing `../..`, so the guard rejects
every single file, including `index.html`, permanently, until `bhtune-server` is fully
recompiled. `.github/workflows/e2e.yml` builds `bhtune-server` before the frontend on a fresh
runner — exactly the trigger order — but this is a real, latent trap for any contributor too:
running `cargo build`/`cargo check`/`cargo test` on a fresh clone before ever running
`pnpm run build` in `frontend/` permanently breaks asset serving for that build, and building the
frontend afterward does not fix it — only a full recompile of `bhtune-server` specifically does.
`build.rs` neutralizes this unconditionally: it `create_dir_all`s `frontend/dist/` (using the
same `CARGO_MANIFEST_DIR`-relative path rust-embed's own attribute references) before the
crate's own source — and therefore the `RustEmbed` derive macro — compiles, so `canonicalize()`
always succeeds and the correct canonical path gets baked in regardless of build order. An
empty, merely-existing directory is sufficient; `#[allow_missing = true]` and the `503` path
above already handle "exists but empty" gracefully. Best-effort by design (`let _ = ...`, no
panic if directory creation itself fails, e.g. a read-only filesystem), deferring to that same
`allow_missing`/503 handling as the fallback safety net. Deliberately did not reorder
`e2e.yml`'s build-server-then-build-frontend step order once this fix landed — that order now
exercises this exact previously-broken scenario as an ongoing regression test on every CI run.
Verified by forcing a genuine full recompile (`cargo clean -p bhtune-server`, not just deleting
the binary, which Cargo can satisfy from cached fingerprinted objects without ever re-running
the derive macro — a trap that produced a misleading "can't reproduce" result until caught) both
without the fix (reproduced the exact `404 not found` CI failure) and with it (`pnpm run
test:e2e`'s full 4-test Playwright suite passes against a server built in that same order).

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

## `docs-generated-cli`: generating the CLI reference, man pages, completions, and config schema

A new `crates/bhtune-cli/examples/gen_docs.rs` regenerates four artifacts from the same
`clap`/`serde` definitions every real `bhtune` invocation already parses against, so none of
them can silently drift the way hand-written usage docs would — reusing `gen_openapi`'s exact
regenerate-and-diff idiom (see "Key architectural decisions" above), the pattern that file's
own doc comment named this example as the intended reuse of:

```sh
cargo run -p bhtune-cli --example gen_docs --features schemars
```

- **`docs/reference/cli.md`** — the full CLI reference as one Markdown document, via
  `clap_markdown::help_markdown_custom::<bhtune_cli::args::Cli>(&options)`. `clap-markdown`
  recurses the entire `Command` tree itself (no manual subcommand walk needed for this one),
  producing a table of contents plus one section per command/subcommand with its usage,
  options, and doc-comment prose.
- **`man/*.1`** — one man page per command _and_ subcommand (`bhtune.1`, `bhtune-tune.1`,
  `bhtune-template.1`, `bhtune-template-list.1`, ... 18 pages total), matching the convention
  real multi-command tools use (git, cargo) rather than one flat page. `clap_mangen::Man`
  only renders a single `clap::Command` at a time, so `gen_docs.rs` walks
  `Command::get_subcommands()` recursively itself, renaming each nested `Command` to its full
  hyphenated path (`cmd.name(...)`, e.g. `template` becomes `bhtune-template`) before
  rendering, exactly mirroring how git/cargo name their own subcommand man pages. `Command::
name` needs an owned `String` converted `impl Into<clap::builder::Str>`, which only exists
  behind clap's `string` feature (not otherwise used by this crate, and not worth enabling
  workspace-wide for one codegen example) — `gen_docs.rs` instead leaks the short-lived
  recursion-computed name strings (`Box::leak`), which is fine for a one-shot process that
  exits immediately after writing its output. These pages are what will let `pkg-aur` install
  real content into `/usr/share/man/man1/` instead of shipping a binary with no man page at
  all.
- **`completions/bhtune.bash`, `completions/_bhtune` (zsh), `completions/bhtune.fish`** — via
  `clap_complete::generate`, one file per shell using each shell's own conventional completion
  file name.
- **`docs/reference/config.md`** — JSON Schema for both `bhtune.toml` (`bhtune_cli::config::
BhtuneConfig`/`LogConfig`) and one DCS/PLC template catalog entry (`bhtune_core::template::
DcsTemplate`, the same type `template import`/the embedded and user catalogs all parse),
  rendered as two labeled fenced JSON code blocks (`schemars::schema_for!` produces a schema
  value, not prose, so there is no single-document API to lean on the way `clap-markdown`
  provides for the CLI reference). `schemars`' derive macro picks up each field's own doc
  comment as the schema's `description`, so this stays a real reflection of `template.rs`/
  `config.rs`'s existing documentation rather than a second, driftable copy of it.

**Where the `schemars` dependency lives, and why.** `DcsTemplate`'s derive lives in
`bhtune-core`, a library target, not in the example itself — so `schemars` needed the same
optional-feature treatment `bhtune-core` already has for `utoipa` (see "Key architectural
decisions" above), not a plain dev-dependency: an optional regular `[dependencies]` entry,
`#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]` alongside the existing
`#[cfg_attr(feature = "utoipa", ...)]` on `DcsTemplate` and the four `pid_config` enums it
embeds (`ProportionalType`/`IntegralType`/`DerivativeType`/`TimeUnit`), and a `schemars`
feature forwarding from `bhtune-cli` to `bhtune-core/schemars`. `BhtuneConfig`/`LogConfig`
(in `bhtune-cli` itself) get the same `cfg_attr` treatment directly. `clap-markdown`/
`clap_mangen`/`clap_complete`, by contrast, are used only by `gen_docs.rs` itself, never by
library-target code, so they stay plain `[dev-dependencies]` with no feature-gating —
mirroring exactly why `cargo add --dev` is right for those three and wrong for `schemars`
(a naive `cargo add --dev schemars` would put a library-target derive dependency somewhere
only test/example targets can see it, silently breaking the ordinary release build the
moment anyone tried to actually use the derive from `bhtune-core`'s own source). Off by
default: neither `bhtune`/`bhtune-server`'s ordinary release builds nor `bhtune-core`'s own
`cargo build -p bhtune-core` ever touch `schemars` — it is a docs-codegen-only concern.

**CI enforcement**, added to `checks.yml`'s existing `check` job right after the OpenAPI
drift step, same shape:

```yaml
- name: Regenerate CLI docs and check for drift
  run: |
    cargo run -p bhtune-cli --example gen_docs --features schemars
    git diff --exit-code -- docs/reference/ man/ completions/
```

`clippy --all-features` (already run earlier in the same job) covers linting the example
itself, since `--all-features` unifies in `schemars` and compiles `gen_docs` as part of
`--all-targets`.

**Gotcha: formatters must never touch generated files.** `.lefthook.yml`'s `prettier-format`
(glob `**/*.{md,yaml,yml,json,ts,tsx,css}`) and `shfmt-format` (glob `**/*.{sh,bash}`) hooks
would otherwise reformat `docs/reference/cli.md`, `docs/reference/config.md`, and
`completions/bhtune.bash` on every commit that touches them — caught by hand before this
phase's first push, since CI's drift check diffs _raw_ generator output against whatever is
committed and never runs prettier/shfmt first, so a single reformatting pre-commit run would
have made the drift check fail permanently. Both hooks now `exclude: ['docs/reference/**',
...]`/`exclude: ['completions/**']`; `openapi.json` and `frontend/src/api/schema.d.ts` are
excluded from `prettier-format` too, even though their generators currently happen to already
produce prettier-compatible output, so a future version bump of `utoipa`/`openapi-typescript`
can't silently reintroduce the same failure mode. Any future generated artifact should be
added to these exclude lists (or `.prettierignore`) if its extension matches an existing
format hook's glob — `man/*.1`, `completions/_bhtune` (no extension), and
`completions/bhtune.fish` happen not to match any current glob, so they needed no exclusion.

## `docs-site-scaffold`: the Docusaurus documentation site

`website/` is a new pnpm workspace member (`bhtune-website`, Docusaurus 3 classic preset)
that publishes `docs/` as a browsable, searchable site, live at
[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/) — see the
`website/` row in "Crate map and phase status" above for what's done versus still pending
(`docs-versioning`).

**The content root is the real `docs/`, not a copy.** `docusaurus.config.ts`'s `docs` preset
sets `path: '../docs'`, so the docs plugin reads Markdown directly from the repo-root folder
every other part of the project already treats as the source of truth — there is no
website-local content duplicate to keep in sync. The one consequence worth knowing:
Markdown links that escape `path`'s root (e.g. a hypothetical `../CONTRIBUTING.md` from
inside a `docs/` file) cannot be resolved by Docusaurus even though they resolve fine when
viewed raw on GitHub, since they point outside the folder the docs plugin scans. The fix is
always an absolute `https://github.com/bytehound-labs/bhtune/blob/main/...` URL instead of a
relative path — already applied once, to `docs/dcs-templates.md`'s CONTRIBUTING.md link.
`docs/internal/**` is excluded from the build entirely (`exclude: ['internal/**']`), so a
similar link from inside `docs/internal/v1-checklist.md` was left as a normal relative
`../AGENTS.md` link rather than converted.

**`docs/intro.md` is the site root, not a separate marketing page.** `routeBasePath: '/'`
(docs plugin) plus `slug: /` (frontmatter on `intro.md` itself) collapses the site's home
page onto `/` directly — no `src/pages/index.tsx` landing page exists; the scaffold's
placeholder one was deleted. Sidebar order everywhere else comes from `sidebar_position`
frontmatter on individual `.md` files plus a `_category_.json` file per subfolder
(`docs/getting-started/`, `docs/guides/`, `docs/reference/`) — `website/sidebars.ts` itself
stays a plain autogenerated-from-filesystem sidebar and does not need editing for routine
content changes (see `website/README.md`'s "Adding or reordering pages").

**`editUrl` must be a function, not a string, when `path` points outside the site
directory.** A plain string `editUrl: '.../edit/main/docs/'` naively concatenates with the
`path`-relative doc path and produces a doubled `.../edit/main/docs/../docs/intro.md` (harmless
in a browser, since `../` segments normalize, but not something to ship deliberately). The fix
is the function form Docusaurus documents for exactly this case:
`editUrl: ({docPath}) => \`https://github.com/bytehound-labs/bhtune/edit/main/docs/${docPath}\``,
which resolves cleanly (`.../edit/main/docs/intro.md`, `.../edit/main/docs/getting-started/
installation.md`, etc.) because `docPath` is already correct relative to the configured
content root.

**Search is `@easyops-cn/docusaurus-search-local`**, not Algolia DocSearch: fully static,
offline, and open-source, consistent with the project's no-proprietary-dependencies stance and
needing no third-party application/approval process. Worth revisiting once the site has
enough content and traffic to justify the extra setup.

**`onBrokenLinks`/`onBrokenAnchors` are both `'throw'`** (Docusaurus's own default, kept
rather than relaxed), which makes `pnpm --filter bhtune-website run build` a real,
zero-extra-effort drift gate against `docs/` content referencing a page or heading that was
renamed or deleted — this already caught the CONTRIBUTING.md link above on the very first
build attempt. A new `website` job in `checks.yml` (parallel to `frontend`, same
`pnpm/setup@v2` pattern) runs `format:check`/`lint`/`typecheck`/`build` on every PR so this
gate is automatic, not something to remember to run by hand. `check:licenses` is not
duplicated in that job: `pnpm licenses list` (which `scripts/check-frontend-licenses.mjs`
shells out to) already reports on every pnpm workspace member from the repo root, so the
existing `frontend` job's license step covers `website`'s dependency tree too.

**License allowlist grew by four entries** (`scripts/check-frontend-licenses.mjs`) for
licenses genuinely new to the dependency tree, none from `bhtune-website`'s own direct
dependencies but all transitive, pulled in by Docusaurus/the search plugin's own toolchains:
`MIT-0` (`@csstools/postcss-*`), `CC-BY-4.0` (`caniuse-lite`'s browser-data tables, an
unavoidable transitive dependency of browserslist/postcss-preset-env across the whole JS
ecosystem), `MPL-1.1` (`lunr-languages`, same weak-copyleft family as the already-allowed
MPL-2.0), and `BlueOak-1.0.0` (`sax`, verified by reading its actual license text — at least
as permissive as MIT). The script also gained AND-expression support (`isAllowed` now
requires every arm of an `X AND Y` expression to be individually allowed, versus OR's "any
one arm suffices") to resolve `@swc/core-linux-x64-gnu`'s `Apache-2.0 AND MIT` without a new
allowlist entry, and a narrow `VERIFIED_UNKNOWN_LICENSES` exception keyed to the exact
`require-like@0.1.2` package version (its license metadata is undeclared, but its `License`
file is verbatim MIT text, confirmed by direct inspection) — deliberately _not_ a blanket
"Unknown is fine" rule, so a genuinely proprietary or license-less future dependency still
fails the check loudly.

**`docs-site-deploy` is also done: the site is live** at
[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/), published by
`.github/workflows/docs-deploy.yml` via `actions/upload-pages-artifact` +
`actions/deploy-pages` — the standard GitHub-Actions-native Pages flow, not the older
`docusaurus deploy`-to-a-branch approach (no `gh-pages` branch exists or is needed).
GitHub Pages itself was switched to `build_type: workflow` via the API
(`gh api --method POST repos/bytehound-labs/bhtune/pages -f build_type=workflow`) — the
default `legacy`/branch build type would otherwise ignore an Actions-based deployment
entirely. Triggers are path-filtered to `docs/**`, `website/**`, `pnpm-lock.yaml`, and the
workflow file itself, and only run on pushes to `main` (plus manual `workflow_dispatch`) —
`checks.yml`'s `website` job already builds/lints every PR, so this workflow's only job is
publishing an already-validated build, not re-validating it. `concurrency: { group: pages,
cancel-in-progress: false }` serializes deployments rather than cancelling one mid-flight,
so a fast-following push can never leave the live site on a half-published build. No custom
domain; `url`/`baseUrl`/`organizationName`/`projectName` in `docusaurus.config.ts` were
already set correctly for the `<org>.github.io/<repo>` path during `docs-site-scaffold`, so
no config changes were needed to go live. Remaining in this area: `docs-versioning`
(deferred until `release-v1` actually cuts a version — a version dropdown with a single
entry is pure overhead).

## `docs-api-rustdoc`: publishing the Rust API reference

`cargo doc --workspace --no-deps --all-features` output is published under `/api/` on the
docs site, alongside a hand-written index page at `docs/reference/api.md` (linked from the
site navigation/footer and from `docs/reference/_category_.json`'s sidebar). Together these
give contributors a real, browsable rustdoc reference for all six crate/binary targets
(`bhtune`, `bhtune_driver`, `bhtune_cli`, `bhtune_core`, `bhtune_db`, `bhtune_server`)
without hand-authoring any of the content itself.

**Rustdoc output has no root `index.html` for a multi-crate workspace**, and produces five
fixed infrastructure directories alongside the real per-crate ones: `search.index`, `src`,
`static.files`, `trait.impl`, `type.impl`. `.github/workflows/docs-deploy.yml`'s publish step
therefore generates its own landing `index.html` by listing `website/static/api/*/` and
excluding exactly those five names — every other directory found is a real crate/binary and
gets a link, so the landing page can never go stale when a crate is added, renamed, or
removed; nothing needs to be hardcoded or kept in sync by hand. A stale `bhtune_desktop`
entry from the deleted `arch-drop-desktop` crate was found locally during development,
persisting in a dirty `target/doc/` from before that crate was removed from the workspace —
the same publish step always runs `rm -rf target/doc` first specifically to guard against
`Swatinem/rust-cache` ever restoring a stale `target/doc` containing docs for a since-deleted
crate.

**`docs/reference/api.md` links via Docusaurus's `pathname://` protocol**, e.g.
`pathname:///api/bhtune_core/index.html`, not a normal Markdown link. This is deliberate and
load-bearing: `checks.yml`'s PR-time `website` job builds the Docusaurus site without ever
generating rustdoc content (that only happens in `docs-deploy.yml`, which runs on `main`
pushes, not PRs), so `website/static/api/` genuinely does not exist at PR-build time.
Confirmed empirically that `pathname://` links bypass Docusaurus's route-resolution
machinery entirely and, critically, are **not checked by the `onBrokenLinks`/
`onBrokenAnchors: 'throw'` gate** — the site builds successfully with these links present
even when the target files are completely absent from disk. A normal internal Markdown link
would have failed that gate on every single PR. The links still correctly receive the site's
`baseUrl` (`/bhtune/`) prefix at build time, verified by grepping the built HTML output for
`href=/bhtune/api/bhtune_core/index.html`-style attributes.

**Two different "`/api/`" concepts exist on this project and `docs/reference/api.md`'s prose
deliberately disambiguates them**: (1) `bhtune-server`'s live HTTP REST API, documented via
OpenAPI/Scalar UI at `/api/docs` on a _running_ server instance (the functional surface the
frontend and any integration script actually call); (2) this static rustdoc Rust-source
reference, published at `/api/` on the docs website — a completely separate, statically
hosted GitHub Pages site unrelated to any running server. Both nominally live under a path
containing "/api/", so the page says so explicitly rather than leaving it to be inferred.

**`docs-deploy.yml` gained a Rust toolchain** (`dtolnay/rust-toolchain@stable` +
`Swatinem/rust-cache@v2`) and `protoc` (`taiki-e/install-action@v2`, the same tool the
`opcda-bridge-proto`/`tonic-build` build dependency needs in `checks.yml`) purely to run
`cargo doc`; it built no Rust code before this. The push-trigger `paths:` filter was widened
to include `crates/**` and root `Cargo.toml`, since rustdoc content now depends on source
changes, not just `docs/`/`website/` edits. `--all-features` (matching `checks.yml`'s
clippy/test convention) ensures `bhtune-cli`'s optional `schemars` feature — which gates the
JSON-Schema-deriving types `docs-generated-cli`'s `gen_docs` example needs — is included in
the published docs.

## `docs-agent-ci`: the AI docs agent

`.github/workflows/docs-agent.yml` runs GitHub Copilot CLI headless on every PR touching
`crates/**` and auto-commits narrative-prose documentation updates onto the PR branch — tier 2
of the documentation contract (see "Documentation contract" above). Tier 1
(`docs/reference/**`, generated) is already diff-gated by `checks.yml`; tier 3 (`AGENTS.md`) is
explicitly off limits to this workflow.

**Guardrails, all load-bearing** (numbered comments in the workflow itself cross-reference
these):

1. **Infinite loop.** The agent's own commits carry a distinct git author identity
   (`bhtune-docs-agent <bhtune-docs-agent@users.noreply.github.com>`), and a separate `guard`
   job checks HEAD's author before anything else runs, skipping if it's already the agent's own
   commit. This can't be done with `github.actor`: the agent authenticates with
   `COPILOT_GITHUB_TOKEN`, a personal PAT (see below), so GitHub attributes its push to that
   token's human owner — indistinguishable from that person pushing themselves. The commit
   author, independent of which token performed the push, is the only reliable signal. The
   `paths: crates/**` trigger filter is a second, structural line of defense (the agent only
   ever touches `docs/**`/`README.md`, which doesn't match that filter), but the explicit
   author check doesn't rely on that alone.
2. **Blast radius.** The agent may only touch `docs/**` (excluding the generated
   `docs/reference/**`) and `README.md`. Enforced twice: a `--deny-tool 'write(AGENTS.md)'`
   flag blocks the one specific file that must never be auto-edited regardless of path-prefix
   ambiguity in the CLI's own tool-permission matching, and a post-run `git status --porcelain`
   check fails the job and discards every change if the diff touched anything outside the
   allowed set — this second check is the one actually enumerated against the full allowlist,
   not just the single denied file.
3. **`AGENTS.md` is special.** The agent is instructed (and tool-blocked) to never edit it; if
   it believes something here is stale, it says so in its final response instead, which gets
   posted as a PR comment for a human to act on or ignore.
4. **Fork PRs.** `pull_request` runs from forks never receive repo secrets, so
   `COPILOT_GITHUB_TOKEN` is absent and the job skips itself — the safe default. Deliberately
   not "fixed" with `pull_request_target` (write permissions in the context of untrusted fork
   code is a known privilege-escalation foot-gun). A `workflow_dispatch` path with a `pr_number`
   input exists instead, for a maintainer who has already read the diff to run manually; since
   a fork PR's branch doesn't live in this repo, that path pushes to a new
   `docs-agent/pr-<n>-followup` branch here rather than trying to push back into the fork.
5. **Auth.** `COPILOT_GITHUB_TOKEN` is a personal classic PAT (scopes include `copilot`, needed
   for Copilot CLI access — the default `GITHUB_TOKEN` cannot grant this) rather than a
   dedicated machine account or GitHub App, since classic PATs are the only token type
   confirmed to carry Copilot access, and a from-scratch bot identity was judged not worth the
   setup cost for a project at this stage. This is a real, accepted trade-off: that token's
   scopes are broader than this one workflow needs (a personal PAT can't be scoped to a single
   repository the way a GitHub App installation can). The workflow only ever reads it from
   `secrets.COPILOT_GITHUB_TOKEN`, so narrowing this later (a dedicated fine-grained PAT or App,
   if one is ever confirmed to support Copilot CLI auth) is a secret-rotation, not a workflow
   change.
6. **Cost.** Each run consumes Copilot premium requests. The `crates/**` path filter keeps this
   off PRs that can't have caused prose drift, and `--model` is pinned (`claude-sonnet-4.5`)
   rather than left on auto-routing so a model upgrade never silently changes cost/behavior on
   every future PR without a reviewed change here.

**Validated locally** (flag parsing via a scratch-repo smoke test, then `actionlint` against
the workflow file — it caught one real script-injection risk worth noting as a general
lesson: `github.event.pull_request.head.ref` was originally interpolated directly into a
`run:` shell block; since PR branch names are attacker-controlled and git ref names permit
shell metacharacters like `$()`, GitHub's literal template substitution would have spliced
attacker-controlled text directly into the script before the shell ever saw it. Fixed by
passing it through `env:` instead, so the value becomes a runtime shell-variable expansion
rather than a compile-time text substitution — the standard fix for this whole vulnerability
class). Not yet validated against a real, non-trivial PR that actually warrants a prose
change (only a trivial same-repo smoke PR) — treat the auto-commit path as unproven under
real-world drift until one goes through it.

## `build-matrix`: the release binary matrix

`.github/workflows/release.yml` builds and packages the `bhtune` (CLI) and `bhtune-server`
(GUI/HTTP) binaries for Linux (`x86_64-unknown-linux-gnu`), macOS
(`aarch64-apple-darwin`), and Windows (`x86_64-pc-windows-msvc`) — the same three-platform
shape opcda-bridge already ships.

**`taiki-e/create-gh-release-action` + `taiki-e/upload-rust-binary-action`, not
`cargo-dist`.** The plan originally called for `cargo-dist`, but reviewing opcda-bridge's
own already-working `release.yml` (part of `cross-project-ci-audit`) turned up a simpler,
already-proven alternative doing exactly what this project needs, with far less machinery:
one action builds the binaries, packages a platform-appropriate archive (`.tar.gz` on
Unix, `.zip` on Windows), computes checksums, and uploads to the tag's GitHub Release.
`cargo-dist` additionally generates shell/PowerShell/npm installer scripts and an
updater — none of which bhtune needs, since the Windows MSI (`pkg-windows-installer`) and
the AUR package (`pkg-aur`) are the actual installer stories, not a `curl | sh` script.
Adopting the sibling project's simpler, working tool beats introducing a second,
heavier one for the same job — directly the kind of cross-project consistency
`cross-project-ci-audit` recommended pursuing.

**One archive per platform, bundling both binaries.** `bin: bhtune,bhtune-server` in a
single `upload-rust-binary-action` step packages both into one
`bhtune-$tag-$target.(tar.gz|zip)` archive (plus `LICENSE`/`README.md` via `include:`) —
matching the "one package, not two" packaging decision below: now that the GUI is
browser-served rather than a Tauri app, there is no GUI-toolkit dependency that would need
keeping off a headless server build, so there is no reason to ship the CLI and the server
as separate packages either.

**The frontend must build before the Rust build, every time, in this specific
workflow.** `bhtune-server`'s `--release` profile embeds `frontend/dist/` into the binary
at compile time via `rust-embed` (see `server-embed-spa`); a debug build (as `e2e.yml`
uses) reads the directory live off disk instead and doesn't need this ordering. This is
the one workflow in the repo that produces `--release` binaries meant to run standalone
without the source tree alongside them, so it's the one place a missing/stale
`frontend/dist/` would silently ship a binary with no UI (or an old one) baked in. The
step order is: `pnpm/setup@v2` (which runs `pnpm install` itself) → `pnpm --filter
bhtune-frontend run build` → the Rust binary build/package step.

**`protoc` is a real build requirement here, not just a test-only one.** Unlike a pure
Rust dependency, `bhtune-driver`'s (non-dev) `opcda-bridge` dependency pulls in
`opcda-bridge-proto`, whose `build.rs` calls `tonic_prost_build::compile_protos` at
compile time — so every platform in the matrix installs `protoc` via
`taiki-e/install-action`, matching `checks.yml`'s `check`/`windows`/`package`/`msrv` jobs.
SQLite itself needs no such step: `bhtune-db`'s `sqlx` dependency uses the bundled
(vendored, statically-linked) SQLite feature, not `sqlite-unbundled`, so the produced
binaries have no external SQLite runtime dependency to document or install separately —
confirmed by running a real `--release` build locally and serving a request from it
directly.

**Two trigger modes, doing genuinely different things, not just a toggle.** A `v[0-9]+.*`
tag push runs the real thing: create the GitHub Release for that tag (unless
`release-plz` — not yet wired up, see `release-v1` — ends up owning that step instead, in
which case the `create-release` job should be deleted rather than fighting over who
creates it), then build, package, and upload real assets into it. A manual
`workflow_dispatch` runs the identical matrix in `dry-run: true` mode — builds and
packages everything, proving the frontend build, `protoc` install, and packaging all still
work on every platform — but uploads nothing and requires no release to already exist,
making it safe to run at any time (e.g. after a dependency bump) without cutting a real
release. Verified directly: a manual dispatch run built and packaged all three platforms
successfully with the dry-run banner in each job's log confirming no upload was attempted.

**This does not, by itself, ship v0.1.0.** `build-matrix` makes cutting a real release
_possible_ — a maintainer can push a `v0.1.0` tag today and get a real GitHub Release with
three working platform archives attached, with no dependency on `release-plz` or its
still-unprovisioned `RELEASE_PLZ_TOKEN` secret. Whether to actually do that now is a
deliberate call left to the project owner, not an automatic next step this todo unblocks
by completing: the golden-master replay validation gate (`core-replay-harness`) that
proves the Rust engine reproduces the legacy app's tuning behavior exactly is not yet
built, and shipping a first public release before that gate is green would let users run
an unverified reimplementation of code that writes PID constants to live plant equipment.
See `release-v1` in "Phases and todos" below.

## `pkg-docker`: the Docker image

A multi-stage root `Dockerfile` plus `.github/workflows/docker-publish.yml` publish
`ghcr.io/bytehound-labs/bhtune`, a ~110 MB image bundling both binaries and the embedded
SPA. This is deliberately a **secondary** distribution channel: the Windows MSI
(`pkg-windows-installer`) remains the primary one, since OT sites frequently prohibit or
simply lack container runtimes — see the "v1 adapters" bullet above under "Key
architectural decisions" for the full reasoning. Nothing about shipping a Docker image
changes that ordering.

**Three stages, each stripped to exactly what the next stage or the runtime needs.**
`frontend` (`node:22-slim`) builds the React SPA with `pnpm`; `builder`
(`rust:1-slim-bookworm`) compiles `bhtune`/`bhtune-server` in release mode; `runtime`
(`debian:bookworm-slim`) contains only the two resulting binaries, `ca-certificates`, and a
non-root user — no Node, no Rust toolchain, no source tree. Manifests are copied before
source in the `frontend` stage specifically so `pnpm install --frozen-lockfile` is
cache-hit across rebuilds that only touch application code.

**The `frontend/dist/`-before-`cargo build` ordering is load-bearing, not just
convenient.** `bhtune-server`'s `rust-embed` usage only embeds `frontend/dist/` into the
binary for `--release` builds (see `server-embed-spa`'s design section) — there is no
after-the-fact embed step, so the Dockerfile must `COPY --from=frontend
/src/frontend/dist/ frontend/dist/` before running `cargo build --release`, exactly
mirroring `build-matrix`'s `release.yml` step ordering above. Verified directly, not just
by reading the code: an earlier local build run with the copy ordered _after_ `cargo
build` produced a container that served a 503 for every static asset; reordering the copy
and rebuilding fixed it, confirming the failure mode is real rather than theoretical before
trusting the final Dockerfile.

**The Rust builder stage needs two system packages beyond `protoc`, because `slim` isn't
`rust`.** `protobuf-compiler` is already a known requirement — `opcda-bridge-proto`
compiles `bridge.proto` via `tonic-build` at build time, the same non-dev requirement
`build-matrix` installs via `taiki-e/install-action` above. `build-essential` is the new
one this todo surfaced: `bhtune-db`'s bundled SQLite (`libsqlite3-sys`) compiles a small C
amalgamation via the `cc` crate at build time, and unlike the default (non-`slim`) `rust`
image, `rust:1-slim-bookworm` does not include a C compiler at all. Skipping it fails the
build with a `cc` "not found" error rather than anything SQLite-specific, which is easy to
misdiagnose as a missing Rust dependency instead of a missing system one.

**`BHTUNE_BIND=0.0.0.0:8787` is the image's own default, deliberately overriding the
native binary's `127.0.0.1`-only default.** The security posture recorded in "Web app
architecture" above (bind loopback by default, LAN exposure as a loud explicit opt-in) is
preserved, not weakened, by this override: a container's loopback interface is invisible to
`docker run -p`/`--publish` port mapping, so binding `127.0.0.1` _inside_ the container
would make the server unreachable even with a port published, which is a confusing
footgun rather than a safety feature. Running this image and choosing to publish a port is
itself the explicit opt-in that `127.0.0.1`-by-default exists to require on the native
binary — Docker's own network isolation is the real boundary. Verified empirically: the
server was reachable via `curl` from outside the container only with this override in
place, matching the reasoning rather than assuming it.

**`.dockerignore` had one real mistake, caught before it shipped.** An early draft
excluded `website/` wholesale to keep docs-site content out of the build context. That
broke the `frontend` stage's `COPY website/package.json website/package.json` step, since
`website` is a real `pnpm-workspace.yaml` member (see `build-matrix`'s and `docs-site-scaffold`'s
notes on this same fact) whose manifest `pnpm install --frozen-lockfile` needs to resolve
the lockfile, even though only `frontend/` is ever actually built. Fixed by excluding only
the docs-site's content subdirectories (`website/docs`, `website/blog`, `website/src`,
`website/static`, and its two root config files) rather than the whole directory, which
still keeps `website/package.json` copyable. Also excludes build artifacts (`target/`,
`node_modules/`, `frontend/dist/`), VCS/editor metadata, `tests/golden/raw/` (kept excluded
as a guard against a future large capture bloating the build context, even though the
existing raw captures have since been deleted — see `cleanup-golden-traces`), and local
secrets/DB files (`.env`, `*.db*`).

**Publish workflow: build on every trigger, push only on a real push.** A `pull_request`
or manual `workflow_dispatch` run builds the full image — proving the Dockerfile still
works on every PR that touches it — but pushes nothing and touches no registry
credentials, matching `release.yml`'s own dry-run convention above. Only a push to `main`
or a `v[0-9]+.*` tag logs into GHCR and pushes. `docker/metadata-action`'s default
`flavor: latest=auto` adds a `latest` tag only alongside a real `type=semver` tag (i.e.
only on a version-tag push, never on a plain push to `main`), and `type=edge,branch=main`
only fires when the active ref genuinely is `refs/heads/main` — so a PR run and a tag-push
run each produce exactly the tags they should with no extra `enable:`/`if:` conditions
needed. Both facts were confirmed against `docker/metadata-action`'s own README rather than
assumed. Build layers are cached via `type=gha`, shared across runs the same way
`checks.yml`'s Rust jobs already cache `~/.cargo`/`target`.

**Validated locally end-to-end before ever touching CI.** Built the image from a clean
checkout; ran it with a published port and confirmed `/api/health`, `/`, and
`/api/openapi.json` all return real content (not the SPA-fallback 503 a broken
`rust-embed` build would produce); confirmed the SPA's hashed JS/CSS assets carry
`Cache-Control: public, max-age=31536000, immutable` while `/` does not; confirmed a
client-side route (`/runs/new`) still serves the SPA shell rather than 404ing; confirmed
the SQLite database file is created under `/var/lib/bhtune/`, owned by the non-root
`bhtune` user; and confirmed `docker exec ... bhtune template list` (the CLI binary) reads
the same database the running server just seeded, proving both binaries share
`BHTUNE_DB` correctly inside the container.

**`provenance: false`/`sbom: false` on the `build-push-action` step, added after a real
user-visible artifact.** `docker/build-push-action` has attached a build-provenance
attestation as an extra manifest inside the pushed image index by default since v4 — that
manifest carries no real OS/architecture, so the GHCR package page's UI renders it as a
fake `unknown/unknown` platform entry alongside the real `linux/amd64` one (confirmed via
GitHub's own community discussion #45969: a known GHCR-UI-only cosmetic quirk, not present
the same way on Docker Hub). Purely cosmetic — `docker pull`/`run` always resolve the real
platform regardless — but confusing enough to ask about, so it's suppressed rather than
left for the next person to wonder about. Verified by fetching the GHCR package page
before and after: the tag pushed before this change shows two manifest entries, the tag
pushed after shows exactly one.

## `pkg-evaluate-others`: the remaining distribution channels

Evaluated the "nearly free" and "moderate effort" channels from the packaging shortlist
(see "Key architectural decisions") and shipped three of them; the fourth (Homebrew) is
prepared but deliberately not yet activated. winget remains out of scope for now (see
below).

**`.deb` and `.rpm`, both built from the same `[package.metadata.*]` blocks on
`crates/bhtune-cli/Cargo.toml`, the same asset set as the Docker image and the release
archives: both binaries, man pages, shell completions, and the `bhtune-server` systemd
unit.** `cargo-deb` builds the `.deb`; `cargo-generate-rpm` builds the `.rpm`. One package
per format, not per binary, for the same reason as the Docker image and the release
archive: there is no GUI-toolkit dependency to keep off a headless install anymore, so
splitting the CLI and the server apart would only add packaging work for no benefit.

**Neither tool builds or strips the binaries itself, unlike `cargo-deb`'s own defaults in
other invocation modes** — both are invoked with pre-built, pre-stripped release binaries
already sitting at `target/release/`, confirmed by testing: `cargo-deb --no-build` and
`cargo generate-rpm` (which has no build step at all, ever, in any invocation) both simply
read whatever is already on disk.

**Path resolution is a real, easy-to-get-wrong difference between the two tools.**
`cargo-deb`'s relative asset paths resolve against _the crate's own manifest directory_
(`crates/bhtune-cli/`), hence the `../../` prefixes on every path in its `assets` block
that reaches outside that directory. `cargo-generate-rpm`'s relative paths resolve against
_the current working directory first_, falling back to the crate directory only if not
found there (confirmed against its own `generate_expanded_path`/`load_script_if_path`
source) — since it's invoked from the workspace root (matching `release.yml`'s actual
invocation), every path in its `assets` block is written workspace-root-relative with no
`../../` prefix at all. Mixing the two conventions up produces a tool that runs without
error but silently packages the wrong files (or none), so this was verified by building
and manually inspecting the contents of both a real `.deb` and a real `.rpm` file — not
just by reading the source.

**`cargo-generate-rpm -p` is a path, not a crate name, despite its own `--help` text
saying otherwise** ("Name of a crate in the workspace") — confirmed in its source
(`Config::new(Path::new(p), ...)` joins the argument directly with `Cargo.toml`). It must
be invoked as `cargo generate-rpm -p crates/bhtune-cli`, not `-p bhtune-cli` (the latter
fails with "No such file or directory"). `cargo-deb -p`, by contrast, really is a crate
name, matching its own `--help` text correctly.

**Neither tool needs a hand-written `dpkg-shlibdeps`/`find-requires` step for shared
library dependencies, but for different reasons.** `cargo-deb`'s `depends = "$auto"` calls
out to Debian's own `dpkg-shlibdeps`, which is standard tooling on any real Debian/Ubuntu
build host (including `ubuntu-latest` GitHub runners) even though it isn't present on
every development sandbox. `cargo-generate-rpm`'s default `auto-req = "auto"` mode instead
uses the Rust `rpm` crate's own built-in ELF scanner when no external `find-requires`
script is present — confirmed by inspecting a built test package's `requirename` header,
which correctly listed versioned `glibc`/`libgcc_s`/`libm`/`ld-linux` requirements with
zero extra configuration.

**`cargo-generate-rpm` has no automatic systemd-unit lifecycle integration (no
`dh_installsystemd` equivalent), unlike `cargo-deb`'s `[package.metadata.deb.systemd-units]`
table.** The unit file is just a plain asset in the `.rpm` case; enabling/disabling/
restarting it across install/upgrade/removal is hand-scripted via
`post_install_script`/`pre_uninstall_script`/`post_uninstall_script`, using the classic,
portable `systemctl preset`/`disable`/`stop`/`daemon-reload`/`try-restart` form (not the
newer `systemd-update-helper`-delegating rewrite some distros' RPM macros now expand to,
since that helper's presence isn't guaranteed across every RPM-based distro) — the same
shell these macros have expanded to for years, confirmed against systemd upstream's own
`macros.systemd.in`. RPM's `$1` scriptlet argument conventions (install vs. upgrade vs.
final removal) follow the Fedora Packaging Guidelines' Scriptlets page exactly.

**`cargo-generate-rpm`'s `-o <dir>/` does not create a missing output directory itself,
unlike `cargo-deb`'s `-o <dir>/`.** Caught by actually dispatching `release.yml` in CI, not
by local testing alone — local testing happened to always pre-create the output
directory, masking the bug. A trailing-slash path that doesn't exist yet makes the tool
treat it as a literal (non-existent) _file_ target rather than a directory to create,
failing with `Is a directory (os error 21)` once the OS's own trailing-slash-implies-
directory rule kicks in. Fixed with a plain `mkdir -p` immediately before the
`cargo generate-rpm` invocation in `release.yml`.

**`release.yml`'s new `package-deb-rpm` job is deliberately separate from `build`'s
existing per-platform matrix, not extra steps on `build`'s Linux leg, because of where
`upload-rust-binary-action` actually leaves its build output.** That action always passes
an explicit `target:` input, so — confirmed by reading its `main.sh` — it always builds via
`cargo build --target x86_64-unknown-linux-gnu`, leaving binaries under
`target/x86_64-unknown-linux-gnu/release/`, not the plain `target/release/` the Cargo.toml
packaging blocks above assume (and that local testing used). Rather than adjust the
asset-path convention to depend on cross-compilation-target-dir details, `package-deb-rpm`
does its own untargeted `cargo build --release` — one extra compile, cheap next to the
existing three-platform matrix, that keeps the already-tested asset paths correct with no
cross-compilation assumptions at all. It builds and packages on every trigger
(`workflow_dispatch` dry run or a real tag push), matching `build`'s own dry-run
convention, and uploads the two files to the release only on a real tag push, via a plain
`gh release upload` — no additional third-party action needed for two files.

**`cargo-deb` installs from a prebuilt binary via `taiki-e/install-action`; `cargo-
generate-rpm` does not and is built from source via `cargo install --locked` instead** —
confirmed against its GitHub Releases, which carry no binary assets at all, only source
tags.

**`[package.metadata.binstall]`, added to `bhtune-cli` only, not `bhtune-server`.** Inert
until `bhtune-cli` is actually published to crates.io (`release-plz.toml` has
`publish = false` workspace-wide, pending `release-v1`), but ready the moment it is, since
`cargo binstall` only reads this from a manifest it's already fetched — no separate opt-in
step needed later. `bhtune-server` is deliberately excluded: it has its own
`publish = false` and is meant to be installed as a system service via the OS packages
above, not fetched by `cargo install`/`cargo binstall` as a CLI tool. Two details had to
match `release.yml`'s actual archive layout exactly, both confirmed against
`taiki-e/upload-rust-binary-action`'s own README rather than assumed: `bin-dir = "{ bin
}{ binary-ext }"` is flat, with no wrapping subdirectory, because that action's
`leading-dir` input defaults to `false`; and `pkg-url` hardcodes a literal `v` before every
`{ version }` reference, because binstall has no `{ tag }` template variable at all, only
the bare, unprefixed crate version — exactly the pattern binstall's own docs show for a
project with `v`-prefixed tags like this one's.

**A prepared-but-inert Homebrew formula, `packaging/homebrew/bhtune.rb`, deliberately not
yet wired to a real tap repo.** Standing up `bytehound-labs/homebrew-bhtune` and computing
real release checksums is deferred until closer to v1 — matching the "moderate effort"
tier in "Key architectural decisions" — but the formula content itself costs nothing to
write and review now. Supports only the two platforms the release matrix actually
produces (Linux x86_64, macOS arm64); there is no Intel Mac or Linux ARM archive to point
a formula at. Installs only the two binaries plus `LICENSE`/`README.md`, matching exactly
what's in the release archive today — man pages and shell completions are deliberately
left out rather than widening `release.yml`'s `include:` list to add them, since that
input has no glob-pattern support (confirmed in `upload-rust-binary-action`'s own
`action.yml`) and would need every one of `docs-generated-cli`'s auto-generated man pages
named individually, which would drift out of sync with the entire point of generating
them.

**winget remains out of scope.** It requires PR-ing a manifest into Microsoft's community
repo on every single release, which only makes sense once `pkg-windows-installer`'s MSI is
itself stable — revisit then, not before.

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

## Correctness-critical design details (also the legacy bug register, `core-bug-register`)

These are easy to get subtly wrong, so they're called out explicitly. Each should have direct
unit-test coverage, not just be caught incidentally by a golden-master replay fixture. This list
also **is** `core-bug-register`'s deliverable: every legacy defect found during the migration is
covered somewhere below with an explicit replicate-or-fix decision, tagged with one of:

- **`[fixed, compat flag available]`** — the correct behavior ships by default; the old, buggy
  behavior can still be reproduced on demand via a `*Compat` struct field, for bug-for-bug replay
  against a legacy trace if one is ever needed.
- **`[fixed, no flag needed]`** — the correct behavior ships and the bug has no legitimate reason
  to ever be reproduced (it was pure defect, never a documented or relied-upon behavior).
- **`[structurally impossible]`** — stronger than "fixed": the bug's precondition cannot occur in
  the new design at all (a compile-time guarantee or a data structure that can't represent the
  invalid state), not merely "we were careful this time."
- **`[not applicable — feature dropped]`** — the subsystem the bug lived in (licensing,
  loop-locking, log encryption) was not ported at all, per the plan's locked decisions, so the
  bug has nothing to attach to.
- **`[preserved rule]`** — not a bug: a real legacy behavior that must be kept exactly as-is,
  included here because it is just as easy to accidentally break as an actual defect.

1. **`[fixed, compat flag available]` The MV boundary clamp must be dimensionally consistent on
   both sides.** If the relay step
   down would drive MV below its configured floor, clamp the relay amplitude to
   `MvValueIni - MvLowerRange` (the actual distance from the initial value down to the floor), not
   an expression that adds the floor back onto the initial value. Get this wrong and cascaded
   loops with a non-zero MV floor get an incorrect (usually oversized) relay amplitude — it's
   silently masked whenever the floor is 0 (the common 0–100% case), which makes it easy to miss
   in testing. Legacy: `CheckMVboundaries`, `OPCClass.cs` ~line 355. Fixed by default in
   `core-mrft`; replicable via `MrftCompat.replicate_lower_clamp_bug`.
2. **`[fixed, compat flag available]` The MRFT oscillation period must use full-precision elapsed
   time** (total seconds as a
   floating-point value), never truncated into separate hour/minute/second integer components and
   reassembled — that discards sub-second precision and wraps incorrectly past 24 hours. This
   matters most on fast loops (flow/pressure) where the whole oscillation period is only a few
   seconds, so truncation error is a large fraction of the signal, not noise. Legacy:
   `CalculatedMRFTperiodMinutes`, `OPCClass.cs` ~line 743 (uses `TimeSpan.Seconds`, an
   integer-truncating property, instead of `.TotalSeconds`). Fixed by default in
   `core-tuning-math`; replicable via `TuningMathCompat.replicate_period_truncation_bug`.
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
   sub-second-precision inputs, not just whole-second ones, to actually enforce it. Separately,
   `core-replay-harness`'s `flow_pi_direct` fixture needed its own dedicated, narrower
   `PERIOD_TOLERANCE_MINUTES` for exactly this reason — see "Validation strategy" above.
3. **`[structurally impossible]` Switch timestamps must reuse the already-captured tick
   timestamp**, never a fresh wall-clock
   read at the moment a switch is performed — the two can differ by however long evaluation took,
   which is small but non-deterministic and breaks exact replay comparison. Legacy:
   `MRFTperformSwitch`, `OPCClass.cs` ~line 430 (stores `DateTime.Now` instead of the tick's own
   `TimeCurrent`). Stronger than a default-off compat flag: `chrono`'s `clock`/`now` features are
   disabled workspace-wide, so `bhtune-core` cannot call `Utc::now()` even by accident — verified
   by temporarily adding such a call and confirming it fails to compile (see `driver-opcda`'s
   notes above for where this was re-verified after `opcda-bridge`/`tonic` entered the dependency
   graph).
4. **`[structurally impossible]` Lookup tables must be sized to exactly the number of process
   types that exist (6)** — no
   extra, unreachable rows/columns in the tuning-constant or default-cycle data. Legacy:
   `matrixCyclesSkip`/`matrixCyclesTest`/`matrixNoiseProt` each held 7 elements against only 6
   process types in the dropdown, leaving the 7th permanently unreachable. `ProcessType::ALL` is a
   `[ProcessType; 6]` and every lookup table in `constants.rs` is a plain `[T; 6]` array — the
   array length and the enum's variant count are the same 6 by construction, and
   `constants.rs`'s own tests assert `.len() == 6` on each table, so a 7th row could not silently
   exist even as an authoring mistake.
5. **`[preserved rule]` If a CSV/tabular export format is ever added**, generate the header and
   each data row's
   column order from the same single ordered list of field names — never maintain them as two
   independently hand-written strings; that's exactly the kind of thing that silently drifts out
   of sync. Legacy: Step Test's dynamic CSV log wrote the header `Time,PV,SV,MV,P,I,D` but the
   data rows as `Time,PV,MV,SV,P,I,D` — the MV and SV columns transposed. Not yet applicable to
   bhtune (Step Test is a deferred phase, per the plan's locked decisions), but recorded here so
   the eventual port doesn't repeat it; `bhtune-cli`'s existing `export.rs` (CSV/JSON of one run's
   samples) already follows the single-source-of-truth pattern this item calls for.
6. **`[fixed, no flag needed]` PID unit labels (Kp vs. PB; Ti vs. Ri vs. Ki; Td vs. Kd) must
   refresh on every relevant state
   change** — process-type change, template switch, and app startup — not only from a single
   settings-changed event handler. A partial refresh trigger is an easy way to end up with stale
   unit labels on a results screen. Legacy: `UpdateAllPIDlabels()` was only wired to the
   PropertyGrid's change handler, never called at startup or on template switch. Not applicable in
   the same form in bhtune's web frontend — React re-derives unit labels from component state on
   every render, so there is no separate "refresh" step that can be forgotten — but the underlying
   rule (labels must always reflect current process type/template, not a stale cached value) still
   held during `frontend-screens`/`template-cli` and is worth remembering if a non-React adapter
   is ever added.
7. **`[fixed, no flag needed]` Tag-name derivation from a single PV tag must use the active
   DCS/PLC template's own
   configured suffix convention, never a hardcoded literal** — different DCS/PLC families name
   their PV item differently (e.g. a `.PV` dot-suffix convention vs. no such convention at all).
   Legacy: `-t`/`--tagname` unconditionally appended the literal `".PV"`. Fixed: `bhtune-core`'s
   `derive_tag(pv_tag, suffix)` takes the suffix from the active `DcsTemplate`'s own
   `process_variable_suffix` field (and the equivalent field for every other derived tag) — the
   convention is template data, never Rust logic.
8. **`[fixed, no flag needed]` Relay amplitude needs real, enforced range validation at the
   model/construction level** — not
   just client-side keystroke filtering plus a single "not blank" check. An unvalidated numeric
   field that only rejects blanks is exactly how a nonsensical value reaches a live control loop.
   Legacy: the hidden debug codes `2014`/`2015`/`2016` typed into the Relay Amplitude box were
   validated only as "not blank", so a leftover debug code could become a 2014% relay step. Fixed:
   the debug codes were dropped entirely rather than ported (see item 10 below), and
   `safety-validation`/`LoopConfig::validate()` enforces real bounds on `relay_amp_percent` at
   construction time regardless.
9. **`[fixed, no flag needed]` Any file export feature must write to a path the user explicitly
   chooses, or a documented
   platform-standard data directory** — never an implicit hardcoded path or "wherever the process
   happened to start". Legacy: `TuningConstantsExport()` wrote to a hardcoded developer path
   (`C:\Dropbox\Auto-Tuner Proj\...`); `LogLoopLocking()` wrote to the current working directory
   rather than the log directory. Fixed: `bhtune-cli`'s `export`/`history export` commands take an
   explicit `--output <path>` (or write structured data to stdout for piping), and logging
   (`cli-logging`) resolves its directory through the normal config precedence, defaulting to a
   documented platform-standard data directory — never an implicit/hardcoded path.
10. **`[fixed, no flag needed]` Test/demo mode must be a first-class, explicit driver choice**
    (e.g. `--driver
simulator`), never triggered implicitly by a magic tag name or hidden UI state — an implicit
    trigger is surprising and easy to leave enabled accidentally. Legacy: `OPCClass.Python` gated
    a hardcoded branch triggered by typing the magic tag name `Simulink.Device1.Python.PV`, which
    also **returned early from `ResetOPC`, skipping all DCS mode-revert logic**, and shelled out to
    a hardcoded `RunModel.bat` path while blocking the UI thread for 7 seconds. Fixed:
    `driver-simulator`'s `SimulatorDriver` is selected explicitly (`--driver simulator`), is a
    real, in-process, non-blocking FOPDT model with no shell-out, and shares the exact same
    restore/mode-revert path every other driver uses — there is no special-cased early return.
11. **`[fixed, no flag needed]` PID-type selection must be modeled as proper enums**
    (`ProportionalType`, `IntegralType`,
    `DerivativeType`, controller action direction, etc.), never as comparisons against magic
    display strings or sentinel values. Legacy: PID type selection compared against display
    strings (`"Kp - Proportional Gain"`, `"Ti - Reset Time"`, `"Ri - Reset Rate"`, `"Ki - Reset
Gain"`, `"Td - Derivative Time"`, `"Kd - Derivative Gain"`, `"Seconds"`), and a sentinel string
    `"__reverse__"` forced a mismatch against `ControllerActionDirect` when the user selected
    Reverse manually. Fixed: `core-model` ported these as proper `serde`-backed Rust enums
    (`ControllerDirection`, etc.) decoupled from any UI display label.
12. **`[preserved rule]` PID is only offered for the two Temperature process types**; every other
    process type offers
    only P and PI. This is a deliberate domain rule (rooted in which tuning-constant columns are
    actually calibrated), not an arbitrary restriction to relax. Preserved as
    `ProcessType::allows_pid()`.
13. **`[preserved rule]` Skip/count/noise-protection defaults are auto-populated per process type**
    from lookup
    tables whenever the process type changes. Preserved via `constants.rs`'s
    `DEFAULT_CYCLES_SKIP`/`DEFAULT_CYCLES_TEST`/`DEFAULT_NOISE_PROTECTION_SECS` tables (see item 4
    above for their sizing).
14. **`[preserved rule]` On the final MRFT step, MV snaps back to the initial value** rather than
    taking a full relay
    step. Preserved in `core-mrft`'s `MrftEngine::step`.
15. **`[fixed by design in this project, not a compat concern]` Significant-digit display
    formatting needs care.** Naive numeric rounding to N digits is not
    the same as significant-digit formatting (e.g. `0.00123` vs. `123000` both have 3 significant
    digits but very different rounding behavior). Decide up front whether exact significant-digit
    formatting matters for a given field or whether straightforward rounding is an acceptable,
    documented simplification for display-only purposes — don't assume the two are
    interchangeable. Legacy: `FormatSigDigs` implemented significant-digit rounding via string
    formatting for on-screen values. Not applicable in the same form: display formatting is now
    entirely the web frontend's concern (plain `toFixed`-style rounding in React/TypeScript, e.g.
    `RunDetailPage.tsx`), not something `bhtune-core` computes or stores — there is no
    calculated/persisted value this affects, only how a number is rendered, so exact legacy
    parity was judged not worth replicating here.
16. **`[new feature, not a legacy bug]` A live PV/MV trend chart is a core UX expectation for the
    web GUI** — plan for high-rate
    streaming updates (multiple times per second) from the start; see "Chart library" below. The
    legacy app never had a trend chart at all (`Telerik.WinControls.ChartView` was referenced in
    the `.csproj` but no chart control was ever built), so this is new scope, not parity work —
    shipped via `frontend-live-stream`'s `TrendChart` (uPlot).
17. **`[not applicable — feature dropped]` A licensing/loop-locking ledger's connection-open
    logic must handle a missing database file without throwing from an unobserved async task.**
    Legacy: `SQLock.CheckDB()` called `.Open()` on a `null` `SQLiteConnection` whenever
    `SQLock.db` was absent, so a genuinely fresh install could throw inside a fire-and-forget task
    nobody awaited. Moot: bhtune has no license-gated loop-locking ledger at all — `bhtune-db`'s
    SQLite schema (`db-schema`) was designed plain from the start (see `db-drop-legacy`), so there
    is no `SQLock`-equivalent connection-open path that could reproduce this.
18. **`[not applicable — feature dropped]` Log "encryption" and a login gate must provide genuine
    protection, or not exist at all.** Legacy: logs were "encrypted" with AES-GCM using the
    password `"imbcontrols2016"` hardcoded in the shipped binary — trivially reversible by anyone
    holding the exe — and `Login.cs` (which shared the same hardcoded password) was itself dead
    code, since `Program.cs` ran `MainForm` directly and never instantiated the login form, making
    the documented `--unlockApp` flag a literal no-op. Moot: per the plan's locked decisions,
    bhtune ships no log encryption and no login gate at all — logs and the database are plain,
    matching the "no need to obfuscate/encrypt/hide anything" requirement — so there is no
    encryption or auth subsystem left to get subtly wrong in this way.

## Documentation contract (`docs-contract`)

A documentation update is part of the definition-of-done for any change that alters
user-visible behavior — a new CLI flag or subcommand, a config key, an HTTP endpoint, a
default value, an error message a user would act on, a template/catalog field, or a safety
rule. There is no dedicated "catch up on docs" phase later; drift that isn't fixed in the same
change tends to never get fixed.

What to update, in order of how much it costs to get wrong:

1. **Generated references** (the CLI reference/man pages/completions via `clap`, the OpenAPI
   spec, the generated TS client, the `bhtune.toml`/template-catalog JSON Schema) are never
   hand-edited and never go stale by definition — the build regenerates them and CI's `git
diff --exit-code` gates fail if a commit forgets to include the regenerated output.
   Nothing to remember here beyond running the generator before committing.
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
4. **`docs/`** (prose guides, `docs/dcs-templates.md`, `docs/internal/v1-checklist.md`) and
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
- **No CLA-enforcement bot wired up yet.** `CLA.md` is a draft naming ByteHound Corp. as the
  entity; it does not bind anyone until the text has had a legal review and a CLA-assistant check
  is added to the PR checks.

## Cross-project CI/CD audit (`cross-project-ci-audit`, done)

Compared `bhtune`'s CI/CD, lefthook, and repo-hygiene setup against the sibling
`opcda-bridge` project in both directions. The flow was overwhelmingly one-directional
(`opcda-bridge` → `bhtune`): `opcda-bridge` is the more mature project and already embodied
the practices below before this audit — `homepage`, `cargo machete`, `cargo deny`, and its
own `windows`/`msrv`/`package` CI jobs all predate this audit on that side. Checked
specifically for anything worth proposing back the other way and found nothing of
substance beyond what's noted below; both repos came out of this audit with equivalent
Dependabot/security posture instead.

Pulled into `bhtune` from `opcda-bridge`'s example:

- **MSRV declared and enforced.** `rust-version = "1.94"` in `[workspace.package]`
  (empirically determined — `sqlx@0.9.0` requires it, higher than `opcda-bridge`'s own 1.88
  floor), with a standalone `msrv` CI job pinning `dtolnay/rust-toolchain@1.94.0` and running
  `cargo check --workspace --all-targets --all-features --locked`.
- **`windows` and `package` CI jobs.** `windows` runs fmt/clippy/test on `windows-latest`
  (skipping the Linux-only doc/OpenAPI drift `git diff` checks, which are CRLF-sensitive);
  `package` runs `cargo package --workspace --locked`, which immediately surfaced a real bug —
  every workspace-internal path dependency lacked a `version` requirement, which `cargo
package` refuses to package. Fixed by giving `bhtune-core`/`bhtune-driver`/`bhtune-db`/
  `bhtune-cli` `{ path, version }` entries in `[workspace.dependencies]` and switching every
  consumer to `.workspace = true`, mirroring `opcda-bridge`'s own already-working pattern
  exactly.
- **`concurrency` groups, `permissions: contents: read`, and `--locked` everywhere** across
  `checks.yml`/`coverage.yml`/`e2e.yml`.
- **`.github/dependabot.yml`** — weekly grouped updates for `cargo`, `npm` (pnpm workspace
  root, covering both root and `frontend/package.json`), and `github-actions`, each labeled
  (`dependencies`/`rust`/`frontend`/`ci` — created on the repo, since Dependabot silently
  skips labels that don't already exist).
- **Branch protection on `main`** — required status checks naming every job context
  (`check`, `frontend`, `Windows validation`, `MSRV`, `Package verification`, `coverage`,
  `e2e`), `strict: false`, `enforce_admins: false` (preserves the existing direct-push
  workflow), `allow_force_pushes`/`allow_deletions: false`. `opcda-bridge`'s own rule only
  requires `check`+`coverage` because its `checks.yml` uses a `dorny/paths-filter` +
  aggregator-gate structure where `check` is a final job that gates on `windows`/`msrv`/
  `package` all having passed — `bhtune`'s five jobs are independent with no such
  aggregator, so equivalent protection means requiring all of them individually.
- **Secret scanning + push protection enabled** on both `bytehound-labs/bhtune` and
  `bytehound-labs/opcda-bridge` (both public repositories) — confirmed disabled on both
  before this audit. `secret_scanning_validity_checks` did not take via the API on either
  repo despite repeated attempts (`secret_scanning`/`secret_scanning_push_protection` both
  enabled fine) — likely an org/plan-gated setting; low priority, flip manually in the repo
  Settings UI if wanted.

Ported from `bhtune` to `opcda-bridge` (the one item that went the other way, discovered
while auditing rather than pre-existing on either side): **`.github/dependabot.yml`** for
`cargo` + `github-actions` (no `npm` — pure Rust workspace, no frontend), with matching
`dependencies`/`rust`/`ci` labels created using the same colors as `bhtune`'s.

The **CLA-enforcement bot** remains a separate pre-existing gap, tracked under "Deferred setup"
below, not part of this audit's CI/CD scope.

**Follow-up, implemented later:** **CODEOWNERS, issue templates, and a PR template** — a
shared gap on both repos, not something to port one way — were added to both
(`.github/CODEOWNERS`; `.github/ISSUE_TEMPLATE/{bug_report,feature_request,config}.yml`;
`.github/pull_request_template.md`), each adapted to its own project's conventions rather
than copy-pasted: `bhtune`'s PR template checklist includes the frontend lint/typecheck
commands and the CLA-sign-off line from `CONTRIBUTING.md`; `opcda-bridge`'s omits both (no
frontend, no CLA — MIT, no CLA required) and instead asks for hardware-in-the-loop manual
verification notes, matching its own `CONTRIBUTING.md`'s "no live OPC DA server in CI" line.
Both bug report forms ask for a version and platform; `bhtune`'s adds a `Driver` dropdown
(OPC DA/simulator/replay) since that's a core `bhtune-driver` concept a maintainer would
otherwise have to ask about, and `opcda-bridge`'s adds an OPC DA server vendor field instead,
plus a note that the gateway crate is Windows-only. Neither repo has GitHub Discussions
enabled (confirmed via `gh api repos/.../{repo}` before writing `config.yml`), so
`blank_issues_enabled: true` with no `contact_links` was the right shape for both — forcing
every report into a rigid form when there's nowhere else to ask would be worse than a
free-form issue.

## Workflow and release hardening (`security-workflows`, `ci-efficiency`, `release-attestations`,

`parser-property-tests`, `api-migration-compatibility`, done)

The repository now has a layered hardening gate for both source changes and release outputs:

- **Security analysis.** `.github/workflows/codeql.yml` builds the Rust and JavaScript/
  TypeScript targets for CodeQL; `semgrep.yml` runs the Rust and TypeScript community rules;
  `gitleaks.yml` scans the complete git history on every relevant change and weekly; and
  `security-lint.yml` runs actionlint plus zizmor against every workflow change. These use
  ordinary `pull_request` events, `persist-credentials: false` wherever checkout does not need
  to push, least-privilege permissions, immutable action commit pins, and explicit job
  timeouts. The docs agent has two narrow, documented zizmor exceptions: its authenticated
  checkout must retain credentials to push its reviewed prose commit, and its isolated,
  version-pinned Copilot CLI install cannot use a repository lockfile.
- **Change-aware CI.** `checks.yml`, `coverage.yml`, and `e2e.yml` use
  `dorny/paths-filter` to skip unrelated work and finish with an always-running aggregator
  status, so branch protection still receives one deterministic result when a fan-out job is
  intentionally skipped. Every major workflow job has a `timeout-minutes` bound.
- **Release integrity.** Docker builds publish provenance and SBOM attestations. Tagged
  releases download their archives/packages into a dedicated supply-chain job, generate a
  CycloneDX SBOM and checksums, create GitHub artifact provenance attestations, and sign every
  release asset with keyless Cosign bundles before uploading the evidence beside the assets.
- **Parser resilience.** `proptest` tests cover config serialization/parsing, template TOML/
  JSON, OPC bridge payload mappings, and template imports. The separate `fuzz/` Cargo-fuzz
  package has targets for each of those byte-stream boundaries without becoming a workspace
  runtime dependency.
- **API compatibility.** `scripts/check_openapi_breaking.py` is a dependency-free comparison
  for removed operations/responses/properties/enum values, newly required request fields, and
  newly mandatory authentication. Its unit tests run in CI, and pull requests compare the
  generated revision to the base branch in addition to the existing drift check.
- **Database compatibility.** Migration `0002_history_query_indexes.sql` is the first
  forward migration after the initial schema. `pool.rs` constructs a representative database
  at migration 0001, inserts data, opens it through the normal connection path, and verifies
  both preservation and application of the new indexes. Future schema changes should extend
  this pattern rather than editing an already-applied migration.

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

| Crate              | Phase                                                                                                                                                                                                                                       | Status                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bhtune-core`      | `core-model`/`core-mrft`/`core-tuning-math`/`template-catalog`/`core-replay-harness`                                                                                                                                                        | All done. `core-model` + `core-mrft` + `core-tuning-math` + `template-catalog` (the four built-in DCS templates now parse from an embedded, contributable TOML catalog — see "Community DCS/PLC template catalog" below); `core-replay-harness` (`crates/bhtune-core/tests/golden_replay.rs`) replays the first real captured trace (`tests/golden/fixtures/flow_pi_direct.json`) tick-by-tick through a real `MrftEngine` plus `calculate_all`, and asserts exact behavioral parity with the legacy C# app — see "Validation strategy: golden-master replay" below for the fixture-reconstruction subtleties this surfaced (a whole-second timestamp precision ceiling in the legacy CSV logger, and the already-known period-truncation bug independently corroborating the fixture's own recorded values)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `bhtune-driver`    | `driver-trait`/`driver-opcda`/`driver-simulator`/`driver-replay`/`driver-list-servers`                                                                                                                                                      | All four done (trait, error model, OPC DA implementation, FOPDT simulator, and trace-driven replay driver, all tested) — Phase 4 complete. `driver-replay`'s `ReplayDriver` (`replay.rs`) is validation-only: it feeds a recorded trace or the real golden-fixture JSON through the `Driver` trait, but has no CLI-selectable driver kind, since it exists to prove the trait abstraction itself is correct, not to drive a live tune — see "Replay driver reference (`driver-replay`)" below. `driver-list-servers` (Phase 7.5) is also done: `opcda::list_opcda_servers(bridge_host)`, a standalone pre-connection free function (not a `Driver`/`OpcDaDriver` method) for OPC DA server discovery, exposed via the CLI as `bhtune opc servers` — see the Status section above for the full rationale                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `bhtune-db`        | `db-schema`/`db-seed-templates`/`history-query-api`/`db-backup-restore`/`template-provenance`/`history-retention`                                                                                                                           | All done (7 tables, tested; 4 templates auto-seed on startup; run-history repository layer with lifecycle, filtering, and pagination, now also `TuneRunRow::delete_matching` for age-based retention sweeps; whole-database backup/restore via `VACUUM INTO`, hardened with an exclusive-access requirement by `safety-db-restore`; `dcs_templates` gained a real three-way `origin` column plus `versions_json`/`description`/`source` — see "Live-plant safety hardening" and "Community DCS/PLC template catalog" below)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| `bhtune-cli`       | `cli-commands`/`cli-config`/`cli-automation`/`cli-safety`/`cli-logging`/`template-user-catalog`/`template-cli`/`docs-generated-cli`/`history-retention`/`history-cli`/`driver-list-servers`                                                 | All five sub-phases done (subcommands, see "CLI reference" above; `CLI > env > TOML > default` config precedence, see "Config precedence" above; `--yes`/`--write-pid`/`--output json` and distinguished exit codes, see "Automation" above; relay-amp validation and mandatory `--timeout-secs`, see "Safety" above; `tracing` file+stderr logging, see "Logging" above) — a fully headless, scriptable CLI, no server required. The Phase 6.5 live-plant safety hardening pass following a post-`cli-logging` review is also done; see "Live-plant safety hardening" below. `template-user-catalog` (Phase 6.6) is also done: auto-loads a user catalog file on startup via the same config precedence chain — see "Auto-loading a user template catalog" above. `template-cli` is also done: multi-template TOML import/export and `template delete` — see "Multi-template import, TOML export, and `template delete`" above. `docs-generated-cli` (Phase 9) is also done: `examples/gen_docs.rs` regenerates the CLI reference, man pages, shell completions, and the `bhtune.toml`/template-catalog JSON Schema from the same `clap`/`serde` definitions, drift-gated in CI — see "`docs-generated-cli`: generating the CLI reference, man pages, completions, and config schema" above. `history-retention` (Phase 10) is also done: a new `retention` module (`cutoff_for`/`sweep_retention`) shared by the startup sweep, `bhtune-server`'s periodic ticker, and `history-cli`'s `prune` below, resolved through `resolve_retention_days`'s usual config precedence. `history-cli` is also done: `bhtune history prune` (`--older-than-days`/`--dry-run`/`--output json`) completes the `history` subcommand surface alongside the already-shipped `list`/`show`/`revert`. `driver-list-servers`'s CLI half (Phase 7.5) is also done: `bhtune opc servers [--bridge-host <HOST>]` fills the one gap in the existing `opc read`/`write`/`browse` diagnostic family — see the Status section above                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `bhtune-server`    | `server-http-api`/`openapi-contract`/`server-start-tune-api`/`server-template-update-api`/`server-embed-spa`/`server-windows-service`/`history-retention`/`history-explorer-ui`/`api-post-run-write`/`ui-prefill-last-run`/`api-opc-browse` | `server-http-api` + `openapi-contract` + `server-start-tune-api` + `server-template-update-api` + `server-embed-spa` + `history-retention` + `history-explorer-ui` done — real Axum binary (health/templates full CRUD/history/runs routes, graceful shutdown, shares the CLI's config/db/logging bootstrap), full OpenAPI 3.1 contract (`utoipa` annotations, `ApiDoc` aggregator, `/api/openapi.json`, Scalar UI at `/api/docs`, checked-in spec with a CI diff gate — see "Key architectural decisions" above), `POST /api/runs`/`POST /api/runs/{id}/cancel` starting and cancelling a real tune over HTTP by reusing `bhtune-cli`'s own `prepare()`/`drive()` orchestration — see "`server-start-tune-api`: starting and cancelling a tune over HTTP" below — `PUT /api/templates/{name}` editing an existing `user`-origin template in place (400 on a name mismatch, 404 if unknown, 409 if not user-owned), `GET /api/runs/{id}/export?format=csv\|json` and `DELETE /api/runs/{id}`completing the history explorer (the delete guard checks the run's own DB`outcome`rather than the in-memory`ActiveRun`slot, closing a real race window — see the Status section above), the built SPA embedded directly into the binary via`rust-embed` with an SPA-fallback route, correct MIME types, and long-lived cache headers on hashed assets — see "`server-embed-spa`: embedding the built SPA into the binary" below — and a `spawn_retention_sweeper`background task resweeping every 24 hours (env-var-only config,`BHTUNE_RETENTION_DAYS`, since this binary has no `clap`) that logs and continues on failure rather than crashing the server. `server-windows-service` is also done: a platform-neutral `ServiceDefinition`/`ServiceLifecycle` in `service.rs`, `#[cfg(target_os = "windows")]` glue over the `windows-service` crate for real SCM install/uninstall/start/stop/status, real (non-panicking) non-Windows stubs pointing at the systemd/launchd equivalents, `install`/`uninstall`/`start`/`stop`/`status` subcommands plus a `--config` flag in `cli.rs`, and `main.rs` rewritten as a platform-split dispatcher, with matching `packaging/systemd/`/`packaging/launchd/` unit files — see the Status section above for the full design and how the Windows-only code was verified without a local Windows toolchain. `api-post-run-write` is also done: `POST /api/runs/{id}/write`/`POST /api/runs/{id}/revert` reuse the CLI's own pre-read/write-and-verify/rollback/audit path via a new shared `write_pid_values` orchestrating function, gated by a new `ActiveRun::reserve`/`release` "exclusive reservation" kind so a post-hoc write can never overlap a live tune — see the Status section above. `ui-prefill-last-run`'s backend half is also done: `GET /api/runs/last-request` and a new `RunDetailResponse.original_request` field, both built on a shared `parse_stored_request` helper that degrades to `null`/`None` (with a logged warning) rather than a `500` on an unparseable historical row — see the Status section above. `api-opc-browse` is also done: three new read-only routes, `GET /api/opc/servers`/`GET /api/opc/browse`/`GET /api/opc/read`, backing the GUI's not-yet-built OPC server/tag browser independently of ever having run a tune, each OPC DA call bounded by a new `with_timeout` 30-second-deadline helper, none touching `AppState::active_run` — see the Status section above. All eleven `bhtune-server` sub-phases are now done |
| `frontend/` (pnpm) | `frontend-shell`/`frontend-screens`/`frontend-live-stream`/`history-explorer-ui`/`ui-prefill-last-run`/`ui-opc-browser`                                                                                                                     | All six done — React + TS + Vite + Tailwind CSS v4 SPA (`bhtune-frontend`), TanStack Query, a typed `openapi-fetch` client generated from `openapi.json` with its own CI drift gate, and an npm license-allowlist gate mirroring `cargo-deny` — see "Key architectural decisions" above. Routing shell, Templates (List/Detail/Create/Edit), History (List/Detail, plus export CSV/JSON and delete actions), a combined New Run screen (Connection/Tag-mapping/Test-parameters/Simulator/Write-back in one form, plus run cancellation), and a live PV/MV trend chart (`TrendChart`, uPlot-based, fed by a new SSE `useRunStream` hook while a run is active and by `useRun`'s `samples` once terminal) are all done and manually verified against a real running server. `ui-prefill-last-run`'s frontend half is also done: the New Run form prefills from the newest run's settings (or a specific run's, via "Duplicate this run" on the run detail page) with a "Start from blank" reset and an explanatory note — see the Status section above, including a real same-batch React-effect race condition this surfaced and fixed before it shipped. `ui-opc-browser` is also done: an `OpcServerDiscovery` button/list widget and an `OpcTagBrowserModal` (a lazily-expanding tag tree with a derived-tag-set preview, "Test read", and "Use this tag") wired into the New Run form's OPC DA path, both manually verified end to end against a real server plus a temporary, never-committed mock gRPC gateway — see the Status section above for the full design, including why the preview and the real backend derivation are guaranteed to agree.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| `website/` (pnpm)  | `docs-site-scaffold`/`docs-site-deploy`/`docs-api-rustdoc`/`docs-versioning`                                                                                                                                                                | `docs-site-scaffold` + `docs-site-deploy` + `docs-api-rustdoc` done — a Docusaurus 3 site (`bhtune-website`) whose `docs` plugin points `path` directly at the repo-root `docs/` (not a website-local copy), so the published site and the Markdown read on GitHub can never diverge; `docs/internal/**` is excluded. `docs/intro.md` is the site root (`slug: /` + `routeBasePath: '/'`, no separate marketing homepage); sidebar ordering comes from `sidebar_position` frontmatter and `_category_.json` files already added to `docs/`. Search is `@easyops-cn/docusaurus-search-local` (static, offline, open-source). `onBrokenLinks`/`onBrokenAnchors` are both `'throw'`, giving the site build a real drift gate for free (a CI `website` job runs `format:check`/`lint`/`typecheck`/`build` on every PR — the license-allowlist gate is covered once, workspace-wide, by the `frontend` job's `check:licenses` step). Live at [bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/), published by `docs-deploy.yml` via `actions/deploy-pages` on every `main` push touching `docs/`/`website/`/`crates/**`. The `cargo doc` API reference is published under `/api/` by that same workflow, indexed from `docs/reference/api.md` via a `pathname://` link (not broken-link-checked, since the content only exists after `docs-deploy.yml` runs on `main`, never during a PR's `website` job) — see "`docs-api-rustdoc`: publishing the Rust API reference" above for the full design. Not yet done: release-time version snapshots (`docs-versioning`, deferred until `release-v1`). See "`docs-site-scaffold`: the Docusaurus documentation site" above for the full design.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |

## Phases and todos (roadmap order)

0. **Behavior specification and reference traces** — the v1 feature/acceptance checklist at
   [`docs/internal/v1-checklist.md`](docs/internal/v1-checklist.md) (done); the trace fixture
   normalizer (`scripts/convert_golden_trace.py`) and the first real captured-and-normalized
   trace (`flow_pi_direct`) are done — see "Validation strategy: golden-master replay" above.
   `capture-traces` is deliberately closed at this one trace: it already proves the full
   hp-VM capture workflow end-to-end and unblocked `core-replay-harness`, and the remaining 5
   process types/PID/reverse-action/cascade/skip-count combinations were a judgement call not
   to pursue further — the marginal parity evidence they'd add isn't worth the recurring
   Windows-VM time against higher-priority phases. `cleanup-golden-traces` (deleting the raw
   CSV captures now that parity is proven) is also done — see "Validation strategy:
   golden-master replay" above.
1. **Repository scaffolding** _(this commit)_ — Cargo/pnpm workspaces, license, CLA draft, CI,
   `cargo-deny` open-source dependency gate.
2. **`opcda-bridge` reusable client library** (published upstream) — consumed as a plain
   crates.io dependency (`opcda-bridge = "0.2"`), local to `bhtune-driver`'s own `Cargo.toml`
   (see "Key architectural decisions" for why it stays out of `[workspace.dependencies]`).
3. **`bhtune-core`** — the critical phase. Data model, MRFT state machine, tuning math, and the
   golden-master replay harness are all done, with the correctness-critical details above baked
   in and unit-tested directly, plus one real captured trace now proving exact behavioral parity
   with the legacy C# app (see "Validation strategy: golden-master replay" above).
   `core-bug-register` is also done: every legacy defect found during the migration has an
   explicit replicate-or-fix decision — see "Correctness-critical design details (also the legacy
   bug register, `core-bug-register`)" above, which doubles as that register.
4. **Drivers** — **all done.** The `Driver` trait (`driver-trait`: `read`/`write`/`browse`
   plus `TagId`/`TagValue`/`TagWrite`/`WriteOutcome`/`TagNode`/`DriverError` in `crates/
bhtune-driver`), its OPC DA implementation (`driver-opcda`: `OpcDaDriver` in
   `crates/bhtune-driver/src/opcda.rs`, see "OPC DA integration reference" above), its
   in-Rust FOPDT simulator (`driver-simulator`: `SimulatorDriver`/`FopdtProcess`/
   `VirtualPid` in `crates/bhtune-driver/src/simulator.rs`, see "Simulator driver reference"
   above), and its trace-driven replay driver (`driver-replay`: `ReplayDriver` in
   `crates/bhtune-driver/src/replay.rs`, validation-only — see "Replay driver reference"
   below).
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
   done: a `react-router` routing shell (`AppLayout` nav + health indicator), the Templates
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
   and its manual end-to-end verification. `server-windows-service` is now also done: a
   platform-neutral `ServiceDefinition`/`ServiceLifecycle` in `crates/bhtune-server/src/
service.rs`, `#[cfg(target_os = "windows")]` glue over the `windows-service` crate for
   real SCM install/uninstall/start/stop/status, real (non-panicking) non-Windows stubs
   pointing at the systemd/launchd equivalents, new `install`/`uninstall`/`start`/`stop`/
   `status` subcommands and a `--config` flag in `cli.rs`, and `main.rs` rewritten as a thin
   platform-split dispatcher — see the Status section above for the full design, the
   packaging files it ships (`packaging/systemd/`, `packaging/launchd/`), and its full manual
   verification against a live Service Control Manager on the `hp` Windows host (every
   subcommand, the interactive fallback, and the `--config` gotcha's mitigation, with no
   defects found). This closes out Phase 7
   entirely. Replaces the earlier Tauri desktop GUI
   phase — see "Key architectural decisions" above for the reversal.

   Phase 7.5 (pre-v1 UX and terminology hardening) is under way, ten of eleven sub-phases
   done. `rename-driver` renamed `backend` → `driver` across every crate, the CLI flag, the
   HTTP JSON field, and the `tune_runs` schema, edited into migration `0001` in place since
   nothing had shipped yet. `db-run-request-snapshot` added flat `opc_server`/`bridge_host`
   columns plus a `request_json` snapshot to `tune_runs`, fixing a latent safety bug where
   `history revert` could re-resolve a different DCS server than the run actually used.
   `ui-simulator-greyout`, `ui-friendly-process-names`, and `ui-tune-nav` round out the GUI
   polish: disabling driver-inert form fields, adding display labels for every raw enum
   surfaced in the UI, and making `/runs/new` the default landing route with a "Tune" nav
   item first. `api-post-run-write`/`ui-post-run-write` add `POST /api/runs/{id}/write`/
   `revert` and matching Write/Revert buttons on the run detail page, reusing the CLI's
   existing pre-read/verify/rollback/audit path under a new `ActiveRun::reserve`
   exclusive-reservation lock. `ui-prefill-last-run` seeds the New Run form from the newest
   run's stored request server-side as a compatibility fallback, plus a "Reset to defaults"
   action and a "Duplicate this run" action. The follow-on New Tune draft flow stores every
   editable field except Notes in the app-wide `settings` row at `new_run_draft`, autosaves
   with debounced, serialized `PUT /api/runs/draft` requests, preserves inactive-driver
   values, and gives the explicit precedence `Duplicate this run` → saved draft → newest-run
   snapshot → built-in defaults. Configuration fields are remembered, but Notes is intentionally
   reset to blank for both kinds of prefill so operator context is not copied into a new tune.
   Run identity is
   consistently presented as the **Tag name**; the former
   user-editable run-name override was removed so history cannot hide the submitted tag.
   A mutable nullable `notes` field is stored on each run, included in new-run requests, and
   exposed through `PUT`/`DELETE /api/runs/{id}/notes` for editing or clearing before, during,
   or after a tune. `driver-list-servers` adds OPC DA server discovery as a standalone
   `bhtune_driver::opcda::list_opcda_servers` free function and a `bhtune opc servers`
   subcommand. `api-opc-browse` adds three read-only `bhtune-server` routes (`GET /api/opc/
servers`/`browse`/`read`) backing the GUI OPC browser, each OPC DA call bounded by a
   30-second timeout. `ui-opc-browser` wires those routes into the New Run form: a
   "Browse servers" modal, and a "Browse tags" modal with a lazily-expanding tag
   tree, a derived-tag-set preview (a client-side mirror of the same suffix-derivation
   algorithm the server uses), a live "Test read", and "Select tag" — manually verified
   end to end, including against a real populated tag tree served by a temporary,
   never-committed mock gateway. Remaining: `phase75-docs` (the documentation wrap-up). See
   the Status section above for the full design and verification detail behind each.

8. **End-to-end testing and CI** — `e2e-simulator` is done: a genuine subprocess-level test
   (`crates/bhtune-cli/tests/e2e_simulator.rs`) spawns the real `bhtune tune` binary against the
   simulator driver across a small process/controller-type matrix (all `direction=reverse`, the
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
   disk, no re-embed step needed between runs) over the in-process simulator driver --
   `smoke.spec.ts` (app shell, health indicator, seeded template list, header nav) and
   `tune.spec.ts` (a full tune through `/runs/new` with `e2e_simulator.rs`'s own
   millisecond-scale simulator parameters, asserting sane/ordered rendered Kp/Ti/Td values,
   plus cancelling an in-flight run). `.github/workflows/e2e.yml` builds a debug
   `bhtune-server` and the frontend, installs Chromium, and runs the suite in CI, uploading
   the HTML report on failure. A direct dividend of dropping Tauri: `tauri-driver`/WebDriver would
   have been markedly more fragile in CI than plain Playwright against a real browser.
   `build-matrix` is also done: `.github/workflows/release.yml` builds and packages the
   `bhtune`+`bhtune-server` binaries for Linux/macOS/Windows via
   `taiki-e/create-gh-release-action` + `taiki-e/upload-rust-binary-action` (opcda-bridge's
   own tooling, in place of the originally-planned `cargo-dist`), building the frontend
   first so the release build's `rust-embed` step captures real SPA assets — no Tauri
   bundler or WebView runtime to manage. See "`build-matrix`: the release binary matrix"
   above for the full design. `e2e-golden-ci` is also done — no dedicated workflow step was
   needed: `checks.yml`'s existing `cargo test --workspace` already auto-discovers and runs
   `crates/bhtune-core/tests/golden_replay.rs` on every push/PR to `main`, the same way
   `e2e-simulator`'s subprocess test rides the same step rather than a separate job.
9. **Documentation and release** — two prerequisites are already done, front-loaded ahead of
   the rest of this phase since they're cheap and are what actually prevents drift: a
   documentation contract in this file (`docs-contract`, see "Documentation contract" above)
   and a paired `sessionStart`/`sessionEnd` Copilot CLI hook warning when a session changes
   `crates/**` without touching any documentation surface (`docs-copilot-hook`, see
   `.github/hooks/README.md`). `docs-generated-cli` is also done: the CLI reference, man
   pages, shell completions, and `bhtune.toml`/template-catalog JSON Schema all regenerate
   from the real `clap`/`serde` definitions and are drift-gated in CI — see
   "`docs-generated-cli`: generating the CLI reference, man pages, completions, and config
   schema" above. `docs-readme` is also done: a getting-started guide (installation, CLI and
   web GUI quickstarts, MRFT concepts, safety) under `docs/getting-started/`+`docs/guides/`,
   linked from the README. `docs-site-scaffold` is also done: a Docusaurus 3 site
   (`bhtune-website`) under `website/`, sourcing `docs/` directly — see the `website/` row in
   "Crate map and phase status" above for the full design. `docs-site-deploy` is also done:
   that site is live at
   [bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/), published by
   `docs-deploy.yml` via `actions/deploy-pages`. `docs-roadmap` is also done: `docs/roadmap.md`
   covers OPC UA/Modbus drivers, remote/multi-user access (Phase 11), the Step Test
   subscription-RPC blocker, multi-loop/batch tuning, and the history explorer (including what's
   deliberately _not_ planned — continuous historization and, for now, cross-run comparison) —
   linked from the README's existing compact roadmap section rather than duplicating it.
   `docs-api-rustdoc` is also done: `cargo doc` output is published under `/api/` alongside the
   site, indexed from a hand-written `docs/reference/api.md` — see "`docs-api-rustdoc`:
   publishing the Rust API reference" above for the full design. Packaging's secondary Docker
   channel, `pkg-docker`, is also done: a multi-stage `Dockerfile` plus
   `.github/workflows/docker-publish.yml` build and publish
   `ghcr.io/bytehound-labs/bhtune` on every push to `main` (tagged `edge`) and every version
   tag (tagged with the version and `latest`), and build-only (no push) on every PR — see
   "`pkg-docker`: the Docker image" below for the full design. `docs-agent-ci` is also done:
   `.github/workflows/docs-agent.yml` runs GitHub Copilot CLI headless on PRs touching
   `crates/**` and auto-commits narrative-prose doc updates, guarded against infinite loops,
   scope creep beyond `docs/**`+`README.md`, and fork PRs — see "`docs-agent-ci`: the AI docs
   agent" above for the full guardrail design; not yet validated against a real PR with
   genuine prose drift. `pkg-evaluate-others` is also done: `.deb`/`.rpm` packages (built
   with `cargo-deb`/`cargo-generate-rpm` from the same asset set as the Docker image),
   `cargo-binstall` metadata on `bhtune-cli`, and a prepared-but-inert Homebrew formula —
   see "`pkg-evaluate-others`: the remaining distribution channels" above for the full
   design, including two real tooling gotchas the `.rpm` path surfaced (a path-vs-name
   `-p` flag mismatch, and a missing-output-directory bug only CI itself caught). Remaining:
   release-time
   version snapshots (`docs-versioning`, deferred until `release-v1`), and the rest of
   packaging: `release-v1` itself (v0.1.0 — now technically possible via `build-matrix`'s
   `release.yml`, but cutting the actual first tag is a deliberate call left to the project
   owner, not automatic — see "`build-matrix`: the release binary matrix" above), a
   Windows MSI installer (`pkg-windows-installer`, the primary distribution artifact), and
   `pkg-aur` (already unblocked — it needs the man pages/completions `docs-generated-cli`
   produces plus `build-matrix`'s Linux archive, both done).
10. **History explorer** (low priority, post-v1, done) — mostly a reader of data earlier
    phases already write, so deliberately scheduled after v1. `history-retention` is done:
    age-based deletion of runs older than a configurable number of days, off by default
    (retain forever), swept on startup (both binaries) and every 24 hours by `bhtune-server`
    while it keeps running. `history-cli` is done: `bhtune history list`/`show`/`revert`/
    `prune` give the same data and the same retention policy headless, with `prune --dry-run`
    to preview a sweep on demand. `history-explorer-ui` is done: a filterable/sortable run
    list, a PV/MV trend chart per run (the same `TrendChart`/uPlot component the live view
    uses), the run's full parameters/calculated-constants/write-back-audit trail, and export
    (CSV/JSON download) and delete actions, all on the web GUI's run detail screen. This
    closes out Phase 10 — see `docs/roadmap.md` for what's deliberately left as an
    open-ended roadmap item instead (continuous historization, cross-run comparison/overlay).
11. **Remote and multi-user access** (post-v1) — local accounts with
    session cookies and revocable API tokens (`server-remote-auth`), TLS (`server-tls`), an
    audit log of who ran/wrote what (`server-audit-log`), and OIDC for SSO-managed orgs
    (`server-oidc`). Deferred, not blocking v1's `127.0.0.1`-by-default posture — see "Key
    architectural decisions" above.
12. **Cross-project CI/CD audit** (`cross-project-ci-audit`, done) — compared `bhtune`'s
    CI/CD and repo-hygiene setup against the sibling `opcda-bridge` project in both
    directions: MSRV enforcement, `windows`/`package` CI jobs, `--locked` everywhere,
    Dependabot, branch protection, and secret scanning — see "Cross-project CI/CD audit"
    above for the full writeup. CODEOWNERS, issue templates, and a PR template (a shared gap
    the audit found on both repos) were added to both as a follow-up. Still deferred as
    recommendations rather than implemented: a CLA-enforcement bot and a future review of
    additional workflow hardening beyond the paths-filter/aggregator gate and pinned actions.

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
