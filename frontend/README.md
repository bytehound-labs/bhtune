# BHTune web GUI frontend

The browser UI for [`bhtune-server`](../crates/bhtune-server), built with React, TypeScript,
Vite, Tailwind CSS, and TanStack Query. See the repository root [`README.md`](../README.md)
and [`AGENTS.md`](../AGENTS.md) for the overall project — this file only covers this package.

There is exactly one transport: plain `fetch`, wrapped by
[`openapi-fetch`](https://openapi-ts.dev/openapi-fetch/) and typed against `src/api/schema.d.ts`,
which is generated from the repo root's checked-in [`openapi.json`](../openapi.json) — never
hand-edit that file. No client-side transport abstraction is used or planned; see "Web app
architecture" in `AGENTS.md` for why.

## Development

```sh
pnpm install    # from the repo root — this is a pnpm workspace
cd frontend
pnpm dev        # Vite dev server on http://localhost:5173, hot-reloading
```

The workspace uses the latest TypeScript 6 release supported by the OpenAPI generator. Keep
the `typescript` pins in `frontend/package.json` and `website/package.json` on the same
compatible major until `openapi-typescript` supports TypeScript 7's compiler API.

`vite.config.ts` proxies `/api/*` requests to `http://127.0.0.1:8787`, so run
`cargo run -p bhtune-server` alongside `pnpm dev` to exercise the real HTTP API while
developing. In production, `bhtune-server` embeds the built SPA (`rust-embed`, see
`crates/bhtune-server/src/spa.rs`) and serves it directly from the same origin — build with
`pnpm run build`, then `cargo run -p bhtune-server` serves both the API and the UI from one
process with no proxy involved.

The header includes a light/dark theme toggle whose selection is remembered by the browser. Its
colored status dot and server version label are vertically centered together. The dot polls
`GET /api/health` every five seconds. Green means the BHTune HTTP service is reachable; it does
not verify OPC DA or another process-driver connection. Hover the dot for the full status detail.

## Regenerating the API client

Whenever `openapi.json` changes (i.e. whenever `bhtune-server`'s routes or DTOs change):

```sh
pnpm run generate:api
```

This regenerates `src/api/schema.d.ts` from `../openapi.json` and reformats it. CI regenerates
it too and fails the build on drift, so a stale, hand-edited, or forgotten-to-regenerate schema
can never merge.

## Routes

| Path                    | Screen                                                                                                                                                                     |
| ----------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `/templates`            | Template list, with delete.                                                                                                                                                |
| `/templates/new`        | Create a template (all `DcsTemplate` fields).                                                                                                                              |
| `/templates/:name`      | Read-only template detail, with an Edit link for user-owned templates.                                                                                                     |
| `/templates/:name/edit` | Edit a user-owned template (all fields except Name, which is immutable once created).                                                                                      |
| `/runs`                 | Filterable, paginated tune-run history list.                                                                                                                               |
| `/runs/new`             | Start a tune: connection, tag mapping, test parameters, simulator parameters, and write-back, all in one form.                                                             |
| `/runs/:id`             | Run detail: configuration, initial readings, calculated results, write-back audit trail, and a PV/MV trend chart with initial-reading and terminal restored-MV boundaries. |

For the OPC DA driver, the connection fields are presented in this order: Bridge host, OPC DA
server ProgID, Tag name, then Notes.

Built-in and catalog templates can't be edited through the UI — they're re-seeded from
their source file on every server startup, so an edit would just be discarded — but they
can still be viewed, and deleting one to make room for a customized replacement works the
same as for any other template. The run detail screen's trend chart streams live via
Server-Sent Events (`GET /api/runs/:id/stream`) while a run is active. The stream sends the
initial PV/MV snapshot as soon as the server records it, before the first MRFT sample, then
replays every sample recorded so far and switches seamlessly to the completed run's stored
samples once it finishes. The chart starts with the run's initial PV/MV readings and ends
with a terminal point at the original MV after restoration; short trends reserve 12 configured
poll intervals on the x-axis and leave unused future space blank, then fit the full elapsed
run once that horizon is reached. The same `TrendChart` component renders live and historical
cases identically without fabricating samples.

## Scripts

| Command                 | Purpose                                                                 |
| ----------------------- | ----------------------------------------------------------------------- |
| `pnpm dev`              | Start the Vite dev server with hot module reload.                       |
| `pnpm build`            | Type-check (`tsc -b`) and produce a production build in `dist/`.        |
| `pnpm lint`             | Lint with [oxlint](https://oxc.rs/).                                    |
| `pnpm run format:check` | Check formatting with Prettier (`pnpm exec prettier --write .` to fix). |
| `pnpm run generate:api` | Regenerate `src/api/schema.d.ts` from `../openapi.json`.                |
| `pnpm preview`          | Serve the production build locally, for a final check before deploying. |
