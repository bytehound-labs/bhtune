---
sidebar_position: 5
---

# Persistence and migration policy

BHTune stores templates, tune requests, samples, results, and write or actuation audit records
in a plain SQLite database. The file is open and inspectable with ordinary SQLite tools; BHTune
does not encrypt it or provide per-row access control. The CLI and server use `bhtune-db` for
the same schema and repository operations.

The schema is embedded and applied through SQLx migrations when the database is opened. The
pre-v0.1 schema is represented by one consolidated migration,
`crates/bhtune-db/migrations/0001_initial_schema.sql`. This is the development baseline while
no supported user-held or deployed database depends on intermediate migration history.

Before v0.1, the migration baseline may be consolidated while its databases remain disposable
and no supported database depends on the old history. Once v0.1 establishes a supported
database format, applied migration history is a compatibility contract: schema changes use
new forward migrations rather than editing an already-applied migration.

For database backup and restore behavior, see the
[Safety guide's recovery boundaries](../guides/safety.md#packaging-and-database-recovery-boundaries)
and the [installation guide](../getting-started/installation.md).
