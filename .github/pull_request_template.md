## What does this PR do?

<!-- Briefly describe the change and why it's needed. Link an issue if one exists. -->

## Checklist

- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] `cargo deny check` passes (no proprietary/non-open-source dependencies — see `deny.toml`)
- [ ] Frontend changes: `pnpm --filter bhtune-frontend run format:check` / `run lint` / `run typecheck` pass
- [ ] Documentation (`README.md`, `docs/`, `AGENTS.md`) updated if this changes user-visible behavior
- [ ] I have signed the [Contributor License Agreement](../CLA.md) (the CLA-assistant bot will
      prompt on this PR if this is your first contribution)
