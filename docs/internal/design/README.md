# Design notes

Durable design rationale for BHTune. [`AGENTS.md`](../../../AGENTS.md) is the agent contract: current architecture, invariants, commands, and safety rules. These notes hold the longer reasoning that contract points at.

Code and tests are authoritative when they disagree with a note. A behavior change belongs in the code, its tests, and the user-facing guide. Add or revise a note here only when the rationale itself changes. Do not paste a phase diary back into `AGENTS.md`.

These notes were preserved from the former long-form agent file so that rationale stayed reviewable after that file became a short contract. They are not a changelog and they are not generated.

| Note | What it holds |
| --- | --- |
| [architecture-decisions.md](architecture-decisions.md) | Scope, driver seam, OpenAPI, SPA embedding, and SQLite rules. |
| [safety-hardening.md](safety-hardening.md) | Live-plant findings, restore, write-back, and actuation. |
| [correctness-register.md](correctness-register.md) | Full numbered register and evidence. |
| [mrft-measurement.md](mrft-measurement.md) | Boundary correction and result validity. |
| [drivers.md](drivers.md) | OPC DA, simulator, and replay drivers. |
| [opc-browser.md](opc-browser.md) | Session-aware browse and indexed search. |
| [config.md](config.md) | Precedence, tuning, and quality policy. |
| [cli.md](cli.md) | CLI surface and automation. |
| [logging.md](logging.md) | Tracing file plus stderr-only console mirroring. |
| [templates.md](templates.md) | Catalog, provenance, user catalog, import, export, and delete. |
| [persistence.md](persistence.md) | Schema, history, backup, and retention. |
| [server-and-api.md](server-and-api.md) | HTTP API, tune start, and OpenAPI. |
| [frontend.md](frontend.md) | SPA screens, live stream, and OPC browser. |
| [demo-mode.md](demo-mode.md) | Public simulator Demo boundary and fixed limits. |
| [testing.md](testing.md) | Simulator, Playwright, and release-matrix tests. |
| [validation-golden-replay.md](validation-golden-replay.md) | Golden-master replay. |
| [packaging-and-release.md](packaging-and-release.md) | Archives, Docker, deb/rpm, and AUR. |
| [windows-installer.md](windows-installer.md) | NSIS installer and optional gateway. |
| [docs-site-and-generated-docs.md](docs-site-and-generated-docs.md) | Docs site, rustdoc, and the docs agent. |
| [ci-and-hardening.md](ci-and-hardening.md) | CI, security workflows, and API compatibility. |
| [cla.md](cla.md) | CLA text and enforcement workflow. |
