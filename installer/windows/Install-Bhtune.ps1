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
    [string]$FinalizerRoot = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0
$script:InstallerTracePath = $TracePath
$script:InstallerScriptPath = $PSCommandPath

$helperPath = Join-Path $PSScriptRoot 'InstallerSupport.ps1'
if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
    throw "Installer helper is missing: $helperPath"
}
. $helperPath

$AddToPath = ConvertTo-InstallerBoolean -Value $AddToPath -Name 'AddToPath'
$StartService = ConvertTo-InstallerBoolean -Value $StartService -Name 'StartService'
$InstallGateway = ConvertTo-InstallerBoolean -Value $InstallGateway -Name 'InstallGateway'
$StartGateway = ConvertTo-InstallerBoolean -Value $StartGateway -Name 'StartGateway'
$CustomDbBackupConfirmed = ConvertTo-InstallerBoolean -Value $CustomDbBackupConfirmed -Name 'CustomDbBackupConfirmed'

function Get-SnapshotValue {
    param(
        [Parameter(Mandatory = $false)]
        [psobject]$Snapshot,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if ($null -eq $Snapshot) {
        return $null
    }

    if ($Snapshot -is [System.Collections.IDictionary]) {
        return $Snapshot[$Name]
    }

    $property = $Snapshot.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Write-TextFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [psobject]$Value
    )

    Write-TextFile -Path $Path -Content ($Value | ConvertTo-Json -Depth 12)
}

function Enter-InstallerTransactionLock {
    param(
        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $mutexName = 'Global\ByteHound.BHTune.Installer'
    $mutex = New-Object System.Threading.Mutex($false, $mutexName)
    $acquired = $false
    try {
        try {
            $acquired = $mutex.WaitOne([TimeSpan]::FromSeconds($TimeoutSeconds))
        } catch [System.Threading.AbandonedMutexException] {
            # An abandoned mutex is safe to take over.  The journal recovery
            # step below decides whether the interrupted transaction itself is
            # recoverable.
            $acquired = $true
        }
        if (-not $acquired) {
            $mutex.Dispose()
            throw "Another BHTune installer transaction is active (system-wide lock '$mutexName')."
        }
        return [pscustomobject]@{
            Mutex = $mutex
            Name  = $mutexName
        }
    } catch {
        if (-not $acquired) {
            $mutex.Dispose()
        }
        throw
    }
}

function Exit-InstallerTransactionLock {
    param(
        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [psobject]$Lock
    )

    if ($null -eq $Lock -or $null -eq $Lock.Mutex) {
        return
    }
    try {
        $Lock.Mutex.ReleaseMutex()
    } catch {
        # The process is already leaving; disposal below still releases the
        # kernel handle and an abandoned mutex is recoverable on next start.
    } finally {
        $Lock.Mutex.Dispose()
    }
}

function Get-ProcessSnapshotById {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId
    )

    try {
        return Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $ProcessId" -ErrorAction Stop
    } catch {
        throw "Unable to inspect process $ProcessId while finalizing the BHTune uninstall: $($_.Exception.Message)"
    }
}

function Wait-ForParentProcessExit {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ExpectedPath = '',

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    if ($ProcessId -le 0) {
        return
    }

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $identityChecked = $false
    while ([DateTime]::UtcNow -lt $deadline) {
        $snapshot = Get-ProcessSnapshotById -ProcessId $ProcessId
        if ($null -eq $snapshot) {
            Write-InstallerTrace -Stage 'uninstall.finalizer.parent-exited' -Detail ([string]$ProcessId)
            return
        }

        if (-not $identityChecked -and -not [string]::IsNullOrWhiteSpace($ExpectedPath)) {
            $actualPath = [string]$snapshot.ExecutablePath
            if (-not [string]::IsNullOrWhiteSpace($actualPath) -and
                (Normalize-PathForComparison -Path $actualPath) -ne
                (Normalize-PathForComparison -Path $ExpectedPath)) {
                throw "The uninstall finalizer process identity changed before cleanup: process $ProcessId is '$actualPath', not '$ExpectedPath'."
            }
            $identityChecked = $true
        }

        Start-Sleep -Milliseconds 250
    }

    throw "The NSIS uninstaller process $ProcessId did not exit within the bounded finalization wait."
}

function Get-CurrentProcessParentId {
    $snapshot = Get-ProcessSnapshotById -ProcessId $PID
    if ($null -eq $snapshot -or [int]$snapshot.ParentProcessId -le 0) {
        throw 'The current installer process has no verifiable parent process. Refusing to schedule asynchronous finalization.'
    }
    return [int]$snapshot.ParentProcessId
}

function ConvertTo-PowerShellProcessArgument {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    if ($Value -notmatch '[\s"]') {
        return $Value
    }
    return '"' + $Value.Replace('"', '\"') + '"'
}

function Start-UninstallFinalizer {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [int]$ParentProcessId = 0
    )

    if ($ParentProcessId -le 0) {
        $ParentProcessId = Get-CurrentProcessParentId
    }
    if ($ParentProcessId -le 0) {
        throw 'The uninstaller parent process could not be identified. Refusing to schedule asynchronous finalization.'
    }
    if ([string]::IsNullOrWhiteSpace($script:InstallerScriptPath) -or
        -not (Test-Path -LiteralPath $script:InstallerScriptPath -PathType Leaf)) {
        throw 'The uninstall orchestrator source is unavailable. Refusing to schedule asynchronous finalization.'
    }
    if ([string]::IsNullOrWhiteSpace($TracePath)) {
        throw 'Uninstall finalization requires a trace path.'
    }

    $finalizerRoot = Join-Path $Paths.InstallerStateRoot ("uninstall-finalizer-{0}" -f ([guid]::NewGuid().ToString('N')))
    Write-InstallerTrace -Stage 'uninstall.finalizer.stage.begin' -Detail $finalizerRoot
    New-Item -ItemType Directory -Path $finalizerRoot -Force | Out-Null
    Assert-NoReparsePointInPath -Path $finalizerRoot -Name 'the uninstall finalizer staging root'

    $finalizerScript = Join-Path $finalizerRoot 'Install-Bhtune.ps1'
    $finalizerHelper = Join-Path $finalizerRoot 'InstallerSupport.ps1'
    Copy-Item -LiteralPath $script:InstallerScriptPath -Destination $finalizerScript -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'InstallerSupport.ps1') -Destination $finalizerHelper -Force
    Write-InstallerTrace -Stage 'uninstall.finalizer.stage.complete' -Detail $finalizerRoot

    $powershellPath = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
    if (-not (Test-Path -LiteralPath $powershellPath -PathType Leaf)) {
        throw "Windows PowerShell is unavailable at '$powershellPath'."
    }

    $arguments = @(
        '-NoLogo',
        '-NoProfile',
        '-NonInteractive',
        '-ExecutionPolicy',
        'Bypass',
        '-File',
        (ConvertTo-PowerShellProcessArgument -Value $finalizerScript),
        '-Mode',
        'Uninstall',
        '-InstallRoot',
        (ConvertTo-PowerShellProcessArgument -Value $Paths.InstallRoot),
        '-ProgramDataRoot',
        (ConvertTo-PowerShellProcessArgument -Value $Paths.ProgramDataRoot),
        '-FinalizeUninstall',
        '-WaitForProcessId',
        [string]$ParentProcessId,
        '-WaitForProcessPath',
        (ConvertTo-PowerShellProcessArgument -Value $Paths.UninstallerPath),
        '-FinalizerRoot',
        (ConvertTo-PowerShellProcessArgument -Value $finalizerRoot),
        '-TracePath',
        (ConvertTo-PowerShellProcessArgument -Value $TracePath)
    )

    try {
        Write-InstallerTrace -Stage 'uninstall.finalizer.launch.begin' -Detail $finalizerRoot
        $process = Start-Process `
            -FilePath $powershellPath `
            -ArgumentList $arguments `
            -WindowStyle Hidden `
            -PassThru `
            -ErrorAction Stop
        Write-InstallerTrace -Stage 'uninstall.finalizer.launch.returned' -Detail ("pid={0}" -f $process.Id)
    } catch {
        throw "Unable to start the bounded uninstall finalizer: $($_.Exception.Message)"
    }

    Write-InstallerTrace -Stage 'uninstall.finalizer.started' -Detail ("pid={0}; parent={1}; root={2}" -f `
            $process.Id, $ParentProcessId, $finalizerRoot)
}

function Assert-InstallerOwnershipMarker {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Marker,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ExpectedVersion = ''
    )

    if ($null -eq $Marker) {
        throw 'The installer ownership marker is missing.'
    }

    $required = @(
        'SchemaVersion',
        'InstallerOwned',
        'InstallDir',
        'Version',
        'ServiceName',
        'ConfigPath',
        'DatabasePath',
        'PathEntry',
        'PathManaged',
        'ShortcutPath'
    )
    foreach ($name in $required) {
        if ($null -eq (Get-SnapshotValue -Snapshot $Marker.Values -Name $name)) {
            throw "The installer ownership marker is missing '$name'."
        }
    }

    $schemaVersion = [int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'SchemaVersion')
    if (-not (Test-SupportedInstallerSchemaVersion -Version $schemaVersion)) {
        throw 'The installer ownership marker has an unsupported schema version.'
    }
    if ([int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'InstallerOwned') -ne 1) {
        throw 'The installer ownership marker does not assert installer ownership.'
    }

    $markerInstall = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'InstallDir')
    $markerConfig = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'ConfigPath')
    $markerDatabase = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'DatabasePath')
    $markerPathEntry = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'PathEntry')
    $markerShortcut = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'ShortcutPath')
    if ((Normalize-PathForComparison -Path $markerInstall) -ne (Normalize-PathForComparison -Path $Paths.InstallRoot) -or
        (Normalize-PathForComparison -Path $markerConfig) -ne (Normalize-PathForComparison -Path $Paths.ConfigPath) -or
        (Normalize-PathForComparison -Path $markerDatabase) -ne (Normalize-PathForComparison -Path $Paths.DatabasePath) -or
        (Normalize-PathForComparison -Path $markerPathEntry) -ne (Normalize-PathForComparison -Path $Paths.InstallRoot) -or
        (Normalize-PathForComparison -Path $markerShortcut) -ne (Normalize-PathForComparison -Path $Paths.ShortcutPath)) {
        throw 'The installer ownership marker points at paths that do not match the fixed BHTune installation.'
    }
    if ([string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'ServiceName') -cne $Paths.ServiceName) {
        throw 'The installer ownership marker names a different service.'
    }

    $version = ConvertTo-NormalizedVersion -Version ([string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'Version'))
    if (-not [string]::IsNullOrWhiteSpace($ExpectedVersion) -and
        $version -ne (ConvertTo-NormalizedVersion -Version $ExpectedVersion)) {
        throw "The installer ownership marker version '$version' does not match the journaled version '$ExpectedVersion'."
    }

    $gatewayState = Get-GatewayMarkerState -Paths $Paths -Marker $Marker
    return [pscustomobject]@{
        Version        = $version
        PathManaged    = [bool]([int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'PathManaged'))
        GatewayManaged = $gatewayState.Managed
        GatewayVersion = $gatewayState.Version
        GatewaySha256  = $gatewayState.Sha256
    }
}

function Get-GatewayMarkerState {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Marker
    )

    $schemaVersion = [int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'SchemaVersion')
    if ($schemaVersion -lt 3) {
        return [pscustomobject]@{
            Managed = $false
            Version = $null
            Sha256  = $null
        }
    }

    foreach ($name in @(
            'GatewayManaged',
            'GatewayVersion',
            'GatewaySha256',
            'GatewayExecutable',
            'GatewayConfigPath',
            'GatewayServiceName',
            'GatewayPort'
        )) {
        if ($null -eq (Get-SnapshotValue -Snapshot $Marker.Values -Name $name)) {
            throw "The schema-3 installer ownership marker is missing '$name'."
        }
    }

    $managedValue = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayManaged')
    if ($managedValue -ne '0' -and $managedValue -ne '1') {
        throw 'The installer ownership marker has an invalid GatewayManaged value.'
    }
    $managed = $managedValue -eq '1'
    $version = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayVersion')
    $sha256 = [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewaySha256')

    if ((Normalize-PathForComparison -Path ([string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayExecutable'))) -ne
        (Normalize-PathForComparison -Path $Paths.GatewayExecutable) -or
        (Normalize-PathForComparison -Path ([string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayConfigPath'))) -ne
        (Normalize-PathForComparison -Path $Paths.GatewayConfigPath) -or
        [string](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayServiceName') -cne $Paths.GatewayServiceName -or
        [int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'GatewayPort') -ne [int]$Paths.GatewayPort) {
        throw 'The installer ownership marker does not describe the fixed OPC DA gateway paths and service.'
    }

    if (-not $managed) {
        if (-not [string]::IsNullOrWhiteSpace($version) -or
            -not [string]::IsNullOrWhiteSpace($sha256)) {
            throw 'The installer ownership marker records gateway identity while GatewayManaged is disabled.'
        }
        return [pscustomobject]@{
            Managed = $false
            Version = $null
            Sha256  = $null
        }
    }

    if (-not (Test-StableVersion -Version $version) -or
        $sha256 -cnotmatch '^[0-9a-f]{64}$') {
        throw 'The installer ownership marker has invalid gateway version or SHA-256 metadata.'
    }

    return [pscustomobject]@{
        Managed = $true
        Version = ConvertTo-NormalizedVersion -Version $version
        Sha256  = $sha256
    }
}

function Assert-InstallerMetadataForRecovery {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Journal,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [psobject]$Marker,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [psobject]$Uninstall,

        [Parameter(Mandatory = $false)]
        [bool]$RequireJournalIdentity = $false
    )

    if ($null -eq $Marker -and $null -eq $Uninstall) {
        throw 'No installer metadata remains to validate.'
    }

    $journalVersion = [string](Get-SnapshotValue -Snapshot $Journal -Name 'Version')
    $journalInstallRoot = [string](Get-SnapshotValue -Snapshot $Journal -Name 'InstallRoot')
    $journalUninstallerPath = [string](Get-SnapshotValue -Snapshot $Journal -Name 'UninstallerPath')
    $hasJournalVersion = -not [string]::IsNullOrWhiteSpace($journalVersion)
    $hasAnyJournalIdentity = -not [string]::IsNullOrWhiteSpace($journalVersion) -or
        -not [string]::IsNullOrWhiteSpace($journalInstallRoot) -or
        -not [string]::IsNullOrWhiteSpace($journalUninstallerPath)
    $hasJournalIdentity = $hasJournalVersion -and
        -not [string]::IsNullOrWhiteSpace($journalInstallRoot) -and
        -not [string]::IsNullOrWhiteSpace($journalUninstallerPath)
    if ($hasAnyJournalIdentity -and -not $hasJournalIdentity) {
        throw 'The transaction journal contains incomplete installer identity metadata.'
    }
    if ($RequireJournalIdentity -and $null -eq $Marker -and -not $hasJournalIdentity) {
        throw 'The interrupted uninstall is missing the journaled installer identity needed to validate remaining metadata safely.'
    }
    if (($hasJournalIdentity -and
            (Normalize-PathForComparison -Path $journalInstallRoot) -ne (Normalize-PathForComparison -Path $Paths.InstallRoot)) -or
        ($hasJournalIdentity -and
            (Normalize-PathForComparison -Path $journalUninstallerPath) -ne (Normalize-PathForComparison -Path $Paths.UninstallerPath))) {
        throw 'The transaction journal points at paths that do not match the fixed BHTune installation.'
    }

    $markerState = $null
    if ($null -ne $Marker) {
        $markerState = Assert-InstallerOwnershipMarker `
            -Paths $Paths `
            -Marker $Marker `
            -ExpectedVersion $(if ($hasJournalIdentity) { $journalVersion } else { '' })
    }

    $version = if ($null -ne $markerState) {
        $markerState.Version
    } elseif ($hasJournalVersion) {
        ConvertTo-NormalizedVersion -Version $journalVersion
    } else {
        throw 'The remaining installer metadata has no independently verifiable version.'
    }

    if ($null -ne $Uninstall -and
        -not (Test-UninstallMetadata `
                -Snapshot $Uninstall `
                -Version $version `
                -InstallRoot $Paths.InstallRoot `
                -UninstallerPath $Paths.UninstallerPath)) {
        throw 'The remaining Windows uninstall metadata does not match the installer-owned BHTune paths and version.'
    }

    return [pscustomobject]@{
        Version     = $version
        PathManaged = if ($null -ne $markerState) { $markerState.PathManaged } else { $false }
        Marker      = $Marker
        Uninstall   = $Uninstall
    }
}

function Read-TransactionJournal {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $path = Join-Path $Paths.InstallerStateRoot 'transaction.json'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        return $null
    }
    try {
        $journal = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    } catch {
        throw "The installer transaction journal '$path' is corrupt and cannot be recovered: $($_.Exception.Message)"
    }
    if ($null -eq $journal -or
        $null -eq $journal.PSObject.Properties['SchemaVersion'] -or
        $null -eq $journal.PSObject.Properties['Mode'] -or
        $null -eq $journal.PSObject.Properties['Phase']) {
        throw "The installer transaction journal '$path' is incomplete and cannot be recovered safely."
    }
    if (-not (Test-SupportedInstallerSchemaVersion -Version ([int]$journal.SchemaVersion))) {
        throw "The installer transaction journal '$path' has an unsupported schema version."
    }
    Assert-TransactionJournalShape -Paths $Paths -Journal $journal
    return $journal
}

function Recover-InterruptedTransaction {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [bool]$FinalizeUninstall = $false
    )

    $journal = Read-TransactionJournal -Paths $Paths
    if ($null -eq $journal) {
        return $null
    }

    $mode = [string]$journal.Mode
    $phase = [string]$journal.Phase
    Write-InstallerTrace -Stage 'journal.recover.begin' -Detail ("mode={0}; phase={1}" -f $mode, $phase)

    if ($mode -eq 'Install' -or $mode -eq 'Upgrade') {
        $backupRoot = if ($journal.PSObject.Properties['BackupRoot']) { [string]$journal.BackupRoot } else { '' }
        if (-not [string]::IsNullOrWhiteSpace($backupRoot)) {
            if (-not (Test-RollbackBackup -BackupRoot $backupRoot)) {
                throw "The interrupted installer transaction has an unverifiable rollback backup '$backupRoot'. Refusing to guess."
            }
            Write-InstallerTrace -Stage 'journal.recover.rollback.begin' -Detail $backupRoot
            Restore-RollbackBackup `
                -Paths $Paths `
                -BackupRoot $backupRoot `
                -AllowPartialGatewayRegistration:($phase -eq 'ServiceCreatePending')
            Remove-TransactionJournal -Paths $Paths
            Write-InstallerTrace -Stage 'journal.recover.rollback.end' -Detail $backupRoot
            return [pscustomobject]@{ Action = 'RolledBack'; Journal = $journal }
        }

        if ($mode -eq 'Install') {
            return Recover-CleanInstallTransaction -Paths $Paths -Journal $journal
        }

        if ($phase -eq 'Preflight' -or $phase -eq 'PathSnapshotted' -or
            $phase -eq 'ServiceStopPending' -or $phase -eq 'ServiceStopped') {
            # An upgrade must have a verified backup before any replacement
            # phase.  These early phases only need to restore a service that
            # this transaction explicitly stopped; the ownership marker and
            # fixed-path checks remain authoritative on the next attempt.
            $serviceWasPresent = [bool](Get-SnapshotValue -Snapshot $journal -Name 'ServiceWasPresent')
            $serviceWasRunning = [bool](Get-SnapshotValue -Snapshot $journal -Name 'ServiceWasRunning')
            $current = Get-ServiceSnapshot -Name $Paths.ServiceName
            if ($serviceWasRunning) {
                if ($null -eq $current -or
                    -not (Test-OwnedServiceSnapshot -ServiceSnapshot $current -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                    throw 'The interrupted upgrade stopped an owned service that is no longer safely restorable.'
                }
                if ($current.State -eq 'Stopped') {
                    Start-InstallerService | Out-Null
                }
            } elseif ($serviceWasPresent -and $null -eq $current) {
                throw 'The interrupted upgrade lost its previously existing service before backup promotion.'
            }
            $gatewayManaged = [bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayManaged')
            $gatewayWasManaged = [bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayWasManaged')
            $gatewayServiceWasPresent = [bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayServiceWasPresent')
            $gatewayServiceWasRunning = [bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayServiceWasRunning')
            if ($gatewayManaged) {
                $currentGateway = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
                if ($gatewayWasManaged -and $gatewayServiceWasRunning) {
                    if ($null -eq $currentGateway -or
                        -not (Test-OwnedGatewayServiceSnapshot `
                                -ServiceSnapshot $currentGateway `
                                -ExecutablePath $Paths.GatewayExecutable `
                                -ConfigPath $Paths.GatewayConfigPath `
                                -LogDirectory $Paths.GatewayLogDirectory)) {
                        throw 'The interrupted upgrade stopped an owned OPC DA gateway service that is no longer safely restorable.'
                    }
                    if ($currentGateway.State -eq 'Stopped') {
                        Start-InstallerGatewayService -Paths $Paths | Out-Null
                        Invoke-GatewaySmokeCheck -Paths $Paths | Out-Null
                    }
                } elseif ($gatewayServiceWasPresent -and $null -eq $currentGateway) {
                    throw 'The interrupted upgrade lost its previously existing OPC DA gateway service before backup promotion.'
                } elseif (-not $gatewayWasManaged -and $null -ne $currentGateway) {
                    throw 'The interrupted upgrade found an unexpected OPC DA gateway service before backup promotion.'
                }

                Remove-InstallerCreatedGatewayConfig `
                    -Paths $Paths `
                    -ConfigWasPresent ([bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayConfigWasPresent')) `
                    -ConfigCreationPending ([bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayConfigCreationPending')) `
                    -ConfigWasCreated ([bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayConfigWasCreated')) `
                    -ExpectedConfigHash ([string](Get-SnapshotValue -Snapshot $journal -Name 'ExpectedGatewayConfigHash')) `
                    -CreatedConfigHash ([string](Get-SnapshotValue -Snapshot $journal -Name 'CreatedGatewayConfigHash'))
                if (-not [bool](Get-SnapshotValue -Snapshot $journal -Name 'GatewayProgramDataRootWasPresent') -and
                    (Test-Path -LiteralPath $Paths.GatewayProgramDataRoot)) {
                    Assert-NoReparsePointInPath `
                        -Path $Paths.GatewayProgramDataRoot `
                        -Name 'the transaction-created OPC DA gateway ProgramData root'
                    Remove-Item -LiteralPath $Paths.GatewayProgramDataRoot -Recurse -Force
                }
            }
            Remove-TransactionJournal -Paths $Paths
            Write-InstallerTrace -Stage 'journal.recover.discard' -Detail $phase
            return [pscustomobject]@{ Action = 'Discarded'; Journal = $journal }
        }

        throw "The interrupted $mode transaction is at unrecoverable phase '$phase' without a verified rollback backup. Refusing to mutate BHTune."
    }

    if ($mode -eq 'Uninstall') {
        $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
        $installExists = Test-Path -LiteralPath $Paths.InstallRoot
        $service = Get-ServiceSnapshot -Name $Paths.ServiceName
        $gatewayManaged = if ($journal.PSObject.Properties['GatewayManaged']) {
            [bool]$journal.GatewayManaged
        } else {
            $false
        }
        if ($null -ne $marker) {
            $markerState = Assert-InstallerOwnershipMarker -Paths $Paths -Marker $marker
            if ($journal.PSObject.Properties['GatewayManaged'] -and
                [bool]$markerState.GatewayManaged -ne $gatewayManaged) {
                throw 'The uninstall journal and ownership marker disagree about OPC DA gateway ownership.'
            }
            $gatewayManaged = [bool]$markerState.GatewayManaged
        }
        $gatewayService = if ($gatewayManaged) {
            Get-ServiceSnapshot -Name $Paths.GatewayServiceName
        } else {
            $null
        }
        if ($phase -eq 'ServiceStopPending') {
            if ($null -ne $service -and
                -not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.ServiceName)'. Refusing to guess."
            }
            if ($null -ne $gatewayService -and
                -not (Test-OwnedGatewayServiceSnapshot `
                        -ServiceSnapshot $gatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.GatewayServiceName)'. Refusing to guess."
            }
            if ($null -eq $service -and $null -eq $gatewayService) {
                $journal.Phase = 'ServiceRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemoved'
            } elseif (($null -ne $service -and $service.State -ne 'Stopped') -or
                ($null -ne $gatewayService -and $gatewayService.State -ne 'Stopped')) {
                $journal.Phase = 'Begin'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'Begin'
            } else {
                $journal.Phase = 'ServiceRemovePending'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemovePending'
            }
        }
        if ($phase -eq 'ServiceRemovePending') {
            $registrationExists = Invoke-ServiceRegistrationQuery -Name $Paths.ServiceName
            $gatewayRegistrationExists = $gatewayManaged -and
                (Invoke-ServiceRegistrationQuery -Name $Paths.GatewayServiceName)
            if (-not $registrationExists -and -not $gatewayRegistrationExists) {
                $journal.Phase = 'ServiceRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemoved'
            } else {
                if ($registrationExists) {
                    $service = Get-ServiceSnapshot -Name $Paths.ServiceName
                }
                if ($registrationExists -and ($null -eq $service -or
                    -not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath))) {
                    throw "The interrupted uninstall found an unowned or unverifiable service at '$($Paths.ServiceName)'. Refusing to guess."
                }
                if ($gatewayRegistrationExists) {
                    $gatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
                }
                if ($gatewayRegistrationExists -and ($null -eq $gatewayService -or
                    -not (Test-OwnedGatewayServiceSnapshot `
                            -ServiceSnapshot $gatewayService `
                            -ExecutablePath $Paths.GatewayExecutable `
                            -ConfigPath $Paths.GatewayConfigPath `
                            -LogDirectory $Paths.GatewayLogDirectory))) {
                    throw "The interrupted uninstall found an unowned or unverifiable service at '$($Paths.GatewayServiceName)'. Refusing to guess."
                }
            }
        }
        if ($phase -ne 'Begin' -and
            $phase -ne 'ServiceStopPending' -and
            $phase -ne 'ServiceRemovePending' -and
            ($null -ne $service -or $null -ne $gatewayService)) {
            if ($null -ne $service -and
                -not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.ServiceName)'. Refusing to guess."
            }
            if ($null -ne $gatewayService -and
                -not (Test-OwnedGatewayServiceSnapshot `
                        -ServiceSnapshot $gatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.GatewayServiceName)'. Refusing to guess."
            }
            # The journal claimed service removal, but the service is still
            # present.  Rewind only this idempotent step so the normal
            # ownership check and removal path can retry it.
            $journal.Phase = 'Begin'
            Write-TransactionJournal -Paths $Paths -Value $journal
            $phase = 'Begin'
        }
        if (($phase -eq 'PathUpdatePending' -or $phase -eq 'PathRemoved' -or $phase -eq 'ShortcutRemovePending' -or
                $phase -eq 'ShortcutRemoved' -or
                $phase -eq 'ReadyForProgramFilesCleanup' -or $phase -eq 'ProgramFilesRemoved') -and
            $null -ne $marker -and
                [string](Get-SnapshotValue -Snapshot $marker.Values -Name 'PathManaged') -eq '1') {
            $pathEntries = @(Split-PathList -PathList (Get-MachinePathSnapshot))
            $matchingPathEntries = @($pathEntries | Where-Object {
                    (Normalize-PathForComparison -Path $_) -eq (Normalize-PathForComparison -Path $Paths.InstallRoot)
                })
            if ($matchingPathEntries.Count -gt 1) {
                throw "Machine PATH contains multiple exact entries for '$($Paths.InstallRoot)'. Refusing to guess during interrupted uninstall recovery."
            }
            if ($phase -eq 'PathUpdatePending' -and $matchingPathEntries.Count -eq 0) {
                $journal.Phase = 'PathRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'PathRemoved'
            } elseif ($matchingPathEntries.Count -gt 0) {
                $journal.Phase = 'ServiceRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemoved'
            }
        } elseif ($phase -eq 'PathUpdatePending') {
            $journal.Phase = 'PathRemoved'
            Write-TransactionJournal -Paths $Paths -Value $journal
            $phase = 'PathRemoved'
        }
        if ($phase -eq 'ShortcutRemovePending') {
            if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
                (Test-Path -LiteralPath $Paths.ShortcutPath)) {
                if (-not (Test-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath)) {
                    throw "The interrupted uninstall found unexpected content at '$($Paths.ShortcutPath)'. Refusing to delete it."
                }
            } else {
                $journal.Phase = 'ShortcutRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ShortcutRemoved'
            }
        }
        if (($phase -eq 'ShortcutRemoved' -or $phase -eq 'ReadyForProgramFilesCleanup' -or
                $phase -eq 'ProgramFilesRemoved') -and
            -not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
            (Test-Path -LiteralPath $Paths.ShortcutPath)) {
            if (-not (Test-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath)) {
                throw "The interrupted uninstall found unexpected content at '$($Paths.ShortcutPath)'. Refusing to delete it."
            }
            $journal.Phase = 'PathRemoved'
            Write-TransactionJournal -Paths $Paths -Value $journal
            $phase = 'PathRemoved'
        }
        if ($phase -eq 'ProgramFilesRemovePending' -and -not $installExists) {
            $journal.Phase = 'ProgramFilesRemoved'
            Write-TransactionJournal -Paths $Paths -Value $journal
            $phase = 'ProgramFilesRemoved'
        }
        if ($FinalizeUninstall -and $phase -eq 'ReadyForProgramFilesCleanup' -and -not $installExists) {
            $journal.Phase = 'ProgramFilesRemoved'
            Write-TransactionJournal -Paths $Paths -Value $journal
            $phase = 'ProgramFilesRemoved'
        }
        if ($phase -eq 'ProgramFilesRemoved' -and -not $installExists -and $FinalizeUninstall) {
            $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath
            if ($null -ne $marker -or $null -ne $uninstall) {
                $journal.Phase = 'MetadataRemovePending'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'MetadataRemovePending'
            } else {
                Remove-TransactionJournal -Paths $Paths
                Write-InstallerTrace -Stage 'journal.recover.uninstall-finalized' -Detail $Paths.InstallRoot
                return [pscustomobject]@{ Action = 'Finalized'; Journal = $journal }
            }
        }
        if ($installExists -and
            (($FinalizeUninstall -and $phase -eq 'ReadyForProgramFilesCleanup') -or
             $phase -eq 'ProgramFilesRemoved' -or
             $phase -eq 'MetadataRemovePending' -or
             $phase -eq 'MetadataRemoved')) {
            throw "The uninstall journal claims Program Files cleanup completed, but '$($Paths.InstallRoot)' still exists. Refusing to remove ownership metadata."
        }
        if ($FinalizeUninstall -and $phase -eq 'MetadataRemovePending') {
            $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
            $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath
            if ($null -eq $marker -and $null -eq $uninstall) {
                $journal.Phase = 'MetadataRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'MetadataRemoved'
            } else {
                Assert-InstallerMetadataForRecovery `
                    -Paths $Paths `
                    -Journal $journal `
                    -Marker $marker `
                    -Uninstall $uninstall `
                    -RequireJournalIdentity:$true | Out-Null
                if ($null -ne $marker) {
                    Remove-RegistryKey -Path $Paths.MarkerPath
                }
                if ($null -ne $uninstall) {
                    Remove-RegistryKey -Path $Paths.UninstallKeyPath
                }
                $journal.Phase = 'MetadataRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'MetadataRemoved'
            }
        }
        if ($phase -eq 'MetadataRemoved') {
            Remove-TransactionJournal -Paths $Paths
            Write-InstallerTrace -Stage 'journal.recover.uninstall-finalized' -Detail $Paths.InstallRoot
            return [pscustomobject]@{ Action = 'Finalized'; Journal = $journal }
        }
        if ($phase -eq 'Begin' -or $phase -eq 'ServiceRemoved' -or
            $phase -eq 'ServiceStopPending' -or $phase -eq 'ServiceRemovePending' -or
            $phase -eq 'PathUpdatePending' -or $phase -eq 'PathRemoved' -or
            $phase -eq 'ShortcutRemovePending' -or $phase -eq 'ShortcutRemoved' -or
            $phase -eq 'ProgramFilesRemovePending' -or
            $phase -eq 'ReadyForProgramFilesCleanup' -or
            $phase -eq 'MetadataRemovePending' -or
            $phase -eq 'ProgramFilesRemoved') {
            # Uninstall is resumed by Invoke-ConservativeUninstall, which
            # treats completed phases as already done.
            return [pscustomobject]@{ Action = 'ResumeUninstall'; Journal = $journal }
        }
        throw "The interrupted uninstall journal has unrecoverable phase '$phase'. Refusing to guess."
    }

    throw "The installer transaction journal has unknown mode '$mode'. Refusing to mutate BHTune."
}

function Assert-InstallerState {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [bool]$AllowMissingService = $false,

        [Parameter(Mandatory = $false)]
        [bool]$AllowMissingInstallRoot = $false
    )

    $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
    $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath
    $service = Get-ServiceSnapshot -Name $Paths.ServiceName
    $registrationExists = Invoke-ServiceRegistrationQuery -Name $Paths.ServiceName
    $gatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
    $gatewayRegistrationExists = Invoke-ServiceRegistrationQuery -Name $Paths.GatewayServiceName
    # Treat any pre-existing filesystem object at the fixed root as a
    # conflicting installation.  A file, junction, or other non-directory
    # object must not be removed just because it is not a container.
    $installExists = Test-Path -LiteralPath $Paths.InstallRoot
    $markerOwned = $false

    if ($null -ne $marker) {
        foreach ($name in @(
                'SchemaVersion',
                'InstallerOwned',
                'InstallDir',
                'Version',
                'ServiceName',
                'ConfigPath',
                'DatabasePath',
                'PathEntry',
                'PathManaged',
                'ShortcutPath'
            )) {
            if ($null -eq $marker.Values[$name]) {
                throw "The ownership marker '$($Paths.MarkerPath)' is missing required value '$name'. Refusing to guess."
            }
        }

        $schemaVersion = [int](Get-SnapshotValue -Snapshot $marker.Values -Name 'SchemaVersion')
        if (-not (Test-SupportedInstallerSchemaVersion -Version $schemaVersion)) {
            throw "The ownership marker '$($Paths.MarkerPath)' has an unsupported schema version."
        }
        $installerOwnedValue = [string](Get-SnapshotValue -Snapshot $marker.Values -Name 'InstallerOwned')
        if ($installerOwnedValue -ne '1') {
            throw "The ownership marker '$($Paths.MarkerPath)' exists but is not an installer-owned BHTune marker. Refusing to adopt it."
        }
        $pathManagedValue = [string](Get-SnapshotValue -Snapshot $marker.Values -Name 'PathManaged')
        if ($pathManagedValue -ne '0' -and $pathManagedValue -ne '1') {
            throw "The ownership marker '$($Paths.MarkerPath)' has an invalid PathManaged value. Refusing to guess."
        }
        $markerOwned = $true

        $markerInstall = Get-SnapshotValue -Snapshot $marker.Values -Name 'InstallDir'
        $markerConfig = Get-SnapshotValue -Snapshot $marker.Values -Name 'ConfigPath'
        $markerService = Get-SnapshotValue -Snapshot $marker.Values -Name 'ServiceName'
        if ((Normalize-PathForComparison -Path $markerInstall) -ne (Normalize-PathForComparison -Path $Paths.InstallRoot) -or
            (Normalize-PathForComparison -Path $markerConfig) -ne (Normalize-PathForComparison -Path $Paths.ConfigPath) -or
            [string]$markerService -ne $Paths.ServiceName) {
            throw 'The installer ownership marker does not describe the fixed BHTune installation paths. Refusing to guess.'
        }
        if (-not $installExists -and -not $AllowMissingInstallRoot) {
            throw 'The installer ownership marker exists but the Program Files installation is missing.'
        }
        if ($installExists -and -not (Test-Path -LiteralPath $Paths.InstallRoot -PathType Container)) {
            throw "The installer ownership marker exists but the Program Files installation root is not a directory: $($Paths.InstallRoot)"
        }
        if ($installExists) {
            $installItem = Get-Item -LiteralPath $Paths.InstallRoot -Force
            if (($installItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "The installer-owned Program Files root is a reparse point: $($Paths.InstallRoot). Refusing to follow or replace it."
            }
        }
        if ($null -eq $service) {
            if ($registrationExists) {
                throw "Service '$($Paths.ServiceName)' remains registered but cannot be inspected. Refusing to guess."
            }
            if (-not $AllowMissingService) {
                throw "The installer ownership marker exists but service '$($Paths.ServiceName)' is missing."
            }
        } else {
            if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "Service '$($Paths.ServiceName)' is not the installer-owned LocalService definition. Refusing to reconfigure it."
            }
            if ([string]$service.State -ne 'Running' -and [string]$service.State -ne 'Stopped') {
                throw "Service '$($Paths.ServiceName)' is in unsupported state '$($service.State)'. Refusing to change it while it is transitioning."
            }
        }
        if ($null -eq $uninstall) {
            throw 'The installer ownership marker exists but the Windows uninstall metadata is missing.'
        }

        $version = ConvertTo-NormalizedVersion -Version ([string](Get-SnapshotValue -Snapshot $marker.Values -Name 'Version'))
        $markerDatabase = Get-SnapshotValue -Snapshot $marker.Values -Name 'DatabasePath'
        $markerPathEntry = Get-SnapshotValue -Snapshot $marker.Values -Name 'PathEntry'
        $markerShortcut = Get-SnapshotValue -Snapshot $marker.Values -Name 'ShortcutPath'
        if ((Normalize-PathForComparison -Path $markerDatabase) -ne (Normalize-PathForComparison -Path $Paths.DatabasePath) -or
            (Normalize-PathForComparison -Path $markerPathEntry) -ne (Normalize-PathForComparison -Path $Paths.InstallRoot) -or
            (Normalize-PathForComparison -Path $markerShortcut) -ne (Normalize-PathForComparison -Path $Paths.ShortcutPath)) {
            throw 'The installer ownership marker does not describe the fixed database, PATH, and shortcut locations. Refusing to guess.'
        }
        if (-not (Test-UninstallMetadata -Snapshot $uninstall -Version $version -InstallRoot $Paths.InstallRoot -UninstallerPath $Paths.UninstallerPath)) {
            throw 'The Windows uninstall metadata does not match the installer-owned BHTune installation.'
        }

        $gatewayState = Get-GatewayMarkerState -Paths $Paths -Marker $marker
        if ($gatewayState.Managed) {
            if ($installExists) {
                $gatewayPayload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $Paths.GatewayInstallRoot
                if ([string]$gatewayPayload.Contract.version -cne $gatewayState.Version -or
                    [string]$gatewayPayload.Contract.executable.sha256 -cne $gatewayState.Sha256) {
                    throw 'The installed OPC DA gateway payload does not match the installer ownership marker.'
                }
                if ((Get-FileSha256 -Path $Paths.GatewayExecutable) -cne $gatewayState.Sha256) {
                    throw 'The installed OPC DA gateway executable hash does not match the installer ownership marker.'
                }
            }
            if ($null -eq $gatewayService) {
                if ($gatewayRegistrationExists) {
                    throw "Service '$($Paths.GatewayServiceName)' remains registered but cannot be inspected. Refusing to guess."
                }
                if (-not $AllowMissingService) {
                    throw "The installer ownership marker owns service '$($Paths.GatewayServiceName)', but the service is missing."
                }
            } else {
                if (-not (Test-OwnedGatewayServiceSnapshot `
                        -ServiceSnapshot $gatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                    throw "Service '$($Paths.GatewayServiceName)' is not the installer-owned LocalSystem definition. Refusing to reconfigure it."
                }
                if ([string]$gatewayService.State -ne 'Running' -and [string]$gatewayService.State -ne 'Stopped') {
                    throw "Service '$($Paths.GatewayServiceName)' is in unsupported state '$($gatewayService.State)'. Refusing to change it while it is transitioning."
                }
            }
        } elseif (Test-Path -LiteralPath $Paths.GatewayInstallRoot) {
            throw "The fixed gateway installation directory '$($Paths.GatewayInstallRoot)' exists without installer ownership. Refusing to overwrite or delete it."
        }

        return [pscustomobject]@{
            IsUpgrade            = $true
            Marker               = $marker
            Uninstall            = $uninstall
            Service              = $service
            ServiceMissing       = $null -eq $service
            Version              = $version
            PathManaged          = ([int](Get-SnapshotValue -Snapshot $marker.Values -Name 'PathManaged') -eq 1)
            ShortcutPath         = [string](Get-SnapshotValue -Snapshot $marker.Values -Name 'ShortcutPath')
            GatewayManaged       = $gatewayState.Managed
            GatewayVersion       = $gatewayState.Version
            GatewaySha256        = $gatewayState.Sha256
            GatewayService       = $gatewayService
            GatewayServiceExists = $gatewayRegistrationExists
        }
    }

    if ($null -ne $service -or $registrationExists) {
        throw "Service '$($Paths.ServiceName)' already exists without an installer ownership marker. Stop and uninstall the manually registered service, preserve its data, and rerun the BHTune installer."
    }
    if ($installExists) {
        throw "The fixed installation directory '$($Paths.InstallRoot)' already exists but is not installer-owned. Refusing to overwrite it."
    }
    if ($null -ne $uninstall) {
        throw "Windows uninstall metadata exists without an ownership marker. Refusing to adopt the conflicting installation state."
    }
    if (-not [string]::IsNullOrWhiteSpace([string]$Paths.ShortcutPath) -and
        (Test-Path -LiteralPath $Paths.ShortcutPath)) {
        throw "The fixed Start Menu shortcut '$($Paths.ShortcutPath)' already exists without installer ownership. Refusing to overwrite it."
    }

    return [pscustomobject]@{
        IsUpgrade            = $false
        Marker               = $null
        Uninstall            = $null
        Service              = $null
        ServiceMissing       = $false
        Version              = $null
        PathManaged          = $false
        ShortcutPath         = $Paths.ShortcutPath
        GatewayManaged       = $false
        GatewayVersion       = $null
        GatewaySha256        = $null
        GatewayService       = $gatewayService
        GatewayServiceExists = $gatewayRegistrationExists
    }
}

function Ensure-InstallerDirectories {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [bool]$ManageGateway = $false
    )

    $directories = @(
            $Paths.ProgramDataRoot,
            $Paths.DatabaseDirectory,
            $Paths.LogDirectory,
            $Paths.InstallerStateRoot
        )
    if ($ManageGateway) {
        $directories += @(
            $Paths.GatewayProgramDataRoot,
            $Paths.GatewayDataDirectory,
            $Paths.GatewayLogDirectory
        )
    }

    foreach ($directory in $directories) {
        if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
            New-Item -ItemType Directory -Path $directory -Force | Out-Null
        }
    }
}

function Ensure-InstallerConfig {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$DefaultContent
    )

    if (Test-Path -LiteralPath $Paths.ConfigPath -PathType Container) {
        throw "The configuration path is a directory, not a file: $($Paths.ConfigPath)"
    }
    if (-not (Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf)) {
        $content = if ($null -ne $DefaultContent) {
            $DefaultContent
        } else {
            Get-DefaultConfigContent -DatabasePath $Paths.DatabasePath -LogDirectory $Paths.LogDirectory
        }
        Write-TextFile -Path $Paths.ConfigPath -Content $content
        return $true
    }

    return $false
}

function Ensure-InstallerGatewayConfig {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$DefaultContent
    )

    if (Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Container) {
        throw "The OPC DA gateway configuration path is a directory, not a file: $($Paths.GatewayConfigPath)"
    }
    if (-not (Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Leaf)) {
        $content = if ($null -ne $DefaultContent) {
            $DefaultContent
        } else {
            Get-DefaultGatewayConfigContent `
                -DatabasePath $Paths.GatewayDatabasePath `
                -LogDirectory $Paths.GatewayLogDirectory
        }
        Write-TextFile -Path $Paths.GatewayConfigPath -Content $content
        return $true
    }

    return $false
}

function Remove-InstallerCreatedGatewayConfig {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [bool]$ConfigWasPresent,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigCreationPending = $false,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigWasCreated = $false,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$ExpectedConfigHash,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$CreatedConfigHash
    )

    if ($ConfigWasPresent -or (-not $ConfigCreationPending -and -not $ConfigWasCreated)) {
        return
    }
    if (-not (Test-Path -LiteralPath $Paths.GatewayConfigPath)) {
        return
    }
    if (-not (Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Leaf)) {
        throw "The installer-created OPC DA gateway configuration path is not a file: $($Paths.GatewayConfigPath)"
    }

    $claimedHash = if ($ConfigCreationPending -and -not [string]::IsNullOrWhiteSpace($ExpectedConfigHash)) {
        $ExpectedConfigHash
    } else {
        $CreatedConfigHash
    }
    if ([string]::IsNullOrWhiteSpace($claimedHash) -or $claimedHash -notmatch '^[0-9a-fA-F]{64}$') {
        throw 'The installer-created OPC DA gateway configuration has no valid journaled content hash. Refusing to delete it.'
    }
    if ((Get-FileSha256 -Path $Paths.GatewayConfigPath) -ne $claimedHash.Trim().ToLowerInvariant()) {
        throw 'The installer-created OPC DA gateway configuration was changed after creation. Refusing to delete operator-owned content.'
    }

    Remove-Item -LiteralPath $Paths.GatewayConfigPath -Force
}

function Remove-InstallerCreatedConfig {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [bool]$ConfigWasPresent,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigCreationPending = $false,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigWasCreated = $false,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$ExpectedConfigHash,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$CreatedConfigHash
    )

    if ($ConfigWasPresent -or (-not $ConfigCreationPending -and -not $ConfigWasCreated)) {
        return
    }
    if (-not (Test-Path -LiteralPath $Paths.ConfigPath)) {
        return
    }
    if (-not (Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf)) {
        throw "The installer-created configuration path is not a file: $($Paths.ConfigPath)"
    }

    $claimedHash = if ($ConfigCreationPending -and -not [string]::IsNullOrWhiteSpace($ExpectedConfigHash)) {
        $ExpectedConfigHash
    } else {
        $CreatedConfigHash
    }
    if ([string]::IsNullOrWhiteSpace($claimedHash) -or $claimedHash -notmatch '^[0-9a-fA-F]{64}$') {
        throw 'The installer-created configuration has no valid journaled content hash. Refusing to delete it.'
    }

    $actualHash = Get-FileSha256 -Path $Paths.ConfigPath
    if ($actualHash.ToLowerInvariant() -ne $claimedHash.Trim().ToLowerInvariant()) {
        throw 'The installer-created configuration was changed after creation. Refusing to delete operator-owned content.'
    }
    Remove-Item -LiteralPath $Paths.ConfigPath -Force
}

function Assert-DatabasePolicy {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [bool]$IsUpgrade,

        [Parameter(Mandatory = $true)]
        [bool]$CustomDbBackupConfirmed,

        [Parameter(Mandatory = $false)]
        [bool]$PreservedData = $false
    )

    $policy = Get-DatabasePolicy -ConfigPath $Paths.ConfigPath -DefaultDatabasePath $Paths.DatabasePath
    if ($policy.Policy -eq 'Missing') {
        return [pscustomobject]@{
            Policy          = 'Default'
            DatabasePath    = $Paths.DatabasePath
            Reason          = 'The configuration file is absent; the installer will create a managed default configuration.'
            AutomaticBackup = $true
        }
    }
    if ($policy.Policy -eq 'Default') {
        return $policy
    }

    if (-not $CustomDbBackupConfirmed) {
        $phase = if ($IsUpgrade) { 'upgrade' } else { 'installation' }
        throw "The BHTune configuration cannot be proven to use the installer-managed ProgramData database ($($policy.Reason)). Refusing the $phase without /CUSTOM_DB_BACKUP_CONFIRMED=1 after an independent database backup."
    }

    Assert-ExternalDatabaseReady -DatabasePath $policy.DatabasePath
    $state = if ($PreservedData) { 'preserved ProgramData' } else { 'configuration' }
    Write-Warning "The configured database is not installer-managed. The installer will reuse the $state only after the independent backup acknowledgement and will preserve the external database without automatically backing it up or rolling it back."
    return $policy
}

function New-StagedPayload {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$SourcePayloadRoot,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$SourceGatewayPayloadRoot = '',

        [Parameter(Mandatory = $false)]
        [bool]$IncludeGateway = $false
    )

    Write-InstallerTrace -Stage 'payload.validate.begin' -Detail ("root={0}; expected={1}" -f $SourcePayloadRoot, $ExpectedVersion)
    Assert-PayloadLayout -PayloadRoot $SourcePayloadRoot | Out-Null
    Assert-PayloadBinaryVersions -PayloadRoot $SourcePayloadRoot -ExpectedVersion $ExpectedVersion | Out-Null

    $stageRoot = Join-Path $Paths.InstallerStateRoot ('candidate-' + [guid]::NewGuid().ToString('N'))
    $stagePayload = Join-Path $stageRoot 'payload'
    New-Item -ItemType Directory -Path $stagePayload -Force | Out-Null

    foreach ($name in (Get-RequiredPayloadFiles)) {
        $source = Join-Path $SourcePayloadRoot $name
        $destination = Join-Path $stagePayload $name
        Write-InstallerTrace -Stage 'payload.stage.file.begin' -Detail $name
        Copy-Item -LiteralPath $source -Destination $destination -Force
        if ((Get-FileSha256 -Path $source) -ne (Get-FileSha256 -Path $destination)) {
            throw "The staged payload copy did not verify for '$name'."
        }
        Write-InstallerTrace -Stage 'payload.stage.file.end' -Detail $name
    }

    $stageGatewayPayload = $null
    $gatewayPayload = $null
    if ($IncludeGateway) {
        if ([string]::IsNullOrWhiteSpace($SourceGatewayPayloadRoot)) {
            throw 'The installer must provide a gateway payload root when gateway installation is enabled.'
        }
        Write-InstallerTrace -Stage 'gateway-payload.validate.begin' -Detail $SourceGatewayPayloadRoot
        $gatewayPayload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $SourceGatewayPayloadRoot
        Assert-GatewayPayloadBinary -GatewayPayload $gatewayPayload | Out-Null
        $stageGatewayPayload = Join-Path $stageRoot 'gateway'
        New-Item -ItemType Directory -Path $stageGatewayPayload -Force | Out-Null
        foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
            $source = Join-Path $SourceGatewayPayloadRoot $name
            $destination = Join-Path $stageGatewayPayload $name
            Write-InstallerTrace -Stage 'gateway-payload.stage.file.begin' -Detail $name
            Copy-Item -LiteralPath $source -Destination $destination -Force
            if ((Get-FileSha256 -Path $source) -ne (Get-FileSha256 -Path $destination)) {
                throw "The staged OPC DA gateway payload copy did not verify for '$name'."
            }
            Write-InstallerTrace -Stage 'gateway-payload.stage.file.end' -Detail $name
        }
        $gatewayPayload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $stageGatewayPayload
        Write-InstallerTrace -Stage 'gateway-payload.validate.end' -Detail $stageGatewayPayload
    }

    Write-InstallerTrace -Stage 'payload.validate.end' -Detail $stageRoot
    return [pscustomobject]@{
        Root               = $stageRoot
        PayloadRoot        = $stagePayload
        GatewayPayloadRoot = $stageGatewayPayload
        GatewayPayload     = $gatewayPayload
    }
}

function Remove-StagedPayload {
    param(
        [Parameter(Mandatory = $false)]
        [psobject]$Stage
    )

    if ($null -ne $Stage -and (Test-Path -LiteralPath $Stage.Root)) {
        Write-InstallerTrace -Stage 'payload.cleanup.begin' -Detail $Stage.Root
        Remove-Item -LiteralPath $Stage.Root -Recurse -Force -ErrorAction SilentlyContinue
        Write-InstallerTrace -Stage 'payload.cleanup.end' -Detail $Stage.Root
    }
}

function Copy-FileToBackup {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourcePath,

        [Parameter(Mandatory = $true)]
        [string]$BackupFilesRoot,

        [Parameter(Mandatory = $true)]
        [string]$RelativePath,

        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.ArrayList]$Manifest
    )

    if (-not (Test-Path -LiteralPath $SourcePath -PathType Leaf)) {
        return
    }

    Assert-NoReparsePointInPath -Path $SourcePath -Name 'a rollback source file'
    $before = Get-FileStabilitySnapshot -Path $SourcePath
    $backupPath = Join-Path $BackupFilesRoot $RelativePath
    $parent = Split-Path -Parent $backupPath
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    Copy-Item -LiteralPath $SourcePath -Destination $backupPath -Force
    $after = Get-FileStabilitySnapshot -Path $SourcePath
    Assert-FileStabilitySnapshot -Path $SourcePath -Before $before -After $after
    $entry = New-HashManifestEntry -SourcePath $SourcePath -BackupPath $backupPath -RelativePath $RelativePath
    [void]$Manifest.Add($entry)
}

function Get-SafeBackupFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RootPath,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    Assert-NoReparsePointInPath -Path $RootPath -Name $Name
    $pending = New-Object System.Collections.Queue
    $pending.Enqueue((Get-Item -LiteralPath $RootPath -Force -ErrorAction Stop).FullName)

    while ($pending.Count -gt 0) {
        $directory = [string]$pending.Dequeue()
        Assert-NoReparsePointInPath -Path $directory -Name $Name
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Name '$($item.FullName)' is a reparse point. Refusing to traverse redirected content."
            }
            if ($item.PSIsContainer) {
                $pending.Enqueue($item.FullName)
            } else {
                Write-Output $item
            }
        }
    }
}

function Get-BackupAclState {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $state = [ordered]@{}
    foreach ($path in @(Get-InstallerAclTargets -Paths $Paths)) {
        $sddl = Get-AclSddl -Path $path
        if (-not [string]::IsNullOrWhiteSpace($sddl)) {
            $state[$path] = $sddl
        }
    }
    return $state
}

function New-RollbackBackup {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$PriorState,

        [Parameter(Mandatory = $true)]
        [psobject]$DatabasePolicy,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigWasCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$InstallRootWasPresent = $false,

        [Parameter(Mandatory = $false)]
        [bool]$ManageGateway = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayConfigWasCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayProgramDataRootWasPresent = $false
    )

    $pendingRoot = Join-Path $Paths.InstallerStateRoot ('rollback.pending-' + [guid]::NewGuid().ToString('N'))
    try {
        Write-InstallerTrace -Stage 'backup.begin' -Detail $pendingRoot
        if ($DatabasePolicy.Policy -eq 'Default') {
            Assert-BhtuneDatabaseQuiescent -DatabasePath $Paths.DatabasePath | Out-Null
        }
        $backupFilesRoot = Join-Path $pendingRoot 'files'
        New-Item -ItemType Directory -Path $backupFilesRoot -Force | Out-Null
        $manifest = New-Object System.Collections.ArrayList

        if (Test-Path -LiteralPath $Paths.InstallRoot -PathType Container) {
            foreach ($file in @(Get-SafeBackupFiles `
                        -RootPath $Paths.InstallRoot `
                        -Name 'the installer-owned Program Files tree')) {
                $relative = $file.FullName.Substring($Paths.InstallRoot.Length).TrimStart('\', '/')
                Write-InstallerTrace -Stage 'backup.file.begin' -Detail (Join-Path 'install' $relative)
                Copy-FileToBackup -SourcePath $file.FullName -BackupFilesRoot $backupFilesRoot -RelativePath (Join-Path 'install' $relative) -Manifest $manifest
                Write-InstallerTrace -Stage 'backup.file.end' -Detail (Join-Path 'install' $relative)
            }
        }
        if ($ManageGateway -and (Test-Path -LiteralPath $Paths.GatewayProgramDataRoot -PathType Container)) {
            foreach ($file in @(Get-SafeBackupFiles `
                        -RootPath $Paths.GatewayProgramDataRoot `
                        -Name 'the managed OPC DA gateway ProgramData tree')) {
                if ($GatewayConfigWasCreated -and
                    (Normalize-PathForComparison -Path $file.FullName) -eq
                    (Normalize-PathForComparison -Path $Paths.GatewayConfigPath)) {
                    continue
                }
                $relative = $file.FullName.Substring($Paths.GatewayProgramDataRoot.Length).TrimStart('\', '/')
                $backupRelative = Join-Path 'gatewaydata' $relative
                Write-InstallerTrace -Stage 'backup.file.begin' -Detail $backupRelative
                Copy-FileToBackup `
                    -SourcePath $file.FullName `
                    -BackupFilesRoot $backupFilesRoot `
                    -RelativePath $backupRelative `
                    -Manifest $manifest
                Write-InstallerTrace -Stage 'backup.file.end' -Detail $backupRelative
            }
        }

        if (-not $ConfigWasCreated) {
            Write-InstallerTrace -Stage 'backup.file.begin' -Detail 'programdata\bhtune.toml'
            Copy-FileToBackup -SourcePath $Paths.ConfigPath -BackupFilesRoot $backupFilesRoot -RelativePath 'programdata\bhtune.toml' -Manifest $manifest
            Write-InstallerTrace -Stage 'backup.file.end' -Detail 'programdata\bhtune.toml'
        }
        if ($DatabasePolicy.Policy -eq 'Default') {
            foreach ($suffix in @('', '-wal', '-shm')) {
                Write-InstallerTrace -Stage 'backup.file.begin' -Detail ('programdata\data\bhtune.db' + $suffix)
                Copy-FileToBackup -SourcePath ($Paths.DatabasePath + $suffix) -BackupFilesRoot $backupFilesRoot -RelativePath ('programdata\data\bhtune.db' + $suffix) -Manifest $manifest
                Write-InstallerTrace -Stage 'backup.file.end' -Detail ('programdata\data\bhtune.db' + $suffix)
            }
        }
        if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath)) {
            Write-InstallerTrace -Stage 'backup.file.begin' -Detail 'shortcut\BHTune.url'
            Copy-FileToBackup -SourcePath $Paths.ShortcutPath -BackupFilesRoot $backupFilesRoot -RelativePath 'shortcut\BHTune.url' -Manifest $manifest
            Write-InstallerTrace -Stage 'backup.file.end' -Detail 'shortcut\BHTune.url'
        }

        $markerValues = if ($null -ne $PriorState.Marker) { $PriorState.Marker.Values } else { $null }
        $uninstallValues = if ($null -ne $PriorState.Uninstall) { $PriorState.Uninstall.Values } else { $null }
        $priorVersion = if ($null -ne $PriorState.Version) { [string]$PriorState.Version } else { $null }
        $priorPathManaged = if ($null -ne $PriorState.PathManaged) { [bool]$PriorState.PathManaged } else { $false }
        $priorPathEntryWasPresent = @(
            (Split-PathList -PathList (Get-MachinePathSnapshot)) | Where-Object {
                (Normalize-PathForComparison -Path $_) -eq (Normalize-PathForComparison -Path $Paths.InstallRoot)
            }
        ).Count -gt 0
        $state = [ordered]@{
            SchemaVersion       = $script:InstallerSchemaVersion
            CreatedUtc          = [DateTime]::UtcNow.ToString('o')
            Version             = $priorVersion
            Service             = $PriorState.Service
            MarkerValues        = $markerValues
            UninstallValues     = $uninstallValues
            DatabasePolicy      = $DatabasePolicy
            ConfigPath          = $Paths.ConfigPath
            DatabasePath        = $Paths.DatabasePath
            ShortcutPath        = $Paths.ShortcutPath
            InstallRootWasPresent = $InstallRootWasPresent
            GatewayManaged       = $ManageGateway
            GatewayWasManaged    = [bool]$PriorState.GatewayManaged
            GatewayService       = $PriorState.GatewayService
            GatewayProgramDataRootWasPresent = $GatewayProgramDataRootWasPresent
            PathManaged         = $priorPathManaged
            PathEntryWasPresent = $priorPathEntryWasPresent
            Acls                = Get-BackupAclState -Paths $Paths
        }
        Write-JsonFile -Path (Join-Path $pendingRoot 'state.json') -Value ([pscustomobject]$state)

        $manifestObject = [ordered]@{
            SchemaVersion = $script:InstallerSchemaVersion
            CreatedUtc    = [DateTime]::UtcNow.ToString('o')
            Files         = @($manifest)
        }
        Write-JsonFile -Path (Join-Path $pendingRoot 'manifest.json') -Value ([pscustomobject]$manifestObject)

        Write-InstallerTrace -Stage 'backup.verify.begin' -Detail $pendingRoot
        if (-not (Test-RollbackBackup -BackupRoot $pendingRoot)) {
            throw 'The newly created rollback backup failed manifest verification.'
        }
        if ($DatabasePolicy.Policy -eq 'Default') {
            Assert-BhtuneDatabaseQuiescent -DatabasePath $Paths.DatabasePath | Out-Null
        }
        Write-InstallerTrace -Stage 'backup.verify.end' -Detail $pendingRoot

        Write-InstallerTrace -Stage 'backup.end' -Detail $pendingRoot
        return [pscustomobject]@{
            PendingRoot = $pendingRoot
            FinalRoot   = $Paths.RollbackRoot
        }
    } catch {
        Write-InstallerTrace -Stage 'backup.failure' -Detail $_.Exception.Message
        if (Test-Path -LiteralPath $pendingRoot) {
            Remove-Item -LiteralPath $pendingRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Update-RollbackManifestPaths {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BackupRoot
    )

    $manifestPath = Join-Path $BackupRoot 'manifest.json'
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    foreach ($entry in @($manifest.Files)) {
        $entry.BackupPath = Join-Path (Join-Path $BackupRoot 'files') $entry.RelativePath
    }
    Write-JsonFile -Path $manifestPath -Value $manifest
}

function Promote-RollbackBackup {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Backup
    )

    if (-not (Test-RollbackBackup -BackupRoot $Backup.PendingRoot)) {
        throw 'The pending rollback backup is not verified.'
    }

    $retiring = $null
    $newFinal = $false
    try {
        Write-InstallerTrace -Stage 'backup.promote.begin' -Detail $Backup.PendingRoot
        if (Test-Path -LiteralPath $Backup.FinalRoot) {
            $retiring = $Backup.FinalRoot + '.retiring-' + [guid]::NewGuid().ToString('N')
            Move-Item -LiteralPath $Backup.FinalRoot -Destination $retiring -Force
        }
        Move-Item -LiteralPath $Backup.PendingRoot -Destination $Backup.FinalRoot -Force
        $newFinal = $true
        Update-RollbackManifestPaths -BackupRoot $Backup.FinalRoot
        if (-not (Test-RollbackBackup -BackupRoot $Backup.FinalRoot)) {
            throw 'The promoted rollback backup failed verification.'
        }
        if ($null -ne $retiring -and (Test-Path -LiteralPath $retiring)) {
            Remove-Item -LiteralPath $retiring -Recurse -Force
        }
        Write-InstallerTrace -Stage 'backup.promote.end' -Detail $Backup.FinalRoot
    } catch {
        Write-InstallerTrace -Stage 'backup.promote.failure' -Detail $_.Exception.Message
        if ($newFinal -and (Test-Path -LiteralPath $Backup.FinalRoot)) {
            Remove-Item -LiteralPath $Backup.FinalRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
        if ($null -ne $retiring -and (Test-Path -LiteralPath $retiring)) {
            if (-not (Test-Path -LiteralPath $Backup.FinalRoot)) {
                Move-Item -LiteralPath $retiring -Destination $Backup.FinalRoot -Force
            }
        }
        if (Test-Path -LiteralPath $Backup.PendingRoot) {
            Remove-Item -LiteralPath $Backup.PendingRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
        throw
    }
}

function Resolve-RollbackManifestDestination {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$RelativePath
    )

    $relative = $RelativePath.Trim().Replace('/', '\')
    if (-not (Test-SafeRollbackRelativePath -RelativePath $relative)) {
        throw "Rollback manifest contains an unsafe relative path '$RelativePath'."
    }

    if ($relative.StartsWith('install\', [System.StringComparison]::OrdinalIgnoreCase)) {
        $child = $relative.Substring('install\'.Length)
        if ([string]::IsNullOrWhiteSpace($child)) {
            throw 'Rollback manifest cannot target the install directory itself.'
        }
        return Join-Path $Paths.InstallRoot $child
    }
    if ($relative.StartsWith('gatewaydata\', [System.StringComparison]::OrdinalIgnoreCase)) {
        $child = $relative.Substring('gatewaydata\'.Length)
        if ([string]::IsNullOrWhiteSpace($child)) {
            throw 'Rollback manifest cannot target the gateway ProgramData directory itself.'
        }
        return Join-Path $Paths.GatewayProgramDataRoot $child
    }

    $fixedDestinations = @{
        'programdata\bhtune.toml'       = $Paths.ConfigPath
        'programdata\data\bhtune.db'    = $Paths.DatabasePath
        'programdata\data\bhtune.db-wal' = $Paths.DatabasePath + '-wal'
        'programdata\data\bhtune.db-shm' = $Paths.DatabasePath + '-shm'
        'shortcut\bhtune.url'           = $Paths.ShortcutPath
    }
    $key = $relative.ToLowerInvariant()
    if (-not $fixedDestinations.ContainsKey($key) -or
        [string]::IsNullOrWhiteSpace([string]$fixedDestinations[$key])) {
        throw "Rollback manifest contains an unsupported destination '$RelativePath'."
    }
    return [string]$fixedDestinations[$key]
}

function Assert-RollbackStateAndManifest {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$BackupRoot,

        [Parameter(Mandatory = $true)]
        [psobject]$State,

        [Parameter(Mandatory = $true)]
        [psobject]$Manifest
    )

    if (-not (Test-TransactionJournalPath -Candidate $BackupRoot -Root $Paths.RollbackRoot -AllowRoot $true)) {
        throw "Rollback state is outside the managed rollback root: $BackupRoot"
    }
    if (-not $State.PSObject.Properties['InstallRootWasPresent']) {
        throw 'Rollback state does not record whether the prior install root existed. Refusing to guess.'
    }
    if (-not $State.PSObject.Properties['PathEntryWasPresent']) {
        throw 'Rollback state does not record exact PATH-entry ownership. Refusing to restore the complete machine PATH snapshot.'
    }
    if ([int]$State.SchemaVersion -ge 3) {
        foreach ($name in @(
                'GatewayManaged',
                'GatewayWasManaged',
                'GatewayProgramDataRootWasPresent'
            )) {
            if (-not $State.PSObject.Properties[$name]) {
                throw "Rollback state does not record '$name'. Refusing to guess."
            }
            $gatewayValue = $State.$name
            if ($gatewayValue -isnot [bool] -and
                [string]$gatewayValue -ne 'True' -and
                [string]$gatewayValue -ne 'False') {
                throw "Rollback state has a non-boolean '$name' value. Refusing to guess."
            }
        }
    }
    $installRootWasPresent = $State.InstallRootWasPresent
    if ($installRootWasPresent -isnot [bool] -and
        [string]$installRootWasPresent -ne 'True' -and
        [string]$installRootWasPresent -ne 'False') {
        throw 'Rollback state has a non-boolean InstallRootWasPresent value. Refusing to guess.'
    }

    foreach ($pathProperty in @('ConfigPath', 'DatabasePath', 'ShortcutPath')) {
        $recordedPath = [string]$State.$pathProperty
        $expectedPath = [string]$Paths.$pathProperty
        if ((Normalize-PathForComparison -Path $recordedPath) -ne
            (Normalize-PathForComparison -Path $expectedPath)) {
            throw "Rollback state path '$pathProperty' does not match the fixed installer path. Refusing to guess."
        }
    }

    if ($null -eq $State.DatabasePolicy -or
        [string]$State.DatabasePolicy.Policy -notin @('Default', 'External')) {
        throw 'Rollback state has an unknown database policy. Refusing to guess.'
    }
    if ($null -ne $State.Service -and
        -not (Test-OwnedServiceSnapshot `
                -ServiceSnapshot $State.Service `
                -ExecutablePath $Paths.ServiceExecutable `
                -ConfigPath $Paths.ConfigPath)) {
        throw 'Rollback state contains a conflicting service snapshot. Refusing to remove or restore it.'
    }
    if ($State.PSObject.Properties['GatewayService'] -and
        $null -ne $State.GatewayService -and
        -not (Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $State.GatewayService `
                -ExecutablePath $Paths.GatewayExecutable `
                -ConfigPath $Paths.GatewayConfigPath `
                -LogDirectory $Paths.GatewayLogDirectory)) {
        throw 'Rollback state contains a conflicting OPC DA gateway service snapshot. Refusing to remove or restore it.'
    }

    $allowedAclTargets = @{}
    foreach ($target in @(Get-InstallerAclTargets -Paths $Paths)) {
        $allowedAclTargets[(Normalize-PathForComparison -Path $target)] = $true
    }
    if ($null -ne $State.Acls) {
        foreach ($property in $State.Acls.PSObject.Properties) {
            $aclPath = Normalize-PathForComparison -Path $property.Name
            if (-not $allowedAclTargets.ContainsKey($aclPath) -or
                [string]::IsNullOrWhiteSpace([string]$property.Value)) {
                throw "Rollback state contains an unsupported ACL target '$($property.Name)'. Refusing to guess."
            }
            if (-not [bool]$installRootWasPresent -and
                ($aclPath -eq (Normalize-PathForComparison -Path $Paths.InstallRoot) -or
                 $aclPath.StartsWith((Normalize-PathForComparison -Path $Paths.InstallRoot).TrimEnd('\') + '\', [System.StringComparison]::OrdinalIgnoreCase))) {
                throw 'Rollback state contains install-root ACL data even though that root was previously absent.'
            }
        }
    }

    $seenSources = @{}
    $hasInstallEntry = $false
    $hasGatewayDataEntry = $false
    foreach ($entry in @($Manifest.Files)) {
        $relativePath = [string]$entry.RelativePath
        $destination = Resolve-RollbackManifestDestination -Paths $Paths -RelativePath $relativePath
        $sourcePath = Normalize-PathForComparison -Path ([string]$entry.SourcePath)
        $destinationPath = Normalize-PathForComparison -Path $destination
        if ([string]::IsNullOrWhiteSpace($sourcePath) -or $sourcePath -ne $destinationPath) {
            throw "Rollback manifest source path does not match its fixed destination '$relativePath'. Refusing to guess."
        }
        $sourceKey = $sourcePath.ToLowerInvariant()
        if ($seenSources.ContainsKey($sourceKey)) {
            throw "Rollback manifest contains duplicate destination '$relativePath'."
        }
        $seenSources[$sourceKey] = $true
        if ($relativePath.Replace('/', '\').StartsWith('install\', [System.StringComparison]::OrdinalIgnoreCase)) {
            $hasInstallEntry = $true
        }
        if ($relativePath.Replace('/', '\').StartsWith('gatewaydata\', [System.StringComparison]::OrdinalIgnoreCase)) {
            $hasGatewayDataEntry = $true
        }
        if ([string]$State.DatabasePolicy.Policy -eq 'External' -and
            $relativePath.Replace('/', '\').StartsWith('programdata\data\bhtune.db', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Rollback manifest contains managed database files while the recorded database policy is external.'
        }
    }
    if (-not [bool]$installRootWasPresent -and $hasInstallEntry) {
        throw 'Rollback manifest contains install files even though the prior install root was absent.'
    }
    if ($State.PSObject.Properties['GatewayProgramDataRootWasPresent'] -and
        -not [bool]$State.GatewayProgramDataRootWasPresent -and $hasGatewayDataEntry) {
        throw 'Rollback manifest contains gateway ProgramData files even though that root was previously absent.'
    }
    if ($State.PSObject.Properties['GatewayManaged'] -and
        -not [bool]$State.GatewayManaged -and $hasGatewayDataEntry) {
        throw 'Rollback manifest contains gateway ProgramData files even though the transaction did not manage the gateway.'
    }
}

function Restore-RollbackBackup {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$BackupRoot,

        [Parameter(Mandatory = $false)]
        [bool]$AllowPartialGatewayRegistration = $false
    )

    if (-not (Test-RollbackBackup -BackupRoot $BackupRoot)) {
        throw "Rollback backup verification failed: $BackupRoot"
    }

    $state = Get-Content -LiteralPath (Join-Path $BackupRoot 'state.json') -Raw | ConvertFrom-Json
    $manifest = Get-Content -LiteralPath (Join-Path $BackupRoot 'manifest.json') -Raw | ConvertFrom-Json
    Assert-RollbackStateAndManifest -Paths $Paths -BackupRoot $BackupRoot -State $state -Manifest $manifest

    $currentService = Get-ServiceSnapshot -Name $Paths.ServiceName
    if ($null -ne $currentService) {
        if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $currentService -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
            throw "Rollback found a conflicting service '$($Paths.ServiceName)'; refusing to remove an unowned service."
        }
        if ($currentService.State -ne 'Stopped') {
            Stop-InstallerService | Out-Null
        }
        Remove-ServiceByName -Name $Paths.ServiceName
    }
    if ($state.PSObject.Properties['GatewayManaged'] -and [bool]$state.GatewayManaged) {
        $currentGatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
        if ($null -ne $currentGatewayService) {
            $ownedGatewayService = Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $currentGatewayService `
                -ExecutablePath $Paths.GatewayExecutable `
                -ConfigPath $Paths.GatewayConfigPath `
                -LogDirectory $Paths.GatewayLogDirectory
            $partialCandidateService = $AllowPartialGatewayRegistration -and
                (Test-InstallerCreatedGatewayServiceSnapshot `
                    -ServiceSnapshot $currentGatewayService `
                    -ExecutablePath $Paths.GatewayExecutable `
                    -ConfigPath $Paths.GatewayConfigPath `
                    -LogDirectory $Paths.GatewayLogDirectory)
            if (-not $ownedGatewayService -and -not $partialCandidateService) {
                throw "Rollback found a conflicting service '$($Paths.GatewayServiceName)'; refusing to remove an unowned service."
            }
            if ($ownedGatewayService) {
                Stop-InstallerGatewayService -Paths $Paths | Out-Null
                Remove-InstallerGatewayService -Paths $Paths
            } else {
                Remove-InstallerCreatedGatewayServiceRegistration -Paths $Paths
            }
        }
    }

    if (Test-Path -LiteralPath $Paths.InstallRoot) {
        Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
    }
    if ([bool]$state.InstallRootWasPresent) {
        New-Item -ItemType Directory -Path $Paths.InstallRoot -Force | Out-Null
    }

    if ($state.DatabasePolicy.Policy -eq 'Default') {
        foreach ($path in @(
                $Paths.ConfigPath,
                $Paths.DatabasePath,
                $Paths.DatabasePath + '-wal',
                $Paths.DatabasePath + '-shm'
            )) {
            if (Test-Path -LiteralPath $path) {
                Remove-Item -LiteralPath $path -Force
            }
        }
    }
    if ($state.PSObject.Properties['GatewayManaged'] -and [bool]$state.GatewayManaged) {
        if (Test-Path -LiteralPath $Paths.GatewayProgramDataRoot) {
            Remove-Item -LiteralPath $Paths.GatewayProgramDataRoot -Recurse -Force
        }
        if ([bool]$state.GatewayProgramDataRootWasPresent) {
            New-Item -ItemType Directory -Path $Paths.GatewayProgramDataRoot -Force | Out-Null
        }
    }
    if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath)) {
        Remove-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
    }

    foreach ($entry in @($manifest.Files)) {
        $destination = Resolve-RollbackManifestDestination -Paths $Paths -RelativePath ([string]$entry.RelativePath)
        $parent = Split-Path -Parent $destination
        if (-not [string]::IsNullOrWhiteSpace($parent)) {
            New-Item -ItemType Directory -Path $parent -Force | Out-Null
        }
        Copy-Item -LiteralPath $entry.BackupPath -Destination $destination -Force
    }

    if ($null -ne $state.ShortcutPath -and (Test-Path -LiteralPath $state.ShortcutPath -PathType Leaf)) {
        # The shortcut is already restored by the manifest; this branch makes
        # the expected path explicit for an operator reading the transaction.
        $null = $state.ShortcutPath
    }

    $aclProperties = $state.Acls.PSObject.Properties
    foreach ($property in $aclProperties) {
        Restore-AclSddl -Path $property.Name -Sddl ([string]$property.Value)
    }

    Remove-RegistryKey -Path $Paths.MarkerPath
    Remove-RegistryKey -Path $Paths.UninstallKeyPath
    if ($null -ne $state.MarkerValues) {
        Set-RegistryValues -Path $Paths.MarkerPath -Values $state.MarkerValues
    }
    if ($null -ne $state.UninstallValues) {
        Set-RegistryValues -Path $Paths.UninstallKeyPath -Values $state.UninstallValues
    }

    Restore-ServiceSnapshot -Snapshot $state.Service
    if ($state.PSObject.Properties['GatewayManaged'] -and [bool]$state.GatewayManaged) {
        $gatewaySnapshot = if ($state.PSObject.Properties['GatewayService']) {
            $state.GatewayService
        } else {
            $null
        }
        Restore-GatewayServiceSnapshot -Paths $Paths -Snapshot $gatewaySnapshot
    }
    if ($null -eq $state.PSObject.Properties['PathEntryWasPresent']) {
        throw 'Rollback state does not record exact PATH-entry ownership. Refusing to restore the complete machine PATH snapshot.'
    }
    Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent ([bool]$state.PathEntryWasPresent) | Out-Null

    if ($null -ne $state.Service -and $state.Service.State -eq 'Running') {
        Invoke-HealthVersionCheck -ExpectedVersion ([string]$state.Version) | Out-Null
    }
    if ($state.PSObject.Properties['GatewayService'] -and
        $null -ne $state.GatewayService -and
        $state.GatewayService.State -eq 'Running') {
        Wait-GatewayListenerOwnership -Paths $Paths | Out-Null
        Invoke-GatewaySmokeCheck -Paths $Paths | Out-Null
    }
}

function Copy-CandidateIntoInstall {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Stage,

        [Parameter(Mandatory = $true)]
        [string]$InstallerScriptRoot,

        [Parameter(Mandatory = $true)]
        [string]$UninstallerSource,

        [Parameter(Mandatory = $false)]
        [bool]$IncludeGateway = $false
    )

    $installRootComparable = Get-ComparableAbsolutePath -Path $Paths.InstallRoot
    if ([string]::IsNullOrWhiteSpace($installRootComparable)) {
        throw "The installer root is not an absolute path: $($Paths.InstallRoot)"
    }
    $installRootPrefix = @(
        $installRootComparable.TrimEnd('\') + '\'
        $installRootComparable.TrimEnd('/') + '/'
    )

    foreach ($source in @(
            (Join-Path $InstallerScriptRoot 'InstallerSupport.ps1'),
            (Join-Path $InstallerScriptRoot 'Install-Bhtune.ps1'),
            $UninstallerSource
        )) {
        $sourceComparable = Get-ComparableAbsolutePath -Path $source
        if ([string]::IsNullOrWhiteSpace($sourceComparable)) {
            throw "The installer support payload path is not absolute: $source"
        }
        if ($sourceComparable -eq $installRootComparable -or
            ($installRootPrefix | Where-Object {
                $sourceComparable.StartsWith($_, [System.StringComparison]::OrdinalIgnoreCase)
            })) {
            throw "The installer support payload must be outside the install root because the existing root is replaced before the candidate is copied: $source"
        }
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "The installer support payload is incomplete: $source"
        }
    }

    if (Test-Path -LiteralPath $Paths.InstallRoot) {
        Write-InstallerTrace -Stage 'candidate.remove-old-install.begin' -Detail $Paths.InstallRoot
        Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
        Write-InstallerTrace -Stage 'candidate.remove-old-install.end' -Detail $Paths.InstallRoot
    }
    Write-InstallerTrace -Stage 'candidate.install-root.begin' -Detail $Paths.InstallRoot
    New-Item -ItemType Directory -Path $Paths.InstallRoot -Force | Out-Null

    foreach ($name in (Get-RequiredPayloadFiles)) {
        Write-InstallerTrace -Stage 'candidate.copy-file.begin' -Detail $name
        Copy-Item -LiteralPath (Join-Path $Stage.PayloadRoot $name) -Destination (Join-Path $Paths.InstallRoot $name) -Force
        Write-InstallerTrace -Stage 'candidate.copy-file.end' -Detail $name
    }

    if ($IncludeGateway) {
        if ([string]::IsNullOrWhiteSpace([string]$Stage.GatewayPayloadRoot)) {
            throw 'The staged candidate does not include the requested OPC DA gateway payload.'
        }
        New-Item -ItemType Directory -Path $Paths.GatewayInstallRoot -Force | Out-Null
        foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
            Write-InstallerTrace -Stage 'candidate.copy-gateway-file.begin' -Detail $name
            Copy-Item `
                -LiteralPath (Join-Path $Stage.GatewayPayloadRoot $name) `
                -Destination (Join-Path $Paths.GatewayInstallRoot $name) `
                -Force
            Write-InstallerTrace -Stage 'candidate.copy-gateway-file.end' -Detail $name
        }
        $installedGateway = Assert-GatewayPayloadLayout -GatewayPayloadRoot $Paths.GatewayInstallRoot
        Assert-GatewayPayloadBinary -GatewayPayload $installedGateway | Out-Null
    }

    Write-InstallerTrace -Stage 'candidate.copy-support.begin' -Detail $Paths.InstallerScriptRoot
    New-Item -ItemType Directory -Path $Paths.InstallerScriptRoot -Force | Out-Null
    Copy-Item -LiteralPath (Join-Path $InstallerScriptRoot 'InstallerSupport.ps1') -Destination (Join-Path $Paths.InstallerScriptRoot 'InstallerSupport.ps1') -Force
    Copy-Item -LiteralPath (Join-Path $InstallerScriptRoot 'Install-Bhtune.ps1') -Destination (Join-Path $Paths.InstallerScriptRoot 'Install-Bhtune.ps1') -Force
    Copy-Item -LiteralPath $UninstallerSource -Destination $Paths.UninstallerPath -Force
    Write-InstallerTrace -Stage 'candidate.copy-support.end' -Detail $Paths.UninstallerPath
}

function Write-OwnershipMetadata {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$Version,

        [Parameter(Mandatory = $true)]
        [bool]$PathManaged,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayManaged = $false,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [psobject]$GatewayContract
    )

    if ($GatewayManaged -and $null -eq $GatewayContract) {
        throw 'Gateway ownership metadata requires the verified gateway release contract.'
    }

    $marker = [pscustomobject]([ordered]@{
        SchemaVersion = $script:InstallerSchemaVersion
        InstallerOwned = 1
        InstallDir    = $Paths.InstallRoot
        Version       = $Version
        ServiceName   = $Paths.ServiceName
        ConfigPath    = $Paths.ConfigPath
        DatabasePath  = $Paths.DatabasePath
        PathEntry     = $Paths.InstallRoot
        PathManaged   = if ($PathManaged) { 1 } else { 0 }
        ShortcutPath  = $Paths.ShortcutPath
        GatewayManaged = if ($GatewayManaged) { 1 } else { 0 }
        GatewayVersion = if ($GatewayManaged) { [string]$GatewayContract.version } else { '' }
        GatewaySha256 = if ($GatewayManaged) { [string]$GatewayContract.executable.sha256 } else { '' }
        GatewayExecutable = $Paths.GatewayExecutable
        GatewayConfigPath = $Paths.GatewayConfigPath
        GatewayServiceName = $Paths.GatewayServiceName
        GatewayPort = [int]$Paths.GatewayPort
    })
    $uninstall = [pscustomobject](Get-ExpectedUninstallMetadata -Version $Version -InstallRoot $Paths.InstallRoot -UninstallerPath $Paths.UninstallerPath)
    Set-RegistryValues -Path $Paths.MarkerPath -Values $marker
    Set-RegistryValues -Path $Paths.UninstallKeyPath -Values $uninstall
}

function Write-TransactionJournal {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Value
    )

    $journalPath = Join-Path $Paths.InstallerStateRoot 'transaction.json'
    $temporaryPath = "$journalPath.tmp"
    $backupPath = "$journalPath.bak"
    try {
        Write-JsonFile -Path $temporaryPath -Value $Value
        if (Test-Path -LiteralPath $journalPath -PathType Leaf) {
            Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue
            [System.IO.File]::Replace($temporaryPath, $journalPath, $backupPath, $true)
            Remove-Item -LiteralPath $backupPath -Force -ErrorAction SilentlyContinue
        } else {
            [System.IO.File]::Move($temporaryPath, $journalPath)
        }
    } catch {
        Remove-Item -LiteralPath $temporaryPath -Force -ErrorAction SilentlyContinue
        throw
    }
}

function Update-TransactionJournalFields {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [System.Collections.IDictionary]$Fields
    )

    $journal = Read-TransactionJournal -Paths $Paths
    if ($null -eq $journal) {
        throw 'Cannot update the installer transaction journal because it is missing.'
    }
    foreach ($entry in $Fields.GetEnumerator()) {
        $journal | Add-Member -MemberType NoteProperty -Name $entry.Key -Value $entry.Value -Force
    }
    Write-TransactionJournal -Paths $Paths -Value $journal
}

function Test-TransactionJournalPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Candidate,

        [Parameter(Mandatory = $true)]
        [string]$Root,

        [Parameter(Mandatory = $false)]
        [bool]$AllowRoot = $false
    )

    if ([string]::IsNullOrWhiteSpace($Candidate)) {
        return $false
    }
    $candidateValue = Get-ComparableAbsolutePath -Path $Candidate
    $rootValue = Get-ComparableAbsolutePath -Path $Root
    if ([string]::IsNullOrWhiteSpace($candidateValue) -or
        [string]::IsNullOrWhiteSpace($rootValue)) {
        return $false
    }
    if ($candidateValue -eq $rootValue) {
        return $AllowRoot
    }
    return $candidateValue.StartsWith($rootValue.TrimEnd('\') + '\', [System.StringComparison]::OrdinalIgnoreCase) -or
        $candidateValue.StartsWith($rootValue.TrimEnd('/') + '/', [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-TransactionJournalShape {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Journal
    )

    $mode = [string]$Journal.Mode
    $phase = [string]$Journal.Phase
    $installPhases = @(
        'Preflight', 'PathSnapshotted', 'ServiceStopPending', 'ServiceStopped',
        'BackupPromoted', 'ServiceRemovePending', 'ServiceRemoved',
        'PayloadStaged', 'PayloadInstalled', 'ServiceCreatePending',
        'ServiceCreated', 'HealthChecked', 'PathUpdatePending', 'PathUpdated',
        'ShortcutCreatePending', 'ShortcutCreated', 'MetadataPending',
        'Committed'
    )
    $uninstallPhases = @(
        'Begin', 'ServiceStopPending', 'ServiceRemovePending', 'ServiceRemoved',
        'PathUpdatePending', 'PathRemoved', 'ShortcutRemovePending',
        'ShortcutRemoved', 'ReadyForProgramFilesCleanup',
        'ProgramFilesRemovePending', 'ProgramFilesRemoved',
        'MetadataRemovePending', 'MetadataRemoved'
    )
    if (($mode -eq 'Install' -or $mode -eq 'Upgrade') -and $installPhases -notcontains $phase) {
        throw "The installer transaction journal has unsupported phase '$phase' for mode '$mode'. Refusing to guess."
    }
    if ($mode -eq 'Uninstall' -and $uninstallPhases -notcontains $phase) {
        throw "The installer transaction journal has unsupported uninstall phase '$phase'. Refusing to guess."
    }
    if ($mode -ne 'Install' -and $mode -ne 'Upgrade' -and $mode -ne 'Uninstall') {
        throw "The installer transaction journal has unknown mode '$mode'. Refusing to mutate BHTune."
    }
    if (($mode -eq 'Install' -or $mode -eq 'Upgrade') -and
        -not $Journal.PSObject.Properties['InstallRootWasPresent']) {
        throw 'The installer transaction journal does not record prior install-root presence. Refusing to guess.'
    }

    if ($Journal.PSObject.Properties['TransactionId'] -and
        -not [string]::IsNullOrWhiteSpace([string]$Journal.TransactionId)) {
        $transactionId = [guid]::Empty
        if (-not [guid]::TryParse([string]$Journal.TransactionId, [ref]$transactionId)) {
            throw 'The installer transaction journal has an invalid transaction identifier. Refusing to guess.'
        }
    }

    if ($Journal.PSObject.Properties['BackupRoot'] -and
        -not [string]::IsNullOrWhiteSpace([string]$Journal.BackupRoot)) {
        if (-not (Test-TransactionJournalPath -Candidate ([string]$Journal.BackupRoot) -Root $Paths.RollbackRoot -AllowRoot $true)) {
            throw "The installer transaction journal points outside the managed rollback root: $($Journal.BackupRoot)"
        }
    }
    if ($Journal.PSObject.Properties['StageRoot'] -and
        -not [string]::IsNullOrWhiteSpace([string]$Journal.StageRoot)) {
        $stageRoot = [string]$Journal.StageRoot
        if (-not (Test-TransactionJournalPath -Candidate $stageRoot -Root $Paths.InstallerStateRoot -AllowRoot $false) -or
            [System.IO.Path]::GetFileName($stageRoot) -notlike 'candidate-*') {
            throw "The installer transaction journal points outside the managed staging root: $stageRoot"
        }
    }

    foreach ($booleanName in @(
            'InstallRootWasPresent',
            'ConfigWasPresent',
            'ConfigCreationPending',
            'ConfigWasCreated',
            'ServiceWasPresent',
            'ServiceWasRunning',
            'GatewayManaged',
            'GatewayWasManaged',
            'GatewayProgramDataRootWasPresent',
            'GatewayConfigWasPresent',
            'GatewayConfigCreationPending',
            'GatewayConfigWasCreated',
            'GatewayServiceWasPresent',
            'GatewayServiceWasRunning',
            'PathEntryWasPresent',
            'ShortcutWasPresent',
            'PathChangedByTransaction'
        )) {
        if ($Journal.PSObject.Properties[$booleanName]) {
            $value = $Journal.$booleanName
            if ($value -isnot [bool] -and [string]$value -ne 'True' -and [string]$value -ne 'False') {
                throw "The installer transaction journal has a non-boolean '$booleanName' value. Refusing to guess."
            }
        }
    }
    foreach ($hashName in @(
            'ExpectedConfigHash',
            'CreatedConfigHash',
            'ExpectedGatewayConfigHash',
            'CreatedGatewayConfigHash'
        )) {
        if ($Journal.PSObject.Properties[$hashName]) {
            $hashValue = $Journal.$hashName
            if ($null -ne $hashValue -and
                -not [string]::IsNullOrWhiteSpace([string]$hashValue) -and
                [string]$hashValue -notmatch '^[0-9a-fA-F]{64}$') {
                throw "The installer transaction journal has an invalid '$hashName' value. Refusing to guess."
            }
        }
    }

    if ([int]$Journal.SchemaVersion -ge 3 -and ($mode -eq 'Install' -or $mode -eq 'Upgrade')) {
        foreach ($name in @(
                'GatewayManaged',
                'GatewayWasManaged',
                'GatewayService',
                'GatewayProgramDataRootWasPresent',
                'GatewayConfigWasPresent',
                'GatewayConfigCreationPending',
                'GatewayConfigWasCreated',
                'ExpectedGatewayConfigHash',
                'CreatedGatewayConfigHash',
                'GatewayServiceWasPresent',
                'GatewayServiceWasRunning'
            )) {
            if (-not $Journal.PSObject.Properties[$name]) {
                throw "The schema-3 installer transaction journal is missing '$name'."
            }
        }
    }
    if ([int]$Journal.SchemaVersion -ge 3 -and $mode -eq 'Uninstall' -and
        -not $Journal.PSObject.Properties['GatewayManaged']) {
        throw "The schema-3 uninstall transaction journal is missing 'GatewayManaged'."
    }
}

function Update-TransactionJournalPhase {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$Phase,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$BackupRoot
    )

    $journal = Read-TransactionJournal -Paths $Paths
    if ($null -eq $journal) {
        throw "Cannot update the installer transaction phase to '$Phase' because its journal is missing."
    }
    $journal.Phase = $Phase
    if ($PSBoundParameters.ContainsKey('BackupRoot')) {
        $journal.BackupRoot = $BackupRoot
    }
    Write-TransactionJournal -Paths $Paths -Value $journal
}

function Get-TransactionPhaseRank {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Phase
    )

    $ranks = @{
        Preflight           = 0
        PathSnapshotted     = 1
        ServiceStopPending   = 2
        ServiceStopped       = 3
        BackupPromoted       = 4
        ServiceRemovePending = 5
        ServiceRemoved       = 6
        PayloadStaged        = 7
        PayloadInstalled     = 8
        ServiceCreatePending = 9
        ServiceCreated       = 10
        HealthChecked        = 11
        PathUpdatePending    = 12
        PathUpdated          = 13
        ShortcutCreatePending = 14
        ShortcutCreated      = 15
        MetadataPending      = 16
        Committed            = 17
    }
    if (-not $ranks.ContainsKey($Phase)) {
        throw "Unknown install transaction phase '$Phase'."
    }
    return [int]$ranks[$Phase]
}

function Test-CleanInstallRootContents {
    param(
        [Parameter(Mandatory = $true)]
        [string]$InstallRoot
    )

    if (-not (Test-Path -LiteralPath $InstallRoot -PathType Container)) {
        return $false
    }

    $allowed = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($name in (Get-RequiredPayloadFiles)) {
        [void]$allowed.Add($name)
    }
    foreach ($name in @(
            'uninstall.exe',
            '.bhtune-installer-transaction.json',
            'installer\InstallerSupport.ps1',
            'installer\Install-Bhtune.ps1'
        )) {
        [void]$allowed.Add($name)
    }
    foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
        [void]$allowed.Add(('gateway\' + $name))
    }

    $rootValue = (Get-Item -LiteralPath $InstallRoot -Force).FullName.TrimEnd('\') + '\'
    foreach ($item in @(Get-ChildItem -LiteralPath $InstallRoot -Force -Recurse -ErrorAction Stop)) {
        $relative = $item.FullName.Substring($rootValue.Length).Replace('/', '\')
        if ($item.PSIsContainer) {
            if ($relative -ne 'installer' -and $relative -ne 'gateway') {
                throw "The interrupted clean installation contains an unexpected directory '$relative'. Refusing to delete operator-owned data."
            }
            continue
        }
        if (-not $allowed.Contains($relative)) {
            throw "The interrupted clean installation contains an unexpected file '$relative'. Refusing to delete operator-owned data."
        }
    }
    return $true
}

function Test-CleanStagingRootContents {
    param(
        [Parameter(Mandatory = $true)]
        [string]$StageRoot
    )

    if (-not (Test-Path -LiteralPath $StageRoot -PathType Container)) {
        return $false
    }
    $children = @(Get-ChildItem -LiteralPath $StageRoot -Force -ErrorAction Stop)
    $allowedRoots = @('payload', 'gateway')
    if ($children.Count -lt 1 -or $children.Count -gt 2 -or
        @($children | Where-Object { -not $_.PSIsContainer -or $allowedRoots -notcontains $_.Name }).Count -gt 0 -or
        @($children | Where-Object { $_.Name -eq 'payload' }).Count -ne 1) {
        throw "The interrupted staging root '$StageRoot' contains unexpected content. Refusing to delete it."
    }
    $payloadRoot = @($children | Where-Object { $_.Name -eq 'payload' })[0].FullName
    $allowed = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($name in (Get-RequiredPayloadFiles)) {
        [void]$allowed.Add($name)
    }
    foreach ($item in @(Get-ChildItem -LiteralPath $payloadRoot -Force -Recurse -ErrorAction Stop)) {
        if ($item.PSIsContainer) {
            throw "The interrupted staging root '$StageRoot' contains unexpected directory '$($item.FullName)'. Refusing to delete it."
        }
        if (-not $allowed.Contains($item.Name)) {
            throw "The interrupted staging root '$StageRoot' contains unexpected file '$($item.Name)'. Refusing to delete it."
        }
    }
    $gatewayRoot = @($children | Where-Object { $_.Name -eq 'gateway' })
    if ($gatewayRoot.Count -eq 1) {
        Assert-GatewayPayloadLayout -GatewayPayloadRoot $gatewayRoot[0].FullName | Out-Null
    }
    return $true
}

function Remove-CleanInstallStagingRoots {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$RecordedStageRoot
    )

    if (-not (Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container)) {
        return
    }
    $roots = @(
        Get-ChildItem -LiteralPath $Paths.InstallerStateRoot -Directory -Filter 'candidate-*' -Force -ErrorAction Stop
    )
    if (-not [string]::IsNullOrWhiteSpace($RecordedStageRoot) -and (Test-Path -LiteralPath $RecordedStageRoot)) {
        $recorded = Get-Item -LiteralPath $RecordedStageRoot -Force
        if ($roots.FullName -notcontains $recorded.FullName) {
            $roots += $recorded
        }
    }
    foreach ($root in $roots) {
        if ($root.Name -notmatch '^candidate-[0-9a-fA-F]{32}$') {
            throw "The installer state contains an unexpected staging directory '$($root.Name)'. Refusing to delete it."
        }
        [void](Test-CleanStagingRootContents -StageRoot $root.FullName)
        Remove-Item -LiteralPath $root.FullName -Recurse -Force
    }
}

function Recover-CleanInstallTransaction {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$Journal
    )

    $phase = [string]$Journal.Phase
    $phaseRank = Get-TransactionPhaseRank -Phase $phase
    $installRootWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'InstallRootWasPresent')
    $serviceWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'ServiceWasPresent')
    $configWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'ConfigWasPresent')
    $configWasCreated = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'ConfigWasCreated')
    $gatewayManaged = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayManaged')
    $gatewayWasManaged = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayWasManaged')
    $gatewayProgramDataRootWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayProgramDataRootWasPresent')
    $gatewayConfigWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayConfigWasPresent')
    $gatewayConfigWasCreated = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayConfigWasCreated')
    $gatewayServiceWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayServiceWasPresent')
    $pathEntryWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'PathEntryWasPresent')
    $shortcutWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'ShortcutWasPresent')
    $pathChanged = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'PathChangedByTransaction')
    $service = Get-ServiceSnapshot -Name $Paths.ServiceName
    $gatewayService = if ($gatewayManaged) {
        Get-ServiceSnapshot -Name $Paths.GatewayServiceName
    } else {
        $null
    }
    $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
    $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath

    if ($installRootWasPresent -or $serviceWasPresent -or $configWasPresent -or
        $pathEntryWasPresent -or $shortcutWasPresent -or $gatewayWasManaged -or
        $gatewayServiceWasPresent) {
        throw 'The clean-install journal claims pre-existing or installer-owned state that cannot be discarded without a verified backup.'
    }
    if ($phaseRank -ge (Get-TransactionPhaseRank -Phase 'ServiceCreatePending')) {
        if ($null -ne $service -and
            -not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
            throw 'The interrupted clean installation found an unowned service at the fixed service name.'
        }
        if ($null -ne $service) {
            if ($service.State -ne 'Stopped') {
                Stop-InstallerService | Out-Null
            }
            Remove-ServiceByName -Name $Paths.ServiceName
        }
    } elseif ($null -ne $service) {
        throw 'The interrupted clean installation found a service before its journaled creation phase.'
    }
    if ($gatewayManaged) {
        if ($phaseRank -ge (Get-TransactionPhaseRank -Phase 'ServiceCreatePending')) {
            if ($null -ne $gatewayService) {
                $ownedGatewayService = Test-OwnedGatewayServiceSnapshot `
                    -ServiceSnapshot $gatewayService `
                    -ExecutablePath $Paths.GatewayExecutable `
                    -ConfigPath $Paths.GatewayConfigPath `
                    -LogDirectory $Paths.GatewayLogDirectory
                $partialCandidateService = $phase -eq 'ServiceCreatePending' -and
                    (Test-InstallerCreatedGatewayServiceSnapshot `
                        -ServiceSnapshot $gatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)
                if (-not $ownedGatewayService -and -not $partialCandidateService) {
                    throw 'The interrupted clean installation found an unowned OPC DA gateway service at the fixed service name.'
                }
                if ($ownedGatewayService) {
                    Stop-InstallerGatewayService -Paths $Paths | Out-Null
                    Remove-InstallerGatewayService -Paths $Paths
                } else {
                    Remove-InstallerCreatedGatewayServiceRegistration -Paths $Paths
                }
            }
        } elseif ($null -ne $gatewayService) {
            throw 'The interrupted clean installation found an OPC DA gateway service before its journaled creation phase.'
        }
    }

    $stageRoot = if ($Journal.PSObject.Properties['StageRoot']) { [string]$Journal.StageRoot } else { '' }
    Remove-CleanInstallStagingRoots -Paths $Paths -RecordedStageRoot $stageRoot

    if ($phaseRank -ge (Get-TransactionPhaseRank -Phase 'PayloadStaged') -and
        (Test-Path -LiteralPath $Paths.InstallRoot)) {
        if (-not (Test-CleanInstallRootContents -InstallRoot $Paths.InstallRoot)) {
            throw 'The interrupted clean installation root could not be proven to contain only transaction-created payload.'
        }
        Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
    } elseif (Test-Path -LiteralPath $Paths.InstallRoot) {
        throw 'The interrupted clean installation has an unexpected Program Files root before payload staging.'
    }

    Remove-InstallerCreatedConfig `
        -Paths $Paths `
        -ConfigWasPresent $configWasPresent `
        -ConfigCreationPending ([bool](Get-SnapshotValue -Snapshot $Journal -Name 'ConfigCreationPending')) `
        -ConfigWasCreated $configWasCreated `
        -ExpectedConfigHash ([string](Get-SnapshotValue -Snapshot $Journal -Name 'ExpectedConfigHash')) `
        -CreatedConfigHash ([string](Get-SnapshotValue -Snapshot $Journal -Name 'CreatedConfigHash'))
    if ($gatewayManaged) {
        Remove-InstallerCreatedGatewayConfig `
            -Paths $Paths `
            -ConfigWasPresent $gatewayConfigWasPresent `
            -ConfigCreationPending ([bool](Get-SnapshotValue -Snapshot $Journal -Name 'GatewayConfigCreationPending')) `
            -ConfigWasCreated $gatewayConfigWasCreated `
            -ExpectedConfigHash ([string](Get-SnapshotValue -Snapshot $Journal -Name 'ExpectedGatewayConfigHash')) `
            -CreatedConfigHash ([string](Get-SnapshotValue -Snapshot $Journal -Name 'CreatedGatewayConfigHash'))
        if (-not $gatewayProgramDataRootWasPresent -and
            (Test-Path -LiteralPath $Paths.GatewayProgramDataRoot)) {
            Assert-NoReparsePointInPath `
                -Path $Paths.GatewayProgramDataRoot `
                -Name 'the transaction-created OPC DA gateway ProgramData root'
            Remove-Item -LiteralPath $Paths.GatewayProgramDataRoot -Recurse -Force
        }
    }

    $pathPending = $phase -eq 'PathUpdatePending'
    if ($pathPending -and -not $pathChanged -and -not $pathEntryWasPresent) {
        $pathEntries = @(Split-PathList -PathList (Get-MachinePathSnapshot))
        $pathMatches = @($pathEntries | Where-Object {
                (Normalize-PathForComparison -Path $_) -eq (Normalize-PathForComparison -Path $Paths.InstallRoot)
            })
        if ($pathMatches.Count -gt 0) {
            throw 'The interrupted clean installation reached a PATH update window without recording ownership. Refusing to remove an ambiguous entry.'
        }
    } elseif (($pathChanged -or $pathPending) -and -not $pathEntryWasPresent) {
        Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent:$false | Out-Null
    }
    if (-not $shortcutWasPresent -and $phaseRank -ge (Get-TransactionPhaseRank -Phase 'ShortcutCreatePending') -and
        -not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath)) {
        if (Test-Path -LiteralPath $Paths.ShortcutPath) {
            if (-not (Test-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath)) {
                throw 'The interrupted clean installation found an unowned Start Menu shortcut at the fixed path.'
            }
            Remove-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
        }
    }

    $metadataPending = $phaseRank -ge (Get-TransactionPhaseRank -Phase 'MetadataPending')
    if ($null -ne $marker -or $null -ne $uninstall) {
        if (-not $metadataPending) {
            throw 'The interrupted clean installation found ownership metadata before its journaled metadata phase.'
        }
        Assert-InstallerMetadataForRecovery `
            -Paths $Paths `
            -Journal $Journal `
            -Marker $marker `
            -Uninstall $uninstall | Out-Null
        if ($null -ne $marker) {
            Remove-RegistryKey -Path $Paths.MarkerPath
        }
        if ($null -ne $uninstall) {
            Remove-RegistryKey -Path $Paths.UninstallKeyPath
        }
    }

    Remove-TransactionJournal -Paths $Paths
    Write-InstallerTrace -Stage 'journal.recover.clean-install-discarded' -Detail $phase
    return [pscustomobject]@{ Action = 'Discarded'; Journal = $Journal }
}

function Remove-TransactionJournal {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $path = Join-Path $Paths.InstallerStateRoot 'transaction.json'
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Force
    }
}

function Invoke-InstallTransaction {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [psobject]$PriorState,

        [Parameter(Mandatory = $true)]
        [psobject]$VersionContract,

        [Parameter(Mandatory = $true)]
        [string]$PayloadRoot,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$GatewayPayloadRoot,

        [Parameter(Mandatory = $true)]
        [string]$InstallerScriptRoot,

        [Parameter(Mandatory = $true)]
        [string]$UninstallerSource,

        [Parameter(Mandatory = $true)]
        [bool]$AddToPath,

        [Parameter(Mandatory = $true)]
        [bool]$StartService,

        [Parameter(Mandatory = $true)]
        [bool]$InstallGateway,

        [Parameter(Mandatory = $true)]
        [bool]$StartGateway,

        [Parameter(Mandatory = $true)]
        [bool]$CustomDbBackupConfirmed,

        [Parameter(Mandatory = $true)]
        [bool]$TestOnly,

        [Parameter(Mandatory = $true)]
        [string]$FailureInjection
    )

    $isUpgrade = [bool]$PriorState.IsUpgrade
    $gatewayWasManaged = [bool]$PriorState.GatewayManaged
    $gatewayRegistrationExists = [bool](Get-SnapshotValue `
            -Snapshot $PriorState `
            -Name 'GatewayServiceExists')
    $manageGateway = $gatewayWasManaged -or $InstallGateway
    $preservedState = Get-PreservedProgramDataState -Paths $Paths
    $rollbackRequired = $isUpgrade -or [bool]$preservedState.ReuseRequired -or
        ($manageGateway -and [bool]$preservedState.GatewayStateExists)
    $configWasCreated = $false
    $createdConfigHash = $null
    $configCreationPending = $false
    $expectedConfigHash = $null
    $gatewayConfigWasCreated = $false
    $createdGatewayConfigHash = $null
    $gatewayConfigCreationPending = $false
    $expectedGatewayConfigHash = $null
    $gatewayRelease = $null
    $stage = $null
    $backup = $null
    $pathManaged = $false
    $pathChangedByTransaction = $false
    $serviceCreated = $false
    $gatewayServiceCreationAttempted = $false
    $gatewayServiceCreated = $false
    $backupPromoted = $false
    $shortcutCreated = $false
    $priorWasRunning = $isUpgrade -and $null -ne $PriorState.Service -and $PriorState.Service.State -eq 'Running'
    $priorGatewayWasRunning = $gatewayWasManaged -and
        $null -ne $PriorState.GatewayService -and
        $PriorState.GatewayService.State -eq 'Running'
    $finalGatewayRunning = if ($gatewayWasManaged) { $priorGatewayWasRunning } else { $StartGateway }
    $installRootWasPresent = Test-Path -LiteralPath $Paths.InstallRoot -PathType Container
    $databaseDirectoryWasPresent = Test-Path -LiteralPath $Paths.DatabaseDirectory -PathType Container
    $logDirectoryWasPresent = Test-Path -LiteralPath $Paths.LogDirectory -PathType Container
    $installerStateRootWasPresent = Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container
    $gatewayProgramDataRootWasPresent = Test-Path -LiteralPath $Paths.GatewayProgramDataRoot -PathType Container
    $gatewayDataDirectoryWasPresent = Test-Path -LiteralPath $Paths.GatewayDataDirectory -PathType Container
    $gatewayLogDirectoryWasPresent = Test-Path -LiteralPath $Paths.GatewayLogDirectory -PathType Container
    $configWasPresent = Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf
    $gatewayConfigWasPresent = Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Leaf
    $serviceWasPresent = $null -ne $PriorState.Service
    $gatewayServiceWasPresent = $null -ne $PriorState.GatewayService
    $shortcutWasPresent = -not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
        (Test-Path -LiteralPath $Paths.ShortcutPath)
    $transactionId = [guid]::NewGuid().ToString('N')

    try {
        Write-InstallerTrace -Stage 'transaction.begin' -Detail ("mode={0}; upgrade={1}; version={2}; addPath={3}; startService={4}; manageGateway={5}; startGateway={6}; customDb={7}" -f `
                'Install', $isUpgrade, $VersionContract.Version, $AddToPath, $StartService, $manageGateway, $finalGatewayRunning, $CustomDbBackupConfirmed)
        if ($manageGateway -and [string]::IsNullOrWhiteSpace($GatewayPayloadRoot)) {
            throw 'The OPC DA gateway component is selected, but no verified gateway payload was provided.'
        }
        if ($manageGateway -and -not $gatewayWasManaged -and
            ($gatewayRegistrationExists -or $null -ne $PriorState.GatewayService)) {
            throw "The service '$($Paths.GatewayServiceName)' already exists but is not installer-owned. Refusing to adopt it."
        }
        if ($manageGateway -and $gatewayConfigWasPresent) {
            Assert-GatewayConfigPolicy `
                -ConfigPath $Paths.GatewayConfigPath `
                -ExpectedDatabasePath $Paths.GatewayDatabasePath `
                -ExpectedLogDirectory $Paths.GatewayLogDirectory | Out-Null
        }
        if ($manageGateway) {
            $gatewayListeners = @(Get-TcpListenerSnapshots -Port $Paths.GatewayPort)
            if ($gatewayWasManaged -and $priorGatewayWasRunning) {
                Assert-GatewayListenerOwnership -Paths $Paths | Out-Null
            } elseif ($gatewayListeners.Count -gt 0) {
                throw "TCP port $($Paths.GatewayPort) is already occupied. Refusing to replace an unexpected listener."
            }
        }
        Write-InstallerTrace -Stage 'preflight.directories.begin' -Detail $Paths.ProgramDataRoot
        Ensure-InstallerDirectories -Paths $Paths -ManageGateway:$manageGateway
        Write-InstallerTrace -Stage 'preflight.directories.end' -Detail $Paths.ProgramDataRoot
        $pathEntryWasPresent = @(
            (Split-PathList -PathList (Get-MachinePathSnapshot)) | Where-Object {
                (Normalize-PathForComparison -Path $_) -eq (Normalize-PathForComparison -Path $Paths.InstallRoot)
            }
        ).Count -gt 0
        Write-TransactionJournal -Paths $Paths -Value ([pscustomobject]@{
                SchemaVersion            = $script:InstallerSchemaVersion
                TransactionId            = $transactionId
                Mode                     = if ($isUpgrade) { 'Upgrade' } else { 'Install' }
                Phase                    = 'Preflight'
                StartedUtc               = [DateTime]::UtcNow.ToString('o')
                Version                  = $VersionContract.Version
                BackupRoot               = $null
                StageRoot                = $null
                InstallRootWasPresent   = $installRootWasPresent
                ConfigWasPresent        = $configWasPresent
                ConfigCreationPending   = $false
                ExpectedConfigHash      = $null
                ConfigWasCreated        = $false
                CreatedConfigHash       = $null
                ServiceWasPresent        = $serviceWasPresent
                ServiceWasRunning        = $priorWasRunning
                GatewayManaged           = $manageGateway
                GatewayWasManaged        = $gatewayWasManaged
                GatewayProgramDataRootWasPresent = $gatewayProgramDataRootWasPresent
                GatewayConfigWasPresent  = $gatewayConfigWasPresent
                GatewayConfigCreationPending = $false
                ExpectedGatewayConfigHash = $null
                GatewayConfigWasCreated  = $false
                CreatedGatewayConfigHash = $null
                GatewayService           = $PriorState.GatewayService
                GatewayServiceWasPresent = $gatewayServiceWasPresent
                GatewayServiceWasRunning = $priorGatewayWasRunning
                PathEntryWasPresent      = $pathEntryWasPresent
                PathChangedByTransaction = $false
                ShortcutWasPresent       = $shortcutWasPresent
            })
        if ($isUpgrade -and -not (Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf)) {
            throw "The installer-owned configuration file is missing: $($Paths.ConfigPath). Refusing to recreate it during an upgrade."
        }
        Write-InstallerTrace -Stage 'preflight.config.begin' -Detail $Paths.ConfigPath
        if (-not $isUpgrade -and -not $configWasPresent) {
            $defaultConfigContent = Get-DefaultConfigContent -DatabasePath $Paths.DatabasePath -LogDirectory $Paths.LogDirectory
            $expectedConfigHash = Get-TextSha256 -Content $defaultConfigContent
            $configCreationPending = $true
            Update-TransactionJournalFields -Paths $Paths -Fields @{
                ConfigCreationPending = $true
                ExpectedConfigHash    = $expectedConfigHash
            }
            if (-not (Ensure-InstallerConfig -Paths $Paths -DefaultContent $defaultConfigContent)) {
                throw "The clean-install configuration appeared while creation was pending: $($Paths.ConfigPath)"
            }
            $createdConfigHash = Get-FileSha256 -Path $Paths.ConfigPath
            if ($createdConfigHash -ne $expectedConfigHash) {
                throw "The clean-install configuration did not match its journaled content hash: $($Paths.ConfigPath)"
            }
            $configWasCreated = $true
            $configCreationPending = $false
            Update-TransactionJournalFields -Paths $Paths -Fields @{
                ConfigCreationPending = $false
                ConfigWasCreated      = $true
                CreatedConfigHash     = $createdConfigHash
            }
        } else {
            $configWasCreated = Ensure-InstallerConfig -Paths $Paths
            if ($configWasCreated) {
                throw "The installer-created configuration was not journaled before writing: $($Paths.ConfigPath)"
            }
        }
        Write-InstallerTrace -Stage 'preflight.config.end' -Detail ("created={0}" -f $configWasCreated)
        if ($manageGateway) {
            Write-InstallerTrace -Stage 'preflight.gateway-config.begin' -Detail $Paths.GatewayConfigPath
            if (-not $gatewayConfigWasPresent) {
                $defaultGatewayConfigContent = Get-DefaultGatewayConfigContent `
                    -DatabasePath $Paths.GatewayDatabasePath `
                    -LogDirectory $Paths.GatewayLogDirectory
                $expectedGatewayConfigHash = Get-TextSha256 -Content $defaultGatewayConfigContent
                $gatewayConfigCreationPending = $true
                Update-TransactionJournalFields -Paths $Paths -Fields @{
                    GatewayConfigCreationPending = $true
                    ExpectedGatewayConfigHash    = $expectedGatewayConfigHash
                }
                if (-not (Ensure-InstallerGatewayConfig `
                        -Paths $Paths `
                        -DefaultContent $defaultGatewayConfigContent)) {
                    throw "The OPC DA gateway configuration appeared while creation was pending: $($Paths.GatewayConfigPath)"
                }
                $createdGatewayConfigHash = Get-FileSha256 -Path $Paths.GatewayConfigPath
                if ($createdGatewayConfigHash -ne $expectedGatewayConfigHash) {
                    throw "The OPC DA gateway configuration did not match its journaled content hash: $($Paths.GatewayConfigPath)"
                }
                $gatewayConfigWasCreated = $true
                $gatewayConfigCreationPending = $false
                Update-TransactionJournalFields -Paths $Paths -Fields @{
                    GatewayConfigCreationPending = $false
                    GatewayConfigWasCreated      = $true
                    CreatedGatewayConfigHash     = $createdGatewayConfigHash
                }
            } elseif (Ensure-InstallerGatewayConfig -Paths $Paths) {
                throw "The installer-created OPC DA gateway configuration was not journaled before writing: $($Paths.GatewayConfigPath)"
            }
            Assert-GatewayConfigPolicy `
                -ConfigPath $Paths.GatewayConfigPath `
                -ExpectedDatabasePath $Paths.GatewayDatabasePath `
                -ExpectedLogDirectory $Paths.GatewayLogDirectory | Out-Null
            Write-InstallerTrace -Stage 'preflight.gateway-config.end' -Detail ("created={0}" -f $gatewayConfigWasCreated)
        }
        Write-InstallerTrace -Stage 'preflight.database.begin' -Detail $Paths.DatabasePath
        $databasePolicy = Assert-DatabasePolicy -Paths $Paths -IsUpgrade $isUpgrade -CustomDbBackupConfirmed $CustomDbBackupConfirmed -PreservedData ([bool]$preservedState.ReuseRequired)
        Write-InstallerTrace -Stage 'preflight.database.end' -Detail ("policy={0}" -f $databasePolicy.Policy)

        Write-InstallerTrace -Stage 'payload.stage.begin' -Detail $PayloadRoot
        $stage = New-StagedPayload `
            -Paths $Paths `
            -SourcePayloadRoot $PayloadRoot `
            -ExpectedVersion $VersionContract.Version `
            -SourceGatewayPayloadRoot $GatewayPayloadRoot `
            -IncludeGateway:$manageGateway
        Update-TransactionJournalFields -Paths $Paths -Fields @{ StageRoot = $stage.Root }
        Write-InstallerTrace -Stage 'payload.stage.end' -Detail $stage.Root
        Write-InstallerTrace -Stage 'path.snapshot.begin' -Detail 'machine'
        Write-InstallerTrace -Stage 'path.snapshot.end' -Detail 'machine'
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathSnapshotted'

        if (($isUpgrade -and $null -ne $PriorState.Service) -or
            ($gatewayWasManaged -and $null -ne $PriorState.GatewayService)) {
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopPending'
            if ($gatewayWasManaged -and $null -ne $PriorState.GatewayService) {
                Write-InstallerTrace -Stage 'gateway.service.stop.begin' -Detail $Paths.GatewayServiceName
                Stop-InstallerGatewayService -Paths $Paths | Out-Null
                Write-InstallerTrace -Stage 'gateway.service.stop.end' -Detail $Paths.GatewayServiceName
            }
            if ($isUpgrade -and $null -ne $PriorState.Service) {
                Write-InstallerTrace -Stage 'service.stop.begin' -Detail $Paths.ServiceName
                Stop-InstallerService | Out-Null
                Write-InstallerTrace -Stage 'service.stop.end' -Detail $Paths.ServiceName
            }
            if ($manageGateway) {
                Wait-TcpPortFree -Port $Paths.GatewayPort -TimeoutSeconds 30
            }
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopped'
        }

        if ($rollbackRequired) {
            Write-InstallerTrace -Stage 'backup.create.begin' -Detail $Paths.RollbackRoot
            $backup = New-RollbackBackup `
                -Paths $Paths `
                -PriorState $PriorState `
                -DatabasePolicy $databasePolicy `
                -ConfigWasCreated $configWasCreated `
                -InstallRootWasPresent:$installRootWasPresent `
                -ManageGateway:$manageGateway `
                -GatewayConfigWasCreated:$gatewayConfigWasCreated `
                -GatewayProgramDataRootWasPresent:$gatewayProgramDataRootWasPresent
            Write-InstallerTrace -Stage 'backup.create.end' -Detail $backup.PendingRoot
            Write-InstallerTrace -Stage 'backup.promote.begin' -Detail $backup.FinalRoot
            Promote-RollbackBackup -Backup $backup
            Write-InstallerTrace -Stage 'backup.promote.end' -Detail $backup.FinalRoot
            $backupPromoted = $true
            Update-TransactionJournalPhase -Paths $Paths -Phase 'BackupPromoted' -BackupRoot $backup.FinalRoot
        }

        if (($isUpgrade -and $null -ne $PriorState.Service) -or
            ($gatewayWasManaged -and $null -ne $PriorState.GatewayService)) {
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemovePending'
            if ($gatewayWasManaged -and $null -ne $PriorState.GatewayService) {
                Write-InstallerTrace -Stage 'gateway.service.remove.begin' -Detail $Paths.GatewayServiceName
                Remove-InstallerGatewayService -Paths $Paths
                Write-InstallerTrace -Stage 'gateway.service.remove.end' -Detail $Paths.GatewayServiceName
            }
            if ($isUpgrade -and $null -ne $PriorState.Service) {
                Write-InstallerTrace -Stage 'service.remove.begin' -Detail $Paths.ServiceName
                Remove-ServiceByName -Name $Paths.ServiceName
                Write-InstallerTrace -Stage 'service.remove.end' -Detail $Paths.ServiceName
            }
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemoved'
        }

        Update-TransactionJournalPhase -Paths $Paths -Phase 'PayloadStaged'
        Write-InstallerTrace -Stage 'candidate.copy.begin' -Detail $Paths.InstallRoot
        Copy-CandidateIntoInstall `
            -Paths $Paths `
            -Stage $stage `
            -InstallerScriptRoot $InstallerScriptRoot `
            -UninstallerSource $UninstallerSource `
            -IncludeGateway:$manageGateway
        Write-InstallerTrace -Stage 'candidate.copy.end' -Detail $Paths.InstallRoot
        Write-InstallerTrace -Stage 'acl.begin' -Detail $Paths.InstallRoot
        # Copy-CandidateIntoInstall replaces the prior root and creates a
        # fresh installer-owned root even during an upgrade.
        Set-InstallerAcls `
            -Paths $Paths `
            -InstallRootCreated:$true `
            -DatabaseDirectoryCreated (-not $databaseDirectoryWasPresent) `
            -LogDirectoryCreated (-not $logDirectoryWasPresent) `
            -InstallerStateRootCreated (-not $installerStateRootWasPresent) `
            -ConfigCreated $configWasCreated `
            -ManageGateway:$manageGateway `
            -GatewayProgramDataRootCreated ($manageGateway -and -not $gatewayProgramDataRootWasPresent) `
            -GatewayDataDirectoryCreated ($manageGateway -and -not $gatewayDataDirectoryWasPresent) `
            -GatewayLogDirectoryCreated ($manageGateway -and -not $gatewayLogDirectoryWasPresent) `
            -GatewayConfigCreated $gatewayConfigWasCreated
        Write-InstallerTrace -Stage 'acl.end' -Detail $Paths.InstallRoot
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PayloadInstalled'
        Write-InstallerTrace -Stage 'service.create.begin' -Detail $Paths.ServiceName
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceCreatePending'
        $newService = New-InstallerService -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath
        # New-InstallerService has verified the complete SCM definition.  Only
        # after it returns is the service considered transaction-owned by the
        # outer rollback path; a create race must not cause the cleanup path to
        # delete an unowned service.
        $serviceCreated = $true
        Write-InstallerTrace -Stage 'service.create.end' -Detail $Paths.ServiceName
        if ($manageGateway) {
            Write-InstallerTrace -Stage 'gateway.service.create.begin' -Detail $Paths.GatewayServiceName
            $installedGateway = Assert-GatewayPayloadLayout -GatewayPayloadRoot $Paths.GatewayInstallRoot
            Assert-GatewayPayloadBinary -GatewayPayload $installedGateway | Out-Null
            $gatewayRelease = $installedGateway.Contract
            $gatewayServiceCreationAttempted = $true
            $newGatewayService = New-InstallerGatewayService -Paths $Paths
            $gatewayServiceCreated = $true
            Write-InstallerTrace -Stage 'gateway.service.create.end' -Detail $Paths.GatewayServiceName
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceCreated'

        if ($isUpgrade -or $StartService) {
            Write-InstallerTrace -Stage 'service.start.begin' -Detail $Paths.ServiceName
            Start-InstallerService | Out-Null
            Write-InstallerTrace -Stage 'service.start.end' -Detail $Paths.ServiceName
            $healthVersion = $VersionContract.Version
            if ($TestOnly -and $FailureInjection -eq 'HealthMismatch') {
                $healthVersion = '999.999.999'
            }
            Write-InstallerTrace -Stage 'health.check.begin' -Detail $healthVersion
            Invoke-HealthVersionCheck -ExpectedVersion $healthVersion | Out-Null
            Write-InstallerTrace -Stage 'health.check.end' -Detail $healthVersion
        }
        if ($manageGateway) {
            Write-InstallerTrace -Stage 'gateway.service.start.begin' -Detail $Paths.GatewayServiceName
            Start-InstallerGatewayService -Paths $Paths | Out-Null
            Write-InstallerTrace -Stage 'gateway.service.start.end' -Detail $Paths.GatewayServiceName
            if ($TestOnly -and $FailureInjection -eq 'GatewaySmokeFailure') {
                throw 'Acceptance-only gateway smoke failure injection requested.'
            }
            Write-InstallerTrace -Stage 'gateway.smoke.begin' -Detail '127.0.0.1:7600'
            Invoke-GatewaySmokeCheck -Paths $Paths | Out-Null
            Write-InstallerTrace -Stage 'gateway.smoke.end' -Detail '127.0.0.1:7600'
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'HealthChecked'

        if ($TestOnly -and $FailureInjection -eq 'CommitFailure') {
            throw 'Acceptance-only commit failure injection requested.'
        }

        if ($isUpgrade -and -not $priorWasRunning) {
            Write-InstallerTrace -Stage 'service.restore-stopped.begin' -Detail $Paths.ServiceName
            Stop-InstallerService | Out-Null
            Write-InstallerTrace -Stage 'service.restore-stopped.end' -Detail $Paths.ServiceName
        }
        if ($manageGateway -and -not $finalGatewayRunning) {
            Write-InstallerTrace -Stage 'gateway.service.restore-stopped.begin' -Detail $Paths.GatewayServiceName
            Stop-InstallerGatewayService -Paths $Paths | Out-Null
            Write-InstallerTrace -Stage 'gateway.service.restore-stopped.end' -Detail $Paths.GatewayServiceName
        }

        if (-not $isUpgrade) {
            Write-InstallerTrace -Stage 'path.update.begin' -Detail $Paths.InstallRoot
            $pathResult = if ($AddToPath) {
                Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdatePending'
                Set-MachinePathExact -Operation Add -Entry $Paths.InstallRoot
            } else {
                [pscustomobject]@{ Changed = $false; Value = $null }
            }
            $pathChangedByTransaction = [bool]$pathResult.Changed
            # Only remove a PATH entry on uninstall when this transaction
            # actually added it.  A pre-existing exact entry belongs to the
            # machine administrator, not to this installer.
            $pathManaged = $AddToPath -and $pathChangedByTransaction
            Write-InstallerTrace -Stage 'path.update.end' -Detail ("managed={0}; changed={1}" -f $pathManaged, $pathChangedByTransaction)
            Update-TransactionJournalFields -Paths $Paths -Fields @{ PathChangedByTransaction = $pathChangedByTransaction }
            Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdated'
        } else {
            if ($AddToPath) {
                Write-InstallerTrace -Stage 'path.update.begin' -Detail $Paths.InstallRoot
                Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdatePending'
                $pathResult = Set-MachinePathExact -Operation Add -Entry $Paths.InstallRoot
                $pathManaged = $PriorState.PathManaged -or [bool]$pathResult.Changed
                $pathChangedByTransaction = [bool]$pathResult.Changed
                Write-InstallerTrace -Stage 'path.update.end' -Detail ("managed={0}; changed={1}" -f $pathManaged, $pathChangedByTransaction)
                Update-TransactionJournalFields -Paths $Paths -Fields @{ PathChangedByTransaction = $pathChangedByTransaction }
                Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdated'
            } elseif ($PriorState.PathManaged) {
                Write-InstallerTrace -Stage 'path.update.begin' -Detail $Paths.InstallRoot
                Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdatePending'
                $pathResult = Set-MachinePathExact -Operation Remove -Entry $Paths.InstallRoot
                $pathManaged = $false
                $pathChangedByTransaction = [bool]$pathResult.Changed
                Write-InstallerTrace -Stage 'path.update.end' -Detail ("managed={0}; changed={1}" -f $pathManaged, $pathChangedByTransaction)
                Update-TransactionJournalFields -Paths $Paths -Fields @{ PathChangedByTransaction = $pathChangedByTransaction }
                Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdated'
            } else {
                $pathManaged = $false
            }
        }

        if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath)) {
            Write-InstallerTrace -Stage 'shortcut.create.begin' -Detail $Paths.ShortcutPath
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ShortcutCreatePending'
            New-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
            $shortcutCreated = $true
            Write-InstallerTrace -Stage 'shortcut.create.end' -Detail $Paths.ShortcutPath
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ShortcutCreated'
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'MetadataPending'
        Write-InstallerTrace -Stage 'metadata.write.begin' -Detail $Paths.MarkerPath
        Write-OwnershipMetadata `
            -Paths $Paths `
            -Version $VersionContract.Version `
            -PathManaged $pathManaged `
            -GatewayManaged:$manageGateway `
            -GatewayContract $gatewayRelease
        Write-InstallerTrace -Stage 'metadata.write.end' -Detail $Paths.MarkerPath
        Write-InstallerTrace -Stage 'journal.remove.begin' -Detail $Paths.InstallerStateRoot
        Remove-TransactionJournal -Paths $Paths
        Write-InstallerTrace -Stage 'journal.remove.end' -Detail $Paths.InstallerStateRoot

        Write-Host ("BHTune {0} installation completed. ProgramData is preserved at {1}." -f $VersionContract.Version, $Paths.ProgramDataRoot)
        Write-InstallerTrace -Stage 'transaction.success' -Detail $VersionContract.Version
    } catch {
        $failure = $_
        Write-InstallerTrace -Stage 'transaction.failure' -Detail $failure.Exception.Message
        $rollbackFailure = $null
        try {
            if ($rollbackRequired) {
                if ($null -ne $backup -and $backupPromoted -and (Test-Path -LiteralPath $backup.FinalRoot)) {
                    Write-InstallerTrace -Stage 'rollback.begin' -Detail $backup.FinalRoot
                    $failedJournal = Read-TransactionJournal -Paths $Paths
                    $allowPartialGatewayRegistration = $null -ne $failedJournal -and
                        [string]$failedJournal.Phase -eq 'ServiceCreatePending'
                    Restore-RollbackBackup `
                        -Paths $Paths `
                        -BackupRoot $backup.FinalRoot `
                        -AllowPartialGatewayRegistration:$allowPartialGatewayRegistration
                    Remove-TransactionJournal -Paths $Paths
                    Write-InstallerTrace -Stage 'rollback.end' -Detail $backup.FinalRoot
                    Write-Warning 'The installation transaction failed and the verified prior state was restored.'
                } else {
                    Write-InstallerTrace -Stage 'clean-rollback.begin' -Detail $Paths.InstallRoot
                    # No verified backup means no replacement was committed.
                    # Preserve the prior installation and only restore its
                    # running/stopped state.
                    $current = Get-ServiceSnapshot -Name $Paths.ServiceName
                    if ($priorWasRunning -and $null -ne $current -and $current.State -eq 'Stopped') {
                        Start-InstallerService | Out-Null
                    }
                    if ($gatewayWasManaged -and $priorGatewayWasRunning) {
                        $currentGateway = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
                        if ($null -ne $currentGateway -and $currentGateway.State -eq 'Stopped') {
                            Start-InstallerGatewayService -Paths $Paths | Out-Null
                            Invoke-GatewaySmokeCheck -Paths $Paths | Out-Null
                        }
                    }
                    Remove-InstallerCreatedConfig `
                        -Paths $Paths `
                        -ConfigWasPresent $configWasPresent `
                        -ConfigCreationPending $configCreationPending `
                        -ConfigWasCreated $configWasCreated `
                        -ExpectedConfigHash $expectedConfigHash `
                        -CreatedConfigHash $createdConfigHash
                    if ($manageGateway) {
                        Remove-InstallerCreatedGatewayConfig `
                            -Paths $Paths `
                            -ConfigWasPresent $gatewayConfigWasPresent `
                            -ConfigCreationPending $gatewayConfigCreationPending `
                            -ConfigWasCreated $gatewayConfigWasCreated `
                            -ExpectedConfigHash $expectedGatewayConfigHash `
                            -CreatedConfigHash $createdGatewayConfigHash
                    }
                    Remove-TransactionJournal -Paths $Paths
                }
            } else {
                $current = Get-ServiceSnapshot -Name $Paths.ServiceName
                if ($null -ne $current) {
                    if (-not $serviceCreated) {
                        throw "Clean-install rollback found service '$($Paths.ServiceName)' before the installer completed registration; refusing to remove it."
                    }
                    if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $current -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                        throw "Clean-install rollback found a conflicting service '$($Paths.ServiceName)'; refusing to remove it."
                    }
                    if ($current.State -ne 'Stopped') {
                        Stop-InstallerService | Out-Null
                    }
                    Remove-ServiceByName -Name $Paths.ServiceName
                }
                if ($manageGateway) {
                    $currentGateway = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
                    if ($null -ne $currentGateway) {
                        if (-not $gatewayServiceCreated) {
                            if (-not $gatewayServiceCreationAttempted -or
                                -not (Test-InstallerCreatedGatewayServiceSnapshot `
                                        -ServiceSnapshot $currentGateway `
                                        -ExecutablePath $Paths.GatewayExecutable `
                                        -ConfigPath $Paths.GatewayConfigPath `
                                        -LogDirectory $Paths.GatewayLogDirectory)) {
                                throw "Clean-install rollback found service '$($Paths.GatewayServiceName)' before the installer completed registration; refusing to remove it."
                            }
                            Remove-InstallerCreatedGatewayServiceRegistration -Paths $Paths
                        } else {
                            if (-not (Test-OwnedGatewayServiceSnapshot `
                                    -ServiceSnapshot $currentGateway `
                                    -ExecutablePath $Paths.GatewayExecutable `
                                    -ConfigPath $Paths.GatewayConfigPath `
                                    -LogDirectory $Paths.GatewayLogDirectory)) {
                                throw "Clean-install rollback found a conflicting service '$($Paths.GatewayServiceName)'; refusing to remove it."
                            }
                            Stop-InstallerGatewayService -Paths $Paths | Out-Null
                            Remove-InstallerGatewayService -Paths $Paths
                        }
                    }
                }
                Remove-RegistryKey -Path $Paths.MarkerPath
                Remove-RegistryKey -Path $Paths.UninstallKeyPath
                if ($pathChangedByTransaction -and -not $pathEntryWasPresent) {
                    Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent:$false | Out-Null
                }
                if ($shortcutCreated) {
                    Remove-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
                }
                if (-not $isUpgrade) {
                    Remove-InstallerCreatedConfig `
                        -Paths $Paths `
                        -ConfigWasPresent $configWasPresent `
                        -ConfigCreationPending $configCreationPending `
                        -ConfigWasCreated $configWasCreated `
                        -ExpectedConfigHash $expectedConfigHash `
                        -CreatedConfigHash $createdConfigHash
                }
                if ($manageGateway) {
                    Remove-InstallerCreatedGatewayConfig `
                        -Paths $Paths `
                        -ConfigWasPresent $gatewayConfigWasPresent `
                        -ConfigCreationPending $gatewayConfigCreationPending `
                        -ConfigWasCreated $gatewayConfigWasCreated `
                        -ExpectedConfigHash $expectedGatewayConfigHash `
                        -CreatedConfigHash $createdGatewayConfigHash
                    if (-not $gatewayProgramDataRootWasPresent -and
                        (Test-Path -LiteralPath $Paths.GatewayProgramDataRoot)) {
                        Assert-NoReparsePointInPath `
                            -Path $Paths.GatewayProgramDataRoot `
                            -Name 'the transaction-created OPC DA gateway ProgramData root'
                        Remove-Item -LiteralPath $Paths.GatewayProgramDataRoot -Recurse -Force
                    }
                }
                if (Test-Path -LiteralPath $Paths.InstallRoot) {
                    if ($installRootWasPresent -or -not (Test-CleanInstallRootContents -InstallRoot $Paths.InstallRoot)) {
                        throw "Clean-install rollback could not prove that '$($Paths.InstallRoot)' contains only transaction-created payload."
                    }
                    Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
                }
                Remove-TransactionJournal -Paths $Paths
                Write-InstallerTrace -Stage 'clean-rollback.end' -Detail $Paths.InstallRoot
            }
        } catch {
            $rollbackFailure = $_
            Write-InstallerTrace -Stage 'rollback.failure' -Detail $rollbackFailure.Exception.Message
        }

        if ($null -ne $rollbackFailure) {
            throw ("BHTune installer failed: {0}`nRollback also failed: {1}`nVerified rollback backup: {2}" -f $failure.Exception.Message, $rollbackFailure.Exception.Message, $(if ($null -ne $backup) { $backup.FinalRoot } else { 'none' }))
        }
        throw $failure
    } finally {
        Remove-StagedPayload -Stage $stage
        Write-InstallerTrace -Stage 'transaction.finally' -Detail $VersionContract.Version
    }
}

function Invoke-ConservativeUninstall {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [bool]$LeaveInstallRoot,

        [Parameter(Mandatory = $false)]
        [bool]$FinalizeUninstall = $false
    )

    if ($FinalizeUninstall -and $LeaveInstallRoot) {
        throw 'Uninstall finalization cannot leave the install root in place.'
    }

    $state = Assert-InstallerState `
        -Paths $Paths `
        -AllowMissingService:$true `
        -AllowMissingInstallRoot:$true
    if (-not $state.IsUpgrade) {
        throw 'BHTune is not installer-owned; refusing to uninstall an unowned installation.'
    }

    $journal = Read-TransactionJournal -Paths $Paths
    if ($null -eq $journal) {
        $journal = [pscustomobject]([ordered]@{
            SchemaVersion      = $script:InstallerSchemaVersion
            Mode               = 'Uninstall'
            Phase              = 'Begin'
            BackupRoot         = $null
            Version            = $state.Version
            InstallRoot        = $Paths.InstallRoot
            UninstallerPath    = $Paths.UninstallerPath
            GatewayManaged     = [bool]$state.GatewayManaged
        })
        Write-TransactionJournal -Paths $Paths -Value $journal
    } elseif ([string]$journal.Mode -ne 'Uninstall') {
        throw "The existing installer transaction journal is for '$($journal.Mode)', not uninstall. Refusing to overlap transactions."
    }
    $journalIdentityNames = @('Version', 'InstallRoot', 'UninstallerPath')
    $journalIdentityPresent = @($journalIdentityNames | Where-Object {
            $journal.PSObject.Properties[$_] -and
            -not [string]::IsNullOrWhiteSpace([string]$journal.$_)
        }).Count
    if ($journalIdentityPresent -ne 0 -and $journalIdentityPresent -ne $journalIdentityNames.Count) {
        throw 'The uninstall transaction journal contains incomplete installer identity metadata.'
    }
    if ($journalIdentityPresent -eq 0) {
        $journal.Version = $state.Version
        $journal.InstallRoot = $Paths.InstallRoot
        $journal.UninstallerPath = $Paths.UninstallerPath
        Write-TransactionJournal -Paths $Paths -Value $journal
    }
    if (-not $journal.PSObject.Properties['GatewayManaged']) {
        $journal | Add-Member -MemberType NoteProperty -Name GatewayManaged -Value ([bool]$state.GatewayManaged)
        Write-TransactionJournal -Paths $Paths -Value $journal
    } elseif ([bool]$journal.GatewayManaged -ne [bool]$state.GatewayManaged) {
        throw 'The uninstall transaction journal and ownership marker disagree about OPC DA gateway ownership.'
    }
    $phase = [string]$journal.Phase

    if ($phase -eq 'Begin') {
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopPending'
        $phase = 'ServiceStopPending'
    }

    if ($phase -eq 'ServiceStopPending') {
        if ($state.GatewayManaged) {
            $currentGatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
            if ($null -ne $currentGatewayService) {
                if (-not (Test-OwnedGatewayServiceSnapshot `
                        -ServiceSnapshot $currentGatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                    throw "Service '$($Paths.GatewayServiceName)' is not the installer-owned LocalSystem gateway service. Refusing to remove it."
                }
                Stop-InstallerGatewayService -Paths $Paths | Out-Null
            }
        }
        $currentService = Get-ServiceSnapshot -Name $Paths.ServiceName
        if ($null -ne $currentService) {
            if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $currentService -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "Service '$($Paths.ServiceName)' is not the installer-owned LocalService service. Refusing to remove it."
            }
            if ($currentService.State -ne 'Stopped') {
                Stop-InstallerService | Out-Null
            }
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemovePending'
        $phase = 'ServiceRemovePending'
    }

    if ($phase -eq 'ServiceRemovePending') {
        if ($state.GatewayManaged) {
            $gatewayRegistrationExists = Invoke-ServiceRegistrationQuery -Name $Paths.GatewayServiceName
            if ($gatewayRegistrationExists) {
                $currentGatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
                if ($null -eq $currentGatewayService) {
                    throw "Service '$($Paths.GatewayServiceName)' remains registered but cannot be inspected. Refusing to remove it."
                }
                if (-not (Test-OwnedGatewayServiceSnapshot `
                        -ServiceSnapshot $currentGatewayService `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                    throw "Service '$($Paths.GatewayServiceName)' is not the installer-owned LocalSystem gateway service. Refusing to remove it."
                }
                Remove-InstallerGatewayService -Paths $Paths
            }
        }
        $registrationExists = Invoke-ServiceRegistrationQuery -Name $Paths.ServiceName
        if ($registrationExists) {
            $currentService = Get-ServiceSnapshot -Name $Paths.ServiceName
            if ($null -eq $currentService) {
                throw "Service '$($Paths.ServiceName)' remains registered but cannot be inspected. Refusing to remove it."
            }
            if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $currentService -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "Service '$($Paths.ServiceName)' is not the installer-owned LocalService service. Refusing to remove it."
            }
            Remove-ServiceByName -Name $Paths.ServiceName
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemoved'
        $phase = 'ServiceRemoved'
    }

    if ($phase -eq 'ServiceRemoved' -and $state.PathManaged) {
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathUpdatePending'
        Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent:$false | Out-Null
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathRemoved'
        $phase = 'PathRemoved'
    } elseif ($phase -eq 'ServiceRemoved') {
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathRemoved'
        $phase = 'PathRemoved'
    }

    if ($phase -eq 'PathUpdatePending') {
        Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent:$false | Out-Null
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathRemoved'
        $phase = 'PathRemoved'
    }

    if ($phase -eq 'PathRemoved') {
        if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
            (Test-Path -LiteralPath $Paths.ShortcutPath -PathType Leaf) -and
            -not (Test-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath)) {
            throw "The shortcut path '$($Paths.ShortcutPath)' contains unexpected content. Refusing to delete it."
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ShortcutRemovePending'
        Remove-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ShortcutRemoved'
        $phase = 'ShortcutRemoved'
    }

    if ($phase -eq 'ShortcutRemoved') {
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ReadyForProgramFilesCleanup'
        $phase = 'ReadyForProgramFilesCleanup'
    }

    if ($phase -eq 'ShortcutRemovePending') {
        if (-not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
            (Test-Path -LiteralPath $Paths.ShortcutPath -PathType Leaf)) {
            if (-not (Test-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath)) {
                throw "The shortcut path '$($Paths.ShortcutPath)' contains unexpected content. Refusing to delete it."
            }
            Remove-StartMenuShortcut -ShortcutPath $Paths.ShortcutPath
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ShortcutRemoved'
        $phase = 'ShortcutRemoved'
    }

    if (-not $LeaveInstallRoot -and $phase -eq 'ReadyForProgramFilesCleanup') {
        if ($FinalizeUninstall) {
            if (Test-Path -LiteralPath $Paths.InstallRoot) {
                throw "Uninstall finalization requires the fixed Program Files tree '$($Paths.InstallRoot)' to be absent."
            }
        } elseif (Test-Path -LiteralPath $Paths.InstallRoot) {
            Assert-InstallerState `
                -Paths $Paths `
                -AllowMissingService:$true `
                -AllowMissingInstallRoot:$false | Out-Null
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ProgramFilesRemovePending'
            Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
        }
        if (Test-Path -LiteralPath $Paths.InstallRoot) {
            throw "Program Files cleanup was incomplete: $($Paths.InstallRoot). Installer ownership metadata was retained so the uninstall can be retried safely."
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ProgramFilesRemoved'
        $phase = 'ProgramFilesRemoved'
    }

    if ($phase -eq 'ProgramFilesRemovePending') {
        if ($FinalizeUninstall -and (Test-Path -LiteralPath $Paths.InstallRoot)) {
            throw "Uninstall finalization found residual Program Files content at '$($Paths.InstallRoot)'. Refusing to remove ownership metadata."
        }
        if (-not $FinalizeUninstall -and (Test-Path -LiteralPath $Paths.InstallRoot)) {
            Assert-InstallerState `
                -Paths $Paths `
                -AllowMissingService:$true `
                -AllowMissingInstallRoot:$false | Out-Null
            Remove-Item -LiteralPath $Paths.InstallRoot -Recurse -Force
        }
        if (Test-Path -LiteralPath $Paths.InstallRoot) {
            throw "Program Files cleanup was incomplete: $($Paths.InstallRoot). Installer ownership metadata was retained so the uninstall can be retried safely."
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ProgramFilesRemoved'
        $phase = 'ProgramFilesRemoved'
    }

    if ($LeaveInstallRoot) {
        if ($phase -ne 'ReadyForProgramFilesCleanup') {
            throw "The uninstall transaction is at phase '$phase'; leaving the install root requires ReadyForProgramFilesCleanup."
        }
        # NSIS removes $INSTDIR after this script returns.  Keep the journal
        # and ownership metadata until the guarded finalization pass verifies
        # that the fixed tree is gone.
        Start-UninstallFinalizer -Paths $Paths
        Write-Host "BHTune service, PATH entry, and shortcut were removed. Program Files cleanup is being finalized by the uninstaller; installer ownership metadata, configuration, databases, logs, and rollback data were preserved at $($Paths.ProgramDataRoot)."
    } else {
        if (-not $FinalizeUninstall) {
            throw 'Removing installer ownership metadata requires the explicit uninstall finalization pass.'
        }
        if (Test-Path -LiteralPath $Paths.InstallRoot) {
            throw "Uninstall finalization found unexpected content at '$($Paths.InstallRoot)'. Refusing to remove ownership metadata."
        }
        Assert-InstallerMetadataForRecovery `
            -Paths $Paths `
            -Journal $journal `
            -Marker $state.Marker `
            -Uninstall $state.Uninstall `
            -RequireJournalIdentity:$true | Out-Null
        Update-TransactionJournalPhase -Paths $Paths -Phase 'MetadataRemovePending'
        Remove-RegistryKey -Path $Paths.MarkerPath
        Remove-RegistryKey -Path $Paths.UninstallKeyPath
        Update-TransactionJournalPhase -Paths $Paths -Phase 'MetadataRemoved'
        Remove-TransactionJournal -Paths $Paths
        Write-Host "BHTune was uninstalled. Configuration, databases, logs, and rollback data were preserved at $($Paths.ProgramDataRoot)."
    }
}

$transactionLock = $null
try {
    Write-InstallerTrace -Stage 'script.begin' -Detail ("mode={0}; testOnly={1}" -f $Mode, $TestOnly)
    Assert-FailureInjectionPolicy -FailureInjection $FailureInjection -TestOnly ([bool]$TestOnly) | Out-Null
    Write-InstallerTrace -Stage 'script.options.validated' -Detail ("mode={0}; finalize={1}" -f $Mode, $FinalizeUninstall)
    if ($FinalizeUninstall -and $Mode -ne 'Uninstall') {
        throw 'The uninstall finalization switch is valid only with -Mode Uninstall.'
    }
    if ($WaitForProcessId -gt 0) {
        if ($Mode -ne 'Uninstall' -or -not $FinalizeUninstall) {
            throw 'Waiting for a parent process is valid only for uninstall finalization.'
        }
        Write-InstallerTrace -Stage 'uninstall.finalizer.wait-begin' -Detail ("pid={0}; expected={1}" -f `
                $WaitForProcessId, $WaitForProcessPath)
        Wait-ForParentProcessExit `
            -ProcessId $WaitForProcessId `
            -ExpectedPath $WaitForProcessPath
        Write-InstallerTrace -Stage 'uninstall.finalizer.wait-end' -Detail ([string]$WaitForProcessId)
    }
    if ($Mode -eq 'Install') {
        if ([string]::IsNullOrWhiteSpace($PayloadRoot)) {
            throw 'Install mode requires -PayloadRoot.'
        }
        if ([string]::IsNullOrWhiteSpace($InstallerScriptRoot)) {
            throw 'Install mode requires -InstallerScriptRoot.'
        }
        if ([string]::IsNullOrWhiteSpace($UninstallerSource)) {
            throw 'Install mode requires -UninstallerSource.'
        }
        if (-not [bool]$TestOnly -and [string]::IsNullOrWhiteSpace($ReleaseTag)) {
            throw 'Install mode requires -ReleaseTag=vX.Y.Z.'
        }

        $versionContract = Assert-InstallerVersionContract -ExpectedVersion $ExpectedVersion -ReleaseTag $ReleaseTag
        $paths = Get-InstallerPaths -InstallRoot $InstallRoot -ProgramDataRoot $ProgramDataRoot
        Write-InstallerTrace -Stage 'script.paths.resolved' -Detail $paths.InstallRoot
        if (-not [bool]$TestOnly) {
            Assert-InstallerFixedPaths -Paths $paths | Out-Null
        }
        $transactionLock = Enter-InstallerTransactionLock
        Write-InstallerTrace -Stage 'transaction.lock.acquired' -Detail $paths.InstallerStateRoot
        Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
        $recovery = Recover-InterruptedTransaction -Paths $paths -FinalizeUninstall:$false
        Write-InstallerTrace -Stage 'transaction.recovery.completed' -Detail $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Action })
        if ($null -ne $recovery -and [string]$recovery.Action -eq 'ResumeUninstall') {
            throw "An uninstall transaction is still finalizing for '$($paths.InstallRoot)'. Wait for the uninstall finalizer to finish, or rerun the uninstaller before installing BHTune again."
        }
        # NSIS removes the old service and Program Files tree before invoking
        # this transaction on an upgrade.  The ownership marker and uninstall
        # metadata in ProgramData remain the authority that permits reuse; a
        # missing owned service/root is therefore an expected reinstall state,
        # not evidence of an unowned installation.
        $priorState = Assert-InstallerState `
            -Paths $paths `
            -AllowMissingService:$true `
            -AllowMissingInstallRoot:$true
        Invoke-InstallTransaction `
            -Paths $paths `
            -PriorState $priorState `
            -VersionContract $versionContract `
            -PayloadRoot $PayloadRoot `
            -GatewayPayloadRoot $GatewayPayloadRoot `
            -InstallerScriptRoot $InstallerScriptRoot `
            -UninstallerSource $UninstallerSource `
            -AddToPath $AddToPath `
            -StartService $StartService `
            -InstallGateway $InstallGateway `
            -StartGateway $StartGateway `
            -CustomDbBackupConfirmed $CustomDbBackupConfirmed `
            -TestOnly ([bool]$TestOnly) `
            -FailureInjection $FailureInjection
    } else {
        $paths = Get-InstallerPaths -InstallRoot $InstallRoot -ProgramDataRoot $ProgramDataRoot
        Write-InstallerTrace -Stage 'script.paths.resolved' -Detail $paths.InstallRoot
        if (-not [bool]$TestOnly) {
            Assert-InstallerFixedPaths -Paths $paths | Out-Null
        }
        $transactionLock = Enter-InstallerTransactionLock
        Write-InstallerTrace -Stage 'transaction.lock.acquired' -Detail $paths.InstallerStateRoot
        Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
        $recovery = Recover-InterruptedTransaction `
            -Paths $paths `
            -FinalizeUninstall:([bool]$FinalizeUninstall)
        Write-InstallerTrace -Stage 'transaction.recovery.completed' -Detail $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Action })
        if ($null -eq $recovery -or [string]$recovery.Action -ne 'Finalized') {
            Write-InstallerTrace `
                -Stage 'uninstall.orchestration.begin' `
                -Detail ("phase={0}; finalize={1}" -f `
                    $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Phase }), $FinalizeUninstall)
            Invoke-ConservativeUninstall `
                -Paths $paths `
                -LeaveInstallRoot ([bool]$LeaveInstallRoot) `
                -FinalizeUninstall ([bool]$FinalizeUninstall)
            Write-InstallerTrace -Stage 'uninstall.orchestration.end' -Detail $paths.InstallRoot
        }
    }

    if ($FinalizeUninstall -and -not [string]::IsNullOrWhiteSpace($FinalizerRoot)) {
        Remove-Item -LiteralPath $FinalizerRoot -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $FinalizerRoot) {
            Write-Warning "The uninstall finalizer completed, but its temporary staging root remains at '$FinalizerRoot'."
            Write-InstallerTrace -Stage 'uninstall.finalizer.cleanup-incomplete' -Detail $FinalizerRoot
        } else {
            Write-InstallerTrace -Stage 'uninstall.finalizer.cleaned' -Detail $FinalizerRoot
        }
    }
    exit 0
} catch {
    Write-InstallerTrace -Stage 'script.failure' -Detail $_.Exception.Message
    Write-Error $_.Exception.Message
    exit 1
} finally {
    Exit-InstallerTransactionLock -Lock $transactionLock
}
