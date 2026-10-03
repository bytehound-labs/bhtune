# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Get-InstallerPaths {
    param(
        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$InstallRoot,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ProgramDataRoot
    )

    if ($script:InstallerLifecycleTestActive) {
        if ([string]::IsNullOrWhiteSpace($InstallRoot)) {
            $InstallRoot = Join-Path $script:InstallerLifecycleTestRoot 'ProgramFiles\ByteHound\bhtune'
        }
        if ([string]::IsNullOrWhiteSpace($ProgramDataRoot)) {
            $ProgramDataRoot = Join-Path $script:InstallerLifecycleTestRoot 'ProgramData\ByteHound\bhtune'
        }
    }

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
    if ($script:InstallerLifecycleTestActive) {
        $commonPrograms = Join-Path $script:InstallerLifecycleTestRoot 'CommonStartMenu\Programs'
    } elseif (-not [string]::IsNullOrWhiteSpace($env:ProgramData)) {
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

function Set-InstallerLifecycleTestContext {
    param(
        [Parameter(Mandatory = $true)]
        [string]$LifecycleTestId,

        [Parameter(Mandatory = $true)]
        [string]$LifecycleTestRoot,

        [Parameter(Mandatory = $true)]
        [string]$InstallRoot,

        [Parameter(Mandatory = $true)]
        [string]$ProgramDataRoot
    )

    if ($LifecycleTestId -cnotmatch '^[0-9a-f]{32}$') {
        throw 'The isolated lifecycle test ID must be a 32-character lowercase hexadecimal value.'
    }

    $root = [System.IO.Path]::GetFullPath($LifecycleTestRoot).TrimEnd([char[]]@('\', '/'))
    $expectedLeaf = "bhtune-installer-lifecycle-$LifecycleTestId"
    if ([System.IO.Path]::GetFileName($root) -cne $expectedLeaf) {
        throw "The isolated lifecycle root must end in '$expectedLeaf'."
    }

    $expectedInstallRoot = [System.IO.Path]::GetFullPath(
        (Join-Path $root 'ProgramFiles\ByteHound\bhtune')
    ).TrimEnd([char[]]@('\', '/'))
    $expectedProgramDataRoot = [System.IO.Path]::GetFullPath(
        (Join-Path $root 'ProgramData\ByteHound\bhtune')
    ).TrimEnd([char[]]@('\', '/'))
    $actualInstallRoot = [System.IO.Path]::GetFullPath($InstallRoot).TrimEnd([char[]]@('\', '/'))
    $actualProgramDataRoot = [System.IO.Path]::GetFullPath($ProgramDataRoot).TrimEnd([char[]]@('\', '/'))
    $pathComparison = [System.StringComparison]::OrdinalIgnoreCase
    if (-not [string]::Equals($actualInstallRoot, $expectedInstallRoot, $pathComparison) -or
        -not [string]::Equals($actualProgramDataRoot, $expectedProgramDataRoot, $pathComparison)) {
        throw 'The isolated lifecycle install and ProgramData roots must remain beneath their unique test root.'
    }

    foreach ($protectedRoot in @(
            $env:SystemRoot,
            [System.Environment]::GetEnvironmentVariable('ProgramFiles'),
            [System.Environment]::GetEnvironmentVariable('ProgramFiles(x86)'),
            [System.Environment]::GetEnvironmentVariable('ProgramData')
        )) {
        if ([string]::IsNullOrWhiteSpace($protectedRoot)) {
            continue
        }
        $protectedPath = [System.IO.Path]::GetFullPath($protectedRoot).TrimEnd([char[]]@('\', '/'))
        $protectedPrefix = $protectedPath + [System.IO.Path]::DirectorySeparatorChar
        if ([string]::Equals($root, $protectedPath, $pathComparison) -or
            $root.StartsWith($protectedPrefix, $pathComparison)) {
            throw "The isolated lifecycle root '$root' overlaps protected system or product state."
        }
    }

    $script:InstallerLifecycleTestId = $LifecycleTestId
    $script:InstallerLifecycleTestRoot = $root
    $script:InstallerLifecycleTestActive = $true
    $script:InstallerServiceName = "BhtuneServer-$LifecycleTestId"
    # The pinned gateway registers a fixed SCM identity, so only BHTune's
    # service name can be scoped to this lifecycle test.
    $script:InstallerMarkerPath = "HKLM:\Software\ByteHound\bhtune\LifecycleTests\$LifecycleTestId"
    $script:InstallerUninstallPath = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\BHTune-Lifecycle-$LifecycleTestId"
    Assert-NoReparsePointInPath -Path $root -Name 'the isolated lifecycle test root'
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
    $moduleTargets = @(
        Get-InstallerModulePayloadRelativePaths |
            ForEach-Object {
                Join-Path $Paths.InstallRoot ("installer\{0}" -f $_.Replace('/', '\'))
            }
    )
    $targets = @(
        $Paths.ProgramDataRoot,
        $Paths.InstallRoot,
        (Join-Path $Paths.InstallRoot 'installer'),
        (Join-Path $Paths.InstallRoot 'installer\Private'),
        (Join-Path $Paths.InstallRoot 'bhtune.exe'),
        (Join-Path $Paths.InstallRoot 'bhtune-server.exe'),
        (Join-Path $Paths.InstallRoot 'LICENSE'),
        (Join-Path $Paths.InstallRoot 'README.md')
    )
    $targets += $moduleTargets
    $targets += @(
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
    return $targets
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
