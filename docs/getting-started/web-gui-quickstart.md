---
sidebar_position: 3
---

# Web GUI quickstart

`bhtune-server` serves one REST/SSE API and the built React frontend. A release binary needs no
separate Node process or static-file server. This quickstart uses the in-process simulator,
without an OPC gateway or plant equipment.

## Build and run

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm --filter bhtune-frontend run build
cargo run --release --locked --bin bhtune-server
```

Build the frontend before the release server: `frontend/dist/` is embedded at Rust build time.
Rebuild the server after frontend changes. Without built assets, the API remains available for
the configured mode, while UI requests return `503` with the frontend build command.

Open the reported address, `http://127.0.0.1:8787` by default. The header shows the application
version and HTTP connection status and offers a remembered light/dark theme. A healthy header
indicator does not verify the gateway, controller, or live tags.

`bhtune-server --help` lists its global `--config PATH` option, read-only `healthcheck`, and
Windows service commands `install`, `uninstall`, `start`, `stop`, and `status`. Binding and other
settings use environment variables or TOML:

```sh
bhtune-server --config /path/to/bhtune.toml
```

Full mode has no v1 authentication and defaults to loopback. An off-loopback `BHTUNE_BIND` is an
explicit network-exposure decision. Direct Full-mode browser access uses same-host Origin/Host
checks; pin `BHTUNE_ORIGIN` or TOML `origin` when a proxy rewrites Host or one external origin is
required. This is CSRF protection, not authentication. Read [network safety](../guides/safety.md#network-exposure)
before exposing the service.

## Run a simulator tune

1. Open **New Tune** at `/runs/new` and choose **Simulator**.
2. Review the template, process/controller type, relay settings, and simulator parameters.
   Live-only controls are disabled; templates still determine the PID-unit/precision convention.
3. Select **Start tune**. The run detail shows status and a streaming PV/Commanded MV trend.
4. Wait for completion, then review the calculated results and restore outcome.
5. Open **History** at `/runs`, select the run, and export its recorded samples as CSV or JSON.

The simulator has no PID constant tags, so it does not write constants to a controller.
**Cancel run** uses the shared abort-and-restore path; leaving the page is not cancellation.
CLI-created runs are visible in the browser when both adapters use the same configured database.

See [Starting a tune](../guides/web-ui/starting-a-tune.md) for process defaults, OPC discovery,
arrow/label tree controls, mapping overrides, readiness checks, drafts, and index controls.
[Runs and history](../guides/web-ui/runs-and-history.md) covers filtering, result validity,
precision, PID review/revert, exports, duplication, and guarded deletion.
[Templates and configuration](../guides/web-ui/templates-and-configuration.md) covers catalog
ownership and installation-wide timing/quality/retention. The
[end-to-end tutorial](../guides/end-to-end-tutorial.md) connects these steps to an approved live workflow.

## Restricted Demo mode

Demo is a separate server-enforced, simulator-only boundary, not Full mode with hidden buttons.
It exposes built-in template reads and visitor-owned simulator history/detail/SSE/cancel/export/
delete, but not OPC, preflight, config/template mutation, server drafts/notes, or PID write-back.
Its anonymous cookie separates browser sessions, not user accounts; history begins with the
first accepted run. Fixed simulator bounds, quotas, and timing cannot be widened by config.

Self-host with one replica, a separate database, and the exact configured origin. Non-loopback
browser access requires the matching HTTPS reverse proxy, not the bound HTTP port. See
[Public simulator demo](../guides/public-simulator-demo.md) for the complete contract.

## Frontend development mode

For hot reload with an isolated temporary database and logs:

```sh
pnpm install --frozen-lockfile
just dev
```

`just` is optional; its repository recipe builds the server, starts API/Vite, and removes its
owned temporary state on Ctrl+C. It refuses occupied 8787/5173 ports without stopping existing
listeners. Open `http://localhost:5173`; `/api/*` proxies to the local server. Restart after Rust/API
changes. Other hostnames require Vite's host allowance and the exact `BHTUNE_ORIGIN`. Both
development listeners are unauthenticated and must remain on a trusted network.

## Explore the API directly

Full mode serves Scalar at `/api/docs` and its generated spec at `/api/openapi.json`; Demo omits
both. The browser's generated TypeScript client comes from that same OpenAPI document.
Health is available in either mode:

```sh
curl --fail http://127.0.0.1:8787/api/health
bhtune-server healthcheck
```

`POST /api/runs` accepts tune inputs through the server adapter, not a CLI "server mode".
Global `[tuning]` settings come from server configuration at preparation. API health proves HTTP
liveness only; it does not establish database readiness, write permission, or live-plant safety.

## Next steps

- [CLI quickstart](cli-quickstart.md) for scripting and read-only checks.
- [Web UI visual reference](../guides/web-ui/overview.md) for Full/Demo screenshots.
- [Safety](../guides/safety.md) for live mutation, cancellation, verification, and recovery.
- [Configuration reference](../reference/config.md) and [templates](../dcs-templates.md) for settings.
