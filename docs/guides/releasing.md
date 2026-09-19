---
sidebar_position: 6
---

# Release automation

BHTune uses a guarded, single-product release state machine. The release process is designed to
make one release preparation change, one product tag, and one GitHub Release, while keeping
artifact generation and supply-chain evidence in one workflow.

The first stable release is not cut. Side-effecting release automation runs only when the
repository variable `RELEASE_AUTOMATION_ENABLED` is exactly `true`; a missing or false value
fails closed. The release PAT is not needed for dry-run validation and must not be provisioned
until the approved RC has passed its canary.

## Ownership

| Responsibility                                        | Owner                                     |
| ----------------------------------------------------- | ----------------------------------------- |
| Product version, product tag, and root `CHANGELOG.md` | `bhtune-cli` through `release-plz`        |
| Release preparation PR                                | `.github/workflows/release-plz.yml`       |
| Release PR integrity status                           | `.github/workflows/release-integrity.yml` |
| Protected-branch squash auto-merge request            | `.github/workflows/auto-merge.yml`        |
| GitHub Release and release assets                     | `.github/workflows/release.yml`           |
| Prerelease artifact and platform acceptance           | `.github/workflows/release-canary.yml`    |

`release-plz` is configured as git-only. It does not publish BHTune crates to crates.io and does
not create GitHub Releases. The `release.yml` workflow is the only workflow that creates a GitHub
Release or uploads release artifacts.

## Normal release flow

1. A meaningful product change reaches `main`.
2. With automation enabled, `release-plz` creates or refreshes one release PR targeting `main`.
   The PR uses the deterministic title `chore(release): prepare vX.Y.Z`, contains one root
   changelog entry covering the workspace crates, and includes the exact stable documentation
   snapshot when the version is stable.
3. `Release integrity`, the normal required checks, CLA validation, and the applicable SonarQube
   analysis must pass. The release PR must be authored by `mikeboiko`, come from the exact
   `bytehound-labs/bhtune` repository on a `release-plz-*` branch, and target `main`.
4. The guarded auto-merge workflow requests GitHub's protected-branch squash auto-merge. It does
   not bypass branch protection or merge directly.
5. After the release PR is merged, `release-plz` creates the single product tag. The exact
   prerelease or stable tag is then handled by `release.yml`.
6. `release.yml` creates the GitHub Release and publishes the matching archives, packages,
   checksums, CycloneDX SBOM, GitHub artifact provenance, and Sigstore bundles.

The release-rate guard runs immediately before GitHub Release creation. It counts stable and
prerelease releases and rejects the operation when there are already three or more releases in
the preceding hour or twelve or more in the preceding 24 hours. API, authentication,
pagination, timestamp, malformed-response, and network failures also reject the operation.

## Safe dry runs

Manual dry runs are available while the kill switch is false. They do not receive the release PAT
and must not create a pull request, push a tag, publish a crate, create a GitHub Release, or
upload release assets.

Run the policy checks locally before dispatching a dry run:

```sh
python3 scripts/check_release_automation_test.py
python3 scripts/check_release_plz_config.py
```

The release-plz workflow's manual dry run validates the release-plz configuration without
activating the side-effecting jobs. The release workflow's manual dispatch builds and packages
the supported matrix without uploading anything to a GitHub Release.

On a stable release PR, also verify the exact documentation snapshot and retention metadata:

```sh
python3 scripts/sync_docs_version.py --repository . --check
```

Before the first stable release, the unversioned site intentionally has no snapshot, so this
check is expected to fail on `main` and on ordinary pre-release branches.

## Prerelease acceptance

An RC requires explicit maintainer approval. Keep `RELEASE_AUTOMATION_ENABLED=false` throughout
RC creation and validation.

1. Create a temporary branch from the accepted `main` commit and update the workspace version to
   an exact SemVer prerelease such as `0.1.0-rc.1`, including internal dependency metadata and
   the lockfile.
2. Do not merge the temporary version commit into `main` and do not create a documentation
   snapshot for the RC.
3. After approval, create and push the exact prerelease tag. `release.yml` alone creates the
   prerelease GitHub Release and its assets.
4. Dispatch the canary with the exact tag:

   ```sh
   gh workflow run release-canary.yml -f tag=v0.1.0-rc.1
   ```

5. Require the canary to verify the asset inventory, checksums, SBOM, provenance, Sigstore
   evidence, archive execution, `--version`, simulator behavior, loopback server health/UI
   responses, and Linux package installation. macOS and Windows run their matching archive and
   runtime checks; the Windows check remains CLI-only. For prerelease RPMs, the workflow maps
   the SemVer prerelease separator to RPM's ordering syntax (for example,
   `0.1.0-rc.1` becomes the RPM version `0.1.0~rc.1`); archives and executables retain the
   exact `0.1.0-rc.1` version.
6. Retain the evidence before removing the temporary branch. Keep the public RC tag and release
   as the tested reference for the stable release.

Prerelease tags are not stable comparison points. The stable release-content guard ignores them,
so the final `v0.1.0` can promote the exact tested RC content.

## First stable release

The first stable release uses version `0.1.0`. Because there is no earlier stable product tag,
release integrity requires the configured first-release baseline
`2016c5c945d20a42140ec57b6e09f4a49ec98aef` to be an ancestor of the release PR base. The stable
release must contain meaningful product content beyond generated release metadata.

After the RC canary is accepted and explicit activation approval is recorded:

1. Provision a repository-scoped `RELEASE_PLZ_TOKEN` with only the repository contents and pull
   request permissions needed by the guarded workflow. Do not use a registry token.
2. Set `RELEASE_AUTOMATION_ENABLED=true` only after verifying the secret name and workflow
   wiring.
3. Dispatch `release-plz.yml` to create or refresh the one stable release PR.
4. Confirm the PR contains the `0.1.0` release commit, root changelog entry, and exact `0.1.0`
   documentation snapshot. Confirm that no crates.io publication or GitHub Release has happened.
5. Require `Release integrity` as a branch-protection status along with the normal repository
   checks, CLA, and applicable SonarQube zero-issue status.
6. Let the guarded auto-merge request squash merge the PR. The merged release commit creates
   exactly `v0.1.0`; `release.yml` then creates the one stable GitHub Release.

Stable documentation snapshots use exact `X.Y.Z` directories, exclude `docs/internal/**`, and
retain only the newest three stable versions. Before the first stable snapshot exists, the
Docusaurus site remains unversioned and does not show a version selector.

## Recovery and non-negotiable rules

- If more than one release-plz PR appears, stop release automation and inspect the candidates.
  Do not merge or tag until exactly one same-repository PR targeting `main`, authored by
  `mikeboiko`, and using the expected release title remains.
- If a product tag exists but its GitHub Release is missing or incomplete, keep the tag, inspect
  the failed workflow, and rerun the exact tag workflow after the cause is understood. Do not
  create a second tag or a replacement release by hand.
- If the release-rate guard or any GitHub API query cannot establish a safe answer, treat the
  release as blocked. Do not bypass the guard or substitute a best-effort count.
- If artifact or canary evidence fails, do not activate the stable release path. Fix the focused
  issue, retain the failed evidence, and use a newly approved prerelease when the release
  contents change.
- Never provision or print release credentials in local logs, workflow output, source files, or
  documentation. Never add a registry token to the git-only release flow.
- Never create a public RC, enable the kill switch, or cut `v0.1.0` without explicit maintainer
  approval.

For the evidence review after a release or canary completes, use the
[release-verification guide](release-verification.md). For installation, service-state,
database, and operator recovery procedures, use the
[installation guide](../getting-started/installation.md) and
[operator runbook](operator-runbook.md).
