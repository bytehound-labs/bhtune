---
sidebar_position: 8
---

# Packaging and distribution

The Windows NSIS installer is BHTune's primary distribution artifact. It installs the CLI
and server and can include the independently verified OPC DA gateway as an optional component.
The installer workflow validates that payload, but the installer is not a public stable-release
asset until the first stable release is activated.

The documented distribution status is:

| Channel | Status |
| --- | --- |
| Windows NSIS installer | Primary distribution path; its stable-release asset is pending the first stable release. |
| Docker / GHCR | The `edge` image is the available pre-stable image. Stable version tags depend on a stable release. |
| Debian `.deb` and RPM `.rpm` | Package definitions and release jobs are prepared; packages are not publicly installable until stable release assets exist. |
| Arch `bhtune-bin` / AUR | The generator and validation workflow are prepared. First publication is a manual post-release step; there is no public package before then. |
| Homebrew | Publishing is an open evaluation, not an active release channel. |

The first stable release has not been cut. Prerelease and dry-run artifacts support validation;
they do not make the stable installation channels available. The
[installation guide](../getting-started/installation.md) describes available artifacts and
platform-specific steps, and the [release guide](../guides/releasing.md) documents the
fail-closed release process and activation gates.
