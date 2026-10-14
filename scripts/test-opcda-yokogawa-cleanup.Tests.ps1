#Requires -Version 7.4
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    (Join-Path $PSScriptRoot 'test-opcda-yokogawa.ps1'), [ref]$tokens, [ref]$errors
)
if ($errors.Count -gt 0) { throw ($errors | Out-String) }
$mainTry = @($ast.EndBlock.Statements | Where-Object {
    $_ -is [System.Management.Automation.Language.TryStatementAst]
})[-1]
$statements = foreach ($statement in $mainTry.Finally.Statements) {
    if ($statement -is [System.Management.Automation.Language.ForEachStatementAst]) { break }
    $statement
}
# Run the actual cleanup caller/control flow, replacing only the native registry opener.
$registryOpen = @($mainTry.Finally.FindAll({
    param($node)
    $node -is [System.Management.Automation.Language.InvokeMemberExpressionAst] -and
        $node.Member.Value -eq 'OpenBaseKey'
}, $true))
if ($registryOpen.Count -ne 1) { throw 'Expected one registry opener in the runner cleanup.' }
$cleanup = [scriptblock]::Create(
    (($statements.Extent.Text -join "`n").Replace($registryOpen[0].Extent.Text, '$registry'))
)
$assertRule = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq 'Assert-SmokeFirewallRule'
}, $true)
. ([scriptblock]::Create($assertRule.Extent.Text))

function Stop-SmokeProcess {
    param($Entry)
    $events.Add("stop:$($Entry.pid)")
    if ($case.stopFails -and $Entry.pid -eq 41) { throw 'Mock process tree remains running.' }
    $Entry.stopped = $true
}

function Get-NetFirewallRule {
    [CmdletBinding()]
    param([string]$PolicyStore)
    $events.Add("query:$PolicyStore")
    if ($state.rulePresent) { $rule }
}

function Get-NetFirewallApplicationFilter {
    [CmdletBinding()]
    param([Parameter(ValueFromPipeline)]$InputObject)
    process {
        $events.Add('verify:application')
        [pscustomobject]@{ Program = $firewall.program }
    }
}

function Remove-NetFirewallRule {
    [CmdletBinding()]
    param($InputObject)
    if (-not $allStopped -or -not [object]::ReferenceEquals($InputObject, $rule)) {
        throw 'Attempted unsafe mocked firewall removal.'
    }
    $events.Add('remove:firewall')
    $state.rulePresent = $false
}

foreach ($case in @(
    @{ name = 'failure-before-first-COM-key'; keys = 0; stopFails = $false },
    @{ name = 'normal-multiple-key-cleanup'; keys = 3; stopFails = $false },
    @{ name = 'process-stop-failure-retains-rule'; keys = 2; stopFails = $true }
)) {
    $runId = 'mock-run'
    $events = [Collections.Generic.List[string]]::new()
    $cleanupErrors = [Collections.Generic.List[string]]::new()
    $processes = @(
        [pscustomobject]@{ pid = 41; stopped = $false },
        [pscustomobject]@{ pid = 42; stopped = $false }
    )
    $firewall = @{
        attempted = $true; removed = $false; name = "BHTuneOpcDaSmoke-$runId"
        owner = $runId; program = 'mock-gateway.exe'
    }
    $state = @{ rulePresent = $true }
    $rule = [pscustomobject]@{ Name = $firewall.name; Description = $firewall.owner }
    $ownedKeys = @(for ($index = 0; $index -lt $case.keys; $index++) {
        @{ hive = 'CurrentUser'; path = "mock-key-$index"; values = @{ BHTuneSmokeOwner = $runId }; removed = $false }
    })
    $key = [pscustomobject]@{ Values = @{ BHTuneSmokeOwner = $runId }; SubKeyCount = 0; ValueCount = 1 }
    $key | Add-Member ScriptMethod GetValue { param($Name, $Default) $this.Values[$Name] }
    $key | Add-Member ScriptMethod GetValueKind { param($Name) [Microsoft.Win32.RegistryValueKind]::String }
    $key | Add-Member ScriptMethod Dispose {}
    $registry = [pscustomobject]@{ Key = $key }
    $registry | Add-Member ScriptMethod OpenSubKey { param($Path) $this.Key }
    $registry | Add-Member ScriptMethod DeleteSubKey { param($Path, $ThrowOnMissing) $events.Add("registry:$Path") }
    $registry | Add-Member ScriptMethod Dispose {}

    . $cleanup

    $queries = @($events | Where-Object { $_ -like 'query:*' }).Count
    $removals = @($events | Where-Object { $_ -eq 'remove:firewall' }).Count
    $verified = @($events | Where-Object { $_ -eq 'verify:application' }).Count
    $registryRemovals = @($events | Where-Object { $_ -like 'registry:*' }).Count
    if (($events[0..1] -join ',') -cne 'stop:42,stop:41' -or $registryRemovals -ne $case.keys -or
        @($ownedKeys | Where-Object { -not $_.removed }).Count -ne 0) {
        throw "$($case.name): cleanup did not stop the process trees before removing owned keys."
    }
    if ($case.stopFails) {
        if ($allStopped -or -not $state.rulePresent -or $firewall.removed -or $queries -ne 0 -or
            $removals -ne 0 -or $verified -ne 0 -or $cleanupErrors.Count -ne 2 -or
            $cleanupErrors[1] -notlike 'Firewall *retained*') {
            throw "$($case.name): firewall protection was not retained once with a clear cleanup error."
        }
    } elseif (-not $allStopped -or $state.rulePresent -or -not $firewall.removed -or
        $queries -ne 3 -or $removals -ne 1 -or $verified -ne 1 -or $cleanupErrors.Count -ne 0 -or
        $events.IndexOf('remove:firewall') -le $events.IndexOf('stop:41')) {
        throw "$($case.name): expected one ownership-verified firewall cleanup after process termination."
    }
    Write-Host "PASS $($case.name)"
}
