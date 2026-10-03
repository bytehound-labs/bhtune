# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Get-RequiredPayloadFiles {
    return @($script:RequiredPayloadFiles)
}

function Get-GatewayRequiredPayloadFiles {
    return @($script:GatewayRequiredPayloadFiles)
}

function Get-ExpectedServiceCommandLine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath
    )

    return '"' + $ExecutablePath + '" --config "' + $ConfigPath + '"'
}

function Get-ExpectedGatewayServiceCommandLine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    return '"' + $ExecutablePath + '" --config "' + $ConfigPath + '" --port ' +
        $script:GatewayPort + ' --log-dir "' + $LogDirectory + '"'
}

function Normalize-ServiceCommandLine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Value
    )

    $withoutQuotes = $Value.Trim().Replace('/', '\').Replace('"', '')
    return [regex]::Replace($withoutQuotes, '\s+', ' ').ToLowerInvariant()
}

function Test-ServiceCommandLine {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ActualPathName,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath
    )

    if ([string]::IsNullOrWhiteSpace($ActualPathName)) {
        return $false
    }

    # WMI may normalize away quotes around paths.  Compare the complete
    # command line after removing only quote and whitespace presentation
    # differences; never use substring matching, which could accept a
    # conflicting executable whose path merely has the expected path as a
    # prefix or an unexpected extra argument.
    $actual = Normalize-ServiceCommandLine -Value $ActualPathName
    $expected = Normalize-ServiceCommandLine -Value (Get-ExpectedServiceCommandLine -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath)
    return $actual.Equals($expected, [System.StringComparison]::OrdinalIgnoreCase)
}

function Test-GatewayServiceCommandLine {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ActualPathName,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    if ([string]::IsNullOrWhiteSpace($ActualPathName)) {
        return $false
    }
    $actual = Normalize-ServiceCommandLine -Value $ActualPathName
    $expected = Normalize-ServiceCommandLine -Value (
        Get-ExpectedGatewayServiceCommandLine `
            -ExecutablePath $ExecutablePath `
            -ConfigPath $ConfigPath `
            -LogDirectory $LogDirectory
    )
    return $actual.Equals($expected, [System.StringComparison]::OrdinalIgnoreCase)
}

function Get-ExpectedUninstallMetadata {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Version,

        [Parameter(Mandatory = $true)]
        [string]$InstallRoot,

        [Parameter(Mandatory = $true)]
        [string]$UninstallerPath
    )

    return [ordered]@{
        DisplayName          = $script:InstallerProductName
        DisplayVersion       = $Version
        Publisher            = $script:InstallerPublisher
        InstallLocation      = $InstallRoot
        DisplayIcon          = $UninstallerPath
        UninstallString      = '"' + $UninstallerPath + '"'
        QuietUninstallString = '"' + $UninstallerPath + '" /S'
        NoModify             = 1
        NoRepair             = 1
        URLInfoAbout         = 'https://github.com/bytehound-labs/bhtune'
    }
}

function Test-UninstallMetadata {
    param(
        [Parameter(Mandatory = $false)]
        [psobject]$Snapshot,

        [Parameter(Mandatory = $true)]
        [string]$Version,

        [Parameter(Mandatory = $true)]
        [string]$InstallRoot,

        [Parameter(Mandatory = $true)]
        [string]$UninstallerPath
    )

    if ($null -eq $Snapshot) {
        return $false
    }

    $expected = Get-ExpectedUninstallMetadata -Version (ConvertTo-NormalizedVersion -Version $Version) -InstallRoot $InstallRoot -UninstallerPath $UninstallerPath
    $values = $Snapshot.Values
    foreach ($name in $expected.Keys) {
        if ($values -is [System.Collections.IDictionary]) {
            if (-not $values.Contains($name)) {
                return $false
            }
            $actualValue = $values[$name]
        } else {
            $property = $values.PSObject.Properties[$name]
            if ($null -eq $property) {
                return $false
            }
            $actualValue = $property.Value
        }

        $actual = [string]$actualValue
        $expectedValue = [string]$expected[$name]
        if ($name -eq 'InstallLocation' -or $name -eq 'DisplayIcon') {
            if ((Normalize-PathForComparison -Path $actual) -ne (Normalize-PathForComparison -Path $expectedValue)) {
                return $false
            }
        } elseif ($actual -cne $expectedValue) {
            return $false
        }
    }

    return $true
}

function Get-MissingPayloadFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PayloadRoot
    )

    $missing = New-Object System.Collections.ArrayList
    foreach ($name in (Get-RequiredPayloadFiles)) {
        $path = Join-Path $PayloadRoot $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            [void]$missing.Add($name)
        }
    }
    return @($missing)
}

function Assert-PayloadLayout {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PayloadRoot
    )

    if (-not (Test-Path -LiteralPath $PayloadRoot -PathType Container)) {
        throw "The staged payload directory does not exist: $PayloadRoot"
    }

    $missing = @(Get-MissingPayloadFiles -PayloadRoot $PayloadRoot)
    if ($missing.Count -gt 0) {
        throw "The staged payload is incomplete. Missing: $($missing -join ', ')."
    }

    return $true
}

function Assert-GatewayReleaseContract {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ContractPath
    )

    if (-not (Test-Path -LiteralPath $ContractPath -PathType Leaf)) {
        throw "The pinned OPC DA gateway release contract is missing: $ContractPath"
    }

    try {
        $contract = Get-Content -LiteralPath $ContractPath -Raw | ConvertFrom-Json
    } catch {
        throw "The pinned OPC DA gateway release contract is invalid JSON: $($_.Exception.Message)"
    }

    if ([int]$contract.schema_version -ne 1) {
        throw "The OPC DA gateway release contract uses unsupported schema '$($contract.schema_version)'."
    }
    if ([string]$contract.repository -cne 'bytehound-labs/opcda-bridge') {
        throw "The OPC DA gateway release contract names unexpected repository '$($contract.repository)'."
    }
    if ([string]$contract.tag -notmatch '^opcda-bridge-gateway-v(?<version>[0-9]+\.[0-9]+\.[0-9]+)$') {
        throw "The OPC DA gateway release contract has invalid stable tag '$($contract.tag)'."
    }
    if ([string]$contract.version -cne $Matches['version']) {
        throw "The OPC DA gateway release contract version does not match its tag."
    }
    foreach ($hash in @(
            [string]$contract.source_commit,
            [string]$contract.archive.sha256,
            [string]$contract.executable.sha256,
            [string]$contract.release_workflow.blob_sha
        )) {
        $expectedLength = if ($hash -eq [string]$contract.source_commit -or
            $hash -eq [string]$contract.release_workflow.blob_sha) { 40 } else { 64 }
        if ($hash -cnotmatch "^[0-9a-f]{$expectedLength}$") {
            throw "The OPC DA gateway release contract contains an invalid hexadecimal digest."
        }
    }
    if ([string]$contract.archive.name -cne 'opcda-bridge-gateway-windows-x86.zip' -or
        @($contract.archive.contents).Count -ne 1 -or
        [string]@($contract.archive.contents)[0] -cne 'opcda-bridge-gateway.exe') {
        throw 'The OPC DA gateway release contract has an unexpected archive layout.'
    }
    if ([string]$contract.executable.name -cne 'opcda-bridge-gateway.exe' -or
        [string]$contract.executable.target -cne 'i686-pc-windows-msvc' -or
        [string]$contract.executable.pe_machine -cne 'I386') {
        throw 'The OPC DA gateway release contract does not describe the supported 32-bit Windows executable.'
    }
    if ([string]$contract.release_workflow.path -cne '.github/workflows/release.yml') {
        throw 'The OPC DA gateway release contract has an unexpected upstream release workflow.'
    }
    $expectedBuilder = "https://github.com/$($contract.repository)/$($contract.release_workflow.path)@refs/tags/$($contract.tag)"
    if ([string]$contract.release_workflow.builder_id -cne $expectedBuilder) {
        throw 'The OPC DA gateway release contract builder identity does not match its repository, workflow, and tag.'
    }
    if ([string]$contract.upstream_evidence.checksums -cne 'release-assets.sha256' -or
        [string]$contract.upstream_evidence.archive_sigstore_bundle -cne 'opcda-bridge-gateway-windows-x86.zip.sigstore.json' -or
        [string]$contract.upstream_evidence.provenance -cne 'attestation.json' -or
        [string]$contract.upstream_evidence.compatibility -cne 'compatibility.json') {
        throw 'The OPC DA gateway release contract has unexpected upstream evidence names.'
    }
    if ([string]$contract.compatibility.release_line -cne 'indexed-on-demand' -or
        [string]$contract.compatibility.min_version -cne '0.5.0' -or
        [string]$contract.compatibility.max_version -cne '0.999.999' -or
        [int]$contract.compatibility.core_protocol -ne 1 -or
        [int]$contract.compatibility.namespace_protocol -ne 2 -or
        [int]$contract.compatibility.indexed_search_protocol -ne 2) {
        throw 'The OPC DA gateway release contract does not describe the supported indexed-on-demand protocol line.'
    }

    return $contract
}

function Get-MissingGatewayPayloadFiles {
    param(
        [Parameter(Mandatory = $true)]
        [string]$GatewayPayloadRoot
    )

    $missing = New-Object System.Collections.ArrayList
    foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
        if (-not (Test-Path -LiteralPath (Join-Path $GatewayPayloadRoot $name) -PathType Leaf)) {
            [void]$missing.Add($name)
        }
    }
    return @($missing)
}

function Assert-GatewayPayloadLayout {
    param(
        [Parameter(Mandatory = $true)]
        [string]$GatewayPayloadRoot
    )

    if (-not (Test-Path -LiteralPath $GatewayPayloadRoot -PathType Container)) {
        throw "The staged OPC DA gateway payload directory does not exist: $GatewayPayloadRoot"
    }

    $missing = @(Get-MissingGatewayPayloadFiles -GatewayPayloadRoot $GatewayPayloadRoot)
    if ($missing.Count -gt 0) {
        throw "The staged OPC DA gateway payload is incomplete. Missing: $($missing -join ', ')."
    }

    $actualFiles = @(
        Get-ChildItem -LiteralPath $GatewayPayloadRoot -File -Recurse |
            ForEach-Object { $_.FullName.Substring($GatewayPayloadRoot.Length).TrimStart('\', '/') } |
            Sort-Object
    )
    $expectedFiles = @(Get-GatewayRequiredPayloadFiles | Sort-Object)
    if (($actualFiles -join '|') -cne ($expectedFiles -join '|')) {
        throw "The staged OPC DA gateway payload contains unexpected files: $($actualFiles -join ', ')."
    }

    $contract = Assert-GatewayReleaseContract -ContractPath (Join-Path $GatewayPayloadRoot 'opcda-gateway-release.json')
    try {
        $provenance = Get-Content -LiteralPath (Join-Path $GatewayPayloadRoot 'opcda-gateway-provenance.json') -Raw | ConvertFrom-Json
    } catch {
        throw "The staged OPC DA gateway provenance manifest is invalid JSON: $($_.Exception.Message)"
    }
    if ([int]$provenance.schema_version -ne 1 -or
        [string]$provenance.repository -cne [string]$contract.repository -or
        [string]$provenance.tag -cne [string]$contract.tag -or
        [string]$provenance.source_commit -cne [string]$contract.source_commit -or
        [string]$provenance.archive_name -cne [string]$contract.archive.name -or
        [string]$provenance.archive_sha256 -cne [string]$contract.archive.sha256 -or
        [string]$provenance.executable_name -cne [string]$contract.executable.name -or
        [string]$provenance.executable_sha256 -cne [string]$contract.executable.sha256 -or
        [string]$provenance.builder_id -cne [string]$contract.release_workflow.builder_id -or
        [string]$provenance.release_workflow_blob_sha -cne [string]$contract.release_workflow.blob_sha -or
        $provenance.sigstore_verified -isnot [bool] -or -not $provenance.sigstore_verified -or
        $provenance.github_provenance_verified -isnot [bool] -or -not $provenance.github_provenance_verified -or
        $provenance.compatibility_verified -isnot [bool] -or -not $provenance.compatibility_verified) {
        throw 'The staged OPC DA gateway provenance manifest does not match the pinned release contract.'
    }

    $executable = Join-Path $GatewayPayloadRoot 'opcda-bridge-gateway.exe'
    if ((Get-FileSha256 -Path $executable) -cne [string]$contract.executable.sha256) {
        throw 'The staged OPC DA gateway executable does not match the pinned SHA-256.'
    }
    foreach ($name in @('LICENSE-opcda-bridge.txt', 'NOTICE-opcda-bridge.txt')) {
        if ((Get-Item -LiteralPath (Join-Path $GatewayPayloadRoot $name)).Length -le 0) {
            throw "The staged OPC DA gateway redistribution file '$name' is empty."
        }
    }

    return [pscustomobject]@{
        Contract   = $contract
        Provenance = $provenance
        Executable = $executable
    }
}

function Get-VersionFromProcessOutput {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Output
    )

    if ($null -eq $Output) {
        return $null
    }

    $lines = @(
        $Output -split "`r?`n" |
            ForEach-Object { $_.Trim() } |
            Where-Object { $_.Length -gt 0 }
    )
    if ($lines.Count -ne 1) {
        return $null
    }

    if ($lines[0] -match '^(?:(?:bhtune(?:-server)?|opcda-bridge-gateway)\s+)?v?([0-9]+\.[0-9]+\.[0-9]+)$') {
        return $Matches[1]
    }

    return $null
}

function Assert-PayloadBinaryVersions {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PayloadRoot,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion
    )

    $expected = ConvertTo-NormalizedVersion -Version $ExpectedVersion
    foreach ($name in @('bhtune.exe', 'bhtune-server.exe')) {
        $path = Join-Path $PayloadRoot $name
        $result = Invoke-CapturedProcess -FilePath $path -Arguments '--version'
        $reported = Get-VersionFromProcessOutput -Output ($result.StdOut + "`n" + $result.StdErr)
        if ($result.ExitCode -ne 0 -or $null -eq $reported) {
            throw "The payload executable '$name' did not report a usable version."
        }
        if ($reported -ne $expected) {
            throw "Payload executable '$name' reports version '$reported', expected '$expected'."
        }
    }

    return $true
}

function Get-PeMachine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Cannot inspect a missing PE file: $Path"
    }

    $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    $reader = New-Object System.IO.BinaryReader($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5a4d) {
            throw "The file is not a PE executable: $Path"
        }
        $stream.Position = 0x3c
        $peOffset = $reader.ReadInt32()
        if ($peOffset -lt 0 -or $peOffset -gt ($stream.Length - 6)) {
            throw "The PE header offset is invalid: $Path"
        }
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "The PE signature is invalid: $Path"
        }
        $machine = $reader.ReadUInt16()
        switch ($machine) {
            0x014c { return 'I386' }
            0x8664 { return 'AMD64' }
            0xaa64 { return 'ARM64' }
            default { return ('0x{0:x4}' -f $machine) }
        }
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

function Assert-GatewayPayloadBinary {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$GatewayPayload
    )

    $contract = $GatewayPayload.Contract
    $executable = [string]$GatewayPayload.Executable
    $machine = Get-PeMachine -Path $executable
    if ($machine -cne [string]$contract.executable.pe_machine) {
        throw "The OPC DA gateway executable has PE machine '$machine', expected '$($contract.executable.pe_machine)'."
    }

    $result = Invoke-CapturedProcess -FilePath $executable -Arguments '--version'
    $reported = Get-VersionFromProcessOutput -Output ($result.StdOut + "`n" + $result.StdErr)
    if ($result.ExitCode -ne 0 -or $null -eq $reported) {
        throw 'The OPC DA gateway executable did not report a usable version.'
    }
    if ($reported -cne [string]$contract.version) {
        throw "The OPC DA gateway executable reports version '$reported', expected '$($contract.version)'."
    }

    return $true
}

function Get-FileSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Cannot hash a missing file: $Path"
    }

    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-TextSha256 {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Content
    )

    $encoding = New-Object System.Text.UTF8Encoding($false)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        $bytes = $encoding.GetBytes($Content)
        return [System.BitConverter]::ToString($sha256.ComputeHash($bytes)).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
    }
}

function New-HashManifestEntry {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourcePath,

        [Parameter(Mandatory = $true)]
        [string]$BackupPath,

        [Parameter(Mandatory = $true)]
        [string]$RelativePath
    )

    if (-not (Test-Path -LiteralPath $SourcePath -PathType Leaf)) {
        throw "Cannot add missing source file to a manifest: $SourcePath"
    }
    if (-not (Test-Path -LiteralPath $BackupPath -PathType Leaf)) {
        throw "Cannot add missing backup file to a manifest: $BackupPath"
    }

    $sourceHash = Get-FileSha256 -Path $SourcePath
    $backupHash = Get-FileSha256 -Path $BackupPath
    $sourceLength = (Get-Item -LiteralPath $SourcePath).Length
    $backupLength = (Get-Item -LiteralPath $BackupPath).Length

    if ($sourceHash -ne $backupHash -or $sourceLength -ne $backupLength) {
        throw "Backup verification failed for '$SourcePath'."
    }

    return [pscustomobject]@{
        RelativePath = $RelativePath.Replace('/', '\')
        SourcePath   = $SourcePath
        BackupPath   = $BackupPath
        Length       = [int64]$sourceLength
        Sha256       = $sourceHash
    }
}

function Test-HashManifestEntry {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Entry
    )

    if (-not (Test-Path -LiteralPath $Entry.BackupPath -PathType Leaf)) {
        return $false
    }

    $item = Get-Item -LiteralPath $Entry.BackupPath
    if ([int64]$item.Length -ne [int64]$Entry.Length) {
        return $false
    }

    return (Get-FileSha256 -Path $Entry.BackupPath) -eq $Entry.Sha256.ToLowerInvariant()
}

function Test-RollbackBackup {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BackupRoot
    )

    $manifestPath = Join-Path $BackupRoot 'manifest.json'
    $statePath = Join-Path $BackupRoot 'state.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $statePath -PathType Leaf)) {
        return $false
    }

    try {
        Assert-NoReparsePointInPath -Path $BackupRoot -Name 'the rollback backup root'
        Assert-NoReparsePointInPath -Path $manifestPath -Name 'the rollback manifest'
        Assert-NoReparsePointInPath -Path $statePath -Name 'the rollback state'
        $state = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        if (-not (Test-SupportedInstallerSchemaVersion -Version ([int]$state.SchemaVersion))) {
            return $false
        }
        if (-not $state.PSObject.Properties['InstallRootWasPresent']) {
            return $false
        }
        $installRootWasPresent = $state.InstallRootWasPresent
        if ($installRootWasPresent -isnot [bool] -and
            [string]$installRootWasPresent -ne 'True' -and
            [string]$installRootWasPresent -ne 'False') {
            return $false
        }
        if ([int]$state.SchemaVersion -ge 3) {
            foreach ($name in @(
                    'GatewayManaged',
                    'GatewayWasManaged',
                    'GatewayProgramDataRootWasPresent'
                )) {
                if (-not $state.PSObject.Properties[$name]) {
                    return $false
                }
                $value = $state.$name
                if ($value -isnot [bool] -and
                    [string]$value -ne 'True' -and
                    [string]$value -ne 'False') {
                    return $false
                }
            }
        }
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if (-not (Test-SupportedInstallerSchemaVersion -Version ([int]$manifest.SchemaVersion))) {
            return $false
        }
        $filesRoot = Join-Path $BackupRoot 'files'
        if (-not (Test-Path -LiteralPath $filesRoot -PathType Container)) {
            return $false
        }
        Assert-NoReparsePointInPath -Path $filesRoot -Name 'the rollback files root'
        $seenRelativePaths = @{}
        foreach ($entry in @($manifest.Files)) {
            $relativePath = [string]$entry.RelativePath
            if (-not (Test-SafeRollbackRelativePath -RelativePath $relativePath)) {
                return $false
            }
            $relativePath = $relativePath.Replace('/', '\')
            $relativeKey = $relativePath.ToLowerInvariant()
            if ($seenRelativePaths.ContainsKey($relativeKey)) {
                return $false
            }
            $seenRelativePaths[$relativeKey] = $true

            $expectedBackupPath = Join-Path $filesRoot $relativePath
            $expectedBackupComparable = Get-ComparableAbsolutePath -Path $expectedBackupPath
            $actualBackupComparable = Get-ComparableAbsolutePath -Path ([string]$entry.BackupPath)
            if ([string]::IsNullOrWhiteSpace($expectedBackupComparable) -or
                $expectedBackupComparable -ne $actualBackupComparable) {
                return $false
            }
            if ([string]::IsNullOrWhiteSpace((Get-ComparableAbsolutePath -Path ([string]$entry.SourcePath)))) {
                return $false
            }
            Assert-NoReparsePointInPath -Path ([string]$entry.BackupPath) -Name 'a rollback backup file'
            if (-not (Test-HashManifestEntry -Entry $entry)) {
                return $false
            }
        }
    } catch {
        return $false
    }

    return $true
}

function Assert-GatewayInfoPayload {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Json,

        [Parameter(Mandatory = $true)]
        [psobject]$Contract
    )

    try {
        $payload = $Json | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "The OPC DA gateway smoke check did not return valid JSON: $($_.Exception.Message)"
    }
    if ([string]$payload.application_version -cne [string]$Contract.version) {
        throw "The running OPC DA gateway reports version '$($payload.application_version)' instead of '$($Contract.version)'."
    }
    if ([int]$payload.compatibility_schema_version -ne 1) {
        throw "The running OPC DA gateway reports unsupported compatibility schema '$($payload.compatibility_schema_version)'."
    }
    if ($null -eq $payload.PSObject.Properties['features']) {
        throw 'The OPC DA gateway smoke check JSON did not contain protocol features.'
    }

    $expectedProtocols = @(
        [pscustomobject]@{
            Name    = 'core'
            Version = [int]$Contract.compatibility.core_protocol
        },
        [pscustomobject]@{
            Name    = 'namespace'
            Version = [int]$Contract.compatibility.namespace_protocol
        },
        [pscustomobject]@{
            Name    = 'indexed_search'
            Version = [int]$Contract.compatibility.indexed_search_protocol
        }
    )
    foreach ($expected in $expectedProtocols) {
        $matches = @($payload.features | Where-Object { [string]$_.feature -ceq $expected.Name })
        if ($matches.Count -ne 1) {
            throw "The OPC DA gateway smoke check expected exactly one '$($expected.Name)' protocol feature."
        }
        $minimum = [int]$matches[0].min_version
        $maximum = [int]$matches[0].max_version
        if ($minimum -lt 1 -or $maximum -lt $minimum -or
            $expected.Version -lt $minimum -or $expected.Version -gt $maximum) {
            throw "The running OPC DA gateway does not support required $($expected.Name) protocol version $($expected.Version)."
        }
    }

    return $payload
}
