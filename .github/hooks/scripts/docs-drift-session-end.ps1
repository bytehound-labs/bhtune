# Copilot CLI sessionEnd hook (PowerShell / Windows). Warns (stderr only -- sessionEnd hook
# output isn't otherwise processed by the CLI, so this can inform but never block) when a
# session touched Rust, user-visible frontend, or screenshot-documentation implementation files
# without touching any documentation surface (docs/**, README.md, AGENTS.md, CONTRIBUTING.md,
# frontend/README.md, website/README.md). See "Documentation contract" in AGENTS.md and
# .github/hooks/README.md for the full design, including why this pairs with a sessionStart
# hook rather than standing alone -- in short, by sessionEnd this project's own workflow has
# usually already committed and pushed, so looking only at the uncommitted working tree would
# almost always see nothing.
#
# Never fails the session: every error path falls through to exit 0.

$ErrorActionPreference = 'SilentlyContinue'

$stdin = [Console]::In.ReadToEnd()
$sessionId = $null
if (-not [string]::IsNullOrEmpty($stdin)) {
    $match = [regex]::Match($stdin, '"sessionId"\s*:\s*"([^"]*)"')
    if ($match.Success) { $sessionId = $match.Groups[1].Value }
}

$repoRoot = (git rev-parse --show-toplevel 2>$null)
if ([string]::IsNullOrEmpty($repoRoot)) { exit 0 }
Set-Location $repoRoot

# $env:TEMP is always set on Windows; fall back for the rare case of pwsh on Linux/macOS,
# where it isn't (matching the bash script's own ${TMPDIR:-/tmp} fallback).
$tempRoot = if ($env:TEMP) { $env:TEMP } elseif ($env:TMPDIR) { $env:TMPDIR } else { '/tmp' }
$stateDir = Join-Path $tempRoot 'bhtune-docs-drift-hook'
$startSha = $null
if (-not [string]::IsNullOrEmpty($sessionId)) {
    $markerPath = Join-Path $stateDir "$sessionId.start-sha"
    if (Test-Path $markerPath) {
        $startSha = (Get-Content -Path $markerPath -Raw).Trim()
        Remove-Item -Path $markerPath -Force
    }
}

# Files changed by commits made since this session started (covers the common case in this
# repo: commit + push after every step, so the working tree is clean again by sessionEnd).
$committedFiles = @()
if (-not [string]::IsNullOrEmpty($startSha)) {
    git cat-file -e $startSha 2>$null
    if ($LASTEXITCODE -eq 0) {
        $committedFiles = @(git diff --name-only $startSha HEAD 2>$null)
    }
}

# Files still sitting uncommitted (covers a session that ends mid-work, or one where no
# sessionStart marker exists at all -- e.g. the hook was installed after this session began).
# Renamed-but-uncommitted paths ("old -> new") are a known, accepted imperfection here: they
# won't cleanly match either pattern below, which just means a rename alone won't trigger the
# warning -- acceptable for a best-effort, warn-only heuristic.
$statusLines = @(git status --porcelain --untracked-files=all 2>$null)
$uncommittedFiles = @()
if ($statusLines) {
    $uncommittedFiles = $statusLines | ForEach-Object { $_.Substring([Math]::Min(3, $_.Length)) }
}

$changedFiles = @($committedFiles) + @($uncommittedFiles)

$touchedImplementation = $false
$touchedDocs = $false
foreach ($f in $changedFiles) {
    if ([string]::IsNullOrEmpty($f)) { continue }
    if (
        $f -like 'crates/*' -or $f -like 'crates\*' -or
        $f -like 'frontend/src/*' -or $f -like 'frontend/src\*' -or
        $f -like 'frontend/e2e/docs-screenshots/*' -or
        $f -like 'frontend/e2e/docs-screenshots\*' -or
        $f -eq 'frontend/playwright.docs.config.ts' -or
        $f -eq 'scripts/web-ui-screenshots.mjs' -or
        $f -eq 'docs/reference/web-ui-screenshots.json' -or
        $f -eq 'package.json' -or
        $f -eq 'pnpm-lock.yaml' -or
        $f -eq 'pnpm-workspace.yaml' -or
        $f -eq '.github/workflows/docs-agent.yml' -or
        $f -eq '.github/workflows/docs-deploy.yml'
    ) { $touchedImplementation = $true }
    if (
        $f -like 'docs/*' -or $f -like 'docs\*' -or
        $f -eq 'README.md' -or $f -eq 'AGENTS.md' -or $f -eq 'CONTRIBUTING.md' -or
        $f -eq 'frontend/README.md' -or $f -eq 'frontend\README.md' -or
        $f -eq 'website/README.md' -or $f -eq 'website\README.md'
    ) { $touchedDocs = $true }
}

if ($touchedImplementation -and -not $touchedDocs) {
    [Console]::Error.WriteLine('bhtune docs-drift hook: this session changed Rust, user-visible frontend, or screenshot-documentation implementation files but none under docs/**, README.md, AGENTS.md, CONTRIBUTING.md, frontend/README.md, or website/README.md. If user-visible behavior changed, update the docs before starting the next task (see "Documentation contract" in AGENTS.md).')
}

exit 0
