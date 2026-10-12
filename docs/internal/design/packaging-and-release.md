# Packaging and release

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Distribution channel summary

The release workflow builds Linux x86_64, macOS arm64, and Windows x86_64 archives containing
both binaries. Docker images are published to GHCR, and the Windows NSIS installer is validated
separately; attaching it to a stable release remains deferred. The `bhtune` manifest retains
`cargo-binstall` metadata for a future approved crates.io publication. No Debian/RPM, Arch/AUR,
Homebrew, or bundled macOS launchd package definitions are shipped.

## `build-matrix`: the release binary matrix

`.github/workflows/release.yml` builds and packages the `bhtune` (CLI) and `bhtune-server`
(GUI/HTTP) binaries for Linux (`x86_64-unknown-linux-gnu`), macOS
(`aarch64-apple-darwin`), and Windows (`x86_64-pc-windows-msvc`) — the same three-platform
shape opcda-bridge already ships.

**`taiki-e/create-gh-release-action` + `taiki-e/upload-rust-binary-action`, not
`cargo-dist`.** The plan originally called for `cargo-dist`, but reviewing opcda-bridge's
own already-working `release.yml` (part of `cross-project-ci-audit`) turned up a simpler,
already-proven alternative doing exactly what this project needs, with far less machinery:
one action builds the binaries, packages a platform-appropriate archive (`.tar.gz` on
Unix, `.zip` on Windows), computes checksums, and uploads to the tag's GitHub Release.
`cargo-dist` additionally generates shell/PowerShell/npm installer scripts and an
updater — none of which BHTune needs, since the Windows NSIS installer
(`pkg-windows-installer`) and the platform release archives are the distribution paths, not a
`curl | sh` script.
Adopting the sibling project's simpler, working tool beats introducing a second,
heavier one for the same job — directly the kind of cross-project consistency
`cross-project-ci-audit` recommended pursuing.

**One archive per platform, bundling both binaries.** `bin: bhtune,bhtune-server` in a
single `upload-rust-binary-action` step packages both into one
`bhtune-$tag-$target.(tar.gz|zip)` archive (plus `LICENSE`/`README.md` via `include:`).
The browser-served UI has no separate GUI-toolkit dependency, so the CLI and server ship
together in each platform archive.

**The frontend must build before the Rust build, every time, in this specific
workflow.** `bhtune-server`'s `--release` profile embeds `frontend/dist/` into the binary
at compile time via `rust-embed` (see `server-embed-spa`); a debug build (as `e2e.yml`
uses) reads the directory live off disk instead and doesn't need this ordering. This is
the one workflow in the repo that produces `--release` binaries meant to run standalone
without the source tree alongside them, so it's the one place a missing/stale
`frontend/dist/` would silently ship a binary with no UI (or an old one) baked in. The
step order is: `pnpm/setup@v2` (which runs `pnpm install` itself) → `pnpm --filter
bhtune-frontend run build` → the Rust binary build/package step.

**`protoc` is a real build requirement here, not just a test-only one.** Unlike a pure
Rust dependency, `bhtune-driver`'s (non-dev) `opcda-bridge` dependency pulls in
`opcda-bridge-proto`, whose `build.rs` calls `tonic_prost_build::compile_protos` at
compile time — so every platform in the matrix installs `protoc` via
`taiki-e/install-action`, matching `checks.yml`'s `check`/`windows`/`package`/`msrv` jobs.
SQLite itself needs no such step: `bhtune-db`'s `sqlx` dependency uses the bundled
(vendored, statically-linked) SQLite feature, not `sqlite-unbundled`, so the produced
binaries have no external SQLite runtime dependency to document or install separately —
confirmed by running a real `--release` build locally and serving a request from it
directly.

**Two trigger modes, doing genuinely different things, not just a toggle.** An approved
`v[0-9]+.*` tag push runs the real thing: `release.yml` creates the GitHub Release for that tag,
then builds, packages, and uploads the real assets into it. `release-plz` is deliberately not a
second GitHub Release owner; it prepares the git-only release PR and product tag. A manual
`workflow_dispatch` runs the identical matrix in `dry-run: true` mode — builds and packages
everything, proving the frontend build, `protoc` install, and packaging all still work on every
platform — but uploads nothing and requires no release to already exist, making it safe to run at
any time without cutting a real release.

**This does not, by itself, ship v0.1.0.** `build-matrix` is the artifact half of the guarded
release state machine. The first stable tag remains blocked on an explicitly approved RC,
successful hosted canary evidence, the release-integrity and branch-protection gates, and later
activation of the repository kill switch with a least-privilege `RELEASE_PLZ_TOKEN`. See the
release automation guide and `release-v1` in "Phases and todos" below.

## `pkg-docker`: the Docker image

A multi-stage root `Dockerfile` plus `.github/workflows/docker-publish.yml` publish
`ghcr.io/bytehound-labs/bhtune`, a ~110 MB image bundling both binaries and the embedded
SPA. This is deliberately a **secondary** distribution channel: the Windows NSIS installer
(`pkg-windows-installer`) remains the primary one, since OT sites frequently prohibit or
simply lack container runtimes — see the "v1 adapters" bullet above under "Key
architectural decisions" for the full reasoning. Nothing about shipping a Docker image
changes that ordering.

**Three stages, each stripped to exactly what the next stage or the runtime needs.**
`frontend` builds the React SPA with `pnpm`; `builder` compiles `bhtune` and
`bhtune-server` in release mode with the repository's pinned Rust toolchain; `runtime`
contains only the two resulting binaries, `ca-certificates`, and a non-root user — no Node,
no Rust toolchain, no source tree. The Node and Debian bases are pinned to verified
multi-platform OCI index digests in the Dockerfile, allowing BuildKit to select the matching
platform manifest. Manifests are copied before source in the `frontend` stage so
`pnpm install --frozen-lockfile` remains layer-cacheable across source-only changes.

BuildKit cache mounts retain the pnpm store and Cargo registry, Git, and architecture-specific
target data between builds. The final release binaries are copied from the mounted Cargo
target into ordinary builder-layer paths before the runtime stage copies them, so the cache
mount does not hide the artifacts from later stages. The runtime image's Docker health check
runs `bhtune-server healthcheck`; it probes the loopback `/api/health` endpoint with a bounded
HTTP request and does not open the database, initialize logging, or prove database readiness.

**The `frontend/dist/`-before-`cargo build` ordering is load-bearing, not just
convenient.** `bhtune-server`'s `rust-embed` usage only embeds `frontend/dist/` into the
binary for `--release` builds (see `server-embed-spa`'s design section) — there is no
after-the-fact embed step, so the Dockerfile must `COPY --from=frontend
/src/frontend/dist/ frontend/dist/` before running `cargo build --release`, exactly
mirroring `build-matrix`'s `release.yml` step ordering above. Verified directly, not just
by reading the code: an earlier local build run with the copy ordered _after_ `cargo
build` produced a container that served a 503 for every static asset; reordering the copy
and rebuilding fixed it, confirming the failure mode is real rather than theoretical before
trusting the final Dockerfile.

**The Rust builder stage needs two system packages beyond `protoc`, because `slim` isn't
`rust`.** `protobuf-compiler` is already a known requirement — `opcda-bridge-proto`
compiles `bridge.proto` via `tonic-build` at build time, the same non-dev requirement
`build-matrix` installs via `taiki-e/install-action` above. `build-essential` is the new
one this todo surfaced: `bhtune-db`'s bundled SQLite (`libsqlite3-sys`) compiles a small C
amalgamation via the `cc` crate at build time, and unlike the default (non-`slim`) `rust`
image, `rust:1-slim-bookworm` does not include a C compiler at all. Skipping it fails the
build with a `cc` "not found" error rather than anything SQLite-specific, which is easy to
misdiagnose as a missing Rust dependency instead of a missing system one.

**`BHTUNE_BIND=0.0.0.0:8787` is the image's own default, deliberately overriding the
native binary's `127.0.0.1`-only default.** The security posture recorded in "Web app
architecture" above (bind loopback by default, LAN exposure as a loud explicit opt-in) is
preserved, not weakened, by this override: a container's loopback interface is invisible to
`docker run -p`/`--publish` port mapping, so binding `127.0.0.1` _inside_ the container
would make the server unreachable even with a port published, which is a confusing
footgun rather than a safety feature. Running this image and choosing to publish a port is
itself the explicit opt-in that `127.0.0.1`-by-default exists to require on the native
binary — Docker's own network isolation is the real boundary. Verified empirically: the
server was reachable via `curl` from outside the container only with this override in
place, matching the reasoning rather than assuming it.

**`.dockerignore` had one real mistake, caught before it shipped.** An early draft
excluded `website/` wholesale to keep docs-site content out of the build context. That
broke the `frontend` stage's `COPY website/package.json website/package.json` step, since
`website` is a real `pnpm-workspace.yaml` member (see `build-matrix`'s and `docs-site-scaffold`'s
notes on this same fact) whose manifest `pnpm install --frozen-lockfile` needs to resolve
the lockfile, even though only `frontend/` is ever actually built. Fixed by excluding only
the docs-site's content subdirectories (`website/docs`, `website/blog`, `website/src`,
`website/static`, and its two root config files) rather than the whole directory, which
still keeps `website/package.json` copyable. Also excludes build artifacts (`target/`,
`node_modules/`, `frontend/dist/`), VCS/editor metadata, `tests/golden/raw/` (kept excluded
as a guard against a future large capture bloating the build context, even though the
existing raw captures have since been deleted — see `cleanup-golden-traces`), and local
secrets/DB files (`.env`, `*.db*`).

**Publish workflow: build on every trigger, push only on a real push.** A `pull_request`
or manual `workflow_dispatch` run builds the full image — proving the Dockerfile still
works on every PR that touches it — but pushes nothing and touches no registry
credentials, matching `release.yml`'s own dry-run convention above. Only a push to `main`
or a `v[0-9]+.*` tag logs into GHCR and pushes. `docker/metadata-action`'s default
`flavor: latest=auto` adds a `latest` tag only alongside a real `type=semver` tag (i.e.
only on a version-tag push, never on a plain push to `main`), and `type=edge,branch=main`
only fires when the active ref genuinely is `refs/heads/main` — so a PR run and a tag-push
run each produce exactly the tags they should with no extra `enable:`/`if:` conditions
needed. Both facts were confirmed against `docker/metadata-action`'s own README rather than
assumed. Build layers are cached via `type=gha`, shared across runs the same way
`checks.yml`'s Rust jobs already cache `~/.cargo`/`target`.

**Validated locally end-to-end before ever touching CI.** Built the image from a clean
checkout; ran it with a published port and confirmed `/api/health`, `/`, and
`/api/openapi.json` all return real content (not the SPA-fallback 503 a broken
`rust-embed` build would produce); confirmed the SPA's hashed JS/CSS assets carry
`Cache-Control: public, max-age=31536000, immutable` while `/` does not; confirmed a
client-side route (`/runs/new`) still serves the SPA shell rather than 404ing; confirmed
the SQLite database file is created under `/var/lib/bhtune/`, owned by the non-root
`bhtune` user; and confirmed `docker exec ... bhtune template list` (the CLI binary) reads
the same database the running server just seeded, proving both binaries share
`BHTUNE_DB` correctly inside the container.

**`provenance: false`/`sbom: false` on the `build-push-action` step, added after a real
user-visible artifact.** `docker/build-push-action` has attached a build-provenance
attestation as an extra manifest inside the pushed image index by default since v4 — that
manifest carries no real OS/architecture, so the GHCR package page's UI renders it as a
fake `unknown/unknown` platform entry alongside the real `linux/amd64` one (confirmed via
GitHub's own community discussion #45969: a known GHCR-UI-only cosmetic quirk, not present
the same way on Docker Hub). Purely cosmetic — `docker pull`/`run` always resolve the real
platform regardless — but confusing enough to ask about, so it's suppressed rather than
left for the next person to wonder about. Verified by fetching the GHCR package page
before and after: the tag pushed before this change shows two manifest entries, the tag
pushed after shows exactly one.

## Cargo-binstall metadata

`[package.metadata.binstall]` is defined on `bhtune` only. Crates.io publication remains
disabled by `release-plz.toml`, so the metadata is not an active installation path. If
publication is separately approved, `cargo-binstall` can use the existing release archives:
the URL includes the project's `v`-prefixed tag, the binary directory is flat, and Windows
uses ZIP while Unix targets use tar archives. `bhtune-server` remains unpublished and is
distributed with `bhtune` in the platform archives.

## Deferred setup (deliberate, not oversights)

- **Release automation is implemented but fail-closed.** `release-plz.toml` is git-only and
  keeps crates.io publication and GitHub Release creation disabled. `bhtune` is the only
  product-release anchor; `.github/workflows/release.yml` is the sole GitHub Release/artifact
  owner. `.github/workflows/release-plz.yml`, `auto-merge.yml`, and `release-integrity.yml`
  exist, but every side-effecting release/tag/PR or auto-merge path requires
  `RELEASE_AUTOMATION_ENABLED == 'true'`. The repository variable remains false during
  hardening and testing, `RELEASE_PLZ_TOKEN` is intentionally unprovisioned, and public RC or
  stable release actions require explicit approval. See `docs/guides/releasing.md`.
- **dotenv-sync (`ds`) is configured for repository secrets.** `.env.example` is committed with
  `SONAR_TOKEN=` while the real token remains in the ignored `.env` file and the `bhtune`
  Bitwarden note configured by `.envsync.yaml`. `ds sync` restores the local file and `ds push`
  updates the note; the existing `.lefthook.yml` runs the latter through `ds-sync` on pre-commit
  and restores it through `ds-sync-pull` after successful merge-mode pulls. That hook refreshes
  RBW with `rbw sync` before running `ds sync`. Keep `rbw` unlocked for those operations, never
  print or commit secret values, and add new keys to `.env.example` through
  `ds reverse` rather than hand-maintaining a divergent schema.
- **Release PR merging uses the guarded `auto-merge.yml` workflow.** It requests GitHub's
  built-in squash auto-merge only for a validated same-repository `release-plz-*` PR authored by
  `mikeboiko`, targeting `main`, while the kill switch is true. Ordinary PRs continue to use
  the normal contributor/maintainer merge process.
