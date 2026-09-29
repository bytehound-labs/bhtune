# Testing

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Playwright end-to-end suite

A Playwright suite (`frontend/e2e/`) drives a full tune through the real, built React SPA served by
a real `bhtune-server` binary (debug profile, which serves `frontend/dist/` live off disk rather
than needing a re-embed step — see `server-embed-spa`'s `rust-embed` feature gating) running the
in-process simulator driver, with no mocked HTTP layer and no Vite dev server involved.
`smoke.spec.ts` covers the app shell, the health indicator reaching a real driver, the seeded
built-in template list, and header nav; `tune.spec.ts` drives `/runs/new` with the same
millisecond-scale simulator parameters `e2e_simulator.rs` uses and asserts the _rendered_ Kp/Ti/Td
values are sane and correctly ordered (not just that the page didn't crash), plus a second test
cancelling an in-flight run. The server now tracks multiple tune tasks independently, so separate
browser requests can start concurrently; only post-hoc PID writes/reverts remain mutually exclusive
with tunes. A third TypeScript project (`tsconfig.e2e.json`, referenced from `tsconfig.json`
alongside the existing `tsconfig.app.json`/`tsconfig.node.json`) wires `e2e/`/`playwright.config.ts`
into the existing `tsc -b`/`pnpm run build` gate, so the suite's own source is genuinely typechecked
in CI, not merely executed. A new `.github/workflows/e2e.yml` job builds a debug `bhtune-server`,
builds the frontend, installs Chromium via `playwright install --with-deps`, and runs the suite,
uploading the Playwright HTML report as a CI artifact on failure. This workflow's first real run
caught a genuine, previously-undiscovered production bug on its very first execution —
`bhtune-server`'s new `build.rs` now fixes a `rust-embed` compile-time build-order trap where
compiling the crate before `frontend/dist/` exists permanently breaks asset serving for that build,
regardless of build order afterward; see "`server-embed-spa`: embedding the built SPA into the
binary" below for the full mechanism and fix.

## End-to-end testing summary

`e2e-simulator` is a genuine subprocess-level test (`crates/bhtune-cli/tests/e2e_simulator.rs`)
spawns the real `bhtune tune` binary against the simulator driver across a small
process/controller-type matrix (all `direction=reverse`, the direction empirically confirmed to
actually oscillate against this simulator's fixed FOPDT parameters), then opens the resulting SQLite
database directly and compares every persisted Kp/Ti/Td and template-converted P/I/D value with
reviewed numeric baselines, alongside positive/ordered-Kp, response-level-invariant, lifecycle,
identity, and sample-trail checks. The matrix runs serially with a fixed 5 ms simulator cadence
shared by FOPDT process evolution and MRFT timestamps, so host scheduling affects runtime but not
the expected numeric results. Writing it surfaced and fixed a real `bhtune-core` bug in the process
— see item 2 of the [correctness register](correctness-register.md). The `e2e-playwright` Playwright
suite (`frontend/e2e/`) drives a full tune through the real, built React SPA served by a real
`bhtune-server` binary (debug profile -- serves `frontend/dist/` live off disk, no re-embed step
needed between runs) over the in-process simulator driver -- `smoke.spec.ts` (app shell, health
indicator, seeded template list, header nav) and `tune.spec.ts` (a full tune through `/runs/new`
with `e2e_simulator.rs`'s own millisecond-scale simulator parameters, asserting sane/ordered
rendered Kp/Ti/Td values, deterministic fixed-step timing diagnostics with zero missed poll
opportunities through the API, plus cancelling an in-flight run). Polling timing diagnostics are
persisted and exposed through CLI/API/logs for both simulator and live runs; the normal web
run-detail UI omits them, and live gaps at least twice the requested interval produce a warning
only, without changing tune or write-back outcomes. `.github/workflows/e2e.yml` builds a debug
`bhtune-server` and the frontend, installs Chromium, and runs the suite in CI, uploading the HTML
report on failure. A direct dividend of dropping Tauri: `tauri-driver`/WebDriver would have been
markedly more fragile in CI than plain Playwright against a real browser. `build-matrix`:
`.github/workflows/release.yml` builds and packages the `bhtune`+`bhtune-server` binaries for
Linux/macOS/Windows via `taiki-e/create-gh-release-action` + `taiki-e/upload-rust-binary-action`
(opcda-bridge's own tooling, in place of the originally-planned `cargo-dist`), building the frontend
first so the release build's `rust-embed` step captures real SPA assets — no Tauri bundler or
WebView runtime to manage. See [packaging-and-release.md](packaging-and-release.md) for the full
design. `e2e-golden-ci` needs no dedicated workflow step: `checks.yml`'s existing `cargo test
--workspace` already auto-discovers and runs `crates/bhtune-core/tests/golden_replay.rs` on every
push/PR to `main`, the same way `e2e-simulator`'s subprocess test rides the same step rather than a
separate job.

## Coverage enforcement

Coverage is tracked by Codecov and enforced at **exactly 100%** via `codecov.yml` (project and
patch targets both at 100% with zero tolerance). The coverage workflow independently parses every
canonical LCOV `DA` source-line record, fails on any zero-hit line, and regenerates the report's
`LF`/`LH` summaries from those records before upload, so LLVM's duplicated line-instance
accounting cannot produce a false failure and a successful upload cannot mask a real uncovered
source line. Even placeholder code must be exercised by a test — see the
`main_runs_without_panicking` smoke tests in each binary crate's `main.rs` for the pattern used
to keep the gate meaningful (not vacuous) from the very first commit. Delete each one once that
binary does something real and gains its own targeted tests.
