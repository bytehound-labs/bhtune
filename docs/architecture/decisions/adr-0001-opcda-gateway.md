---
sidebar_position: 2
---

# ADR-0001: Keep OPC DA behind a Windows gateway

**Status:** Accepted

## Context

BHTune runs on Linux, macOS, and Windows, while the OPC DA integration belongs at a Windows
boundary. Linking Windows COM/DCOM behavior into the tuning application would couple every
application build and runtime to that platform-specific interface. The
[project introduction](../../intro.md#design-principles) describes the cross-platform
application and its Windows-side gateway.

## Decision

The OPC DA driver communicates with a separate `opcda-bridge-gateway` over the network. The
gateway owns the Windows-specific connection to the OPC DA server; BHTune interacts with it
through the protocol-neutral `Driver` interface.

## Consequences

BHTune's CLI and server can run on supported non-Windows hosts. A live OPC DA deployment still
requires a reachable, compatible gateway on Windows with access to the OPC DA server. Gateway
availability and lifecycle are separate from the BHTune process.
