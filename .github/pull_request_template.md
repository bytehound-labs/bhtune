## What does this PR do?

<!-- Briefly describe the change and why it's needed. Link an issue if one exists. -->

## Validation

<!-- List targeted checks and any manual verification performed. -->

## Checklist

- [ ] This change is on a feature branch and will be squash-merged through a pull request
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo deny check` passes (no proprietary/non-open-source dependencies — see `deny.toml`)
- [ ] Frontend changes: `pnpm --filter bhtune-frontend run format:check` / `run lint` / `run typecheck` pass
- [ ] Documentation (`README.md`, `docs/`, `AGENTS.md`) updated if this changes user-visible behavior
- [ ] Applicable SonarQube analysis reports zero `OPEN`/`CONFIRMED` issues, or any remaining
      Accepted/False Positive finding has a documented rationale and related link
- [ ] I have signed the [Contributor License Agreement](../CLA.md) (the CLA-assistant bot will
      prompt on this PR if this is your first contribution)
