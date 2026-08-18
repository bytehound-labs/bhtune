# Contributing to BHTune

BHTune is under active development. The practices below apply to all contributions.

## Contributor License Agreement

Before your first pull request can be merged, you must sign the [Contributor License
Agreement](CLA.md). A CLA-assistant bot will comment on your PR with instructions the first time
you contribute. The CLA documents the rights needed to accept and maintain contributions — see
the CLA itself for exactly what rights you are and are not granting.

## Development workflow: trunk-based

- `main` is the only long-lived branch and should always be green (builds, passes CI).
- Work happens on short-lived branches named `<type>/<short-description>` (e.g.
  `feat/mrft-state-machine`, `fix/mv-boundary-clamp`), opened as a PR and merged within a day or
  two — not long-running feature branches.
- PRs are squash-merged, so the squash commit message (not the intermediate commits) must follow
  the commit convention below.
- No `develop` branch and no long-lived `release` branches. Releases are tagged directly off
  `main` ([SemVer](https://semver.org/)).
- Incomplete or experimental work that must land before it's fully ready goes behind a Cargo
  feature flag rather than sitting unmerged on a branch.
- Trivial fixes (typos, doc tweaks) may be pushed directly to `main`; everything else goes
  through a PR so CI runs.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <description>`.

Common types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`.
Example: `feat(core): port MRFT hysteresis switch detection`.

## Code style

- Format Rust with `cargo fmt --all` (default rustfmt settings) before committing.
- Lint with `cargo clippy --workspace --all-targets --all-features -- -D warnings`; fix every
  warning or justify an explicit `#[allow(...)]` with a comment.
- Format frontend code (`frontend/`) with `pnpm --filter bhtune-frontend run format:check` /
  `pnpm exec prettier --write .`, and lint it with `pnpm --filter bhtune-frontend run lint`
  ([oxlint](https://oxc.rs/)). The documentation site (`website/`) uses the same tools via
  `pnpm --filter bhtune-website run format:check`/`run lint`.
- All of the above are enforced automatically by a
  [lefthook](https://github.com/evilmartians/lefthook) `pre-commit` hook (`.lefthook.yml`),
  which also formats `Cargo.toml`/TOML with `taplo` and Markdown/YAML/JSON/TypeScript/CSS with
  `prettier`. Run `lefthook install` once after cloning to enable it.
- No proprietary or non-open-source dependencies, ever, on either side of the stack.
  `cargo deny check` enforces this in CI for Rust dependencies against the allow-list in
  `deny.toml`; `pnpm run check:licenses` (`scripts/check-frontend-licenses.mjs`) enforces the
  equivalent allow-list for npm dependencies. If either fails on a new dependency, look for an
  open-source alternative rather than widening the allow-list.

## Testing

- Unit-test domain logic with `cargo test --workspace`.
- `bhtune-core` (the MRFT engine and tuning math) must stay a pure, I/O-free state machine so it
  can be tested deterministically and validated by replaying golden-master traces. See
  `AGENTS.md` for the replay-validation approach and the correctness-critical details that need
  direct unit-test coverage.
- Coverage is tracked by Codecov and enforced at 100% (`codecov.yml`). Add tests for new code —
  including error branches and edge cases — in the same PR.
- End-to-end browser tests live in `frontend/e2e/` (Playwright), driving a real
  `bhtune-server` running the simulator driver through the actual built UI — no mocked HTTP
  layer. Run locally with:

  ```sh
  pnpm --filter bhtune-frontend run build   # builds frontend/dist/
  cargo build -p bhtune-server              # debug build serves dist/ live off disk
  npx --prefix frontend playwright install chromium   # first run only
  pnpm --filter bhtune-frontend run test:e2e
  ```

## CI

Rust PRs must pass `cargo fmt --check --all`, `cargo clippy --workspace --all-targets
--all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo
machete`, and a check that the generated OpenAPI spec (`openapi.json`) and CLI reference
(`docs/reference/cli.md`, `man/`, `completions/`) are up to date before merge — run `cargo
run -p bhtune-server --example gen_openapi` and `cargo run -p bhtune-cli --example gen_docs
--features schemars` and commit the result after changing an HTTP route/DTO or a `clap`
argument, respectively. PRs touching `frontend/` must additionally pass `pnpm run
check:licenses`, a check that the generated OpenAPI TS client (`frontend/src/api/schema.d.ts`)
is up to date, `pnpm --filter bhtune-frontend run format:check`, `run lint`, and `run build`
(which also typechecks `frontend/e2e/`). `.github/workflows/e2e.yml` runs the Playwright
suite above in CI on every push/PR, uploading the HTML report as an artifact if it fails. PRs
touching `docs/` or `website/` must pass `pnpm --filter bhtune-website run format:check`,
`run lint`, `run typecheck`, and `run build` — the build step doubles as a broken-link/anchor
check across `docs/`, since Docusaurus fails the build rather than shipping a dead link or
a heading reference that no longer exists.

## Documentation

A documentation update is part of a PR's definition of done whenever it changes user-visible
behavior — a new CLI flag, config key, HTTP endpoint, default value, or safety rule. Update
whichever of `README.md`, `AGENTS.md`, and `docs/` describes the area you're changing; see
"Documentation contract" in `AGENTS.md` for the full policy. If you use Copilot CLI against
this repo, `.github/hooks/docs-drift.json` prints a one-line reminder at the end of a session
that changed `crates/**` without touching any documentation surface — a safety net, not a
substitute for doing this deliberately.

## Pull requests

- Keep PRs small and focused — one logical change each.
- Describe what changed and why; link an issue if one exists.
- Squash-merge once CI is green and the CLA check passes.

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

## Releases

SemVer tags cut directly from `main`. No release branches. release-plz tracks per-crate versions
and changelogs in-repo; see `release-plz.toml`.

## License

By contributing, you agree your contributions are licensed under the project's [AGPL-3.0-or-later
license](LICENSE), subject to the terms of the [CLA](CLA.md) you sign.
