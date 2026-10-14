# Design records and canonical documentation

Public architecture pages and area guides own implementation/operating rationale. This
unpublished directory retains research and documentation-process records, not a second manual
or phase diary. Code and tests are authoritative; [`AGENTS.md`](../../../AGENTS.md) is the
agent contract.

## Retained records

| Record                                                  | Purpose                                                                            |
| ------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| [Correctness register](correctness-register.md)         | Numbered legacy fix decisions, preserved rules, and evidence requirements.         |
| [Golden replay](validation-golden-replay.md)            | Recorded-trace provenance, numeric tolerances, and limits of replay evidence.      |
| [Documentation system](docs-site-and-generated-docs.md) | Generated-reference ownership, publication, screenshot, and docs-agent guardrails. |

## Canonical public pages

| Area                            | Read and update                                                                                                                                                                                                                                                                                      |
| ------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Scope and responsibilities      | [Runtime topology](../../architecture/topology.md), [crate graph/driver seam](../../architecture/crate-graph.md), [roadmap](../../roadmap.md).                                                                                                                                                       |
| MRFT and measurement            | [Engine/time](../../architecture/engine-and-time.md), [simulator model](../../guides/simulator-model.md), plus the retained correctness/replay records.                                                                                                                                              |
| Safety and recovery             | [Safety model](../../architecture/safety-model.md), [operator safety](../../guides/safety.md), [runbook](../../guides/operator-runbook.md).                                                                                                                                                          |
| SQLite/history                  | [Persistence](../../architecture/persistence.md), [history UI](../../guides/web-ui/runs-and-history.md).                                                                                                                                                                                             |
| API and browser                 | [API/streaming](../../architecture/api-and-streaming.md), [frontend](../../architecture/frontend.md), [Web UI guides](../../guides/web-ui/overview.md).                                                                                                                                              |
| OPC namespace/index             | [Gateway compatibility](../../guides/opc-gateway-compatibility.md), [OPC form workflow](../../guides/web-ui/starting-a-tune.md#opc-da-connection-and-mapping).                                                                                                                                       |
| Templates/configuration/logging | [DCS/PLC templates](../../dcs-templates.md), [configuration UI](../../guides/web-ui/templates-and-configuration.md), [runbook](../../guides/operator-runbook.md#application-logging); generated [CLI](../../reference/cli.md)/[config](../../reference/config.md) references remain generator-owned. |
| Public Demo                     | [Simulator Demo guide](../../guides/public-simulator-demo.md).                                                                                                                                                                                                                                       |
| Distribution and releases       | [Packaging](../../architecture/packaging.md), [installation](../../getting-started/installation.md), [releasing](../../guides/releasing.md), [verification](../../guides/release-verification.md).                                                                                                   |
| Testing, CI, and contributions  | [CONTRIBUTING](../../../CONTRIBUTING.md), [CLA](../../../CLA.md), [security policy](../../../SECURITY.md).                                                                                                                                                                                           |

Keep facts with their canonical page. Add a separate record only for research/process evidence
that does not belong in those guides; link it instead of duplicating it.
