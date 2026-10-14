---
sidebar_position: 5
---

# Persistence and migration policy

BHTune stores templates, tune requests, samples, results, and write or actuation audit records
in a plain SQLite database. The file is inspectable with ordinary SQLite tools and is not
encrypted; protect it with host filesystem permissions. Demo's visitor isolation is enforced
by the application and owner-scoped queries, not by separate SQLite user accounts.
The CLI and server use `bhtune-db` for the same schema and repository operations.

## Record layout

Stable fields used for filtering are ordinary columns. Nested or evolving values use JSON
with `json_valid` constraints and typed Rust decoding; syntactically valid JSON can still
have an invalid shape and must not be treated as usable data. Enum columns and their `CHECK`
constraints use the same snake_case literals as serde.

| Records                                                                                           | Responsibility                                                                                                                         |
| ------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `dcs_templates`, `loops`, `settings`                                                              | Template definitions/origins, saved loop data, and application settings such as the Full-mode New Tune draft.                          |
| `tune_runs`                                                                                       | Run identity, lifecycle, request/effective configuration, template/tag/connection snapshots, restore status, and recovery eligibility. |
| `tune_samples`                                                                                    | Ordered UTC/PV/quality/commanded-MV samples used by history, streaming, and exports.                                                   |
| `tune_results`                                                                                    | Per-response-level calculations, validity, and invalid-result reasons.                                                                 |
| `tune_writes`                                                                                     | PID write/revert targets, previous values, written/readback values, and rollback outcomes.                                             |
| `tune_mv_actuations`                                                                              | Accepted relay/restore commands, confirmation deadlines, observations, and final actuation status.                                     |
| `live_operation_owners`, `live_operation_claims`, `live_mutation_steps`, `tune_recovery_attempts` | Cross-process ownership, mutation intent/evidence, and explicit recovery audits.                                                       |
| `demo_sessions` and run ownership                                                                 | Hashed anonymous session state and visitor-owned simulator history.                                                                    |

Calculated results and controller writes are separate records: a completed calculation does
not imply that a PID write occurred or succeeded. Partial writes retain absent values as null,
not invented zeros. Measured MV evidence likewise does not replace the commanded-MV sample
series.

## Run snapshots and startup

A run snapshots its template, explicit PID precision, derived tags, submitted request, and OPC
connection before mutation. It does not depend on the catalog row surviving unchanged.
Historical units and controller targets come from that snapshot; write/revert/recovery cannot
silently redirect to a gateway resolved from today's configuration.

Startup re-applies `builtin` and `catalog` definitions only to rows with the matching origin;
it never overwrites a differently owned template or auto-edits a `user` row. See
[template ownership](../dcs-templates.md#where-templates-come-from).

Full-mode orphan handling exports evidence before ownership transitions and makes no controller
connection. Database rows, OS ownership, and mutation evidence work together; neither an
in-memory active-task registry nor a stale heartbeat is recovery authority. See the
[safety model](safety-model.md#durable-ownership-and-recovery).

## Migration policy

The schema is embedded and applied through SQLx migrations when the database is opened. The
pre-v0.1 schema is represented by one baseline migration,
`crates/bhtune-db/migrations/0001_initial.sql`. Writable and read-only opens compare every
SQLx migration-history version/checksum pair with the embedded set before migration or use.
A database from the earlier
pre-release migration chain is rejected with its path and backup-and-recreate guidance; BHTune
does not rewrite migration history, delete the database, or attempt a partial migration.
Preserve a verified backup and use a new database file.

While no supported database format exists, the pre-v0.1 baseline may be consolidated and
earlier pre-release databases remain unsupported. Once v0.1 establishes a supported database
format, applied migration history is a compatibility contract: schema changes use new forward
migrations rather than editing an already-applied migration.

## Retention and deletion

Retention is off by default. A positive day count uses one shared cutoff for CLI startup,
server startup/periodic sweeps, and `history prune`; the dry-run count uses the same deletion
filter. Startup sweep failure stops startup, whereas a periodic server failure is logged
without stopping the service.

Deleting a run cascades its samples, results, and write audits. The database outcome, not
absence from the HTTP registry, determines whether it is terminal. Runs with `eligible`,
`running`, or `incomplete` recovery state remain protected until recovery is confirmed.
Changing retention in the browser changes future sweeps, not an immediate deletion.

## Database backup and restore

`bhtune-db` exposes `backup_to` and `restore_from` as library APIs, not CLI commands.
`VACUUM INTO` creates a consistent snapshot including committed WAL content.
Restore consumes/closes the supplied pool, checks backup integrity and database exclusivity,
takes a pre-restore snapshot, and replaces the file through a same-directory temporary and
rename. A busy database returns `DatabaseInUse`.

The exclusivity probe is point-in-time, not a held lock preventing another process from opening
the file afterward. Stop every database user before replacement; never restore over another
process's open database. For operational copies, sidecars, and package-specific rollback
boundaries, follow the
[operator runbook](../guides/operator-runbook.md#10-backup-restore-retention-and-export) and
[safety guide](../guides/safety.md#packaging-and-database-recovery-boundaries).
