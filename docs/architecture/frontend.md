---
sidebar_position: 7
---

# Browser frontend

The browser interface is a React and TypeScript single-page application in `frontend/`. It
uses the generated TypeScript client for `bhtune-server`'s HTTP API. The browser talks to the
server rather than connecting to SQLite or the OPC DA gateway directly.

In release builds, the built SPA is embedded in the `bhtune-server` executable. The server
serves the page and API from one process, so a released server does not need Node.js or a
separate static-file server on the target host. Debug builds read the built `frontend/dist/`
assets from disk.

The Vite development server is a separate development tool. Its `/api` proxy forwards requests
to a locally running `bhtune-server` while Vite provides frontend hot reload; that proxy is not
part of the release serving path.

The Docusaurus site in `website/` is a separate static documentation site. It reads the
repository's `docs/` directory and is not embedded in `bhtune-server`.
