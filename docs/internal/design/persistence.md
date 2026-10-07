# Persistence

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Persistence design decisions

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
- **Live ownership and recovery are structured, additive evidence.** Migration
  `0003_live_recovery.sql` adds owner and canonical-resource claim tables, per-mutation audit
  rows, recovery-attempt audit, and `tune_runs.recovery_state`/
  `recovery_evidence_json`. Heartbeat age, process identifiers, and error text do not establish
  orphan eligibility. Recovery requires the persisted ownership claim, initial readings,
  recorded connection and resource identity, effective restore policy, and mutation evidence;
  legacy rows without that evidence remain ineligible. Startup exports affected evidence
  before a conditional ownership transition, and a failed recovery audit never becomes a
  successful run transition. User deletion and age-based retention preserve rows whose
  recovery state is `eligible`, `running`, or `incomplete` until recovery is confirmed.
- **`tune_results` (calculated) and `tune_writes` (actually written to the DCS) are separate
  tables.** A run can produce three calculated candidate results and zero or more writes;
  conflating "the tool suggested this" with "this went into the controller" would lose the one
  fact the legacy CSV logs never captured — see `history-writeback-audit`.
- **Template PID precision is explicit and snapshotted.** Migration
  `0004_pid_rounding.sql` adds constrained `pid_rounding_kind` and
  `pid_rounding_digits` columns without changing earlier migration checksums.
  Run template snapshots include their precision policy; missing snapshot
  precision is not inferred from a template name. Raw result columns remain
  unrounded. Requested write columns contain rounded active controller terms
  and exact disabled-term sentinels; previous and readback columns remain
  independent observations and are not quantized.
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
  its schema, the same as opening an old database file normally would. Production filesystem
  checks and replacement use Tokio's asynchronous filesystem APIs; synchronous filesystem
  calls remain limited to test setup and assertions.
