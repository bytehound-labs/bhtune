# Configuration

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

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
[`crates/bhtune-cli/bhtune.example.toml`](../../../crates/bhtune-cli/bhtune.example.toml) for every
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
| Server exposure mode  | —                  | `BHTUNE_SERVER_MODE`    | `server_mode`    | `full` — `demo` is the restricted, simulator-only public surface                                                                                                                                    |
| Browser origin        | —                  | `BHTUNE_ORIGIN`         | `origin`         | automatic same-host validation in Full mode when unset; explicit exact origin when configured; Demo requires one exact configured origin                                                            |
| Trusted proxy         | —                  | —                       | `trusted_proxy`  | none — forwarded client-IP headers are ignored unless the immediate peer matches this exact IP/CIDR                                                                                                 |

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

`server_mode` is resolved from `BHTUNE_SERVER_MODE` over the TOML value and defaults to
`full`; it is intentionally not exposed as a browser Config control. `origin` is resolved from
`BHTUNE_ORIGIN` over the TOML value. In Full mode, an unset value enables same-host Origin/Host
validation; an explicit value is a strict origin pin. In Demo mode, a value is mandatory and is
used for exact-Origin checks on state-changing requests. `trusted_proxy` is a startup-only
deployment setting, not a list of arbitrary forwarded addresses: Demo quota accounting trusts
one normalized client-IP header only when the direct peer is inside the configured boundary.

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

## Browser configuration page and global quality policy

The browser's `/config` page edits the same `bhtune.toml` file used by the CLI and server; it
does not create a second SQLite settings authority. It currently exposes the global
`allow_uncertain_quality` policy and `retention_days`, while startup-only settings such as the
database path, bind address, logging, and template-catalog path remain file/configuration
settings rather than live UI controls.

`allow_uncertain_quality` defaults to `true` when the key or file is absent. `Good` readings
always pass, `Uncertain` readings follow this global policy, and `Bad` readings are always
rejected. The policy is captured at the start of each tune and PID write/revert operation and
stored with the resulting history/audit rows, so changing Config cannot reinterpret an
operation already in progress or make its historical quality decision ambiguous.

Config saves preserve comments, unknown TOML keys, and unrelated formatting, validate the
patched document, create a timestamped sibling backup when replacing an existing file, and
atomically replace the live file. The server updates its live quality/retention snapshot after a
successful save. A stale browser revision or an external edit detected since startup/last save
returns `409 Conflict`; hand edits intentionally take effect after a server restart rather than
being hot-reloaded underneath the running process.

Retention remains `CLI --retention-days > BHTUNE_RETENTION_DAYS > bhtune.toml > retain forever`;
the TOML value must be a positive whole number when present, and zero is rejected at startup.
Saving Config changes the policy for future startup or scheduled sweeps and never deletes
history immediately. The server's periodic sweep reads the live snapshot each time; the CLI
continues to resolve its policy when a new invocation starts.
