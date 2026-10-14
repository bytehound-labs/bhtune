# BHTune

BHTune is an open-source PID auto-tuner for industrial DCS/PLC loops that runs Modified Relay
Feedback Tests (MRFT) and calculates PID constants for operator review.

MRFT periods retain sub-second precision, and result extrema include relay-switch samples. The
engine follows one corrected calculation path, without legacy bug-emulation modes.

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

The [Web UI visual reference](docs/guides/web-ui/overview.md) includes dynamically generated
screenshots of the Full and Demo workflows. This completed Full-mode run shows calculated PID
results and the PV/commanded-MV trend.

[![Completed BHTune Full-mode run detail with calculated PID results and a PV/commanded-MV trend](https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=a459523a7bbb)](https://bytehound-labs.github.io/bhtune/generated/web-ui/full-run-complete.png?v=a459523a7bbb)

## Installation

No stable `v0.1.0` release has been published. Prerelease and validation artifacts are not
supported installs. See the [installation guide](docs/getting-started/installation.md) for
source-build instructions, supported distribution details, Windows installer diagnostics, and
recovery.

Source-build prerequisites, development recipes, and applicable checks are covered in the
[contributor guide](CONTRIBUTING.md). The [crate graph](docs/architecture/crate-graph.md) explains
the shared runtime and peer CLI/server adapters; the [agent contract](AGENTS.md) records their
invariants.

Release packaging includes platform archives, Debian/RPM packages, AUR, Docker, and the Windows
installer. macOS packaging includes a launchd job and a Homebrew formula template that requires
a tap and verified stable-release checksums before installation.

The pre-v0.1 schema baseline does not migrate databases created by earlier pre-release builds.
Before switching versions, preserve a verified SQLite backup and configure a new database
file. BHTune reports incompatible migration history without applying migrations or deleting
the old database.

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
navigation. Each branch arrow expands or collapses its children with a single click without
changing the selected tag; tag labels retain selection and double-click behavior.
Run status changes are announced politely. History filters and page offsets are
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

The OPC tag browser provides **Refresh Index**, **Cancel Indexing** during a build, and
**Delete Index**, plus **Enable Auto-refresh** or **Disable Auto-refresh** for a usable index.
New and recreated indexes start with automatic refresh off. Manual builds and retries do not
change the saved choice; an explicit opt-in survives gateway restarts. Disabling it stops
future scheduling without cancelling an active build or deleting cached tags.
`Index: ready` describes cached search independently of scheduling. A usable enabled index shows
a days-and-hours countdown when the gateway reports a next-refresh time. If auto-refresh is
enabled but no usable index exists, the browser prompts you to run **Refresh Index** once; it never
starts the first build automatically.

## Safety

A live MRFT can switch a loop to Manual and stroke its valve. Read the
[safety guide](docs/guides/safety.md) before connecting to plant equipment: PID constants are
not written automatically, CLI and GUI write-back actions require explicit confirmation and
readback verification, and an incomplete restore requires an operator to inspect the loop.
Omitting PID write-back does not make a live tune read-only: it still changes mode and commands
MV relay steps. Use `bhtune check` for a read-only rehearsal.
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
[end-to-end tutorial](docs/guides/end-to-end-tutorial.md),
[troubleshooting](docs/guides/troubleshooting.md), [glossary](docs/reference/glossary.md),
[release guide](docs/guides/releasing.md), and [roadmap](docs/roadmap.md).

## Plant-free native OPC DA validation

Windows validation runs the real CLI through a verified official gateway and isolated synthetic
COM fixture: read-only preflight, paged browsing, one manual index, and a Yokogawa-template MRFT
with PID write-back and restoration. Product OPC I/O remains COM-free. See
[native smoke requirements and reproduction](CONTRIBUTING.md#windows-native-opc-da-smoke) for
registration, firewall, cleanup, and evidence rules. This plant-free test does not establish
CENTUM firmware, site/DCOM/write-permission compatibility, or browser rendering.

## License

BHTune is licensed under [AGPL-3.0-or-later](LICENSE). Contributors should read and sign the [Contributor License Agreement](CLA.md).
