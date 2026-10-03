[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [ValidateSet('Install', 'Uninstall')]
    [string]$Mode = 'Install',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ExpectedVersion = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ReleaseTag = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$PayloadRoot = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$GatewayPayloadRoot = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$InstallerScriptRoot = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$UninstallerSource = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$InstallRoot = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ProgramDataRoot = '',

    [Parameter(Mandatory = $false)]
    [object]$AddToPath = '1',

    [Parameter(Mandatory = $false)]
    [object]$StartService = '1',

    [Parameter(Mandatory = $false)]
    [object]$InstallGateway = '1',

    [Parameter(Mandatory = $false)]
    [object]$StartGateway = '1',

    [Parameter(Mandatory = $false)]
    [object]$CustomDbBackupConfirmed = '0',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$TracePath = '',

    # These switches are intentionally not used by the NSIS release build.
    # They exist only for bounded acceptance tests and are rejected in normal
    # mode by Assert-FailureInjectionPolicy.
    [Parameter(Mandatory = $false)]
    [switch]$TestOnly,

    [Parameter(Mandatory = $false)]
    [ValidateSet('None', 'HealthMismatch', 'GatewaySmokeFailure', 'CommitFailure')]
    [string]$FailureInjection = 'None',

    [Parameter(Mandatory = $false)]
    [switch]$LeaveInstallRoot,

    [Parameter(Mandatory = $false)]
    [switch]$FinalizeUninstall,

    [Parameter(Mandatory = $false)]
    [int]$WaitForProcessId = 0,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$WaitForProcessPath = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$FinalizerRoot = '',

    [Parameter(Mandatory = $false)]
    [switch]$IsolatedLifecycleTest,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$LifecycleTestId = '',

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$LifecycleTestRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$modulePath = Join-Path $PSScriptRoot 'BhtuneInstaller.psm1'
if (-not (Test-Path -LiteralPath $modulePath -PathType Leaf)) {
    throw "Installer module is missing: $modulePath"
}
Import-Module -Name $modulePath -ErrorAction Stop

$exitCode = 1
Invoke-BhtuneInstaller @PSBoundParameters -EntryScriptPath $PSCommandPath -ExitCode ([ref]$exitCode)
exit $exitCode
