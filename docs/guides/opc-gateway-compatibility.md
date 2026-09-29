---
sidebar_position: 7
---

# OPC gateway compatibility

BHTune talks to a DCS or PLC through an opcda-bridge gateway. Before it changes a live loop, it
checks that the gateway's protocol versions overlap with the versions this BHTune build
implements. The check uses the gateway's published compatibility metadata. It does not require a
separate configuration file.

## When the check runs

The check runs before every live mutation:

- `bhtune tune` checks after the OPC DA connection succeeds and before the loop is switched to
  manual.
- `bhtune history revert` checks after connecting to the run's recorded gateway and before
  writing the previous PID constants.
- The web GUI's post-run Write and Revert actions use the same check.

Discovery, server listing, browse, search, and tag reads do not refuse an incompatible gateway.
Those responses include the compatibility result and keep working with whatever features the
gateway can actually serve.

## Results

| Result | Live tune, write, or revert | Inspection |
| --- | --- | --- |
| Full | Proceeds | Uses every supported feature |
| Partial | Proceeds, with a warning | Degrades the features that do not overlap |
| Unknown | Proceeds, with a warning | Reports that the gateway protocol was not verified |
| Incompatible | Refused before any loop change | Still returns a result instead of failing the request |

An incompatible result names the gateway version, the client version, and the protocol ranges
that do not overlap. The fix is to upgrade the opcda-bridge gateway or BHTune so their core
protocol versions overlap. Do not bypass the refusal: an incompatible core can accept a write
that this BHTune build cannot interpret safely.

A refused `bhtune tune` exits with code `1` and does not leave a running run. A refused web GUI
write or revert returns an error and does not insert a PID change row.

## Where to see the result

- `bhtune opc gateway-info` prints the compatibility status. A partial or unknown result also
  prints the warning. `--output json` includes the same report under `compatibility`.
- `bhtune history show` prints the snapshot stored for that run, in both table and JSON output.
- `GET /api/runs/{id}` returns the same snapshot as `gateway_compatibility`.
- `GET /api/opc/capabilities`, browse, search, and read responses include
  `gateway_compatibility` for the gateway used by that request.
- The run detail page shows a Gateway compatibility badge when a snapshot was stored. Simulator
  runs and runs recorded before this check have no badge.

The snapshot records the client version, the gateway version when the gateway reported one, where
the metadata came from, the overall status, and the per-feature ranges. It is part of the run's
history, so a later gateway upgrade does not rewrite what was true when the run started.
