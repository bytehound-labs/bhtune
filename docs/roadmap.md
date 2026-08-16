---
sidebar_position: 6
---

# Roadmap

Everything below is planned, in the sense that a design exists and there is nothing blocking
it in principle — but none of it is scheduled to a date, and it ships in whatever order makes
sense as the project progresses. The project is licensed under the
[AGPL-3.0-or-later](https://github.com/bytehound-labs/bhtune/blob/main/LICENSE). The roadmap
below describes planned capabilities without committing to a delivery schedule.

If one of these matters to you sooner than it might otherwise land, say so on the
[issue tracker](https://github.com/bytehound-labs/bhtune/issues) — real usage is what decides
order here, not a fixed sequence.

## Additional protocol backends: OPC UA and Modbus

OPC DA (via [`opcda-bridge`](https://github.com/bytehound-labs/opcda-bridge)) is the primary,
supported driver today, but it isn't the only industrial protocol in the field, and it isn't
architecturally privileged: BHTune talks to the plant entirely through one `Backend` trait
(`read`/`write`/`browse`), and the tuning engine has no idea which protocol is on the other
side of it. Adding `OpcUaBackend` and `ModbusBackend` implementations is additive work at the
`bhtune-backend` layer — it needs no changes to `bhtune-core`'s tuning math or the MRFT state
machine, and no schema changes either, since a loop's tags are just strings regardless of the
protocol that resolves them.

## Step Test

The legacy tool this project's design was informed by supported a simpler, manual alternative
to MRFT called Step Test: instead of relay-switching the loop, it forces a single MV step and
lets you read the process response directly. It's a real, useful method, and it's on the
roadmap — but it's genuinely blocked, not just deprioritized. Step Test observes PV changes by
subscribing to OPC DA value-change notifications rather than polling on a fixed interval, and
the `opcda-bridge` protocol doesn't have a subscription RPC yet (only unary `Read`/`Write`/
`Browse`/`ListServers`). MRFT doesn't need this — it's fine polling a tag on a timer — so this
gap didn't block v1. Landing Step Test means adding a streaming `Subscribe` RPC to the bridge
first.

## Remote and multi-user access

BHTune's web GUI binds `127.0.0.1` by default and has no authentication — safe out of the box,
but it means the shared-host deployment model (one BHTune instance near the OPC DA gateway,
multiple engineers pointing a browser at it) currently relies on trusting the host's own access
control rather than BHTune's. The planned shape:

- Local accounts with `argon2id` password hashing, session cookies for the browser, and
  revocable API tokens for scripting.
- A configurable bind address, with a loud startup warning whenever BHTune is bound off-loopback
  without authentication enabled.
- Optional TLS termination, with a documented reverse-proxy path for sites that already run one.
- Every PID write-back attributed to an authenticated user, extending the write-back audit
  trail (`tune_writes`) that already records every attempted write today.
- Optional OIDC/SSO for sites that already run an identity provider.

## Multi-loop and batch tuning

Today, one `bhtune tune`/`bhtune simulate` invocation (or one web GUI run) tunes exactly one
loop. A batch mode — point BHTune at a list of loops and run them as an unattended campaign,
one after another — is a natural extension once single-loop tuning is solid, and composes
directly with the existing scheduled/scripted CLI usage rather than needing a new execution
model.

## History explorer

Every completed run, its calculated tuning results, and every write-back attempt are
recorded in SQLite as they happen (`tune_runs`, `tune_samples`, `tune_results`, and
`tune_writes`, with a query/filter/pagination layer over all of them). The full explorer —
age-based retention, headless access, and a browsable GUI screen — is done:

- **Age-based retention** — done. Off by default (retain forever): set
  `--retention-days`/`BHTUNE_RETENTION_DAYS`/`retention_days` in `bhtune.toml` (the same
  `CLI > env > TOML > default` precedence as every other setting) to delete runs older than N
  days automatically, on every startup and, for `bhtune-server`, again every 24 hours while it
  keeps running.
- **Headless history commands** — done. `bhtune history list`/`show`/`revert`/`prune`
  (`prune` applies the configured retention policy on demand, with `--dry-run` to preview the
  count first and `--older-than-days` to override the configured policy for one invocation),
  so all of this is reachable without the web GUI.
- **A GUI history screen** — done. A filterable/sortable run list, a PV/MV trend chart per
  run (the same chart component the live view uses), the run's full parameters, calculated
  constants, and write-back audit trail in one place, plus export (CSV or JSON download) and
  delete actions on the run detail screen.

Two things deliberately _aren't_ planned as part of this:

- **Continuous historization.** BHTune only records data while a tune is actually running, by
  design — polling and storing plant data around the clock would make it a small, worse process
  historian, competing with whatever the site already runs for that (PI, Aspen, or the DCS's own
  historian). If continuous historization ever lands, the right shape is probably exporting
  BHTune's run history _into_ the site's real historian, not building a second one.
- **Cross-run comparison and overlay** (charting several past runs of the same loop together,
  to answer "has this valve degraded since last year?") is the most valuable question the
  history explorer could eventually answer, but it's being deliberately deferred rather than
  designed in from day one — it needs no new schema, only new queries, once the single-run
  explorer above exists.

## Contributing to the roadmap

The [DCS/PLC template catalog](/dcs-templates) is the other place this project actively wants
outside contributions — adding a new control system is a data-file pull request, not a Rust
change. See [`CONTRIBUTING.md`](https://github.com/bytehound-labs/bhtune/blob/main/CONTRIBUTING.md)
for both.
