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

`vite.config.ts` proxies `/api/*` requests to `http://127.0.0.1:8787`, so run
`cargo run -p bhtune-server` alongside `pnpm dev` to exercise the real HTTP API while
developing. In production, `bhtune-server` serves the built SPA directly from the same origin
(see the `server-embed-spa` phase in `AGENTS.md`) and no proxy is involved.

## Regenerating the API client

Whenever `openapi.json` changes (i.e. whenever `bhtune-server`'s routes or DTOs change):

```sh
pnpm run generate:api
```

This regenerates `src/api/schema.d.ts` from `../openapi.json` and reformats it. CI regenerates
it too and fails the build on drift, so a stale, hand-edited, or forgotten-to-regenerate schema
can never merge.

## Routes

| Path               | Screen                                                                                           |
| ------------------ | ------------------------------------------------------------------------------------------------ |
| `/templates`       | Template list, with delete.                                                                      |
| `/templates/new`   | Create a template (all `DcsTemplate` fields).                                                    |
| `/templates/:name` | Read-only template detail.                                                                       |
| `/runs`            | Filterable, paginated tune-run history list.                                                     |
| `/runs/:id`        | Run detail: configuration, initial readings, calculated results, and the write-back audit trail. |

There is no template edit screen (no update endpoint yet) and no trend chart on the run
detail screen yet (that's the `history-explorer-ui` phase). Connection, tag mapping, test
parameters, results with write-PID, and the simulator screen are not built yet — they need a
way to start a tune over HTTP, which doesn't exist yet either; see `AGENTS.md`'s roadmap.

## Scripts

| Command                 | Purpose                                                                 |
| ----------------------- | ----------------------------------------------------------------------- |
| `pnpm dev`              | Start the Vite dev server with hot module reload.                       |
| `pnpm build`            | Type-check (`tsc -b`) and produce a production build in `dist/`.        |
| `pnpm lint`             | Lint with [oxlint](https://oxc.rs/).                                    |
| `pnpm run format:check` | Check formatting with Prettier (`pnpm exec prettier --write .` to fix). |
| `pnpm run generate:api` | Regenerate `src/api/schema.d.ts` from `../openapi.json`.                |
| `pnpm preview`          | Serve the production build locally, for a final check before deploying. |
