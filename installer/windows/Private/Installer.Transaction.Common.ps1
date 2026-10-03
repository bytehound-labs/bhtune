# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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
    if ([string]::IsNullOrWhiteSpace($script:InstallerTracePath)) {
        throw 'Uninstall finalization requires a trace path.'
    }

    $finalizerRoot = Join-Path $Paths.InstallerStateRoot ("uninstall-finalizer-{0}" -f ([guid]::NewGuid().ToString('N')))
    Write-InstallerTrace -Stage 'uninstall.finalizer.stage.begin' -Detail $finalizerRoot
    New-Item -ItemType Directory -Path $finalizerRoot -Force | Out-Null
    Assert-NoReparsePointInPath -Path $finalizerRoot -Name 'the uninstall finalizer staging root'

    $finalizerScript = Join-Path $finalizerRoot 'Install-Bhtune.ps1'
    Copy-InstallerModulePayload `
        -SourceRoot $script:InstallerModuleRoot `
        -DestinationRoot $finalizerRoot
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
        (ConvertTo-PowerShellProcessArgument -Value $script:InstallerTracePath)
    )
    if ($script:InstallerLifecycleTestActive) {
        $arguments += @(
            '-TestOnly',
            '-IsolatedLifecycleTest',
            '-LifecycleTestId',
            $script:InstallerLifecycleTestId,
            '-LifecycleTestRoot',
            (ConvertTo-PowerShellProcessArgument -Value $script:InstallerLifecycleTestRoot)
        )
    }

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
