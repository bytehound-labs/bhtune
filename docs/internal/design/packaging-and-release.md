# Packaging and release

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Distribution channel summary

`pkg-evaluate-others` and `pkg-aur` provide `.deb` and `.rpm` packages (via `cargo-deb` and
`cargo-generate-rpm`, sharing the same asset set as the Docker image), the guarded `bhtune-bin` AUR
generator and reusable validation/publication workflow, `cargo-binstall` metadata on `bhtune`,
and a prepared-but-inert Homebrew formula awaiting a real tap repo and release checksums. The AUR
workflow accepts only exact stable `vX.Y.Z` tags for publication; prereleases and arbitrary refs are
validation-only, and the first publication is still a manual post-release action. `release.yml`
gained a new `package-deb-rpm` job, deliberately separate from the existing per-platform `build`
matrix rather than extra steps on its Linux leg, because `upload-rust-binary-action` always builds
with an explicit `--target`, leaving binaries in a target-triple subdirectory the packaging asset
paths don't expect. Both new package formats, and the job itself, were validated by actually
dispatching `release.yml` in GitHub Actions rather than trusting local testing alone — which caught
a real bug (`cargo generate-rpm` doesn't create its own missing output directory, unlike
`cargo-deb`) invisible to local runs because the local test directory always happened to pre-exist.
See "`pkg-evaluate-others`: the remaining distribution channels" below for the full design,
including a `-p` flag that is a path for one tool and a crate name for the other despite identical
`--help` wording, and why winget stays out of scope for now.

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
updater — none of which bhtune needs, since the Windows NSIS installer
(`pkg-windows-installer`) and
the AUR package (`pkg-aur`) are the actual installer stories, not a `curl | sh` script.
Adopting the sibling project's simpler, working tool beats introducing a second,
heavier one for the same job — directly the kind of cross-project consistency
`cross-project-ci-audit` recommended pursuing.

**One archive per platform, bundling both binaries.** `bin: bhtune,bhtune-server` in a
single `upload-rust-binary-action` step packages both into one
`bhtune-$tag-$target.(tar.gz|zip)` archive (plus `LICENSE`/`README.md` via `include:`) —
matching the "one package, not two" packaging decision below: now that the GUI is
browser-served rather than a Tauri app, there is no GUI-toolkit dependency that would need
keeping off a headless server build, so there is no reason to ship the CLI and the server
as separate packages either.

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

## `pkg-evaluate-others`: the remaining distribution channels

Evaluated the "nearly free" and "moderate effort" channels from the packaging shortlist
(see "Key architectural decisions") and shipped three of them; the fourth (Homebrew) is
prepared but deliberately not yet activated. winget remains out of scope for now (see
below).

**`.deb` and `.rpm`, both built from the same `[package.metadata.*]` blocks on
`crates/bhtune-cli/Cargo.toml`, the same asset set as the Docker image and the release
archives: both binaries, man pages, shell completions, and the `bhtune-server` systemd
unit.** `cargo-deb` builds the `.deb`; `cargo-generate-rpm` builds the `.rpm`. One package
per format, not per binary, for the same reason as the Docker image and the release
archive: there is no GUI-toolkit dependency to keep off a headless install anymore, so
splitting the CLI and the server apart would only add packaging work for no benefit.

**Neither tool builds or strips the binaries itself, unlike `cargo-deb`'s own defaults in
other invocation modes** — both are invoked with pre-built, pre-stripped release binaries
already sitting at `target/release/`, confirmed by testing: `cargo-deb --no-build` and
`cargo generate-rpm` (which has no build step at all, ever, in any invocation) both simply
read whatever is already on disk.

**Path resolution is a real, easy-to-get-wrong difference between the two tools.**
`cargo-deb`'s relative asset paths resolve against _the crate's own manifest directory_
(`crates/bhtune-cli/`), hence the `../../` prefixes on every path in its `assets` block
that reaches outside that directory. `cargo-generate-rpm`'s relative paths resolve against
_the current working directory first_, falling back to the crate directory only if not
found there (confirmed against its own `generate_expanded_path`/`load_script_if_path`
source) — since it's invoked from the workspace root (matching `release.yml`'s actual
invocation), every path in its `assets` block is written workspace-root-relative with no
`../../` prefix at all. Mixing the two conventions up produces a tool that runs without
error but silently packages the wrong files (or none), so this was verified by building
and manually inspecting the contents of both a real `.deb` and a real `.rpm` file — not
just by reading the source.

**`cargo-generate-rpm -p` is a path, not a package name, despite its own `--help` text
saying otherwise** ("Name of a crate in the workspace") — confirmed in its source
(`Config::new(Path::new(p), ...)` joins the argument directly with `Cargo.toml`). It must
be invoked as `cargo generate-rpm -p crates/bhtune-cli`, not `-p bhtune` (the latter fails
with "No such file or directory"). `cargo-deb -p`, by contrast, really is a package name,
matching its own `--help` text correctly.

**Neither tool needs a hand-written `dpkg-shlibdeps`/`find-requires` step for shared
library dependencies, but for different reasons.** `cargo-deb`'s `depends = "$auto"` calls
out to Debian's own `dpkg-shlibdeps`, which is standard tooling on any real Debian/Ubuntu
build host (including `ubuntu-latest` GitHub runners) even though it isn't present on
every development sandbox. `cargo-generate-rpm`'s default `auto-req = "auto"` mode instead
uses the Rust `rpm` crate's own built-in ELF scanner when no external `find-requires`
script is present — confirmed by inspecting a built test package's `requirename` header,
which correctly listed versioned `glibc`/`libgcc_s`/`libm`/`ld-linux` requirements with
zero extra configuration.

**`cargo-generate-rpm` has no automatic systemd-unit lifecycle integration (no
`dh_installsystemd` equivalent), unlike `cargo-deb`'s `[package.metadata.deb.systemd-units]`
table.** The unit file is just a plain asset in the `.rpm` case; enabling/disabling/
restarting it across install/upgrade/removal is hand-scripted via
`post_install_script`/`pre_uninstall_script`/`post_uninstall_script`, using the classic,
portable `systemctl preset`/`disable`/`stop`/`daemon-reload`/`try-restart` form (not the
newer `systemd-update-helper`-delegating rewrite some distros' RPM macros now expand to,
since that helper's presence isn't guaranteed across every RPM-based distro) — the same
shell these macros have expanded to for years, confirmed against systemd upstream's own
`macros.systemd.in`. RPM's `$1` scriptlet argument conventions (install vs. upgrade vs.
final removal) follow the Fedora Packaging Guidelines' Scriptlets page exactly.

**`cargo-generate-rpm`'s `-o <dir>/` does not create a missing output directory itself,
unlike `cargo-deb`'s `-o <dir>/`.** Caught by actually dispatching `release.yml` in CI, not
by local testing alone — local testing happened to always pre-create the output
directory, masking the bug. A trailing-slash path that doesn't exist yet makes the tool
treat it as a literal (non-existent) _file_ target rather than a directory to create,
failing with `Is a directory (os error 21)` once the OS's own trailing-slash-implies-
directory rule kicks in. Fixed with a plain `mkdir -p` immediately before the
`cargo generate-rpm` invocation in `release.yml`.

**`release.yml`'s new `package-deb-rpm` job is deliberately separate from `build`'s
existing per-platform matrix, not extra steps on `build`'s Linux leg, because of where
`upload-rust-binary-action` actually leaves its build output.** That action always passes
an explicit `target:` input, so — confirmed by reading its `main.sh` — it always builds via
`cargo build --target x86_64-unknown-linux-gnu`, leaving binaries under
`target/x86_64-unknown-linux-gnu/release/`, not the plain `target/release/` the Cargo.toml
packaging blocks above assume (and that local testing used). Rather than adjust the
asset-path convention to depend on cross-compilation-target-dir details, `package-deb-rpm`
does its own untargeted `cargo build --release` — one extra compile, cheap next to the
existing three-platform matrix, that keeps the already-tested asset paths correct with no
cross-compilation assumptions at all. It builds and packages on every trigger
(`workflow_dispatch` dry run or a real tag push), matching `build`'s own dry-run
convention, and uploads the two files to the release only on a real tag push, via a plain
`gh release upload` — no additional third-party action needed for two files.

**`cargo-deb` installs from a prebuilt binary via `taiki-e/install-action`; `cargo-
generate-rpm` does not and is built from source via `cargo install --locked` instead** —
confirmed against its GitHub Releases, which carry no binary assets at all, only source
tags.

**`[package.metadata.binstall]`, added to `bhtune` only, not `bhtune-server`.** Inert
until `bhtune` is actually published to crates.io (`release-plz.toml` has
`publish = false` workspace-wide, pending `release-v1`), but ready the moment it is, since
`cargo binstall` only reads this from a manifest it's already fetched — no separate opt-in
step needed later. `bhtune-server` is deliberately excluded: it has its own
`publish = false` and is meant to be installed as a system service via the OS packages
above, not fetched by `cargo install`/`cargo binstall` as a CLI tool. Two details had to
match `release.yml`'s actual archive layout exactly, both confirmed against
`taiki-e/upload-rust-binary-action`'s own README rather than assumed: `bin-dir = "{ bin
}{ binary-ext }"` is flat, with no wrapping subdirectory, because that action's
`leading-dir` input defaults to `false`; and `pkg-url` hardcodes a literal `v` before every
`{ version }` reference, because binstall has no `{ tag }` template variable at all, only
the bare, unprefixed crate version — exactly the pattern binstall's own docs show for a
project with `v`-prefixed tags like this one's.

**A prepared-but-inert Homebrew formula, `packaging/homebrew/bhtune.rb`, deliberately not
yet wired to a real tap repo.** Standing up `bytehound-labs/homebrew-bhtune` and computing
real release checksums is deferred until closer to v1 — matching the "moderate effort"
tier in "Key architectural decisions" — but the formula content itself costs nothing to
write and review now. Supports only the two platforms the release matrix actually
produces (Linux x86_64, macOS arm64); there is no Intel Mac or Linux ARM archive to point
a formula at. Installs only the two binaries plus `LICENSE`/`README.md`, matching exactly
what's in the release archive today — man pages and shell completions are deliberately
left out rather than widening `release.yml`'s `include:` list to add them, since that
input has no glob-pattern support (confirmed in `upload-rust-binary-action`'s own
`action.yml`) and would need every one of `docs-generated-cli`'s auto-generated man pages
named individually, which would drift out of sync with the entire point of generating
them.

**winget remains out of scope.** It requires PR-ing a manifest into Microsoft's community
repo on every single release, which only makes sense once `pkg-windows-installer`'s NSIS
installer is itself stable — revisit then, not before.

**`bhtune-bin` publication remains intentionally conditional.** The generator produces a
binary-only Arch package from an exact stable Linux release archive and immutable tag-pinned
ancillary files. It generates `.SRCINFO` through non-root `makepkg --printsrcinfo`, validates
every source checksum and package path, and exercises installation, upgrade, service-state,
and removal preservation in disposable Arch environments. The reusable workflow supports
dry-run validation for arbitrary refs without AUR credentials, but publication requires the
real stable release assets, release evidence, AUR SSH credentials, and a clean expected AUR
remote. No AUR package is publicly available until the first stable release and manual
publication have completed. Debian's `depends = "$auto"` remains deliberate: package builds
must run where `dpkg-shlibdeps` is available rather than substituting a stale hand-maintained
dependency list.

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
  and restores it through `ds-sync-pull` after successful pulls. Keep `rbw` unlocked for those
  operations, never print or commit secret values, and add new keys to `.env.example` through
  `ds reverse` rather than hand-maintaining a divergent schema.
- **Release PR merging uses the guarded `auto-merge.yml` workflow.** It requests GitHub's
  built-in squash auto-merge only for a validated same-repository `release-plz-*` PR authored by
  `mikeboiko`, targeting `main`, while the kill switch is true. Ordinary PRs continue to use
  the normal contributor/maintainer merge process.
