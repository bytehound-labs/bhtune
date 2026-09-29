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

The [Safety guide](../guides/safety.md) documents cancellation, input validation, OPC quality,
restoration, network exposure, and write-back behavior. The
[MRFT concepts guide](../guides/mrft-concepts.md) explains the live relay test and its
actuation checks.
