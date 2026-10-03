[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$modulePath = Join-Path (Split-Path -Parent $PSScriptRoot) 'BhtuneInstaller.psm1'
if (-not (Test-Path -LiteralPath $modulePath -PathType Leaf)) {
    throw "Installer module is missing: $modulePath"
}
$testWorkRoot = Join-Path $PSScriptRoot '.work'
$testWorkRootExisted = Test-Path -LiteralPath $testWorkRoot
$module = Import-Module -Name $modulePath -PassThru -ErrorAction Stop
if (-not $testWorkRootExisted -and (Test-Path -LiteralPath $testWorkRoot)) {
    throw 'Importing the installer module created test or product state.'
}
$testBodyPath = Join-Path $PSScriptRoot 'Test-InstallerModule.Body.ps1'
if (-not (Test-Path -LiteralPath $testBodyPath -PathType Leaf)) {
    throw "Installer module self-test body is missing: $testBodyPath"
}

$moduleScope = $module.NewBoundScriptBlock({
    param(
        [Parameter(Mandatory = $true)]
        [string]$TestBodyPath
    )

    . $TestBodyPath
})
& $moduleScope $testBodyPath
