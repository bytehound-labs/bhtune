# Public simulator Demo mode

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Demo design decisions

- **`server-demo-mode` is done: public exposure uses a separate, server-enforced route surface.**
  `ServerMode::Demo` is an explicit runtime mode in the same binary; `Full` remains the default
  and retains the live-plant API. Demo mounts health, capabilities, built-in read-only
  templates, and visitor-owned simulator run/history operations only. Anonymous
  `__Host-bhtune_demo_session` cookies are opaque isolation/quota tokens: only their SHA-256
  hashes are stored, ownership is attached during the initial `tune_runs` insert, and every
  Demo list/detail/stream/cancel/export/delete query is owner-scoped. `/api/capabilities`
  describes the mode, fixed policy, simulator bounds/defaults, restrictions, quotas, and
  security metadata; the frontend fails closed if Demo metadata is missing or unsafe.
- **Demo mode is not an authentication boundary; it is a reduced public capability boundary.**
  A Demo deployment is safe to expose only because the server mounts no OPC, PID write/revert,
  Config mutation, template mutation, notes, drafts, OpenAPI, or Scalar routes. Its anonymous
  cookie identifies an isolated visitor namespace and carries quotas, but is not an account and
  does not prove a person's identity. Fixed application-owned limits cannot be widened by
  deployment configuration, in-memory coordination supports one application replica, and a
  separate Demo database is required; Caddy, CrowdSec, and network controls remain necessary
  perimeter defenses rather than being replaced by the application.

## Public simulator Demo mode (`server-demo-mode`)

`bhtune-server` has two runtime exposure modes. `full` is the default and preserves the
trusted/operator API, including OPC DA access, mutable templates/configuration, notes, drafts,
and PID write/revert operations. `demo` is a server-enforced public surface for anonymous
simulator demonstrations; it is not a second binary and it is not implemented by hiding Full
mode controls in React.

`BHTUNE_SERVER_MODE` overrides the optional `server_mode` TOML key and accepts only `full` or
`demo`. Full mode automatically matches browser `Origin` to request `Host` when
`BHTUNE_ORIGIN` and the `origin` TOML key are both absent; an explicit value remains a strict
origin pin. Demo mode requires one exact configured browser origin from `BHTUNE_ORIGIN` or the
`origin` TOML key. HTTPS is required except for explicit loopback HTTP origins used by tests and
local development. `trusted_proxy` may name one exact IP address or matching-family CIDR; the
server accepts the single `X-BHTune-Client-IP` value only from that peer. The deployment proxy
overwrites the header, and the application falls back to the Axum peer address otherwise.

The `[demo]` table is declarative rather than a tuning surface. Missing values resolve to the
application-owned constants below; any present value must match exactly, so a deployment cannot
weaken the public contract:

| Control                                |           Fixed value |
| -------------------------------------- | --------------------: |
| Anonymous session lifetime             |        86,400 seconds |
| Simulator poll interval                |                200 ms |
| Whole-run timeout                      |            30 seconds |
| Active runs globally / per visitor     |                 8 / 1 |
| Accepted starts per token / client IP  | 6 / 6 per 600 seconds |
| Retained terminal runs per visitor     |                    10 |
| Current Demo-owned run rows globally   |                 5,000 |
| JSON request body                      |                32 KiB |
| SSE streams per visitor / globally     |                2 / 32 |
| SSE absolute lifetime                  |            45 seconds |
| Ordinary request concurrency / timeout |       64 / 10 seconds |
| Cleanup interval                       |           300 seconds |

Demo accepts only bounded simulator requests. The curated defaults are Yokogawa CentumVP,
Flow/PI, reverse action, relay amplitude 10%, cycles skip/count `1/2`, zero noise protection,
simulator gain/time constant/dead time/noise/seed `1.0/0.5/1.0/0/0`, PV/MV ranges `0–100`,
and initial PV/MV `50`. Explicit values are bounded before the owned `prepare()` path: relay
amplitude `1–20%`, skipped cycles `0–2`, counted cycles `1–3`, noise protection `0–3` seconds,
positive gain `0.1–5.0`, time constant `0.05–5` seconds, dead time `0–2`
seconds, range endpoints `-1,000–1,000` with spans `1–1,000`, non-negative noise up to 5% of
the PV span, and initial values inside their ranges. Demo uses Reverse controller action by
default, while Demo process gain remains constrained to positive values; the normal
process/controller compatibility rules still apply.
OPC server/bridge values, tag overrides, notes, `write_pid`, and write confirmation are
rejected rather than ignored. The persisted/display identity is the fixed label
`Simulator demo`; simulator internals remain `Sim.PV` and `Sim.MV`.

The Demo route tree contains only:

- `GET /api/health` and `GET /api/capabilities`
- read-only built-in template list/detail
- visitor-scoped run start/list/last-request/detail/stream/cancel/export/delete
- the embedded SPA fallback

Config, template mutation, server-backed drafts, notes, PID write/revert, OPC discovery/
browse/read, OpenAPI, and Scalar are not mounted in Demo mode. The route boundary is therefore
the security control; a client cannot reach an omitted capability by constructing an HTTP
request manually. Demo list pagination is capped at 10, another visitor's numeric run ID
returns the same `404` as an unknown ID, and every owner-scoped query applies the session
condition in the database query itself.

Anonymous identity is an opaque isolation token, not an account. The server issues a 32-byte
cryptographically random lowercase-hex value in the host-only
`__Host-bhtune_demo_session` cookie with `Secure`, `HttpOnly`, `SameSite=Strict`, `Path=/`,
`Max-Age=86400`, and no `Domain`. Only its SHA-256 hash is stored. Capability requests may
issue the cookie without creating a row; a session row is created lazily only after an
accepted, quota-checked run start. A shared browser profile shares its history and quotas;
clearing cookies creates a new anonymous namespace. Sessions expire after the fixed lifetime
and do not slide.

The consolidated pre-v0.1 schema includes `demo_sessions`, nullable
`tune_runs.demo_session_id` ownership with cascading deletion, owner indexes, and database
triggers that require an active session, require the simulator driver, make ownership
immutable, and enforce the global current-row cap. Full-mode and pre-Demo rows retain
`NULL` ownership. Startup recovery terminalizes owned rows still marked `running`; cleanup
runs immediately and every five minutes, removing expired sessions only when they have no live
run and pruning excess terminal history. The owned `prepare()` path records ownership in the
initial insert; if follow-up provenance/effective-metadata persistence fails, it terminalizes
the row or deletes it as a fallback rather than leaking a permanent `running` row.

Admission uses non-queueing RAII permits: one global and one persistent per-visitor active-run
permit are acquired before preparation, and accepted-start windows are checked per token and
normalized client-IP before session creation or database writes. SSE has separate per-visitor
and global permits plus a 45-second deadline. Ordinary Demo requests have their own
concurrency and timeout layers. These controls are deliberately in-memory, so one application
replica is supported; multiple replicas would require shared coordination before they could
serve the same public namespace safely. Caddy/CrowdSec and network-level volumetric DDoS
protection remain required perimeter controls.

All Demo API responses are private (`Cache-Control: no-store`, `Vary: Cookie`, and no-index
headers). State-changing browser requests require the exact configured `Origin`, CORS remains
disabled, and responses receive CSP, framing, content-type, referrer, cross-origin, and
Permissions Policy headers. The self-hosted deployment uses a dedicated database and runtime
directories, a non-root single container with a read-only root filesystem, dropped
capabilities, `no-new-privileges`, bounded CPU/memory/PIDs, a private Docker network, and an
app1 firewall rule that admits the Caddy host only. GitHub Actions publishes a full-commit
image tag; Woodpecker resolves and verifies its immutable digest before invoking the separate
`FrontEnd` deployment wrapper. The public Caddy route remains a deliberate deployment
activation step, not an application default.

## Demo implementation status

The public simulator Demo extension is complete across `bhtune-db`, `bhtune-cli`, `bhtune-server`,
and `frontend/`; the separate FrontEnd deployment/Caddy assets are also authored. Public activation
remains pending private deployment and rollback rehearsal, so DNS, firewall exposure, Caddy routing,
Cloudflare Tunnel, authentication, CAPTCHA, and multi-replica coordination remain disabled and out
of scope.

| Demo surface        | Status                                                                                                                                                                                                               |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `bhtune-db`         | Complete: anonymous session storage, nullable run ownership, owner-scoped queries, cleanup, recovery, and compatibility migration are implemented.                                                                   |
| `bhtune-cli`        | Complete: shared owned preparation and deterministic simulator-only orchestration support the server without changing the Full-mode CLI contract.                                                                    |
| `bhtune-server`     | Complete: validated Full/Demo runtime modes, restricted Demo routes, quotas, security checks, recovery, cleanup, and capabilities contract are implemented. Private deployment and public activation remain pending. |
| `frontend/`         | Complete: capability-aware simulator-only UI, private history, live streaming, bounded requests, and Demo browser coverage are implemented.                                                                          |
| FrontEnd deployment | Authored separately under `/home/mike/git/FrontEnd`; Compose, Caddy, Woodpecker, rollout, rollback, and validation assets await private rehearsal.                                                                   |
