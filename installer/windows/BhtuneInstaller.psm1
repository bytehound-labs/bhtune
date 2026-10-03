Set-StrictMode -Version 2.0

$script:InstallerModuleRoot = $PSScriptRoot
$script:InstallerPrivateFiles = @(
    'Private/Installer.Constants.ps1'
    'Private/Installer.Common.ps1'
    'Private/Installer.Paths.ps1'
    'Private/Installer.Config.ps1'
    'Private/Installer.Payload.ps1'
    'Private/Installer.Services.ps1'
    'Private/Installer.Security.ps1'
    'Private/Installer.ModulePayload.ps1'
    'Private/Installer.Transaction.Common.ps1'
    'Private/Installer.Transaction.Recovery.ps1'
    'Private/Installer.Transaction.Install.ps1'
    'Private/Installer.Transaction.Uninstall.ps1'
    'Private/Installer.Transaction.Entry.ps1'
)

if (-not (Get-Variable -Name InstallerTracePath -Scope Script -ErrorAction SilentlyContinue)) {
    $script:InstallerTracePath = ''
}
if (-not (Get-Variable -Name InstallerScriptPath -Scope Script -ErrorAction SilentlyContinue)) {
    $script:InstallerScriptPath = ''
}

foreach ($relativePath in $script:InstallerPrivateFiles) {
    $privatePath = Join-Path $PSScriptRoot $relativePath
    if (-not (Test-Path -LiteralPath $privatePath -PathType Leaf)) {
        throw "Installer module source is missing: $privatePath"
    }
    . $privatePath
}

Export-ModuleMember -Function @(
    'Invoke-BhtuneInstaller',
    'Assert-GatewayReleaseContract',
    'Assert-GatewayPayloadLayout',
    'Assert-GatewayPayloadBinary'
)
