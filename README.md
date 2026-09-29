# BHTune

BHTune is an open-source PID auto-tuner for industrial DCS/PLC loops that runs Modified Relay
Feedback Tests (MRFT) and calculates PID constants for operator review.

## Features

- **Runs on Linux, macOS, and Windows.** BHTune itself has no Windows or COM requirement.
- **No proprietary dependencies.** The project's dependencies are open-source.
- **CLI and web GUI.** Both use the same tuning engine and SQLite database.
- **Plain SQLite.** Run history and templates are stored in an inspectable database.
- **OPC DA through [opcda-bridge](https://github.com/bytehound-labs/opcda-bridge).** A separate
  Windows-side gateway handles OPC DA access; BHTune runs on any supported platform.

## Screenshots

The [Web UI visual reference](docs/guides/web-ui/overview.md) includes screenshots of the Full
and Demo workflows.

## Installation

No stable `v0.1.0` release has been published. Prerelease and validation artifacts are not
supported installs. See the [installation guide](docs/getting-started/installation.md) for
source-build instructions and current distribution status.

## Try it in 5 minutes

1. Run a plant-free simulated tune with `bhtune simulate` using the [CLI quickstart](docs/getting-started/cli-quickstart.md).
2. Start `bhtune-server` and open the simulator workflow in your browser using the [Web GUI quickstart](docs/getting-started/web-gui-quickstart.md).

## Safety

A live MRFT can switch a loop to Manual and stroke its valve. Read the
[safety guide](docs/guides/safety.md) before connecting to plant equipment: PID constants are
not written automatically, CLI and GUI write-back actions require explicit confirmation and
readback verification, and an incomplete restore requires an operator to inspect the loop.

## Explore

Read the [full documentation site](https://bytehound-labs.github.io/bhtune/) or
[try the public simulator Demo](https://bhtunedemo.bytehound.ca/), which uses synthetic process
data without a plant connection. References include the [CLI](docs/reference/cli.md),
[Rust API](docs/reference/api.md), [configuration](docs/reference/config.md),
[DCS/PLC templates](docs/dcs-templates.md), [Demo policy](docs/guides/public-simulator-demo.md),
[release guide](docs/guides/releasing.md), and [roadmap](docs/roadmap.md).

## License

BHTune is licensed under [AGPL-3.0-or-later](LICENSE). Contributors should read and sign the [Contributor License Agreement](CLA.md).
