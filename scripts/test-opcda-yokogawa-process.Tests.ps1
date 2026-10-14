#Requires -Version 7.4
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) { throw 'The native process regression requires Windows.' }

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'test-opcda-yokogawa.ps1'), [ref]$tokens, [ref]$errors
)
if ($errors.Count -gt 0) { throw ($errors | Out-String) }
$helper = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.CommandAst] -and $node.GetCommandName() -eq 'Add-Type'
}, $true)
Add-Type -TypeDefinition $helper.CommandElements[-1].Value

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$runId = [guid]::NewGuid().ToString('N')
$scratch = Join-Path $repositoryRoot "target/native process regression-$runId"
$parent = Split-Path -Parent $scratch
if ((Test-Path -LiteralPath $parent) -and
    (Get-Item -LiteralPath $parent -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) {
    throw 'Refusing a reparse-point regression output directory.'
}
if (Test-Path -LiteralPath $scratch) { throw 'Regression scratch directory already exists.' }
New-Item -ItemType Directory -Path $scratch | Out-Null
[IO.File]::WriteAllText((Join-Path $scratch '.owner'), $runId)
$child = $null
$stopped = $true
try {
    $cmd = Join-Path ([Environment]::GetFolderPath('System')) 'cmd.exe'
    $stdout = Join-Path $scratch 'child.stdout'
    $stderr = Join-Path $scratch 'child.stderr'
    $child = [BhtuneSmokeProcess]::Start(
        $cmd, @('/d', '/s', '/c', 'echo native-stdout&echo native-stderr 1>&2'),
        $scratch, $stdout, $stderr
    )
    $stopped = $false
    if (-not $child.Wait(10000) -or $child.ExitCode -ne 0 -or
        [IO.File]::ReadAllText($stdout).Trim() -cne 'native-stdout' -or
        [IO.File]::ReadAllText($stderr).Trim() -cne 'native-stderr') {
        throw 'Native cmd launch, deadline, or redirected output regression failed.'
    }
    $child.Stop()
    $stopped = $true
    $child.Dispose()
    $child = $null
    Write-Host 'PASS native-cmd-launch-and-output'

    $child = [BhtuneSmokeProcess]::Start(
        $cmd, @('/d', '/s', '/c', 'for /L %I in (1,1,1000000000) do @rem'),
        $scratch, $stdout, $stderr
    )
    $stopped = $false
    if ($child.Wait(100)) { throw 'The harmless bounded cmd child exited before the cleanup check.' }
    $child.Stop()
    $stopped = $true
    if (-not $child.Wait(0)) { throw 'The stopped native child was not reaped.' }
    $child.Dispose()
    $child = $null
    Write-Host 'PASS native-cmd-deadline-job-cleanup'

    try {
        $child = [BhtuneSmokeProcess]::Start(
            (Join-Path $scratch 'missing-child.exe'), @(), $scratch, $stdout, $stderr
        )
        $stopped = $false
        throw 'A missing executable unexpectedly launched.'
    } catch {
        $cause = $_.Exception
        while ($null -ne $cause.InnerException) { $cause = $cause.InnerException }
        if ($cause -isnot [ComponentModel.Win32Exception] -or
            $cause.Message -notlike 'CreateProcessW(*failed (Win32 *') { throw }
    }
    Write-Host 'PASS native-launch-error-identifies-call'
} finally {
    if ($null -ne $child) {
        try { $child.Stop(); $stopped = $true }
        finally { $child.Dispose() }
    }
    if (-not $stopped) { throw "Native cleanup is incomplete; owned scratch retained: $scratch" }
    if ([IO.File]::ReadAllText((Join-Path $scratch '.owner')) -cne $runId -or
        (Get-Item -LiteralPath $scratch -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or
        @(Get-ChildItem -LiteralPath $scratch -Recurse -Force | Where-Object {
            $_.Attributes -band [IO.FileAttributes]::ReparsePoint
        }).Count -ne 0) {
        throw 'Native regression scratch ownership changed; refusing deletion.'
    }
    Remove-Item -LiteralPath $scratch -Recurse -Force
}
