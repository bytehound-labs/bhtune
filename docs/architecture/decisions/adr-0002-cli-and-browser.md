---
sidebar_position: 3
---

# ADR-0002: Use CLI and browser-based application adapters

**Status:** Accepted

## Context

BHTune needs both headless execution for scripts and an interactive operator interface. The
public interface is a CLI and a browser GUI, both backed by the same tuning engine and run
history, as described in the [project introduction](../../intro.md#design-principles).

## Decision

The supported application adapters are the `bhtune` CLI and a React SPA served by
`bhtune-server`. The server exposes the HTTP API and serves the built browser interface; a
native desktop shell is not part of this architecture.

## Consequences

The browser interface uses the server's HTTP and SSE contracts, while the CLI can run without
the server. Release builds package the browser assets with `bhtune-server`, keeping the web
interface independent of a separate Node.js or static-file service on the target host.
