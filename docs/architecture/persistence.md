---
sidebar_position: 5
---

# Persistence and migration policy

BHTune stores templates, tune requests, samples, results, and write or actuation audit records
in a plain SQLite database. The file is open and inspectable with ordinary SQLite tools; BHTune
does not encrypt it or provide per-row access control. The CLI and server use `bhtune-db` for
the same schema and repository operations.

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

For database backup and restore behavior, see the
[Safety guide's recovery boundaries](../guides/safety.md#packaging-and-database-recovery-boundaries)
and the [installation guide](../getting-started/installation.md).
