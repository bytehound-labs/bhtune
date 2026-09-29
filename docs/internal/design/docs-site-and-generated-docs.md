# Documentation site and generated references

Design notes preserved from the former monolithic `AGENTS.md`. Code and tests are authoritative; see
the [design notes index](README.md) for provenance.

## Documentation drift hook

`docs-copilot-hook` is a paired `sessionStart`/`sessionEnd` Copilot CLI hook
(`.github/hooks/docs-drift.json`) that warns when a session changed Rust, user-visible
`frontend/src/**`, or screenshot-documentation implementation change without touching any
documentation surface, covering both a session's already-committed-and- pushed changes and anything
still uncommitted (see `.github/hooks/README.md` for why it's a pair, not a single hook).

## `docs-generated-cli`: generating the CLI reference, man pages, completions, and config schema

A new `crates/bhtune-cli/examples/gen_docs.rs` regenerates four artifacts from the same
`clap`/`serde` definitions every real `bhtune` invocation already parses against, so none of
them can silently drift the way hand-written usage docs would — reusing `gen_openapi`'s exact
regenerate-and-diff idiom (see "Key architectural decisions" above), the pattern that file's
own doc comment named this example as the intended reuse of:

```sh
cargo run -p bhtune-cli --example gen_docs --features schemars
```

- **`docs/reference/cli.md`** — the full CLI reference as one Markdown document, via
  `clap_markdown::help_markdown_custom::<bhtune_cli::args::Cli>(&options)`. `clap-markdown`
  recurses the entire `Command` tree itself (no manual subcommand walk needed for this one),
  producing a table of contents plus one section per command/subcommand with its usage,
  options, and doc-comment prose.
- **`man/*.1`** — one man page per command _and_ subcommand (`bhtune.1`, `bhtune-tune.1`,
  `bhtune-template.1`, `bhtune-template-list.1`, ... 18 pages total), matching the convention
  real multi-command tools use (git, cargo) rather than one flat page. `clap_mangen::Man`
  only renders a single `clap::Command` at a time, so `gen_docs.rs` walks
  `Command::get_subcommands()` recursively itself, renaming each nested `Command` to its full
  hyphenated path (`cmd.name(...)`, e.g. `template` becomes `bhtune-template`) before
  rendering, exactly mirroring how git/cargo name their own subcommand man pages. `Command::
name` needs an owned `String` converted `impl Into<clap::builder::Str>`, which only exists
  behind clap's `string` feature (not otherwise used by this crate, and not worth enabling
  workspace-wide for one codegen example) — `gen_docs.rs` instead leaks the short-lived
  recursion-computed name strings (`Box::leak`), which is fine for a one-shot process that
  exits immediately after writing its output. These pages are what will let `pkg-aur` install
  real content into `/usr/share/man/man1/` instead of shipping a binary with no man page at
  all.
- **`completions/bhtune.bash`, `completions/_bhtune` (zsh), `completions/bhtune.fish`** — via
  `clap_complete::generate`, one file per shell using each shell's own conventional completion
  file name.
- **`docs/reference/config.md`** — JSON Schema for both `bhtune.toml` (`bhtune_cli::config::
BhtuneConfig`/`LogConfig`) and one DCS/PLC template catalog entry (`bhtune_core::template::
DcsTemplate`, the same type `template import`/the embedded and user catalogs all parse),
  rendered as two labeled fenced JSON code blocks (`schemars::schema_for!` produces a schema
  value, not prose, so there is no single-document API to lean on the way `clap-markdown`
  provides for the CLI reference). `schemars`' derive macro picks up each field's own doc
  comment as the schema's `description`, so this stays a real reflection of `template.rs`/
  `config.rs`'s existing documentation rather than a second, driftable copy of it.

**Where the `schemars` dependency lives, and why.** `DcsTemplate`'s derive lives in
`bhtune-core`, a library target, not in the example itself — so `schemars` needed the same
optional-feature treatment `bhtune-core` already has for `utoipa` (see "Key architectural
decisions" above), not a plain dev-dependency: an optional regular `[dependencies]` entry,
`#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]` alongside the existing
`#[cfg_attr(feature = "utoipa", ...)]` on `DcsTemplate` and the four `pid_config` enums it
embeds (`ProportionalType`/`IntegralType`/`DerivativeType`/`TimeUnit`), and a `schemars`
feature forwarding from `bhtune-cli` to `bhtune-core/schemars`. `BhtuneConfig`/`LogConfig`
(in `bhtune-cli` itself) get the same `cfg_attr` treatment directly. `clap-markdown`/
`clap_mangen`/`clap_complete`, by contrast, are used only by `gen_docs.rs` itself, never by
library-target code, so they stay plain `[dev-dependencies]` with no feature-gating —
mirroring exactly why `cargo add --dev` is right for those three and wrong for `schemars`
(a naive `cargo add --dev schemars` would put a library-target derive dependency somewhere
only test/example targets can see it, silently breaking the ordinary release build the
moment anyone tried to actually use the derive from `bhtune-core`'s own source). Off by
default: neither `bhtune`/`bhtune-server`'s ordinary release builds nor `bhtune-core`'s own
`cargo build -p bhtune-core` ever touch `schemars` — it is a docs-codegen-only concern.

**CI enforcement**, added to `checks.yml`'s existing `check` job right after the OpenAPI
drift step, same shape:

```yaml
- name: Regenerate CLI docs and check for drift
  run: |
    cargo run -p bhtune-cli --example gen_docs --features schemars
    git diff --exit-code -- docs/reference/ man/ completions/
```

`clippy --all-features` (already run earlier in the same job) covers linting the example
itself, since `--all-features` unifies in `schemars` and compiles `gen_docs` as part of
`--all-targets`.

**Gotcha: formatters must never touch generated files.** `.lefthook.yml`'s `prettier-format`
(glob `**/*.{md,yaml,yml,json,ts,tsx,css}`) and `shfmt-format` (glob `**/*.{sh,bash}`) hooks
would otherwise reformat `docs/reference/cli.md`, `docs/reference/config.md`, and
`completions/bhtune.bash` on every commit that touches them — caught by hand before this
phase's first push, since CI's drift check diffs _raw_ generator output against whatever is
committed and never runs prettier/shfmt first, so a single reformatting pre-commit run would
have made the drift check fail permanently. Both hooks now `exclude: ['docs/reference/**',
...]`/`exclude: ['completions/**']`; `openapi.json` and `frontend/src/api/schema.d.ts` are
excluded from `prettier-format` too, even though their generators currently happen to already
produce prettier-compatible output, so a future version bump of `utoipa`/`openapi-typescript`
can't silently reintroduce the same failure mode. Any future generated artifact should be
added to these exclude lists (or `.prettierignore`) if its extension matches an existing
format hook's glob — `man/*.1`, `completions/_bhtune` (no extension), and
`completions/bhtune.fish` happen not to match any current glob, so they needed no exclusion.

## `docs-site-scaffold`: the Docusaurus documentation site

`website/` is a new pnpm workspace member (`bhtune-website`, Docusaurus 3 classic preset)
that publishes `docs/` as a browsable, searchable site, live at
[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/) — see the
`website/` row in "Crate map and phase status" above for what's done versus still pending.
Stable documentation snapshot generation is implemented; no first-release snapshot is committed
until the stable release PR.

**The content root is the real `docs/`, not a copy.** `docusaurus.config.ts`'s `docs` preset
sets `path: '../docs'`, so the docs plugin reads Markdown directly from the repo-root folder
every other part of the project already treats as the source of truth — there is no
website-local content duplicate to keep in sync. The one consequence worth knowing:
Markdown links that escape `path`'s root (e.g. a hypothetical `../CONTRIBUTING.md` from
inside a `docs/` file) cannot be resolved by Docusaurus even though they resolve fine when
viewed raw on GitHub, since they point outside the folder the docs plugin scans. The fix is
always an absolute `https://github.com/bytehound-labs/bhtune/blob/main/...` URL instead of a
relative path — already applied once, to `docs/dcs-templates.md`'s CONTRIBUTING.md link.
`docs/internal/**` is excluded from the build entirely (`exclude: ['internal/**']`), so a
similar link from inside `docs/internal/v1-checklist.md` was left as a normal relative
`../AGENTS.md` link rather than converted.

**`docs/intro.md` is the site root, not a separate marketing page.** `routeBasePath: '/'`
(docs plugin) plus `slug: /` (frontmatter on `intro.md` itself) collapses the site's home
page onto `/` directly — no `src/pages/index.tsx` landing page exists; the scaffold's
placeholder one was deleted. Sidebar order everywhere else comes from `sidebar_position`
frontmatter on individual `.md` files plus a `_category_.json` file per subfolder
(`docs/getting-started/`, `docs/guides/`, `docs/reference/`) — `website/sidebars.ts` itself
stays a plain autogenerated-from-filesystem sidebar and does not need editing for routine
content changes (see `website/README.md`'s "Adding or reordering pages").

**`editUrl` must be a function, not a string, when `path` points outside the site
directory.** A plain string `editUrl: '.../edit/main/docs/'` naively concatenates with the
`path`-relative doc path and produces a doubled `.../edit/main/docs/../docs/intro.md` (harmless
in a browser, since `../` segments normalize, but not something to ship deliberately). The fix
is the function form Docusaurus documents for exactly this case:
`editUrl: ({docPath}) => \`https://github.com/bytehound-labs/bhtune/edit/main/docs/${docPath}\``,
which resolves cleanly (`.../edit/main/docs/intro.md`, `.../edit/main/docs/getting-started/
installation.md`, etc.) because `docPath` is already correct relative to the configured
content root.

**Search is `@easyops-cn/docusaurus-search-local`**, not Algolia DocSearch: fully static,
offline, and open-source, consistent with the project's no-proprietary-dependencies stance and
needing no third-party application/approval process. Worth revisiting once the site has
enough content and traffic to justify the extra setup.

**`onBrokenLinks`/`onBrokenAnchors` are both `'throw'`** (Docusaurus's own default, kept
rather than relaxed), which makes `pnpm --filter bhtune-website run build` a real,
zero-extra-effort drift gate against `docs/` content referencing a page or heading that was
renamed or deleted — this already caught the CONTRIBUTING.md link above on the very first
build attempt. A new `website` job in `checks.yml` (parallel to `frontend`, same
`pnpm/setup@v2` pattern) runs `format:check`/`lint`/`typecheck`/`build` on every PR so this
gate is automatic, not something to remember to run by hand. `check:licenses` is not
duplicated in that job: `pnpm licenses list` (which `scripts/check-frontend-licenses.mjs`
shells out to) already reports on every pnpm workspace member from the repo root, so the
existing `frontend` job's license step covers `website`'s dependency tree too.

**License allowlist grew by four entries** (`scripts/check-frontend-licenses.mjs`) for
licenses genuinely new to the dependency tree, none from `bhtune-website`'s own direct
dependencies but all transitive, pulled in by Docusaurus/the search plugin's own toolchains:
`MIT-0` (`@csstools/postcss-*`), `CC-BY-4.0` (`caniuse-lite`'s browser-data tables, an
unavoidable transitive dependency of browserslist/postcss-preset-env across the whole JS
ecosystem), `MPL-1.1` (`lunr-languages`, same weak-copyleft family as the already-allowed
MPL-2.0), and `BlueOak-1.0.0` (`sax`, verified by reading its actual license text — at least
as permissive as MIT). The script also gained AND-expression support (`isAllowed` now
requires every arm of an `X AND Y` expression to be individually allowed, versus OR's "any
one arm suffices") to resolve `@swc/core-linux-x64-gnu`'s `Apache-2.0 AND MIT` without a new
allowlist entry, and a narrow `VERIFIED_UNKNOWN_LICENSES` exception keyed to the exact
`require-like@0.1.2` package version (its license metadata is undeclared, but its `License`
file is verbatim MIT text, confirmed by direct inspection) — deliberately _not_ a blanket
"Unknown is fine" rule, so a genuinely proprietary or license-less future dependency still
fails the check loudly.

**Deployment (`docs-site-deploy`).** The site is live at
[bytehound-labs.github.io/bhtune](https://bytehound-labs.github.io/bhtune/), published by
`.github/workflows/docs-deploy.yml` via `actions/upload-pages-artifact` +
`actions/deploy-pages` — the standard GitHub-Actions-native Pages flow, not the older
`docusaurus deploy`-to-a-branch approach (no `gh-pages` branch exists or is needed).
GitHub Pages itself was switched to `build_type: workflow` via the API
(`gh api --method POST repos/bytehound-labs/bhtune/pages -f build_type=workflow`) — the
default `legacy`/branch build type would otherwise ignore an Actions-based deployment
entirely. Triggers are path-filtered to `docs/**`, `website/**`, `pnpm-lock.yaml`, and the
workflow file itself, and only run on pushes to `main` (plus manual `workflow_dispatch`) —
`checks.yml`'s `website` job already builds/lints every PR, so this workflow's only job is
publishing an already-validated build, not re-validating it. `concurrency: { group: pages,
cancel-in-progress: false }` serializes deployments rather than cancelling one mid-flight,
so a fast-following push can never leave the live site on a half-published build. No custom
domain; `url`/`baseUrl`/`organizationName`/`projectName` in `docusaurus.config.ts` were
already set correctly for the `<org>.github.io/<repo>` path during `docs-site-scaffold`, so
no config changes were needed to go live. Release-time snapshot generation is implemented by
`scripts/sync_docs_version.py`; the first snapshot remains intentionally deferred until the
first stable release so the site does not expose a meaningless single-version selector.

## `docs-api-rustdoc`: publishing the Rust API reference

`cargo doc --workspace --no-deps --all-features` output is published under `/api/` on the
docs site, alongside a hand-written index page at `docs/reference/api.md` (linked from the
site navigation/footer and from `docs/reference/_category_.json`'s sidebar). Together these
give contributors a real, browsable rustdoc reference for all six crate/binary targets
(`bhtune`, `bhtune_driver`, `bhtune_cli`, `bhtune_core`, `bhtune_db`, `bhtune_server`)
without hand-authoring any of the content itself.

**Rustdoc output has no root `index.html` for a multi-crate workspace**, and produces five
fixed infrastructure directories alongside the real per-crate ones: `search.index`, `src`,
`static.files`, `trait.impl`, `type.impl`. `.github/workflows/docs-deploy.yml`'s publish step
therefore generates its own landing `index.html` by listing `website/static/api/*/` and
excluding exactly those five names — every other directory found is a real crate/binary and
gets a link, so the landing page can never go stale when a crate is added, renamed, or
removed; nothing needs to be hardcoded or kept in sync by hand. A stale `bhtune_desktop`
entry from the deleted `arch-drop-desktop` crate was found locally during development,
persisting in a dirty `target/doc/` from before that crate was removed from the workspace —
the same publish step always runs `rm -rf target/doc` first specifically to guard against
`Swatinem/rust-cache` ever restoring a stale `target/doc` containing docs for a since-deleted
crate.

**`docs/reference/api.md` links via Docusaurus's `pathname://` protocol**, e.g.
`pathname:///api/bhtune_core/index.html`, not a normal Markdown link. This is deliberate and
load-bearing: `checks.yml`'s PR-time `website` job builds the Docusaurus site without ever
generating rustdoc content (that only happens in `docs-deploy.yml`, which runs on `main`
pushes, not PRs), so `website/static/api/` genuinely does not exist at PR-build time.
Confirmed empirically that `pathname://` links bypass Docusaurus's route-resolution
machinery entirely and, critically, are **not checked by the `onBrokenLinks`/
`onBrokenAnchors: 'throw'` gate** — the site builds successfully with these links present
even when the target files are completely absent from disk. A normal internal Markdown link
would have failed that gate on every single PR. The links still correctly receive the site's
`baseUrl` (`/bhtune/`) prefix at build time, verified by grepping the built HTML output for
`href=/bhtune/api/bhtune_core/index.html`-style attributes.

**Two different "`/api/`" concepts exist on this project and `docs/reference/api.md`'s prose
deliberately disambiguates them**: (1) `bhtune-server`'s live HTTP REST API, documented via
OpenAPI/Scalar UI at `/api/docs` on a _running_ server instance (the functional surface the
frontend and any integration script actually call); (2) this static rustdoc Rust-source
reference, published at `/api/` on the docs website — a completely separate, statically
hosted GitHub Pages site unrelated to any running server. Both nominally live under a path
containing "/api/", so the page says so explicitly rather than leaving it to be inferred.

**`docs-deploy.yml` gained a Rust toolchain** (`dtolnay/rust-toolchain@stable` +
`Swatinem/rust-cache@v2`) and `protoc` (`taiki-e/install-action@v2`, the same tool the
`opcda-bridge-proto`/`tonic-build` build dependency needs in `checks.yml`) purely to run
`cargo doc`; it built no Rust code before this. The push-trigger `paths:` filter was widened
to include `crates/**` and root `Cargo.toml`, since rustdoc content now depends on source
changes, not just `docs/`/`website/` edits. `--all-features` (matching `checks.yml`'s
clippy/test convention) ensures `bhtune-cli`'s optional `schemars` feature — which gates the
JSON-Schema-deriving types `docs-generated-cli`'s `gen_docs` example needs — is included in
the published docs.

## `docs-agent-ci`: the AI docs agent

`.github/workflows/docs-agent.yml` captures deterministic Full/Demo Web UI screenshots on PRs
touching `crates/**`, user-visible `frontend/src/**`, or screenshot tooling, then runs GitHub
Copilot CLI headless and auto-commits narrative-prose documentation plus workflow-generated
screenshot metadata onto same-repository PR branches — tier 2 of the documentation contract
(see "Documentation contract" above). Fork PRs receive the candidate gallery without repository
secrets; a maintainer-triggered run is required to apply text updates. Tier 1
(`docs/reference/**`, generated) is already diff-gated by `checks.yml`; the screenshot lock is
workflow-owned; tier 3 (`AGENTS.md`) is explicitly off limits to this workflow.

The History and template-list screenshot scenarios disable CSS animations and wait for their
fixture-backed rows before capture so lazy-page and request timing do not become part of the
pixel baseline.

**Guardrails, all load-bearing** (numbered comments in the workflow itself cross-reference
these):

1. **Infinite loop.** The agent's own commits carry a distinct git author identity
   (`bhtune-docs-agent <bhtune-docs-agent@users.noreply.github.com>`), and a separate `guard`
   job checks HEAD's author before anything else runs, skipping if it's already the agent's own
   commit. This can't be done with `github.actor`: the agent authenticates with
   `COPILOT_GITHUB_TOKEN`, a personal PAT (see below), so GitHub attributes its push to that
   token's human owner — indistinguishable from that person pushing themselves. The commit
   author, independent of which token performed the push, is the only reliable signal. The
   implementation-path trigger filter is a second, structural line of defense (the workflow-
   owned commit only touches `docs/**`/`README.md` and the generated screenshot lock, which do
   not match those implementation paths), but the explicit author check doesn't rely on that
   alone.
2. **Blast radius.** The agent may only touch `docs/**` (excluding generated references except
   for the workflow-owned Web UI screenshot lock) and `README.md`; the screenshot capture is the
   only workflow step allowed to update that lock. Enforced twice: a `--deny-tool 'write(AGENTS.md)'`
   flag blocks the one specific file that must never be auto-edited regardless of path-prefix
   ambiguity in the CLI's own tool-permission matching, and a post-run `git status --porcelain`
   check fails the job and discards every change if the diff touched anything outside the
   allowed set — this second check is the one actually enumerated against the full allowlist,
   not just the single denied file.
3. **`AGENTS.md` is special.** The agent is instructed (and tool-blocked) to never edit it; if
   it believes something here is stale, it says so in its final response instead, which gets
   posted as a PR comment for a human to act on or ignore.
4. **Fork PRs.** `pull_request` runs from forks never receive repo secrets, so
   `COPILOT_GITHUB_TOKEN` is absent and the Copilot prose step skips itself — the safe default.
   The non-secret screenshot capture and review artifact still run. Deliberately not "fixed" with
   `pull_request_target` (write permissions in the context of untrusted fork code is a known
   privilege-escalation foot-gun). A `workflow_dispatch` path with a `pr_number` input exists
   instead, for a maintainer who has already read the diff to apply text updates manually; since
   a fork PR's branch doesn't live in this repo, that path pushes to a new
   `docs-agent/pr-<n>-followup` branch here rather than trying to push back into the fork.
5. **Auth.** `COPILOT_GITHUB_TOKEN` is a personal classic PAT (scopes include `copilot`, needed
   for Copilot CLI access — the default `GITHUB_TOKEN` cannot grant this) rather than a
   dedicated machine account or GitHub App, since classic PATs are the only token type
   confirmed to carry Copilot access, and a from-scratch bot identity was judged not worth the
   setup cost for a project at this stage. This is a real, accepted trade-off: that token's
   scopes are broader than this one workflow needs (a personal PAT can't be scoped to a single
   repository the way a GitHub App installation can). The workflow only ever reads it from
   `secrets.COPILOT_GITHUB_TOKEN`, so narrowing this later (a dedicated fine-grained PAT or App,
   if one is ever confirmed to support Copilot CLI auth) is a secret-rotation, not a workflow
   change.
6. **Cost.** Each same-repository run consumes Copilot premium requests. The implementation-path
   filter keeps this off PRs that can't have caused prose drift or visual-documentation drift,
   and `--model`/`--effort` are pinned (`gpt-6-luna`/`max`) rather than left on auto-routing or
   default reasoning effort, so a model or effort upgrade never silently changes cost/behavior on
   every future PR without a reviewed change here.

**Script-injection lesson.** `github.event.pull_request.head.ref` must never be interpolated
directly into a `run:` shell block: PR branch names are attacker-controlled and git ref names
permit shell metacharacters like `$()`, so GitHub's literal template substitution would splice
attacker-controlled text directly into the script before the shell ever saw it. Pass such
values through `env:` instead, so the value becomes a runtime shell-variable expansion rather
than a compile-time text substitution — the standard fix for this whole vulnerability class
(`actionlint` flags it). The auto-commit path has only been exercised by a trivial same-repo
smoke PR — treat it as unproven under real-world drift until a real, non-trivial PR that
actually warrants a prose change goes through it.
