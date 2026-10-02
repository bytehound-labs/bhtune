# bhtune

Agent contract for this repository. It states the current architecture, invariants, commands, and safety rules. Durable rationale lives in [`docs/internal/design/`](docs/internal/design/README.md). Code and tests are authoritative if they disagree with either document. Do not grow this file back into a phase diary.

## Summary

BHTune is an open-source Rust PID auto-tuner for industrial DCS/PLC systems. It runs a Modified Relay Feedback Test (MRFT) against one loop and can calculate and write back PID constants. v1 adapters are the `bhtune` CLI and the browser GUI served by `bhtune-server`. There is no desktop app.

OPC DA I/O uses the published `opcda-bridge` 0.5 crate and a separate Windows gateway. This repository has no Windows/COM dependency. The MRFT engine is a pure, clock-free state machine. Dependencies are open source and enforced in CI. The license is AGPL-3.0-or-later with the CLA in [`CLA.md`](CLA.md).

v1 is MRFT over OPC DA, plus the in-process simulator and a validation-only replay driver. OPC UA, Modbus, Step Test, multi-loop batch tuning, and a built-in scheduler are roadmap items in [`docs/roadmap.md`](docs/roadmap.md), not v1 work. Step Test stays blocked on a live subscription RPC in `opcda-bridge`. Scheduled tuning is an external scheduler invoking the CLI.

`win` is the Windows build and manual-verification host. Keep the checkout at `C:\git\bhtune`. Ordinary BHTune builds use `stable-x86_64-pc-windows-gnu`, MinGW-w64, and `protoc`; they do not need MSVC. Yokogawa read validation defaults to `yok3` unless the user names another station. Before a read, run only `StopSebol.get_online_controllers()`. Do not call `StopSebol.run()` or `stop-sebol`. Read one tag from a reported-online controller. Do not write, tune, or refresh a namespace as part of that check.

## Crate map

| Path                  | Responsibility                                                                                                                                                                                                                             |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `bhtune-core`         | Model, MRFT state machine, tuning math, and the embedded template catalog. No I/O, async, or clock reads.                                                                                                                                  |
| `bhtune-driver`       | `Driver` trait plus OPC DA, FOPDT simulator, and replay. The only crate that depends on `opcda-bridge`.                                                                                                                                    |
| `bhtune-db`           | SQLite schema, migrations, template seeding, run history, backup/restore, and retention.                                                                                                                                                   |
| `bhtune-runtime`      | Shared configuration, database bootstrap, logging, retention, driver setup, tune orchestration and safety, history writes/reverts, and export serialization. Its source and direct dependencies contain no CLI, HTTP, or OpenAPI concerns. |
| `bhtune`              | CLI adapter package (`bhtune_cli` Rust library and `bhtune` binary); terminal prompts and output, command dispatch, and generated CLI references; shared application work goes through `bhtune-runtime`.                                   |
| `bhtune-server`       | Axum HTTP/OpenAPI adapter and embedded React SPA; shared application work goes through `bhtune-runtime`, not the CLI.                                                                                                                      |
| `bhtune-test-support` | Unpublished shared mock gRPC bridge for tests. Not a product or release artifact. The empty `mock-driver` feature is a cycle guard. CLI and server enable it; `bhtune-driver` must not.                                                    |
| `frontend/`           | React, TypeScript, Vite, and Tailwind SPA. One generated `openapi-fetch` client. The trend chart is `uPlot`.                                                                                                                               |
| `website/`            | Docusaurus site. Its docs plugin reads repo-root `docs/` and excludes `docs/internal/**`.                                                                                                                                                  |
| `fuzz/`               | Separate Cargo workspace for parser fuzz targets. Not a product-workspace member.                                                                                                                                                          |

The server package and `[[bin]]` are both named `bhtune-server`, so tests must use `env!("CARGO_BIN_EXE_bhtune-server")`. The CLI binary is `bhtune` (`CARGO_BIN_EXE_bhtune`).

## Invariants

- `bhtune-core` is a pure `step` state machine. `chrono` `clock` and `now` stay disabled workspace-wide, so `Utc::now()` cannot compile there. Callers supply tick time.
- Tuning math uses `f32`. Simulator MRFT time advances by the configured poll interval. Live OPC DA time is monotonic elapsed time projected onto the run's UTC start, captured after a successful PV read. A live driver's timestamp is never engine time. Replay is the narrow exception: its recorded timestamp is the trace.
- `Driver` does not depend on `bhtune-core`. Tag values stay strings. A rejected write is `Ok(WriteOutcome)`, not `DriverError`. Connect, operation, and unsupported failures stay distinct.
- `OpcDaDriver` uses `tokio::sync::Mutex` because the client guard crosses `.await`. `SimulatorDriver` uses `std::sync::Mutex` because it does not hold the guard across `.await`. Do not unify them.
- OPC DA quality is an exact `Good` or `Uncertain` match. Every other quality string is `Bad`. OPC DA `TagValue.timestamp` is always `None`.
- Browse uses gateway-owned sessions, opaque node keys, and page tokens. Never split `.`, `!`, or `/` to infer hierarchy. Indexed search is optional. Simulator and replay browse/search return `Unsupported`.
- `opcda-bridge` stays a crates.io dependency local to `bhtune-driver`. Published and packaged builds must not use a git dependency or a path override.
- `bhtune-runtime` owns application services shared by the CLI and server. Keep direct `clap`, HTTP-framework, and OpenAPI dependencies and types in their respective adapters; transport crates may appear transitively through the OPC DA gRPC client.
- CLI `TuneArgs` and HTTP `StartRunRequest` convert to runtime-owned `ValidatedTuneRequest` before preparation. Keep shared simulator defaults and common finite/positive/tag-override checks in the runtime; Demo-specific restrictions remain an additional server policy.
- `bhtune check` and Full-mode `POST /api/runs/preflight` use the shared runtime preflight and `ReadOnlyDriver`. The CLI dispatches before `db::open` and queries persisted templates only through `db::open_read_only`; the server endpoint uses its read-only template pool. Neither may create run history, start tune work, write tags, restore a loop, refresh a namespace, or otherwise mutate database/controller state. Demo mode does not mount the HTTP route. A passing write-back-readiness result proves only that P/I/D tags are configured and readable and any template-specific prerequisites are met; it cannot prove write permission.
- SQLite is plain and unencrypted. Flatten stable filterable fields. Keep nested evolving values in `json_valid` JSON. `tune_results` and `tune_writes` stay separate tables.
- Enum columns reuse serde snake_case. Matching `CHECK` constraints use the same literals.
- Startup re-upserts `builtin` and `catalog` templates and never overwrites a row with a different `origin`. `user` rows are never auto-edited.
- Each run snapshots its template, tags, submitted request, and OPC connection before driver mutation. `history revert` trusts that recorded connection. An explicit flag is a cross-check, not an override.
- If follow-up provenance persistence fails, `prepare()` marks the owned row failed, or deletes it if that update cannot be stored. Do not leave a permanent `Running` row.
- Production persistence uses `calculate_all_checked`. Invalid results store null numbers and cannot be written. Do not enable `MrftCompat.replicate_lower_clamp_bug`, `TuningMathCompat.replicate_period_truncation_bug`, or `MrftCompat.replicate_extrema_reset_bug` on a production path.
- Result extrema are separate from hysteresis extrema. Hysteresis still resets the legacy way, so switch timing does not change. Variant B is research-only.
- Full mode is the default, binds `127.0.0.1` by default, and has no authentication in v1. Off-loopback bind is an explicit opt-in. Browser mutations use same-host Origin/Host checks unless an explicit origin is pinned. That check is not authentication.
- Demo mode is a server-enforced capability boundary, not an account system. It mounts health, capabilities, built-in template reads, and visitor-owned simulator history only. OPC, PID write/revert, config and template mutation, notes, drafts, preflight, OpenAPI, and Scalar are not mounted. Quotas and simulator bounds are application constants; deployment config cannot widen them. One replica and a separate database are required. Only the SHA-256 of the anonymous session token is stored.
- `--output json` is exactly one JSON value on stdout. Logs go to the rotating file and, when a console is attached, to stderr. Never to stdout.
- There is one HTTP transport and no client-side transport interface. Regenerate `openapi.json` and `frontend/src/api/schema.d.ts` together. Do not hand-edit generated OpenAPI, TypeScript schema, CLI reference, man pages, or completions.
- Frontend production code must not call `window.alert`, `window.confirm`, or `window.prompt`. Use the shared `Modal` and `ConfirmModal`.
- Release automation is fail-closed. Do not publish crates, push a tag, or create a GitHub Release unless the documented gates are open and the user explicitly approved that release. `.github/workflows/release.yml` is the only GitHub Release owner.
- Do not commit secrets, binaries, caches, or machine-local state. Never add a `Co-authored-by: Copilot` trailer. If `.env` is absent, commit with `LEFTHOOK=0` so `ds-sync` cannot wipe the Bitwarden note.

## Conventions

- Trunk-based flow: `main` and short-lived `<type>/<description>` branches, squash-merged. Never push or force-push to `main`, and never admin-merge.
- Conventional Commits. User-facing docs state current behavior, not the history of a change.
- MSRV is Rust 1.94 (`rust-version` in the root `Cargo.toml`). Edition is 2024.
- Lint policy is `[workspace.lints]` and the "Lint policy" section in [`CONTRIBUTING.md`](CONTRIBUTING.md). Do not weaken a lint, add `NOSONAR`, or accept a finding only to clear a dashboard.
- Do not add an unused path dependency to reserve a crate graph. Promote a dependency to `[workspace.dependencies]` when a second crate needs it.
- `bhtune` and `bhtune-server` are peer adapters over `bhtune-runtime`; neither adapter may become the other adapter's application-service dependency.
- Local browser testing binds `bhtune-server` to `0.0.0.0:8787` with an isolated temporary database, never the user's normal database. Rebuild and restart after a source or frontend change. Do not test a stale copied binary. Demo access off loopback requires the exact configured HTTPS origin.
- Export affected rows before a destructive database change.
- A non-interactive `gh` call does not source the zsh token wrapper. This repository uses the personal identity. Never print a token.

## Commands and gates

Run the smallest command that covers the change, then every gate that change can affect.

```sh
cargo fmt --all --check
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo machete
cargo llvm-cov --workspace --lcov --output-path lcov.info
pnpm --filter bhtune-frontend run format:check
pnpm --filter bhtune-frontend run lint
pnpm --filter bhtune-frontend run build
pnpm run check:licenses
pnpm run check:dead-code
```

When the producing definitions change, regenerate and commit the output:

```sh
cargo run -p bhtune-server --example gen_openapi
cargo run -p bhtune --example gen_docs --features schemars
pnpm --filter bhtune-frontend run generate:api
```

A documentation-only change does not need `cargo test`. A workflow change needs `actionlint`. Coverage remains 100% of canonical LCOV `DA` source lines; a docs-only change does not relax that bar for untouched Rust.

SonarCloud project key: `bytehound-labs_bhtune`. Before merge, query `pullRequest=<number>` with `issueStatuses=OPEN,CONFIRMED`. After merge, wait until the `branch=main` analysis revision includes the merge commit and the same query is zero. Read `SONAR_TOKEN` from `.env` and never print it.

Required status names stay `Required validation status`, `Required coverage status`, `Required E2E status`, and `Required Sonar quality status`.

`pnpm --filter bhtune-frontend exec playwright test --list` reports 109 tests in 15 files. The OPC DA browser suite is 31 of those: `opc-browser-discovery.spec.ts` (6), `opc-browser-index.spec.ts` (9), `opc-browser-mapping.spec.ts` (8), `opc-browser-restore.spec.ts` (5), and `opc-browser-selection.spec.ts` (3). Shared helpers live in `frontend/e2e/support/opcBrowser.ts`. The `full` project ignores `demo-real.spec.ts`; the `demo` project matches only that file.

## Config precedence (`cli-config`)

Resolution is CLI flag, then environment variable, then TOML, then the built-in default. An explicit missing path is an error. A missing auto-discovered file means all defaults. A file that exists but does not parse is always an error. The example file is [`crates/bhtune-cli/bhtune.example.toml`](crates/bhtune-cli/bhtune.example.toml). Rationale is in [`docs/internal/design/config.md`](docs/internal/design/config.md).

| Setting       | Flag               | Env                     | TOML             | Default                                                                 |
| ------------- | ------------------ | ----------------------- | ---------------- | ----------------------------------------------------------------------- |
| Database      | `--db`             | `BHTUNE_DB`             | `db`             | platform data dir `bhtune.db`                                           |
| Gateway       | `--bridge-host`    | `BHTUNE_BRIDGE_HOST`    | `bridge_host`    | `localhost:7600`                                                        |
| OPC server    | `--server`         |                         | `server`         | none; required for OPC DA tune and `opc`                                |
| User catalog  | `--templates`      | `BHTUNE_TEMPLATES`      | `templates`      | platform config dir `templates.toml`; a missing default is not an error |
| Retention     | `--retention-days` | `BHTUNE_RETENTION_DAYS` | `retention_days` | retain forever                                                          |
| Server mode   |                    | `BHTUNE_SERVER_MODE`    | `server_mode`    | `full`                                                                  |
| Origin        |                    | `BHTUNE_ORIGIN`         | `origin`         | same-host check in Full when unset; required in Demo                    |
| Trusted proxy |                    |                         | `trusted_proxy`  | none                                                                    |

`allow_uncertain_quality` defaults to true. `Good` always passes, `Bad` never passes, and `Uncertain` follows the policy and logs a warning. The decision is snapshotted onto the run. `/config` edits this same TOML file. It does not create a second settings database. Saves preserve comments and unknown keys, validate, back up, and replace the file atomically. A stale revision returns `409`. A retention save does not delete history immediately.

Global `[tuning]` timeouts are resolved before any driver connection or live mutation. There is no unlimited run.

## Automation (`cli-automation`)

`tune` and `simulate` accept `--yes`, `--write-pid <aggressive|moderate|sluggish>`, and `--output table|json`. `--write-pid` requires `--yes` and is rejected before any I/O when `--yes` is absent. `simulate` accepts the write flags but skips write-back: the simulator has no PID constant tags. `check` accepts the tune inputs, `--output table|json`, `--strict`, and `--write-pid` for readiness assessment only; it never writes and does not require `--yes`.

JSON mode prints nothing but the final object on stdout. Prompts go to stderr. JSON mode without `--write-pid` does not read stdin. `check` exits with `0` when checks pass, `1` when it cannot run, and `8` when a check fails or strict mode rejects a warning.

| Code | Name                      | Meaning                                                                                |
| ---- | ------------------------- | -------------------------------------------------------------------------------------- |
| 0    | `EXIT_SUCCESS`            | Completed, or write-back was skipped cleanly.                                          |
| 1    | `EXIT_FAILURE`            | Setup or command error before a normal tune outcome.                                   |
| 2    | `EXIT_ABORTED`            | Ctrl+C, with restore confirmed.                                                        |
| 3    | `EXIT_WRITE_BACK_FAILED`  | The test completed and the PID write-back failed.                                      |
| 4    | `EXIT_TIMED_OUT`          | `[tuning].timeout_secs` elapsed.                                                       |
| 5    | `EXIT_POOR_QUALITY`       | A tuning-critical read was `Bad`, or `Uncertain` while the quality policy rejected it. |
| 6    | `EXIT_RESTORE_INCOMPLETE` | Restore was not confirmed, including a second Ctrl+C during restore.                   |
| 7    | `EXIT_ACTUATION_FAILED`   | MV actuation failed and restore was confirmed.                                         |
| 8    | `EXIT_CHECK_FAILED`       | A preflight check failed, or `--strict` rejected a warning.                            |

`tune_runs.outcome` stores only `Completed`, `Aborted`, or `Failed`. A write-back failure does not rewrite an already completed row. Exit 6 outranks exit 7 when restore is incomplete.

## OPC DA integration

`OpcDaDriver::connect(host, server)` passes `host:port` to `Client::connect`. The default port is 7600. The ProgID is stored on the driver and sent with every later call. Gateway info and server listing are pre-connection free functions, not `Driver` methods. Server listing returns servers registered on the gateway machine.

The supported contract is `opcda-bridge` 0.5 or newer: capabilities, bounded browse pages, session close, live search, persistent indexed search, and unary read/write. CLI commands are `bhtune opc gateway-info`, `servers`, `read`, `write`, `browse`, `search`, `search-index`, and `close`. A CLI browse session stays open until `bhtune opc close <session-id>`. The browser closes its session when the modal closes.

An accepted OPC write is not proof the controller moved. The confirmation rule is in the live-plant section below.

The Windows installer always installs `bhtune.exe` and `bhtune-server.exe`. It can also install a pinned, checksum-verified official 32-bit `opcda-bridge-gateway`; it never downloads a gateway at runtime. Base-installer and gateway-extension acceptance is recorded in [`docs/internal/design/windows-installer.md`](docs/internal/design/windows-installer.md). Attaching that installer to a stable release remains deferred until the coordinated release-workflow change.

Release archives, the Docker image, deb/rpm packages, and the guarded AUR package ship both binaries together. The UI is browser-served, so there is no GUI toolkit to split out. See [`docs/internal/design/packaging-and-release.md`](docs/internal/design/packaging-and-release.md).

## Live-plant safety hardening

`cli-safety` covers validation, cancellation, OPC quality (`safety-quality`), and restore bounds (`safety-cancellation`). The closed finding write-up is [`docs/internal/design/safety-hardening.md`](docs/internal/design/safety-hardening.md).

- Validate before any live mutation. Relay amplitude is finite and from 0.1 through 50 percent. `cycles_count` is at least 1. `mrft_delay_secs` is at most 3600. PV bounds must differ. MV bounds must be finite and ordered `low < high`. The initial MV must lie inside that range. There is no `--dry-run`. Omitting `--write-pid` is the non-writing run.
- `--write-pid` requires `--yes` on every path, including an interactive one. Reject the combination before connection or database writes.
- `[tuning].timeout_secs` defaults to 3600 and cannot be disabled. Ctrl+C and that timeout must reach an in-flight driver call through one process-wide `CtrlC` handle and `bounded_driver_call`, not only the idle wait between ticks. `[tuning].op_timeout_secs` caps one operation. `[tuning].restore_timeout_secs` is a separate restore budget.
- Every exit attempts restore for steps that actually succeeded. Attempt every restore step; do not stop at the first failure. Persist mode, mode attribute, and setpoint before the first mutating write. Record `restore_status` and a detail naming each failed step.
- A second Ctrl+C during restore, or a restore timeout, is exit 6. After the authoritative MV restore write is accepted, extend the effective deadline to at least four seconds after acceptance so confirmation cannot be cut short.
- `Good` always passes. `Bad` never passes. `Uncertain` passes only when `allow_uncertain_quality` is enabled, and every such acceptance logs a warning. Enforce this on initial reads, the poll, and PID confirmation. Store a rejected poll sample, then abort and restore.
- PID write-back pre-reads P, I, and D before any write. Write and verify one constant at a time. Tolerance is 0.001 absolute or 1 percent relative. Roll back only constants whose own write-and-verify succeeded. A failed rollback points the operator at `bhtune history revert <run-id>`.
- `history revert` checks the run, OPC DA driver, write row, previous values, `--yes`, and PID tags before connecting. It uses the recorded server and bridge host. A contradicting flag is an error. A revert records the current values as its own previous values and does not nest another rollback.
- An accepted OPC DA MV write is not actuation. Track each accepted relay or restore command in `tune_mv_actuations`. Confirm the exact target within four seconds. A matching readback that completes after the deadline is still a failure. While a relay is pending, poll PV and MV together, map by tag identity, and judge MV evidence before the engine advances. Do not issue the replacement relay after a failed confirmation. Exit 7 means actuation failed and restore was confirmed. Exit 6 still wins when restore is incomplete. Simulator and replay do not run this live check. Commanded MV remains the sample and trend series. Measured MV stays in the actuation audit, not the normal browser table.
- Invalid calculated results are not writable. Sampling adequacy and latency summaries are advisory. They do not abort a run or block a valid write. A shared PV/MV batch can populate both PV-read and MV-verification latency categories; those durations overlap and must not be added.
- `restore_from` takes the pool by value, checks integrity, requires an exclusivity probe, copies with `VACUUM INTO`, and replaces the file through a same-directory temporary plus rename. A busy database is `DatabaseInUse`. The probe is point-in-time, not a held lock.
- Backup and restore stay library APIs until a command is explicitly added. Do not restore a database another process still has open.

Official gateway deployment, when explicitly requested, uses one checksum-verified GitHub artifact on every configured host. Do not substitute a local build, and do not write a tag as a release smoke test.

## History explorer

Retention is off unless a positive day count is configured. `history prune`, startup, and the server's periodic sweep share one cutoff. Startup failure is fatal. The server sweeper logs and continues.

Run detail can export CSV or JSON and can delete a terminal run. Delete checks the database outcome, not the in-memory active-run registry. The trend adds presentation-only initial and restored-MV points and reserves 12 poll intervals on a short run. Continuous historization and cross-run overlay are not implemented. At current data volumes, retaining history forever is safer than an unexpected auto-delete.

## Web app architecture

One HTTP API, described by OpenAPI, consumed by one generated client. There is no second transport and no swappable client interface. The release binary embeds `frontend/dist/`. `index.html` is `Cache-Control: no-cache`; content-hashed assets are immutable. A dotted missing asset is 404. A client route falls back to `index.html`. Unknown `/api/*` returns JSON 404, not the SPA shell. Live samples use server-sent events, not WebSocket. `build.rs` creates `frontend/dist/` before the embed derive runs, so a missing directory at compile time cannot permanently reject every asset.

`bhtune-server healthcheck` is a read-only liveness probe dispatched before server startup, SCM dispatch, logging, and database bootstrap. It sends one raw HTTP/1.1 request to loopback `/api/health`, using the port from `BHTUNE_BIND > config file > default`, with a three-second overall deadline. The Docker image uses this command for its built-in health status. The probe does not check database readiness or live-plant safety.

Write and revert use one styled review modal and name the loop, tags, and exact values. Calculated results move above the trend once they exist. Sampling diagnostics start collapsed. The normal page does not render the raw actuation table.

Rationale: [`docs/internal/design/architecture-decisions.md`](docs/internal/design/architecture-decisions.md) and [`docs/internal/design/frontend.md`](docs/internal/design/frontend.md).

## Documentation contract

A behavior change updates, in order: generated references, this file when an invariant, command, safety rule, or correctness tag changes, [`README.md`](README.md) when a new user would notice, and the prose guide for that area. Put new rationale in `docs/internal/design/` instead of pasting a diary here.

The docs-drift hook warns when a session changes Rust, user-visible `frontend/src/**`, or screenshot tooling without touching a documentation surface. It is a backstop, not permission to skip the update. This file stays on that surface list.

The docs agent may update narrative prose and the workflow-owned screenshot lock on a same-repository pull request. It must not edit this file. `--deny-tool 'write(AGENTS.md)'` and the post-run path check stay in place. A secrets-gated docs step must not run on a fork pull request that cannot receive the secret.

## Documentation system

Tier 1 is generated: `docs/reference/**`, OpenAPI, man pages, and completions. CI diffs it. Tier 2 is narrative prose plus the workflow-owned Web UI screenshot lock. Tier 3 is this file, which the docs agent must not edit. Release snapshot generation exists. The first stable snapshot waits for the approved release. The published site is [bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/). Design and agent guardrails: [`docs/internal/design/docs-site-and-generated-docs.md`](docs/internal/design/docs-site-and-generated-docs.md).

## Correctness-critical design details

Condensed register. Item numbers and decision tags match [`docs/internal/design/correctness-register.md`](docs/internal/design/correctness-register.md). Do not enable a compat flag on a production path.

1. **`[fixed, compat flag available]`** The MV lower clamp is the distance from the initial MV down to the floor, not an expression that adds the floor back onto the initial value. Compat: `MrftCompat.replicate_lower_clamp_bug`.
2. **`[fixed, compat flag available]`** Oscillation period uses full-precision elapsed milliseconds on the default path. It does not truncate to whole seconds or wrap at 24 hours. Compat: `TuningMathCompat.replicate_period_truncation_bug`. Whole-second tests do not prove this.
3. **`[structurally impossible]`** A switch reuses the tick timestamp. `bhtune-core` cannot call `Utc::now()`.
4. **`[structurally impossible]`** Process-type lookup tables have length 6, matching `ProcessType::ALL`.
5. **`[preserved rule]`** A tabular export header and its rows come from one ordered field list.
6. **`[fixed, no flag needed]`** PID unit labels follow the current process type and template. Historical result labels come from the run's template snapshot, not the mutable catalog. The React UI re-derives labels on render.
7. **`[fixed, no flag needed]`** Tag derivation uses the active template suffix and replaces the component after the last `.`, `!`, or `/`. It does not append a hardcoded `.PV`.
8. **`[fixed, no flag needed]`** Relay amplitude is range-checked by `LoopConfig::validate`. A blank check is not sufficient, and the legacy numeric debug codes are not accepted.
9. **`[fixed, no flag needed]`** Exports use an explicit path or stdout. Logs use the resolved platform data directory. There is no hardcoded developer path.
10. **`[fixed, no flag needed]`** Simulator mode is the explicit `--driver simulator` choice. No magic tag name selects it, and it does not skip restore.
11. **`[fixed, no flag needed]`** PID type, units, and controller direction are enums, not comparisons against display strings.
12. **`[preserved rule]`** PID is offered only for the two temperature process types. Every other process type offers P and PI. See `ProcessType::allows_pid()`.
13. **`[preserved rule]`** Skip, count, and noise-protection defaults come from the process-type tables when the process type changes. CLI and HTTP callers may still omit them for server-side defaulting.
14. **`[preserved rule]`** On the final MRFT step, MV snaps back to the initial value instead of taking a full relay step.
15. **`[fixed by design in this project, not a compat concern]`** Significant-digit formatting is a frontend rendering choice. The engine does not persist a legacy formatted string.
16. **`[new feature, not a legacy bug]`** The live and historical PV/MV trend uses `uPlot`. Short trends reserve 12 configured poll intervals. The blank future area is intentional.
17. **`[not applicable — feature dropped]`** There is no license or loop-locking ledger, so the legacy null-connection open bug has no equivalent path.
18. **`[not applicable — feature dropped]`** There is no log encryption and no login gate.
19. **`[fixed, no flag needed]`** An accepted MV write is not physical actuation. Confirmation, exit precedence, and latency overlap are in the live-plant section.
20. **`[fixed, no flag needed]`** A base-tag change resets Custom tag mappings and custom direction or range read mappings. Fixed-value direction and range mappings stay. Omitted direction or range values hydrate as template-tag sources, not fixed values.
21. **`[fixed, compat flag available]`** Result extrema include the switch sample and are not reset to `pv_value_ini`. Hysteresis extrema still use the legacy reset, so switching is unchanged. Compat: `MrftCompat.replicate_extrema_reset_bug`. Variant B, which would also seed hysteresis from the switch sample, is `[research-only]`. It needs closed-loop simulation and an explicitly approved noncritical trial. No such trial is part of the current implementation.
22. **`[fixed, no flag needed]`** A zero, negative, or non-finite amplitude, period, or converted P/I/D value is an invalid result with a reason and null numbers. Write paths reject it before connecting to a driver.
23. **`[advisory diagnostic]`** Sampling adequacy is `adequate` at 6.0 or more samples per measured period, `marginal` below that, and `not_assessed` without a usable period. Latency summaries are advisory and do not change validity or write permission.
24. **`[fixed, no flag needed]`** Saved OPC tag restoration keeps the tree mounted but covered until the selected row is visible. Errors, missing tags, cancellation, and fallback selection must clear that overlay. Reuse the shared loading primitives.

## Design notes

| Note                                                                                    | What it holds                                                  |
| --------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| [architecture-decisions.md](docs/internal/design/architecture-decisions.md)             | Scope, driver seam, OpenAPI, SPA embedding, and SQLite rules.  |
| [safety-hardening.md](docs/internal/design/safety-hardening.md)                         | Live-plant findings, restore, write-back, and actuation.       |
| [correctness-register.md](docs/internal/design/correctness-register.md)                 | Full numbered register and evidence.                           |
| [mrft-measurement.md](docs/internal/design/mrft-measurement.md)                         | Boundary correction and result validity.                       |
| [drivers.md](docs/internal/design/drivers.md)                                           | OPC DA, simulator, and replay drivers.                         |
| [opc-browser.md](docs/internal/design/opc-browser.md)                                   | Session-aware browse and indexed search.                       |
| [config.md](docs/internal/design/config.md)                                             | Precedence, tuning, and quality policy.                        |
| [cli.md](docs/internal/design/cli.md)                                                   | CLI surface and automation.                                    |
| [logging.md](docs/internal/design/logging.md)                                           | Tracing file plus stderr-only console mirroring.               |
| [templates.md](docs/internal/design/templates.md)                                       | Catalog, provenance, user catalog, import, export, and delete. |
| [persistence.md](docs/internal/design/persistence.md)                                   | Schema, history, backup, and retention.                        |
| [server-and-api.md](docs/internal/design/server-and-api.md)                             | HTTP API, tune start, and OpenAPI.                             |
| [frontend.md](docs/internal/design/frontend.md)                                         | SPA screens, live stream, and OPC browser.                     |
| [demo-mode.md](docs/internal/design/demo-mode.md)                                       | Public simulator Demo boundary and fixed limits.               |
| [testing.md](docs/internal/design/testing.md)                                           | Simulator, Playwright, and release-matrix tests.               |
| [validation-golden-replay.md](docs/internal/design/validation-golden-replay.md)         | Golden-master replay.                                          |
| [packaging-and-release.md](docs/internal/design/packaging-and-release.md)               | Archives, Docker, deb/rpm, and AUR.                            |
| [windows-installer.md](docs/internal/design/windows-installer.md)                       | NSIS installer and optional gateway.                           |
| [docs-site-and-generated-docs.md](docs/internal/design/docs-site-and-generated-docs.md) | Docs site, rustdoc, and the docs agent.                        |
| [ci-and-hardening.md](docs/internal/design/ci-and-hardening.md)                         | CI, security workflows, and API compatibility.                 |
| [cla.md](docs/internal/design/cla.md)                                                   | CLA text and enforcement workflow.                             |

## Open questions

Whether site-specific DCS/PLC templates should remain shareable JSON/TOML exports in addition to the SQLite rows.
