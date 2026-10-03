[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ExpectedVersion = '',

    [int]$TimeoutSeconds = 30,

    [int]$OverallTimeoutSeconds = 3600,

    [switch]$CustomDbBackupConfirmed,

    [switch]$Child,

    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{32}$')]
    [string]$LifecycleTestId,

    [Parameter(Mandatory = $true)]
    [string]$LifecycleTestRoot
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$modulePath = Join-Path (Split-Path -Parent $PSScriptRoot) 'BhtuneInstaller.psm1'
if (-not (Test-Path -LiteralPath $modulePath -PathType Leaf)) {
    throw "Installer module is missing: $modulePath"
}
$module = Import-Module -Name $modulePath -PassThru -ErrorAction Stop
$bodyPath = Join-Path $PSScriptRoot 'Run-NsisLifecycle.Body.ps1'
if (-not (Test-Path -LiteralPath $bodyPath -PathType Leaf)) {
    throw "NSIS lifecycle body is missing: $bodyPath"
}

$arguments = @{} + $PSBoundParameters
$arguments.RunnerScriptPath = $PSCommandPath
$moduleScope = $module.NewBoundScriptBlock({
    param(
        [Parameter(Mandatory = $true)]
        [string]$BodyPath,

        [Parameter(Mandatory = $true)]
        [hashtable]$BodyArguments
    )

    . $BodyPath @BodyArguments
})
& $moduleScope $bodyPath $arguments
