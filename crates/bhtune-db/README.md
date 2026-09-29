# bhtune-db

SQLite persistence for BHTune templates, loop configuration, run history, tuning results,
write audits, and settings. The crate provides database connection helpers, embedded schema
migrations, typed repository APIs, and whole-database backup and restore operations.

The database is plain and unencrypted. Protect the database file and its backups using the
operating system's access controls.

API documentation: <https://docs.rs/bhtune-db>
