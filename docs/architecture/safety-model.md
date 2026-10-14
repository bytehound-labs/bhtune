---
sidebar_position: 4
---

# Safety model

A tune changes a live loop's mode and MV, so BHTune treats a run as a physical operation rather
than as a calculation-only preview. The same core guardrails apply whether a run is started
from the CLI or the browser.

- **Validate before mutation.** External values and the loop's initial readings are checked
  before the loop is switched to Manual. Invalid ranges, non-finite values, or an unusable
  relay amplitude stop preparation before the live loop is changed.
- **Restore changed state.** Cancellation, a run timeout, a driver error, and normal
  completion all reach the restore path. The restore attempts each applicable step and records
  whether restoration was confirmed or incomplete; restore timeouts and a second Ctrl+C are
  reported separately from an ordinary abort.
- **Apply one quality policy to tuning-critical reads.** `Good` readings pass, `Bad` readings
  are rejected, and `Uncertain` readings follow the installation-wide
  `allow_uncertain_quality` policy. A poor-quality PV read can abort a live run.
- **Verify physical MV actuation.** Acceptance by the OPC DA gateway is not proof that the
  controller reached the commanded value. Relay and restore commands are checked against
  bounded readback evidence, and an unconfirmed relay is not replaced by another relay step.
- **Separate authorization from write verification.** PID constants are written only when
  requested. The CLI requires `--yes` with `--write-pid`; the browser presents a review of the
  destination tags and values before confirmation. The shared write path pre-reads existing
  values, verifies each write, rolls back confirmed partial changes when possible, and records
  an audit row.

These controls do different jobs: operator confirmation authorizes a PID-constant write,
readback confirms what the controller accepted, and the run restore returns the loop's
pre-test state as far as the driver and controller allow. A gateway response alone is not
treated as evidence of successful actuation.

## Bounded execution and restoration

The shared runtime validates CLI and HTTP requests before preparation, then validates resolved
driver readings before the first live mutation. A read-only preflight uses the same checks
without entering preparation or starting a run; readability is not evidence of write permission.

Cancellation is observed during idle waits and in-flight driver calls through one persistent
signal handle. Whole-run, per-operation, and restore budgets serve different purposes: a restore
triggered by a run timeout must not inherit an expired run budget. An accepted authoritative
MV restore receives its complete four-second confirmation window even near the initial restore
deadline.

Restore is best-effort, not an atomic controller transaction. Every permitted step is attempted
independently within the remaining budget, and incomplete restoration remains a distinct
outcome. Returning the controller to Auto can precede a later setpoint or mode-attribute failure;
neither a terminal tune outcome nor a successful gateway response proves that every original
value is restored. The [restore procedure](../guides/safety.md#restoration) defines ordering
and operator follow-up.

## Durable ownership and recovery

Every live mutation or recovery combines an OS-level exclusive guard for the canonical database
with a conditional database claim for the controller/MV resource. This protects the loop across
CLI/server processes sharing that database, not unrelated databases or independent deployments;
an in-memory HTTP task registry alone cannot do so. An owner heartbeats
independently every five seconds, including during slow driver calls.

The run records its template, effective tags, submitted request, OPC connection, initial
readings, captured policy, and restore intent before live changes. Mutation steps and actuation
evidence are durable audit records rather than inferred from a later error message.
If follow-up preparation metadata cannot be stored, the owned run is marked failed or removed
if that terminal update also fails; it must not remain permanently running.

Full-mode startup acquires the guard and exports evidence before retiring verified orphan
ownership. It never contacts a controller or restores a loop automatically. A stale heartbeat,
PID, or lock-file presence is only a diagnostic: a paused process that still holds the OS guard
blocks takeover. Complete and consistent provenance is required for explicit recovery;
legacy or incomplete evidence fails closed.

`bhtune restore-loop <run-id> --yes` uses the recorded connection and policy; supplied connection
flags can only cross-check them. It reports every permitted restore step and cannot report success
when restoration or required audit persistence is incomplete. Unresolved recovery records remain
protected from retention and history deletion. See
[interrupted-tune recovery](../guides/safety.md#recovering-an-interrupted-live-tune) and
[persistence](persistence.md).

The [Safety guide](../guides/safety.md) documents cancellation, input validation, OPC quality,
restoration, network exposure, and write-back behavior. The
[MRFT concepts guide](../guides/mrft-concepts.md) explains the live relay test and its
actuation checks.
