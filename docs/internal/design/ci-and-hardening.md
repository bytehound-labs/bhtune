# CI and repository hardening

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## SonarQube Cloud analysis

SonarQube Cloud analysis is configured for both BHTune and `opcda-bridge`. BHTune's
`sonar-project.properties` indexes the Rust, frontend, documentation-site, and repository-script
sources, imports the Rust LCOV report from `cargo llvm-cov`, and excludes generated/build/test/
fuzz/documentation artifacts plus frontend and website coverage until JavaScript LCOV generation
exists. Each repository's dedicated `SonarQube` workflow runs on relevant pull requests and pushes
to `main`, supports manual dispatch, and performs a full weekly scan; its required aggregate status
passes intentional documentation-only skips and fork pull-request skips while failing when an
applicable analysis or the Sonar quality gate fails. The two projects use separate Sonar
configurations because `opcda-bridge` is Rust-only.

## Knip dead-code analysis (`knip`)

Knip runs full analysis across the root pnpm workspace, `frontend/`, and `website/` through the
root `check:dead-code` script. It checks unused files, dependencies, exports, duplicate exports,
unresolved imports, and unlisted dependencies rather than limiting the scan to production code.

`knip.jsonc` keeps the configuration narrow: the Playwright server helper is an explicit entry,
generated OpenAPI declarations have their unused generated types ignored, and the Docusaurus
search theme plus root Prettier have documented exceptions for dynamic resolution and lefthook's
runtime invocation. Genuine findings are fixed in the manifests or source instead of being
hidden behind broad issue suppression.

The `checks.yml` workflow runs Knip on every relevant pull request and push to `main`, plus
manual workflow dispatch. Change detection covers the pnpm manifests, lockfile, workspace
configuration, Knip configuration, frontend, website, scripts, lefthook configuration, and
workflow files. The job is required through the existing `Required validation status` aggregate;
no new branch-protection context is needed. Knip applies to BHTune's JavaScript/TypeScript
workspaces and is not applicable to the Rust-only `opcda-bridge` repository.

## Cross-project CI/CD audit (`cross-project-ci-audit`)

Compared `bhtune`'s CI/CD, lefthook, and repo-hygiene setup against the sibling
`opcda-bridge` project in both directions. The flow was overwhelmingly one-directional
(`opcda-bridge` → `bhtune`): `opcda-bridge` is the more mature project and already embodied
the practices below before this audit — `homepage`, `cargo machete`, `cargo deny`, and its
own `windows`/`msrv`/`package` CI jobs all predate this audit on that side. Checked
specifically for anything worth proposing back the other way and found nothing of
substance beyond what's noted below; both repos came out of this audit with equivalent
Dependabot/security posture instead.

Pulled into `bhtune` from `opcda-bridge`'s example:

- **MSRV declared and enforced.** `rust-version = "1.94"` in `[workspace.package]`
  (empirically determined — `sqlx@0.9.0` requires it, higher than `opcda-bridge`'s own 1.88
  floor), with a standalone `msrv` CI job pinning `dtolnay/rust-toolchain@1.94.0` and running
  `cargo check --workspace --all-targets --all-features --locked`.
- **`windows` and `package` CI jobs.** `windows` runs fmt/clippy/test on `windows-latest`
  (skipping the Linux-only doc/OpenAPI drift `git diff` checks, which are CRLF-sensitive).
  `package` runs `cargo package --workspace --locked --no-verify`: this still validates
  metadata, path/version requirements, and assembly of every publishable tarball, while the
  ordinary workspace check/Clippy/test jobs compile the real local cross-crate API graph.
  `--no-verify` is load-bearing for this same-version, not-yet-published workspace: Cargo's
  tarball verification strips local paths and resolves dependencies such as `bhtune-db
0.1.0` from crates.io, so a coordinated local API addition otherwise compiles a dependent
  crate against the older published `0.1.0` instead of the package assembled moments earlier.
  The original package job still surfaced a real bug before this distinction mattered —
  every workspace-internal path dependency lacked a `version` requirement, which `cargo
package` refuses even to assemble. Giving `bhtune-core`/`bhtune-driver`/`bhtune-db`/
`bhtune` `{ path, version }` entries in `[workspace.dependencies]` and switching every
  consumer to `.workspace = true` remains required.
- **`concurrency` groups, `permissions: contents: read`, and `--locked` everywhere** across
  `checks.yml`/`coverage.yml`/`e2e.yml`.
- **`.github/dependabot.yml`** — weekly grouped updates for `cargo`, `npm` (pnpm workspace
  root, covering both root and `frontend/package.json`), and `github-actions`, each labeled
  (`dependencies`/`rust`/`frontend`/`ci` — created on the repo, since Dependabot silently
  skips labels that don't already exist).
- **Branch protection on `main`** — both repositories require pull requests and protected
  status checks, with `strict: true`, `enforce_admins: true` (direct pushes, including
  administrator pushes, are blocked), and `allow_force_pushes`/`allow_deletions: false`.
  Repository merge settings allow squash merges only; merge commits and rebase merges are
  disabled. `bhtune` requires `Required validation status`, `Required coverage status`,
  `Required E2E status`, and `Required Sonar quality status`; `opcda-bridge` requires
  `check`, `coverage`, `release-integrity`, and `Required Sonar quality status`.
- **Secret scanning + push protection enabled** on both `bytehound-labs/bhtune` and
  `bytehound-labs/opcda-bridge` (both public repositories) — confirmed disabled on both
  before this audit. `secret_scanning_validity_checks` did not take via the API on either
  repo despite repeated attempts (`secret_scanning`/`secret_scanning_push_protection` both
  enabled fine) — likely an org/plan-gated setting; low priority, flip manually in the repo
  Settings UI if wanted.

Ported from `bhtune` to `opcda-bridge` (the one item that went the other way, discovered
while auditing rather than pre-existing on either side): **`.github/dependabot.yml`** for
`cargo` + `github-actions` (no `npm` — pure Rust workspace, no frontend), with matching
`dependencies`/`rust`/`ci` labels created using the same colors as `bhtune`'s.

The **CLA-enforcement bot** was a separate pre-existing gap, outside this audit's CI/CD scope; it
is now implemented — see "Contributor License Agreement (`cla-tooling`, done)" above.

**Follow-up, implemented later:** **CODEOWNERS, issue templates, and a PR template** — a
shared gap on both repos, not something to port one way — were added to both
(`.github/CODEOWNERS`; `.github/ISSUE_TEMPLATE/{bug_report,feature_request,config}.yml`;
`.github/pull_request_template.md`), each adapted to its own project's conventions rather
than copy-pasted: `bhtune`'s PR template checklist includes the frontend lint/typecheck
commands and the CLA-sign-off line from `CONTRIBUTING.md`; `opcda-bridge`'s omits both (no
frontend, no CLA — MIT, no CLA required) and instead asks for hardware-in-the-loop manual
verification notes, matching its own `CONTRIBUTING.md`'s "no live OPC DA server in CI" line.
Both bug report forms ask for a version and platform; `bhtune`'s adds a `Driver` dropdown
(OPC DA/simulator/replay) since that's a core `bhtune-driver` concept a maintainer would
otherwise have to ask about, and `opcda-bridge`'s adds an OPC DA server vendor field instead,
plus a note that the gateway crate is Windows-only. Neither repo has GitHub Discussions
enabled (confirmed via `gh api repos/.../{repo}` before writing `config.yml`), so
`blank_issues_enabled: true` with no `contact_links` was the right shape for both — forcing
every report into a rigid form when there's nowhere else to ask would be worse than a
free-form issue.

## Workflow and release hardening

The repository now has a layered hardening gate for both source changes and release outputs:

- **Security analysis.** `.github/workflows/codeql.yml` builds the Rust and JavaScript/
  TypeScript targets for CodeQL; `semgrep.yml` runs the Rust and TypeScript community rules;
  `gitleaks.yml` scans the complete git history on every relevant change and weekly; and
  `security-lint.yml` runs actionlint plus zizmor against every workflow change. These use
  ordinary `pull_request` events, `persist-credentials: false` wherever checkout does not need
  to push, least-privilege permissions, immutable action commit pins, and explicit job
  timeouts. The docs agent has two narrow, documented zizmor exceptions: its authenticated
  checkout must retain credentials to push its reviewed prose commit, and its isolated,
  version-pinned Copilot CLI install cannot use a repository lockfile.
- **Change-aware CI.** `checks.yml`, `coverage.yml`, and `e2e.yml` use
  `dorny/paths-filter` to skip unrelated work and finish with an always-running aggregator
  status, so branch protection still receives one deterministic result when a fan-out job is
  intentionally skipped. Every major workflow job has a `timeout-minutes` bound.
- **Release integrity.** Docker builds publish provenance and SBOM attestations. Tagged
  releases download their archives/packages into a dedicated supply-chain job, generate a
  CycloneDX SBOM and checksums, create GitHub artifact provenance attestations, and sign every
  release asset with keyless Cosign bundles before uploading the evidence beside the assets.
- **Parser resilience.** `proptest` tests cover config serialization/parsing, template TOML/
  JSON, OPC bridge payload mappings, and template imports. The separate `fuzz/` Cargo-fuzz
  package has targets for each of those byte-stream boundaries without becoming a workspace
  runtime dependency.
- **API compatibility.** `scripts/check_openapi_breaking.py` is a dependency-free comparison
  for removed operations/responses/properties/enum values, newly required request fields, and
  newly mandatory authentication. Its unit tests run in CI, and pull requests compare the
  generated revision to the base branch in addition to the existing drift check. The only
  response nullability allowance is the exact pre-v1 checked-result migration for the six
  numeric fields of `ResultResponse`, because invalid calculations persist no safe numeric
  value; unrelated response changes remain breaking. The request-property removals allowed by
  the comparator are exact pre-v1 migrations of the per-tune quality/timing settings into global
  configuration; unrelated removals remain breaking.
- **Database compatibility.** Before v0.1, the migrations directory contains one consolidated
  `0001_initial_schema.sql` describing the complete current schema. Fresh-schema tests verify
  the single recorded migration plus the final indexes, checked-result constraints, Demo
  ownership triggers, and MV actuation audit table. While no supported external database
  depends on the pre-release history, another squash is allowed; local and test databases are
  disposable and may need recreation. Once v0.1 ships or a database is distributed or
  supported outside development, applied migration history becomes a compatibility contract and
  future schema changes must use new forward migrations rather than editing `0001`.
