# Contributing to BHTune

Keep contributions focused, behavior-preserving unless a change is intentional, and covered by the relevant checks and documentation.

## Contributor License Agreement

Read and sign the [CLA](CLA.md) before your first merge. The **CLA signature check** on your PR supplies the exact one-line comment to post; commenting re-runs it. Signatures are recorded on `cla-signatures` and recognized on later PRs.

The PR check is read-only. Signing runs the default-branch workflow without checking out
contributor PR code and writes only the separate signature branch; the agreement itself owns
the legal terms.

## Development workflow: trunk-based

- `main` is the only long-lived branch and must stay green. Work on short-lived `<type>/<description>` branches, including docs and one-line fixes; no `develop` or release branches.
- Open a focused PR and repair that same branch until all applicable checks pass. Keep it current with `main`; never bypass protection with direct/force pushes or admin merges.
- Squash-merge only after maintainer review, any required manual testing, and explicit merge approval. Passing checks alone do not authorize merging.
- Incomplete work that must land uses a Cargo feature flag, not a long-lived experimental branch.

## Workspace architecture

The [crate graph](docs/architecture/crate-graph.md) and [agent contract](AGENTS.md#crate-map) describe the boundaries. `bhtune-runtime` owns shared configuration/bootstrap, logging, drivers, tune/safety/recovery, history operations, and exports. The CLI and HTTP server are peer adapters: keep terminal/Clap and Axum/HTTP/OpenAPI concerns out of runtime source, direct dependencies, and APIs, and never make the server depend on CLI application services. Transitive gRPC transport dependencies are not permission to expose transport types in runtime APIs.

## Commit messages

Use [Conventional Commits](https://www.conventionalcommits.org/): `<type>(<scope>): <description>`, with optional scope. Example: `feat(core): port MRFT hysteresis switch detection`.

Release-worthy types are `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, and `revert`; release preparation and generic `chore` commits are not product content. The reviewed squash message is authoritative. Do not add a Copilot co-author trailer.

## Code style

- Use stable Rust for ordinary development; root `Cargo.toml` declares MSRV 1.94 and edition 2024. CI separately checks MSRV and Windows code.
- Rust formatting/linting, frontend/site formatting and oxlint, and dependency/license checks use the commands below. No proprietary/non-open-source dependencies; find an allowed alternative rather than widening license allow-lists.
- Python uses the Ruff version pinned in CI and standard-library unittest: `ruff check scripts`, `ruff format --check scripts`, and `python3 -m unittest discover -s scripts -p '*_test.py'`.
- Install Lefthook with `lefthook install`. Hooks format staged non-generated files and run source-triggered lint/build checks, not the full test suite. Preserve generated exclusions and `.env`/Bitwarden sync. Never commit secrets; without a local `.env`, use `LEFTHOOK=0` so sync cannot erase the shared note.
- Knip's `pnpm run check:dead-code` reports unused files/dependencies/exports; exceptions in `knip.jsonc` must stay narrow.

### Optional local task runner (`just`)

`just` is optional; its recipes use the same underlying repository commands as CI. Install pnpm workspace dependencies with `pnpm install --frozen-lockfile`.

| Recipe                                  | Purpose                                                                                           |
| --------------------------------------- | ------------------------------------------------------------------------------------------------- |
| `just check`                            | Rust fmt, strict Clippy/rustdoc, tests, deny/machete, generated-reference drift, workspace build. |
| `just fmt` / `just fmt check`           | Format Rust / check without editing.                                                              |
| `just lint` / `just test` / `just deny` | Individual locked lint/test or dependency checks.                                                 |
| `just cov`                              | Generate Rust LCOV; enforce the canonical-line policy below.                                      |
| `just gen`                              | Regenerate OpenAPI, CLI/config references, man/completions, and TS API schema.                    |
| `just fe`                               | Licenses, client drift, format, lint, unit tests, and frontend typecheck/build.                   |
| `just e2e`                              | Build server/frontend, install Chromium, run Full/Demo serially with isolated servers.            |
| `just dev`                              | Temporary DB/config/logs, API on `0.0.0.0:8787`, Vite on 5173; Ctrl+C cleans owned state.         |

`just dev` refuses occupied ports and never stops unrelated listeners. Use `http://localhost:5173`; another host needs the exact `BHTUNE_ORIGIN` and Vite host allowance. It is an unauthenticated trusted-network development setup.

### Lint policy

`[workspace.lints]` in root `Cargo.toml` applies to every product/test-support member's libraries, binaries, tests, examples, and build scripts. `fuzz/` and the native fixture are separate workspaces; the fixture has explicit Windows gates below.

- Denied: production unsafe code, `dbg!`, `todo!`, `unimplemented!`, undocumented unsafe blocks, default Clippy groups, and `unwrap`/`expect`/panic restrictions. Production returns typed errors, especially after loop mutation when panic could skip restore.
- Tests allow panic restrictions at the appropriate crate root or integration/example/bench file. Existing production invariant exceptions need a site-level allow with a reason, never a blanket exemption.
- Hygiene and the curated pedantic subset warn locally but fail strict CI. The remainder of pedantic, nursery, and cargo groups is intentionally off; dependency policy belongs to deny/machete.
- Fix findings. A justified suppression uses the smallest `#[expect(lint, reason = "...")]`; an unfulfilled expectation must fail CI. Do not weaken lints or use broad allows, `NOSONAR`, or dashboard dispositions to hide actionable issues.
- A necessary unsafe test helper needs a scoped `#[expect(unsafe_code, reason = "...")]` and a `// SAFETY:` comment inside its body directly above the block. Native COM ABI code stays confined to the unpublished fixture, never product crates.
- Propose a lint by measuring `cargo clippy --workspace --all-targets --all-features -- -W clippy::<lint>` and fixing its findings in that change.

## Dependency updates

Dependabot groups weekly patch/minor updates for product/fixture Cargo, pnpm, and Actions; major upgrades are focused PRs. Change manifests and lockfiles together, preserve MSRV and frontend/site/codegen compatibility, and document necessary pins rather than treating latest as unconditional.

Run all affected ecosystem gates below, plus the MSRV check for Cargo updates. A release-time dependency sweep follows the same review and validation policy.

## Testing

Test pure domain logic with `cargo test --workspace --locked` and frontend helpers/components with `pnpm --filter bhtune-frontend run test`. Keep core clock/I/O-free; [golden replay](docs/internal/design/validation-golden-replay.md) complements focused regressions for the [correctness register](docs/internal/design/correctness-register.md).

Large unit suites use `#[cfg(test)] mod tests;`: `foo.rs` and `foo/mod.rs` both use `foo/tests.rs`; nested modules use `foo/tests/<module>.rs`. File-only refactors preserve private access, the `cargo test --workspace --locked -- --list` inventory, and canonical source-line coverage.

Coverage is 100% of canonical LCOV `DA` product-source lines. CI rejects zero-hit lines and normalizes aggregate `LF`/`LH` before Codecov upload; never weaken this policy. Add focused cases for changed behavior, errors, and meaningful edge cases.

Playwright's real suites drive the built UI and actual simulator server; the mocked Demo contract suite is separate evidence within the Demo project. List the actual inventory with `pnpm --filter bhtune-frontend exec playwright test --list`. Run projects with:

```sh
pnpm --filter bhtune-frontend run build
cargo build -p bhtune-server --locked
pnpm --filter bhtune-frontend exec playwright install chromium
PLAYWRIGHT_MODE=full pnpm --filter bhtune-frontend exec playwright test --project=full
PLAYWRIGHT_MODE=demo pnpm --filter bhtune-frontend exec playwright test --project=demo
```

Demo also needs `openssl` for its isolated loopback HTTPS certificate. `run test:e2e` without `PLAYWRIGHT_MODE` runs both projects and starts their own isolated servers. Shared OPC helpers are in `frontend/e2e/support/opcBrowser.ts`.

### Windows native OPC DA smoke

Use only a disposable plant-free Windows host. The serial smoke runs the real CLI, installer-pinned verified official x86 gateway, and unpublished synthetic `ByteHound.BHTuneOpcDaSmoke.1` COM fixture. It covers read-only preflight without DB creation, paged browse/session close, a manual index, and a Yokogawa temperature/PID tune with Moderate write-back and restoration. It does not validate CENTUM firmware/site permissions, real OPCEnum implementation, remote DCOM, or React rendering.

Preflight needs `OPC.ServerList.1`: preserve any real OPCEnum; add the fixture's test catalog only when absent from both hives. Refuse existing 32-bit COM-class collisions. Default User registration requires non-elevated UAC; real OPCEnum may require machine-scoped fixture categories. Machine scope must be explicitly selected on a disposable host, never automatic elevation/fallback.

Use PowerShell >= 7.4, Rustup, Visual C++ x86 tools, `protoc`, gh, Cosign, and cargo-deny. Firewall creation/removal independently requires administrator permission. Explicit Machine-scope local reproduction:

```powershell
rustup toolchain install stable-x86_64-pc-windows-msvc --profile minimal --component rustfmt --component clippy --target i686-pc-windows-msvc
cargo +stable-x86_64-pc-windows-msvc fmt --manifest-path tests/opcda-yokogawa-fixture/Cargo.toml --all --check
cargo +stable-x86_64-pc-windows-msvc clippy --manifest-path tests/opcda-yokogawa-fixture/Cargo.toml --target i686-pc-windows-msvc --target-dir target/opcda-yokogawa-fixture --all-targets --all-features --locked -- -D warnings
cargo +stable-x86_64-pc-windows-msvc test --manifest-path tests/opcda-yokogawa-fixture/Cargo.toml --target i686-pc-windows-msvc --target-dir target/opcda-yokogawa-fixture --locked -- --test-threads=1
cargo deny --manifest-path tests/opcda-yokogawa-fixture/Cargo.toml --target i686-pc-windows-msvc --locked --config deny.toml check
pwsh -NoProfile -File scripts/test-opcda-yokogawa.ps1 -RegistrationScope Machine -ArtifactsDirectory target/opcda-yokogawa-smoke
```

The runner tests model/COM components serially before building the DLL; ordinary GNU BHTune tooling is unchanged. Root workspace commands do not cover fixture gates and leave the native integration test ignored.

Use an owned foreground gateway on an unused high port, never port 7600, services, installed gateways, or plant namespaces. Its listener is wildcard although the client is loopback: local runs require enabled firewall profiles and an enforced owned executable inbound-block rule. Disposable hosted CI may record its disabled-firewall plant-free VM boundary without changing global policy.

Finite deadlines/job objects and ownership-checked `finally` cleanup protect processes, COM keys, firewall, and scratch state; retain firewall protection on unresolved process-stop failure. Preserve environment and global UAC/DCOM/firewall policy. Unique `run-*` evidence includes source/toolchain/hash/PID metadata, bounded logs/phase outputs, audit, discovery/firewall boundary, and DB snapshots copied after owned processes stop. CI uploads failure evidence.

The plant-free mocked cleanup/process checks run without starting a gateway or registering COM:

```powershell
pwsh -NoProfile -File scripts/test-opcda-yokogawa-cleanup.Tests.ps1
pwsh -NoProfile -File scripts/test-opcda-yokogawa-process.Tests.ps1
```

Numeric simulator regressions, golden replay, mocked bridge/browser tests, and native smoke
prove different boundaries; none alone establishes real-plant compatibility or tuning quality.
See the [simulator model](docs/guides/simulator-model.md) and
[frontend architecture](docs/architecture/frontend.md).

## CI

Run the smallest affected check first, then every applicable gate. Source/version definitions, package scripts, `justfile`, and workflows are authoritative:

| Area                 | Commands                                                                                                                                                                                                                                                                             |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Rust                 | `cargo fmt --all --check`; `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`; `cargo test --workspace --locked`; `cargo deny check`; `cargo machete`; `cargo build --workspace --locked` |
| Coverage             | `cargo llvm-cov --workspace --locked --lcov --output-path lcov.info`                                                                                                                                                                                                                 |
| Generated references | `cargo run --locked -p bhtune-server --example gen_openapi`; `cargo run --locked -p bhtune --example gen_docs --features schemars`; `pnpm --filter bhtune-frontend run generate:api`; review/commit source-required output                                                           |
| Frontend             | `pnpm --filter bhtune-frontend run format:check`; `run lint`; `run test`; `run build` (typechecks SPA and E2E)                                                                                                                                                                       |
| Documentation site   | `pnpm --filter bhtune-website run format:check`; `run lint`; `run typecheck`; `run build` (broken links/anchors are errors)                                                                                                                                                          |
| pnpm policy          | `pnpm run check:licenses`; `pnpm run check:dead-code`                                                                                                                                                                                                                                |
| Packages             | `cargo package --workspace --locked --no-verify`; applicable AUR/installer/package acceptance                                                                                                                                                                                        |

Regeneration/drift gates remain; there is no OpenAPI base-branch breaking-comparison job. Review API contract changes and regenerate OpenAPI/TS together. Package `--no-verify` avoids registry resolution of coordinated unpublished APIs; workspace build/Clippy/tests validate the actual local graph.

Windows validation includes separate fixture gates and native smoke when source/runner/contract changes affect it. Browser checks use the Full/Demo projects above; screenshot CI rebuilds real assets. Documentation-only changes need authored Markdown and website gates, not Rust tests. Workflow changes require actionlint.

Sonar runs relevant same-repository PR/main changes and scheduled scans; docs-only/fork analyses explicitly skip. Require zero `OPEN`/`CONFIRMED` issues for applicable analysis, not just a badge. Maintainers read `SONAR_TOKEN` from ignored `.env`, restored with `rbw unlock && rbw sync && ds sync`; never print/commit it. Legitimate Accepted/False Positive findings require evidence and a linked rationale.

Keep required statuses named `Required validation status`, `Required coverage status`, `Required E2E status`, and `Required Sonar quality status`. Repair failed checks on the same branch; keep the base current before an approved merge.

### Packaging validation

Keep generated artifacts/disposable workspaces outside Git. AUR generation is metadata-only, network-free in its regression suite, and never compiles/publishes. Run from repository root:

```sh
python3 scripts/aurpkg_test.py
shellcheck scripts/aurpkg
shfmt -d scripts/aurpkg
```

Use disposable Arch/non-root `makepkg` for source verification, `.SRCINFO` generation, and install/upgrade/removal/service checks. Never hand-edit `.SRCINFO`. Publication requires exact stable tags and independently verified release evidence; first publication is manual until the remote commit is verified. Preserve `docker run -i` script input and host-mounted `aur-evidence/`; archive assertions accept `usr/...` and `./usr/...`.

Debian `depends = "$auto"` needs `dpkg-shlibdeps`, not a hard-coded fallback list. Keep paired binaries and every distribution channel: archives, deb/rpm, AUR, Docker, NSIS, launchd/Homebrew. Do not change `release.yml` during reusable installer/AUR work; integration is coordinated separately. See [packaging architecture](docs/architecture/packaging.md) and [release verification](docs/guides/release-verification.md).

## Security and compatibility checks

Report vulnerabilities privately via [SECURITY.md](SECURITY.md). CodeQL, Semgrep, full-history Gitleaks, actionlint, and zizmor are repository gates. Keep least-privilege permissions and immutable Action SHA pins; never use `pull_request_target` to obtain fork secrets.

Parser changes need focused property tests and an externally reachable boundary's fuzz target. API changes need reviewed contracts and regenerated outputs; DB changes need representative schema/upgrade tests. Release assets must retain checksums, CycloneDX SBOM, Sigstore bundles, and GitHub provenance.

## Documentation

User-visible behavior/default/config/API/safety changes require README, relevant guides, generated references, and AGENTS invariant updates as applicable. Follow [Documentation contract](AGENTS.md#documentation-contract). The paired [docs-drift hook](.github/hooks/README.md) warns about committed/uncommitted implementation changes without a documentation surface; it does not replace deliberate updates.

Generated CLI/config references, OpenAPI, TS schema, man pages, and completions must not be hand-edited/formatted. Other reference pages are authored; the CI docs agent still cannot edit `docs/reference/**` or AGENTS. User-facing prose describes enduring behavior, not a change log.

### Web UI screenshots

Full/Demo captures use deterministic Playwright against the real SPA. PNGs never enter Git; `docs/reference/web-ui-screenshots.json` is the workflow-owned text lock of scenarios/dimensions/hashes/Pages URLs.

```sh
pnpm docs:screenshots
pnpm docs:screenshots:validate
pnpm docs:screenshots:gallery
```

Capture runs serially, updates the lock, and creates `frontend/test-results/docs-screenshots/index.html`; `docs:screenshots:check` compares candidates without changing it. Pages regenerates ignored image assets from the merged commit. Do not rewrite locks merely for local rendering differences.

Validation maps static `documentationId`/`data-doc-section` markers to scenarios. Add captures for uncovered routes/sections/modals/safety states; supply meaningful alt text, captions, full-size links, and complete prose instructions. Keep the fixed viewport; short pages use `capture: "content-fit"` with shared automatic sizing, not per-page heights.

## Pull requests

- One logical change; describe why, link existing issues, and use the repository PR template.
- Record exact validation commands with results and any outstanding manual review. Inspect status/staged diff; stage only intended files, never build outputs or secrets.
- Wait for applicable required, CLA, and zero-issue Sonar checks plus explicit maintainer review/testing approval. Then use protected squash merge and delete only the task branch; never queue an unreviewed auto-merge or hide findings.

## Contributing a DCS/PLC template

Add a `[[template]]` entry to [builtin.toml](crates/bhtune-core/templates/builtin.toml), following the [field reference/worked example](docs/dcs-templates.md). Copy a close entry or export one with `bhtune template export <name> out.toml --format toml`, then supply every suffix/raw mode/precision field and vendor-version list.

Changed vendor conventions need a new uniquely named entry, not edits that break sites using the old mapping. Embedded-catalog tests parse/validate every entry.

## Releases and changelogs

Root [CHANGELOG.md](CHANGELOG.md) covers all six product crates; unpublished test support/COM fixtures are not release artifacts. Contributors describe behavior in commits/docs, not hand-edited release versions. `release-plz` owns the versioned entry and `bhtune` product tag; only `release.yml` creates GitHub Releases/assets. The release policy is git-only, not crates.io publication.

Keep `RELEASE_AUTOMATION_ENABLED` disabled until an explicitly approved RC/canary and activation gates pass. Follow the [release guide](docs/guides/releasing.md) for baseline, snapshot, rate-limit, approval, and failure-recovery rules. No release, tag, publication, or deployment is implicit in a feature PR.

## License

Contributions use [AGPL-3.0-or-later](LICENSE), subject to the signed [CLA](CLA.md).
