# BHTune

BHTune is an open-source PID auto-tuner for industrial DCS/PLC loops that runs Modified Relay
Feedback Tests (MRFT) and calculates PID constants for operator review.

## Features

- **Runs on Linux, macOS, and Windows.** BHTune itself has no Windows or COM requirement.
- **No proprietary dependencies.** The project's dependencies are open-source.
- **CLI and web GUI.** Both use the same application runtime, tuning engine, SQLite database,
  simulator defaults, and common tune-request validation.
- **Read-only preflight.** `bhtune check` and the Full-mode New Tune page's "Check readiness"
  action validate inputs and inspect configured tags before a tune starts. They use the same
  runtime checks without starting an MRFT test, creating run history, or writing controller
  values. The browser report shows each check and tag-read result; the readiness endpoint is
  not mounted in Demo mode.
- **Plain SQLite.** Run history and templates are stored in an inspectable database.
- **Template-based PID precision.** Yokogawa candidates use one decimal place; the other
  built-in families and new custom templates use three significant digits. The PID results
  and write review show the same controller-ready values that are written. Precision is
  editable in a custom template; raw calculations, exports, and recorded restore targets
  retain full precision.
- **OPC DA through [opcda-bridge](https://github.com/bytehound-labs/opcda-bridge).** A separate
  Windows-side gateway handles OPC DA access; BHTune runs on any supported platform. Closing
  the tag browser dismisses seen index-refresh errors without clearing gateway status or
  hiding a later failed attempt.

## Screenshots

The [Web UI visual reference](docs/guides/web-ui/overview.md) includes screenshots of the Full
and Demo workflows.

## Installation

No stable `v0.1.0` release has been published. Prerelease and validation artifacts are not
supported installs. See the [installation guide](docs/getting-started/installation.md) for
source-build instructions, supported distribution details, Windows installer diagnostics, and
recovery.

The Windows distribution uses an NSIS installer for the CLI and web server, with an optional
verified OPC DA gateway component. Installer validation covers external SQLite databases and
WAL-preserving rollback fixtures under the service account.

The pre-release Docker image includes an HTTP liveness check; a healthy status does not establish
database readiness or live-plant safety.

## Try it in 5 minutes

1. Run a plant-free simulated tune with `bhtune simulate` using the [CLI quickstart](docs/getting-started/cli-quickstart.md).
2. Start `bhtune-server` and open the simulator workflow in your browser using the [Web GUI quickstart](docs/getting-started/web-gui-quickstart.md).

## Web UI accessibility

The browser UI associates form labels, hints, and validation errors with their controls.
Dialogs keep keyboard focus inside until they close, and the OPC tag tree supports keyboard
navigation. Run status changes are announced politely. History filters and page offsets are
stored in the URL, so a selected history view can be reloaded or shared. ItemIDs and run IDs
have copy controls with success and failure feedback. PV and commanded-MV trends use concise
legends and a hidden text description for screen readers; live measurements and cycle progress
remain in the status panel above the chart without a repeated measurement-count footer.
Engineering units are not inferred when the run does not record them. Run pages retain
safety outcomes and invalid-result reasons, while
advisory sampling and latency details remain available through CLI history and the run-detail
API rather than a tuning-quality badge. The main navigation, forms, and
history table adapt to viewports at and below 1024 pixels. See the
[Web UI guide](docs/guides/web-ui/overview.md).

The OPC tag browser distinguishes a usable index from its automatic refresh schedule.
The GUI controls the selected server's preference, not the gateway's administrative override.
When gateway configuration blocks scheduling, the browser explains the blocker and disables
**Enable auto-refresh**; a saved enabled preference is labelled separately and can be disabled
without removing searchable data. A countdown appears only when the gateway permits scheduling
and reports a next-refresh time. Older gateways that do not report policy are labelled unverified,
and their controls explicitly save a server preference rather than promise a running schedule.

## Safety

A live MRFT can switch a loop to Manual and stroke its valve. Read the
[safety guide](docs/guides/safety.md) before connecting to plant equipment: PID constants are
not written automatically, CLI and GUI write-back actions require explicit confirmation and
readback verification, and an incomplete restore requires an operator to inspect the loop.
When a live tune process ends unexpectedly, Full-mode startup exports its ownership and
mutation evidence without contacting the controller. Export failure stops orphan retirement
without overwriting existing evidence. Only evidence-backed orphan runs can be
restored with `bhtune restore-loop <run-id> --yes`; legacy or incomplete records fail closed
for manual operator recovery. A stale heartbeat alone never permits restoration: a paused
owner that still holds its OS lock blocks takeover. When an Auto-start run has a configured
setpoint restore target, its initial value must be recorded. Eligible, in-progress, and incomplete
recovery records are retained by age-based pruning and cannot be deleted from history until
recovery is confirmed.
`bhtune check` and Full mode's "Check readiness" action are read-only preflights for a proposed
tune; neither proves controller write permissions or replaces live-plant safety procedures.

## Explore

Read the [full documentation site](https://bytehound-labs.github.io/bhtune/) or
[try the public simulator Demo](https://bhtunedemo.bytehound.ca/), which uses synthetic process
data without a plant connection. References include the [CLI](docs/reference/cli.md),
[Rust API](docs/reference/api.md), [configuration](docs/reference/config.md),
[DCS/PLC templates](docs/dcs-templates.md), [Demo policy](docs/guides/public-simulator-demo.md),
[release guide](docs/guides/releasing.md), and [roadmap](docs/roadmap.md).

## License

BHTune is licensed under [AGPL-3.0-or-later](LICENSE). Contributors should read and sign the [Contributor License Agreement](CLA.md).
