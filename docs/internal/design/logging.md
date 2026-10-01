# Logging

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Shared application logging

`bhtune-runtime` provides structured `tracing`/`tracing-subscriber` logging for both adapters,
matching `opcda-bridge-gateway`'s own
stack and `log.*` config conventions (level/directory/format/rotation, resolved through the
same `CLI flag > env var > TOML config file > default` precedence as every other setting —
see "Config precedence" above), adapted for one hard constraint: it must never be able to
corrupt `--output json`'s single-object stdout contract (see "Automation" above).

- **`--log-level`** (env `RUST_LOG`) — an `EnvFilter` directive spec, e.g. `"debug"` or
  `"bhtune_cli=debug,bhtune_runtime=debug,sqlx=warn"`; defaults to `info`, and falls back to `info` on a spec that
  fails to parse rather than erroring — a config typo shouldn't stop a tune from running.
- **`--log-dir`** — defaults to a platform-standard data directory
  (`config::default_log_dir_from`, the same precedence machinery `cli-config`'s DB path
  already uses, not a directory next to the binary).
- **`--log-format`** — `pretty` (human-readable, ANSI-free — log files aren't a terminal) or
  `json` (newline-delimited, for log shippers). Defaults to `pretty`.
- **`--log-rotation`** — `hourly`, `daily`, or `never`. Defaults to `daily`.
- **`[log]` in `bhtune.toml`** — `level`/`dir`/`format`/`rotation` keys underneath config-file
  precedence, mirrored 1:1 with the CLI flags above via `LogConfig` in `bhtune-runtime`.

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

**Initialized once by each adapter.** `bhtune-cli`'s `lib.rs::run()` loads the config, resolves
`default_log_dir`, calls `logging::resolve_log_settings`/`logging::init_tracing`, holds the
returned `WorkerGuard` for the rest of the process's life (dropping it early would silently
truncate buffered lines not yet flushed on exit), then delegates to `run_with_cli`. This
keeps logging setup fully decoupled from `run_with_cli`'s own large, injection-based test
suite (zero existing tests call `run()` directly) and means `cargo test` never touches a real
platform log directory. `bhtune-server`'s bootstrap uses the same runtime resolver and
initializer, retaining its guard for the server lifetime. Both treat initialization as
best-effort (`let _log_guard = ...`, no
`?`) — an unwritable log directory shouldn't prevent a user from getting their tune's actual
result or the server from starting, a deliberate deviation from the gateway's hard-error approach.

**Instrumentation is added at meaningful points**, not exhaustively: database open and template
seed count, driver construction for both the OPC DA and simulator branches, and the runtime's
tune orchestration — run start/finish, the `Err` path, both abort
branches (Ctrl+C and the global `[tuning].timeout_secs`), MRFT engine completion, a per-tick trace event, and
write-back outcomes (success/readback-failure/rejected) in `maybe_write_back`. Bare
`tracing::*!` calls are always safe to sprinkle through already-tested code with no dedicated
new tests, because `tracing`'s global-subscriber single-assignment semantics mean events are
silently dropped whenever no subscriber is installed (as in every other test in the suite) —
they only do anything once a real `init_tracing` call succeeds, which only happens in
`tests/ctrlc_abort.rs`'s one real subprocess.

**Testing approach.** `bhtune-runtime`'s logging tests cover `parse_log_format`/`parse_rotation`
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
