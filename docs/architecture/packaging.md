---
sidebar_position: 8
---

# Packaging and distribution

The Windows NSIS installer is BHTune's primary distribution artifact. It installs the CLI
and server and can include the independently verified OPC DA gateway as an optional component.
The installer workflow validates that payload, but the installer is not a public stable-release
asset until the first stable release is activated.

The documented distribution status is:

| Channel                      | Status                                                                                                                                                     |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Linux/macOS/Windows archives | Paired CLI/server archives are built for Linux x86_64 GNU, macOS arm64, and Windows x86_64 MSVC. Stable installation requires the approved stable release. |
| Windows NSIS installer       | Primary distribution path; its stable-release asset is pending the first stable release.                                                                   |
| Docker / GHCR                | The `edge` image is the available pre-stable image. Stable version tags depend on a stable release.                                                        |
| Debian `.deb` and RPM `.rpm` | Package definitions and release jobs are prepared; packages are not publicly installable until stable release assets exist.                                |
| Arch `bhtune-bin` / AUR      | The generator and validation workflow are prepared. First publication is a manual post-release step; there is no public package before then.               |
| Homebrew / launchd           | A formula template and LaunchDaemon are supplied. The formula needs a real tap and verified release checksums before publication.                          |

## Paired artifacts and build inputs

Archives, OS packages, the installer, and Docker ship `bhtune` and `bhtune-server` together.
The browser-served UI has no desktop GUI toolkit to justify splitting the two adapters into
different packages. Portable archives also contain `LICENSE` and `README.md`; service-manager
templates are fetched from the same immutable tag, not assumed to be inside the archive.

Standalone release builds must build `frontend/dist/` before compiling the server's embedded
assets. Build hosts need `protoc` for the bridge's gRPC code generation and a C compiler for
bundled SQLite; installed binaries do not need a separate SQLite library, Node, or a Rust
toolchain. These are build inputs, not target-host runtime services.

## Linux packages and macOS service files

Debian/RPM metadata lives in `crates/bhtune-cli/Cargo.toml`. Both package formats ship the
paired binaries, generated man pages/completions, and the package-managed
`/usr/bin/bhtune-server` systemd unit. Debian resolves asset paths relative to the crate
manifest; the RPM job is invoked from the workspace root and uses root-relative asset paths.
`cargo deb -p bhtune --no-build` selects a package, whereas
`cargo generate-rpm -p crates/bhtune-cli` selects a crate directory. Packaging consumes the
prebuilt release binaries and must not accidentally pick stale target output. Create the RPM
output directory before passing it to `-o`; unlike cargo-deb, cargo-generate-rpm does not create it.

Debian's automatic shared-library dependencies require `dpkg-shlibdeps`; an empty result on
an unsupported build host is not a dependency-free artifact. Package lifecycle handling reloads
systemd without enabling/starting a fresh service, preserves running/stopped state on upgrade,
and leaves configuration, database/sidecars, and logs on removal. The manual archive unit uses
`/usr/local/bin` instead and must not be mixed with the package-managed unit.

`bhtune-bin` is generated from exact stable binary assets and tag-pinned ancillary files.
Non-root `makepkg` generates `.SRCINFO` and validates source checksums and lifecycle behavior.
Prerelease/arbitrary refs are validation-only; first AUR publication remains a separately
verified manual post-release step. Never hand-edit generated package metadata.

The Homebrew template covers the release matrix's Linux x86_64 and macOS arm64 archives.
The supplied macOS LaunchDaemon is separate from formula activation. See
[installation](../getting-started/installation.md) for service identities, paths, and rollback.

## Container boundary

The Dockerfile builds the frontend, then the Rust binaries, then a non-root runtime containing
the binaries and CA certificates. Pinned base-image digests and BuildKit dependency/target
caches support repeatable inputs without shipping caches or source in the runtime layer.
Only image-affecting pushes publish images; PR/manual validation builds do not push.

The image binds `0.0.0.0:8787` inside the container so explicit host port publication works.
This does not add authentication: Full mode still requires trusted-network placement.
The `/var/lib/bhtune` volume is persistent operator state, not disposable container payload.
`bhtune-server healthcheck` is a bounded loopback HTTP liveness check, not database readiness,
proxy validation, or evidence of plant safety.

## Windows installer and release ownership

The optional gateway is one pinned official 32-bit upstream artifact, independently verified
before embedding. It is never rebuilt locally or downloaded by the installer at runtime.
Ownership checks and a combined two-service rollback protect upgrades without adopting an
unowned service or overwriting operator data. See the
[Windows installation contract](../getting-started/installation.md#windows-nsis-installer).

`release-plz` prepares the git-only product version, changelog, and tag; it does not publish
crates or create a second GitHub Release. Only `release.yml` owns the GitHub Release/assets.
The reusable installer and AUR workflows validate their channels separately; validation does
not activate stable installer attachment, AUR publication, or a Homebrew tap.
Checksums, CycloneDX SBOM, Sigstore bundles, and GitHub provenance accompany release acceptance.
The [release-verification guide](../guides/release-verification.md) distinguishes what each
piece of evidence proves.

The first stable release has not been cut. Prerelease and dry-run artifacts support validation;
they do not make the stable installation channels available. The
[installation guide](../getting-started/installation.md) describes available artifacts and
platform-specific steps, and the [release guide](../guides/releasing.md) documents the
fail-closed release process and activation gates.
