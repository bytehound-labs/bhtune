---
sidebar_position: 4
---

# ADR-0003: Use plain SQLite for application persistence

**Status:** Accepted

## Context

Run history must be local, portable, and inspectable by operators and administrators. The
[project introduction](../../intro.md#design-principles) describes SQLite as an open database
file rather than an application-managed access boundary.

## Decision

BHTune stores its templates, tune history, results, and audit records in a plain SQLite
database. It does not encrypt the database or implement per-row access control.

## Consequences

The database can be inspected and backed up with ordinary SQLite tools. File access and
protection remain responsibilities of the host and its operating-system controls; BHTune does
not provide a separate database security layer. The
[persistence policy](../persistence.md) describes the migration compatibility boundary.
