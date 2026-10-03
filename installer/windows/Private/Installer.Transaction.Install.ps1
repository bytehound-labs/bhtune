# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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

    Assert-InstallerModulePayload -SourceRoot $InstallerScriptRoot
    $installerSources = @(
        Get-InstallerModulePayloadRelativePaths |
            ForEach-Object {
                Join-Path $InstallerScriptRoot $_
            }
    )
    foreach ($source in ($installerSources + @($UninstallerSource))) {
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
    Copy-InstallerModulePayload `
        -SourceRoot $InstallerScriptRoot `
        -DestinationRoot $Paths.InstallerScriptRoot
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
