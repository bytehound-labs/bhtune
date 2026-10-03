# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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
