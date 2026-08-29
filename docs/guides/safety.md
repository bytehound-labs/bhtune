# Safety

BHTune's MRFT test switches a real loop to manual and strokes its MV — the same as the legacy
tool, but with one important difference: the legacy tool assumed an operator was always
watching and could hit Stop. BHTune adds real guardrails for scheduled and scripted tunes that
run with nobody present. This page explains exactly what those guardrails do, since "what
happens if I press Ctrl+C" and "what happens if the network hiccups mid-test" both matter before
you point BHTune at a real process.

## Cancellation

Pressing Ctrl+C during a `bhtune tune`/`bhtune simulate` run (or clicking **Cancel** on the web
GUI's run detail screen, which triggers the same code path) is always safe:

- **First Ctrl+C** stops polling immediately and starts the restore (see
  [Restoration](#restoration) below). It works no matter when it's pressed — including mid-read
  or mid-write to a stalled driver, not just while idle between poll ticks. Every in-flight
  driver call is bounded by `--op-timeout-secs` (default 30s), so a stalled OPC DA read or
  write is abandoned rather than waited on forever, which is what makes cancellation reliable
  even against a wedged gateway or a black-holed network.
- **Second Ctrl+C**, pressed while the restore itself is still running, forces an immediate hard
  stop instead of waiting any longer for the restore to finish. BHTune prints exactly which MV
  tag it was restoring and what value it was last written to, so you can put it back by hand.
  This exits with a distinct code (`6`, see the exit code table below) rather than the normal
  abort code, because "aborted and restored" and "aborted, restore abandoned — go check the
  loop" are different situations for whoever (or whatever scheduler) is watching the exit code.
- **`--timeout-secs`** (default 3600) is the overall wall-clock budget for the whole test. It
  fires the same restore path as Ctrl+C — including working correctly mid-hung-read — and is
  meant as the backstop for scheduled/unattended runs where nobody is present to press Ctrl+C at
  all.
- **`--restore-timeout-secs`** (default 30) bounds the restore step itself, independent of
  `--timeout-secs` — a restore triggered by a timeout doesn't inherit an already-expired budget.

Try it yourself: start a run with a long poll interval and press Ctrl+C while it's waiting
between ticks (works immediately); then point it at an unreachable `--bridge-host` and press
Ctrl+C — it should abort and report within `--op-timeout-secs`, not hang.

## Timing and host responsiveness

Live OPC DA runs measure MRFT time with a monotonic clock paired to the run's UTC start
timestamp. The timestamps stored in history remain ordinary UTC values, but their progression
comes from monotonic elapsed time. An NTP correction or manual system-clock adjustment after the
run starts therefore cannot shorten, lengthen, reverse, or skip an apparent relay period.

Real delays are not hidden. If the operating system schedules BHTune late, the gateway responds
slowly, or an OPC read/write takes longer than expected, that elapsed time remains part of the
sample and switch timeline. The polling loop delays its next schedule instead of issuing a burst
of catch-up reads or writes against a live controller.

BHTune is not a hard-real-time controller and cannot guarantee identical live samples on an
overloaded host. Keep the BHTune host and OPC DA gateway responsive, avoid competing heavy work
during a tune, and choose a poll interval comfortably shorter than the loop's expected oscillation
period. The whole-run and per-operation safety timeouts remain independent monotonic timers.

Each run with at least one successful PV poll stores a timing snapshot in history. The CLI's
`bhtune history show <run>` output and the web run-detail page show the requested interval,
observed sample-gap count, mean and maximum sample gap, measured oscillation period when the test
completed, and approximate samples per period. A live run is flagged when an adjacent sample gap
is at least twice the requested interval, because that objectively means at least one complete
polling opportunity was missed. This is a warning, not a validity verdict: it does not abort the
run, change its calculated constants, or prevent an engineer from applying them.

## Input validation

Every number that reaches the tuning engine is validated before any live I/O happens: relay
amplitude, cycle counts (a zero cycle count is rejected outright rather than reaching the engine
and panicking mid-test, which is what the earliest builds did), PV/MV ranges (must be finite and
correctly ordered — a `NaN` or an inverted range is rejected, not silently propagated into a PID
write), and the initial MV must fall inside the validated MV range. Command-line flags reject
non-finite/out-of-range input immediately with a clear message; anything read from the driver
(a real DCS/PLC's current ranges, for instance) is validated again right after being read, before
the loop is ever switched to manual. An effective relay step below the minimum that can be
distinguished safely at `f32` precision is rejected at this same pre-mutation boundary.

## MV actuation verification

Every accepted OPC DA relay write is read back before a later relay command can replace it. The
first check occurs at the earlier of the MRFT noise-protection boundary and four seconds after
write acceptance. An early mismatch remains pending and is retried; a mismatch at four seconds,
or when the engine genuinely needs the next relay command, aborts the run without writing the
replacement.

The absolute tolerance combines the `f32` precision floor with 0.1% of the configured MV span.
For relay commands it is capped at 25% of the actual step, preventing a wide range from making a
small command appear confirmed accidentally. Restore confirmation uses the same precision/span
tolerance without the relay cap. The final MRFT snapback hands responsibility to the
authoritative restore write, so BHTune does not wait twice for the same original-MV target.
The restore readback is attempted immediately; only a mismatch is retried, and a
`--restore-timeout-secs` value below four seconds is rejected before a live loop is mutated.
An MV read that starts before the deadline but returns after it is still treated as late and does
not confirm the command. The fresh read started at the deadline is independently capped at one
second, rather than inheriting the full per-operation timeout, so a stalled read cannot hold the
tune open indefinitely.

Verification reads are separate from PV samples: they do not advance MRFT time, add trend/export
samples, or increment polling timing statistics. `bhtune history show <run>` records each
accepted command, observation, tolerance, deadline, and final status.

## OPC quality

Every OPC DA read reports a quality alongside its value (`Good`/`Uncertain`/`Bad`). BHTune
always accepts `Good`, accepts `Uncertain` by default, and never accepts `Bad` for tuning-critical
operations:

- **Before the loop is touched** (initial PV/MV/range/mode/direction reads): any non-`Good`
  reading is a hard failure. Nothing has been mutated yet, so this is a clean refusal.
- **During the test** (every polled PV sample): a non-`Good` sample aborts the run and restores
  the loop. A held or stale PV during a relay half-cycle would corrupt the exact period
  measurement the test depends on — silently tolerating it is worse than aborting loudly. The
  triggering sample is still recorded (with its real quality) before the abort.
- **During MV actuation verification**: an unacceptable readback is recorded but does not
  immediately prove failure. Confirmation remains pending and is retried until the four-second
  deadline; if the engine needs the next relay command first, the run aborts without issuing that
  replacement.
- **During write-back confirmation**: a non-`Good` readback is treated as an unconfirmed write,
  which triggers rollback (see [PID write-back](#pid-write-back) below).
- **When selecting a tag in the web browser**: BHTune re-reads the exact item selected in the
  tree before applying the template's PV suffix. `Good` quality proceeds normally; `Uncertain`
  or `Bad` quality requires an explicit choice to select another tag or proceed anyway. This
  only accepts the item into the form; tune execution still enforces the quality rules above.

`Bad` quality is never accepted under any setting. Sites whose gateway reports `Uncertain` as a
matter of course can leave the default `allow_uncertain_quality = true`, or disable that global
policy on the Config page / in `bhtune.toml` when uncertain readings must be rejected —
enabled by default, logged loudly every time it changes the outcome, and recorded on the run so
history shows which runs executed under relaxed rules.

## Restoration

BHTune guarantees a best-effort restore on **every** exit path — successful completion, an
error partway through, Ctrl+C, or a timeout — not just the happy path. Each mutation (mode
switched to manual, setpoint captured, MV stroked, mode-attribute written, where applicable) is
recorded the instant it actually succeeds, and the restore step always attempts to undo exactly
what was recorded — nothing more, nothing that was never touched.

The restore itself attempts every step independently rather than stopping at the first failure,
so a rejected MV write doesn't also prevent the mode from being put back. `bhtune history show
<run-id>` (or the run detail screen) reports the restore outcome as one of two states:
**confirmed**, or **incomplete** — naming exactly which step(s) failed so you know what to check
by hand. An incomplete restore exits with code `6`, distinct from a normal abort.

## PID write-back

Requesting `--write-pid <level>` (or the Automatic PID settings section of the New tune form) is the only
part of a tune that writes tuning constants rather than just testing the loop, and it's the only
part that requires `--yes` — an explicit, deliberate confirmation that no human needs to
approve it interactively. BHTune:

1. **Reads and persists the current P/I/D values first**, before writing anything. If this
   pre-read fails, nothing is written at all.
2. **Writes and verifies each constant individually** (P, then I, then D), checking the
   readback against what was requested within a small tolerance — a DCS's own unit rounding
   means a just-written value isn't always bit-identical on readback, so exact equality would
   produce false failures.
3. **Rolls back only what was actually confirmed** if any constant fails partway through — if P
   succeeds and I fails, only P is rolled back (D, never attempted, needs nothing; I, never
   confirmed, has nothing to put back).
4. **Records every outcome**, including which case applies: nothing written, everything written
   and confirmed, a partial write successfully rolled back, or — the case that needs a human —
   a partial write whose rollback itself failed. That last case prints a message pointing at
   `bhtune history revert <run-id>`, which writes the persisted previous values back under the
   same pre-read/verify contract, so a write-back that turns out wrong can be undone later
   without anyone having written the old numbers down by hand.

## Network exposure

`bhtune-server` binds `127.0.0.1` (localhost only) by default and ships with **no
authentication** — anyone who can reach the port can start, cancel, or configure a tune.
Binding to any other address (`BHTUNE_BIND=0.0.0.0:8787` or a LAN IP) is an explicit choice you
make yourself; there is no installer-driven firewall rule or prompt that does this for you.
Until authentication ships (a planned, not yet available, feature), treat a non-loopback bind
the same way you'd treat any other unauthenticated service on your OT network: only do it on a
trusted, isolated network, and prefer console/remote-desktop access to the host running
`bhtune-server` over exposing it further.

The frontend development server is also unauthenticated. It binds all local interfaces so a
trusted host can use `http://asus:5173`, and proxies browser API requests to the local
`bhtune-server`; use this development-only path only on the same trusted network.

## Scripting and exit codes

`--output json` emits exactly one parseable JSON value on stdout, on every path — success,
abort, timeout, poor quality, restore-incomplete, or write-back failure — so a scheduler never
has to guard against stray prose interleaved with the object it's trying to parse. Exit codes
are equally specific:

| Code | Meaning                                                                                  |
| ---- | ---------------------------------------------------------------------------------------- |
| `0`  | Completed successfully                                                                   |
| `1`  | Setup error (unknown template, bad flag combination, database/driver connection failure) |
| `2`  | Aborted by Ctrl+C, restore confirmed                                                     |
| `3`  | Test completed, but the requested PID write-back failed                                  |
| `4`  | `--timeout-secs` elapsed before the test finished                                        |
| `5`  | A non-`Good` OPC sample aborted the run                                                  |
| `6`  | The post-run restore could not be confirmed — check the loop by hand                     |
| `7`  | An accepted OPC DA MV command could not be confirmed; restore was confirmed              |

Exit code `6` takes precedence over `7` if the actuation failure is followed by an incomplete
restore.

## Next steps

- [MRFT concepts](mrft-concepts.md) — what the test is actually doing while these guardrails
  watch over it.
- [CLI quickstart](../getting-started/cli-quickstart.md) — see `--output json` and the automation
  flags in context.
