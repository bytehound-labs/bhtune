# Copilot CLI hooks (`docs-copilot-hook`)

This directory holds repository-level [Copilot CLI hooks](https://docs.github.com/en/copilot/reference/hooks-reference)
(`.github/hooks/*.json`, loaded automatically for anyone working in this repo with Copilot
CLI). JSON can't hold comments, so the rationale for what's here lives in this file instead.
See "Documentation contract" in [`AGENTS.md`](../../AGENTS.md) for the policy this hook backs
up.

## `docs-drift.json`: warn when `crates/**` changes without a docs change

**What it does.** At the end of a session, prints a one-line warning to stderr if the session
changed files under `crates/**` but none under `docs/**`, `README.md`, `AGENTS.md`, or
`CONTRIBUTING.md`. It never blocks anything — `sessionEnd` hook output isn't consumed as a
decision by the CLI at all (see the hooks reference's event table), so this can only inform,
never fail a session or a command.

**Why it's a pair of hooks, not just one.** A `sessionEnd`-only hook can only see two things:
the working tree's currently uncommitted changes, and repo history in general — it has no
notion of "what this session did" on its own. That's a real gap for this project specifically:
the established workflow here is to commit and push after every step, so by the time
`sessionEnd` fires, the working tree is usually already clean and the session's commits are
already on `origin/main`. A hook that only checked `git status`/`git diff` against the working
tree would almost always see nothing and silently never fire for the exact case it exists to
catch.

So `docs-drift.json` registers a `sessionStart` hook too. It writes the repo's HEAD commit SHA
to a temp file keyed by `sessionId` (`$TMPDIR/bhtune-docs-drift-hook/<sessionId>.start-sha`,
or `%TEMP%\bhtune-docs-drift-hook\<sessionId>.start-sha` on Windows). The `sessionEnd` hook
reads that marker, diffs from the recorded SHA to the current `HEAD` to see everything the
session committed, unions it with whatever is still sitting uncommitted, and then checks the
combined file list. The marker file is deleted as soon as `sessionEnd` reads it, so it never
accumulates across sessions. If no marker exists (for example, the hook was only just
installed and this session started before it existed), the check silently falls back to
looking at uncommitted changes only, rather than erroring.

**Both `bash` and `powershell` entries are provided** (`docs-drift-session-start.sh` /
`.ps1` and `docs-drift-session-end.sh` / `.ps1`), so the hook works the same way for
contributors on Linux, macOS, or Windows, per the project's own cross-platform stance. Shared
bash logic (reading the `sessionId` field out of the hook's JSON stdin payload, and the
temp-state-directory path) lives in `scripts/lib.sh`, sourced by both `.sh` scripts; there is
no equivalent shared file for the two `.ps1` scripts since PowerShell has no simple bash-style
`source` idiom worth the indirection for two short scripts.

**Deliberately not a JSON parser dependency.** The `sessionId` field is pulled out of the raw
JSON payload with `grep`/`sed` (bash) or a single regex (`[regex]::Match`, PowerShell) rather
than a real JSON library, so the hook has no dependency beyond what every dev machine already
has. This is safe here because the one field being read is a flat, unnested string value.

**Known, accepted limitation.** A file renamed (but not yet committed) shows up in
`git status --porcelain` as `old -> new`, which won't cleanly match either the `crates/*` or
`docs/*` pattern the scripts check against. A pure rename with no other changes therefore
won't trigger the warning. This is an acceptable gap for a best-effort, warn-only heuristic —
not worth the extra parsing complexity to close.

**Testing it by hand.** Pipe a fake payload into either script directly, e.g.:

```bash
echo '{"sessionId":"test-123","timestamp":0,"cwd":".","source":"new"}' \
  | bash .github/hooks/scripts/docs-drift-session-start.sh
# ... change a file under crates/, without touching docs/ ...
echo '{"sessionId":"test-123","timestamp":0,"cwd":".","reason":"complete"}' \
  | bash .github/hooks/scripts/docs-drift-session-end.sh
```

The second command should print the warning to stderr.
