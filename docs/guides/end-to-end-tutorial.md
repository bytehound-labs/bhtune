---
sidebar_position: 8
---

# End-to-end tutorial

This tutorial starts with a simulator run in the Web UI, then shows how to discover an OPC DA
server, check a live loop, run a tune without PID-constant write-back, and deliberately review a
write-back before reverting it.

> **Live-plant safety:** A live MRFT is not a dry run. It changes the loop mode and commands MV
> relay steps before attempting restoration. Follow your site's procedures and the
> [safety guide](safety.md) before connecting to plant equipment. Full mode has no
> authentication in v1; its default loopback binding is not a reason to expose it to an
> untrusted network.

The angle-bracket values in commands below are placeholders, not real tags or credentials.
Replace them only with the exact values for your installation. Use a template, process type,
controller type, and relay amplitude approved for the loop.

## 1. Run a simulator tune in the Web UI

Start `bhtune-server` using the [Web GUI quickstart](../getting-started/web-gui-quickstart.md#build-and-run),
then open its reported local address in a browser. In Full mode, select **Simulator** on the
New Tune page, review the simulator and test settings, and start the tune. The simulator
generates synthetic PV/MV data without an OPC DA server or a live loop.

When the run completes:

1. Open **History** and select the simulator run.
2. Review its calculated results and trend.
3. Export the recorded samples as CSV or JSON from the run detail page.

When the CLI and server use the same configured database, the same run is available through:

```sh
bhtune history show "<run-id>"
bhtune export "<run-id>" --format csv --output "samples.csv"
```

The CLI export contains samples, not run metadata or calculated results. The simulator has no
PID constant tags, so it cannot write PID settings back to a controller.

## 2. Discover the gateway server and map a tag

Configure the separate `opcda-bridge` gateway according to its
[troubleshooting and setup guidance](https://github.com/bytehound-labs/opcda-bridge/blob/main/docs/troubleshooting.md).
From the BHTune host, inspect gateway compatibility and list the OPC DA servers registered on
that gateway:

```sh
bhtune opc gateway-info --bridge-host "<gateway-host>:7600"
bhtune opc servers --bridge-host "<gateway-host>:7600"
```

Use the exact ProgID returned by `bhtune opc servers` in the Full-mode New Tune form. Select
**Browse tags**, expand the tree to the intended item, and use **Read selected tag** to inspect
its value and quality. Select the item and review the resulting loop mapping; the active
template derives the other tag roles from its suffix rules. Use the exact ItemID returned by
browsing or a trusted mapping. Do not infer a hierarchy by splitting punctuation in a tag.

## 3. Run a read-only check

Before a live tune, run `bhtune check` with the same connection, template, tag, and test
settings. This example also requests a read-only assessment of Moderate PID write-back
readiness:

```sh
bhtune check \
  --driver opcda \
  --bridge-host "<gateway-host>:7600" \
  --server "<server-progid>" \
  --tagname "<pv-tag-prefix>" \
  --template "<template-name>" \
  --process-type "<approved-process-type>" \
  --controller-type "<approved-controller-type>" \
  --relay-amp "<approved-relay-amplitude>" \
  --write-pid moderate
```

`bhtune check` is read-only: it does not start a tune, create run history, or write controller
values. With `--write-pid moderate`, it reads the configured PID tags and any required
template-specific prerequisites; it does not test write permission. A passing check cannot
prove that a later PID write will be accepted or that an accepted MV command will move the
process. No `--yes` is required for `check`.

## 4. Run the live tune without PID write-back

Use the same verified values and an approved relay amplitude:

```sh
bhtune tune \
  --driver opcda \
  --bridge-host "<gateway-host>:7600" \
  --server "<server-progid>" \
  --tagname "<pv-tag-prefix>" \
  --template "<template-name>" \
  --process-type "<approved-process-type>" \
  --controller-type "<approved-controller-type>" \
  --relay-amp "<approved-relay-amplitude>"
```

Omitting `--write-pid` means no calculated PID constants are written. The MRFT itself is still
a live operation: it changes mode, commands MV relay steps, and attempts to restore the
applicable pre-test values. There is no `--dry-run` option for `tune`. Review the completed
run in **History** before deciding whether any PID settings should be applied.

Use the same database for `bhtune` and `bhtune-server` if you want the CLI-created run to
appear in the Web UI. The CLI's global `--db` option, `BHTUNE_DB`, and the `db` config key
select the database.

## 5. Review and confirm PID write-back

On the completed OPC DA run's detail page, choose a response level and select **Review &
write**. Read the loop tag, each destination tag, and every displayed value in the **Review PID
settings** dialog. Cancel if any destination, value, or operating condition is unexpected.
Only after an authorized person approves those exact values should you select **Write PID
settings**. BHTune reads the values back and records the write result.

The CLI rejects `--write-pid <level>` on `tune` or `simulate` unless `--yes` is also present.
`simulate` still skips write-back because it has no PID tags. On `check`, `--write-pid <level>`
only requests a read-only readiness assessment and does not require `--yes`. The Web UI review
dialog is the workflow here when you want to inspect exact targets and confirm them before
writing.

## 6. Revert the recorded PID write-back

To restore the PID values recorded before the run's last recorded PID write-back, verify the
run ID and use:

```sh
bhtune history revert "<run-id>" --yes
```

This is a separate live write and requires `--yes`; the CLI does not offer an interactive
prompt. It uses the server and bridge recorded with the run, and optional `--server` or
`--bridge-host` values only cross-check that connection. Confirm the run ID and follow
site-specific procedures before running it. This PID revert is distinct from restoring the
loop's pre-test operating values or recovering an orphaned tune with
`bhtune restore-loop`.
