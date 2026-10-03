# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

$script:InstallerSchemaVersion = 3
$script:SupportedInstallerSchemaVersions = @(2, 3)
$script:InstallerProductName = 'BHTune'
$script:InstallerPublisher = 'ByteHound Corp.'
$script:InstallerServiceName = 'BhtuneServer'
$script:InstallerServiceDisplayName = 'BHTune Server'
$script:InstallerServiceAccount = 'NT AUTHORITY\LocalService'
$script:InstallerBindAddress = '127.0.0.1:8787'
$script:InstallerStartUri = 'http://127.0.0.1:8787'
$script:InstallerHealthUri = 'http://127.0.0.1:8787/api/health'
$script:GatewayServiceName = 'OpcdaBridgeGateway'
$script:GatewayServiceDisplayName = 'OPC DA Bridge Gateway'
$script:GatewayServiceDescription = 'Bridges native OPC DA (COM/DCOM) tags to opcda-bridge clients over the network. https://github.com/bytehound-labs/opcda-bridge'
$script:GatewayPort = 7600
$script:InstallerMarkerPath = 'HKLM:\Software\ByteHound\bhtune'
$script:InstallerUninstallPath = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\BHTune'
$script:RequiredPayloadFiles = @('bhtune.exe', 'bhtune-server.exe', 'LICENSE', 'README.md')
$script:GatewayRequiredPayloadFiles = @(
    'opcda-bridge-gateway.exe',
    'opcda-gateway-release.json',
    'opcda-gateway-provenance.json',
    'LICENSE-opcda-bridge.txt',
    'NOTICE-opcda-bridge.txt'
)
$script:InstallerLifecycleTestId = ''
$script:InstallerLifecycleTestRoot = ''
$script:InstallerLifecycleTestActive = $false
