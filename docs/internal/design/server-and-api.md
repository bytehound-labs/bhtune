# Server and HTTP API

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Server design decisions

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

## Preparation failure finalization

`prepare()` also finalizes any run row whose follow-up provenance snapshot fails: it records a
`Failed` outcome, falling back to deleting the row if that terminal update cannot be persisted, so
an owned HTTP preparation cannot leak a permanent `Running` row.

## Template update endpoint

`PUT /api/templates/{name}` edits an existing `origin = "user"` template in place — 400 if the
body's `name` doesn't match the path (renames aren't supported; delete and recreate instead), 404 if
no template exists at that name, 409 if the existing row isn't `user`-owned (a `Builtin`/`Catalog`
row would just be discarded by the next startup reseed) — see
`crates/bhtune-server/src/routes/templates.rs`'s `update_template` doc comment for the full
contract.

## Driver rename and run request snapshot

The `backend` → `driver` rename landed across the entire workspace (crate, trait, error types, every
concrete driver, the `--driver` CLI flag, the HTTP/OpenAPI `driver` field, and the frontend), with
migration `0001` edited in place — the crate map and every other section below already use
`driver`/`bhtune-driver` terminology throughout as a result. `db-run-request-snapshot`: `tune_runs`
gained `opc_server`/`bridge_host` (flat, nullable columns — `NULL`/`NULL` for a non-opcda run) and
`request_json` (the complete run request exactly as submitted, before any config-driven defaulting),
added to the same in-place `0001` migration `rename-driver` had just edited.
`TuneRunRow::record_connection` populates all three via a follow-up `UPDATE` right after `start()`,
matching `record_initial_readings`/`record_allow_uncertain_quality`'s existing precedent, and
`bhtune-cli`'s `prepare()` calls it immediately, before any driver I/O. This closes a real latent
safety bug in `bhtune history revert`, which used to re-resolve the OPC server/bridge host from
`--server`/`--bridge-host`/config _at revert time_ — silently able to write a run's old PID
constants into a different plant's controller than the one it actually tuned.
`resolve_revert_connection` now always trusts the run's own recorded connection, treating an
explicit flag as a cross-check (a hard error on contradiction) rather than an override — see the
`bhtune history revert <run-id>` entry under "Live-plant safety hardening" below for the full design
and the three new tests (`revert_errors_when_the_run_has_no_recorded_connection`,
`revert_errors_when_an_explicit_server_flag_contradicts_the_recorded_one`,
`revert_errors_when_an_explicit_bridge_host_flag_contradicts_the_recorded_one`). `bhtune-server`'s
`/api/runs` list gained matching `opc_server`/`bridge_host` query filters, and the run-detail
response gained the same two fields (deliberately _not_ the list/summary rows, matching `history
list`'s table having no connection column either); `bhtune-cli`'s `history show` gained a
"Connection:" line in `Table` mode and the same two fields in its `RunDetailJson`, so the CLI and
HTTP API stay in JSON-shape parity. Unblocks `api-post-run-write` and `ui-prefill-last-run`, both of
which need a stored, trustworthy connection/request to act on.

## Post-run PID write and revert API

`POST /api/runs/{id}/write` (body: a `response_level`) and `POST /api/runs/{id}/revert` (no body)
let PID constants be written to a live loop _after_ a run finishes, rather than only via the CLI's
pre-run `--write-pid` flow — an engineer can compare Sluggish/Moderate/Aggressive on the run detail
screen and act on whichever one looks right. Neither endpoint reimplements the write path:
`read_previous_pid_values`/`write_and_verify_pid_value` (already shared between the in-run write and
`bhtune history revert`) are promoted from `pub(crate)` to `pub`, and a new shared orchestrating
function, `write_pid_values`, wraps pre-read → write-and-verify-each-constant →
roll-back-on-partial-failure → audit-row-insert exactly once, called by both the CLI's existing
write-back path and these two new HTTP handlers — the same reuse pattern `server-start-tune-api`
established for `prepare()`/`drive()`. `require_writable_run` enforces run eligibility in a fixed
order (still-running → wrong driver → missing PID constant tags → no recorded connection) before
either handler does anything else, and `revert_run` additionally requires the most recent
`write`-kind row to have recorded pre-write values (a write whose own pre-read failed records
`previous = None` and cannot be reverted from). Both handlers take the single `ActiveRun` slot for
the duration of the operation — a post-hoc write strokes the same live loop a tune does, so the two
must never overlap — via a new `ActiveRun::reserve`/`release` pair alongside a new `ActiveRunKind`
distinguishing a short, directly-awaited "exclusive" reservation (a write/revert) from a spawned
tune task (`start`'s existing kind, now `ActiveRunKind::Task`); `cancel`/ `cancel_and_wait` handle
both kinds correctly (an exclusive reservation has nothing to cancel or wait for — axum's own
graceful-shutdown request drain already covers it). A physical write/revert failure is reported as
an ordinary `200` with the failure visible in the returned `writes[]` audit row, never a `4xx`/`5xx`
— matching how a failed write already behaved during an in-run write-back, and confirmed directly by
a dedicated test. Tests use a crate-local minimal mock gRPC `Bridge` service
(`routes::runs::tests::mock_bridge`), deliberately mirroring — not sharing — the same pattern
already used by `bhtune-cli::test_support` and `driver-opcda`'s own `smoke_tests`, since three
internal, already-thorough consumers didn't justify a shared test-support crate. Both new routes
were initially missing from `openapi.rs`'s explicit `paths(...)`/ `components(schemas(...))` lists —
that module's own doc comment warns this fails silently (the route works; it's just absent from the
spec) rather than loudly, and this was exactly the omission it warned about; fixed before
`openapi.json`/`frontend/src/api/schema.d.ts` were regenerated.

## History explorer export and delete

The filterable/sortable run list, full run detail, and the PV/MV trend chart (including
presentation-only initial-reading and terminal restored-MV boundary points plus a 12-poll-interval
left-anchored startup horizon) were already in place from `frontend-screens`/`frontend-live-stream`;
the remaining piece — export and delete actions on the run detail screen — is now shipped too. `GET
/api/runs/{id}/export?format=csv|json` (`export_run`, reusing `bhtune-cli`'s own `samples_to_bytes`,
so the HTTP and CLI export paths can never disagree on the CSV/JSON shape) and `DELETE
/api/runs/{id}` (`delete_run`, cascading through `tune_samples`/ `tune_results`/`tune_writes` via
the schema's existing `ON DELETE CASCADE`) are both new `bhtune-server` routes; the frontend adds
Export CSV/Export JSON download links (plain `<a download>` tags, deliberately not a fetch-then-blob
dance, so the browser's native download handling does the work) and a Delete run button using the
shared styled confirmation modal before navigating back to the run list. `delete_run`'s conflict
check deliberately reads the run's own DB `outcome` column rather than `ActiveRun`'s in-memory
registry: `drive()` persists a run's terminal outcome to the database _before_ returning, and
`ActiveRun::release` only runs strictly after `drive()` returns (see `routes::runs::start_run`), so
there is a real — if brief — window where a run is already durably `completed` but its registry
entry has not been released yet. Checking the DB's own outcome instead of the best-effort in-memory
tracker closes that race outright, with the registry reserved for live tune/write coordination and
the database outcome remaining authoritative for deletion — found and fixed by writing a real
Playwright E2E test for delete (`tune.spec.ts`) that first failed against the naive
`ActiveRun`-based guard. That same Playwright run also surfaced a second, unrelated pre-existing bug
in TanStack Query's setup: `queryClient` had no `retry` policy at all, so a genuine 404 (like the
deleted run's own detail page) retried 3 times with exponential backoff before the UI's error banner
ever appeared, leaving the page stuck on "Loading run…" for several seconds. Fixed with a new
`ApiError` class (`frontend/src/api/errors.ts`) carrying the HTTP status code, threaded through
every `queryFn`/`mutationFn` in `runs.ts`/`templates.ts`/`AppLayout.tsx`, and a `queryClient`
default `retry` that skips retrying any 4xx response — permanent failures — while keeping the
default 3-retry behavior for genuinely transient ones (network drops, 5xx).

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

## Windows service integration

`server-windows-service` implements a platform-neutral `ServiceDefinition`/`ServiceLifecycle` in the
new `crates/bhtune-server/src/service.rs`, a `#[cfg(target_os = "windows")]` module wrapping the
`windows-service` crate for real SCM `install`/`uninstall`/`start`/`stop`/`status`, and — since
`bhtune-server` (unlike the Windows-only `opcda-bridge-gateway` it borrows this pattern from) is
genuinely cross-platform — real, informative, non-panicking stub functions on every other OS that
explain the actual platform equivalent (systemd on Linux, launchd on macOS) and point at the new
packaging files instead of silently doing nothing. `crates/bhtune-server/src/cli.rs` gained the five
subcommands plus a global `--config <path>` flag, captured into the service's own registered launch
arguments at install time so a service-launched process always resolves the same config file
regardless of which account the SCM runs it as (a real gotcha — see the installation guide's
callout). `main.rs` is now a thin platform-split dispatcher: a synchronous `fn main()` on Windows
tries SCM dispatch first, falling back to building its own Tokio runtime for interactive use when
run outside the SCM; every other platform still runs the async server directly, unchanged. New
`packaging/systemd/bhtune-server.service` (validated with `systemd-analyze verify`) and
`packaging/launchd/com.bytehound-labs.bhtune-server.plist` (validated with Python's `plistlib`)
supply the Linux/macOS equivalents. This Linux sandbox still cannot compile `#[cfg(windows)]` code
directly (`libsqlite3-sys` needs an `x86_64-w64-mingw32-gcc` cross-compiler that isn't installed and
can't be — no passwordless sudo to install `mingw-w64`), so the Windows-specific SCM glue was
verified by careful line-by-line comparison against the already-CI-proven `opcda-bridge-gateway`
reference implementation and against the `windows-service` 0.8 API surface on docs.rs, plus the real
`windows-latest` CI job, rather than compiled locally — and then manually verified against a live
Service Control Manager on the `hp` Windows host, exercising every subcommand against a real,
freshly-cloned build: `install` (confirmed via `sc qc` — exact expected
`SERVICE_NAME`/`DISPLAY_NAME`/`AUTO_START`/`LocalSystem`, and, with `--config <path>`, the path
correctly baked into `BINARY_PATH_NAME`), `start` (confirmed via `sc query` showing `RUNNING`, the
process actually listening in the `Services` session, and a real HTTP `/api/health` request
succeeding), `stop` (clean `WIN32_EXIT_CODE 0`, process actually gone from `tasklist`), and
`uninstall` (service fully deregistered, `status` correctly returning to the pre-install "does not
exist" error). Also confirmed the interactive/foreground fallback (`is_run_outside_scm`) genuinely
serves requests when run outside the SCM, and that a `--config` pointing at a custom `db`/`log.dir`
is actually honored by a `LocalSystem`-run service rather than falling back to that account's own
profile directory — the exact gotcha the installation guide documents a mitigation for. `protoc`
turned out to be a previously undocumented build prerequisite on Windows (transitively via
`opcda-bridge-proto`'s gRPC codegen; `choco install protoc` resolves it) and has been added to the
installation guide. No code defects were found; two apparent anomalies during testing (an
interactive run started via `cmd /c start /b` over SSH leaving no process behind, and a log file
that `dir` reported as 0 bytes while running) were both root-caused to test-methodology artifacts,
not real bugs — `start /b` doesn't survive the invoking SSH channel closing, and `dir` shows a stale
cached size for a file another process still has open for writing (`type` confirmed the real-time
content was correct and matched Linux's own output exactly).
