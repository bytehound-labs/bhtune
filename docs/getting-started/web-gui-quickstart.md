# Web GUI quickstart

`bhtune-server` is a single binary that serves both a REST + Server-Sent-Events API and the
built React frontend — no separate Node process, reverse proxy, or Docker container required on
the target host. This walks through running a complete tune from a browser, against the
built-in simulator.

## Build and run

```sh
pnpm install
pnpm --filter bhtune-frontend run build   # embeds frontend/dist/ into the server binary
cargo run --release --bin bhtune-server
```

```text
bhtune-server listening on http://127.0.0.1:8787
```

Open that URL in a browser. If you skip the frontend build, the server still starts and serves
its API — every `/api/*` route works — but any other path returns a clear
`503 the web UI has not been built yet` instead of a blank page, naming the exact command to
run.

`bhtune-server` has no command-line flags of its own (unlike `bhtune`, it doesn't use `clap`
yet). Every setting — including which port it binds — is resolved from a config file or
environment variable, the same way the CLI resolves its own flags:

```sh
BHTUNE_BIND=0.0.0.0:8080 cargo run --release --bin bhtune-server   # bind elsewhere, e.g. for LAN access
```

Binding anywhere other than `127.0.0.1` exposes the server to your network with **no
authentication** — see [Safety](../guides/safety.md#network-exposure) before doing this outside
a trusted, isolated OT network. Authentication is a planned, not yet shipped, feature (see the
roadmap in the main [README](https://github.com/bytehound-labs/bhtune#readme)).

## Frontend development mode

If you're changing the frontend itself, run the Vite dev server alongside a running
`bhtune-server` instead of rebuilding on every change — Vite proxies `/api/*` requests through
to the real server so both stay in sync:

```sh
cargo run --bin bhtune-server &     # first, in one terminal
pnpm --filter bhtune-frontend run dev   # then, in another -- hot-reloads on save
```

## Run a tune from the browser

1. **Templates** (`/templates`) — the four built-in DCS/PLC templates are listed on first
   launch (seeded automatically into the database). Open one to see its full tag-suffix
   mapping, or create a new one from the browser instead of hand-editing a TOML file.
2. **New Run** (`/runs/new`) — one form covering everything `bhtune tune` takes as flags:
   connection (which backend — OPC DA or the simulator), tag mapping, test parameters (process
   type, controller type, relay amplitude, cycles), simulator parameters (gain/time constant/
   dead time/noise, when the simulator backend is selected), and write-back. Submitting POSTs
   to the same `/api/runs` endpoint the CLI's `bhtune-server` mode exposes — there's exactly
   one API, used by both the browser and any script that wants to drive a run over HTTP
   directly.
3. **Run detail** (`/runs/:id`) — while a run is in progress, a live PV/MV trend chart updates
   in real time over Server-Sent Events (`GET /api/runs/:id/stream`), alongside the current
   relay switch count and cycles remaining. A **Cancel** button stops the run early (the same
   Ctrl+C-triggered abort-and-restore path the CLI uses — see
   [Safety](../guides/safety.md#cancellation)). Once complete, the same page shows the
   calculated Aggressive/Moderate/Sluggish PID constants and any write-back audit rows.
4. **History** (`/runs`) — every past run, filterable by loop/outcome/process type, with the
   same detail view available for any completed run — not just the one you just started.

## Explore the API directly

Every route is documented at `/api/docs` (an interactive Scalar UI generated from the same
OpenAPI 3.1 spec the frontend's TypeScript client is generated from — the two can never drift
apart), and the raw spec is at `/api/openapi.json`. A minimal health check:

```sh
curl http://127.0.0.1:8787/api/health
# {"status":"ok"}
```

Starting a tune over HTTP directly (no browser) needs the same fields the CLI's `tune`/
`simulate` commands take — see `/api/docs` for the full request schema, including the extra
range/direction fields the simulator backend requires that a real OPC DA backend would instead
read live from the DCS.

## Next steps

- [CLI quickstart](cli-quickstart.md) — the same tuning engine, scriptable for scheduled/
  unattended runs.
- [Safety](../guides/safety.md) — cancellation, quality enforcement, and write-back rollback,
  all shared between the CLI and the server.
- [DCS/PLC templates](../dcs-templates.md) — the tag-mapping system behind the Templates screen.
