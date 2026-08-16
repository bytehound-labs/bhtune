#!/usr/bin/env bash
# Copilot CLI sessionEnd hook. Warns (stderr only -- sessionEnd hook output isn't otherwise
# processed by the CLI, so this can inform but never block) when a session touched crates/**
# without touching any documentation surface (docs/**, README.md, AGENTS.md, CONTRIBUTING.md).
# See "Documentation contract" in AGENTS.md and .github/hooks/README.md for the full design,
# including why this pairs with a sessionStart hook rather than standing alone -- in short,
# by sessionEnd this project's own workflow has usually already committed and pushed, so
# looking only at the uncommitted working tree would almost always see nothing.
#
# Never fails the session: every error path falls through to a plain `exit 0`.

set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

INPUT="$(cat 2>/dev/null || true)"
SESSION_ID="$(docs_drift_read_session_id "$INPUT")"

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
cd "$REPO_ROOT" || exit 0

STATE_DIR="$(docs_drift_state_dir)"
START_SHA=""
if [ -n "$SESSION_ID" ] && [ -f "$STATE_DIR/$SESSION_ID.start-sha" ]; then
	START_SHA="$(cat "$STATE_DIR/$SESSION_ID.start-sha" 2>/dev/null || true)"
	rm -f "$STATE_DIR/$SESSION_ID.start-sha" 2>/dev/null || true
fi

# Files changed by commits made since this session started (covers the common case in this
# repo: commit + push after every step, so the working tree is clean again by sessionEnd).
COMMITTED_FILES=""
if [ -n "$START_SHA" ] && git cat-file -e "$START_SHA" 2>/dev/null; then
	COMMITTED_FILES="$(git diff --name-only "$START_SHA" HEAD 2>/dev/null || true)"
fi

# Files still sitting uncommitted (covers a session that ends mid-work, or one where no
# sessionStart marker exists at all -- e.g. the hook was installed after this session began).
# Renamed-but-uncommitted paths ("old -> new") are a known, accepted imperfection here: they
# won't cleanly match either pattern below, which just means a rename alone won't trigger the
# warning -- acceptable for a best-effort, warn-only heuristic.
UNCOMMITTED_FILES="$(git status --porcelain --untracked-files=all 2>/dev/null | cut -c4- || true)"

CHANGED_FILES="$(printf '%s\n%s\n' "$COMMITTED_FILES" "$UNCOMMITTED_FILES")"

TOUCHED_CRATES=0
TOUCHED_DOCS=0
while IFS= read -r f; do
	[ -z "$f" ] && continue
	case "$f" in
	crates/*) TOUCHED_CRATES=1 ;;
	esac
	case "$f" in
	docs/* | README.md | AGENTS.md | CONTRIBUTING.md) TOUCHED_DOCS=1 ;;
	esac
done <<<"$CHANGED_FILES"

if [ "$TOUCHED_CRATES" -eq 1 ] && [ "$TOUCHED_DOCS" -eq 0 ]; then
	echo 'bhtune docs-drift hook: this session changed files under crates/** but none under docs/**, README.md, AGENTS.md, or CONTRIBUTING.md. If user-visible behavior changed, update the docs before starting the next task (see "Documentation contract" in AGENTS.md).' >&2
fi

exit 0
