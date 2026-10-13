# bhtune-db

SQLite persistence for BHTune templates, loop configuration, run history, tuning results,
write audits, and settings. The crate provides database connection helpers, embedded schema
migrations, typed repository APIs, and whole-database backup and restore operations.

`connect` applies the embedded schema to fresh databases and rejects incompatible SQLx
migration histories. Preserve a verified backup and use a new database file; incompatible
history is never rewritten automatically.

`connect_read_only` opens an existing database without creating it or applying migrations. It
rejects incompatible migration history, and SQL write statements are rejected by SQLite.

Template precision is stored in constrained kind/count columns and copied into each
run's template snapshot. Calculated results remain full precision; PID write audits
record controller-ready targets separately from original values and actual readbacks.

The database is plain and unencrypted. Protect the database file and its backups using the
operating system's access controls.

API documentation: <https://docs.rs/bhtune-db>
