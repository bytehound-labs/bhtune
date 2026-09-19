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
    [ValidateSet('None', 'HealthMismatch', 'CommitFailure')]
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
    New-Item -ItemType Directory -Path $finalizerRoot -Force | Out-Null
    Assert-NoReparsePointInPath -Path $finalizerRoot -Name 'the uninstall finalizer staging root'

    $finalizerScript = Join-Path $finalizerRoot 'Install-Bhtune.ps1'
    $finalizerHelper = Join-Path $finalizerRoot 'InstallerSupport.ps1'
    Copy-Item -LiteralPath $script:InstallerScriptPath -Destination $finalizerScript -Force
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'InstallerSupport.ps1') -Destination $finalizerHelper -Force

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
        $process = Start-Process `
            -FilePath $powershellPath `
            -ArgumentList $arguments `
            -WindowStyle Hidden `
            -PassThru `
            -ErrorAction Stop
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

    if ([int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'SchemaVersion') -ne $script:InstallerSchemaVersion) {
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

    return [pscustomobject]@{
        Version     = $version
        PathManaged = [bool]([int](Get-SnapshotValue -Snapshot $Marker.Values -Name 'PathManaged'))
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
    if ([int]$journal.SchemaVersion -ne $script:InstallerSchemaVersion) {
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
            Restore-RollbackBackup -Paths $Paths -BackupRoot $backupRoot
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
        if ($phase -eq 'ServiceStopPending') {
            if ($null -eq $service) {
                $journal.Phase = 'ServiceRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemoved'
            } elseif (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.ServiceName)'. Refusing to guess."
            } elseif ($service.State -ne 'Stopped') {
                $journal.Phase = 'Begin'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'Begin'
            }
        }
        if ($phase -eq 'ServiceRemovePending') {
            if ($null -eq $service) {
                $journal.Phase = 'ServiceRemoved'
                Write-TransactionJournal -Paths $Paths -Value $journal
                $phase = 'ServiceRemoved'
            } elseif (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.ServiceName)'. Refusing to guess."
            }
        }
        if ($phase -ne 'Begin' -and
            $phase -ne 'ServiceStopPending' -and
            $phase -ne 'ServiceRemovePending' -and
            $null -ne $service) {
            if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "The interrupted uninstall found an unowned service at '$($Paths.ServiceName)'. Refusing to guess."
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

        if ([int](Get-SnapshotValue -Snapshot $marker.Values -Name 'SchemaVersion') -ne $script:InstallerSchemaVersion) {
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
        return [pscustomobject]@{
            IsUpgrade      = $true
            Marker         = $marker
            Uninstall      = $uninstall
            Service        = $service
            ServiceMissing = $null -eq $service
            Version        = $version
            PathManaged    = ([int](Get-SnapshotValue -Snapshot $marker.Values -Name 'PathManaged') -eq 1)
            ShortcutPath   = [string](Get-SnapshotValue -Snapshot $marker.Values -Name 'ShortcutPath')
        }
    }

    if ($null -ne $service) {
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
        IsUpgrade    = $false
        Marker       = $null
        Uninstall    = $null
        Service      = $null
        ServiceMissing = $false
        Version      = $null
        PathManaged  = $false
        ShortcutPath = $Paths.ShortcutPath
    }
}

function Ensure-InstallerDirectories {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    foreach ($directory in @(
            $Paths.ProgramDataRoot,
            $Paths.DatabaseDirectory,
            $Paths.LogDirectory,
            $Paths.InstallerStateRoot
        )) {
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
        [string]$ExpectedVersion
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

    Write-InstallerTrace -Stage 'payload.validate.end' -Detail $stageRoot
    return [pscustomobject]@{
        Root        = $stageRoot
        PayloadRoot = $stagePayload
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
        [bool]$InstallRootWasPresent = $false
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
            foreach ($file in @(Get-ChildItem -LiteralPath $Paths.InstallRoot -Recurse -Force | Where-Object { -not $_.PSIsContainer })) {
                $relative = $file.FullName.Substring($Paths.InstallRoot.Length).TrimStart('\', '/')
                Write-InstallerTrace -Stage 'backup.file.begin' -Detail (Join-Path 'install' $relative)
                Copy-FileToBackup -SourcePath $file.FullName -BackupFilesRoot $backupFilesRoot -RelativePath (Join-Path 'install' $relative) -Manifest $manifest
                Write-InstallerTrace -Stage 'backup.file.end' -Detail (Join-Path 'install' $relative)
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
        if ([string]$State.DatabasePolicy.Policy -eq 'External' -and
            $relativePath.Replace('/', '\').StartsWith('programdata\data\bhtune.db', [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Rollback manifest contains managed database files while the recorded database policy is external.'
        }
    }
    if (-not [bool]$installRootWasPresent -and $hasInstallEntry) {
        throw 'Rollback manifest contains install files even though the prior install root was absent.'
    }
}

function Restore-RollbackBackup {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$BackupRoot
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
    if ($null -eq $state.PSObject.Properties['PathEntryWasPresent']) {
        throw 'Rollback state does not record exact PATH-entry ownership. Refusing to restore the complete machine PATH snapshot.'
    }
    Set-MachinePathEntryState -Entry $Paths.InstallRoot -WasPresent ([bool]$state.PathEntryWasPresent) | Out-Null

    if ($null -ne $state.Service -and $state.Service.State -eq 'Running') {
        Invoke-HealthVersionCheck -ExpectedVersion ([string]$state.Version) | Out-Null
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
        [string]$UninstallerSource
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
        [bool]$PathManaged
    )

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
    foreach ($hashName in @('ExpectedConfigHash', 'CreatedConfigHash')) {
        if ($Journal.PSObject.Properties[$hashName]) {
            $hashValue = $Journal.$hashName
            if ($null -ne $hashValue -and
                -not [string]::IsNullOrWhiteSpace([string]$hashValue) -and
                [string]$hashValue -notmatch '^[0-9a-fA-F]{64}$') {
                throw "The installer transaction journal has an invalid '$hashName' value. Refusing to guess."
            }
        }
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

    $rootValue = (Get-Item -LiteralPath $InstallRoot -Force).FullName.TrimEnd('\') + '\'
    foreach ($item in @(Get-ChildItem -LiteralPath $InstallRoot -Force -Recurse -ErrorAction Stop)) {
        $relative = $item.FullName.Substring($rootValue.Length).Replace('/', '\')
        if ($item.PSIsContainer) {
            if ($relative -ne 'installer') {
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
    if ($children.Count -ne 1 -or -not $children[0].PSIsContainer -or $children[0].Name -ne 'payload') {
        throw "The interrupted staging root '$StageRoot' contains unexpected content. Refusing to delete it."
    }
    $payloadRoot = $children[0].FullName
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
    $pathEntryWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'PathEntryWasPresent')
    $shortcutWasPresent = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'ShortcutWasPresent')
    $pathChanged = [bool](Get-SnapshotValue -Snapshot $Journal -Name 'PathChangedByTransaction')
    $service = Get-ServiceSnapshot -Name $Paths.ServiceName
    $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
    $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath

    if ($installRootWasPresent -or $serviceWasPresent -or $configWasPresent -or
        $pathEntryWasPresent -or $shortcutWasPresent) {
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

        [Parameter(Mandatory = $true)]
        [string]$InstallerScriptRoot,

        [Parameter(Mandatory = $true)]
        [string]$UninstallerSource,

        [Parameter(Mandatory = $true)]
        [bool]$AddToPath,

        [Parameter(Mandatory = $true)]
        [bool]$StartService,

        [Parameter(Mandatory = $true)]
        [bool]$CustomDbBackupConfirmed,

        [Parameter(Mandatory = $true)]
        [bool]$TestOnly,

        [Parameter(Mandatory = $true)]
        [string]$FailureInjection
    )

    $isUpgrade = [bool]$PriorState.IsUpgrade
    $preservedState = Get-PreservedProgramDataState -Paths $Paths
    $rollbackRequired = $isUpgrade -or [bool]$preservedState.ReuseRequired
    $configWasCreated = $false
    $createdConfigHash = $null
    $configCreationPending = $false
    $expectedConfigHash = $null
    $stage = $null
    $backup = $null
    $pathManaged = $false
    $pathChangedByTransaction = $false
    $serviceCreated = $false
    $backupPromoted = $false
    $shortcutCreated = $false
    $priorWasRunning = $isUpgrade -and $null -ne $PriorState.Service -and $PriorState.Service.State -eq 'Running'
    $installRootWasPresent = Test-Path -LiteralPath $Paths.InstallRoot -PathType Container
    $databaseDirectoryWasPresent = Test-Path -LiteralPath $Paths.DatabaseDirectory -PathType Container
    $logDirectoryWasPresent = Test-Path -LiteralPath $Paths.LogDirectory -PathType Container
    $installerStateRootWasPresent = Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container
    $configWasPresent = Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf
    $serviceWasPresent = $null -ne $PriorState.Service
    $shortcutWasPresent = -not [string]::IsNullOrWhiteSpace($Paths.ShortcutPath) -and
        (Test-Path -LiteralPath $Paths.ShortcutPath)
    $transactionId = [guid]::NewGuid().ToString('N')

    try {
        Write-InstallerTrace -Stage 'transaction.begin' -Detail ("mode={0}; upgrade={1}; version={2}; addPath={3}; startService={4}; customDb={5}" -f `
                'Install', $isUpgrade, $VersionContract.Version, $AddToPath, $StartService, $CustomDbBackupConfirmed)
        Write-InstallerTrace -Stage 'preflight.directories.begin' -Detail $Paths.ProgramDataRoot
        Ensure-InstallerDirectories -Paths $Paths
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
        Write-InstallerTrace -Stage 'preflight.database.begin' -Detail $Paths.DatabasePath
        $databasePolicy = Assert-DatabasePolicy -Paths $Paths -IsUpgrade $isUpgrade -CustomDbBackupConfirmed $CustomDbBackupConfirmed -PreservedData ([bool]$preservedState.ReuseRequired)
        Write-InstallerTrace -Stage 'preflight.database.end' -Detail ("policy={0}" -f $databasePolicy.Policy)

        Write-InstallerTrace -Stage 'payload.stage.begin' -Detail $PayloadRoot
        $stage = New-StagedPayload -Paths $Paths -SourcePayloadRoot $PayloadRoot -ExpectedVersion $VersionContract.Version
        Update-TransactionJournalFields -Paths $Paths -Fields @{ StageRoot = $stage.Root }
        Write-InstallerTrace -Stage 'payload.stage.end' -Detail $stage.Root
        Write-InstallerTrace -Stage 'path.snapshot.begin' -Detail 'machine'
        Write-InstallerTrace -Stage 'path.snapshot.end' -Detail 'machine'
        Update-TransactionJournalPhase -Paths $Paths -Phase 'PathSnapshotted'

        if ($isUpgrade -and $null -ne $PriorState.Service) {
            Write-InstallerTrace -Stage 'service.stop.begin' -Detail $Paths.ServiceName
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopPending'
            Stop-InstallerService | Out-Null
            Write-InstallerTrace -Stage 'service.stop.end' -Detail $Paths.ServiceName
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopped'
        }

        if ($rollbackRequired) {
            Write-InstallerTrace -Stage 'backup.create.begin' -Detail $Paths.RollbackRoot
            $backup = New-RollbackBackup `
                -Paths $Paths `
                -PriorState $PriorState `
                -DatabasePolicy $databasePolicy `
                -ConfigWasCreated $configWasCreated `
                -InstallRootWasPresent:$installRootWasPresent
            Write-InstallerTrace -Stage 'backup.create.end' -Detail $backup.PendingRoot
            Write-InstallerTrace -Stage 'backup.promote.begin' -Detail $backup.FinalRoot
            Promote-RollbackBackup -Backup $backup
            Write-InstallerTrace -Stage 'backup.promote.end' -Detail $backup.FinalRoot
            $backupPromoted = $true
            Update-TransactionJournalPhase -Paths $Paths -Phase 'BackupPromoted' -BackupRoot $backup.FinalRoot
        }

        if ($isUpgrade -and $null -ne $PriorState.Service) {
            Write-InstallerTrace -Stage 'service.remove.begin' -Detail $Paths.ServiceName
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemovePending'
            Remove-ServiceByName -Name $Paths.ServiceName
            Write-InstallerTrace -Stage 'service.remove.end' -Detail $Paths.ServiceName
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemoved'
        }

        Update-TransactionJournalPhase -Paths $Paths -Phase 'PayloadStaged'
        Write-InstallerTrace -Stage 'candidate.copy.begin' -Detail $Paths.InstallRoot
        Copy-CandidateIntoInstall -Paths $Paths -Stage $stage -InstallerScriptRoot $InstallerScriptRoot -UninstallerSource $UninstallerSource
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
            -ConfigCreated $configWasCreated
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
            Update-TransactionJournalPhase -Paths $Paths -Phase 'HealthChecked'
        }

        if ($TestOnly -and $FailureInjection -eq 'CommitFailure') {
            throw 'Acceptance-only commit failure injection requested.'
        }

        if ($isUpgrade -and -not $priorWasRunning) {
            Write-InstallerTrace -Stage 'service.restore-stopped.begin' -Detail $Paths.ServiceName
            Stop-InstallerService | Out-Null
            Write-InstallerTrace -Stage 'service.restore-stopped.end' -Detail $Paths.ServiceName
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
        Write-OwnershipMetadata -Paths $Paths -Version $VersionContract.Version -PathManaged $pathManaged
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
                    Restore-RollbackBackup -Paths $Paths -BackupRoot $backup.FinalRoot
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
                    Remove-InstallerCreatedConfig `
                        -Paths $Paths `
                        -ConfigWasPresent $configWasPresent `
                        -ConfigCreationPending $configCreationPending `
                        -ConfigWasCreated $configWasCreated `
                        -ExpectedConfigHash $expectedConfigHash `
                        -CreatedConfigHash $createdConfigHash
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
    $phase = [string]$journal.Phase

    if ($phase -eq 'Begin' -and $null -ne $state.Service) {
        if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $state.Service -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
            throw "Service '$($Paths.ServiceName)' is not the installer-owned LocalService service. Refusing to remove it."
        }
        if ($state.Service.State -ne 'Stopped') {
            Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceStopPending'
            Stop-InstallerService | Out-Null
        }
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemovePending'
        Remove-ServiceByName -Name $Paths.ServiceName
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemoved'
        $phase = 'ServiceRemoved'
    } elseif ($phase -eq 'Begin') {
        # A missing service is an already-completed service-removal step.  The
        # ownership marker still protects every remaining uninstall action.
        Update-TransactionJournalPhase -Paths $Paths -Phase 'ServiceRemoved'
        $phase = 'ServiceRemoved'
    }

    if ($phase -eq 'ServiceStopPending') {
        $currentService = Get-ServiceSnapshot -Name $Paths.ServiceName
        if ($null -ne $currentService) {
            if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $currentService -ExecutablePath $Paths.ServiceExecutable -ConfigPath $Paths.ConfigPath)) {
                throw "Service '$($Paths.ServiceName)' is not the installer-owned LocalService service. Refusing to remove it."
            }
            if ($currentService.State -ne 'Stopped') {
                Stop-InstallerService | Out-Null
            }
        }
        $phase = 'ServiceRemovePending'
    }

    if ($phase -eq 'ServiceRemovePending') {
        $currentService = Get-ServiceSnapshot -Name $Paths.ServiceName
        if ($null -ne $currentService) {
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
        if (-not [bool]$TestOnly) {
            Assert-InstallerFixedPaths -Paths $paths | Out-Null
        }
        $transactionLock = Enter-InstallerTransactionLock
        Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
        $recovery = Recover-InterruptedTransaction -Paths $paths -FinalizeUninstall:$false
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
            -InstallerScriptRoot $InstallerScriptRoot `
            -UninstallerSource $UninstallerSource `
            -AddToPath $AddToPath `
            -StartService $StartService `
            -CustomDbBackupConfirmed $CustomDbBackupConfirmed `
            -TestOnly ([bool]$TestOnly) `
            -FailureInjection $FailureInjection
    } else {
        $paths = Get-InstallerPaths -InstallRoot $InstallRoot -ProgramDataRoot $ProgramDataRoot
        if (-not [bool]$TestOnly) {
            Assert-InstallerFixedPaths -Paths $paths | Out-Null
        }
        $transactionLock = Enter-InstallerTransactionLock
        Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
        $recovery = Recover-InterruptedTransaction `
            -Paths $paths `
            -FinalizeUninstall:([bool]$FinalizeUninstall)
        if ($null -eq $recovery -or [string]$recovery.Action -ne 'Finalized') {
            Invoke-ConservativeUninstall `
                -Paths $paths `
                -LeaveInstallRoot ([bool]$LeaveInstallRoot) `
                -FinalizeUninstall ([bool]$FinalizeUninstall)
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
