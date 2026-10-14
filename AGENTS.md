# bhtune

Agent contract: architecture, invariants, commands, and safety rules. Code and tests are authoritative. Read the relevant [public architecture page](docs/architecture/topology.md) or guide before changing its area; retained research/process records are indexed in [design notes](docs/internal/design/README.md). Keep rationale with its canonical page, not in a phase diary here.

## Summary

BHTune is an AGPL-3.0-or-later Rust PID auto-tuner for industrial DCS/PLC loops, with the [CLA](CLA.md). v1 is one-loop MRFT through the `bhtune` CLI or browser GUI served by `bhtune-server`, plus an in-process simulator and validation-only replay. There is no desktop app.

OPC DA uses the published `opcda-bridge` 0.6 crates.io dependency and a separate Windows gateway. Product OPC I/O is COM-free; Windows service support stays platform-gated. The native COM smoke fixture is test-only. Dependencies must be open source.

OPC UA, Modbus, Step Test, batch tuning, and a built-in scheduler remain [roadmap](docs/roadmap.md) work. Step Test requires a live subscription RPC from the bridge; scheduled tuning uses an external scheduler invoking the CLI.

Windows build/manual verification uses `win`, checkout `C:\git\bhtune`, and ordinarily `stable-x86_64-pc-windows-gnu`, MinGW-w64, and `protoc`, not MSVC. Use CLI/SSH only. Yokogawa read validation defaults to `yok3` unless another station is requested: run only `StopSebol.get_online_controllers()`, never `StopSebol.run()` or `stop-sebol`, then read one exact browsed/mapped ItemID from a reported-online controller. Do not write, tune, or refresh a namespace.

## Crate map

| Path                                     | Responsibility                                                                                        |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| `crates/bhtune-core`                     | Pure model, MRFT, math, embedded templates; no I/O, async, or clock reads.                            |
| `crates/bhtune-driver`                   | Driver trait, OPC DA, FOPDT simulator, replay; sole production bridge dependency.                     |
| `crates/bhtune-db`                       | SQLite schema, migrations, templates, history, backup/restore, retention.                             |
| `crates/bhtune-runtime`                  | Shared configuration, bootstrap, logging, drivers, tune/safety/recovery, history operations, exports. |
| `crates/bhtune-cli`                      | Package/binary `bhtune`, library `bhtune_cli`; terminal adapter and generated CLI references.         |
| `crates/bhtune-server`                   | Axum/OpenAPI adapter and React SPA; peer of the CLI, not its consumer.                                |
| `crates/bhtune-test-support`             | Unpublished mock gRPC bridge, never a product/release artifact.                                       |
| `frontend/`, `website/`                  | React/TypeScript/Vite/Tailwind SPA with uPlot; Docusaurus reads `docs/`, excluding `internal/**`.     |
| `fuzz/`, `tests/opcda-yokogawa-fixture/` | Separate test-only Cargo workspaces, outside product membership.                                      |

Binary tests use `CARGO_BIN_EXE_bhtune` or `CARGO_BIN_EXE_bhtune-server`, matching the actual binary names. The empty `mock-driver` feature is a cycle guard: CLI/server enable it; `bhtune-driver` must not.

## Invariants

- Core is a pure `step` state machine using `f32`, with no clock reads. Callers supply tick time, reused by switches. Core alone does not request chrono clock features, but workspace feature unification enables them; this is a source/API invariant, not a compile-time clock barrier.
- Simulator time advances by the configured poll interval. Live engine time is monotonic elapsed time projected onto UTC start, captured after successful PV reads, never a driver's timestamp. Replay uses recorded timestamps.
- `Driver` has no production core dependency; tag values are strings. Rejected writes are `Ok(WriteOutcome)`; connect, operation, and unsupported errors stay distinct.
- OPC DA uses `tokio::sync::Mutex` across awaits; simulator uses `std::sync::Mutex` without awaits. Do not unify them. Exact `Good`/`Uncertain` strings map to those qualities; everything else is `Bad`. OPC timestamps are `None`.
- Browse uses gateway-owned sessions, opaque node keys/page tokens, and exact ItemIDs. Never infer hierarchy by splitting punctuation. Indexed search is optional; simulator/replay browse and search return `Unsupported`.
- Closing Browse acknowledges only displayed terminal index-error notices for the exact bridge/server on the New Tune page. Preserve raw status, cached search, and diagnostics. Later attempts/accepted or observed builds re-arm notices; hidden or later errors remain unacknowledged. No browser-storage persistence.
- Saved-tag restoration keeps the tree mounted but covered until the selected row is visible. Errors, unavailable tags, cancellation, and fallback selection must clear the shared overlay.
- Native smoke uses only the synthetic fixture and installer-pinned official gateway, never plant equipment, production port `7600`, services, or existing COM classes. Preserve real OPCEnum; add the fixture discovery shim only when absent from both hives. User scope requires non-elevated UAC; Machine scope is explicit on a disposable host. Protect the wildcard gateway listener with its owned executable firewall block, or record the disposable hosted-CI boundary. Ownership-checked process/COM/firewall cleanup and isolated evidence are mandatory.
- The bridge remains a crates.io dependency local to the driver. Published/packaged builds must not use git or path overrides.
- Runtime owns shared application services; direct `clap`, HTTP-framework, and OpenAPI dependencies/types stay in adapters. Transitive gRPC transport crates must not enter runtime APIs. Neither adapter may depend on the other's application services.
- CLI `TuneArgs` and HTTP `StartRunRequest` become runtime `ValidatedTuneRequest` before preparation. Shared defaults and finite/positive/tag-override validation belong there; Demo restrictions are additional server policy.
- CLI `check` dispatches before writable DB bootstrap and uses only `db::open_read_only` for persisted templates. Full HTTP preflight uses its read-only template pool and shared `ReadOnlyDriver`. Neither may create history, start tune work, write/restore tags, refresh indexes, or mutate DB/controller state. Demo omits the route. Readiness cannot prove write permission.
- SQLite is plain/unencrypted. Flatten stable filterable fields; nested evolving fields use `json_valid` JSON. Keep results and writes separate. Enum columns/CHECK literals match serde snake_case.
- Pre-v0.1 uses one embedded schema baseline. Writable and read-only connections reject unknown migration versions/checksums before use. Preserve a verified backup and select a new file; never edit `_sqlx_migrations` or automatically delete an old DB. Use forward migrations after v0.1.
- Startup re-upserts `builtin`/`catalog` templates without overwriting a different origin; never auto-edit `user` rows.
- Snapshot template, tags, request, and OPC connection before mutation. Revert/recovery use that recorded connection; supplied flags are cross-checks, not overrides.
- Every live mutation/recovery holds the canonical database's OS exclusive guard plus a conditional database/controller-MV claim. Heartbeat independently every five seconds. Stale heartbeat/PID/file presence or an in-memory registry is not recovery authority; a paused owner holding the guard blocks takeover.
- Full startup exports evidence before retiring verified orphaned owned running tunes and setting explicit recovery eligibility; never contact controllers. Complete ownership, provenance, restore intent, and mutation evidence are required; incomplete/legacy rows fail closed. Demo bypasses live recovery.
- `restore-loop <run-id> --yes` reports every restore step and cannot succeed with incomplete restoration or required audit persistence. If preparation provenance persistence fails, mark the owned run failed or delete it if that update fails; never leave a permanent running row.
- Production uses `calculate_all_checked`; invalid results retain reasons and null numbers and cannot be written. Keep the fixed lower clamp, full elapsed-time period, and switch-sample result extrema. Hysteresis resets remain unchanged; Variant B is research-only and requires closed-loop simulation and an explicitly approved noncritical trial.
- Every template/snapshot explicitly carries `pid_rounding`: 0-7 decimal places or 1-7 significant digits shared by P/I/D. Yokogawa uses one decimal; other built-ins/new custom templates use three significant digits. Runtime applies it after conversion, supplies canonical display text/numeric write targets, preserves disable sentinels, and rejects active terms rounded to zero before PID I/O. Never infer precision from vendor names. Raw math/exports, originals/readbacks, rollback, revert, and recovery are unrounded.
- Full mode defaults to `127.0.0.1` and has no v1 authentication. Off-loopback is opt-in; same-host Origin/Host checks or an explicit origin pin are CSRF protection, not authentication.
- Demo is a server capability boundary, not accounts. Only health/capabilities, built-in template reads, and visitor-owned simulator workflows are mounted. No OPC, PID writes/reverts, config/template mutation, notes, server drafts, preflight, OpenAPI, or Scalar. Quotas/bounds are application constants; configuration cannot widen them. Require one replica, a separate DB, exact origin/HTTPS for non-loopback access, and only SHA-256 session-token storage.
- JSON stdout is exactly one value; logs go to rotating files and attached-console stderr, never stdout. Prompts use stderr; JSON without `--write-pid` never reads stdin.
- One HTTP transport and one generated `openapi-fetch` client, no swappable transport interface. Regenerate OpenAPI and TS schema together. Production UI uses shared `Modal`/`ConfirmModal`, never browser alert/confirm/prompt.
- Releases fail closed: explicit approval and documented gates precede publication/tags/releases. Only `.github/workflows/release.yml` owns GitHub Releases. Never commit secrets, binaries, caches, or local state; never add a Copilot co-author trailer. With no `.env`, commit using `LEFTHOOK=0` to protect the Bitwarden note.

## Conventions

- Short-lived `<type>/<description>` branches from current `main`, Conventional Commits, reviewed/approved squash PRs. Never push/force-push main or admin-merge; a green check is not merge approval.
- MSRV/edition come from root `Cargo.toml`: Rust 1.94, edition 2024. Follow [Lint policy](CONTRIBUTING.md#lint-policy); never weaken it, coverage, or Sonar policy with `NOSONAR` or unjustified dispositions.
- Reuse existing helpers. Do not reserve unused path dependencies; promote a dependency to workspace scope when a second crate needs it.
- Local browser testing binds `0.0.0.0:8787` using isolated temporary DB/logs, never the normal user DB. Rebuild/restart after source/frontend edits; do not use a stale copied binary. Demo LAN access requires its exact HTTPS origin.
- Export affected rows before destructive database operations. Backup/restore are library APIs; do not restore another process's open database.
- Non-interactive `gh` does not source the zsh token wrapper. This repository uses the personal identity; never print tokens.

## Commands and gates

Run the smallest affected check, then every applicable gate in [CONTRIBUTING](CONTRIBUTING.md#ci). Optional `just` recipes reuse repository commands:

```sh
just check
just cov
just fe
just e2e
pnpm run check:dead-code
```

`just check` covers Rust formatting, strict Clippy/rustdoc, tests, dependency checks, generated drift, and build. `just cov` must retain 100% canonical LCOV `DA` source lines. Docs-only work needs documentation gates, not Rust tests; workflow changes require `actionlint`.

Regenerate producing-definition changes with `just gen` (OpenAPI, CLI/config references, man pages/completions, TS schema); never hand-edit those outputs. Use `pnpm --filter bhtune-frontend exec playwright test --list` for the actual browser inventory. OPC helpers are in `frontend/e2e/support/opcBrowser.ts`; Full excludes `demo-real.spec.ts`, Demo matches it.

The native fixture is outside root workspace gates. Follow [Windows native OPC DA smoke](CONTRIBUTING.md#windows-native-opc-da-smoke) for explicit MSVC x86 formatting/Clippy/tests/deny and the serial runner; do not change the ordinary GNU toolchain or product coverage exclusions. Local firewall creation/removal needs administrator permission independently of COM scope; never auto-elevate/fall back.

Sonar project is `bytehound-labs_bhtune`. Read `SONAR_TOKEN` from ignored `.env`, never print it. Before merge, query `pullRequest=<number>` with `issueStatuses=OPEN,CONFIRMED`; after merge, wait for analysis of the merge revision and verify `branch=main` is zero. Preserve required status names: `Required validation status`, `Required coverage status`, `Required E2E status`, `Required Sonar quality status`.

## Config precedence (`cli-config`)

CLI flag > environment > TOML > built-in default. Explicit missing paths and malformed existing files are errors; missing auto-discovered config/catalog files mean defaults. See the [example TOML](crates/bhtune-cli/bhtune.example.toml), [configuration reference](docs/reference/config.md), and [configuration guide](docs/guides/web-ui/templates-and-configuration.md#configuration) for keys/paths.

`allow_uncertain_quality` defaults true: Good passes, Bad never does, Uncertain follows policy and logs a warning. Snapshot that decision. `/config` edits the same TOML, preserving comments/unknown keys with validation, backup, and atomic replacement; stale revisions return 409. Retention saves do not delete immediately. Resolve `[tuning]` before connecting/mutating; no unlimited run.

## Automation (`cli-automation`)

`tune`/`simulate` accept `--yes`, `--write-pid <aggressive|moderate|sluggish>`, and `--output table|json`; reject write-back without `--yes` before I/O. Simulator skips PID write-back because it has no PID tags. `check` accepts tune inputs, output/strict, and write-readiness flags without requiring `--yes` or writing. `restore-loop` requires `--yes`.

The [exit-code table](docs/guides/safety.md#scripting-and-exit-codes) is canonical. `check` uses 0/pass, 1/cannot run, 8/failed or strict warning. Terminal DB outcomes are Completed/Aborted/Failed; write-back failure does not rewrite a completed tune. Restore-incomplete exit 6 outranks actuation-failed exit 7.

## OPC DA integration

`OpcDaDriver::connect(host, server)` passes host:port (default 7600) to the client and sends the stored ProgID on later calls. Gateway info and registered-server listing are pre-connection free functions, not Driver methods. The [CLI reference](docs/reference/cli.md#bhtune-opc) lists discovery, read/write, browse/search/index, and close commands. CLI browse sessions stay open until explicit close; the browser closes its session with the modal.

Preserve Refresh Index, conditional Cancel Indexing, Delete Index, and persisted Enable/Disable Auto-refresh. New/recreated indexes default off; manual refresh/retry never opts in. No `index.enabled`/`index.paused` override. Countdown requires an enabled usable index with a real next date. Disable preserves cache/active builds; grace, maintenance, health, pacing, and failure backoff remain independent. See [browser guide](docs/guides/web-ui/starting-a-tune.md#opc-da-connection-and-mapping).

All distribution channels remain: paired CLI/server archives, Docker, deb/rpm, AUR, Windows NSIS, and macOS launchd/Homebrew. Homebrew needs a tap and verified checksums. Optional installer gateways are pinned, checksum/provenance-verified official x86 artifacts, never runtime downloads. Follow [installation](docs/getting-started/installation.md) and [release guidance](docs/guides/releasing.md); stable installer attachment requires the coordinated release handoff.

## Live-plant safety hardening

`cli-safety`, `safety-quality`, and `safety-cancellation` follow the [safety guide](docs/guides/safety.md) and [safety architecture](docs/architecture/safety-model.md).

- Validate before mutation: finite relay amplitude 0.1-50%, count >= 1, delay <= 3600 seconds, distinct PV bounds, finite ordered MV bounds, and initial MV inside them. `tune` has no dry run: omitting `--write-pid` avoids PID constants only, not mode/MV writes. Use `check` for read-only rehearsal.
- Whole-run timeout defaults to 3600 seconds and cannot be disabled. One process-wide `CtrlC` and `bounded_driver_call` must interrupt in-flight I/O, not just idle waits. Cap operations separately; restoration has its own budget.
- Every exit attempts all applicable restore steps, not just the first successful one. Persist initial mode/mode attribute/applicable Auto-start setpoint before mutation; retain restore status and each failed step. A second Ctrl+C or exhausted restore budget is exit 6. An accepted authoritative MV restore extends its deadline to at least four seconds after acceptance.
- Apply quality policy to initial reads, polling, and PID confirmation. Persist rejected poll samples before abort/restore.
- Pre-read/persist all P/I/D before writing. Write/verify one at a time with 0.001 absolute or 1% relative tolerance. Roll back only individually confirmed constants; failed rollback points to `history revert`.
- Before connecting, revert checks run, OPC driver, write audit, previous values, `--yes`, and PID tags. Use recorded server/bridge; contradicting flags fail. Record current values as the revert's previous values; do not nest rollback.
- Accepted MV writes are not actuation. Audit relay/restore commands in `tune_mv_actuations`; confirm exact targets within four seconds, rejecting late matching reads. Pending relays batch PV/MV, map by tag identity, and evaluate MV before advancing MRFT. Never issue a replacement after failed confirmation. Simulator/replay omit this live check.
- Commanded MV stays in samples/trends; measured MV stays in the actuation audit, not the normal browser table. Invalid results cannot be written; sampling/latency advice does not abort/block valid writes. Shared PV/MV latency categories overlap and must not be summed.
- `restore_from` consumes the pool, checks integrity/exclusivity, uses `VACUUM INTO` plus same-directory temporary/rename, and returns `DatabaseInUse` for busy data. Its probe is point-in-time, not a held lock.

Official gateway deployment needs separate authorization and the same verified GitHub artifact on every configured host, never a local substitute or tag-write smoke test.

## History explorer

Retention is opt-in via positive days. Startup, `history prune`, and server sweeps share a cutoff; retain eligible/running/incomplete recovery until confirmed. Startup sweep failure is fatal; periodic sweeps log and continue. Deletion requires a terminal DB outcome with no unresolved recovery, not merely absence from the active registry.

CSV/JSON exports contain persisted samples. Initial/restored-MV chart points are presentation-only; short trends reserve 12 poll intervals. Continuous historization and cross-run overlays are outside v1.

## Web app architecture

Release embeds `frontend/dist/`; rebuild after asset changes. `build.rs` creates the directory before embedding. HTML uses no-cache, hashed assets immutable caching, missing dotted assets 404, client routes SPA fallback, unknown `/api/*` JSON 404. Samples use SSE, not WebSocket.

`bhtune-server healthcheck` dispatches before startup, SCM, logging, or DB bootstrap: one raw loopback HTTP/1.1 `/api/health` probe, port resolved from `BHTUNE_BIND > TOML > default`, three-second overall deadline. Docker uses it for liveness, not database readiness or plant safety.

Write/revert share a styled review naming loop/tags/exact values. Results appear above trend once available; live status stays above it. Keep PV/Commanded MV labels and hidden screen-reader description, no point selector/recorded-tag caption/repeated measurement footer/raw actuation table. Sampling/latency diagnostics remain CLI/API/log data; genuine safety/result warnings remain visible.

## Documentation contract

Behavior changes update generated references, this file when invariants/commands/safety/correctness change, README when a new user would notice, and the relevant guide. Use timeless user-facing facts, not change history. Read the [correctness register](docs/internal/design/correctness-register.md) before changing math, mapping, exports, or related UX; its numbered decisions require focused coverage.

The paired docs-drift hook covers committed/uncommitted implementation changes and warns when no documentation surface changed. Keep this file on its surface list; it is a backstop, not permission to skip documentation.

The CI docs agent may edit narrative docs/README and the workflow may update its screenshot lock on same-repository PRs. It must not edit AGENTS or other protected paths. Preserve `--deny-tool 'write(AGENTS.md)'`, post-run path checks, and fork secret isolation.

## Documentation system

Generated: CLI/config references, OpenAPI, TS schema, man pages, completions. Authored reference pages such as glossary/API are not generated; the CI docs agent still treats all `docs/reference/**` as read-only. Screenshot PNGs never enter Git; the workflow owns `docs/reference/web-ui-screenshots.json`. See [contributor screenshot policy](CONTRIBUTING.md#web-ui-screenshots).

Release snapshots wait for the approved stable release. The [published site](https://bytehound-labs.github.io/bhtune/) and [documentation design](docs/internal/design/docs-site-and-generated-docs.md) describe publication and ownership.

## Correctness-critical design details

The [full register](docs/internal/design/correctness-register.md) is canonical; preserve its decisions and evidence instead of maintaining a second numbered copy here. In addition to the invariants above: process tables match `ProcessType::ALL`; CSV headers/rows share one field list; labels use the run's template snapshot; tag suffix replacement uses the final `.`, `!`, or `/` component; exports use explicit paths/stdout and logs use resolved data paths.

Simulator selection is explicit, never a magic tag, and does not skip restore. PID/units/direction are enums; only temperature types allow PID. Process changes reset skip/count/noise defaults; the final MRFT step snaps MV to its initial value. Base-tag/template changes reset Custom mappings but preserve Fixed values; omitted direction/ranges hydrate as template sources. Sampling is adequate at >= 6 samples/period, marginal below, not assessed without a usable period. No licensing/loop-locking ledger, log encryption, or login gate.

## Design notes

The [public architecture pages](docs/architecture/topology.md) and area guides own durable rationale. The [design-note index](docs/internal/design/README.md) links them and the retained correctness/replay/documentation-process records. Code/tests take precedence over prose; do not create a second parallel manual.

## Open questions

Whether site-specific templates should remain shareable JSON/TOML exports alongside SQLite rows.
