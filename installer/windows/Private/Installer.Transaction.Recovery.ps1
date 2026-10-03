# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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
            '.bhtune-installer-transaction.json'
        )) {
        [void]$allowed.Add($name)
    }
    foreach ($relativePath in (Get-InstallerModulePayloadRelativePaths)) {
        [void]$allowed.Add(('installer\' + $relativePath.Replace('/', '\')))
    }
    foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
        [void]$allowed.Add(('gateway\' + $name))
    }

    $rootValue = (Get-Item -LiteralPath $InstallRoot -Force).FullName.TrimEnd('\') + '\'
    $allowedDirectories = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($relativePath in @('installer', 'installer\Private', 'gateway')) {
        [void]$allowedDirectories.Add($relativePath)
    }
    foreach ($item in @(Get-ChildItem -LiteralPath $InstallRoot -Force -Recurse -ErrorAction Stop)) {
        $relative = $item.FullName.Substring($rootValue.Length).Replace('/', '\')
        if ($item.PSIsContainer) {
            if (-not $allowedDirectories.Contains($relative)) {
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
