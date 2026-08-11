# Contributing to BHTune

BHTune is under active development. The practices below apply to all contributions.

## Contributor License Agreement

Before your first pull request can be merged, you must sign the [Contributor License
Agreement](CLA.md). A CLA-assistant bot will comment on your PR with instructions the first time
you contribute. This is what lets ByteHound keep BHTune under the AGPL for everyone while also
offering separate commercial licensing terms to enterprise customers — see the CLA itself for
exactly what rights you are and are not granting.

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

- Format with `cargo fmt --all` (default rustfmt settings) before committing.
- Lint with `cargo clippy --workspace --all-targets --all-features -- -D warnings`; fix every
  warning or justify an explicit `#[allow(...)]` with a comment.
- Both are enforced automatically by a [lefthook](https://github.com/evilmartians/lefthook)
  `pre-commit` hook (`.lefthook.yml`), which also formats `Cargo.toml`/TOML with `taplo` and
  Markdown/YAML/JSON with `prettier`. Run `lefthook install` once after cloning to enable it.
- No proprietary or non-FOSS dependencies, ever. `cargo deny check` enforces this in CI against
  the allow-list in `deny.toml`; if it fails on a new dependency, look for a FOSS alternative
  rather than widening the allow-list.

## Testing

- Unit-test domain logic with `cargo test --workspace`.
- `bhtune-core` (the MRFT engine and tuning math) must stay a pure, I/O-free state machine so it
  can be tested deterministically and validated by replaying golden-master traces. See
  `AGENTS.md` for the replay-validation approach and the correctness-critical details that need
  direct unit-test coverage.
- Coverage is tracked by Codecov and enforced at 100% (`codecov.yml`). Add tests for new code —
  including error branches and edge cases — in the same PR.

## CI

PRs must pass `cargo fmt --check --all`, `cargo clippy --workspace --all-targets --all-features
-- -D warnings`, `cargo test --workspace`, `cargo deny check`, and `cargo machete` before merge.

## Pull requests

- Keep PRs small and focused — one logical change each.
- Describe what changed and why; link an issue if one exists.
- Squash-merge once CI is green and the CLA check passes.

## Releases

SemVer tags cut directly from `main`. No release branches. release-plz tracks per-crate versions
and changelogs in-repo; see `release-plz.toml`.

## License

By contributing, you agree your contributions are licensed under the project's [AGPL-3.0-or-later
license](LICENSE), subject to the terms of the [CLA](CLA.md) you sign.
