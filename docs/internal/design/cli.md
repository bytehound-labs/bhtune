# CLI

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## CLI reference (`cli-commands`)

`bhtune-cli` (binary name `bhtune`) is the `clap` adapter over `bhtune-runtime`. It owns
command parsing, terminal prompts, exit codes, and command output; the runtime owns shared
configuration, database bootstrap, driver setup, and tune orchestration. Data commands use
the same SQLite database through `bhtune_runtime::db::open`, which also seeds the four
built-in templates, and share one dispatcher in `lib.rs::run_with_cli`.

- **`bhtune tune`** — runs a full MRFT test against a named template: resolves the template,
  converts `TuneArgs` to the runtime's `TuneRequest`, then calls runtime `prepare()` and
  `drive()` services. The runtime resolves the template and tag set (`build_loop_tags`),
  selects a driver (`bhtune_runtime::driver::build`, `--driver opcda|simulator`), transitions
  the loop to Manual, polls at the global `[tuning].poll_interval_ms` while driving a real
  `MrftEngine`, persists every tick
  (`TuneSampleRow::insert`) and the final per-response-level results
  (`TuneResultRow::insert`), restores the loop's original mode, and optionally writes back one
  response level's PID constants with a stdin confirmation prompt (`maybe_write_back`) —
  audited via `TuneWriteRow`. `[tuning].mrft_delay_secs` pads the run with pre-/post-test
  recording-only ticks (PV still read and logged; no switch evaluation), matching the legacy
  delay behavior. `[tuning].timeout_secs` adds a mandatory-unattended-operation guardrail —
  see "Safety" below. A run's outcome (`Completed`/`Aborted` on Ctrl+C or
  `[tuning].timeout_secs`/`Failed` on any setup or mid-poll error) is always recorded in `tune_runs`
  before the process returns, even on failure.
- **`bhtune simulate`** — a zero-configuration wrapper around `tune` that forces
  `--driver simulator` against a synthetic FOPDT process (`SIMULATOR_PV_TAG`/
  `SIMULATOR_MV_TAG`), for a demo/smoke-test run with no real DCS/PLC needed.
  `SimulateArgs::into_tune_args` converts to the same `TuneArgs` `tune` uses, then the CLI
  adapter maps both to the runtime request type so they share the same execution path.
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
- **`bhtune opc gateway-info|servers|read|write|browse|search`** — low-level passthrough
  straight to the `opcda-bridge` gateway (via `opcda_bridge::Client`, bypassing the tuning
  engine entirely) for diagnostics. `gateway-info` reports gateway-wide application and
  protocol metadata without contacting an OPC server. `browse` returns one bounded page by
  default and accepts opaque session/node/page-token values; `--all` explicitly drains
  continuation pages. `search` reports progressive matches and completion/truncation metadata
  while keeping machine-readable JSON on stdout.

The CLI adapter delegates configuration, logging, database setup, tune execution, and live
plant safety checks to `bhtune-runtime`. It owns only command-specific parsing, prompts,
presentation, and process exit codes; see "Automation" and "Logging" for those adapter
contracts.

**Testing approach.** Runtime tests characterize tune preparation, execution, restore,
write-back, cancellation, and the simulator path with `MockDriver` and `SimulatorDriver`
fixtures. OPC DA driver and CLI passthrough tests use the shared unpublished
`bhtune-test-support` mock gRPC `Bridge` service; neither needs a real gateway or OPC DA server.
CLI tests separately cover argument parsing, command dispatch, terminal output, and process
exit behavior.

`run_polling_loop`'s cancellation path (and the `Aborted`-outcome branches in runtime tune
execution) is covered by `tests/ctrlc_abort.rs`, a black-box
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
= 4` (`[tuning].timeout_secs` elapsed before the test finished), `EXIT_POOR_QUALITY = 5` (a
  non-`Good` OPC sample aborted the run — see the OPC-quality bullet under "Live-plant safety
  hardening" above), and `EXIT_RESTORE_INCOMPLETE = 6` (the post-run restore could not be
  confirmed within `[tuning].restore_timeout_secs`, or was cut short by a second Ctrl+C — see
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

## History retention and pruning

Age-based deletion of `tune_runs` (and their cascaded samples/results/write-back audit rows) older
than a configurable number of days, off by default (retain forever). Runtime
`resolve_retention_days` resolves the policy through the usual `CLI --retention-days >
BHTUNE_RETENTION_DAYS env > retention_days in bhtune.toml > (no default)` precedence, and a new
shared `bhtune_runtime::retention` module (`cutoff_for`, `sweep_retention`) is the single place that turns "N
days" into an actual delete — used identically by `db::open`'s startup sweep (both binaries),
`bhtune-server`'s periodic 24-hour ticker while it keeps running, and `history prune`'s
real-deletion path, so a preview and an actual sweep can never disagree about which runs are in
scope. The startup sweep is fatal on failure (propagates via `?`, matching the existing
template-seeding precedent — a one-shot CLI/server-startup invocation should fail fast rather than
silently proceed against a possibly-broken database); the server's periodic sweep instead logs a
warning and continues, since a background maintenance hiccup must never crash a long-running server
out from under an in-flight HTTP connection or tune. `bhtune history prune` (`--older-than-days` to
override the configured policy for one invocation, required if no policy is configured at all;
`--dry-run` to report a count and cutoff without deleting anything, via `TuneRunRow::count` against
the identical filter shape the real sweep uses; `--output json`) completes the four-subcommand
`history` surface (`list`/`show`/`revert`/`prune`) started under
`cli-commands`/`safety-writeback-rollback`.
