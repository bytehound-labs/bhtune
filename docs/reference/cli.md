# bhtune CLI reference

This document contains the help content for the `bhtune` command-line program.

**Command Overview:**

* [`bhtune`↴](#bhtune)
* [`bhtune tune`↴](#bhtune-tune)
* [`bhtune simulate`↴](#bhtune-simulate)
* [`bhtune template`↴](#bhtune-template)
* [`bhtune template list`↴](#bhtune-template-list)
* [`bhtune template show`↴](#bhtune-template-show)
* [`bhtune template import`↴](#bhtune-template-import)
* [`bhtune template export`↴](#bhtune-template-export)
* [`bhtune template delete`↴](#bhtune-template-delete)
* [`bhtune history`↴](#bhtune-history)
* [`bhtune history list`↴](#bhtune-history-list)
* [`bhtune history show`↴](#bhtune-history-show)
* [`bhtune history revert`↴](#bhtune-history-revert)
* [`bhtune history prune`↴](#bhtune-history-prune)
* [`bhtune export`↴](#bhtune-export)
* [`bhtune opc`↴](#bhtune-opc)
* [`bhtune opc servers`↴](#bhtune-opc-servers)
* [`bhtune opc read`↴](#bhtune-opc-read)
* [`bhtune opc write`↴](#bhtune-opc-write)
* [`bhtune opc browse`↴](#bhtune-opc-browse)
* [`bhtune opc close`↴](#bhtune-opc-close)
* [`bhtune opc search`↴](#bhtune-opc-search)
* [`bhtune opc search-index`↴](#bhtune-opc-search-index)
* [`bhtune opc search-index status`↴](#bhtune-opc-search-index-status)
* [`bhtune opc search-index search`↴](#bhtune-opc-search-index-search)
* [`bhtune opc search-index refresh`↴](#bhtune-opc-search-index-refresh)
* [`bhtune opc search-index control`↴](#bhtune-opc-search-index-control)

## `bhtune`

Headless MRFT auto-tuner

**Usage:** `bhtune [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `tune` — Run an MRFT tune against a real OPC DA loop or the in-process simulator
* `simulate` — Run a zero-configuration demo MRFT tune against the built-in FOPDT simulator
* `template` — Inspect and manage DCS/PLC templates
* `history` — Inspect past tune runs
* `export` — Export one run's recorded samples as CSV or JSON
* `opc` — Low-level OPC DA passthrough (diagnostics) via the opcda-bridge gateway, bypassing the tuning engine entirely

###### **Options:**

* `--config <PATH>` — Path to a TOML config file (default: platform-specific, see `crate::config`)
* `--db <PATH>` — Path to the SQLite database file (default: a platform-standard data directory, see `crate::config::default_db_path_from`). CLI > `BHTUNE_DB` env var > `db` in the config file > platform default -- see `crate::config::resolve_db_path`
* `--templates <PATH>` — Path to a user-supplied DCS/PLC template catalog, auto-loaded on every startup in addition to the built-in templates (default: platform-specific, next to the config file -- see `crate::config::templates_path_from`). A missing file at the default location is fine; a file that fails to parse or validate is a hard error. CLI > `BHTUNE_TEMPLATES` env var > `templates` in the config file > platform default -- see `crate::config::load_user_templates`
* `--retention-days <RETENTION_DAYS>` — Delete tune runs (and their samples/results/write-back audit rows) older than this many days, automatically, on every startup (default: unset -- retain forever). CLI > `BHTUNE_RETENTION_DAYS` env var > `retention_days` in the config file > (no default) -- see `crate::config::resolve_retention_days`. `bhtune history prune` applies the same policy on demand, with a `--dry-run` preview, instead of waiting for the next startup
* `--log-level <LOG_LEVEL>` — Log level / directive spec, e.g. "info" or "bhtune_cli=debug,sqlx=warn" (default: info). Diagnostic detail only -- never printed to stdout, so it can never interleave with `--output json`'s single-object contract; see `crate::logging`
* `--log-dir <PATH>` — Directory to write log files to (default: a platform-standard data directory, see `crate::config::default_log_dir_from`)
* `--log-format <LOG_FORMAT>` — Log file format: "pretty" or "json" (default: pretty)
* `--log-rotation <LOG_ROTATION>` — Log file rotation: "hourly", "daily", or "never" (default: daily)



## `bhtune tune`

Run an MRFT tune against a real OPC DA loop or the in-process simulator

**Usage:** `bhtune tune [OPTIONS] --tagname <TAGNAME> --template <TEMPLATE> --process-type <PROCESS_TYPE> --controller-type <CONTROLLER_TYPE> --relay-amp <RELAY_AMP> --driver <DRIVER>`

###### **Options:**

* `-t`, `--tagname <TAGNAME>` — PV tag prefix; the rest of the tag set is derived from it using `--template`'s suffix convention. Ignored for `--driver simulator`, which uses two fixed internal tag names instead
* `--template <TEMPLATE>` — DCS/PLC template name (see `bhtune template list`)
* `--process-type <PROCESS_TYPE>`

  Possible values: `flow`, `pressure-line`, `pressure-vessel`, `level`, `temperature-mixing`, `temperature-heat-exchange`

* `--controller-type <CONTROLLER_TYPE>`

  Possible values: `p`, `pi`, `pid`

* `--relay-amp <RELAY_AMP>` — Relay amplitude, as a percentage of the MV range
* `--cycles-skip <CYCLES_SKIP>` — Relay cycles to skip before counting begins (default: looked up per `--process-type`)
* `--cycles-count <CYCLES_COUNT>` — Relay cycles to count once the skip period ends (default: looked up per `--process-type`)
* `--noise-protection-secs <NOISE_PROTECTION_SECS>` — Seconds a switch must persist before it's accepted (default: looked up per `--process-type`)
* `--driver <DRIVER>` — Which driver drives this tune

  Possible values:
  - `opcda`:
    A real OPC DA server, reached through an opcda-bridge gateway
  - `simulator`:
    The in-process FOPDT simulator — no external dependency at all

* `--bridge-host <BRIDGE_HOST>` — opcda-bridge gateway address. bhtune connects to the bridge gateway rather than a DCOM host directly — see AGENTS.md's OPC DA integration notes. Only meaningful with `--driver opcda` (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via the `BHTUNE_BRIDGE_HOST` env var or the config file's `bridge_host` key)
* `--server <SERVER>` — OPC DA server ProgID (legacy: `-s`/`--opcServerID`). Required with `--driver opcda`
* `--sim-gain <SIM_GAIN>` — Simulator process gain (`--driver simulator` only)

  Default value: `1`
* `--sim-tau <SIM_TAU>` — Simulator process time constant, in seconds (`--driver simulator` only)

  Default value: `2`
* `--sim-dead-time <SIM_DEAD_TIME>` — Simulator dead time, in seconds (`--driver simulator` only)

  Default value: `5`
* `--sim-noise <SIM_NOISE>` — Simulator measurement noise amplitude (`--driver simulator` only)

  Default value: `0`
* `--sim-seed <SIM_SEED>` — Simulator RNG seed, for reproducible noise (`--driver simulator` only)

  Default value: `0`
* `--sim-initial-pv <SIM_INITIAL_PV>` — Simulator initial PV (`--driver simulator` only)

  Default value: `50`
* `--sim-initial-mv <SIM_INITIAL_MV>` — Simulator initial MV (`--driver simulator` only)

  Default value: `50`
* `--pv-range-high <PV_RANGE_HIGH>` — Fixed PV range high, overriding a live tag read (legacy: the PV range "toggle tag/value" button). Required (defaults to 100.0) for `--driver simulator`, which has no range tags at all
* `--pv-range-low <PV_RANGE_LOW>` — Fixed PV range low, overriding a live tag read
* `--mv-range-high <MV_RANGE_HIGH>` — Fixed MV range high, overriding a live tag read
* `--mv-range-low <MV_RANGE_LOW>` — Fixed MV range low, overriding a live tag read
* `--direction <DIRECTION>` — Fixed controller direction, overriding a live tag read

  Possible values: `direct`, `reverse`

* `--notes <NOTES>` — Operator notes to attach to this run. Notes can be edited or cleared from the web GUI while the run is active or after it finishes
* `--yes` — Confirm an unattended PID write-back. Required alongside `--write-pid` -- the command refuses to start otherwise -- since writing to a live loop with no human present must be an explicit, deliberate choice. Has no effect without `--write-pid`
* `--write-pid <WRITE_PID>` — Non-interactively write this response level's calculated PID parameters back to the DCS instead of prompting on stdin -- the flag that makes a scheduled/scripted tune able to actually update a loop with no one watching. Requires `--yes`

  Possible values: `aggressive`, `moderate`, `sluggish`

* `--output <OUTPUT>` — How to print this run's final outcome line

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune simulate`

Run a zero-configuration demo MRFT tune against the built-in FOPDT simulator

**Usage:** `bhtune simulate [OPTIONS]`

###### **Options:**

* `-t`, `--tagname <TAGNAME>`

  Default value: `Sim.Loop1.PV`
* `--template <TEMPLATE>`

  Default value: `Yokogawa CentumVP`
* `--process-type <PROCESS_TYPE>`

  Default value: `flow`

  Possible values: `flow`, `pressure-line`, `pressure-vessel`, `level`, `temperature-mixing`, `temperature-heat-exchange`

* `--controller-type <CONTROLLER_TYPE>`

  Default value: `pi`

  Possible values: `p`, `pi`, `pid`

* `--relay-amp <RELAY_AMP>`

  Default value: `10`
* `--cycles-skip <CYCLES_SKIP>`
* `--cycles-count <CYCLES_COUNT>`
* `--noise-protection-secs <NOISE_PROTECTION_SECS>`
* `--sim-gain <SIM_GAIN>`

  Default value: `1`
* `--sim-tau <SIM_TAU>`

  Default value: `2`
* `--sim-dead-time <SIM_DEAD_TIME>`

  Default value: `5`
* `--sim-noise <SIM_NOISE>`

  Default value: `0`
* `--sim-seed <SIM_SEED>`

  Default value: `0`
* `--sim-initial-pv <SIM_INITIAL_PV>`

  Default value: `50`
* `--sim-initial-mv <SIM_INITIAL_MV>`

  Default value: `50`
* `--notes <NOTES>` — Operator notes to attach to this run. See [`TuneArgs::notes`]
* `--yes` — See `TuneArgs::yes`
* `--write-pid <WRITE_PID>` — See `TuneArgs::write_pid`. Note the built-in FOPDT simulator has no PID constant tags at all (see `build_loop_tags`), so write-back is always skipped for `simulate` regardless of this flag -- it's accepted here purely so `simulate`'s flag surface stays a strict defaulted subset of `tune`'s, matching every other field

  Possible values: `aggressive`, `moderate`, `sluggish`

* `--output <OUTPUT>` — See `TuneArgs::output`

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune template`

Inspect and manage DCS/PLC templates

**Usage:** `bhtune template <COMMAND>`

###### **Subcommands:**

* `list` — List every template (built-in and user-imported)
* `show` — Show one template's full detail as JSON
* `import` — Import a template from a file. Accepts either a single template as JSON (see `template export`'s default output shape) or a multi-template TOML catalog (the same `[[template]]` array-of-tables shape as the embedded/user catalog, see `template export --format toml`) -- the format is auto-detected from the file's content, not its extension. A JSON single-template import is rejected outright if a template with that name already exists; a TOML catalog import instead skips (and reports) any template whose name already exists, so re-importing an updated community catalog only adds what's new
* `export` — Export a template to a file, e.g. as a starting point for a site-specific copy or a community catalog contribution
* `delete` — Delete a template. Refuses if any saved loop still references it. A `Builtin`- or `Catalog`-origin template reappears automatically the next time bhtune starts unless it's also removed from its source (bhtune-core's embedded catalog for `Builtin`, which only a new bhtune release can change; the user catalog file for `Catalog`)



## `bhtune template list`

List every template (built-in and user-imported)

**Usage:** `bhtune template list`



## `bhtune template show`

Show one template's full detail as JSON

**Usage:** `bhtune template show <NAME>`

###### **Arguments:**

* `<NAME>`



## `bhtune template import`

Import a template from a file. Accepts either a single template as JSON (see `template export`'s default output shape) or a multi-template TOML catalog (the same `[[template]]` array-of-tables shape as the embedded/user catalog, see `template export --format toml`) -- the format is auto-detected from the file's content, not its extension. A JSON single-template import is rejected outright if a template with that name already exists; a TOML catalog import instead skips (and reports) any template whose name already exists, so re-importing an updated community catalog only adds what's new

**Usage:** `bhtune template import <PATH>`

###### **Arguments:**

* `<PATH>`



## `bhtune template export`

Export a template to a file, e.g. as a starting point for a site-specific copy or a community catalog contribution

**Usage:** `bhtune template export [OPTIONS] <NAME> <PATH>`

###### **Arguments:**

* `<NAME>`
* `<PATH>`

###### **Options:**

* `--format <FORMAT>` — File format to write. `toml` emits a single-entry `[[template]]` catalog block, ready to paste into a catalog file or open as a contribution pull request

  Default value: `json`

  Possible values: `json`, `toml`




## `bhtune template delete`

Delete a template. Refuses if any saved loop still references it. A `Builtin`- or `Catalog`-origin template reappears automatically the next time bhtune starts unless it's also removed from its source (bhtune-core's embedded catalog for `Builtin`, which only a new bhtune release can change; the user catalog file for `Catalog`)

**Usage:** `bhtune template delete <NAME>`

###### **Arguments:**

* `<NAME>`



## `bhtune history`

Inspect past tune runs

**Usage:** `bhtune history <COMMAND>`

###### **Subcommands:**

* `list` — List past runs, newest first
* `show` — Show one run's full detail: config, initial readings, calculated results, and any PID write-back audit rows
* `revert` — Undo a run's PID write-back, writing its recorded pre-write P/I/D values back to the live loop. Reverts whichever `write`-kind write-back that run last recorded; refuses if the run has none, if that write-back's pre-read itself failed (nothing to revert to), or if the run did not use the `opcda` driver (nothing live to revert against)
* `prune` — Delete runs older than the configured retention policy (`history-retention`), without waiting for the next automatic startup sweep



## `bhtune history list`

List past runs, newest first

**Usage:** `bhtune history list [OPTIONS]`

###### **Options:**

* `--outcome <OUTCOME>`

  Possible values: `running`, `completed`, `failed`, `aborted`

* `--limit <LIMIT>`

  Default value: `50`
* `--offset <OFFSET>`

  Default value: `0`
* `--output <OUTPUT>` — How to print the run list

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune history show`

Show one run's full detail: config, initial readings, calculated results, and any PID write-back audit rows

**Usage:** `bhtune history show [OPTIONS] <RUN_ID>`

###### **Arguments:**

* `<RUN_ID>`

###### **Options:**

* `--output <OUTPUT>` — How to print the run detail

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune history revert`

Undo a run's PID write-back, writing its recorded pre-write P/I/D values back to the live loop. Reverts whichever `write`-kind write-back that run last recorded; refuses if the run has none, if that write-back's pre-read itself failed (nothing to revert to), or if the run did not use the `opcda` driver (nothing live to revert against)

**Usage:** `bhtune history revert [OPTIONS] <RUN_ID>`

###### **Arguments:**

* `<RUN_ID>`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>` — Cross-checked against the run's own recorded bridge host -- never used to resolve a default, and deliberately has no `BHTUNE_BRIDGE_HOST`/config fallback the way every other command's `--bridge-host` does, so an unrelated ambient env var can never silently affect which gateway a revert targets. Omit this to use the recorded value; a value that contradicts it is refused rather than preferred, so a revert can never target a different gateway than the run it is undoing actually used (`db-run-request-snapshot`)
* `--server <SERVER>` — Cross-checked against the run's own recorded OPC server -- never used to resolve a default. Omit this to use the recorded value; a value that contradicts it is refused rather than preferred, so a revert can never target a different server than the run it is undoing actually used (`db-run-request-snapshot`)
* `--yes` — Confirm writing to a live loop. Required -- there is no interactive prompt for reverting, since there is no calculated result to choose between as there is for `tune`'s own write-back step
* `--output <OUTPUT>` — How to print the revert outcome

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune history prune`

Delete runs older than the configured retention policy (`history-retention`), without waiting for the next automatic startup sweep

**Usage:** `bhtune history prune [OPTIONS]`

###### **Options:**

* `--older-than-days <OLDER_THAN_DAYS>` — Delete runs older than this many days, overriding the configured `retention_days` policy for this invocation only. Required if no retention policy is configured at all (`--retention-days` / `BHTUNE_RETENTION_DAYS` / the config file's `retention_days` key) -- there is no default "prune everything older than X" to fall back to
* `--dry-run` — Report how many runs would be deleted, and as of what cutoff, without deleting anything
* `--output <OUTPUT>` — How to print the prune outcome

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune export`

Export one run's recorded samples as CSV or JSON

**Usage:** `bhtune export [OPTIONS] <RUN_ID>`

###### **Arguments:**

* `<RUN_ID>`

###### **Options:**

* `--format <FORMAT>`

  Default value: `csv`

  Possible values: `csv`, `json`

* `--output <OUTPUT>` — Output file path (default: stdout)



## `bhtune opc`

Low-level OPC DA passthrough (diagnostics) via the opcda-bridge gateway, bypassing the tuning engine entirely

**Usage:** `bhtune opc [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `servers` — List the OPC DA servers registered on the bridge gateway's host
* `read` — Read one or more tags
* `write` — Write a value to one tag
* `browse` — Browse one bounded page of tags. Without a session, lists the root level
* `close` — Explicitly release a gateway browse session returned by `opc browse`
* `search` — Search the OPC DA namespace without downloading the whole tree
* `search-index` — Query and manage the gateway's persistent namespace search index

###### **Options:**

* `--output <OUTPUT>` — How to print OPC diagnostic results

  Default value: `table`

  Possible values:
  - `table`:
    Human-readable text (default)
  - `json`:
    Pretty-printed JSON. This is the external contract for scripted/scheduled consumers, so its shape must not change silently once shipped




## `bhtune opc servers`

List the OPC DA servers registered on the bridge gateway's host

**Usage:** `bhtune opc servers [OPTIONS]`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>` — (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via `BHTUNE_BRIDGE_HOST` or the config file's `bridge_host` key.)



## `bhtune opc read`

Read one or more tags

**Usage:** `bhtune opc read [OPTIONS] [TAGS]...`

###### **Arguments:**

* `<TAGS>`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>` — (default: `crate::config::DEFAULT_BRIDGE_HOST`, overridable via `BHTUNE_BRIDGE_HOST` or the config file's `bridge_host` key.)
* `--server <SERVER>` — (default: the config file's `server` key; errors if neither is set.)



## `bhtune opc write`

Write a value to one tag

**Usage:** `bhtune opc write [OPTIONS] <TAG> <VALUE>`

###### **Arguments:**

* `<TAG>`
* `<VALUE>`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`



## `bhtune opc browse`

Browse one bounded page of tags. Without a session, lists the root level

**Usage:** `bhtune opc browse [OPTIONS]`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`
* `--session-id <SESSION_ID>` — Existing bridge browse session to continue or use with `parent-node-key`
* `--parent-node-key <PARENT_NODE_KEY>` — Opaque node key returned by a previous page
* `--page-token <PAGE_TOKEN>` — Opaque continuation token returned by a previous page
* `--page-size <PAGE_SIZE>` — Number of immediate children to request

  Default value: `200`
* `--all` — Follow continuation pages until the requested level is complete
* `--refresh` — Ask the gateway to refresh its namespace view



## `bhtune opc close`

Explicitly release a gateway browse session returned by `opc browse`

**Usage:** `bhtune opc close [OPTIONS] <SESSION_ID>`

###### **Arguments:**

* `<SESSION_ID>` — Opaque browse-session ID returned by `opc browse`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`



## `bhtune opc search`

Search the OPC DA namespace without downloading the whole tree

**Usage:** `bhtune opc search [OPTIONS] <QUERY>`

###### **Arguments:**

* `<QUERY>` — Text to find in node labels/item IDs

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`
* `--match-mode <MATCH_MODE>` — How the query should match

  Default value: `contains`

  Possible values: `exact`, `prefix`, `contains`

* `--max-results <MAX_RESULTS>` — Maximum number of matches

  Default value: `200`
* `--session-id <SESSION_ID>` — Existing bridge browse session to search within
* `--scope-node-key <SCOPE_NODE_KEY>` — Opaque node key limiting the search scope
* `--include-branches` — Include branch nodes as search results
* `--refresh` — Ask the gateway to refresh its namespace view



## `bhtune opc search-index`

Query and manage the gateway's persistent namespace search index

**Usage:** `bhtune opc search-index <COMMAND>`

###### **Subcommands:**

* `status` — Show persistent namespace-index status
* `search` — Search the persistent namespace index without traversing the live OPC tree
* `refresh` — Start or coalesce a persistent namespace-index refresh
* `control` — Pause, resume, or cancel a persistent namespace-index build



## `bhtune opc search-index status`

Show persistent namespace-index status

**Usage:** `bhtune opc search-index status [OPTIONS]`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`



## `bhtune opc search-index search`

Search the persistent namespace index without traversing the live OPC tree

**Usage:** `bhtune opc search-index search [OPTIONS] <QUERY>`

###### **Arguments:**

* `<QUERY>` — Text to find in indexed node labels/item IDs

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`
* `--match-mode <MATCH_MODE>` — How the query should match

  Default value: `contains`

  Possible values: `exact`, `prefix`, `contains`

* `--max-results <MAX_RESULTS>` — Maximum number of matches

  Default value: `50`



## `bhtune opc search-index refresh`

Start or coalesce a persistent namespace-index refresh

**Usage:** `bhtune opc search-index refresh [OPTIONS]`

###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`
* `--force` — Start a refresh even when the active index is already current



## `bhtune opc search-index control`

Pause, resume, or cancel a persistent namespace-index build

**Usage:** `bhtune opc search-index control [OPTIONS] <ACTION>`

###### **Arguments:**

* `<ACTION>`

  Possible values: `pause`, `resume`, `cancel`


###### **Options:**

* `--bridge-host <BRIDGE_HOST>`
* `--server <SERVER>`



