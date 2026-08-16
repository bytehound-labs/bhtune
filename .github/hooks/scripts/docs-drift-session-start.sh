#!/usr/bin/env bash
# Copilot CLI sessionStart hook. Records the repo's current HEAD commit, keyed by session ID,
# so the paired sessionEnd hook (docs-drift-session-end.sh) can look at everything a session
# changed -- including work that was already committed (and possibly pushed) by the time the
# session ends -- not just whatever is still sitting uncommitted in the working tree. See
# "Documentation contract" in AGENTS.md and .github/hooks/README.md for the full design.
#
# Never fails the session: every error path falls through to a plain `exit 0`. This hook emits
# no output; sessionStart's optional `additionalContext` mechanism is intentionally unused here.

set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

INPUT="$(cat 2>/dev/null || true)"
SESSION_ID="$(docs_drift_read_session_id "$INPUT")"

[ -n "$SESSION_ID" ] || exit 0

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0

STATE_DIR="$(docs_drift_state_dir)"
mkdir -p "$STATE_DIR" 2>/dev/null || exit 0

git -C "$REPO_ROOT" rev-parse HEAD >"$STATE_DIR/$SESSION_ID.start-sha" 2>/dev/null || true

exit 0
