---
sidebar_position: 1
---

# Architecture decision records

These records capture shipped design choices that shape BHTune's boundaries. They summarize
the decision, its context, and its consequences; operator procedures and implementation
details remain in the linked guides and references.

| Record | Decision | Status |
| --- | --- | --- |
| [ADR-0001: OPC DA gateway boundary](adr-0001-opcda-gateway.md) | Keep Windows-specific OPC DA integration in a separate gateway. | Accepted |
| [ADR-0002: CLI and browser adapters](adr-0002-cli-and-browser.md) | Provide a headless CLI and a browser UI served by `bhtune-server`. | Accepted |
| [ADR-0003: Plain SQLite persistence](adr-0003-plain-sqlite.md) | Use one inspectable SQLite database without application-level encryption. | Accepted |
| [ADR-0004: Single-precision engine values](adr-0004-f32-engine.md) | Use `f32` for the core's process and tuning numerics. | Accepted |
| [ADR-0005: External scheduling](adr-0005-external-scheduling.md) | Use operating-system schedulers with the CLI rather than an in-product scheduler. | Accepted |

For the runtime picture, see [topology](../topology.md) and the
[crate graph](../crate-graph.md). For user-facing rationale and operation, see the
[project introduction](../../intro.md), [MRFT concepts](../../guides/mrft-concepts.md),
[safety guide](../../guides/safety.md), [CLI quickstart](../../getting-started/cli-quickstart.md),
and [release guide](../../guides/releasing.md). The repository
[README architecture section](https://github.com/bytehound-labs/bhtune/blob/main/README.md#architecture)
also summarizes the workspace.
