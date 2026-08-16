#!/usr/bin/env bash
# Shared helpers for the bhtune docs-drift Copilot CLI hook scripts. Sourced by
# docs-drift-session-start.sh and docs-drift-session-end.sh -- not a hook entry point itself,
# so it has no shebang-driven behavior of its own beyond documenting how it's meant to be used.

# Reads a hook's JSON payload (passed as $1) and echoes its "sessionId" field, or nothing if
# the field can't be found. Deliberately a plain grep/sed extraction rather than a real JSON
# parser, so the hook has no dependency beyond a bash + grep + sed, which every supported dev
# machine already has -- pulling in `jq` would be one more thing to check for before this
# cheap, zero-cost hook could be relied on.
docs_drift_read_session_id() {
	local input="$1"
	printf '%s' "$input" |
		grep -o '"sessionId"[[:space:]]*:[[:space:]]*"[^"]*"' |
		head -n1 |
		sed -E 's/^"sessionId"[[:space:]]*:[[:space:]]*"//; s/"$//'
}

# The directory the two scripts use to pass the session's starting HEAD commit from
# sessionStart to sessionEnd, keyed by session ID (one file per in-flight session).
docs_drift_state_dir() {
	printf '%s/bhtune-docs-drift-hook' "${TMPDIR:-/tmp}"
}
