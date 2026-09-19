# BHTune Windows installer support functions.
#
# This file intentionally contains no top-level installation work.  It is
# dot-sourced by Install-Bhtune.ps1 and by the cross-platform self-tests.
# Keep the syntax compatible with Windows PowerShell 5.1.

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

function Get-GatewayRequiredPayloadFiles {
    return @($script:GatewayRequiredPayloadFiles)
}

function Test-SupportedInstallerSchemaVersion {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Version
    )

    return $script:SupportedInstallerSchemaVersions -contains $Version
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
        [ValidateSet('None', 'HealthMismatch', 'GatewaySmokeFailure', 'CommitFailure')]
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
        GatewayInstallRoot = Join-Path $InstallRoot 'gateway'
        GatewayExecutable  = Join-Path $InstallRoot 'gateway\opcda-bridge-gateway.exe'
        GatewayReleasePath = Join-Path $InstallRoot 'gateway\opcda-gateway-release.json'
        GatewayProvenancePath = Join-Path $InstallRoot 'gateway\opcda-gateway-provenance.json'
        GatewayLicensePath = Join-Path $InstallRoot 'gateway\LICENSE-opcda-bridge.txt'
        GatewayNoticePath  = Join-Path $InstallRoot 'gateway\NOTICE-opcda-bridge.txt'
        GatewayProgramDataRoot = Join-Path $ProgramDataRoot 'gateway'
        GatewayConfigPath  = Join-Path $ProgramDataRoot 'gateway\opcda-bridge-gateway.toml'
        GatewayDataDirectory = Join-Path $ProgramDataRoot 'gateway\data'
        GatewayDatabasePath = Join-Path $ProgramDataRoot 'gateway\data\index.sqlite3'
        GatewayLogDirectory = Join-Path $ProgramDataRoot 'gateway\logs'
        GatewayServiceName = $script:GatewayServiceName
        GatewayPort        = $script:GatewayPort
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
    $gatewayConfigExists = Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Leaf
    $gatewayDataExists = (Test-Path -LiteralPath $Paths.GatewayDataDirectory -PathType Container) -and
        @(
            Get-ChildItem -LiteralPath $Paths.GatewayDataDirectory -Force -ErrorAction SilentlyContinue
        ).Count -gt 0
    $gatewayLogsExist = (Test-Path -LiteralPath $Paths.GatewayLogDirectory -PathType Container) -and
        @(
            Get-ChildItem -LiteralPath $Paths.GatewayLogDirectory -Force -ErrorAction SilentlyContinue
        ).Count -gt 0
    $gatewayStateExists = Test-Path -LiteralPath $Paths.GatewayProgramDataRoot

    return [pscustomobject]@{
        ConfigExists       = $configExists
        DatabaseArtifacts  = @($databaseArtifacts)
        RollbackExists     = $rollbackExists
        LogsExist          = $logsExist
        InstallerStateExists = $installerStateExists
        GatewayConfigExists = $gatewayConfigExists
        GatewayDataExists   = $gatewayDataExists
        GatewayLogsExist    = $gatewayLogsExist
        GatewayStateExists  = $gatewayStateExists
        GatewayReuseRequired = $gatewayConfigExists -or $gatewayDataExists -or $gatewayLogsExist
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
        $Paths.ConfigPath,
        $Paths.GatewayInstallRoot,
        $Paths.GatewayExecutable,
        $Paths.GatewayReleasePath,
        $Paths.GatewayProvenancePath,
        $Paths.GatewayLicensePath,
        $Paths.GatewayNoticePath,
        $Paths.GatewayProgramDataRoot,
        $Paths.GatewayDataDirectory,
        $Paths.GatewayLogDirectory,
        $Paths.GatewayConfigPath
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

function Get-TomlTopLevelIntegerValue {
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

    foreach ($line in ($Content -split "`r?`n")) {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('#') -or $trimmed.Length -eq 0) {
            continue
        }
        if ($trimmed.StartsWith('[')) {
            break
        }
        if ($trimmed -notmatch '^\s*([A-Za-z0-9_-]+)\s*=') {
            continue
        }
        if ($Matches[1] -cne $Key) {
            continue
        }
        if ($found) {
            $duplicate = $true
            continue
        }

        $found = $true
        if ($trimmed -notmatch '^\s*[A-Za-z0-9_-]+\s*=\s*([0-9]+)\s*(?:#.*)?$') {
            $valid = $false
            continue
        }
        try {
            $value = [int]$Matches[1]
        } catch {
            $valid = $false
        }
    }

    return [pscustomobject]@{
        Key       = $Key
        Found     = $found
        Duplicate = $duplicate
        Valid     = $valid
        Value     = $value
    }
}

function Get-TomlTableStringValue {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Content,

        [Parameter(Mandatory = $true)]
        [string]$Table,

        [Parameter(Mandatory = $true)]
        [string]$Key
    )

    $found = $false
    $duplicate = $false
    $valid = $true
    $value = $null
    $inTable = $false

    foreach ($line in ($Content -split "`r?`n")) {
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('#') -or $trimmed.Length -eq 0) {
            continue
        }
        if ($trimmed -match '^\[([A-Za-z0-9_.-]+)\]\s*(?:#.*)?$') {
            $inTable = $Matches[1] -ceq $Table
            continue
        }
        if (-not $inTable -or $trimmed -notmatch '^\s*([A-Za-z0-9_-]+)\s*=') {
            continue
        }
        if ($Matches[1] -cne $Key) {
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

    return [pscustomobject]@{
        Table     = $Table
        Key       = $Key
        Found     = $found
        Duplicate = $duplicate
        Valid     = $valid
        Value     = $value
    }
}

function Assert-GatewayConfigPolicy {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedDatabasePath,

        [Parameter(Mandatory = $true)]
        [string]$ExpectedLogDirectory,

        [Parameter(Mandatory = $false)]
        [int]$ExpectedPort = $script:GatewayPort
    )

    if (-not (Test-Path -LiteralPath $ConfigPath -PathType Leaf)) {
        throw "The installer-managed OPC DA gateway configuration is missing: $ConfigPath"
    }
    try {
        $content = [System.IO.File]::ReadAllText($ConfigPath)
    } catch {
        throw "The installer-managed OPC DA gateway configuration cannot be read: $($_.Exception.Message)"
    }

    $database = Get-TomlTableStringValue -Content $content -Table 'index' -Key 'database_path'
    if (-not $database.Found -or $database.Duplicate -or -not $database.Valid -or
        [string]::IsNullOrWhiteSpace([string]$database.Value)) {
        throw "The OPC DA gateway configuration must contain one valid [index] database_path value."
    }
    $actual = Get-ComparableAbsolutePath -Path ([string]$database.Value)
    $expected = Get-ComparableAbsolutePath -Path $ExpectedDatabasePath
    if ($null -eq $actual -or $actual -ne $expected) {
        throw "The OPC DA gateway index database must remain at the installer-managed path '$ExpectedDatabasePath'."
    }

    $logDirectory = Get-TomlTableStringValue -Content $content -Table 'log' -Key 'dir'
    if (-not $logDirectory.Found -or $logDirectory.Duplicate -or -not $logDirectory.Valid -or
        [string]::IsNullOrWhiteSpace([string]$logDirectory.Value)) {
        throw "The OPC DA gateway configuration must contain one valid [log] dir value."
    }
    $actualLogDirectory = Get-ComparableAbsolutePath -Path ([string]$logDirectory.Value)
    $expectedLogDirectoryPath = Get-ComparableAbsolutePath -Path $ExpectedLogDirectory
    if ($null -eq $actualLogDirectory -or $actualLogDirectory -ne $expectedLogDirectoryPath) {
        throw "The OPC DA gateway log directory must remain at the installer-managed path '$ExpectedLogDirectory'."
    }

    $port = Get-TomlTopLevelIntegerValue -Content $content -Key 'port'
    if (-not $port.Found -or $port.Duplicate -or -not $port.Valid -or
        [int]$port.Value -ne $ExpectedPort) {
        throw "The OPC DA gateway configuration must contain one top-level port value equal to $ExpectedPort."
    }

    return $true
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

function Get-DefaultGatewayConfigContent {
    param(
        [Parameter(Mandatory = $true)]
        [string]$DatabasePath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    $database = ConvertTo-TomlPath -Path $DatabasePath
    $logs = ConvertTo-TomlPath -Path $LogDirectory
    return @"
# BHTune installer defaults. Operator edits are preserved across upgrades.
port = $($script:GatewayPort)

[log]
dir = "$logs"

[index]
database_path = "$database"
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
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutMilliseconds)) {
            try {
                $process.Kill()
                [void]$process.WaitForExit(5000)
            } catch {
            }
            throw "The process did not exit within $TimeoutMilliseconds ms: $FilePath"
        }
        $process.WaitForExit()

        return [pscustomobject]@{
            ExitCode = $process.ExitCode
            StdOut   = $stdoutTask.GetAwaiter().GetResult()
            StdErr   = $stderrTask.GetAwaiter().GetResult()
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

            $processId = 0
            if ($null -ne $service.PSObject.Properties['ProcessId']) {
                $processId = [int]$service.ProcessId
            }
            return [pscustomobject]@{
                Exists      = $true
                Name        = [string]$service.Name
                State       = [string]$service.State
                ProcessId   = $processId
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

function Test-LocalSystemAccount {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Account
    )

    if ([string]::IsNullOrWhiteSpace($Account)) {
        return $false
    }

    $value = $Account.Trim().ToLowerInvariant()
    return $value -eq 'localsystem' -or $value -eq '.\localsystem' -or
        $value -eq 'nt authority\system'
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

function Test-InstallerCreatedGatewayServiceSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$ServiceSnapshot,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    if ($null -eq $ServiceSnapshot -or -not $ServiceSnapshot.Exists) {
        return $false
    }
    if ([string]$ServiceSnapshot.Name -cne $script:GatewayServiceName -or
        -not (Test-LocalSystemAccount -Account $ServiceSnapshot.StartName) -or
        [string]$ServiceSnapshot.StartMode -ne 'Auto' -or
        [string]$ServiceSnapshot.DisplayName -cne $script:GatewayServiceDisplayName) {
        return $false
    }
    return Test-GatewayServiceCommandLine `
        -ActualPathName $ServiceSnapshot.PathName `
        -ExecutablePath $ExecutablePath `
        -ConfigPath $ConfigPath `
        -LogDirectory $LogDirectory
}

function Test-OwnedGatewayServiceSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$ServiceSnapshot,

        [Parameter(Mandatory = $true)]
        [string]$ExecutablePath,

        [Parameter(Mandatory = $true)]
        [string]$ConfigPath,

        [Parameter(Mandatory = $true)]
        [string]$LogDirectory
    )

    if (-not (Test-InstallerCreatedGatewayServiceSnapshot `
            -ServiceSnapshot $ServiceSnapshot `
            -ExecutablePath $ExecutablePath `
            -ConfigPath $ConfigPath `
            -LogDirectory $LogDirectory)) {
        return $false
    }
    return [string]$ServiceSnapshot.Description -ceq $script:GatewayServiceDescription
}

function Remove-InstallerCreatedGatewayServiceRegistration {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $snapshot = Get-ServiceSnapshot -Name $script:GatewayServiceName
    if ($null -eq $snapshot) {
        return
    }
    if (-not (Test-InstallerCreatedGatewayServiceSnapshot `
            -ServiceSnapshot $snapshot `
            -ExecutablePath $Paths.GatewayExecutable `
            -ConfigPath $Paths.GatewayConfigPath `
            -LogDirectory $Paths.GatewayLogDirectory)) {
        throw "Refusing to remove service '$($script:GatewayServiceName)' because it does not match the candidate registration."
    }
    if ($snapshot.State -ne 'Stopped') {
        Stop-ServiceByName -Name $script:GatewayServiceName | Out-Null
    }
    Remove-ServiceByName -Name $script:GatewayServiceName
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

function New-LocalSystemService {
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
        ErrorAction    = 'Stop'
    }
    if (-not [string]::IsNullOrWhiteSpace($Description)) {
        $parameters.Description = $Description
    }

    New-Service @parameters | Out-Null
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

function New-InstallerGatewayService {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    $registrationExists = Invoke-ServiceRegistrationQuery -Name $script:GatewayServiceName
    $serviceSnapshot = Get-ServiceSnapshot -Name $script:GatewayServiceName
    if ($registrationExists -or $null -ne $serviceSnapshot) {
        throw "The service '$script:GatewayServiceName' already exists."
    }
    if (-not (Test-Path -LiteralPath $Paths.GatewayExecutable -PathType Leaf)) {
        throw "The OPC DA gateway executable is missing: $($Paths.GatewayExecutable)"
    }

    $installAttempted = $false
    try {
        $arguments = '--config "' + $Paths.GatewayConfigPath + '" --port ' +
            $script:GatewayPort + ' --log-dir "' + $Paths.GatewayLogDirectory + '" install'
        $installAttempted = $true
        $result = Invoke-CapturedProcess -FilePath $Paths.GatewayExecutable -Arguments $arguments -TimeoutMilliseconds 30000
        if ($result.ExitCode -ne 0) {
            throw "The OPC DA gateway service installer failed with exit code $($result.ExitCode): $($result.StdOut) $($result.StdErr)"
        }

        $created = Get-ServiceSnapshot -Name $script:GatewayServiceName
        if (-not (Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $created `
                -ExecutablePath $Paths.GatewayExecutable `
                -ConfigPath $Paths.GatewayConfigPath `
                -LogDirectory $Paths.GatewayLogDirectory)) {
            throw 'The newly registered OPC DA gateway service does not match the installer-owned LocalSystem definition.'
        }
    } catch {
        if ($installAttempted) {
            try {
                $partial = Get-ServiceSnapshot -Name $script:GatewayServiceName
                if ($null -ne $partial -and
                    (Test-InstallerCreatedGatewayServiceSnapshot `
                        -ServiceSnapshot $partial `
                        -ExecutablePath $Paths.GatewayExecutable `
                        -ConfigPath $Paths.GatewayConfigPath `
                        -LogDirectory $Paths.GatewayLogDirectory)) {
                    Remove-InstallerCreatedGatewayServiceRegistration -Paths $Paths
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

function Get-ProcessSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId
    )

    if ($ProcessId -le 0) {
        return $null
    }

    try {
        $process = Get-WmiObject -Class Win32_Process -Filter ("ProcessId={0}" -f $ProcessId) -ErrorAction Stop
    } catch {
        throw "Unable to inspect process ${ProcessId}: $($_.Exception.Message)"
    }
    if ($null -eq $process) {
        return $null
    }

    return [pscustomobject]@{
        ProcessId      = [int]$process.ProcessId
        ExecutablePath = [string]$process.ExecutablePath
        CommandLine    = [string]$process.CommandLine
    }
}

function Get-TcpListenerSnapshots {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Port
    )

    try {
        $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction Stop)
    } catch {
        throw "Unable to inspect TCP port $Port listeners: $($_.Exception.Message)"
    }

    return @($listeners | ForEach-Object {
            [pscustomobject]@{
                LocalAddress  = [string]$_.LocalAddress
                LocalPort     = [int]$_.LocalPort
                OwningProcess = [int]$_.OwningProcess
            }
        })
}

function Test-TcpPortFree {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Port
    )

    return @(Get-TcpListenerSnapshots -Port $Port).Count -eq 0
}

function Wait-TcpPortFree {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Port,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        if (Test-TcpPortFree -Port $Port) {
            return
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    $owners = @(Get-TcpListenerSnapshots -Port $Port | ForEach-Object { $_.OwningProcess } | Sort-Object -Unique)
    throw "TCP port $Port remains in use by process ID(s): $($owners -join ', ')."
}

function Assert-GatewayPortAvailable {
    param(
        [Parameter(Mandatory = $false)]
        [int]$Port = $script:GatewayPort
    )

    $listeners = @(Get-TcpListenerSnapshots -Port $Port)
    if ($listeners.Count -gt 0) {
        $owners = @($listeners | ForEach-Object { $_.OwningProcess } | Sort-Object -Unique)
        throw "TCP port $Port is already owned by process ID(s): $($owners -join ', ')."
    }
    return $true
}

function Assert-GatewayListenerOwnership {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [psobject]$ServiceSnapshot
    )

    if ($null -eq $ServiceSnapshot) {
        $ServiceSnapshot = Get-ServiceSnapshot -Name $script:GatewayServiceName
    }
    if ($null -eq $ServiceSnapshot -or $ServiceSnapshot.State -ne 'Running' -or
        [int]$ServiceSnapshot.ProcessId -le 0) {
        throw "The installer-owned OPC DA gateway service is not running with a valid process ID."
    }

    $listeners = @(Get-TcpListenerSnapshots -Port $script:GatewayPort)
    if ($listeners.Count -eq 0) {
        throw "The installer-owned OPC DA gateway is not listening on TCP port $script:GatewayPort."
    }
    if (-not ($listeners | Where-Object { $_.LocalAddress -eq '0.0.0.0' })) {
        throw "The installer-owned OPC DA gateway is not listening on 0.0.0.0:$script:GatewayPort."
    }
    $unexpected = @($listeners | Where-Object { $_.OwningProcess -ne [int]$ServiceSnapshot.ProcessId })
    if ($unexpected.Count -gt 0) {
        $owners = @($unexpected | ForEach-Object { $_.OwningProcess } | Sort-Object -Unique)
        throw "TCP port $script:GatewayPort has an unexpected listener owner: $($owners -join ', ')."
    }

    $process = Get-ProcessSnapshot -ProcessId ([int]$ServiceSnapshot.ProcessId)
    if ($null -eq $process) {
        throw "The OPC DA gateway service process $($ServiceSnapshot.ProcessId) disappeared during validation."
    }
    $actualPath = Get-ComparableAbsolutePath -Path ([string]$process.ExecutablePath)
    $expectedPath = Get-ComparableAbsolutePath -Path $Paths.GatewayExecutable
    if ($null -eq $actualPath -or $actualPath -ne $expectedPath) {
        throw "TCP port $script:GatewayPort is owned by unexpected executable '$($process.ExecutablePath)'."
    }

    return [pscustomobject]@{
        Process   = $process
        Listeners = $listeners
    }
}

function Wait-GatewayListenerOwnership {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $lastError = 'the listener has not appeared'
    do {
        try {
            $service = Get-ServiceSnapshot -Name $script:GatewayServiceName
            return Assert-GatewayListenerOwnership -Paths $Paths -ServiceSnapshot $service
        } catch {
            $lastError = $_.Exception.Message
            Start-Sleep -Milliseconds 250
        }
    } while ([DateTime]::UtcNow -lt $deadline)

    throw "The OPC DA gateway listener did not become valid within $TimeoutSeconds seconds: $lastError"
}

function Start-InstallerGatewayService {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $service = Start-ServiceByName -Name $script:GatewayServiceName -TimeoutSeconds $TimeoutSeconds
    Wait-GatewayListenerOwnership -Paths $Paths -TimeoutSeconds $TimeoutSeconds | Out-Null
    return $service
}

function Stop-InstallerGatewayService {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $snapshot = Get-ServiceSnapshot -Name $script:GatewayServiceName
    if ($null -eq $snapshot) {
        Wait-TcpPortFree -Port $script:GatewayPort -TimeoutSeconds $TimeoutSeconds
        return $null
    }
    if (-not (Test-OwnedGatewayServiceSnapshot `
            -ServiceSnapshot $snapshot `
            -ExecutablePath $Paths.GatewayExecutable `
            -ConfigPath $Paths.GatewayConfigPath `
            -LogDirectory $Paths.GatewayLogDirectory)) {
        throw "The service '$script:GatewayServiceName' no longer matches the installer-owned definition."
    }

    $verifiedProcessId = [int]$snapshot.ProcessId
    if ($snapshot.State -ne 'Stopped') {
        Stop-ServiceByName -Name $script:GatewayServiceName -TimeoutSeconds $TimeoutSeconds | Out-Null
    }

    $portFree = $false
    try {
        Wait-TcpPortFree -Port $script:GatewayPort -TimeoutSeconds 5
        $portFree = $true
    } catch {
        $listeners = @(Get-TcpListenerSnapshots -Port $script:GatewayPort)
        if ($verifiedProcessId -le 0 -or
            @($listeners | Where-Object { $_.OwningProcess -ne $verifiedProcessId }).Count -gt 0) {
            throw
        }
    }

    $process = if ($verifiedProcessId -gt 0) {
        Get-ProcessSnapshot -ProcessId $verifiedProcessId
    } else {
        $null
    }
    if ($null -ne $process) {
        if ((Get-ComparableAbsolutePath -Path $process.ExecutablePath) -ne
            (Get-ComparableAbsolutePath -Path $Paths.GatewayExecutable)) {
            throw "The previously verified gateway PID $verifiedProcessId now belongs to an unexpected executable."
        }

        Stop-Process -Id $verifiedProcessId -Force -ErrorAction Stop
    }
    if (-not $portFree -or $null -ne $process) {
        Wait-TcpPortFree -Port $script:GatewayPort -TimeoutSeconds $TimeoutSeconds
    }
    return $snapshot
}

function Invoke-GatewaySmokeCheck {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutMilliseconds = 30000
    )

    if (-not (Test-Path -LiteralPath $Paths.CliExecutable -PathType Leaf)) {
        throw "Cannot smoke-test the OPC DA gateway because the BHTune CLI is missing: $($Paths.CliExecutable)"
    }

    $result = Invoke-CapturedProcess `
        -FilePath $Paths.CliExecutable `
        -Arguments 'opc --output json servers --bridge-host 127.0.0.1:7600' `
        -TimeoutMilliseconds $TimeoutMilliseconds
    if ($result.ExitCode -ne 0) {
        throw "The OPC DA gateway smoke check failed with exit code $($result.ExitCode): $($result.StdOut) $($result.StdErr)"
    }
    try {
        $payload = $result.StdOut | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "The OPC DA gateway smoke check did not return valid JSON: $($_.Exception.Message)"
    }
    if ($null -eq $payload.PSObject.Properties['servers']) {
        throw "The OPC DA gateway smoke check JSON did not contain a servers array."
    }

    return $payload
}

function Remove-InstallerGatewayService {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [switch]$AllowMissing
    )

    $existing = Get-ServiceSnapshot -Name $script:GatewayServiceName
    if ($null -eq $existing) {
        if ($AllowMissing) {
            return
        }
        throw "The installer-owned OPC DA gateway service is missing."
    }
    if (-not (Test-OwnedGatewayServiceSnapshot `
            -ServiceSnapshot $existing `
            -ExecutablePath $Paths.GatewayExecutable `
            -ConfigPath $Paths.GatewayConfigPath `
            -LogDirectory $Paths.GatewayLogDirectory)) {
        throw "The service '$script:GatewayServiceName' no longer matches the installer-owned definition."
    }
    Stop-InstallerGatewayService -Paths $Paths | Out-Null
    if (-not (Test-Path -LiteralPath $Paths.GatewayExecutable -PathType Leaf)) {
        throw "Cannot unregister the OPC DA gateway because its installer-owned executable is missing."
    }

    $result = Invoke-CapturedProcess `
        -FilePath $Paths.GatewayExecutable `
        -Arguments 'uninstall' `
        -TimeoutMilliseconds 30000
    if ($result.ExitCode -ne 0) {
        throw "The OPC DA gateway service uninstaller failed with exit code $($result.ExitCode): $($result.StdOut) $($result.StdErr)"
    }
    Wait-ServiceRegistrationGone -Name $script:GatewayServiceName
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

function Start-ServiceByName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    Start-Service -Name $Name -ErrorAction Stop
    return Wait-ServiceState -Name $Name -State Running -TimeoutSeconds $TimeoutSeconds
}

function Stop-ServiceByName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $snapshot = Get-ServiceSnapshot -Name $Name
    if ($null -eq $snapshot -or $snapshot.State -eq 'Stopped') {
        return $snapshot
    }

    Stop-Service -Name $Name -Force -ErrorAction Stop
    return Wait-ServiceState -Name $Name -State Stopped -TimeoutSeconds $TimeoutSeconds
}

function Start-InstallerService {
    param(
        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    return Start-ServiceByName -Name $script:InstallerServiceName -TimeoutSeconds $TimeoutSeconds
}

function Stop-InstallerService {
    param(
        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    return Stop-ServiceByName -Name $script:InstallerServiceName -TimeoutSeconds $TimeoutSeconds
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

function Restore-GatewayServiceSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [psobject]$Snapshot
    )

    $current = Get-ServiceSnapshot -Name $script:GatewayServiceName
    if ($null -ne $current) {
        if (-not (Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $current `
                -ExecutablePath $Paths.GatewayExecutable `
                -ConfigPath $Paths.GatewayConfigPath `
                -LogDirectory $Paths.GatewayLogDirectory)) {
            throw "Cannot replace service '$script:GatewayServiceName' during rollback because its definition is no longer installer-owned."
        }
        Remove-InstallerGatewayService -Paths $Paths
    }

    if ($null -eq $Snapshot -or -not $Snapshot.Exists) {
        return
    }
    if (-not (Test-LocalSystemAccount -Account $Snapshot.StartName)) {
        throw "Cannot restore service '$script:GatewayServiceName' without a supported LocalSystem account."
    }
    $startMode = switch ($Snapshot.StartMode) {
        'Auto' { 'Automatic' }
        'Disabled' { 'Disabled' }
        default { 'Manual' }
    }
    New-LocalSystemService `
        -Name $script:GatewayServiceName `
        -BinaryPathName $Snapshot.PathName `
        -DisplayName $Snapshot.DisplayName `
        -StartupType $startMode `
        -Description $Snapshot.Description

    if ($Snapshot.State -eq 'Running') {
        Start-InstallerGatewayService -Paths $Paths | Out-Null
    } else {
        Wait-ServiceState -Name $script:GatewayServiceName -State Stopped | Out-Null
        Wait-TcpPortFree -Port $script:GatewayPort
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
        [bool]$ConfigCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$ManageGateway = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayProgramDataRootCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayDataDirectoryCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayLogDirectoryCreated = $false,

        [Parameter(Mandatory = $false)]
        [bool]$GatewayConfigCreated = $false
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

    if ($ManageGateway) {
        if ($GatewayProgramDataRootCreated) {
            Set-InstallerAclPath -Path $Paths.GatewayProgramDataRoot -LocalServiceRights $null -Directory $true
        }
        if ($GatewayDataDirectoryCreated) {
            Set-InstallerAclPath -Path $Paths.GatewayDataDirectory -LocalServiceRights $null -Directory $true
        }
        if ($GatewayLogDirectoryCreated) {
            Set-InstallerAclPath -Path $Paths.GatewayLogDirectory -LocalServiceRights $null -Directory $true
        }
        if ($GatewayConfigCreated -and (Test-Path -LiteralPath $Paths.GatewayConfigPath -PathType Leaf)) {
            Set-InstallerAclPath -Path $Paths.GatewayConfigPath -LocalServiceRights $null -Directory $false
        }

        foreach ($path in @(
                $Paths.GatewayInstallRoot,
                $Paths.GatewayExecutable,
                $Paths.GatewayReleasePath,
                $Paths.GatewayProvenancePath,
                $Paths.GatewayLicensePath,
                $Paths.GatewayNoticePath
            )) {
            if (Test-Path -LiteralPath $path) {
                Set-InstallerAclPath `
                    -Path $path `
                    -LocalServiceRights $null `
                    -Directory (Test-Path -LiteralPath $path -PathType Container)
            }
        }
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
