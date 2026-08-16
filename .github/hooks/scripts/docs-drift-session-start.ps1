# Copilot CLI sessionStart hook (PowerShell / Windows). Records the repo's current HEAD
# commit, keyed by session ID, so the paired sessionEnd hook (docs-drift-session-end.ps1)
# can look at everything a session changed -- including work that was already committed
# (and possibly pushed) by the time the session ends -- not just whatever is still sitting
# uncommitted in the working tree. See "Documentation contract" in AGENTS.md and
# .github/hooks/README.md for the full design.
#
# Never fails the session: every error path falls through to exit 0. This hook emits no
# output; sessionStart's optional `additionalContext` mechanism is intentionally unused here.

$ErrorActionPreference = 'SilentlyContinue'

$stdin = [Console]::In.ReadToEnd()
if ([string]::IsNullOrEmpty($stdin)) { exit 0 }

$match = [regex]::Match($stdin, '"sessionId"\s*:\s*"([^"]*)"')
if (-not $match.Success) { exit 0 }
$sessionId = $match.Groups[1].Value
if ([string]::IsNullOrEmpty($sessionId)) { exit 0 }

$repoRoot = (git rev-parse --show-toplevel 2>$null)
if ([string]::IsNullOrEmpty($repoRoot)) { exit 0 }

# $env:TEMP is always set on Windows; fall back for the rare case of pwsh on Linux/macOS,
# where it isn't (matching the bash script's own ${TMPDIR:-/tmp} fallback).
$tempRoot = if ($env:TEMP) { $env:TEMP } elseif ($env:TMPDIR) { $env:TMPDIR } else { '/tmp' }
$stateDir = Join-Path $tempRoot 'bhtune-docs-drift-hook'
New-Item -ItemType Directory -Force -Path $stateDir | Out-Null

$headSha = (git -C $repoRoot rev-parse HEAD 2>$null)
if (-not [string]::IsNullOrEmpty($headSha)) {
    Set-Content -Path (Join-Path $stateDir "$sessionId.start-sha") -Value $headSha -NoNewline
}

exit 0
