---
sidebar_position: 2
---

# CLI quickstart

This walks through a complete tune from the command line against BHTune's built-in FOPDT
process simulator — no OPC DA connection, gateway, or real plant equipment required. It's the
fastest way to see the whole tuning lifecycle (relay test → calculated PID constants → history)
end to end.

## Run a zero-configuration demo tune

```sh
bhtune simulate
```

`simulate` is a defaulted subset of the full `tune` command (see [CLI reference](../reference/cli.md#bhtune-tune)
for every flag `tune` accepts against a real loop) — it needs no flags at all to run a complete
MRFT test against a synthetic Flow-type loop, using a fixed internal PV/MV tag pair instead of a
real OPC DA tag. A run typically takes under a minute:

```text
No PID constant tags configured for this run's driver/template; skipping write-back.
Tune completed successfully (run id 1).
```

("No PID constant tags configured" is expected and correct here — the simulator driver has no
PID constant tags to write to at all, so write-back is always skipped for `simulate` regardless
of `--write-pid`. See [`bhtune tune`](../reference/cli.md#bhtune-tune) for writing PID constants
back to a real controller.)

Every run is persisted to BHTune's SQLite database (see
[Installation](installation.md#where-bhtune-stores-its-data)) the moment it starts, not just on
completion, so even an aborted or crashed run leaves a record.

## Look at what it calculated

```sh
bhtune history show 1
```

```text
Run #1 — Tag name: Sim.Loop1.PV
  Notes:           —
  Driver:          Simulator
  Outcome:          Completed
  Started at:       2026-08-16T06:51:28.165183623+00:00
  Completed at:     2026-08-16T06:52:18.574446068+00:00
  Template:         Yokogawa CentumVP (Builtin)
  Process/controller: Flow / Pi
  Relay amplitude:  10%
  Cycles skip/count: 1 / 2
  Initial PV / MV:  50 / 50
  MV range:         0 - 100
  PV range:         0 - 100
  Direction:        Reverse
  Samples recorded: 64
  Timing:
    Basis:                    FixedStep
    Requested interval:       800 ms
    Observed sample gaps:     63
    Mean / max sample gap:    800.000 / 800.000 ms
    Missed poll opportunities: 0
    Oscillation period:       46500.000 ms
    Approx. samples / period: 58.125
    Sampling adequacy:        adequate
    Poll latency:
    PV reads:                 count=64 mean=0.010 ms max=0.050 ms
    MV writes:                none
    MV verification:          none
    Sample persistence:       count=64 mean=0.100 ms max=0.500 ms
    Total tick work:          count=64 mean=0.200 ms max=1.000 ms
  Restore:          confirmed
  Calculated results:
    LEVEL        STATUS     KP         TI(min)    TD(min)    PROP         INTEGRAL   DERIV      REASON
    Aggressive   Valid      0.5885     0.0971     0.0000     169.9304     5.8256     0.0000     -
    Moderate     Valid      0.3941     0.0971     0.0000     253.7703     5.8256     0.0000     -
    Sluggish     Valid      0.2949     0.0971     0.0000     339.1090     5.8256     0.0000     -
```

Three response levels are always evaluated (Aggressive/Moderate/Sluggish) — see
[MRFT concepts](../guides/mrft-concepts.md) for what they mean and how to pick one. A usable
row has `STATUS` set to `Valid`; if the measured amplitude, period, or a converted PID value is
zero, non-finite, or otherwise unusable, the row is retained as `Invalid` with a diagnostic
reason and no numeric values, and cannot be written back. `PROP` and `INTEGRAL` here are the
Yokogawa CentumVP template's own units (Proportional Band % and Reset Time in minutes); a
different template reports these in whatever units that DCS/PLC family expects — see
[DCS/PLC templates](../dcs-templates.md).

`history show` also prints timing diagnostics, including the requested and observed cadence,
sampling adequacy (`adequate`, `marginal`, or `not assessed`), and successful latency summaries
for PV reads, MV writes, MV verification, sample persistence, and total tick work. Sampling
adequacy is advisory: `marginal` does not automatically reject a valid result, but it is a
reason to inspect the trend before applying constants.

Every run's exact numbers depend on the simulator's process parameters
(`--sim-gain`/`--sim-tau`/`--sim-dead-time`/`--sim-noise`/`--sim-seed`). The simulator
advances its process and MRFT timestamps by the same configured poll step, so zero-noise runs
with the same inputs are reproducible even when host scheduling differs. A fixed `--sim-seed`
also reproduces the configured noise sequence within the same supported build.

List every run so far, and export one run's per-tick samples:

```sh
bhtune history list
bhtune export 1 --format csv > run-1-samples.csv
```

By default nothing is ever deleted automatically — BHTune retains every run forever until you
opt in to a retention policy. Set `--retention-days`/`BHTUNE_RETENTION_DAYS`/`retention_days` to
a positive whole number in `bhtune.toml` (see [Configuration reference](../reference/config.md))
to delete runs older than that many days automatically on every startup, or run `bhtune history
prune --older-than-days <N>` to prune on demand without waiting for the next startup — add
`--dry-run` to see how many runs would be deleted first.

## Try different loop and controller types

`simulate` accepts the same `--process-type`/`--controller-type`/`--relay-amp` flags as `tune`:

```sh
bhtune simulate --process-type temperature-heat-exchange --controller-type pid --relay-amp 15
```

PID (as opposed to P/PI) is only offered for the two Temperature process types, matching the
tag conventions the built-in templates were authored against.

## Point it at a real loop

Once `opcda-bridge-gateway` is reachable from wherever BHTune runs, `bhtune tune` runs the same
test against a real OPC DA tag instead of the simulator:

```sh
bhtune tune \
  --driver opcda --server Matrikon.OPC.Simulation.1 --bridge-host gateway.plant.local:7600 \
  --tagname FIC101 --template "Yokogawa CentumVP" \
  --process-type flow --controller-type pi --relay-amp 5
```

This reads live tags (PV, MV, ranges, mode, direction), switches the loop to manual, strokes the
relay, and restores the loop when the test ends — see [Safety](../guides/safety.md) before
running this against anything connected to a real process, especially unattended. Live MRFT
timestamps use monotonic elapsed time anchored to UTC, so NTP/manual clock changes cannot distort
the measured relay period; real host, gateway, and OPC latency remains visible. Keep the host and
gateway responsive and use a poll interval comfortably shorter than the expected oscillation
period. Afterward, `bhtune history show <run-id>` reports the requested and observed sampling
cadence, measured oscillation period, approximate samples per period, sampling adequacy, and
successful operation-latency summaries. A live sample gap at least twice the requested interval
is reported as a missed-poll warning without aborting the run or blocking write-back. To also
write the calculated PID constants back:

```sh
bhtune tune ... --write-pid moderate --yes
```

`--yes` is mandatory alongside `--write-pid` for any non-interactive write — see
[Safety](../guides/safety.md#pid-write-back).

## Scripting and automation

`--output json` emits exactly one parseable JSON value on stdout on every path (success,
timeout, abort, write-back failure), with meaningful, distinguished process exit codes, so a
scheduler can tell a clean completion, a Ctrl+C abort, and a failed write-back apart without
parsing prose — see [Automation](../reference/cli.md) and the exit code table in
[`AGENTS.md`](https://github.com/bytehound-labs/bhtune/blob/main/AGENTS.md#automation-cli-automation)
for the full contract.

## Next steps

- [Web GUI quickstart](web-gui-quickstart.md) — the same tuning engine, driven from a browser.
- [Safety](../guides/safety.md) — what happens on Ctrl+C, a stalled read, or a timeout, and how
  PID write-back is verified and rolled back.
- [DCS/PLC templates](../dcs-templates.md) — the tag-mapping system, and how to contribute a
  template for a control system BHTune doesn't cover yet.
