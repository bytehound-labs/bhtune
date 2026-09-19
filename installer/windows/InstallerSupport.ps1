# BHTune Windows installer support functions.
#
# This file intentionally contains no top-level installation work.  It is
# dot-sourced by Install-Bhtune.ps1 and by the cross-platform self-tests.
# Keep the syntax compatible with Windows PowerShell 5.1.

$script:InstallerSchemaVersion = 2
$script:InstallerProductName = 'BHTune'
$script:InstallerPublisher = 'ByteHound Corp.'
$script:InstallerServiceName = 'BhtuneServer'
$script:InstallerServiceDisplayName = 'BHTune Server'
$script:InstallerServiceAccount = 'NT AUTHORITY\LocalService'
$script:InstallerBindAddress = '127.0.0.1:8787'
$script:InstallerStartUri = 'http://127.0.0.1:8787'
$script:InstallerHealthUri = 'http://127.0.0.1:8787/api/health'
$script:InstallerMarkerPath = 'HKLM:\Software\ByteHound\bhtune'
$script:InstallerUninstallPath = 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\BHTune'
$script:RequiredPayloadFiles = @('bhtune.exe', 'bhtune-server.exe', 'LICENSE', 'README.md')
if (-not (Get-Variable -Name InstallerTracePath -Scope Script -ErrorAction SilentlyContinue)) {
    $script:InstallerTracePath = ''
}

function Write-InstallerTrace {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Stage,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Detail = ''
    )

    if ([string]::IsNullOrWhiteSpace($script:InstallerTracePath)) {
        return
    }

    try {
        $parent = Split-Path -Parent $script:InstallerTracePath
        if (-not [string]::IsNullOrWhiteSpace($parent)) {
            New-Item -ItemType Directory -Path $parent -Force -ErrorAction Stop | Out-Null
        }
        $record = [ordered]@{
            TimestampUtc = [DateTime]::UtcNow.ToString('o')
            ProcessId    = $PID
            Stage        = $Stage
            Detail       = $Detail
        }
        $encoding = New-Object System.Text.UTF8Encoding($false)
        [System.IO.File]::AppendAllText(
            $script:InstallerTracePath,
            (($record | ConvertTo-Json -Compress) + [Environment]::NewLine),
            $encoding
        )
    } catch {
        # Diagnostics must never change installer behavior.
    }
}

function Get-RequiredPayloadFiles {
    return @($script:RequiredPayloadFiles)
}

function Test-StableVersion {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Version
    )

    if ([string]::IsNullOrWhiteSpace($Version)) {
        return $false
    }

    return $Version.Trim() -cmatch '^(?:v)?[0-9]+\.[0-9]+\.[0-9]+$'
}

function ConvertTo-NormalizedVersion {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Version
    )

    $candidate = $Version.Trim()
    if ($candidate.StartsWith('v', [System.StringComparison]::Ordinal)) {
        $candidate = $candidate.Substring(1)
    }

    if (-not (Test-StableVersion -Version $candidate)) {
        throw "Expected a stable semantic version in the form X.Y.Z, received '$Version'."
    }

    return $candidate
}

function Get-VersionFromStableTag {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Tag
    )

    if (-not ($Tag.Trim() -cmatch '^v([0-9]+\.[0-9]+\.[0-9]+)$')) {
        throw "Release tag '$Tag' is not a stable vX.Y.Z tag."
    }

    return $Matches[1]
}

function Assert-InstallerVersionContract {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ReleaseTag
    )

    $normalizedVersion = ConvertTo-NormalizedVersion -Version $ExpectedVersion
    $normalizedTag = $null
    if (-not [string]::IsNullOrWhiteSpace($ReleaseTag)) {
        $normalizedTag = Get-VersionFromStableTag -Tag $ReleaseTag
        if ($normalizedTag -ne $normalizedVersion) {
            throw "Release tag '$ReleaseTag' does not match expected version '$normalizedVersion'."
        }
    }

    return [pscustomobject]@{
        Version   = $normalizedVersion
        ReleaseTag = $ReleaseTag
        IsStable  = $true
    }
}

function Assert-FailureInjectionPolicy {
    param(
        [Parameter(Mandatory = $false)]
        [ValidateSet('None', 'HealthMismatch', 'CommitFailure')]
        [string]$FailureInjection = 'None',

        [Parameter(Mandatory = $false)]
        [bool]$TestOnly = $false
    )

    if ($FailureInjection -ne 'None' -and -not $TestOnly) {
        throw "Failure-injection '$FailureInjection' is test-only and is rejected in normal installer mode."
    }

    if ($TestOnly -and $FailureInjection -eq 'None') {
        throw 'Test-only installer mode requires an explicit failure-injection value.'
    }

    return $true
}

function ConvertTo-InstallerBoolean {
    param(
        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [object]$Value,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if ($Value -is [bool]) {
        return [bool]$Value
    }

    $text = if ($null -eq $Value) { '' } else { ([string]$Value).Trim().ToLowerInvariant() }
    switch ($text) {
        '1' { return $true }
        'true' { return $true }
        'yes' { return $true }
        'on' { return $true }
        '0' { return $false }
        'false' { return $false }
        'no' { return $false }
        'off' { return $false }
        default { throw "The installer boolean '$Name' must be 0 or 1." }
    }
}

function Get-InstallerPaths {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$InstallRoot,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ProgramDataRoot
    )

    if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
        $programFiles = $env:ProgramW6432
        if ([string]::IsNullOrWhiteSpace($programFiles)) {
            $programFiles = $env:ProgramFiles
        }
        if ([string]::IsNullOrWhiteSpace($programFiles)) {
            throw 'The Program Files directory is not available.'
        }
        $InstallRoot = Join-Path $programFiles 'ByteHound\bhtune'
    }

    if ([string]::IsNullOrWhiteSpace($ProgramDataRoot)) {
        if ([string]::IsNullOrWhiteSpace($env:ProgramData)) {
            throw 'The ProgramData directory is not available.'
        }
        $ProgramDataRoot = Join-Path $env:ProgramData 'ByteHound\bhtune'
    }

    $commonPrograms = $null
    if (-not [string]::IsNullOrWhiteSpace($env:ProgramData)) {
        $commonPrograms = Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu\Programs'
    }

    return [pscustomobject]@{
        InstallRoot        = $InstallRoot
        ProgramDataRoot    = $ProgramDataRoot
        ConfigPath         = Join-Path $ProgramDataRoot 'bhtune.toml'
        DatabaseDirectory  = Join-Path $ProgramDataRoot 'data'
        DatabasePath       = Join-Path $ProgramDataRoot 'data\bhtune.db'
        LogDirectory       = Join-Path $ProgramDataRoot 'logs'
        InstallerStateRoot = Join-Path $ProgramDataRoot 'installer'
        RollbackRoot       = Join-Path $ProgramDataRoot 'installer\rollback'
        ServiceExecutable  = Join-Path $InstallRoot 'bhtune-server.exe'
        CliExecutable      = Join-Path $InstallRoot 'bhtune.exe'
        UninstallerPath    = Join-Path $InstallRoot 'uninstall.exe'
        InstallerScriptRoot = Join-Path $InstallRoot 'installer'
        ShortcutPath       = if ($null -eq $commonPrograms) {
            $null
        } else {
            Join-Path $commonPrograms 'BHTune\BHTune.url'
        }
        MarkerPath         = $script:InstallerMarkerPath
        UninstallKeyPath   = $script:InstallerUninstallPath
        ServiceName        = $script:InstallerServiceName
    }
}

function Get-ServiceEnvironmentOverrides {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ServiceName
    )

    $values = [ordered]@{}
    $serviceKey = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
    try {
        $key = Get-Item -LiteralPath $serviceKey -ErrorAction Stop
        $environment = $key.GetValue(
            'Environment',
            $null,
            [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames
        )
        foreach ($entry in @($environment)) {
            if ([string]$entry -match '^(?<name>[^=]+)=(?<value>.*)$') {
                $values[$Matches['name']] = $Matches['value']
            }
        }
    } catch {
        # A service-specific environment block is optional.  Missing keys or
        # values are represented by an empty dictionary; provider or access
        # failures remain fail-closed without matching localized text.
        $category = [string]$_.CategoryInfo.Category
        $fullyQualifiedId = [string]$_.FullyQualifiedErrorId
        $isMissing = $category -eq 'ObjectNotFound' -or
            $fullyQualifiedId -like '*PathNotFound*' -or
            $fullyQualifiedId -like '*PropertyNotFound*'
        if (-not $isMissing) {
            throw "Unable to inspect the environment block for service '$ServiceName': $($_.Exception.Message)"
        }
    }
    return $values
}

function Get-InstallerEnvironmentPolicy {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$ProcessDatabaseOverride,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$ProcessBindOverride,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$MachineDatabaseOverride,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$MachineBindOverride,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [System.Collections.IDictionary]$ServiceOverrides
    )

    if ($PSBoundParameters.ContainsKey('ProcessDatabaseOverride') -eq $false) {
        $ProcessDatabaseOverride = [Environment]::GetEnvironmentVariable('BHTUNE_DB', 'Process')
    }
    if ($PSBoundParameters.ContainsKey('ProcessBindOverride') -eq $false) {
        $ProcessBindOverride = [Environment]::GetEnvironmentVariable('BHTUNE_BIND', 'Process')
    }
    if ($PSBoundParameters.ContainsKey('MachineDatabaseOverride') -eq $false) {
        $MachineDatabaseOverride = [Environment]::GetEnvironmentVariable('BHTUNE_DB', 'Machine')
    }
    if ($PSBoundParameters.ContainsKey('MachineBindOverride') -eq $false) {
        $MachineBindOverride = [Environment]::GetEnvironmentVariable('BHTUNE_BIND', 'Machine')
    }
    if ($PSBoundParameters.ContainsKey('ServiceOverrides') -eq $false -or $null -eq $ServiceOverrides) {
        $ServiceOverrides = Get-ServiceEnvironmentOverrides -ServiceName $Paths.ServiceName
    }

    $serviceDatabase = if ($ServiceOverrides.Contains('BHTUNE_DB')) { [string]$ServiceOverrides['BHTUNE_DB'] } else { $null }
    $serviceBind = if ($ServiceOverrides.Contains('BHTUNE_BIND')) { [string]$ServiceOverrides['BHTUNE_BIND'] } else { $null }
    $databaseSources = [ordered]@{
        Process = $ProcessDatabaseOverride
        Machine  = $MachineDatabaseOverride
        Service  = $serviceDatabase
    }
    $bindSources = [ordered]@{
        Process = $ProcessBindOverride
        Machine  = $MachineBindOverride
        Service  = $serviceBind
    }
    $expectedDatabase = Get-ComparableAbsolutePath -Path $Paths.DatabasePath
    $databaseConflicts = New-Object System.Collections.ArrayList
    foreach ($source in $databaseSources.GetEnumerator()) {
        if (-not [string]::IsNullOrWhiteSpace([string]$source.Value)) {
            $comparable = Get-ComparableAbsolutePath -Path ([string]$source.Value)
            if ($null -eq $comparable -or $comparable -ne $expectedDatabase) {
                [void]$databaseConflicts.Add("$($source.Key)=$($source.Value)")
            }
        }
    }
    $bindConflicts = New-Object System.Collections.ArrayList
    foreach ($source in $bindSources.GetEnumerator()) {
        if (-not [string]::IsNullOrWhiteSpace([string]$source.Value) -and
            ([string]$source.Value).Trim() -ne $script:InstallerBindAddress) {
            [void]$bindConflicts.Add("$($source.Key)=$($source.Value)")
        }
    }

    return [pscustomobject]@{
        DatabaseSources  = $databaseSources
        BindSources      = $bindSources
        DatabaseConflicts = @($databaseConflicts)
        BindConflicts    = @($bindConflicts)
        DatabaseSafe     = $databaseConflicts.Count -eq 0
        BindSafe         = $bindConflicts.Count -eq 0
    }
}

function Assert-InstallerEnvironmentPolicy {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $policy = Get-InstallerEnvironmentPolicy -Paths $Paths
    if (-not $policy.DatabaseSafe) {
        throw "BHTUNE_DB has an effective override outside the installer-managed database. Refusing to mutate BHTune until the conflicting override is removed or pinned to '$($Paths.DatabasePath)': $($policy.DatabaseConflicts -join ', ')"
    }
    if (-not $policy.BindSafe) {
        throw "BHTUNE_BIND has an effective non-loopback override. Refusing to mutate BhtuneServer until the conflicting override is removed or pinned to '$($script:InstallerBindAddress)': $($policy.BindConflicts -join ', ')"
    }
    return $policy
}

function Get-PreservedProgramDataState {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $databaseArtifacts = @(
        @(
            $Paths.DatabasePath,
            $Paths.DatabasePath + '-wal',
            $Paths.DatabasePath + '-shm'
        ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
    )
    $configExists = Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf
    $rollbackExists = Test-Path -LiteralPath $Paths.RollbackRoot
    $logsExist = (Test-Path -LiteralPath $Paths.LogDirectory -PathType Container) -and
        @(
            Get-ChildItem -LiteralPath $Paths.LogDirectory -Force -ErrorAction SilentlyContinue
        ).Count -gt 0
    $installerStateExists = (Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container) -and
        @(
            Get-ChildItem -LiteralPath $Paths.InstallerStateRoot -Force -ErrorAction SilentlyContinue
        ).Count -gt 0

    return [pscustomobject]@{
        ConfigExists       = $configExists
        DatabaseArtifacts  = @($databaseArtifacts)
        RollbackExists     = $rollbackExists
        LogsExist          = $logsExist
        InstallerStateExists = $installerStateExists
        ReuseRequired      = $configExists -or $databaseArtifacts.Count -gt 0 -or
            $rollbackExists -or $logsExist -or $installerStateExists
    }
}

function Test-PreservedProgramData {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    return [bool](Get-PreservedProgramDataState -Paths $Paths).ReuseRequired
}

function Get-InstallerAclTargets {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    # ProgramData is deliberately not traversed. Its database and log
    # directories are installer-managed roots, but their descendants may
    # contain operator-created files. The top-level BHTune root is secured
    # explicitly so inherited broad user-group write access is not retained.
    # The install root is rebuilt from the verified payload, so only its known
    # installer paths are touched.
    return @(
        $Paths.ProgramDataRoot,
        $Paths.InstallRoot,
        (Join-Path $Paths.InstallRoot 'installer'),
        (Join-Path $Paths.InstallRoot 'bhtune.exe'),
        (Join-Path $Paths.InstallRoot 'bhtune-server.exe'),
        (Join-Path $Paths.InstallRoot 'LICENSE'),
        (Join-Path $Paths.InstallRoot 'README.md'),
        (Join-Path $Paths.InstallRoot 'installer\InstallerSupport.ps1'),
        (Join-Path $Paths.InstallRoot 'installer\Install-Bhtune.ps1'),
        $Paths.DatabaseDirectory,
        $Paths.LogDirectory,
        $Paths.InstallerStateRoot,
        $Paths.ConfigPath
    )
}

function Assert-InstallerFixedPaths {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $expected = Get-InstallerPaths
    if ((Normalize-PathForComparison -Path $Paths.InstallRoot) -ne (Normalize-PathForComparison -Path $expected.InstallRoot) -or
        (Normalize-PathForComparison -Path $Paths.ProgramDataRoot) -ne (Normalize-PathForComparison -Path $expected.ProgramDataRoot)) {
        throw "The installer only supports the fixed Program Files and ProgramData locations ('$($expected.InstallRoot)' and '$($expected.ProgramDataRoot)')."
    }

    Assert-NoReparsePointInPath -Path $expected.InstallRoot -Name 'the fixed Program Files root'
    Assert-NoReparsePointInPath -Path $expected.ProgramDataRoot -Name 'the fixed ProgramData root'
    return $true
}

function Normalize-PathForComparison {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Path
    )

    if ($null -eq $Path) {
        return ''
    }

    $normalized = $Path.Trim().Trim('"').Replace('/', '\')
    if ($normalized.Length -gt 3) {
        $normalized = $normalized.TrimEnd('\')
    }

    return $normalized.ToLowerInvariant()
}

function Test-AbsolutePathValue {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $candidate = $Path.Trim()
    if ($candidate -match '^[A-Za-z]:[\\/]') {
        return $true
    }
    if ($candidate -match '^\\\\[^\\]+\\[^\\]+') {
        return $true
    }

    # The helper tests also run on Unix hosts. Preserve normal POSIX absolute
    # path semantics there while keeping Windows root-relative paths rejected.
    if ($env:OS -ne 'Windows_NT') {
        return [System.IO.Path]::IsPathRooted($candidate)
    }

    # A root-relative path such as "\data\bhtune.db" is rooted according to
    # .NET, but it is still drive-dependent and therefore unsafe for a service
    # account or a rollback policy. Accept only Windows drive-absolute and UNC
    # paths.
    return $false
}

function Get-ComparableAbsolutePath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-AbsolutePathValue -Path $Path)) {
        return $null
    }

    $candidate = $Path.Trim().Trim('"')
    try {
        if (($candidate -match '^[A-Za-z]:[\\/]') -or
            ($candidate -match '^\\\\[^\\]+\\[^\\]+')) {
            if ($env:OS -eq 'Windows_NT') {
                return (Normalize-PathForComparison -Path ([System.IO.Path]::GetFullPath($candidate)))
            }
            # The cross-platform self-tests exercise Windows path syntax on
            # Unix hosts.  Preserve that syntax there while Windows callers
            # receive .NET's canonical absolute-path normalization above.
            return (Normalize-PathForComparison -Path $candidate)
        }
        if ($env:OS -ne 'Windows_NT') {
            return [System.IO.Path]::GetFullPath($candidate)
        }
        return (Normalize-PathForComparison -Path ([System.IO.Path]::GetFullPath($candidate)))
    } catch {
        return $null
    }
}

function Assert-NoReparsePointInPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $false)]
        [string]$Name = 'the managed path'
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        throw "$Name is empty. Refusing to continue."
    }

    try {
        $current = [System.IO.Path]::GetFullPath($Path)
    } catch {
        throw "$Name '$Path' is not a valid absolute path: $($_.Exception.Message)"
    }

    while (-not [string]::IsNullOrWhiteSpace($current)) {
        if (Test-Path -LiteralPath $current) {
            try {
                $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
            } catch {
                throw "Unable to inspect $Name '$current' for reparse-point redirection: $($_.Exception.Message)"
            }
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "$Name '$current' is a reparse point. Refusing to follow or mutate redirected content."
            }
        }

        $parent = [System.IO.Directory]::GetParent($current)
        if ($null -eq $parent -or $parent.FullName -eq $current) {
            break
        }
        $current = $parent.FullName
    }
}

function Test-SafeRollbackRelativePath {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$RelativePath
    )

    if ([string]::IsNullOrWhiteSpace($RelativePath)) {
        return $false
    }

    $normalized = $RelativePath.Trim().Replace('/', '\')
    if ($normalized.StartsWith('\') -or
        $normalized -match '^[A-Za-z]:' -or
        $normalized -match ':') {
        return $false
    }

    $segments = $normalized -split '\\'
    if (@($segments | Where-Object {
                [string]::IsNullOrWhiteSpace($_) -or $_ -eq '.' -or $_ -eq '..'
            }).Count -gt 0) {
        return $false
    }

    return $true
}

function Split-PathList {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$PathList
    )

    if ($null -eq $PathList -or $PathList.Length -eq 0) {
        return @()
    }

    return @($PathList -split ';')
}

function Add-ExactPathEntry {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ExistingPath,

        [Parameter(Mandatory = $true)]
        [string]$Entry
    )

    $entries = @(Split-PathList -PathList $ExistingPath)
    $normalizedEntry = Normalize-PathForComparison -Path $Entry
    foreach ($current in $entries) {
        if ((Normalize-PathForComparison -Path $current) -eq $normalizedEntry) {
            return [pscustomobject]@{
                Value   = if ($null -eq $ExistingPath) { '' } else { $ExistingPath }
                Changed = $false
            }
        }
    }

    if ([string]::IsNullOrEmpty($ExistingPath)) {
        $value = $Entry
    } else {
        # Preserve unrelated empty segments and their original formatting.
        # They can be meaningful PATH entries (for example, the current
        # directory), so adding one installer entry must not rewrite them.
        $value = $ExistingPath + ';' + $Entry
    }

    return [pscustomobject]@{
        Value   = $value
        Changed = $true
    }
}

function Remove-ExactPathEntry {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ExistingPath,

        [Parameter(Mandatory = $true)]
        [string]$Entry
    )

    if ($null -eq $ExistingPath) {
        return [pscustomobject]@{
            Value   = ''
            Changed = $false
        }
    }

    $normalizedEntry = Normalize-PathForComparison -Path $Entry
    $entries = @(Split-PathList -PathList $ExistingPath)
    $removeIndex = -1
    for ($index = $entries.Count - 1; $index -ge 0; $index--) {
        if ((Normalize-PathForComparison -Path $entries[$index]) -eq $normalizedEntry) {
            # The installer appends its entry.  Remove only one, the last
            # normalized match, so an administrator-owned equivalent entry
            # that predates installation remains untouched.
            $removeIndex = $index
            break
        }
    }

    return [pscustomobject]@{
        Value   = if ($removeIndex -ge 0) {
            $kept = New-Object System.Collections.ArrayList
            for ($index = 0; $index -lt $entries.Count; $index++) {
                if ($index -ne $removeIndex) {
                    [void]$kept.Add($entries[$index])
                }
            }
            ($kept -join ';')
        } else {
            $ExistingPath
        }
        Changed = $removeIndex -ge 0
    }
}

function Set-MachinePathExact {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('Add', 'Remove')]
        [string]$Operation,

        [Parameter(Mandatory = $true)]
        [string]$Entry
    )

    $existing = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if ($Operation -eq 'Add') {
        $result = Add-ExactPathEntry -ExistingPath $existing -Entry $Entry
    } else {
        $result = Remove-ExactPathEntry -ExistingPath $existing -Entry $Entry
    }

    if ($result.Changed) {
        [Environment]::SetEnvironmentVariable('Path', $result.Value, 'Machine')
    }

    return $result
}

function Set-MachinePathEntryState {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Entry,

        [Parameter(Mandatory = $true)]
        [bool]$WasPresent
    )

    $current = Get-MachinePathSnapshot
    $entries = @(Split-PathList -PathList $current)
    $matches = @($entries | Where-Object {
            (Normalize-PathForComparison -Path $_) -eq (Normalize-PathForComparison -Path $Entry)
        })

    if ($WasPresent) {
        if ($matches.Count -eq 0) {
            return Set-MachinePathExact -Operation Add -Entry $Entry
        }
        return [pscustomobject]@{
            Value   = $current
            Changed = $false
        }
    }

    if ($matches.Count -gt 1) {
        throw "Machine PATH contains multiple exact entries for '$Entry'. Refusing to remove an ambiguous installer-owned entry."
    }
    if ($matches.Count -eq 1) {
        return Set-MachinePathExact -Operation Remove -Entry $Entry
    }
    return [pscustomobject]@{
        Value   = $current
        Changed = $false
    }
}

function Get-MachinePathSnapshot {
    return [Environment]::GetEnvironmentVariable('Path', 'Machine')
}

function Set-MachinePathSnapshot {
    param(
        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$Value
    )

    [Environment]::SetEnvironmentVariable('Path', $Value, 'Machine')
}

function Get-BhtuneProcessSnapshot {
    $names = @('bhtune', 'bhtune-server')
    $processes = @(Get-Process -ErrorAction Stop | Where-Object {
            $names -contains $_.ProcessName
        })
    return @($processes | ForEach-Object {
            [pscustomobject]@{
                Id          = [int]$_.Id
                ProcessName = [string]$_.ProcessName
                Path        = $null
            }
        })
}

function Assert-BhtuneDatabaseQuiescent {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DatabasePath
    )

    $processes = @(Get-BhtuneProcessSnapshot)
    if ($processes.Count -gt 0) {
        $details = ($processes | ForEach-Object {
                '{0} (PID {1})' -f $_.ProcessName, $_.Id
            }) -join ', '
        throw "BHTune processes still exist while the managed database is being backed up: $details. Stop those processes and retry."
    }

    return $true
}

function Get-FileStabilitySnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        return [pscustomobject]@{
            Exists       = $false
            Length       = $null
            LastWriteUtc = $null
        }
    }

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    return [pscustomobject]@{
        Exists       = $true
        Length       = [int64]$item.Length
        LastWriteUtc = ([DateTime]$item.LastWriteTimeUtc).Ticks
    }
}

function Assert-FileStabilitySnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [psobject]$Before,

        [Parameter(Mandatory = $true)]
        [psobject]$After
    )

    $beforeJson = $Before | ConvertTo-Json -Compress
    $afterJson = $After | ConvertTo-Json -Compress
    if ($beforeJson -ne $afterJson) {
        throw "The managed database companion '$Path' changed while it was being backed up. Refusing to keep an inconsistent snapshot."
    }
}

function ConvertFrom-TomlQuotedString {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Token
    )

    if ($Token.Length -lt 2) {
        throw "Invalid TOML string token '$Token'."
    }

    $quote = $Token.Substring(0, 1)
    $value = $Token.Substring(1, $Token.Length - 2)
    if ($quote -eq "'") {
        return $value
    }

    return $value.Replace('\r', "`r").Replace('\n', "`n").Replace('\t', "`t").Replace('\"', '"').Replace('\\', '\')
}

function Get-TomlTopLevelStringValue {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Content,

        [Parameter(Mandatory = $true)]
        [string]$Key
    )

    $found = $false
    $duplicate = $false
    $valid = $true
    $value = $null
    $lineNumber = 0

    foreach ($line in ($Content -split "`r?`n")) {
        $lineNumber++
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('#') -or $trimmed.Length -eq 0) {
            continue
        }

        # Once a table begins, subsequent keys are not top-level keys.
        if ($trimmed.StartsWith('[')) {
            break
        }

        if ($trimmed -match '^\s*([A-Za-z0-9_-]+)\s*=') {
            $lineKey = $Matches[1]
            if ($lineKey -ne $Key) {
                continue
            }

            if ($found) {
                $duplicate = $true
                continue
            }

            $found = $true
            if ($trimmed -notmatch '^\s*[A-Za-z0-9_-]+\s*=\s*("(?:\\.|[^"])*"|''[^'']*'')\s*(?:#.*)?$') {
                $valid = $false
                continue
            }

            try {
                $value = ConvertFrom-TomlQuotedString -Token $Matches[1]
            } catch {
                $valid = $false
            }
        }
    }

    return [pscustomobject]@{
        Key        = $Key
        Found      = $found
        Duplicate  = $duplicate
        Valid      = $valid
        Value      = $value
        LineNumber = $lineNumber
    }
}

function Get-DatabasePolicy {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$DefaultDatabasePath
    )

    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        return [pscustomobject]@{
            Policy          = 'Missing'
            DatabasePath    = $null
            Reason          = 'The installer-managed configuration file does not exist.'
            AutomaticBackup = $false
        }
    }

    try {
        $content = [System.IO.File]::ReadAllText($ConfigPath)
    } catch {
        return [pscustomobject]@{
            Policy          = 'Ambiguous'
            DatabasePath    = $null
            Reason          = "The configuration file could not be read: $($_.Exception.Message)"
            AutomaticBackup = $false
        }
    }

    $entry = Get-TomlTopLevelStringValue -Content $content -Key 'db'
    if (-not $entry.Found) {
        return [pscustomobject]@{
            Policy          = 'Ambiguous'
            DatabasePath    = $null
            Reason          = 'The top-level db setting is absent; the service account would resolve a profile-dependent default.'
            AutomaticBackup = $false
        }
    }
    if ($entry.Duplicate -or -not $entry.Valid -or [string]::IsNullOrWhiteSpace($entry.Value)) {
        return [pscustomobject]@{
            Policy          = 'Ambiguous'
            DatabasePath    = $entry.Value
            Reason          = 'The top-level db setting is duplicated, malformed, or empty.'
            AutomaticBackup = $false
        }
    }

    $absolute = Get-ComparableAbsolutePath -Path $entry.Value
    if ($null -eq $absolute) {
        return [pscustomobject]@{
            Policy          = 'Ambiguous'
            DatabasePath    = $entry.Value
            Reason          = 'The top-level db setting is not an absolute path.'
            AutomaticBackup = $false
        }
    }

    $defaultComparable = Get-ComparableAbsolutePath -Path $DefaultDatabasePath
    if ($absolute -eq $defaultComparable) {
        return [pscustomobject]@{
            Policy          = 'Default'
            DatabasePath    = $absolute
            Reason          = 'The database is the installer-managed ProgramData database.'
            AutomaticBackup = $true
        }
    }

    return [pscustomobject]@{
        Policy          = 'External'
        DatabasePath    = $absolute
        Reason          = 'The database is outside the installer-managed ProgramData database.'
        AutomaticBackup = $false
    }
}

function Assert-ExternalDatabaseReady {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DatabasePath
    )

    $absolute = Get-ComparableAbsolutePath -Path $DatabasePath
    if ($null -eq $absolute) {
        throw "The confirmed external database '$DatabasePath' is not an absolute Windows path. Prepare and independently back up an existing database file before rerunning the installer."
    }

    try {
        $database = Get-Item -LiteralPath $DatabasePath -Force -ErrorAction Stop
    } catch {
        throw "The confirmed external database '$DatabasePath' does not exist or cannot be accessed. Prepare and independently back up an existing database file before rerunning the installer."
    }

    if ($database.PSIsContainer) {
        throw "The confirmed external database path '$DatabasePath' is a directory. Prepare and independently back up an existing SQLite database file before rerunning the installer."
    }
    if (($database.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "The confirmed external database '$DatabasePath' is a reparse point. Resolve the real database path and independently back it up before rerunning the installer."
    }

    try {
        $stream = [System.IO.File]::Open(
            $database.FullName,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite
        )
        $stream.Dispose()
    } catch {
        throw "The confirmed external database '$DatabasePath' cannot be opened for reading: $($_.Exception.Message)"
    }
}

function ConvertTo-TomlPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    return $Path.Replace('\', '/')
}

function Get-DefaultConfigContent {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DatabasePath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    $db = ConvertTo-TomlPath -Path $DatabasePath
    $logs = ConvertTo-TomlPath -Path $LogDirectory
    return @"
# BHTune installer defaults. Operator edits are preserved across upgrades.
bind = "$($script:InstallerBindAddress)"
db = "$db"

[log]
dir = "$logs"
"@
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
    $normalizeCommandLine = {
        param([string]$Value)
        $withoutQuotes = $Value.Trim().Replace('/', '\').Replace('"', '')
        return [regex]::Replace($withoutQuotes, '\s+', ' ').ToLowerInvariant()
    }
    $actual = & $normalizeCommandLine $ActualPathName
    $expected = & $normalizeCommandLine (Get-ExpectedServiceCommandLine -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath)
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

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(Mandatory = $false)]
        [string]$Arguments = '--version',

        [Parameter(Mandatory = $false)]
        [int]$TimeoutMilliseconds = 15000
    )

    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = $FilePath
    $info.Arguments = $Arguments
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) {
            throw "The process could not be started: $FilePath"
        }
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            try {
                $process.Kill()
            } catch {
            }
            throw "The process did not exit within $TimeoutMilliseconds ms: $FilePath"
        }

        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            StdOut   = $process.StandardOutput.ReadToEnd()
            StdErr   = $process.StandardError.ReadToEnd()
        }
    } finally {
        $process.Dispose()
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

    if ($lines[0] -match '^(?:bhtune(?:-server)?\s+)?v?([0-9]+\.[0-9]+\.[0-9]+)$') {
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
        if ([int]$state.SchemaVersion -ne $script:InstallerSchemaVersion) {
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
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
        if ([int]$manifest.SchemaVersion -ne $script:InstallerSchemaVersion) {
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

function Get-RegistrySnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }

    $properties = Get-ItemProperty -LiteralPath $Path
    $values = [ordered]@{}
    foreach ($property in $properties.PSObject.Properties) {
        if ($property.Name -notlike 'PS*') {
            $values[$property.Name] = $property.Value
        }
    }

    return [pscustomobject]@{
        Path   = $Path
        Values = $values
    }
}

function Set-RegistryValues {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [psobject]$Values
    )

    New-Item -Path $Path -Force | Out-Null
    foreach ($property in $Values.PSObject.Properties) {
        $value = $property.Value
        if ($value -is [int] -or $value -is [uint32] -or $value -is [long]) {
            New-ItemProperty -LiteralPath $Path -Name $property.Name -Value ([int]$value) -PropertyType DWord -Force | Out-Null
        } else {
            New-ItemProperty -LiteralPath $Path -Name $property.Name -Value ([string]$value) -PropertyType String -Force | Out-Null
        }
    }
}

function Remove-RegistryKey {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
}

function Get-ServiceSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$RetryCount = 3,

        [Parameter(Mandatory = $false)]
        [int]$RetryDelayMilliseconds = 250
    )

    $lastError = $null
    for ($attempt = 0; $attempt -lt $RetryCount; $attempt++) {
        try {
            $service = Get-WmiObject -Class Win32_Service -Filter ("Name='{0}'" -f $Name) -ErrorAction Stop
            if ($null -eq $service) {
                return $null
            }

            return [pscustomobject]@{
                Exists      = $true
                Name        = [string]$service.Name
                State       = [string]$service.State
                StartMode   = [string]$service.StartMode
                StartName   = [string]$service.StartName
                PathName    = [string]$service.PathName
                DisplayName = [string]$service.DisplayName
                Description = [string]$service.Description
            }
        } catch {
            $lastError = $_.Exception
            if ($attempt -lt ($RetryCount - 1)) {
                Write-InstallerTrace `
                    -Stage 'service.snapshot.retry' `
                    -Detail ("name={0}; attempt={1}; error={2}" -f $Name, ($attempt + 1), $lastError.Message)
                Start-Sleep -Milliseconds $RetryDelayMilliseconds
            }
        }
    }

    throw "Unable to inspect service '$Name' after $RetryCount attempts: $($lastError.Message)"
}

function Test-LocalServiceAccount {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Account
    )

    if ([string]::IsNullOrWhiteSpace($Account)) {
        return $false
    }

    $value = $Account.Trim().ToLowerInvariant()
    return $value -eq 'localservice' -or $value -eq 'nt authority\localservice'
}

function Test-OwnedServiceSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$ServiceSnapshot,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath
    )

    if ($null -eq $ServiceSnapshot -or -not $ServiceSnapshot.Exists) {
        return $false
    }
    if (-not (Test-LocalServiceAccount -Account $ServiceSnapshot.StartName)) {
        return $false
    }
    if ([string]$ServiceSnapshot.StartMode -ne 'Auto') {
        return $false
    }
    if ([string]$ServiceSnapshot.DisplayName -ne $script:InstallerServiceDisplayName) {
        return $false
    }
    return Test-ServiceCommandLine -ActualPathName $ServiceSnapshot.PathName -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath
}

function Invoke-ScCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $scPath = Join-Path $env:SystemRoot 'System32\sc.exe'
    if (-not (Test-Path -LiteralPath $scPath -PathType Leaf)) {
        throw 'The Windows Service Control Manager command (sc.exe) is unavailable.'
    }

    $output = (& $scPath @Arguments 2>&1 | Out-String)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "sc.exe $($Arguments -join ' ') failed with exit code $exitCode. $output"
    }

    return $output
}

function Invoke-ServiceRegistrationQuery {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $scPath = Join-Path $env:SystemRoot 'System32\sc.exe'
    if (-not (Test-Path -LiteralPath $scPath -PathType Leaf)) {
        throw 'The Windows Service Control Manager command (sc.exe) is unavailable.'
    }

    $output = (& $scPath query $Name 2>&1 | Out-String)
    $exitCode = $LASTEXITCODE
    if ($exitCode -eq 0) {
        return $true
    }
    if ($exitCode -eq 1060) {
        return $false
    }

    throw "sc.exe query $Name failed with exit code $exitCode. $output"
}

function Wait-ServiceRegistrationGone {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30,

        [Parameter(Mandatory = $false)]
        [int]$PollMilliseconds = 250
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $attempt = 0
    $lastError = 'the service is still registered'
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            if (-not (Invoke-ServiceRegistrationQuery -Name $Name)) {
                Write-InstallerTrace `
                    -Stage 'service.delete.disappeared' `
                    -Detail ("name={0}; attempts={1}" -f $Name, ($attempt + 1))
                return
            }
            $lastError = 'the service is still registered'
        } catch {
            $lastError = $_.Exception.Message
        }

        $attempt++
        if ($attempt -eq 1 -or $attempt % 10 -eq 0) {
            Write-InstallerTrace `
                -Stage 'service.delete.poll' `
                -Detail ("name={0}; attempt={1}; error={2}" -f $Name, $attempt, $lastError)
        }
        Start-Sleep -Milliseconds $PollMilliseconds
    }

    throw "The service '$Name' did not disappear after $TimeoutSeconds seconds: $lastError"
}

function New-LocalServiceCredential {
    $emptyPassword = New-Object System.Security.SecureString
    return New-Object System.Management.Automation.PSCredential(
        $script:InstallerServiceAccount,
        $emptyPassword
    )
}

function New-LocalService {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [string]$BinaryPathName,

        [Parameter(Mandatory = $true)]
        [string]$DisplayName,

        [Parameter(Mandatory = $true)]
        [ValidateSet('Automatic', 'Manual', 'Disabled')]
        [string]$StartupType,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Description
    )

    $parameters = @{
        Name           = $Name
        BinaryPathName = $BinaryPathName
        DisplayName    = $DisplayName
        StartupType    = $StartupType
        Credential     = New-LocalServiceCredential
        ErrorAction    = 'Stop'
    }
    if (-not [string]::IsNullOrWhiteSpace($Description)) {
        $parameters.Description = $Description
    }

    New-Service @parameters | Out-Null
}

function New-InstallerService {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath
    )

    if ($null -ne (Get-ServiceSnapshot -Name $script:InstallerServiceName)) {
        throw "The service '$script:InstallerServiceName' already exists."
    }

    $commandLine = Get-ExpectedServiceCommandLine -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath
    $createdByTransaction = $false
    try {
        New-LocalService `
            -Name $script:InstallerServiceName `
            -BinaryPathName $commandLine `
            -DisplayName $script:InstallerServiceDisplayName `
            -StartupType Automatic `
            -Description 'BHTune HTTP API and embedded web GUI.'
        $createdByTransaction = $true

        $created = Get-ServiceSnapshot -Name $script:InstallerServiceName
        if (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $created -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath)) {
            throw 'The newly registered service did not match the installer-owned LocalService definition.'
        }
    } catch {
        if ($createdByTransaction) {
            try {
                $partial = Get-ServiceSnapshot -Name $script:InstallerServiceName
                if ($null -ne $partial -and
                    (Test-ServiceCommandLine -ActualPathName $partial.PathName -ExecutablePath $ExecutablePath -ConfigPath $ConfigPath)) {
                    if ($partial.State -ne 'Stopped') {
                        Stop-InstallerService | Out-Null
                    }
                    Remove-ServiceByName -Name $script:InstallerServiceName
                }
            } catch {
                # Leave an unverified service in place rather than deleting a
                # service whose ownership cannot be established.
            }
        }
        throw
    }

    return $created
}

function Remove-ServiceByName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if (-not (Invoke-ServiceRegistrationQuery -Name $Name)) {
        return
    }
    $existing = Get-ServiceSnapshot -Name $Name
    if ($null -eq $existing) {
        throw "Service '$Name' remains registered but cannot be inspected. Refusing to remove it."
    }

    Write-InstallerTrace -Stage 'service.delete.begin' -Detail $Name
    Invoke-ScCommand -Arguments @('delete', $Name) | Out-Null
    Write-InstallerTrace -Stage 'service.delete.requested' -Detail $Name
    Wait-ServiceRegistrationGone -Name $Name
    Write-InstallerTrace -Stage 'service.delete.end' -Detail $Name
}

function Wait-ServiceState {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $true)]
        [ValidateSet('Running', 'Stopped')]
        [string]$State,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $snapshot = Get-ServiceSnapshot -Name $Name
        if ($null -eq $snapshot) {
            throw "The service '$Name' disappeared while waiting for state '$State'."
        }
        if ($snapshot.State -eq $State) {
            return $snapshot
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "The service '$Name' did not reach state '$State' within $TimeoutSeconds seconds."
}

function Start-InstallerService {
    param(
        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    Start-Service -Name $script:InstallerServiceName -ErrorAction Stop
    return Wait-ServiceState -Name $script:InstallerServiceName -State Running -TimeoutSeconds $TimeoutSeconds
}

function Stop-InstallerService {
    param(
        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $snapshot = Get-ServiceSnapshot -Name $script:InstallerServiceName
    if ($null -eq $snapshot -or $snapshot.State -eq 'Stopped') {
        return $snapshot
    }

    Stop-Service -Name $script:InstallerServiceName -Force -ErrorAction Stop
    return Wait-ServiceState -Name $script:InstallerServiceName -State Stopped -TimeoutSeconds $TimeoutSeconds
}

function Restore-ServiceSnapshot {
    param(
        [Parameter(Mandatory = $false)]
        [psobject]$Snapshot
    )

    if ($null -eq $Snapshot -or -not $Snapshot.Exists) {
        Remove-ServiceByName -Name $script:InstallerServiceName
        return
    }

    Remove-ServiceByName -Name $script:InstallerServiceName
    if (-not (Test-LocalServiceAccount -Account $Snapshot.StartName)) {
        throw "Cannot restore service '$script:InstallerServiceName' without a supported LocalService account."
    }

    $startMode = switch ($Snapshot.StartMode) {
        'Auto' { 'Automatic' }
        'Disabled' { 'Disabled' }
        default { 'Manual' }
    }
    New-LocalService `
        -Name $script:InstallerServiceName `
        -BinaryPathName $Snapshot.PathName `
        -DisplayName $Snapshot.DisplayName `
        -StartupType $startMode `
        -Description $Snapshot.Description

    if ($Snapshot.State -eq 'Running') {
        Start-InstallerService | Out-Null
    } else {
        Wait-ServiceState -Name $script:InstallerServiceName -State Stopped | Out-Null
    }
}

function Get-AclSddl {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }
    return (Get-Acl -LiteralPath $Path).Sddl
}

function Invoke-Icacls {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $icacls = Join-Path $env:SystemRoot 'System32\icacls.exe'
    if (-not (Test-Path -LiteralPath $icacls -PathType Leaf)) {
        throw 'The Windows ACL command (icacls.exe) is unavailable.'
    }

    $output = (& $icacls @Arguments 2>&1 | Out-String)
    $exitCode = $LASTEXITCODE
    if ($exitCode -ne 0) {
        throw "icacls.exe failed with exit code $exitCode. $output"
    }
}

function Set-InstallerAcls {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [bool]$InstallRootCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$DatabaseDirectoryCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$LogDirectoryCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$InstallerStateRootCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$ConfigCreated = $false
    )

    foreach ($directory in @($Paths.InstallRoot, $Paths.DatabaseDirectory, $Paths.LogDirectory, $Paths.InstallerStateRoot)) {
        if (-not (Test-Path -LiteralPath $directory -PathType Container)) {
            New-Item -ItemType Directory -Path $directory -Force | Out-Null
        }
    }

    # Do not recursively rewrite ProgramData.  Operators may place files
    # below the preserved database/log roots, and a recursive ACL operation
    # would make those unrelated descendants part of an irreversible
    # installer mutation.  The explicit targets below are installer-created
    # roots and payload files only.
    # Always secure the fixed BHTune root. This removes inherited broad
    # user-group write access without traversing or rewriting operator-owned
    # descendants.
    Set-InstallerAclPath -Path $Paths.ProgramDataRoot -LocalServiceRights 'RX' -Directory $true

    if ($InstallRootCreated) {
        Set-InstallerAclPath -Path $Paths.InstallRoot -LocalServiceRights 'RX' -Directory $true
    }
    if ($DatabaseDirectoryCreated) {
        Set-InstallerAclPath -Path $Paths.DatabaseDirectory -LocalServiceRights 'M' -Directory $true
    }
    if ($LogDirectoryCreated) {
        Set-InstallerAclPath -Path $Paths.LogDirectory -LocalServiceRights 'M' -Directory $true
    }
    if ($InstallerStateRootCreated) {
        Set-InstallerAclPath -Path $Paths.InstallerStateRoot -LocalServiceRights $null -Directory $true
    }

    foreach ($path in @(
            (Join-Path $Paths.InstallRoot 'bhtune.exe'),
            (Join-Path $Paths.InstallRoot 'bhtune-server.exe'),
            (Join-Path $Paths.InstallRoot 'LICENSE'),
            (Join-Path $Paths.InstallRoot 'README.md'),
            (Join-Path $Paths.InstallRoot 'installer\InstallerSupport.ps1'),
            (Join-Path $Paths.InstallRoot 'installer\Install-Bhtune.ps1')
        )) {
        if (Test-Path -LiteralPath $path) {
            Set-InstallerAclPath -Path $path -LocalServiceRights 'RX' -Directory (Test-Path -LiteralPath $path -PathType Container)
        }
    }

    if ($ConfigCreated -and (Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf)) {
        Set-InstallerAclPath -Path $Paths.ConfigPath -LocalServiceRights 'M' -Directory $false
    }
}

function Set-InstallerAclPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [string]$LocalServiceRights,

        [Parameter(Mandatory = $true)]
        [bool]$Directory
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }
    $systemRights = if ($Directory) { '*S-1-5-18:(OI)(CI)(F)' } else { '*S-1-5-18:(F)' }
    $administratorsRights = if ($Directory) { '*S-1-5-32-544:(OI)(CI)(F)' } else { '*S-1-5-32-544:(F)' }
    $arguments = @($Path, '/inheritance:r', '/grant:r', $systemRights, $administratorsRights)
    if (-not [string]::IsNullOrWhiteSpace($LocalServiceRights)) {
        $localServiceRights = if ($Directory) {
            "*S-1-5-19:(OI)(CI)($LocalServiceRights)"
        } else {
            "*S-1-5-19:($LocalServiceRights)"
        }
        $arguments += $localServiceRights
    }
    Invoke-Icacls -Arguments $arguments
}

function Restore-AclSddl {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Sddl
    )

    if ([string]::IsNullOrWhiteSpace($Sddl) -or -not (Test-Path -LiteralPath $Path)) {
        return
    }

    # Reuse the path's native security object so both directory and file SDDL
    # can be restored.  DirectorySecurity cannot be applied to a file on all
    # Windows PowerShell/.NET combinations.
    $security = Get-Acl -LiteralPath $Path
    $security.SetSecurityDescriptorSddlForm($Sddl)
    Set-Acl -LiteralPath $Path -AclObject $security
}

function New-StartMenuShortcut {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ShortcutPath
    )

    $directory = Split-Path -Parent $ShortcutPath
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    @(
        '[InternetShortcut]'
        "URL=$($script:InstallerStartUri)"
    ) | Set-Content -LiteralPath $ShortcutPath -Encoding ASCII
}

function Remove-StartMenuShortcut {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ShortcutPath
    )

    if ([string]::IsNullOrWhiteSpace($ShortcutPath)) {
        return
    }
    if (Test-Path -LiteralPath $ShortcutPath -PathType Leaf) {
        Remove-Item -LiteralPath $ShortcutPath -Force
    }
}

function Test-StartMenuShortcut {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ShortcutPath
    )

    if (-not (Test-Path -LiteralPath $ShortcutPath -PathType Leaf)) {
        return $false
    }

    $content = Get-Content -LiteralPath $ShortcutPath -Raw -ErrorAction Stop
    return $content -match '(?m)^\[InternetShortcut\]\s*$' -and
        $content -match ('(?m)^URL={0}\s*$' -f [regex]::Escape($script:InstallerStartUri))
}

function Invoke-HealthVersionCheck {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ExpectedVersion,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 45,

        [Parameter(Mandatory = $false)]
        [string]$Uri = $script:InstallerHealthUri
    )

    Write-InstallerTrace -Stage 'health.begin' -Detail ("uri={0}; expected={1}; timeout={2}" -f $Uri, $ExpectedVersion, $TimeoutSeconds)
    $expected = ConvertTo-NormalizedVersion -Version $ExpectedVersion
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = 'No response received.'

    do {
        try {
            $response = Invoke-WebRequest -Uri $Uri -UseBasicParsing -TimeoutSec 3 -ErrorAction Stop
            $body = $response.Content | ConvertFrom-Json
            if ($body.status -eq 'ok' -and [string]$body.version -eq $expected) {
                Write-InstallerTrace -Stage 'health.success' -Detail ("uri={0}; version={1}" -f $Uri, $body.version)
                return [pscustomobject]@{
                    Status  = [string]$body.status
                    Version = [string]$body.version
                    Uri     = $Uri
                }
            }
            $lastError = "Health response was not the expected status/version: $($response.Content)"
        } catch {
            $lastError = $_.Exception.Message
        }

        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)

    Write-InstallerTrace -Stage 'health.failure' -Detail ("uri={0}; error={1}" -f $Uri, $lastError)
    throw "BHTune health/version validation failed for '$Uri': $lastError"
}
