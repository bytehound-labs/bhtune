---
sidebar_position: 9
---

# Troubleshooting

Use this guide to interpret BHTune messages and decide what to check next. Text in angle
brackets, such as `<path>`, stands for a value from your installation. For gateway service,
Windows, and OPC DA diagnostics, see the
[opcda-bridge troubleshooting guide](https://github.com/bytehound-labs/opcda-bridge/blob/main/docs/troubleshooting.md).
See the [glossary](../reference/glossary.md) for control-loop and OPC DA terms.

## Gateway unreachable or incompatible

**Symptom.** An OPC command or live tune cannot connect and reports `failed to connect to
driver`. A missing gateway feature can report:

```text
gateway does not support <operation>; upgrade the OPC DA bridge gateway to a compatible version
```

A protocol incompatibility message has this form:

```text
<gateway-label> is incompatible with bhtune <client-version><details>; upgrade the opcda-bridge gateway or bhtune so their core protocol versions overlap
```

`<details>` contains the reported protocol incompatibility when available.

**Cause.** BHTune cannot reach the configured bridge address, or the gateway and client do not
share a compatible core protocol version. BHTune uses the gateway rather than connecting to an
OPC DA server directly.

**Fix.** Verify the `--bridge-host` value or the resolved `BHTUNE_BRIDGE_HOST`/config setting,
then check that the gateway is running and reachable from the BHTune host. Run
`bhtune opc gateway-info --bridge-host "<gateway-host>:7600"` to inspect its version and
compatibility report. If it is incompatible, upgrade the gateway or BHTune so their core
protocol versions overlap; a live tune, PID write, or revert is refused before changing the
loop. Do not bypass that refusal. See [OPC gateway compatibility](opc-gateway-compatibility.md)
for the compatibility states and their effect on live operations.

## OPC quality failure (exit code 5)

**Symptom.** `bhtune tune` exits with code `5`. For a rejected poll reading, the CLI reports:

```text
Tune aborted: tag '<tag>' reported OPC quality <quality> during polling; loop restored.
```

In this message, `<quality>` is the reported `Bad` or `Uncertain` value.

**Cause.** A tuning-critical OPC reading had `Bad` quality, or had `Uncertain` quality while
the configured quality policy rejected it. This can occur during initial reads, setpoint
capture, or PV polling. `Good` always passes; `Bad` never passes; `Uncertain` follows the
configured policy and is accepted by default.

**Fix.** Review the named tag, its template mapping, and the quality shown by
`bhtune check` or the run history. Check the controller and OPC server before trying again.
Do not treat a bad-quality value as safe to tune from. See the [safety guide](safety.md) for
the quality policy and restore behavior.

## Restore could not be confirmed (exit code 6)

**Symptom.** A tune exits with code `6` and reports:

```text
Tune ended, but the loop's restore could not be confirmed (<reason>). Check the loop by hand -- see the warning above for the tag and value to check.
```

**Cause.** One or more applicable restore steps, the required readback, or the final recovery
audit was not confirmed before the restore deadline or interruption.

**Fix.** Do not start another tune. Check the actual mode, MV, setpoint, and other applicable
restore values at the DCS/PLC, and review the run's per-step evidence. `bhtune restore-loop
"<run-id>" --yes` is only for a failed run that Full-mode startup explicitly marked eligible
for recovery. It requires confirmation, uses the run's recorded connection and policy, and
reports every step. If the run is not eligible or the controller state is uncertain, follow
the site's manual recovery procedure instead of bypassing the guard.

## MV actuation was not confirmed (exit code 7)

**Symptom.** A live tune exits with code `7`. The output identifies an MV tag, requested
target, readback, tolerance, elapsed time, and confirmation deadline, followed by `Loop
restored.` An example of the message shape is:

```text
Tune aborted: MV tag '<tag>' did not confirm target <target> (readback <readback>, tolerance <tolerance>) after <elapsed-seconds>s; the confirmation deadline was 4s. Loop restored.
```

**Cause.** An accepted MV write is not proof that the controller reached the requested value.
The readback was unavailable, outside tolerance, or arrived after the confirmation deadline.

**Fix.** Inspect the run's actuation evidence and the actual MV at the DCS/PLC before doing
anything else. Check the configured MV tag, controller response, and gateway communication.
Do not retry the tune blindly. If restore was also incomplete, exit code `6` takes precedence;
follow the restore guidance above. See [live-plant safety](safety.md).

## `bhtune check` failed (exit code 8)

**Symptom.** `bhtune check` prints `Preflight: fail` and exits with code `8`.

**Cause.** One or more checks failed, or `--strict` treated a warning as a failure. Exit code
`1` means the preflight could not run; code `0` means it passed under the selected strictness.

**Fix.** Read the check details and tag-read results, then correct the inputs, template mapping,
gateway/server selection, or tag quality before starting a tune. `bhtune check --write-pid
<level>` only checks whether PID tags and template prerequisites are readable. It does not
write, does not prove controller write permission, and does not establish that an accepted MV
write will move the process. See the [CLI quickstart](../getting-started/cli-quickstart.md#check-a-live-loop-without-starting-a-tune)
and [safety guide](safety.md#read-only-tune-preflight).

## Database is in use during restore

**Symptom.** A database restore operation reports:

```text
database at <path> appears to be in use by another connection or process -- close it before restoring
```

**Cause.** The restore operation found another SQLite connection holding the live database
open. Replacing a file that another process is using could leave that process with a stale or
inconsistent view.

**Fix.** Close BHTune and any other process that has the database open, then retry the restore
through the application using it. The refusal leaves the live database untouched. Do not
remove its WAL/SHM sidecars to work around the lock; BHTune's database backup and restore
interfaces are library APIs, not CLI commands.

## Existing database has an incompatible migration history

**Symptom.** BHTune reports:

```text
database at <path> has an incompatible migration history; back it up and configure a new database file before using this version
```

**Cause.** The database records migration history that this version does not recognize.

**Fix.** BHTune does not migrate, delete, or recreate the old database in response to this
error. Preserve a verified backup of the old file, then configure a different database file
with `--db`, `BHTUNE_DB`, or the `db` config key before using this version. Keep the original
database intact.

## Automatic refresh is waiting for its first index

**Symptom.** The tag browser says:

```text
Auto-refresh: waiting for the first index; run Refresh Index once
```

**Cause.** Automatic refresh is enabled, but no usable index exists and no build or deletion
is active. The first index build is manual; this message does not mean automatic refresh is
broken.

**Fix.** In the selected server's tag browser, click **Refresh Index** once and let the build
finish. Automatic scheduling applies after a usable index exists. The choice to enable
automatic refresh is separate from starting that first build.

## Indexing failed or was interrupted

**Symptom.** A failed build can show `Index: failed` and
`Index error: <gateway diagnostic>`. If there is no complete index, global search says:

```text
Global search is unavailable because the gateway has no complete index. Lazy browse and direct ItemID entry remain available.
```

An initial build that is still in progress can show `Index: building` and:

```text
Global search will be available when the gateway finishes building the index. Lazy browse and direct ItemID entry remain available.
```

**Cause.** The gateway could not finish the inventory, or a build is still partial. If a
previous complete generation exists, it can remain usable even while the latest index status
is `failed`.

**Fix.** Read the gateway diagnostic and resolve its cause before retrying with **Refresh
Index**. While a build is active, **Cancel Indexing** stops that build; **Disable Auto-refresh**
only stops future scheduling and does not cancel work. Lazy browse and direct ItemID entry do
not require global search.

## Full-mode browser request rejected by Origin/Host checks

**Symptom.** A browser mutation returns HTTP `403` with this JSON error:

```json
{ "error": "cross-origin request rejected" }
```

**Cause.** In Full mode, a state-changing browser request's `Origin` did not match the request
`Host` and effective port, or did not match the explicitly configured origin. A reverse proxy
that rewrites `Host` can cause this mismatch.

**Fix.** Open the UI through one consistent browser-visible origin. If a trusted reverse proxy
rewrites `Host`, configure `BHTUNE_ORIGIN` or the `origin` config key to the exact external
origin and verify the proxy's forwarded request. Demo mode requires its exact configured
origin. This check is CSRF protection, not authentication; Full mode has no authentication.

## Demo request or run limit reached

**Symptom.** Demo requests can return HTTP `429` for a per-client/session limit or HTTP `503`
when shared Demo capacity is exhausted. Both responses include a `Retry-After` header.

**Cause.** Demo applies fixed application limits, including one active run per visitor, eight
active runs globally, six accepted starts per token and client IP in ten minutes, ten accepted
runs per browser session, ten retained terminal runs per visitor, and a 30-second run timeout.
These limits cannot be raised through deployment configuration.

**Fix.** For `429`, wait for the advertised retry interval and reduce repeated starts. For
`503`, wait briefly and try again. Demo is simulator-only and has no OPC, live tag, or PID
write-back access. See the [Demo resource limits](public-simulator-demo.md#resource-limits)
for the complete set of ceilings.
