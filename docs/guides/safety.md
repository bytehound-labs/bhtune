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
  driver call is bounded by the global `[tuning].op_timeout_secs` setting (default 30s), so a
  stalled OPC DA read or write is abandoned rather than waited on forever, which is what makes
  cancellation reliable even against a wedged gateway or a black-holed network.
- **Second Ctrl+C**, pressed while the restore itself is still running, forces an immediate hard
  stop instead of waiting any longer for the restore to finish. BHTune prints exactly which MV
  tag it was restoring and what value it was last written to, so you can put it back by hand.
  This exits with a distinct code (`6`, see the exit code table below) rather than the normal
  abort code, because "aborted and restored" and "aborted, restore abandoned — go check the
  loop" are different situations for whoever (or whatever scheduler) is watching the exit code.
- **`[tuning].timeout_secs`** (default 3600) is the overall wall-clock budget for the whole test.
  It fires the same restore path as Ctrl+C — including working correctly mid-hung-read — and is
  meant as the backstop for scheduled/unattended runs where nobody is present to press Ctrl+C at
  all.
- **`[tuning].restore_timeout_secs`** (default 30) is the initial budget for the restore step,
  independent of `[tuning].timeout_secs` — a restore triggered by a timeout doesn't inherit an
  already-expired budget. If the authoritative MV restore write is accepted near that deadline,
  BHTune extends the effective deadline as needed to preserve the complete four-second MV
  confirmation window; the remaining restore steps use that same effective deadline.

These five values are global installation settings in the browser's **Configuration** page or
the `[tuning]` section of `bhtune.toml`; they apply to future runs only. Try it yourself: set a
long `[tuning].poll_interval_ms`, start a run, and press Ctrl+C while it's waiting between ticks
(works immediately); then point it at an unreachable `--bridge-host` and press Ctrl+C — it
should abort and report within `[tuning].op_timeout_secs`, not hang.

## Global tune timing settings

The `[tuning]` section controls the operational timing and safety limits shared by all new tunes:

| Setting                |  Default | Purpose                                                                       |
| ---------------------- | -------: | ----------------------------------------------------------------------------- |
| `mrft_delay_secs`      |    `0` s | Pre/post-test recording padding                                               |
| `poll_interval_ms`     | `800` ms | Delay between driver polls                                                    |
| `timeout_secs`         | `3600` s | Whole-run wall-clock limit                                                    |
| `op_timeout_secs`      |   `30` s | Limit for one driver read or write                                            |
| `restore_timeout_secs` |   `30` s | Initial restore budget; an accepted MV restore may extend it for confirmation |

Values must be valid before a tune can touch the loop. `mrft_delay_secs` accepts `0` through
`3600`; the other settings must be positive whole numbers. OPC DA preparation additionally
requires `restore_timeout_secs` of at least four seconds because accepted MV commands have a
four-second actuation-confirmation window. Simulator runs may use a shorter positive restore
timeout. Configuration changes affect future tune preparations only; an already-prepared or
running tune keeps its captured values.

### Public Demo mode

Demo mode is a separate, simulator-only server surface. Its 200 ms poll interval, 30-second
run timeout, request limits, quotas, session lifetime, and history caps are fixed
application-owned safeguards. The optional `[demo]` configuration table can declare those
values for deployment validation, but it cannot widen or otherwise override them. Demo runs
use the stable **Simulator demo** identity and never connect to OPC DA or write PID constants.
See the [public simulator demo guide](public-simulator-demo.md) for the complete boundary and
self-hosting requirements.

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
`bhtune history show <run>` output, the run-detail API, and structured logs retain the requested
interval, observed sample-gap count, mean and maximum sample gap, measured oscillation period
when the test completed, and approximate samples per period. The normal web run-detail page
intentionally omits these low-level diagnostics so it can focus on actionable run and safety
information. A live run is flagged in structured logs when an adjacent sample gap is at least
twice the requested interval, because that objectively means at least one complete polling
opportunity was missed. This is a warning, not a validity verdict: it does not abort the run,
change its calculated constants, or prevent an engineer from applying them.

Each run also reports sampling adequacy in the collapsed **Sampling diagnostics** section on the
web run-detail page. `adequate` means at least six observed samples per measured oscillation
period; `marginal` means fewer than six; and `not_assessed` means no usable finite period was
available. This is an advisory signal, not an automatic rejection: a valid result with marginal
sampling remains writable, but should be reviewed against the trend and the recorded timing data
before it is applied.

The timing snapshot includes successful PV-read, MV-write, MV-verification-read,
sample-persistence, and total-tick-work latency summaries in addition to sample gaps. When a
pending relay is checked by the normal batched PV/MV poll, the same underlying OPC operation may
contribute to both the PV-read and MV-verification categories; those categories describe
overlapping evidence and must not be added together as independent network durations. Failed,
cancelled, and timed-out operations are excluded from these successful-latency measurements.

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

## Invalid calculated results

Every response-level result is checked before it is stored as usable tuning data. A non-positive
or non-finite PV amplitude or period, or any non-finite intermediate or template-converted PID
value, is stored as `Invalid` with an explicit diagnostic reason and without numeric tuning
values. Invalid calculated-result rows cannot be selected for PID write-back in the CLI or web
GUI. This backstop remains important even when sampling is adequate, because a degenerate
measurement can arise from a future algorithm or data-path defect.

## MV actuation verification

Every accepted OPC DA relay write is read back before a later relay command can replace it. While
that command is pending, the normal poll requests PV and MV in one deduplicated batch and evaluates
the MV observation before the PV sample is persisted or the MRFT engine advances. This makes the
normal poll the primary verification path without weakening the safety rule. If normal polling
does not provide usable evidence before the deadline, a separately bounded MV-only read remains
available as a fallback.

The first check occurs at the earlier of the MRFT noise-protection boundary and four seconds
after write acceptance. An early mismatch remains pending and is retried; a mismatch at four
seconds, or when the engine genuinely needs the next relay command, aborts the run without
writing the replacement.

The absolute tolerance combines the `f32` precision floor with 0.1% of the configured MV span.
For relay commands it is capped at 25% of the actual step, preventing a wide range from making a
small command appear confirmed accidentally. Restore confirmation uses the same precision/span
tolerance without the relay cap. The final MRFT snapback hands responsibility to the
authoritative restore write, so BHTune does not wait twice for the same original-MV target.
The restore readback is attempted immediately; only a mismatch is retried.
A `restore_timeout_secs` value below four seconds is rejected during OPC DA preparation, before
a live loop is mutated. This is the initial budget: when an authoritative MV restore write is
accepted close to its end, the effective deadline extends to at least four seconds after that
acceptance so the required confirmation window cannot be cut short. Remaining mode and setpoint
restore steps share the extended deadline.
An MV read that starts before the deadline but returns after it is still treated as late and does
not confirm the command. The fresh read started at the deadline is independently capped at one
second, rather than inheriting the full per-operation timeout, so a stalled read cannot hold the
tune open indefinitely.

The MV observation is evidence about physical actuation, not a second trend point: it does not
advance MRFT time or add a separate trend/export sample. A batched PV/MV request does, however,
record the successful operation in both the PV-read and MV-verification latency categories;
those categories overlap because they describe one request. `bhtune history show <run>` records
each accepted command, observation, tolerance, deadline, and final status.

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

Preparation records the run before writing its effective timing, quality policy, connection,
and other provenance metadata. If one of those follow-up writes fails, BHTune immediately
marks the row **failed** instead of leaving it permanently **running**. If the terminal update
itself cannot be persisted, the row is removed as a fallback and the failure is logged.

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
4. **Writes D explicitly for PI controllers.** A PI result writes its calculated integral value
   and `D = 0.0`, clearing any stale derivative action in the controller. A P-only result uses
   the template-specific integral-disabling sentinel; a full PID result writes its calculated
   derivative value.
5. **Records every outcome**, including which case applies: nothing written, everything written
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
Full mode provides CSRF protection for browser mutations without requiring per-host setup:
when no `BHTUNE_ORIGIN` or `origin` config value is present, the server accepts an `Origin` only
when its host and effective port match the request `Host`. Set an explicit origin when a reverse
proxy rewrites `Host` or when one external browser origin must be pinned. Requests without an
`Origin` remain available for CLI/curl compatibility. This policy does not authenticate users,
authorize operators, or make an unauthenticated non-loopback deployment safe to expose publicly.

The optional Windows-installer gateway has a different network contract. It runs as
`LocalSystem`, is unauthenticated, and listens on `0.0.0.0:7600` so BHTune and approved remote
clients can reach native OPC DA/DCOM on that host. Interactive installation displays this
warning before a clean install selects the component; silent clean installation requires
`/INSTALL_GATEWAY=1`. The installer never creates or modifies a Windows Firewall rule.
Selecting the component is therefore not authorization to expose TCP `7600` beyond a trusted OT
network.

The installer refuses to adopt an existing unowned `OpcdaBridgeGateway` registration, overwrite
unexpected content in its managed Program Files directory, or proceed while TCP `7600` has an
unexpected listener. Do not work around those checks by deleting services, terminating
processes by name, or changing ownership metadata. Resolve the existing deployment explicitly
and preserve its configuration/data first.

## Packaging and database recovery boundaries

The Windows NSIS installer keeps exactly one verified rollback backup under
`%ProgramData%\ByteHound\bhtune\` once an installer-managed database exists. A genuinely empty
clean install has no prior database to protect and therefore has no rollback backup. Automatic
database backup and rollback cover only the
installer-managed SQLite database named by the absolute top-level `db` setting in the preserved
configuration, including its `-wal` and `-shm` companions when present. The installer validates
that path before stopping the service; it refuses malformed, relative, ambiguous, or external
database paths by default.

When the optional gateway is installer-owned, both Windows services participate in one
transaction. The installer stops them before replacement, snapshots the gateway executable and
the complete gateway ProgramData subtree, validates the candidate service/listener/read-only
smoke path, and restores both service definitions, data, and prior running/stopped states on
failure. Gateway configuration, index database and SQLite sidecars, build metadata, logs, and
rollback evidence remain inside this boundary even when BHTune uses an external database.

An operator who prepares an existing external database file and independently backs it up can
explicitly acknowledge that boundary with `/CUSTOM_DB_BACKUP_CONFIRMED=1`. A missing or
inaccessible external file, or a path that names a directory, fails closed before the installer
stops the service; the installer does not create external database parents or files. After the
preflight succeeds, the installer may replace binaries and service state, but the external
database remains operator-owned: the installer does not claim to have backed it up or to be able
to roll it back. A failed health/version validation, including validation of the restored service
after rollback, still requires checking that database and the service manually.

The NSIS-installed BHTune service runs as `NT AUTHORITY\LocalService`. An external database
therefore needs service-account access on both the existing database file and its parent
directory, including permission to create or update SQLite's `-wal` and `-shm` sidecars.
Elevated installer access is not evidence that the service can use an operator-owned path; the
installer leaves those ACLs unchanged and the startup health check is the definitive validation.
Grant only the minimum access required for the service, and independently back up the database
before acknowledging the override. The optional gateway runs separately as `LocalSystem`,
matching the upstream native OPC DA/DCOM service contract.

Linux packages preserve `/etc/bhtune` and `/var/lib/bhtune` across upgrades and removal; they do
not silently delete operator data. Docker deployments have the same boundary through the mounted
`/var/lib/bhtune` volume. For every packaging path, stop the service before copying or restoring a
SQLite database and keep the database, `-wal`, and `-shm` files together. Package installers do
not create firewall rules or widen BHTune's loopback bind. The optional Windows gateway retains
its explicit all-interface bind without adding a firewall rule. Windows uninstall uses a guarded
two-pass cleanup: ProgramData, gateway configuration/index/logs, installer recovery state, and
operator data remain in place, while ownership metadata is removed only after the fixed Program
Files tree has been independently verified absent.

The Debian, RPM, and Arch package units are package-managed variants of the same hardened
systemd service and use `/usr/bin/bhtune-server`; the manual archive unit is separate and uses
`/usr/local/bin/bhtune-server`. Installing any package reloads systemd metadata but does not
enable or start BHTune. Operators must explicitly run `sudo systemctl enable --now
bhtune-server` after reviewing the configuration. Package upgrades preserve the previous
service state, and package removal preserves `/etc/bhtune`, `/var/lib/bhtune`, SQLite sidecars,
and logs rather than treating them as disposable payload files.

Debian package dependency metadata is intentionally adaptive: `cargo-deb` uses
`depends = "$auto"` and relies on `dpkg-shlibdeps` in a Debian-capable packaging environment.
An empty `Depends:` field from a host that lacks that tool is an incomplete packaging result, not
evidence that the runtime has no shared-library requirements. Do not install such an artifact
without rebuilding it in the supported Debian environment or independently verifying its
dependencies.

The Arch `bhtune-bin` package is generated from exact stable release inputs and is not a live
source build. Its publication workflow rejects prerelease and arbitrary refs, validates every
source checksum, builds as a non-root Arch user, and never enables or starts the service during
package installation. Until the first stable release and manual AUR publication exist, use the
workflow only for validation; do not present `bhtune-bin` as an available public package.

The Windows installer is not Authenticode-signed. SmartScreen may therefore warn about the
publisher even when the file is intact. Verify the release checksum and, when available, the
Sigstore bundle and GitHub provenance separately; those checks establish integrity and build
provenance, not Windows publisher trust.

If an upgrade health check fails, leave the affected services stopped or in the
installer-reported rollback state and preserve the installer log, service definitions, health
response, gateway listener/process evidence, and database files. Do not repeatedly rerun an
upgrade against the same live database while the failure is unexplained. For an NSIS
installation, the installer reports rollback success only after the restored BHTune service
answers the expected health/version check and a previously running gateway passes its bounded
read-only smoke check; if either check fails, treat the rollback as incomplete, inspect the
preserved ProgramData rollback directory, and restore the
database plus `-wal`/`-shm` companions only while the service is stopped. For package or manual
archive installs, keep the prior verified package/archive and restore the binaries as a matched
pair, then start the service and verify `/api/health` before considering the recovery complete.
The [operator runbook](operator-runbook.md#12-incident-response) lists the evidence to collect and
the bounded recovery sequence.

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
| `4`  | `[tuning].timeout_secs` elapsed before the test finished                                 |
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
