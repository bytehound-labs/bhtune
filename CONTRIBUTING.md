# Contributing to BHTune

BHTune is under active development. The practices below apply to all contributions.

## Contributor License Agreement

Before your first pull request can be merged, you must sign the [Contributor License
Agreement](CLA.md). Please read it in full before signing — it sets out exactly what rights you
grant and what rights you keep.

Signing happens on the pull request itself; there is no paperwork and no account to create. If
you have not signed yet, the **CLA signature check** fails and its summary quotes the exact
one-line statement to post. Add that line as a new comment on your pull request and the check
re-runs automatically. Your signature is recorded on the repository's `cla-signatures` branch and
recognized automatically on every later pull request.

## Development workflow: trunk-based

- `main` is the only long-lived branch and should always be green (builds, passes CI).
- Work happens on short-lived branches named `<type>/<short-description>` (e.g.
  `feat/mrft-state-machine`, `fix/mv-boundary-clamp`), opened as a PR and merged within a day or
  two — not long-running feature branches.
- PRs are squash-merged, so the squash commit message (not the intermediate commits) must follow
  the commit convention below.
- No `develop` branch and no long-lived `release` branches. Releases are tagged directly off
  `main` ([SemVer](https://semver.org/)).
- Incomplete or experimental work that must land before it is fully ready goes behind a Cargo
  feature flag rather than sitting unmerged on a branch.
- Every change goes through a feature branch and pull request, including documentation and
  one-line fixes, so CI runs consistently and concurrent work does not bypass review.
- After opening a pull request, keep repairing the same branch until every applicable required
  check passes. If a check reports that the branch is behind `main`, update the branch before
  merging; do not bypass the protection rule with a direct push.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <description>`.

Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`.
Example: `feat(core): port MRFT hysteresis switch detection`.

## Code style

- Use Rust's `stable` toolchain for local development to match the regular validation, coverage,
  SonarQube, and release jobs. The MSRV job separately checks Rust 1.94.0; on rustup-managed
  hosts, run `rustup update stable` if the selected toolchain is older.
- Format Rust with `cargo fmt --all` (default rustfmt settings) before committing. The optional
  `just fmt` recipe applies the same formatter to the workspace.
- Lint with `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`. Lint
  levels come from the workspace lint policy described under [Lint policy](#lint-policy).
- Format frontend code (`frontend/`) with `pnpm --filter bhtune-frontend run format:check` /
  `pnpm exec prettier --write .`, and lint it with `pnpm --filter bhtune-frontend run lint`
  ([oxlint](https://oxc.rs/)). The documentation site (`website/`) uses the same tools via
  `pnpm --filter bhtune-website run format:check`/`run lint`.
- Python scripts in `scripts/` use Ruff 0.16.9 and the standard-library `unittest` runner.
  Run `ruff check scripts`, `ruff format --check scripts`, and
  `python3 -m unittest discover -s scripts -p '*_test.py'`.
- The [lefthook](https://github.com/evilmartians/lefthook) `pre-commit` hook (`.lefthook.yml`)
  runs source-triggered lint/build checks and formats staged files with `rustfmt`, `taplo`,
  `prettier`, and `shfmt`. Its generated-file exclusions are deliberate. Run `lefthook install`
  once after cloning to enable the hooks; they also keep the `.env.example`/Bitwarden
  synchronization workflow active.
- No proprietary or non-open-source dependencies, ever, on either side of the stack.
  `cargo deny check` enforces this in CI for Rust dependencies against the allow-list in
  `deny.toml`; `pnpm run check:licenses` (`scripts/check-frontend-licenses.mjs`) enforces the
  equivalent allow-list for npm dependencies. If either fails on a new dependency, look for an
  open-source alternative rather than widening the allow-list.
- Run `pnpm run check:dead-code` to use Knip across the root, frontend, and documentation-site
  workspaces. Treat unused files, dependencies, and exports as cleanup candidates; configuration
  exceptions are intentionally narrow and belong in `knip.jsonc`.
- SonarQube Cloud analyzes Rust, TypeScript/TSX, the documentation site, and repository scripts.
  To reproduce its Rust coverage input locally, run
  `cargo llvm-cov --workspace --locked --lcov --output-path lcov.info` followed by
  `sonar-scanner` with `SONAR_TOKEN` exported. The Sonar workflow runs for relevant pull
  requests and pushes to `main`, plus a Wednesday 04:17 UTC weekly scan; fork pull requests
  intentionally skip the secret-bearing analysis.
- Maintainers can restore the local Sonar token with `rbw unlock && ds sync`; it lives in the
  ignored `.env` file and is never committed or printed. Use `ds push` to update the Bitwarden
  note after changing it. The committed `.env.example` contains only the key names and the
  repository's Lefthook hooks keep the schema and local file synchronized.

### Optional local task runner (`just`)

The repository-root `justfile` offers shortcuts for local development. `just` is optional:
GitHub Actions continues to run its workflow commands directly and does not require `just` to be
installed. Frontend recipes assume the pnpm workspace dependencies are installed with
`pnpm install --frozen-lockfile`.

- `just check` is the practical local equivalent of the Linux Rust check job: it checks
  formatting, runs Clippy and workspace tests, runs `cargo deny check` and `cargo machete`,
  checks OpenAPI and CLI-reference drift, and builds the workspace. PR CI additionally checks
  OpenAPI compatibility against the base branch.
- `just fmt` formats all Rust code. `just fmt check` runs the non-mutating
  `cargo fmt --check --all` gate; use `just check` for validation.
- `just lint` and `just test` run the locked workspace lint and test commands. `just deny` runs
  `cargo deny check` and `cargo machete`. `just cov` writes `lcov.info` with the same
  `cargo llvm-cov` command as CI; the coverage workflow also enforces 100% source-line coverage
  and uploads the report.
- `just gen` regenerates `openapi.json`, the CLI reference/man pages/completions/config schema,
  and the TypeScript API client. Review generated diffs and commit output only when a source
  change requires it.
- `just fe` runs the frontend license, generated-client drift, format, lint, unit-test, and
  typecheck/build checks.
- `just e2e` builds the server and frontend, installs Chromium, and runs the Full and Demo
  Playwright projects. Playwright starts its own isolated test servers; a separate production
  server is not required.
- `just dev` runs a local `bhtune-server` and Vite against a temporary database and log
  directory, then removes that temporary state when stopped with Ctrl+C. Ports `8787` and
  `5173` must be free; the recipe refuses to start when either is occupied and never stops an
  existing listener. The server binds `0.0.0.0:8787`; use `http://localhost:5173` by default,
  or set `BHTUNE_ORIGIN` to the exact Vite origin when using another host name.

Lefthook remains independent of `just` and keeps its file-scoped behavior: `rustfmt-format`
corresponds to `just fmt` but formats only staged Rust files. `clippy-lint` uses the same
workspace Clippy checks as `just lint` without `--locked`; `oxlint-frontend` and
`frontend-typecheck` correspond to checks in `just fe`. For frontend files,
`prettier-format` corresponds to `just fe`'s format check, while the hook formats only staged,
non-generated files. `taplo-format`, `shfmt-format`, `check-release`, and `ds-sync` remain
hook-specific. Pre-commit does not run the workspace test suite or invoke `just check`; it
retains its staged-file inputs and generated-file exclusions.

### Lint policy

The root `Cargo.toml` defines one workspace-wide lint policy in `[workspace.lints]`. Every member
crate opts in with `[lints] workspace = true`, so libraries, binaries, tests, examples, and build
scripts share the same baseline. The standalone `fuzz/` package is a separate Cargo workspace and
is not covered. Code behind `#[cfg(windows)]` is linted by the Windows CI job.

- `deny` marks constructs that must never land and is an error even without `-D warnings`:
  `unsafe_code`, which every `rustc` build checks, and these lints, which `cargo clippy`
  checks: leftover `dbg!`, `todo!`, and `unimplemented!` macros, undocumented `unsafe` blocks,
  every default Clippy group (`clippy::all`), and the panic restriction lints
  `clippy::unwrap_used`, `clippy::expect_used`, and `clippy::panic`.
- Production code returns a typed error instead of panicking. That includes code that runs
  after a live loop has been switched to manual, where a panic would skip restore. Unit tests
  allow those three lints with a crate-root `#![cfg_attr(test, allow(...))]`. Integration
  tests, examples, and benches allow them with a file-level `#![allow(...)]`, because a
  crate-root test allow does not cover those targets. Four production `expect` calls remain.
  Each is a real invariant with a site-level `#[allow(clippy::expect_used, reason = "...")]`:
  request-snapshot serialization before any driver I/O, the embedded template catalog parse,
  a non-empty browse-page vector, and an integer `Retry-After` header value.
- `warn` marks hygiene findings: `rust_2018_idioms`, the redundant/unused lifetime and unused
  macro-rule lints, and a curated subset of `clippy::pedantic` whose lints are cheap and
  mechanical to satisfy. Warnings stay non-fatal in a local build but fail CI, whose Clippy job
  runs with `-D warnings`.
- The rest of `clippy::pedantic` stays off because its noisiest lints are subjective or would add
  hundreds of low-value annotations. `clippy::nursery` (unstable, prone to false positives) and
  `clippy::cargo` (duplicate transitive versions are not fixable here, and `cargo deny` owns
  dependency policy) stay off as well.
- Fix a finding rather than suppressing it. When a suppression is genuinely correct, put a
  narrowly scoped `#[expect(lint, reason = "...")]` on the smallest item that needs it. Prefer
  `#[expect]` over `#[allow]`: once the finding disappears it raises an
  `unfulfilled_lint_expectations` warning, which fails CI, so a stale suppression cannot
  linger. The panic-lint allows above are the exception, because those lints are intentionally
  permitted in tests and on the four invariant sites. Other crate-level blanket allows are not
  accepted.
- Keep `unsafe` out of production code. When a test genuinely needs it (for example, delivering
  a real OS signal with `libc::kill`), isolate it in a small function carrying
  `#[expect(unsafe_code, reason = "...")]` and put a `// SAFETY:` comment inside the function
  body, directly above the `unsafe` block. `clippy::undocumented_unsafe_blocks` does not
  recognize a comment placed above the function signature or its attributes, including a doc
  comment.
- To propose another lint, measure its findings with
  `cargo clippy --workspace --all-targets --all-features -- -W clippy::<lint>` and fix them in
  the same change that enables it.

## Dependency updates

Dependabot checks the Cargo workspace, pnpm workspace, and GitHub Actions weekly, grouping routine
patch and minor updates within each ecosystem. Major updates are intentionally left as focused,
individual PRs. Routine updates should use the grouped PRs and update the relevant manifest and
lockfile together; use the latest compatible version rather than blindly accepting a major release
that breaks the current toolchain or code-generation stack.

Major updates should be handled as focused PRs so compatibility work is easy to review. Cargo
updates must preserve the declared `rust-version` unless the project intentionally raises its
MSRV, and pnpm updates must keep the frontend and documentation-site toolchains compatible
(notably TypeScript and `openapi-typescript`). Record any necessary compatibility pin in the
nearest contributor-facing documentation and revisit it when the blocking dependency supports
the newer major.

Before merging an update, run the full ecosystem gates: Rust formatting, Clippy, workspace
tests, the declared MSRV check, `cargo deny check`, and `cargo machete`; frontend and website
format/lint/typecheck/build checks; OpenAPI and generated-reference drift checks; the Playwright
suite; and npm license validation. A periodic intentional sweep of all direct dependencies is
appropriate for a release or maintenance cycle, but it should still follow these compatibility
and validation rules rather than treating "latest" as an unconditional upgrade policy.

## Testing

- Unit-test domain logic with `cargo test --workspace`.
- Frontend helper and component unit tests use Vitest with jsdom:
  `pnpm --filter bhtune-frontend run test`.
- `bhtune-core` (the MRFT engine and tuning math) must stay a pure, I/O-free state machine so it
  can be tested deterministically and validated by replaying golden-master traces. See
  `AGENTS.md` for the replay-validation approach and the correctness-critical details that need
  direct unit-test coverage.
- Coverage is tracked by Codecov and enforced at exactly 100% (`codecov.yml`). The coverage
  workflow independently checks every canonical LCOV `DA` source-line record, rejects any
  zero-hit line, and normalizes the report's aggregate `LF`/`LH` summary before uploading it.
  Add tests for new code — including error branches and edge cases — in the same PR.
- End-to-end browser tests live in `frontend/e2e/` (Playwright), driving a real
  `bhtune-server` running the simulator driver through the actual built UI — no mocked HTTP
  layer for the real suites. The mocked Demo contract suite runs in the same Demo project.
  Run the Full and Demo projects independently with:

  ```sh
  pnpm --filter bhtune-frontend run build   # builds frontend/dist/
  cargo build -p bhtune-server              # debug build serves dist/ live off disk
  npx --prefix frontend playwright install chromium   # first run only
  PLAYWRIGHT_MODE=full pnpm --filter bhtune-frontend exec playwright test --project=full
  PLAYWRIGHT_MODE=demo pnpm --filter bhtune-frontend exec playwright test --project=demo
  ```

  The Demo project also requires `openssl` for its isolated loopback HTTPS certificate. Running
  `pnpm --filter bhtune-frontend run test:e2e` without `PLAYWRIGHT_MODE` runs both projects and
  starts both isolated test servers.

## CI

Rust PRs must pass `cargo fmt --check --all`, `cargo clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo
machete`, and a check that the generated OpenAPI spec (`openapi.json`) and CLI reference
(`docs/reference/cli.md`, `man/`, `completions/`) are up to date before merge — run `cargo
run -p bhtune-server --example gen_openapi` and `cargo run -p bhtune-cli --example gen_docs
--features schemars` and commit the result after changing an HTTP route/DTO or a `clap`
argument, respectively. The package job runs `cargo package --workspace --locked
--no-verify` to validate that every release archive can be assembled. Tarball verification
is deliberately skipped there because Cargo removes local paths and resolves same-version
workspace dependencies from crates.io, which cannot compile coordinated unpublished API
changes; the workspace build, Clippy, and test jobs compile the real local dependency graph.
PRs touching `frontend/` must additionally pass `pnpm run
check:licenses`, a check that the generated OpenAPI TS client (`frontend/src/api/schema.d.ts`)
is up to date, `pnpm --filter bhtune-frontend run test`, `run format:check`, `run lint`, and
`run build` (which also typechecks `frontend/e2e/`). `.github/workflows/e2e.yml` runs the Playwright
suite above in CI on every push/PR, uploading the HTML report as an artifact if it fails. PRs
touching `docs/` or `website/` must pass `pnpm --filter bhtune-website run format:check`,
`run lint`, `run typecheck`, and `run build` — the build step doubles as a broken-link/anchor
check across `docs/`, since Docusaurus fails the build rather than shipping a dead link or
a heading reference that no longer exists. Knip runs as the required `Knip dead-code analysis`
status for changes affecting the pnpm workspaces, their dependency metadata, or its configuration.
SonarQube runs as a separate `Required Sonar quality status` check for relevant changes; its
quality-gate result is blocking when analysis runs, while documentation-only and fork pull
requests receive an explicit successful skip status. Applicable PR analyses must also report zero
`OPEN`/`CONFIRMED` issues; Accepted and False Positive findings require a documented rationale
and a link to the related pull request or documentation.

### Packaging validation

Packaging changes must keep generated artifacts and disposable package workspaces outside the
repository. The AUR generator is metadata-only: it does not compile Rust or frontend code,
contact AUR, or publish anything. Run its network-free regression suite and shell checks before
opening a packaging pull request:

```sh
python3 scripts/aurpkg_test.py
shellcheck scripts/aurpkg
shfmt -d scripts/aurpkg
```

Use a disposable Arch environment and a non-root build user for `makepkg --verifysource`,
package installation, upgrade, removal, and service-state checks. Generate `.SRCINFO` with
`makepkg --printsrcinfo`; never hand-edit it. Publication is intentionally separate from pull
request validation, requires an exact stable `vX.Y.Z` tag and independently verified release
evidence, and remains a manual first-release action until the post-release AUR commit has been
verified. The reusable Arch job feeds its validation script to `docker run -i` and writes
machine-readable evidence through the host-mounted `aur-evidence/` directory; preserve both
invariants when changing that workflow. The regression suite must run with the repository root
as its working directory because its ancillary-file inventory intentionally uses repository-
relative paths.
Package-content assertions use archive-relative paths and accept either `bsdtar`
leading-path convention (`usr/...` or `./usr/...`).

Debian packages use adaptive `depends = "$auto"` metadata and therefore require
`dpkg-shlibdeps` in the packaging environment. Do not hard-code a dependency list to compensate
for an incomplete local package build. Keep `.github/workflows/release.yml` unchanged when
working on reusable installer or AUR validation/publication workflows; release integration is a
separate coordination task.

## Security and compatibility checks

Report suspected vulnerabilities privately as described in [`SECURITY.md`](SECURITY.md), never
in public issues, discussions, or pull requests.

Security workflows run CodeQL, Semgrep, full-history Gitleaks, actionlint, and zizmor. Keep
workflow permissions least-privilege and do not replace `pull_request` with
`pull_request_target` to obtain secrets for forked contributions.
All referenced GitHub Actions are pinned to immutable commit SHAs; Dependabot updates those
pins through the configured `github-actions` ecosystem.

Parser changes should include both a focused property test and, where the input boundary is
externally reachable, a `fuzz/` target. OpenAPI changes must regenerate `openapi.json` and
pass the breaking-change comparison against the pull request base. Database changes must
include a representative upgrade test when they alter an existing schema.

Release tags publish checksums, a CycloneDX SBOM, Sigstore blob-signature bundles, and GitHub
artifact provenance. Do not add release artifacts that bypass those steps.

## Documentation

A documentation update is part of a PR's definition of done whenever it changes user-visible
behavior — a new CLI flag, config key, HTTP endpoint, default value, or safety rule. Update
whichever of `README.md`, `AGENTS.md`, and `docs/` describes the area you're changing; see
"Documentation contract" in `AGENTS.md` for the full policy. If you use Copilot CLI against
this repo, `.github/hooks/docs-drift.json` prints a one-line reminder at the end of a session
that changed `crates/**` or user-visible `frontend/src/**` without touching any documentation
surface — a safety net, not a substitute for doing this deliberately.

### Web UI screenshots

The browser documentation uses deterministic Playwright screenshots generated from the real
Full and Demo SPA. Screenshot PNGs are generated assets and must never be committed to Git.
The text-only lock at `docs/reference/web-ui-screenshots.json` records scenario coverage,
dimensions, hashes, and Pages URLs.

Run the capture and lock update after a UI change:

```sh
pnpm docs:screenshots
pnpm docs:screenshots:validate
pnpm docs:screenshots:gallery
```

`docs:screenshots` runs the Full and Demo capture suites serially, updates the text lock, and
creates the local review gallery at `frontend/test-results/docs-screenshots/index.html`.
`docs:screenshots:check` regenerates candidates without changing the lock and fails when the
canonical screenshots drift. The generated Pages directory is ignored locally; the Pages
deployment workflow recreates it from the merged commit.

The validator also compares every static `documentationId`/`data-doc-section` marker in
`frontend/src/` with the manifest, so a newly marked UI section fails validation until it has
an associated screenshot scenario.

Add a new scenario when a new route, major section, modal, or safety-relevant state is not
clearly represented by an existing capture. Give the image meaningful alt text and a caption,
keep all operational instructions in prose, and link the screenshot to its full-size Pages URL.
The viewport stays fixed for deterministic layout; use `capture: "content-fit"` for short pages
whose rendered content does not fill the viewport. Its height is computed automatically from the
shared content root, so do not add per-page screenshot heights.

## Pull requests

- Keep PRs small and focused — one logical change each.
- Describe what changed and why; link an issue if one exists.
- Include the targeted validation performed and any manual verification needed for the change.
- After every applicable required check, the CLA check, and the applicable SonarQube zero-issue
  check pass, queue the built-in GitHub squash auto-merge with
  `gh pr merge <PR> --auto --squash --delete-branch` and confirm that the PR reaches `MERGED`.
  Do not use `NOSONAR` or a dashboard status change to hide a real unresolved code issue.

## Contributing a DCS/PLC template

Adding support for a control system BHTune doesn't already know about is a data file
change, not a Rust change — no code, no rebuild logic, just a `[[template]]` block in
[`crates/bhtune-core/templates/builtin.toml`](crates/bhtune-core/templates/builtin.toml).
This is one of the easiest ways to contribute, and it's one we'd especially like to see:
the goal is a community-maintained library covering as many DCS/PLC systems as possible.

See [`docs/dcs-templates.md`](docs/dcs-templates.md) for the full field-by-field reference
and a worked example. In short:

- Copy the closest existing `[[template]]` block as a starting point, or generate one from
  a template you've already built with `bhtune template export <name> out.toml --format toml`.
- Fill in every tag suffix, the raw mode values, and a `versions` list naming the
  release(s) you're targeting, in that vendor's own version-naming convention.
- If a newer release of a vendor you've already contributed changes its tag conventions,
  add a **new** `[[template]]` entry with its own `name` — never edit an existing entry's
  suffixes in place, since sites on the older release still depend on the mapping as
  written.
- A unit test parses and validates the entire embedded catalog on every CI run, so a
  malformed or incomplete contribution fails the build rather than merging silently broken.

## Releases and changelogs

Use [Conventional Commits](https://www.conventionalcommits.org/) for every commit. Release-worthy
types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, and `revert`;
release-preparation commits and generic `chore` commits do not create product release content.
Scopes are optional, but a specific scope such as `core`, `driver`, `db`, `cli`, or `server` makes
the root changelog easier to review.

The root [`CHANGELOG.md`](CHANGELOG.md) is the single product changelog for all five workspace
crates. Contributors should describe user-visible changes in commits and update the relevant
documentation, but should not hand-edit release version entries or workspace version numbers.
`release-plz` generates the release preparation commit and owns the versioned changelog entry
when the release process is activated.

Releases use a single product tag cut from `main`; there are no release branches. `bhtune-cli`
owns the product tag and release version, while `.github/workflows/release.yml` alone creates the
GitHub Release and uploads artifacts. The current policy is git-only: BHTune crates are not
published to crates.io. Release automation is guarded by the
`RELEASE_AUTOMATION_ENABLED` repository variable and remains disabled until an approved RC has
passed the hosted canary and the maintainer activation gates are complete.

Maintainers must follow the [release automation guide](docs/guides/releasing.md), including the
first-release baseline, documentation-snapshot rules, rate-limit guard, and partial-failure
recovery procedure.

## License

By contributing, you agree your contributions are licensed under the project's [AGPL-3.0-or-later
license](LICENSE), subject to the terms of the [CLA](CLA.md) you sign.
