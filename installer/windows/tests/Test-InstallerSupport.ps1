[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$helperPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'InstallerSupport.ps1'
if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
    throw "Installer helper is missing: $helperPath"
}
. $helperPath

$script:Passed = 0
$script:WorkRoot = Join-Path $PSScriptRoot '.work'
$script:OriginalProgramData = $env:ProgramData

function Assert-True {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Condition,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if (-not $Condition) {
        throw "Assertion failed: $Message"
    }
    $script:Passed++
}

function Assert-Equal {
    param(
        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [object]$Actual,

        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [object]$Expected,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if ($Actual -is [System.Array] -or $Expected -is [System.Array]) {
        $actualText = @($Actual) -join '|'
        $expectedText = @($Expected) -join '|'
        $same = $actualText -ceq $expectedText
    } else {
        $same = $Actual -ceq $Expected
    }

    if (-not $same) {
        throw "Assertion failed: $Message. Expected '$Expected', got '$Actual'."
    }
    $script:Passed++
}

function Assert-Null {
    param(
        [Parameter(Mandatory = $false)]
        [AllowNull()]
        [object]$Actual,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if ($null -ne $Actual) {
        throw "Assertion failed: $Message. Expected a null value, got '$Actual'."
    }
    $script:Passed++
}

function Assert-Throws {
    param(
        [Parameter(Mandatory = $true)]
        [scriptblock]$Action,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    $thrown = $false
    try {
        & $Action
    } catch {
        $thrown = $true
    }

    Assert-True -Condition $thrown -Message $Message
}

function Write-TestFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $parent = Split-Path -Parent $Path
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    [System.IO.File]::WriteAllText($Path, $Content, (New-Object System.Text.UTF8Encoding($false)))
}

function Import-InstallerFunction {
    param(
        [Parameter(Mandatory = $true)]
        [System.Management.Automation.Language.Ast]$ScriptAst,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $functionAst = $ScriptAst.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $Name
        }, $true)
    if ($null -eq $functionAst) {
        throw "Installer function '$Name' is missing from the parsed source."
    }
    Set-Item -Path ("Function:\global:{0}" -f $Name) -Value $functionAst.Body.GetScriptBlock()
}

try {
    if (Test-Path -LiteralPath $script:WorkRoot) {
        Remove-Item -LiteralPath $script:WorkRoot -Recurse -Force
    }
    New-Item -ItemType Directory -Path $script:WorkRoot -Force | Out-Null
    if ([string]::IsNullOrWhiteSpace($env:ProgramData)) {
        $env:ProgramData = Join-Path $script:WorkRoot 'ProgramData'
    }

    Assert-True -Condition (Test-StableVersion -Version '1.2.3') -Message 'stable versions are accepted'
    Assert-True -Condition (Test-StableVersion -Version 'v1.2.3') -Message 'v-prefixed stable versions are accepted'
    Assert-True -Condition (-not (Test-StableVersion -Version '1.2.3-rc.1')) -Message 'pre-release versions are rejected'
    Assert-Equal -Actual (ConvertTo-NormalizedVersion -Version ' v1.2.3 ') -Expected '1.2.3' -Message 'version normalization removes v and whitespace'
    Assert-Equal -Actual (Get-VersionFromStableTag -Tag 'v2.4.6') -Expected '2.4.6' -Message 'stable tag parsing returns the version'
    Assert-Throws -Action { Get-VersionFromStableTag -Tag 'release-2.4.6' } -Message 'non-stable tags are rejected'
    Assert-Throws -Action { Assert-InstallerVersionContract -ExpectedVersion '1.2.3' -ReleaseTag 'v1.2.4' } -Message 'mismatched release tags are rejected'
    Assert-True -Condition (Test-SupportedInstallerSchemaVersion -Version 2) -Message 'legacy schema-v2 installer state remains readable'
    Assert-True -Condition (Test-SupportedInstallerSchemaVersion -Version 3) -Message 'gateway-aware schema-v3 installer state is supported'
    Assert-True -Condition (-not (Test-SupportedInstallerSchemaVersion -Version 1)) -Message 'unsupported installer schemas are rejected'
    $objectSnapshot = [pscustomobject]@{ Value = 'object-value' }
    $dictionarySnapshot = [ordered]@{ Value = 'dictionary-value' }
    Assert-Equal -Actual (Get-SnapshotValue -Snapshot $objectSnapshot -Name 'Value') -Expected 'object-value' -Message 'object snapshot values are available to every installer caller'
    Assert-Equal -Actual (Get-SnapshotValue -Snapshot $dictionarySnapshot -Name 'Value') -Expected 'dictionary-value' -Message 'dictionary snapshot values are available to every installer caller'
    Assert-Null -Actual (Get-SnapshotValue -Snapshot $objectSnapshot -Name 'Missing') -Message 'missing object snapshot values remain null'
    Assert-Null -Actual (Get-SnapshotValue -Snapshot $null -Name 'Value') -Message 'null snapshots remain null'
    Assert-True -Condition (Test-SafeRollbackRelativePath -RelativePath 'install\bhtune.exe') -Message 'normalized rollback paths are accepted'
    foreach ($unsafePath in @(
            '',
            '.\bhtune.exe',
            '..\bhtune.exe',
            'install\..\bhtune.exe',
            '\absolute\bhtune.exe',
            'C:\bhtune.exe',
            'install::bhtune.exe',
            'install\\bhtune.exe'
        )) {
        Assert-True -Condition (-not (Test-SafeRollbackRelativePath -RelativePath $unsafePath)) -Message "unsafe rollback path '$unsafePath' is rejected"
    }

    Assert-True -Condition (Assert-FailureInjectionPolicy -FailureInjection None -TestOnly:$false) -Message 'normal mode accepts no injection'
    Assert-Throws -Action { Assert-FailureInjectionPolicy -FailureInjection HealthMismatch -TestOnly:$false } -Message 'normal mode rejects test-only injection'
    Assert-True -Condition (Assert-FailureInjectionPolicy -FailureInjection CommitFailure -TestOnly:$true) -Message 'test mode accepts an explicit injection'
    Assert-Throws -Action { Assert-FailureInjectionPolicy -FailureInjection None -TestOnly:$true } -Message 'test mode requires an explicit injection'
    Assert-True -Condition (ConvertTo-InstallerBoolean -Value '1' -Name 'add') -Message 'numeric true installer switches are parsed'
    Assert-True -Condition (-not (ConvertTo-InstallerBoolean -Value '0' -Name 'add')) -Message 'numeric false installer switches are parsed'
    Assert-True -Condition (ConvertTo-InstallerBoolean -Value $true -Name 'add') -Message 'boolean installer switches are preserved'
    Assert-Throws -Action { ConvertTo-InstallerBoolean -Value 'maybe' -Name 'add' } -Message 'invalid installer switches are rejected'

    $paths = Get-InstallerPaths -InstallRoot (Join-Path $script:WorkRoot 'ProgramFiles\ByteHound\bhtune') -ProgramDataRoot (Join-Path $script:WorkRoot 'ProgramData\ByteHound\bhtune')
    Assert-Equal -Actual $paths.ConfigPath -Expected (Join-Path $script:WorkRoot 'ProgramData\ByteHound\bhtune\bhtune.toml') -Message 'fixed config path is derived from ProgramData'
    Assert-Equal -Actual $paths.ServiceName -Expected 'BhtuneServer' -Message 'service name is fixed'
    Assert-Equal -Actual $paths.GatewayExecutable -Expected (Join-Path $script:WorkRoot 'ProgramFiles\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe') -Message 'gateway executable path is fixed below the installer root'
    Assert-Equal -Actual $paths.GatewayConfigPath -Expected (Join-Path $script:WorkRoot 'ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml') -Message 'gateway config path is fixed below ProgramData'
    Assert-Equal -Actual $paths.GatewayDatabasePath -Expected (Join-Path $script:WorkRoot 'ProgramData\ByteHound\bhtune\gateway\data\index.sqlite3') -Message 'gateway index path is fixed below ProgramData'
    Assert-Equal -Actual $paths.GatewayServiceName -Expected 'OpcdaBridgeGateway' -Message 'gateway service name is fixed'
    Assert-Equal -Actual $paths.GatewayPort -Expected 7600 -Message 'gateway listener port is fixed'
    $oldProgramW6432 = $env:ProgramW6432
    $oldProgramFiles = $env:ProgramFiles
    $oldProgramData = $env:ProgramData
    try {
        $env:ProgramW6432 = Join-Path $script:WorkRoot 'fixed-program-files'
        $env:ProgramFiles = Join-Path $script:WorkRoot 'fallback-program-files'
        $env:ProgramData = Join-Path $script:WorkRoot 'fixed-program-data'
        $fixedPaths = Get-InstallerPaths
        Assert-True -Condition (Assert-InstallerFixedPaths -Paths $fixedPaths) -Message 'default installer roots are accepted'
        $wrongPaths = Get-InstallerPaths -InstallRoot (Join-Path $script:WorkRoot 'other-program-files') -ProgramDataRoot $fixedPaths.ProgramDataRoot
        Assert-Throws -Action { Assert-InstallerFixedPaths -Paths $wrongPaths } -Message 'custom installer roots are rejected'
    } finally {
        if ($null -eq $oldProgramW6432) { Remove-Item Env:ProgramW6432 -ErrorAction SilentlyContinue } else { $env:ProgramW6432 = $oldProgramW6432 }
        if ($null -eq $oldProgramFiles) { Remove-Item Env:ProgramFiles -ErrorAction SilentlyContinue } else { $env:ProgramFiles = $oldProgramFiles }
        if ($null -eq $oldProgramData) { Remove-Item Env:ProgramData -ErrorAction SilentlyContinue } else { $env:ProgramData = $oldProgramData }
    }

    $pathEntry = Add-ExactPathEntry -ExistingPath 'C:\Tools;C:\Program Files\ByteHound\bhtune' -Entry 'c:/program files/bytehound/bhtune'
    Assert-True -Condition (-not $pathEntry.Changed) -Message 'PATH add is case and separator insensitive'
    Assert-Equal -Actual $pathEntry.Value -Expected 'C:\Tools;C:\Program Files\ByteHound\bhtune' -Message 'duplicate PATH add preserves the original string'
    $pathWithEmptySegment = Add-ExactPathEntry -ExistingPath 'C:\Tools;;C:\Other;' -Entry 'C:\Program Files\ByteHound\bhtune'
    Assert-True -Condition $pathWithEmptySegment.Changed -Message 'PATH add changes a list without the installer entry'
    Assert-Equal -Actual $pathWithEmptySegment.Value -Expected 'C:\Tools;;C:\Other;;C:\Program Files\ByteHound\bhtune' -Message 'PATH add preserves unrelated empty segments'
    $removed = Remove-ExactPathEntry -ExistingPath 'C:\Tools;C:\Program Files\ByteHound\bhtune;C:\Other' -Entry 'c:/program files/bytehound/bhtune'
    Assert-True -Condition $removed.Changed -Message 'PATH remove detects the exact normalized entry'
    Assert-Equal -Actual $removed.Value -Expected 'C:\Tools;C:\Other' -Message 'PATH remove preserves unrelated entries'
    $removedOneDuplicate = Remove-ExactPathEntry -ExistingPath 'C:\Program Files\ByteHound\bhtune;C:\Tools;C:/Program Files/ByteHound/bhtune' -Entry 'C:\Program Files\ByteHound\bhtune'
    Assert-Equal -Actual $removedOneDuplicate.Value -Expected 'C:\Program Files\ByteHound\bhtune;C:\Tools' -Message 'PATH remove leaves an administrator-owned equivalent duplicate untouched'

    $defaultConfig = Get-DefaultConfigContent -DatabasePath $paths.DatabasePath -LogDirectory $paths.LogDirectory
    Assert-True -Condition ($defaultConfig.Contains('bind = "127.0.0.1:8787"')) -Message 'default config binds localhost'
    Assert-True -Condition ($defaultConfig.Contains('db = "')) -Message 'default config declares the database'
    $defaultGatewayConfig = Get-DefaultGatewayConfigContent `
        -DatabasePath $paths.GatewayDatabasePath `
        -LogDirectory $paths.GatewayLogDirectory
    Assert-True -Condition ($defaultGatewayConfig.Contains('port = 7600')) -Message 'default gateway config pins the managed listener port'
    Assert-True -Condition ($defaultGatewayConfig.Contains(($paths.GatewayDatabasePath.Replace('\', '/')))) -Message 'default gateway config pins the managed index database'
    Assert-True -Condition ($defaultGatewayConfig.Contains(($paths.GatewayLogDirectory.Replace('\', '/')))) -Message 'default gateway config pins the managed log directory'
    Write-TestFile -Path $paths.GatewayConfigPath -Content $defaultGatewayConfig
    Assert-True -Condition (Assert-GatewayConfigPolicy `
            -ConfigPath $paths.GatewayConfigPath `
            -ExpectedDatabasePath $paths.GatewayDatabasePath `
            -ExpectedLogDirectory $paths.GatewayLogDirectory) -Message 'default gateway config satisfies the managed policy'
    Write-TestFile -Path $paths.GatewayConfigPath -Content ($defaultGatewayConfig.Replace('port = 7600', 'port = 7601'))
    Assert-Throws -Action {
        Assert-GatewayConfigPolicy `
            -ConfigPath $paths.GatewayConfigPath `
            -ExpectedDatabasePath $paths.GatewayDatabasePath `
            -ExpectedLogDirectory $paths.GatewayLogDirectory
    } -Message 'gateway config drift from the managed port is rejected'
    Write-TestFile -Path $paths.GatewayConfigPath -Content ($defaultGatewayConfig.Replace(
            $paths.GatewayDatabasePath.Replace('\', '/'),
            (Join-Path $script:WorkRoot 'external\gateway-index.sqlite3').Replace('\', '/')
        ))
    Assert-Throws -Action {
        Assert-GatewayConfigPolicy `
            -ConfigPath $paths.GatewayConfigPath `
            -ExpectedDatabasePath $paths.GatewayDatabasePath `
            -ExpectedLogDirectory $paths.GatewayLogDirectory
    } -Message 'gateway config drift from the managed index path is rejected'
    Write-TestFile -Path $paths.GatewayConfigPath -Content ($defaultGatewayConfig.Replace(
            $paths.GatewayLogDirectory.Replace('\', '/'),
            (Join-Path $script:WorkRoot 'external\gateway-logs').Replace('\', '/')
        ))
    Assert-Throws -Action {
        Assert-GatewayConfigPolicy `
            -ConfigPath $paths.GatewayConfigPath `
            -ExpectedDatabasePath $paths.GatewayDatabasePath `
            -ExpectedLogDirectory $paths.GatewayLogDirectory
    } -Message 'gateway config drift from the managed log path is rejected'
    Remove-Item -LiteralPath $paths.GatewayConfigPath -Force
    $configPath = Join-Path $script:WorkRoot 'config\bhtune.toml'
    Write-TestFile -Path $configPath -Content $defaultConfig
    $policy = Get-DatabasePolicy -ConfigPath $configPath -DefaultDatabasePath $paths.DatabasePath
    Assert-Equal -Actual $policy.Policy -Expected 'Default' -Message 'default database policy is recognized'
    Write-TestFile -Path $configPath -Content "db = ""C:/external/bhtune.db""`n`n[log]`ndir = ""C:/external/logs""`n"
    $policy = Get-DatabasePolicy -ConfigPath $configPath -DefaultDatabasePath $paths.DatabasePath
    Assert-Equal -Actual $policy.Policy -Expected 'External' -Message 'external database policy is recognized'
    Write-TestFile -Path $configPath -Content "db = C:/not-a-string.db`n"
    $policy = Get-DatabasePolicy -ConfigPath $configPath -DefaultDatabasePath $paths.DatabasePath
    Assert-Equal -Actual $policy.Policy -Expected 'Ambiguous' -Message 'malformed database configuration is ambiguous'

    function Get-ServiceEnvironmentOverrides {
        param(
            [Parameter(Mandatory = $true)]
            [string]$ServiceName
        )

        return [ordered]@{}
    }

    $oldProcessDatabase = [Environment]::GetEnvironmentVariable('BHTUNE_DB', 'Process')
    $oldProcessBind = [Environment]::GetEnvironmentVariable('BHTUNE_BIND', 'Process')
    try {
        $env:BHTUNE_DB = $paths.DatabasePath
        $env:BHTUNE_BIND = '127.0.0.1:8787'
        $environmentPolicy = Get-InstallerEnvironmentPolicy `
            -Paths $paths `
            -MachineDatabaseOverride $null `
            -MachineBindOverride $null `
            -ServiceOverrides ([ordered]@{})
        Assert-True -Condition $environmentPolicy.DatabaseSafe -Message 'managed process database override is accepted'
        Assert-True -Condition $environmentPolicy.BindSafe -Message 'loopback process bind override is accepted'

        $env:BHTUNE_DB = Join-Path $script:WorkRoot 'external\bhtune.db'
        $environmentPolicy = Get-InstallerEnvironmentPolicy `
            -Paths $paths `
            -MachineDatabaseOverride $null `
            -MachineBindOverride $null `
            -ServiceOverrides ([ordered]@{})
        Assert-True -Condition (-not $environmentPolicy.DatabaseSafe) -Message 'external process database override is rejected'
        Assert-True -Condition (@($environmentPolicy.DatabaseConflicts).Count -eq 1) -Message 'database conflict identifies the process scope'
        Assert-Throws -Action { Assert-InstallerEnvironmentPolicy -Paths $paths } -Message 'external process database override fails closed before mutation'

        $env:BHTUNE_DB = $paths.DatabasePath
        $env:BHTUNE_BIND = '0.0.0.0:8787'
        $environmentPolicy = Get-InstallerEnvironmentPolicy `
            -Paths $paths `
            -MachineDatabaseOverride $null `
            -MachineBindOverride $null `
            -ServiceOverrides ([ordered]@{})
        Assert-True -Condition (-not $environmentPolicy.BindSafe) -Message 'non-loopback process bind override is rejected'
        Assert-True -Condition (@($environmentPolicy.BindConflicts).Count -eq 1) -Message 'bind conflict identifies the process scope'
        Assert-Throws -Action { Assert-InstallerEnvironmentPolicy -Paths $paths } -Message 'non-loopback process bind override fails closed before mutation'

        $environmentPolicy = Get-InstallerEnvironmentPolicy `
            -Paths $paths `
            -ProcessDatabaseOverride $paths.DatabasePath `
            -ProcessBindOverride '127.0.0.1:8787' `
            -MachineDatabaseOverride (Join-Path $script:WorkRoot 'machine\bhtune.db') `
            -MachineBindOverride '192.0.2.10:8787' `
            -ServiceOverrides ([ordered]@{
                BHTUNE_DB = Join-Path $script:WorkRoot 'service\bhtune.db'
                BHTUNE_BIND = '10.0.0.5:8787'
            })
        Assert-Equal -Actual @($environmentPolicy.DatabaseConflicts).Count -Expected 2 -Message 'machine and service database conflicts are both reported'
        Assert-Equal -Actual @($environmentPolicy.BindConflicts).Count -Expected 2 -Message 'machine and service bind conflicts are both reported'
    } finally {
        if ($null -eq $oldProcessDatabase) { Remove-Item Env:BHTUNE_DB -ErrorAction SilentlyContinue } else { $env:BHTUNE_DB = $oldProcessDatabase }
        if ($null -eq $oldProcessBind) { Remove-Item Env:BHTUNE_BIND -ErrorAction SilentlyContinue } else { $env:BHTUNE_BIND = $oldProcessBind }
    }

    Assert-True -Condition (-not (Test-PreservedProgramData -Paths $paths)) -Message 'an empty ProgramData root is treated as a clean install'
    Write-TestFile -Path $paths.ConfigPath -Content $defaultConfig
    Write-TestFile -Path $paths.DatabasePath -Content 'database'
    Write-TestFile -Path ($paths.DatabasePath + '-wal') -Content 'wal'
    Write-TestFile -Path ($paths.DatabasePath + '-shm') -Content 'shm'
    Write-TestFile -Path (Join-Path $paths.LogDirectory 'operator.log') -Content 'operator log'
    Write-TestFile -Path (Join-Path $paths.InstallerStateRoot 'operator-state.json') -Content '{}'
    New-Item -ItemType Directory -Path $paths.RollbackRoot -Force | Out-Null
    $preservedState = Get-PreservedProgramDataState -Paths $paths
    Assert-True -Condition $preservedState.ReuseRequired -Message 'existing ProgramData is classified as preserved state'
    Assert-Equal -Actual @($preservedState.DatabaseArtifacts).Count -Expected 3 -Message 'database and WAL/SHM artifacts are preserved'
    Assert-True -Condition $preservedState.ConfigExists -Message 'existing configuration is preserved'
    Assert-True -Condition $preservedState.RollbackExists -Message 'existing rollback state is preserved'
    Assert-True -Condition $preservedState.LogsExist -Message 'existing logs are preserved'
    Assert-True -Condition $preservedState.InstallerStateExists -Message 'existing installer state is preserved'

    $expectedCommand = Get-ExpectedServiceCommandLine -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml'
    Assert-True -Condition (Test-ServiceCommandLine -ActualPathName $expectedCommand -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml') -Message 'exact service command lines are accepted'
    Assert-True -Condition (Test-ServiceCommandLine -ActualPathName 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe --config C:\ProgramData\ByteHound\bhtune\bhtune.toml' -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml') -Message 'normalized service command lines are accepted'
    Assert-True -Condition (-not (Test-ServiceCommandLine -ActualPathName '"C:\Other\bhtune-server.exe" --config "C:\ProgramData\ByteHound\bhtune\bhtune.toml"' -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml')) -Message 'conflicting service executable paths are rejected'
    Assert-True -Condition (-not (Test-ServiceCommandLine -ActualPathName $expectedCommand -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe.bak' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml')) -Message 'executable path prefixes are rejected'
    Assert-True -Condition (-not (Test-ServiceCommandLine -ActualPathName ($expectedCommand + ' --extra') -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml')) -Message 'unexpected service arguments are rejected'

    $payloadRoot = Join-Path $script:WorkRoot 'payload'
    foreach ($name in (Get-RequiredPayloadFiles)) {
        Write-TestFile -Path (Join-Path $payloadRoot $name) -Content $name
    }
    Assert-True -Condition (Assert-PayloadLayout -PayloadRoot $payloadRoot) -Message 'complete payload layouts are accepted'
    Remove-Item -LiteralPath (Join-Path $payloadRoot 'README.md') -Force
    Assert-Throws -Action { Assert-PayloadLayout -PayloadRoot $payloadRoot } -Message 'incomplete payload layouts are rejected'
    Assert-Equal -Actual (Get-VersionFromProcessOutput -Output 'bhtune 3.2.1') -Expected '3.2.1' -Message 'process version output is parsed'
    Assert-Equal -Actual (Get-VersionFromProcessOutput -Output 'bhtune-server v3.2.1') -Expected '3.2.1' -Message 'server process version output is parsed'
    Assert-Equal -Actual (Get-VersionFromProcessOutput -Output 'opcda-bridge-gateway 0.5.9') -Expected '0.5.9' -Message 'gateway process version output is parsed'
    Assert-Null -Actual (Get-VersionFromProcessOutput -Output 'warning: bhtune 3.2.1 payload') -Message 'unstructured version output is rejected'

    $largeOutputScript = Join-Path $script:WorkRoot 'large-output.ps1'
    Write-TestFile -Path $largeOutputScript -Content @'
[Console]::Out.Write(('O' * 262144))
[Console]::Error.Write(('E' * 262144))
exit 7
'@
    $powerShellHost = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
    $captured = Invoke-CapturedProcess `
        -FilePath $powerShellHost `
        -Arguments ('-NoLogo -NoProfile -NonInteractive -File "{0}"' -f $largeOutputScript) `
        -TimeoutMilliseconds 10000
    Assert-Equal -Actual $captured.ExitCode -Expected 7 -Message 'captured processes preserve nonzero exit codes'
    Assert-Equal -Actual $captured.StdOut.Length -Expected 262144 -Message 'captured processes drain large stdout without deadlocking'
    Assert-Equal -Actual $captured.StdErr.Length -Expected 262144 -Message 'captured processes drain large stderr without deadlocking'

    $nsiSource = Get-Content -LiteralPath (Join-Path (Split-Path -Parent $PSScriptRoot) 'bhtune-installer.nsi') -Raw
    Assert-True `
        -Condition $nsiSource.Contains('Function ApplySilentGatewayDefault') `
        -Message 'the NSIS installer defines a silent gateway default policy'
    Assert-True `
        -Condition $nsiSource.Contains('IfSilent silentGatewayDefault applySilentGatewayDefaultDone') `
        -Message 'silent installs take the explicit gateway opt-in path'
    Assert-True `
        -Condition $nsiSource.Contains('StrCpy $InstallGateway "1"') `
        -Message 'interactive clean installs keep the gateway selected by default'
    Assert-True `
        -Condition (
            $nsiSource.IndexOf('Call ApplySilentGatewayDefault', [System.StringComparison]::Ordinal) -lt
            $nsiSource.IndexOf('Call ParseInstallerOptions', [System.StringComparison]::Ordinal)
        ) `
        -Message 'explicit silent command-line options override the safe gateway default'

    $checkedInContractPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'opcda-gateway-release.json'
    $checkedInContract = Assert-GatewayReleaseContract -ContractPath $checkedInContractPath
    Assert-Equal -Actual $checkedInContract.tag -Expected 'opcda-bridge-gateway-v0.5.9' -Message 'the pinned gateway release tag is stable and exact'
    Assert-Equal -Actual $checkedInContract.archive.sha256 -Expected 'd372ff30d6fb61b66767a63aa5bee4548d7c6c1052800f1369fc34f57ab25f30' -Message 'the pinned gateway archive checksum is exact'
    $gatewayInfoJson = '{"application_version":"0.5.9","compatibility_schema_version":1,"features":[{"feature":"core","min_version":1,"max_version":1},{"feature":"namespace","min_version":2,"max_version":2},{"feature":"indexed_search","min_version":2,"max_version":2}]}'
    $gatewayInfo = Assert-GatewayInfoPayload -Json $gatewayInfoJson -Contract $checkedInContract
    Assert-Equal -Actual $gatewayInfo.application_version -Expected '0.5.9' -Message 'gateway-wide handshake accepts the pinned version and protocols'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload `
            -Json ($gatewayInfoJson.Replace('"application_version":"0.5.9"', '"application_version":"0.5.8"')) `
            -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects a different running version'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload `
            -Json ($gatewayInfoJson.Replace('"compatibility_schema_version":1', '"compatibility_schema_version":2')) `
            -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects an unsupported compatibility schema'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload `
            -Json ($gatewayInfoJson.Replace('"min_version":1,"max_version":1', '"min_version":2,"max_version":2')) `
            -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects an unsupported protocol range'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload `
            -Json ($gatewayInfoJson.Replace(',{"feature":"indexed_search","min_version":2,"max_version":2}', '')) `
            -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects a missing required protocol'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload `
            -Json ($gatewayInfoJson.Replace('{"feature":"core","min_version":1,"max_version":1}', '{"feature":"core","min_version":1,"max_version":1},{"feature":"core","min_version":1,"max_version":1}')) `
            -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects a duplicate required protocol'
    Assert-Throws -Action {
        Assert-GatewayInfoPayload -Json 'not-json' -Contract $checkedInContract
    } -Message 'gateway-wide handshake rejects malformed JSON'

    Write-TestFile -Path $paths.CliExecutable -Content 'fixture CLI'
    Write-TestFile -Path $paths.GatewayReleasePath -Content (Get-Content -LiteralPath $checkedInContractPath -Raw)
    $script:GatewaySmokeArguments = $null
    function Invoke-CapturedProcess {
        param(
            [string]$FilePath,
            [string]$Arguments,
            [int]$TimeoutMilliseconds
        )
        $script:GatewaySmokeArguments = $Arguments
        return [pscustomobject]@{
            ExitCode = 0
            StdOut   = $gatewayInfoJson
            StdErr   = ''
        }
    }
    $gatewaySmoke = Invoke-GatewaySmokeCheck -Paths $paths
    Assert-Equal -Actual $gatewaySmoke.application_version -Expected '0.5.9' -Message 'gateway smoke validates the gateway-wide handshake'
    Assert-Equal -Actual $script:GatewaySmokeArguments -Expected 'opc --output json gateway-info --bridge-host 127.0.0.1:7600' -Message 'gateway smoke does not require OPC server enumeration'

    $gatewayPayloadRoot = Join-Path $script:WorkRoot 'gateway-payload'
    New-Item -ItemType Directory -Path $gatewayPayloadRoot -Force | Out-Null
    $gatewayExecutable = Join-Path $gatewayPayloadRoot 'opcda-bridge-gateway.exe'
    $peBytes = New-Object byte[] 512
    $peBytes[0] = 0x4d
    $peBytes[1] = 0x5a
    [BitConverter]::GetBytes([int]0x80).CopyTo($peBytes, 0x3c)
    $peBytes[0x80] = 0x50
    $peBytes[0x81] = 0x45
    $peBytes[0x82] = 0
    $peBytes[0x83] = 0
    [BitConverter]::GetBytes([uint16]0x014c).CopyTo($peBytes, 0x84)
    [System.IO.File]::WriteAllBytes($gatewayExecutable, $peBytes)
    Assert-Equal -Actual (Get-PeMachine -Path $gatewayExecutable) -Expected 'I386' -Message '32-bit x86 PE payloads are recognized'
    [BitConverter]::GetBytes([uint16]0x8664).CopyTo($peBytes, 0x84)
    [System.IO.File]::WriteAllBytes($gatewayExecutable, $peBytes)
    Assert-Equal -Actual (Get-PeMachine -Path $gatewayExecutable) -Expected 'AMD64' -Message '64-bit PE payloads are distinguished from the required gateway architecture'
    [BitConverter]::GetBytes([uint16]0x014c).CopyTo($peBytes, 0x84)
    [System.IO.File]::WriteAllBytes($gatewayExecutable, $peBytes)

    $fixtureContract = $checkedInContract | ConvertTo-Json -Depth 12 | ConvertFrom-Json
    $fixtureContract.executable.sha256 = Get-FileSha256 -Path $gatewayExecutable
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'opcda-gateway-release.json') -Content ($fixtureContract | ConvertTo-Json -Depth 12)
    $fixtureProvenance = [ordered]@{
        schema_version              = 1
        repository                  = $fixtureContract.repository
        tag                         = $fixtureContract.tag
        source_commit               = $fixtureContract.source_commit
        archive_name                = $fixtureContract.archive.name
        archive_sha256              = $fixtureContract.archive.sha256
        executable_name             = $fixtureContract.executable.name
        executable_sha256           = $fixtureContract.executable.sha256
        builder_id                  = $fixtureContract.release_workflow.builder_id
        release_workflow_blob_sha   = $fixtureContract.release_workflow.blob_sha
        sigstore_verified           = $true
        github_provenance_verified  = $true
        compatibility_verified      = $true
    }
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'opcda-gateway-provenance.json') -Content ($fixtureProvenance | ConvertTo-Json -Depth 8)
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'LICENSE-opcda-bridge.txt') -Content 'MIT'
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'NOTICE-opcda-bridge.txt') -Content 'notice'
    $gatewayPayload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $gatewayPayloadRoot
    Assert-Equal -Actual $gatewayPayload.Contract.version -Expected '0.5.9' -Message 'gateway payloads matching the pinned release contract are accepted'
    $fixtureProvenance.compatibility_verified = $false
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'opcda-gateway-provenance.json') -Content ($fixtureProvenance | ConvertTo-Json -Depth 8)
    Assert-Throws -Action { Assert-GatewayPayloadLayout -GatewayPayloadRoot $gatewayPayloadRoot } -Message 'unverified gateway compatibility metadata is rejected'
    $fixtureProvenance.compatibility_verified = $true
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'opcda-gateway-provenance.json') -Content ($fixtureProvenance | ConvertTo-Json -Depth 8)
    Write-TestFile -Path (Join-Path $gatewayPayloadRoot 'unexpected.txt') -Content 'unexpected'
    Assert-Throws -Action { Assert-GatewayPayloadLayout -GatewayPayloadRoot $gatewayPayloadRoot } -Message 'unexpected gateway payload files are rejected'

    $sourceFile = Join-Path $script:WorkRoot 'source.txt'
    $backupRoot = Join-Path $script:WorkRoot 'rollback'
    $backupFile = Join-Path $backupRoot 'files\source.txt'
    Write-TestFile -Path $sourceFile -Content 'rollback content'
    Write-TestFile -Path $backupFile -Content 'rollback content'
    $manifestEntries = New-Object System.Collections.ArrayList
    [void]$manifestEntries.Add((New-HashManifestEntry -SourcePath $sourceFile -BackupPath $backupFile -RelativePath 'source.txt'))
    $manifest = [pscustomobject]@{
        SchemaVersion = 2
        Files = @($manifestEntries)
    }
    $state = [pscustomobject]@{
        SchemaVersion        = 2
        Version              = '1.2.3'
        InstallRootWasPresent = $false
    }
    Write-TestFile -Path (Join-Path $backupRoot 'state.json') -Content ($state | ConvertTo-Json -Depth 8)
    Write-TestFile -Path (Join-Path $backupRoot 'manifest.json') -Content ($manifest | ConvertTo-Json -Depth 8)
    Assert-True -Condition (Test-RollbackBackup -BackupRoot $backupRoot) -Message 'verified rollback manifests are accepted'
    Write-TestFile -Path $backupFile -Content 'tampered'
    Assert-True -Condition (-not (Test-RollbackBackup -BackupRoot $backupRoot)) -Message 'tampered rollback files are rejected'

    $reparseBackupRoot = Join-Path $script:WorkRoot 'rollback-reparse'
    $reparseOutsideRoot = Join-Path $script:WorkRoot 'rollback-reparse-outside'
    $reparseTargetFile = Join-Path $reparseOutsideRoot 'source.txt'
    $reparseLinkRoot = Join-Path $reparseBackupRoot 'files\nested'
    New-Item -ItemType Directory -Path $reparseOutsideRoot -Force | Out-Null
    Write-TestFile -Path $reparseTargetFile -Content 'redirected rollback content'
    $symlinkError = $null
    try {
        New-Item -ItemType SymbolicLink -Path $reparseLinkRoot -Target $reparseOutsideRoot -ErrorAction Stop | Out-Null
    } catch {
        $symlinkError = $_.Exception
    }
    if ($null -eq $symlinkError) {
        $reparseBackupFile = Join-Path $reparseLinkRoot 'source.txt'
        $reparseManifestEntry = New-HashManifestEntry `
            -SourcePath $reparseTargetFile `
            -BackupPath $reparseBackupFile `
            -RelativePath 'nested\source.txt'
        $reparseManifest = [pscustomobject]@{
            SchemaVersion = 2
            Files         = @($reparseManifestEntry)
        }
        $reparseState = [pscustomobject]@{
            SchemaVersion          = 2
            InstallRootWasPresent  = $false
        }
        Write-TestFile -Path (Join-Path $reparseBackupRoot 'state.json') -Content ($reparseState | ConvertTo-Json -Depth 8)
        Write-TestFile -Path (Join-Path $reparseBackupRoot 'manifest.json') -Content ($reparseManifest | ConvertTo-Json -Depth 8)
        Assert-True -Condition (-not (Test-RollbackBackup -BackupRoot $reparseBackupRoot)) -Message 'nested rollback reparse points are rejected'
    }

    $installScriptPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'Install-Bhtune.ps1'
    $parseErrors = $null
    $parseTokens = $null
    $installAst = [System.Management.Automation.Language.Parser]::ParseFile(
        $installScriptPath,
        [ref]$parseTokens,
        [ref]$parseErrors
    )
    Assert-Equal -Actual @($parseErrors).Count -Expected 0 -Message 'installer orchestration script parses for manifest regression coverage'
    $installTransactionAst = $installAst.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Invoke-InstallTransaction'
        }, $true)
    Assert-True -Condition ($null -ne $installTransactionAst) -Message 'install transaction function is present for gateway wiring coverage'
    Assert-True `
        -Condition ($installTransactionAst.Extent.Text -match '(?m)^\s*GatewayService\s*=\s*\$PriorState\.GatewayService\s*$') `
        -Message 'schema-3 install journals record the prior gateway service snapshot required by validation'
    Assert-True `
        -Condition (
            $installTransactionAst.Extent.Text.Contains("-Name 'GatewayServiceExists'") -and
            $installTransactionAst.Extent.Text.Contains('$gatewayRegistrationExists -or $null -ne $PriorState.GatewayService')
        ) `
        -Message 'gateway install preflight treats the SCM registration query as authoritative'
    foreach ($callContract in @(
            @{
                Name       = 'New-StagedPayload'
                Required   = @('SourceGatewayPayloadRoot', 'IncludeGateway')
                Forbidden  = @('GatewayPayloadRoot', 'InstallGateway')
            },
            @{
                Name       = 'Copy-CandidateIntoInstall'
                Required   = @('IncludeGateway')
                Forbidden  = @('InstallGateway')
            },
            @{
                Name       = 'Set-InstallerAcls'
                Required   = @('ManageGateway')
                Forbidden  = @('GatewayInstallRootCreated')
            }
        )) {
        $calls = @($installTransactionAst.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.CommandAst] -and
                    $node.GetCommandName() -eq $callContract.Name
                }, $true))
        Assert-Equal -Actual $calls.Count -Expected 1 -Message "$($callContract.Name) is called exactly once by the install transaction"
        $parameterNames = @(
            $calls[0].CommandElements |
                Where-Object { $_ -is [System.Management.Automation.Language.CommandParameterAst] } |
                ForEach-Object { $_.ParameterName }
        )
        foreach ($requiredParameter in $callContract.Required) {
            Assert-True `
                -Condition ($parameterNames -contains $requiredParameter) `
                -Message "$($callContract.Name) binds gateway argument '$requiredParameter'"
        }
        foreach ($forbiddenParameter in $callContract.Forbidden) {
            Assert-True `
                -Condition ($parameterNames -notcontains $forbiddenParameter) `
                -Message "$($callContract.Name) does not silently ignore obsolete argument '$forbiddenParameter'"
        }
    }
    $copyFunctionAst = $installAst.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Copy-FileToBackup'
        }, $true)
    Assert-True -Condition ($null -ne $copyFunctionAst) -Message 'backup copy function is present for manifest regression coverage'
    Invoke-Expression $copyFunctionAst.Extent.Text
    $emptyManifest = New-Object System.Collections.ArrayList
    Copy-FileToBackup `
        -SourcePath (Join-Path $script:WorkRoot 'does-not-exist.txt') `
        -BackupFilesRoot (Join-Path $script:WorkRoot 'empty-manifest') `
        -RelativePath 'missing.txt' `
        -Manifest $emptyManifest
    Assert-Equal -Actual $emptyManifest.Count -Expected 0 -Message 'backup copy accepts an initially empty manifest'

    $safeBackupFilesAst = $installAst.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Get-SafeBackupFiles'
        }, $true)
    Assert-True -Condition ($null -ne $safeBackupFilesAst) -Message 'safe recursive backup enumeration is present'
    Invoke-Expression $safeBackupFilesAst.Extent.Text
    $backupTraversalRoot = Join-Path $script:WorkRoot 'backup-traversal'
    $backupTraversalOutside = Join-Path $script:WorkRoot 'backup-traversal-outside'
    $backupTraversalLink = Join-Path $backupTraversalRoot 'redirected'
    New-Item -ItemType Directory -Path $backupTraversalRoot -Force | Out-Null
    New-Item -ItemType Directory -Path $backupTraversalOutside -Force | Out-Null
    Write-TestFile -Path (Join-Path $backupTraversalOutside 'outside.txt') -Content 'outside'
    $backupTraversalSymlinkError = $null
    try {
        New-Item -ItemType SymbolicLink -Path $backupTraversalLink -Target $backupTraversalOutside -ErrorAction Stop | Out-Null
    } catch {
        $backupTraversalSymlinkError = $_.Exception
    }
    if ($null -eq $backupTraversalSymlinkError) {
        Assert-Throws -Action {
            @(Get-SafeBackupFiles `
                    -RootPath $backupTraversalRoot `
                    -Name 'the test backup tree')
        } -Message 'recursive backup enumeration rejects nested reparse points before traversal'
    }

    $copyCandidateFunctionAst = $installAst.Find({
            param($node)
            $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Copy-CandidateIntoInstall'
        }, $true)
    Assert-True -Condition ($null -ne $copyCandidateFunctionAst) -Message 'candidate copy function is present for source-ordering regression coverage'
    Invoke-Expression $copyCandidateFunctionAst.Extent.Text
    $candidateRoot = Join-Path $script:WorkRoot 'candidate-install'
    $candidateScriptRoot = Join-Path $script:WorkRoot 'candidate-sources'
    $candidatePayloadRoot = Join-Path $script:WorkRoot 'candidate-payload'
    $candidateUninstaller = Join-Path $script:WorkRoot 'candidate-uninstaller.exe'
    foreach ($name in (Get-RequiredPayloadFiles)) {
        Write-TestFile -Path (Join-Path $candidatePayloadRoot $name) -Content $name
    }
    Write-TestFile -Path (Join-Path $candidateScriptRoot 'InstallerSupport.ps1') -Content 'support'
    Write-TestFile -Path (Join-Path $candidateScriptRoot 'Install-Bhtune.ps1') -Content 'installer'
    Write-TestFile -Path $candidateUninstaller -Content 'uninstaller'
    $candidatePaths = [pscustomobject]@{
        InstallRoot          = $candidateRoot
        InstallerScriptRoot  = Join-Path $candidateRoot 'installer'
        UninstallerPath      = Join-Path $candidateRoot 'uninstall.exe'
    }
    $candidateStage = [pscustomobject]@{ PayloadRoot = $candidatePayloadRoot }
    $inRootUninstaller = Join-Path $candidateRoot 'uninstall-source.exe'
    Write-TestFile -Path (Join-Path $candidateRoot 'old-version.txt') -Content 'old'
    Write-TestFile -Path $inRootUninstaller -Content 'invalid source'
    $orderingError = $null
    try {
        Copy-CandidateIntoInstall `
            -Paths $candidatePaths `
            -Stage $candidateStage `
            -InstallerScriptRoot $candidateScriptRoot `
            -UninstallerSource $inRootUninstaller
    } catch {
        $orderingError = $_.Exception.Message
    }
    Assert-True -Condition ($orderingError -like '*must be outside the install root*') -Message 'candidate copy rejects an uninstaller source inside the replaceable install root'
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $candidateRoot 'old-version.txt') -PathType Leaf) -Message 'source-ordering rejection occurs before the old install root is removed'

    Copy-CandidateIntoInstall `
        -Paths $candidatePaths `
        -Stage $candidateStage `
        -InstallerScriptRoot $candidateScriptRoot `
        -UninstallerSource $candidateUninstaller
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $candidateRoot 'bhtune.exe') -PathType Leaf) -Message 'candidate payload is copied when all sources are outside the install root'
    Assert-True -Condition (Test-Path -LiteralPath (Join-Path $candidateRoot 'uninstall.exe') -PathType Leaf) -Message 'external uninstaller source is copied after candidate replacement'

    $metadata = Get-ExpectedUninstallMetadata -Version '1.2.3' -InstallRoot 'C:\Program Files\ByteHound\bhtune' -UninstallerPath 'C:\Program Files\ByteHound\bhtune\uninstall.exe'
    Assert-Equal -Actual $metadata.QuietUninstallString -Expected '"C:\Program Files\ByteHound\bhtune\uninstall.exe" /S' -Message 'quiet uninstall metadata is explicit'
    $metadataSnapshot = [pscustomobject]@{ Values = $metadata }
    Assert-True -Condition (Test-UninstallMetadata -Snapshot $metadataSnapshot -Version '1.2.3' -InstallRoot 'C:\Program Files\ByteHound\bhtune' -UninstallerPath 'C:\Program Files\ByteHound\bhtune\uninstall.exe') -Message 'complete uninstall metadata is accepted'
    $metadataSnapshot.Values['DisplayVersion'] = '9.9.9'
    Assert-True -Condition (-not (Test-UninstallMetadata -Snapshot $metadataSnapshot -Version '1.2.3' -InstallRoot 'C:\Program Files\ByteHound\bhtune' -UninstallerPath 'C:\Program Files\ByteHound\bhtune\uninstall.exe')) -Message 'mismatched uninstall metadata is rejected'

    $serviceSnapshot = [pscustomobject]@{
        Exists      = $true
        StartName   = 'NT AUTHORITY\LocalService'
        StartMode   = 'Auto'
        DisplayName = 'BHTune Server'
        PathName    = $expectedCommand
    }
    Assert-True -Condition (Test-OwnedServiceSnapshot -ServiceSnapshot $serviceSnapshot -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml') -Message 'complete owned service snapshots are accepted'
    $serviceSnapshot.StartMode = 'Manual'
    Assert-True -Condition (-not (Test-OwnedServiceSnapshot -ServiceSnapshot $serviceSnapshot -ExecutablePath 'C:\Program Files\ByteHound\bhtune\bhtune-server.exe' -ConfigPath 'C:\ProgramData\ByteHound\bhtune\bhtune.toml')) -Message 'non-automatic service snapshots are rejected'

    $expectedGatewayCommand = Get-ExpectedGatewayServiceCommandLine `
        -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
        -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
        -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs'
    Assert-True -Condition (Test-GatewayServiceCommandLine `
            -ActualPathName $expectedGatewayCommand `
            -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
            -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
            -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs') -Message 'exact gateway service command lines are accepted'
    Assert-True -Condition (-not (Test-GatewayServiceCommandLine `
                -ActualPathName ($expectedGatewayCommand + ' --extra') `
                -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
                -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
                -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs')) -Message 'unexpected gateway service arguments are rejected'
    $gatewayServiceSnapshot = [pscustomobject]@{
        Exists      = $true
        Name        = 'OpcdaBridgeGateway'
        State       = 'Stopped'
        ProcessId   = 0
        StartName   = 'LocalSystem'
        StartMode   = 'Auto'
        DisplayName = 'OPC DA Bridge Gateway'
        PathName    = $expectedGatewayCommand
        Description = $script:GatewayServiceDescription
    }
    Assert-True -Condition (Test-OwnedGatewayServiceSnapshot `
            -ServiceSnapshot $gatewayServiceSnapshot `
            -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
            -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
            -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs') -Message 'complete installer-owned gateway service snapshots are accepted'
    Assert-True -Condition (Test-InstallerCreatedGatewayServiceSnapshot `
            -ServiceSnapshot $gatewayServiceSnapshot `
            -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
            -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
            -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs') -Message 'gateway registrations created by the candidate are recognizable before description verification'
    $gatewayServiceSnapshot.Description = 'partial registration'
    Assert-True -Condition (Test-InstallerCreatedGatewayServiceSnapshot `
            -ServiceSnapshot $gatewayServiceSnapshot `
            -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
            -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
            -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs') -Message 'a partial candidate registration remains safely identifiable for cleanup'
    Assert-True -Condition (-not (Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $gatewayServiceSnapshot `
                -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
                -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
                -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs')) -Message 'a partial gateway registration is not accepted as fully owned'
    $gatewayServiceSnapshot.Description = $script:GatewayServiceDescription
    $gatewayServiceSnapshot.StartName = 'NT AUTHORITY\LocalService'
    Assert-True -Condition (-not (Test-OwnedGatewayServiceSnapshot `
                -ServiceSnapshot $gatewayServiceSnapshot `
                -ExecutablePath 'C:\Program Files\ByteHound\bhtune\gateway\opcda-bridge-gateway.exe' `
                -ConfigPath 'C:\ProgramData\ByteHound\bhtune\gateway\opcda-bridge-gateway.toml' `
                -LogDirectory 'C:\ProgramData\ByteHound\bhtune\gateway\logs')) -Message 'gateway services outside LocalSystem are rejected'

    foreach ($functionName in @(
            'Write-JsonFile',
            'Enter-InstallerTransactionLock',
            'Exit-InstallerTransactionLock',
            'Read-TransactionJournal',
            'Remove-TransactionJournal',
            'Test-CleanInstallRootContents',
            'Test-CleanStagingRootContents',
            'Remove-CleanInstallStagingRoots',
            'Recover-CleanInstallTransaction',
            'Recover-InterruptedTransaction',
            'Assert-InstallerOwnershipMarker',
            'Get-GatewayMarkerState',
            'Assert-InstallerMetadataForRecovery',
            'Assert-InstallerState',
            'Assert-DatabasePolicy',
            'Write-TransactionJournal',
            'Test-TransactionJournalPath',
            'Assert-TransactionJournalShape',
            'Ensure-InstallerGatewayConfig',
            'Remove-InstallerCreatedGatewayConfig',
            'Remove-InstallerCreatedConfig',
            'Get-TransactionPhaseRank',
            'Get-SafeBackupFiles',
            'Get-BackupAclState',
            'New-RollbackBackup',
            'Assert-RollbackStateAndManifest',
            'Resolve-RollbackManifestDestination',
            'Restore-RollbackBackup'
        )) {
        Import-InstallerFunction -ScriptAst $installAst -Name $functionName
    }

    $missingExternalConfig = "db = ""C:/external/missing/bhtune.db""`n"
    Write-TestFile -Path $paths.ConfigPath -Content $missingExternalConfig
    Assert-Throws -Action {
        Assert-DatabasePolicy -Paths $paths -IsUpgrade:$true -CustomDbBackupConfirmed:$false -PreservedData:$true
    } -Message 'preserved external database state requires an explicit backup acknowledgement'
    Assert-Throws -Action {
        Assert-DatabasePolicy -Paths $paths -IsUpgrade:$true -CustomDbBackupConfirmed:$true -PreservedData:$true
    } -Message 'confirmed external database state still requires an existing accessible database file'
    $externalDatabasePath = Join-Path $script:WorkRoot 'external\bhtune.db'
    Write-TestFile -Path $externalDatabasePath -Content 'SQLite acceptance fixture'
    $externalTomlPath = $externalDatabasePath.Replace('\', '/')
    Write-TestFile -Path $paths.ConfigPath -Content "db = ""$externalTomlPath""`n"
    $externalPolicy = Assert-DatabasePolicy `
        -Paths $paths `
        -IsUpgrade:$true `
        -CustomDbBackupConfirmed:$true `
        -PreservedData:$true
    Assert-Equal -Actual $externalPolicy.Policy -Expected 'External' -Message 'acknowledged preserved external database state is accepted'
    Assert-True -Condition (-not $externalPolicy.AutomaticBackup) -Message 'external database policy disables installer-managed automatic backup'
    $externalDirectoryPath = Join-Path $script:WorkRoot 'external\directory'
    New-Item -ItemType Directory -Path $externalDirectoryPath -Force | Out-Null
    Assert-Throws -Action {
        Assert-ExternalDatabaseReady -DatabasePath $externalDirectoryPath
    } -Message 'external database directories are rejected'

    $helperParseErrors = $null
    $helperParseTokens = $null
    $helperAst = [System.Management.Automation.Language.Parser]::ParseFile(
        $helperPath,
        [ref]$helperParseTokens,
        [ref]$helperParseErrors
    )
    Assert-Equal -Actual @($helperParseErrors).Count -Expected 0 -Message 'installer support script parses for ACL regression coverage'
    Import-InstallerFunction -ScriptAst $helperAst -Name 'Set-InstallerAcls'

    $diagnosticScriptPath = Join-Path $PSScriptRoot 'Run-NsisDiagnostic.ps1'
    $diagnosticParseErrors = $null
    $diagnosticParseTokens = $null
    $diagnosticAst = [System.Management.Automation.Language.Parser]::ParseFile(
        $diagnosticScriptPath,
        [ref]$diagnosticParseTokens,
        [ref]$diagnosticParseErrors
    )
    Assert-Equal -Actual @($diagnosticParseErrors).Count -Expected 0 -Message 'NSIS lifecycle diagnostic parses for command-scope regression coverage'
    $diagnosticFunctions = @(
        $diagnosticAst.FindAll({
                param($node)
                $node -is [System.Management.Automation.Language.FunctionDefinitionAst]
            }, $true) |
            ForEach-Object { $_.Name }
    )
    $windowsOnlyCommands = @(
        'Get-CimInstance',
        'Get-NetFirewallPortFilter',
        'Get-NetFirewallRule',
        'Get-ScheduledTask',
        'Get-ScheduledTaskInfo',
        'New-ScheduledTaskAction',
        'New-ScheduledTaskPrincipal',
        'New-Service',
        'Register-ScheduledTask',
        'Start-ScheduledTask',
        'Start-Service',
        'Stop-ScheduledTask',
        'Unregister-ScheduledTask'
    )
    $unresolvedDiagnosticCommands = @(
        $diagnosticAst.FindAll({
                param($node)
                $node -is [System.Management.Automation.Language.CommandAst]
            }, $true) |
            ForEach-Object { $_.GetCommandName() } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            Sort-Object -Unique |
            Where-Object {
                $_ -notin $diagnosticFunctions -and
                $_ -notin $windowsOnlyCommands -and
                $null -eq (Get-Command $_ -ErrorAction SilentlyContinue)
            }
    )
    Assert-Equal `
        -Actual $unresolvedDiagnosticCommands `
        -Expected @() `
        -Message 'NSIS lifecycle diagnostic calls only local, shared, built-in, or explicit Windows-only commands'

    $journalPaths = Get-InstallerPaths `
        -InstallRoot (Join-Path $script:WorkRoot 'journal\ProgramFiles\ByteHound\bhtune') `
        -ProgramDataRoot (Join-Path $script:WorkRoot 'journal\ProgramData\ByteHound\bhtune')
    # Keep synthetic recovery fixtures from observing a real host Start Menu shortcut.
    $journalPaths.ShortcutPath = Join-Path $script:WorkRoot 'journal\StartMenu\Programs\BHTune\BHTune.url'
    $journalRoot = $journalPaths.InstallerStateRoot
    $journalStageRoot = Join-Path $journalRoot 'candidate-123'
    $journalBackupRoot = Join-Path $journalPaths.RollbackRoot 'backup-123'
    $journal = [pscustomobject][ordered]@{
        SchemaVersion          = 2
        Mode                   = 'Install'
        Phase                  = 'Preflight'
        TransactionId          = ([guid]::NewGuid().ToString('D'))
        BackupRoot             = $journalBackupRoot
        StageRoot              = $journalStageRoot
        InstallRootWasPresent  = $false
        ConfigWasPresent       = $false
        ConfigCreationPending  = $false
        ExpectedConfigHash     = $null
        ConfigWasCreated       = $true
        CreatedConfigHash      = ('a' * 64)
        ServiceWasPresent      = $false
        ServiceWasRunning      = $false
        PathEntryWasPresent    = $false
        ShortcutWasPresent     = $false
        PathChangedByTransaction = $false
    }
    Assert-True -Condition (Test-TransactionJournalPath -Candidate $journalBackupRoot -Root $journalPaths.RollbackRoot) -Message 'journal backup paths stay inside the rollback root'
    Assert-True -Condition (Test-TransactionJournalPath -Candidate $journalStageRoot -Root $journalPaths.InstallerStateRoot) -Message 'journal staging paths stay inside the installer state root'
    $journalCanonicalInside = [System.IO.Path]::Combine($journalPaths.RollbackRoot, 'nested', '..', 'backup-123')
    Assert-True -Condition (Test-TransactionJournalPath -Candidate $journalCanonicalInside -Root $journalPaths.RollbackRoot) -Message 'journal paths are checked after canonicalization'
    $journalTraversal = [System.IO.Path]::Combine($journalPaths.RollbackRoot, '..', 'outside')
    Assert-True -Condition (-not (Test-TransactionJournalPath -Candidate $journalTraversal -Root $journalPaths.RollbackRoot)) -Message 'journal dot-segment traversal outside managed roots is rejected'
    Assert-True -Condition (-not (Test-TransactionJournalPath -Candidate ($journalPaths.RollbackRoot + '-outside') -Root $journalPaths.RollbackRoot)) -Message 'journal sibling-prefix paths are rejected'
    Assert-True -Condition (-not (Test-TransactionJournalPath -Candidate (Join-Path $script:WorkRoot 'outside') -Root $journalPaths.InstallerStateRoot)) -Message 'journal paths outside managed roots are rejected'
    $finalJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $finalJournal.BackupRoot = $journalPaths.RollbackRoot
    Assert-TransactionJournalShape -Paths $journalPaths -Journal $finalJournal
    $gatewayJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $gatewayJournal.SchemaVersion = 3
    foreach ($property in ([ordered]@{
            GatewayManaged                    = $true
            GatewayWasManaged                 = $false
            GatewayService                    = $null
            GatewayProgramDataRootWasPresent  = $false
            GatewayConfigWasPresent           = $false
            GatewayConfigCreationPending      = $true
            GatewayConfigWasCreated           = $false
            ExpectedGatewayConfigHash         = ('b' * 64)
            CreatedGatewayConfigHash          = $null
            GatewayServiceWasPresent          = $false
            GatewayServiceWasRunning          = $false
        }).GetEnumerator()) {
        $gatewayJournal | Add-Member -MemberType NoteProperty -Name $property.Key -Value $property.Value
    }
    Assert-TransactionJournalShape -Paths $journalPaths -Journal $gatewayJournal
    $missingGatewayJournalField = $gatewayJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $missingGatewayJournalField.PSObject.Properties.Remove('GatewayService')
    Assert-Throws -Action {
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $missingGatewayJournalField
    } -Message 'schema-v3 install journals require the prior gateway service snapshot field'
    $invalidGatewayJournalHash = $gatewayJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $invalidGatewayJournalHash.ExpectedGatewayConfigHash = 'not-a-sha256'
    Assert-Throws -Action {
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $invalidGatewayJournalHash
    } -Message 'schema-v3 install journals reject invalid gateway config hashes'
    $schema3UninstallJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $schema3UninstallJournal.SchemaVersion = 3
    $schema3UninstallJournal.Mode = 'Uninstall'
    $schema3UninstallJournal.Phase = 'Begin'
    Assert-Throws -Action {
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $schema3UninstallJournal
    } -Message 'schema-v3 uninstall journals require explicit gateway ownership'
    $schema3UninstallJournal | Add-Member -MemberType NoteProperty -Name GatewayManaged -Value $true
    Assert-TransactionJournalShape -Paths $journalPaths -Journal $schema3UninstallJournal
    Assert-Equal -Actual (Get-TransactionPhaseRank -Phase 'Preflight') -Expected 0 -Message 'journal phases have a stable ordering'
    Assert-Throws -Action { Get-TransactionPhaseRank -Phase 'not-a-phase' } -Message 'unknown journal phases are rejected'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.Phase = 'not-a-phase'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'invalid install journal phases fail closed'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.TransactionId = 'not-a-guid'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'invalid transaction identifiers fail closed'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.StageRoot = Join-Path $script:WorkRoot 'outside\candidate-123'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'staging paths outside installer state fail closed'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.ConfigWasCreated = 'maybe'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'non-boolean journal fields fail closed'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.ConfigCreationPending = 'maybe'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'config creation pending requires a boolean journal field'
    Assert-Throws -Action {
        $badJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $badJournal.ExpectedConfigHash = 'not-a-sha256'
        Assert-TransactionJournalShape -Paths $journalPaths -Journal $badJournal
    } -Message 'expected config hashes require 64 hexadecimal characters'
    $legacyJournal = $journal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    foreach ($legacyProperty in @(
            'ConfigCreationPending',
            'ExpectedConfigHash',
            'ConfigWasCreated',
            'CreatedConfigHash'
        )) {
        $legacyJournal.PSObject.Properties.Remove($legacyProperty)
    }
    Assert-TransactionJournalShape -Paths $journalPaths -Journal $legacyJournal
    Write-TestFile -Path $journalPaths.ConfigPath -Content 'legacy operator configuration'
    Remove-InstallerCreatedConfig `
        -Paths $journalPaths `
        -ConfigWasPresent:$false
    Assert-True -Condition (Test-Path -LiteralPath $journalPaths.ConfigPath -PathType Leaf) -Message 'legacy journals without config creation fields preserve existing configuration'
    Remove-Item -LiteralPath $journalPaths.ConfigPath -Force

    $configJournalContent = "bind = ""127.0.0.1:8787""`n"
    $configJournalHash = Get-TextSha256 -Content $configJournalContent
    Write-TestFile -Path $journalPaths.ConfigPath -Content $configJournalContent
    Remove-InstallerCreatedConfig `
        -Paths $journalPaths `
        -ConfigWasPresent:$false `
        -ConfigCreationPending:$true `
        -ExpectedConfigHash $configJournalHash
    Assert-True -Condition (-not (Test-Path -LiteralPath $journalPaths.ConfigPath)) -Message 'pending installer-created configuration is removed when its hash matches'

    Write-TestFile -Path $journalPaths.ConfigPath -Content 'operator edit'
    Assert-Throws -Action {
        Remove-InstallerCreatedConfig `
            -Paths $journalPaths `
            -ConfigWasPresent:$false `
            -ConfigCreationPending:$true `
            -ExpectedConfigHash $configJournalHash
    } -Message 'modified pending installer-created configuration is preserved'
    Assert-Equal -Actual (Get-Content -LiteralPath $journalPaths.ConfigPath -Raw) -Expected 'operator edit' -Message 'modified pending configuration remains intact'
    Remove-Item -LiteralPath $journalPaths.ConfigPath -Force

    Write-TestFile -Path $journalPaths.ConfigPath -Content $configJournalContent
    Remove-InstallerCreatedConfig `
        -Paths $journalPaths `
        -ConfigWasPresent:$false `
        -ConfigWasCreated:$true `
        -CreatedConfigHash $configJournalHash
    Assert-True -Condition (-not (Test-Path -LiteralPath $journalPaths.ConfigPath)) -Message 'completed installer-created configuration is removed from its recorded hash'

    Write-TestFile -Path $journalPaths.ConfigPath -Content 'pre-existing operator config'
    Remove-InstallerCreatedConfig `
        -Paths $journalPaths `
        -ConfigWasPresent:$true `
        -ConfigWasCreated:$true `
        -CreatedConfigHash $configJournalHash
    Assert-Equal -Actual (Get-Content -LiteralPath $journalPaths.ConfigPath -Raw) -Expected 'pre-existing operator config' -Message 'pre-existing configuration is never removed'
    Remove-Item -LiteralPath $journalPaths.ConfigPath -Force

    $gatewayConfigJournalContent = Get-DefaultGatewayConfigContent `
        -DatabasePath $journalPaths.GatewayDatabasePath `
        -LogDirectory $journalPaths.GatewayLogDirectory
    $gatewayConfigJournalHash = Get-TextSha256 -Content $gatewayConfigJournalContent
    Assert-True -Condition (Ensure-InstallerGatewayConfig `
            -Paths $journalPaths `
            -DefaultContent $gatewayConfigJournalContent) -Message 'missing gateway configuration is created by the top-level installer helper'
    Assert-True -Condition (-not (Ensure-InstallerGatewayConfig -Paths $journalPaths)) -Message 'existing gateway configuration is preserved'
    Remove-InstallerCreatedGatewayConfig `
        -Paths $journalPaths `
        -ConfigWasPresent:$false `
        -ConfigWasCreated:$true `
        -CreatedConfigHash $gatewayConfigJournalHash
    Assert-True -Condition (-not (Test-Path -LiteralPath $journalPaths.GatewayConfigPath)) -Message 'installer-created gateway configuration is removed when its hash matches'
    Write-TestFile -Path $journalPaths.GatewayConfigPath -Content 'operator gateway edit'
    Assert-Throws -Action {
        Remove-InstallerCreatedGatewayConfig `
            -Paths $journalPaths `
            -ConfigWasPresent:$false `
            -ConfigWasCreated:$true `
            -CreatedConfigHash $gatewayConfigJournalHash
    } -Message 'modified installer-created gateway configuration is preserved'
    Assert-Equal -Actual (Get-Content -LiteralPath $journalPaths.GatewayConfigPath -Raw) -Expected 'operator gateway edit' -Message 'modified gateway configuration remains intact'
    Remove-Item -LiteralPath $journalPaths.GatewayConfigPath -Force

    Write-TransactionJournal -Paths $journalPaths -Value $journal
    $journalPath = Join-Path $journalRoot 'transaction.json'
    $script:JournalPathForMock = $journalPath
    Assert-True -Condition (Test-Path -LiteralPath $journalPath -PathType Leaf) -Message 'transaction journal is written'
    Assert-True -Condition (-not (Test-Path -LiteralPath ($journalPath + '.tmp'))) -Message 'journal temporary file is not left behind after commit'
    $readJournal = Read-TransactionJournal -Paths $journalPaths
    Assert-Equal -Actual $readJournal.Phase -Expected 'Preflight' -Message 'written journal can be read and validated'
    $journal.Phase = 'PathSnapshotted'
    Write-TransactionJournal -Paths $journalPaths -Value $journal
    Assert-Equal -Actual (Read-TransactionJournal -Paths $journalPaths).Phase -Expected 'PathSnapshotted' -Message 'journal replacement is atomic and repeatable'
    Write-TestFile -Path $journalPath -Content '{not valid json'
    Assert-Throws -Action { Read-TransactionJournal -Paths $journalPaths } -Message 'corrupt journals fail closed'
    Write-TransactionJournal -Paths $journalPaths -Value $journal

    $lock = Enter-InstallerTransactionLock -TimeoutSeconds 1
    try {
        Assert-Equal -Actual $lock.Name -Expected 'Global\ByteHound.BHTune.Installer' -Message 'installer transactions use a system-wide mutex'
    } finally {
        Exit-InstallerTransactionLock -Lock $lock
    }
    $lockAgain = Enter-InstallerTransactionLock -TimeoutSeconds 1
    Exit-InstallerTransactionLock -Lock $lockAgain
    Assert-True -Condition $true -Message 'released installer mutex permits a subsequent transaction'

    $script:MockMarker = $null
    $script:MockUninstall = $null
    $script:MockService = $null
    $script:MockServiceRegistration = $null
    $script:MockGatewayService = $null
    $script:MockGatewayServiceRegistration = $null
    function Get-RegistrySnapshot {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Path
        )

        if ($Path -eq $journalPaths.MarkerPath) { return $script:MockMarker }
        if ($Path -eq $journalPaths.UninstallKeyPath) { return $script:MockUninstall }
        return $null
    }
    function Get-ServiceSnapshot {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Name
        )

        if ($Name -eq $journalPaths.GatewayServiceName) {
            return $script:MockGatewayService
        }
        return $script:MockService
    }
    function Invoke-ServiceRegistrationQuery {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Name
        )

        if ($Name -eq $journalPaths.GatewayServiceName) {
            if ($null -ne $script:MockGatewayServiceRegistration) {
                return [bool]$script:MockGatewayServiceRegistration
            }
            return $null -ne $script:MockGatewayService
        }
        if ($null -ne $script:MockServiceRegistration) {
            return [bool]$script:MockServiceRegistration
        }
        return $null -ne $script:MockService
    }
    $script:MockGatewayServiceRegistration = $true
    $hiddenGatewayState = Assert-InstallerState -Paths $journalPaths
    Assert-True -Condition $hiddenGatewayState.GatewayServiceExists -Message 'clean-state inspection records a gateway SCM registration that WMI cannot inspect'
    $hiddenGatewayPaths = Get-InstallerPaths `
        -InstallRoot (Join-Path $script:WorkRoot 'hidden-gateway\ProgramFiles\ByteHound\bhtune') `
        -ProgramDataRoot (Join-Path $script:WorkRoot 'hidden-gateway\ProgramData\ByteHound\bhtune')
    Write-TestFile -Path $hiddenGatewayPaths.GatewayExecutable -Content 'gateway fixture'
    $script:CapturedGatewayInstallCalls = 0
    function Invoke-CapturedProcess {
        param(
            [string]$FilePath,
            [string]$Arguments,
            [int]$TimeoutMilliseconds
        )
        $script:CapturedGatewayInstallCalls++
        return [pscustomobject]@{
            ExitCode = 0
            StdOut   = ''
            StdErr   = ''
        }
    }
    Assert-Throws -Action {
        New-InstallerGatewayService -Paths $hiddenGatewayPaths
    } -Message 'gateway service creation rejects an existing SCM registration when WMI inspection is unavailable'
    Assert-Equal -Actual $script:CapturedGatewayInstallCalls -Expected 0 -Message 'hidden gateway registration rejection occurs before invoking the upstream installer'
    $script:MockGatewayServiceRegistration = $null
    $script:RemovedRegistryKeys = @()
    $script:JournalPresentDuringMetadataRemoval = @()
    function Remove-RegistryKey {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Path
        )

        $script:RemovedRegistryKeys += $Path
        if ($Path -eq $journalPaths.MarkerPath -or $Path -eq $journalPaths.UninstallKeyPath) {
            $script:JournalPresentDuringMetadataRemoval += (Test-Path -LiteralPath $script:JournalPathForMock -PathType Leaf)
        }
        if ($Path -eq $journalPaths.MarkerPath) {
            $script:MockMarker = $null
        } elseif ($Path -eq $journalPaths.UninstallKeyPath) {
            $script:MockUninstall = $null
        }
    }

    $ownedMarkerValues = [ordered]@{
        SchemaVersion = 2
        InstallerOwned = 1
        InstallDir = $journalPaths.InstallRoot
        Version = '1.2.3'
        ServiceName = $journalPaths.ServiceName
        ConfigPath = $journalPaths.ConfigPath
        DatabasePath = $journalPaths.DatabasePath
        PathEntry = $journalPaths.InstallRoot
        PathManaged = 0
        ShortcutPath = $journalPaths.ShortcutPath
    }
    $legacyGatewayMarkerState = Get-GatewayMarkerState `
        -Paths $journalPaths `
        -Marker ([pscustomobject]@{ Values = $ownedMarkerValues })
    Assert-True -Condition (-not $legacyGatewayMarkerState.Managed) -Message 'schema-v2 ownership markers preserve gateway absence'

    $schema3MarkerValues = [ordered]@{}
    foreach ($entry in $ownedMarkerValues.GetEnumerator()) {
        $schema3MarkerValues[$entry.Key] = $entry.Value
    }
    $schema3MarkerValues.SchemaVersion = 3
    $schema3MarkerValues.GatewayManaged = 0
    $schema3MarkerValues.GatewayVersion = ''
    $schema3MarkerValues.GatewaySha256 = ''
    $schema3MarkerValues.GatewayExecutable = $journalPaths.GatewayExecutable
    $schema3MarkerValues.GatewayConfigPath = $journalPaths.GatewayConfigPath
    $schema3MarkerValues.GatewayServiceName = $journalPaths.GatewayServiceName
    $schema3MarkerValues.GatewayPort = $journalPaths.GatewayPort
    $schema3GatewayMarkerState = Get-GatewayMarkerState `
        -Paths $journalPaths `
        -Marker ([pscustomobject]@{ Values = $schema3MarkerValues })
    Assert-True -Condition (-not $schema3GatewayMarkerState.Managed) -Message 'schema-v3 ownership markers explicitly preserve gateway absence'
    $schema3MarkerValues.GatewayManaged = 1
    $schema3MarkerValues.GatewayVersion = '0.5.9'
    $schema3MarkerValues.GatewaySha256 = 'a' * 64
    $schema3GatewayMarkerState = Get-GatewayMarkerState `
        -Paths $journalPaths `
        -Marker ([pscustomobject]@{ Values = $schema3MarkerValues })
    Assert-True -Condition $schema3GatewayMarkerState.Managed -Message 'schema-v3 ownership markers record an installer-managed gateway'
    Assert-Equal -Actual $schema3GatewayMarkerState.Version -Expected '0.5.9' -Message 'schema-v3 gateway version metadata round-trips'
    $schema3MarkerValues.GatewayManaged = 0
    Assert-Throws -Action {
        Get-GatewayMarkerState `
            -Paths $journalPaths `
            -Marker ([pscustomobject]@{ Values = $schema3MarkerValues })
    } -Message 'disabled schema-v3 gateway markers reject retained version and hash identity'
    $schema3MarkerValues.GatewayVersion = ''
    $schema3MarkerValues.GatewaySha256 = ''
    $schema3MarkerValues.GatewayExecutable = Join-Path $script:WorkRoot 'other\gateway.exe'
    Assert-Throws -Action {
        Get-GatewayMarkerState `
            -Paths $journalPaths `
            -Marker ([pscustomobject]@{ Values = $schema3MarkerValues })
    } -Message 'schema-v3 gateway markers reject noncanonical fixed paths even when management is disabled'
    $schema3MarkerValues.GatewayExecutable = $journalPaths.GatewayExecutable

    $script:MockMarker = $null
    $script:MockUninstall = $null
    $script:MockService = $null
    $script:MockGatewayService = $null
    $cleanGatewayJournal = $gatewayJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $cleanGatewayJournal.Mode = 'Install'
    $cleanGatewayJournal.Phase = 'Preflight'
    $cleanGatewayJournal.BackupRoot = ''
    $cleanGatewayJournal.GatewayProgramDataRootWasPresent = $false
    $cleanGatewayJournal.GatewayConfigWasPresent = $false
    $cleanGatewayJournal.GatewayConfigCreationPending = $false
    $cleanGatewayJournal.GatewayConfigWasCreated = $true
    $cleanGatewayJournal.ExpectedGatewayConfigHash = $gatewayConfigJournalHash
    $cleanGatewayJournal.CreatedGatewayConfigHash = $gatewayConfigJournalHash
    Write-TestFile -Path $journalPaths.GatewayConfigPath -Content $gatewayConfigJournalContent
    Write-TestFile -Path (Join-Path $journalPaths.GatewayDataDirectory 'candidate-index.sqlite3') -Content 'candidate index'
    Write-TransactionJournal -Paths $journalPaths -Value $cleanGatewayJournal
    $cleanGatewayRecovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $cleanGatewayRecovery.Action -Expected 'Discarded' -Message 'interrupted clean gateway installs are discarded before backup promotion'
    Assert-True -Condition (-not (Test-Path -LiteralPath $journalPaths.GatewayProgramDataRoot)) -Message 'clean recovery removes only the transaction-created gateway ProgramData root'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $journalPaths.InstallerStateRoot 'transaction.json'))) -Message 'clean gateway recovery removes its transaction journal'

    $script:StartedGatewayServices = 0
    $script:GatewaySmokeChecks = 0
    function Start-InstallerGatewayService {
        param([psobject]$Paths)
        $script:StartedGatewayServices++
        return [pscustomobject]@{ State = 'Running' }
    }
    function Invoke-GatewaySmokeCheck {
        param([psobject]$Paths)
        $script:GatewaySmokeChecks++
        return [pscustomobject]@{ application_version = '0.5.9' }
    }
    $journalGatewayCommand = Get-ExpectedGatewayServiceCommandLine `
        -ExecutablePath $journalPaths.GatewayExecutable `
        -ConfigPath $journalPaths.GatewayConfigPath `
        -LogDirectory $journalPaths.GatewayLogDirectory
    $script:MockGatewayService = [pscustomobject]@{
        Exists      = $true
        Name        = $journalPaths.GatewayServiceName
        State       = 'Stopped'
        ProcessId   = 0
        StartName   = 'LocalSystem'
        StartMode   = 'Auto'
        DisplayName = $script:GatewayServiceDisplayName
        PathName    = $journalGatewayCommand
        Description = $script:GatewayServiceDescription
    }
    $earlyGatewayUpgradeJournal = $gatewayJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $earlyGatewayUpgradeJournal.Mode = 'Upgrade'
    $earlyGatewayUpgradeJournal.Phase = 'ServiceStopped'
    $earlyGatewayUpgradeJournal.BackupRoot = ''
    $earlyGatewayUpgradeJournal.InstallRootWasPresent = $true
    $earlyGatewayUpgradeJournal.ConfigWasPresent = $true
    $earlyGatewayUpgradeJournal.ConfigWasCreated = $false
    $earlyGatewayUpgradeJournal.GatewayManaged = $true
    $earlyGatewayUpgradeJournal.GatewayWasManaged = $true
    $earlyGatewayUpgradeJournal.GatewayProgramDataRootWasPresent = $true
    $earlyGatewayUpgradeJournal.GatewayConfigWasPresent = $true
    $earlyGatewayUpgradeJournal.GatewayConfigCreationPending = $false
    $earlyGatewayUpgradeJournal.GatewayConfigWasCreated = $false
    $earlyGatewayUpgradeJournal.ExpectedGatewayConfigHash = $null
    $earlyGatewayUpgradeJournal.CreatedGatewayConfigHash = $null
    $earlyGatewayUpgradeJournal.GatewayServiceWasPresent = $true
    $earlyGatewayUpgradeJournal.GatewayServiceWasRunning = $true
    Write-TransactionJournal -Paths $journalPaths -Value $earlyGatewayUpgradeJournal
    $earlyGatewayRecovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $earlyGatewayRecovery.Action -Expected 'Discarded' -Message 'early interrupted upgrades restore the managed gateway without requiring a rollback snapshot'
    Assert-Equal -Actual $script:StartedGatewayServices -Expected 1 -Message 'early upgrade recovery restarts a previously running managed gateway'
    Assert-Equal -Actual $script:GatewaySmokeChecks -Expected 1 -Message 'early upgrade recovery smoke-checks the restarted gateway'

    $managedSchema3MarkerValues = [ordered]@{}
    foreach ($entry in $schema3MarkerValues.GetEnumerator()) {
        $managedSchema3MarkerValues[$entry.Key] = $entry.Value
    }
    $managedSchema3MarkerValues.GatewayManaged = 1
    $managedSchema3MarkerValues.GatewayVersion = '0.5.9'
    $managedSchema3MarkerValues.GatewaySha256 = 'a' * 64
    $script:MockMarker = [pscustomobject]@{ Values = $managedSchema3MarkerValues }
    $script:MockUninstall = [pscustomobject]@{
        Values = (Get-ExpectedUninstallMetadata `
                -Version '1.2.3' `
                -InstallRoot $journalPaths.InstallRoot `
                -UninstallerPath $journalPaths.UninstallerPath)
    }
    $script:MockService = [pscustomobject]@{
        Exists      = $true
        State       = 'Stopped'
        StartName   = 'NT AUTHORITY\LocalService'
        StartMode   = 'Auto'
        DisplayName = 'BHTune Server'
        PathName    = (Get-ExpectedServiceCommandLine `
                -ExecutablePath $journalPaths.ServiceExecutable `
                -ConfigPath $journalPaths.ConfigPath)
    }
    $script:MockServiceRegistration = $true
    $script:MockGatewayServiceRegistration = $true
    $gatewayUninstallJournal = [pscustomobject][ordered]@{
        SchemaVersion          = 3
        Mode                   = 'Uninstall'
        Phase                  = 'ServiceRemovePending'
        TransactionId          = ([guid]::NewGuid().ToString('D'))
        BackupRoot             = ''
        StageRoot              = ''
        Version                = '1.2.3'
        InstallRoot            = $journalPaths.InstallRoot
        UninstallerPath        = $journalPaths.UninstallerPath
        InstallRootWasPresent  = $true
        ConfigWasPresent       = $true
        ConfigWasCreated       = $false
        ServiceWasPresent      = $true
        ServiceWasRunning      = $false
        GatewayManaged         = $true
        PathEntryWasPresent    = $false
        ShortcutWasPresent     = $false
        PathChangedByTransaction = $false
    }
    Write-TransactionJournal -Paths $journalPaths -Value $gatewayUninstallJournal
    $gatewayUninstallRecovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $gatewayUninstallRecovery.Action -Expected 'ResumeUninstall' -Message 'interrupted uninstall recovery validates and retains both managed service removals'
    Assert-Equal -Actual (Read-TransactionJournal -Paths $journalPaths).Phase -Expected 'ServiceRemovePending' -Message 'both-service uninstall recovery leaves the idempotent removal phase ready to retry'
    Remove-TransactionJournal -Paths $journalPaths
    $script:MockService = $null
    $script:MockGatewayService = $null
    $script:MockGatewayServiceRegistration = $null
    $script:MockServiceRegistration = $null

    $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
    $script:MockUninstall = [pscustomobject]@{
        Values = (Get-ExpectedUninstallMetadata `
                -Version '1.2.3' `
                -InstallRoot $journalPaths.InstallRoot `
                -UninstallerPath $journalPaths.UninstallerPath)
    }
    $recoveryIdentity = [pscustomobject]@{
        Version          = '1.2.3'
        InstallRoot      = $journalPaths.InstallRoot
        UninstallerPath  = $journalPaths.UninstallerPath
    }
    $metadataState = Assert-InstallerMetadataForRecovery `
        -Paths $journalPaths `
        -Journal $recoveryIdentity `
        -Marker $script:MockMarker `
        -Uninstall $script:MockUninstall `
        -RequireJournalIdentity:$true
    Assert-Equal -Actual $metadataState.Version -Expected '1.2.3' -Message 'complete metadata recovery accepts consistent marker and uninstall metadata'
    $script:MockUninstall = $null
    $metadataState = Assert-InstallerMetadataForRecovery `
        -Paths $journalPaths `
        -Journal $recoveryIdentity `
        -Marker $script:MockMarker `
        -RequireJournalIdentity:$true
    Assert-Equal -Actual $metadataState.Version -Expected '1.2.3' -Message 'marker-only recovery accepts the ownership marker'
    $script:MockMarker = $null
    $script:MockUninstall = [pscustomobject]@{
        Values = (Get-ExpectedUninstallMetadata `
                -Version '1.2.3' `
                -InstallRoot $journalPaths.InstallRoot `
                -UninstallerPath $journalPaths.UninstallerPath)
    }
    $metadataState = Assert-InstallerMetadataForRecovery `
        -Paths $journalPaths `
        -Journal $recoveryIdentity `
        -Uninstall $script:MockUninstall `
        -RequireJournalIdentity:$true
    Assert-Equal -Actual $metadataState.Version -Expected '1.2.3' -Message 'uninstall-only recovery accepts journaled identity'
    Assert-Throws -Action {
        Assert-InstallerMetadataForRecovery `
            -Paths $journalPaths `
            -Journal ([pscustomobject]@{ Version = '1.2.3'; InstallRoot = $journalPaths.InstallRoot }) `
            -Uninstall $script:MockUninstall `
            -RequireJournalIdentity:$true
    } -Message 'uninstall-only recovery rejects incomplete journal identity'
    Assert-Throws -Action {
        Assert-InstallerMetadataForRecovery `
            -Paths $journalPaths `
            -Journal $recoveryIdentity `
            -Uninstall ([pscustomobject]@{
                Values = (Get-ExpectedUninstallMetadata `
                        -Version '9.9.9' `
                        -InstallRoot $journalPaths.InstallRoot `
                        -UninstallerPath $journalPaths.UninstallerPath)
            }) `
            -RequireJournalIdentity:$true
    } -Message 'recovery rejects contradictory uninstall metadata'
    Assert-Throws -Action {
        Assert-InstallerMetadataForRecovery `
            -Paths $journalPaths `
            -Journal $recoveryIdentity `
            -Marker ([pscustomobject]@{
                Values = [ordered]@{
                    SchemaVersion = 2
                    InstallerOwned = 1
                }
            }) `
            -RequireJournalIdentity:$true
    } -Message 'recovery rejects malformed remaining ownership metadata'
    Assert-Throws -Action {
        Assert-InstallerMetadataForRecovery `
            -Paths $journalPaths `
            -Journal $recoveryIdentity `
            -RequireJournalIdentity:$true
    } -Message 'recovery rejects a journal with no remaining metadata'
    $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
    $script:MockUninstall = [pscustomobject]@{
        Values = (Get-ExpectedUninstallMetadata `
                -Version '1.2.3' `
                -InstallRoot $journalPaths.InstallRoot `
                -UninstallerPath $journalPaths.UninstallerPath)
    }
    Assert-Throws -Action {
        Assert-InstallerState -Paths $journalPaths
    } -Message 'missing owned service or installation root cannot be treated as a clean install'
    Assert-Throws -Action {
        Assert-InstallerState -Paths $journalPaths -AllowMissingInstallRoot:$true
    } -Message 'missing owned service still requires an explicit retry state'
    $script:MockServiceRegistration = $true
    Assert-Throws -Action {
        Assert-InstallerState `
            -Paths $journalPaths `
            -AllowMissingService:$true `
            -AllowMissingInstallRoot:$true
    } -Message 'owned service registration cannot be treated as absent when WMI inspection is unavailable'
    $script:MockServiceRegistration = $null
    $ownedMissingState = Assert-InstallerState `
        -Paths $journalPaths `
        -AllowMissingService:$true `
        -AllowMissingInstallRoot:$true
    Assert-True -Condition $ownedMissingState.IsUpgrade -Message 'owned state with missing service and root is recognized for repair'
    Assert-True -Condition $ownedMissingState.ServiceMissing -Message 'owned missing service state is reported explicitly'

    $uninstallJournal = [pscustomobject][ordered]@{
        SchemaVersion = 2
        Mode = 'Uninstall'
        Phase = 'ServiceRemoved'
        TransactionId = ([guid]::NewGuid().ToString('D'))
        BackupRoot = ''
        StageRoot = ''
        Version = ''
        InstallRoot = $journalPaths.InstallRoot
        UninstallerPath = $journalPaths.UninstallerPath
        InstallRootWasPresent = $true
        ConfigWasPresent = $true
        ConfigWasCreated = $false
        ServiceWasPresent = $true
        ServiceWasRunning = $false
        PathEntryWasPresent = $false
        ShortcutWasPresent = $false
        PathChangedByTransaction = $false
    }
    Write-TransactionJournal -Paths $journalPaths -Value $uninstallJournal
    $recovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $recovery.Action -Expected 'ResumeUninstall' -Message 'uninstall with a missing service can be retried from retained journal state'

    $script:MockService = $null
    $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
    $script:MockUninstall = $null
    $script:RemovedRegistryKeys = @()
    $metadataJournal = $uninstallJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $metadataJournal.Phase = 'MetadataRemovePending'
    $metadataJournal.Version = '1.2.3'
    $metadataJournal.InstallRoot = $journalPaths.InstallRoot
    $metadataJournal.UninstallerPath = $journalPaths.UninstallerPath
    Write-TransactionJournal -Paths $journalPaths -Value $metadataJournal
    $metadataRecovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $metadataRecovery.Action -Expected 'ResumeUninstall' -Message 'ordinary metadata recovery preserves the uninstall transaction for explicit finalization'
    Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @() -Message 'ordinary metadata recovery does not remove the remaining marker key'
    $metadataRecovery = Recover-InterruptedTransaction -Paths $journalPaths -FinalizeUninstall:$true
    Assert-Equal -Actual $metadataRecovery.Action -Expected 'Finalized' -Message 'explicit finalization removes the remaining marker key'
    Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @($journalPaths.MarkerPath) -Message 'metadata recovery removes only the remaining marker key'
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $journalPaths.InstallerStateRoot 'transaction.json') -PathType Leaf)) -Message 'completed metadata recovery removes the transaction journal'

    $script:MockMarker = $null
    $script:MockUninstall = [pscustomobject]@{
        Values = (Get-ExpectedUninstallMetadata `
                -Version '1.2.3' `
                -InstallRoot $journalPaths.InstallRoot `
                -UninstallerPath $journalPaths.UninstallerPath)
    }
    $script:RemovedRegistryKeys = @()
    $metadataJournal = $uninstallJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $metadataJournal.Phase = 'MetadataRemovePending'
    $metadataJournal.Version = '1.2.3'
    $metadataJournal.InstallRoot = $journalPaths.InstallRoot
    $metadataJournal.UninstallerPath = $journalPaths.UninstallerPath
    Write-TransactionJournal -Paths $journalPaths -Value $metadataJournal
    $metadataRecovery = Recover-InterruptedTransaction -Paths $journalPaths
    Assert-Equal -Actual $metadataRecovery.Action -Expected 'ResumeUninstall' -Message 'ordinary metadata recovery preserves the uninstall transaction for explicit uninstall-entry finalization'
    Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @() -Message 'ordinary metadata recovery does not remove the remaining uninstall key'
    $metadataRecovery = Recover-InterruptedTransaction -Paths $journalPaths -FinalizeUninstall:$true
    Assert-Equal -Actual $metadataRecovery.Action -Expected 'Finalized' -Message 'explicit finalization removes the remaining uninstall key'
    Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @($journalPaths.UninstallKeyPath) -Message 'metadata recovery removes only the remaining uninstall key'

    function New-TestUninstallJournal {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Phase
        )

        $testJournal = $uninstallJournal | ConvertTo-Json -Depth 8 | ConvertFrom-Json
        $testJournal.Phase = $Phase
        $testJournal.Version = '1.2.3'
        $testJournal.InstallRoot = $journalPaths.InstallRoot
        $testJournal.UninstallerPath = $journalPaths.UninstallerPath
        return $testJournal
    }

    $uninstallRecoveryPhases = @(
        'ReadyForProgramFilesCleanup',
        'ProgramFilesRemovePending',
        'ProgramFilesRemoved',
        'MetadataRemovePending',
        'MetadataRemoved'
    )
    foreach ($phase in $uninstallRecoveryPhases) {
        if (Test-Path -LiteralPath $journalPaths.InstallRoot) {
            Remove-Item -LiteralPath $journalPaths.InstallRoot -Recurse -Force
        }
        $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
        $script:MockUninstall = [pscustomobject]@{
            Values = (Get-ExpectedUninstallMetadata `
                    -Version '1.2.3' `
                    -InstallRoot $journalPaths.InstallRoot `
                    -UninstallerPath $journalPaths.UninstallerPath)
        }
        $script:RemovedRegistryKeys = @()
        $script:JournalPresentDuringMetadataRemoval = @()
        Write-TransactionJournal -Paths $journalPaths -Value (New-TestUninstallJournal -Phase $phase)

        $ordinaryRecovery = Recover-InterruptedTransaction -Paths $journalPaths
        if ($phase -eq 'MetadataRemoved') {
            Assert-Equal -Actual $ordinaryRecovery.Action -Expected 'Finalized' -Message "ordinary recovery finalizes an already metadata-free '$phase' transaction"
        } else {
            Assert-Equal -Actual $ordinaryRecovery.Action -Expected 'ResumeUninstall' -Message "ordinary recovery resumes '$phase' without finalization"
        }
        Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @() -Message "ordinary recovery never removes ownership metadata for '$phase'"
        Assert-True -Condition ($null -ne $script:MockMarker) -Message "registry test state remains mock-owned after ordinary '$phase' recovery"
        if ($phase -ne 'MetadataRemoved') {
            Assert-True -Condition (Test-Path -LiteralPath (Join-Path $journalPaths.InstallerStateRoot 'transaction.json') -PathType Leaf) -Message "ordinary '$phase' recovery retains the journal for the uninstall retry"
        }
    }

    foreach ($phase in @(
            'ReadyForProgramFilesCleanup',
            'ProgramFilesRemovePending',
            'ProgramFilesRemoved',
            'MetadataRemovePending'
        )) {
        if (Test-Path -LiteralPath $journalPaths.InstallRoot) {
            Remove-Item -LiteralPath $journalPaths.InstallRoot -Recurse -Force
        }
        $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
        $script:MockUninstall = [pscustomobject]@{
            Values = (Get-ExpectedUninstallMetadata `
                    -Version '1.2.3' `
                    -InstallRoot $journalPaths.InstallRoot `
                    -UninstallerPath $journalPaths.UninstallerPath)
        }
        $script:RemovedRegistryKeys = @()
        $script:JournalPresentDuringMetadataRemoval = @()
        Write-TransactionJournal -Paths $journalPaths -Value (New-TestUninstallJournal -Phase $phase)

        $finalization = Recover-InterruptedTransaction -Paths $journalPaths -FinalizeUninstall:$true
        Assert-Equal -Actual $finalization.Action -Expected 'Finalized' -Message "explicit finalization completes '$phase' with no Program Files tree"
        Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @($journalPaths.MarkerPath, $journalPaths.UninstallKeyPath) -Message "finalization removes both ownership records for '$phase'"
        Assert-True -Condition (@($script:JournalPresentDuringMetadataRemoval | Where-Object { $_ -eq $false }).Count -eq 0) -Message "the journal remains present while metadata is removed for '$phase'"
        Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $journalPaths.InstallerStateRoot 'transaction.json') -PathType Leaf)) -Message "finalization removes the '$phase' journal last"
        Assert-Null -Actual (Recover-InterruptedTransaction -Paths $journalPaths) -Message "recovery retry is idempotent after finalizing '$phase'"
    }

    foreach ($phase in @(
            'ReadyForProgramFilesCleanup',
            'ProgramFilesRemoved',
            'MetadataRemovePending',
            'MetadataRemoved'
        )) {
        if (Test-Path -LiteralPath $journalPaths.InstallRoot) {
            Remove-Item -LiteralPath $journalPaths.InstallRoot -Recurse -Force
        }
        Write-TestFile -Path (Join-Path $journalPaths.InstallRoot 'bhtune-server.exe') -Content 'preserved Program Files content'
        $script:MockMarker = [pscustomobject]@{ Values = $ownedMarkerValues }
        $script:MockUninstall = [pscustomobject]@{
            Values = (Get-ExpectedUninstallMetadata `
                    -Version '1.2.3' `
                    -InstallRoot $journalPaths.InstallRoot `
                    -UninstallerPath $journalPaths.UninstallerPath)
        }
        $script:RemovedRegistryKeys = @()
        $script:JournalPresentDuringMetadataRemoval = @()
        Write-TransactionJournal -Paths $journalPaths -Value (New-TestUninstallJournal -Phase $phase)

        Assert-Throws -Action {
            Recover-InterruptedTransaction -Paths $journalPaths -FinalizeUninstall:$true
        } -Message "finalization refuses to remove metadata while Program Files remains for '$phase'"
        Assert-Equal -Actual @($script:RemovedRegistryKeys) -Expected @() -Message "guarded '$phase' finalization leaves ownership metadata untouched"
        Assert-True -Condition (Test-Path -LiteralPath (Join-Path $journalPaths.InstallerStateRoot 'transaction.json') -PathType Leaf) -Message "guarded '$phase' finalization retains the journal"
    }
    Remove-Item -LiteralPath $journalPaths.InstallRoot -Recurse -Force

    $restorePaths = Get-InstallerPaths `
        -InstallRoot (Join-Path $script:WorkRoot 'restore\ProgramFiles\ByteHound\bhtune') `
        -ProgramDataRoot (Join-Path $script:WorkRoot 'restore\ProgramData\ByteHound\bhtune')
    $restoreBackupRoot = Join-Path $restorePaths.RollbackRoot 'root-absent'
    $restoreConfigContent = "bind = ""127.0.0.1:8787""`ndb = ""$($restorePaths.DatabasePath.Replace('\', '/'))""`n"
    Write-TestFile -Path $restorePaths.ConfigPath -Content $restoreConfigContent
    $restoreBackupFile = Join-Path $restoreBackupRoot 'files\programdata\bhtune.toml'
    Write-TestFile -Path $restoreBackupFile -Content $restoreConfigContent
    $restoreManifestEntry = New-HashManifestEntry `
        -SourcePath $restorePaths.ConfigPath `
        -BackupPath $restoreBackupFile `
        -RelativePath 'programdata\bhtune.toml'
    $restoreManifest = [pscustomobject]@{
        SchemaVersion = 2
        Files         = @($restoreManifestEntry)
    }
    $restoreState = [pscustomobject]@{
        SchemaVersion          = 2
        Version                = '1.2.3'
        Service                = $null
        MarkerValues           = $null
        UninstallValues        = $null
        DatabasePolicy         = [pscustomobject]@{ Policy = 'Default' }
        ConfigPath             = $restorePaths.ConfigPath
        DatabasePath           = $restorePaths.DatabasePath
        ShortcutPath           = $restorePaths.ShortcutPath
        InstallRootWasPresent  = $false
        PathManaged            = $false
        PathEntryWasPresent    = $false
        Acls                   = [pscustomobject]@{}
    }
    Write-TestFile -Path (Join-Path $restoreBackupRoot 'state.json') -Content ($restoreState | ConvertTo-Json -Depth 12)
    Write-TestFile -Path (Join-Path $restoreBackupRoot 'manifest.json') -Content ($restoreManifest | ConvertTo-Json -Depth 12)
    Assert-True -Condition (Test-RollbackBackup -BackupRoot $restoreBackupRoot) -Message 'rollback with an absent prior install root is verifiable'
    Assert-RollbackStateAndManifest `
        -Paths $restorePaths `
        -BackupRoot $restoreBackupRoot `
        -State $restoreState `
        -Manifest $restoreManifest
    Assert-True -Condition $true -Message 'fixed rollback destinations and root-presence state are accepted'
    $badSourceManifest = $restoreManifest | ConvertTo-Json -Depth 12 | ConvertFrom-Json
    $badSourceManifest.Files[0].SourcePath = Join-Path $script:WorkRoot 'wrong-source.txt'
    Assert-Throws -Action {
        Assert-RollbackStateAndManifest `
            -Paths $restorePaths `
            -BackupRoot $restoreBackupRoot `
            -State $restoreState `
            -Manifest $badSourceManifest
    } -Message 'rollback manifests reject source paths that do not match fixed destinations'
    $badInstallManifest = [pscustomobject]@{
        SchemaVersion = 2
        Files = @([pscustomobject]@{
                RelativePath = 'install\bhtune.exe'
                SourcePath   = $restorePaths.CliExecutable
                BackupPath   = $restoreBackupFile
                Length       = 0
                Sha256       = '0' * 64
            })
    }
    Assert-Throws -Action {
        Assert-RollbackStateAndManifest `
            -Paths $restorePaths `
            -BackupRoot $restoreBackupRoot `
            -State $restoreState `
            -Manifest $badInstallManifest
    } -Message 'rollback manifests reject install files when the prior install root was absent'
    New-Item -ItemType Directory -Path $restorePaths.InstallRoot -Force | Out-Null
    Write-TestFile -Path (Join-Path $restorePaths.InstallRoot 'stale.txt') -Content 'stale'
    function Remove-RegistryKey {
        param([string]$Path)
    }
    function Set-RegistryValues {
        param([string]$Path, [psobject]$Values)
    }
    function Restore-AclSddl {
        param([string]$Path, [string]$Sddl)
    }
    function Remove-StartMenuShortcut {
        param([string]$ShortcutPath)
    }
    function Set-MachinePathSnapshot {
        param([AllowNull()][string]$Value)
    }
    function Restore-ServiceSnapshot {
        param([psobject]$Snapshot)
    }
    Restore-RollbackBackup -Paths $restorePaths -BackupRoot $restoreBackupRoot
    Assert-True -Condition (-not (Test-Path -LiteralPath $restorePaths.InstallRoot)) -Message 'rollback leaves a previously absent install root absent'
    Assert-Equal -Actual (Get-Content -LiteralPath $restorePaths.ConfigPath -Raw) -Expected $restoreConfigContent -Message 'rollback restores fixed ProgramData configuration'

    $gatewayRestorePaths = Get-InstallerPaths `
        -InstallRoot (Join-Path $script:WorkRoot 'gateway-restore\ProgramFiles\ByteHound\bhtune') `
        -ProgramDataRoot (Join-Path $script:WorkRoot 'gateway-restore\ProgramData\ByteHound\bhtune')
    $gatewayRestoreBackupRoot = Join-Path $gatewayRestorePaths.RollbackRoot 'gateway-data'
    $gatewayRestoreFilesRoot = Join-Path $gatewayRestoreBackupRoot 'files'
    $gatewayRestoreManifestEntries = New-Object System.Collections.ArrayList
    $gatewayRestoreContents = [ordered]@{
        'gatewaydata\opcda-bridge-gateway.toml' = "port = 7600`n"
        'gatewaydata\data\index.sqlite3'         = 'index'
        'gatewaydata\data\index.sqlite3-wal'     = 'wal'
        'gatewaydata\data\index.sqlite3-shm'     = 'shm'
        'gatewaydata\data\index.lock'            = 'lock'
        'gatewaydata\logs\gateway.log'           = 'log'
        'gatewaydata\evidence\rollback.json'     = 'evidence'
    }
    foreach ($entry in $gatewayRestoreContents.GetEnumerator()) {
        $destination = Resolve-RollbackManifestDestination `
            -Paths $gatewayRestorePaths `
            -RelativePath $entry.Key
        $backupFilePath = Join-Path $gatewayRestoreFilesRoot $entry.Key
        Write-TestFile -Path $destination -Content $entry.Value
        Write-TestFile -Path $backupFilePath -Content $entry.Value
        [void]$gatewayRestoreManifestEntries.Add((New-HashManifestEntry `
                    -SourcePath $destination `
                    -BackupPath $backupFilePath `
                    -RelativePath $entry.Key))
    }
    $gatewayRestoreManifest = [pscustomobject]@{
        SchemaVersion = 3
        Files         = @($gatewayRestoreManifestEntries)
    }
    $gatewayRestoreState = [pscustomobject]@{
        SchemaVersion                     = 3
        Version                           = '1.2.3'
        Service                           = $null
        MarkerValues                      = $null
        UninstallValues                   = $null
        DatabasePolicy                    = [pscustomobject]@{ Policy = 'External' }
        ConfigPath                        = $gatewayRestorePaths.ConfigPath
        DatabasePath                      = $gatewayRestorePaths.DatabasePath
        ShortcutPath                      = $gatewayRestorePaths.ShortcutPath
        InstallRootWasPresent             = $false
        GatewayManaged                    = $true
        GatewayWasManaged                 = $false
        GatewayService                    = $null
        GatewayProgramDataRootWasPresent  = $true
        PathManaged                       = $false
        PathEntryWasPresent               = $false
        Acls                              = [pscustomobject]@{}
    }
    Write-TestFile -Path (Join-Path $gatewayRestoreBackupRoot 'state.json') -Content ($gatewayRestoreState | ConvertTo-Json -Depth 12)
    Write-TestFile -Path (Join-Path $gatewayRestoreBackupRoot 'manifest.json') -Content ($gatewayRestoreManifest | ConvertTo-Json -Depth 12)
    Assert-True -Condition (Test-RollbackBackup -BackupRoot $gatewayRestoreBackupRoot) -Message 'gateway ProgramData rollback manifests are verifiable'
    Assert-RollbackStateAndManifest `
        -Paths $gatewayRestorePaths `
        -BackupRoot $gatewayRestoreBackupRoot `
        -State $gatewayRestoreState `
        -Manifest $gatewayRestoreManifest
    $gatewayNotManagedState = $gatewayRestoreState | ConvertTo-Json -Depth 12 | ConvertFrom-Json
    $gatewayNotManagedState.GatewayManaged = $false
    Assert-Throws -Action {
        Assert-RollbackStateAndManifest `
            -Paths $gatewayRestorePaths `
            -BackupRoot $gatewayRestoreBackupRoot `
            -State $gatewayNotManagedState `
            -Manifest $gatewayRestoreManifest
    } -Message 'gateway ProgramData rollback entries require a gateway-managed transaction'
    Remove-Item -LiteralPath $gatewayRestorePaths.GatewayProgramDataRoot -Recurse -Force
    Write-TestFile -Path (Join-Path $gatewayRestorePaths.GatewayProgramDataRoot 'candidate.txt') -Content 'candidate'
    Restore-RollbackBackup -Paths $gatewayRestorePaths -BackupRoot $gatewayRestoreBackupRoot
    Assert-True -Condition (-not (Test-Path -LiteralPath (Join-Path $gatewayRestorePaths.GatewayProgramDataRoot 'candidate.txt'))) -Message 'gateway rollback removes candidate ProgramData before restoration'
    foreach ($entry in $gatewayRestoreContents.GetEnumerator()) {
        $destination = Resolve-RollbackManifestDestination `
            -Paths $gatewayRestorePaths `
            -RelativePath $entry.Key
        Assert-Equal -Actual (Get-Content -LiteralPath $destination -Raw) -Expected $entry.Value -Message "gateway rollback restores '$($entry.Key)'"
    }

    $gatewayBackupPaths = Get-InstallerPaths `
        -InstallRoot (Join-Path $script:WorkRoot 'gateway-backup\ProgramFiles\ByteHound\bhtune') `
        -ProgramDataRoot (Join-Path $script:WorkRoot 'gateway-backup\ProgramData\ByteHound\bhtune')
    Write-TestFile -Path $gatewayBackupPaths.CliExecutable -Content 'bhtune'
    $gatewayBackupContents = [ordered]@{
        $gatewayBackupPaths.GatewayConfigPath                         = 'config'
        $gatewayBackupPaths.GatewayDatabasePath                       = 'index'
        ($gatewayBackupPaths.GatewayDatabasePath + '-wal')            = 'wal'
        ($gatewayBackupPaths.GatewayDatabasePath + '-shm')            = 'shm'
        (Join-Path $gatewayBackupPaths.GatewayDataDirectory 'build.lock') = 'lock'
        (Join-Path $gatewayBackupPaths.GatewayDataDirectory 'build-owner.json') = 'owner'
        (Join-Path $gatewayBackupPaths.GatewayLogDirectory 'gateway.log') = 'log'
        (Join-Path $gatewayBackupPaths.GatewayProgramDataRoot 'rollback-evidence.json') = 'evidence'
    }
    foreach ($entry in $gatewayBackupContents.GetEnumerator()) {
        Write-TestFile -Path $entry.Key -Content $entry.Value
    }
    function Get-BackupAclState {
        param([psobject]$Paths)
        return [ordered]@{}
    }
    function Get-MachinePathSnapshot {
        return ''
    }
    $gatewayBackupPriorState = [pscustomobject]@{
        Marker         = $null
        Uninstall      = $null
        Version        = '1.2.3'
        Service        = $null
        PathManaged    = $false
        GatewayManaged = $true
        GatewayService = $null
    }
    $gatewayBackup = New-RollbackBackup `
        -Paths $gatewayBackupPaths `
        -PriorState $gatewayBackupPriorState `
        -DatabasePolicy ([pscustomobject]@{ Policy = 'External' }) `
        -ConfigWasCreated:$true `
        -InstallRootWasPresent:$true `
        -ManageGateway:$true `
        -GatewayConfigWasCreated:$false `
        -GatewayProgramDataRootWasPresent:$true
    Assert-True -Condition (Test-RollbackBackup -BackupRoot $gatewayBackup.PendingRoot) -Message 'new combined rollback backups verify before promotion'
    $gatewayBackupManifest = Get-Content -LiteralPath (Join-Path $gatewayBackup.PendingRoot 'manifest.json') -Raw | ConvertFrom-Json
    $gatewayBackupRelativePaths = @($gatewayBackupManifest.Files | ForEach-Object { ([string]$_.RelativePath).Replace('/', '\') })
    Assert-Equal -Actual @($gatewayBackupRelativePaths | Where-Object { $_.StartsWith('gatewaydata\', [System.StringComparison]::OrdinalIgnoreCase) }).Count -Expected $gatewayBackupContents.Count -Message 'combined rollback captures every managed gateway ProgramData file'
    foreach ($requiredGatewayBackup in @(
            'gatewaydata\opcda-bridge-gateway.toml',
            'gatewaydata\data\index.sqlite3',
            'gatewaydata\data\index.sqlite3-wal',
            'gatewaydata\data\index.sqlite3-shm',
            'gatewaydata\data\build.lock',
            'gatewaydata\data\build-owner.json',
            'gatewaydata\logs\gateway.log',
            'gatewaydata\rollback-evidence.json'
        )) {
        Assert-True -Condition ($gatewayBackupRelativePaths -contains $requiredGatewayBackup) -Message "combined rollback contains '$requiredGatewayBackup'"
    }

    $script:AclCalls = New-Object System.Collections.ArrayList
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

        [void]$script:AclCalls.Add($Path)
    }
    Set-InstallerAcls -Paths $paths `
        -InstallRootCreated:$false `
        -DatabaseDirectoryCreated:$false `
        -LogDirectoryCreated:$false `
        -InstallerStateRootCreated:$false `
        -ConfigCreated:$false
    Assert-True -Condition (@($script:AclCalls | Where-Object { $_ -eq $paths.ProgramDataRoot }).Count -eq 1) -Message 'the fixed ProgramData root always receives the scoped ACL'
    Assert-True -Condition ((Get-InstallerAclTargets -Paths $paths) -contains $paths.ProgramDataRoot) -Message 'the fixed ProgramData root is included in rollback ACL targets'
    $script:AclCalls.Clear()
    Set-InstallerAcls -Paths $paths -ConfigCreated:$true
    Assert-True -Condition (@($script:AclCalls | Where-Object { $_ -eq $paths.ProgramDataRoot }).Count -eq 1) -Message 'the fixed ProgramData root remains secured when configuration is created'
    Assert-True -Condition (@($script:AclCalls | Where-Object { $_ -eq $paths.ConfigPath }).Count -eq 1) -Message 'new installer configuration receives its scoped ACL'
    $script:AclCalls.Clear()
    Set-InstallerAcls -Paths $paths -InstallRootCreated:$true
    Assert-True -Condition (@($script:AclCalls | Where-Object { $_ -eq $paths.InstallRoot }).Count -eq 1) -Message 'recreated install roots receive the scoped installer ACL'
    $operatorDescendant = Join-Path $paths.DatabaseDirectory 'operator-owned.db'
    Assert-True -Condition ((Get-InstallerAclTargets -Paths $paths) -notcontains $operatorDescendant) -Message 'operator-owned database descendants are outside ACL targets'
    foreach ($directory in @(
            $paths.GatewayInstallRoot,
            $paths.GatewayProgramDataRoot,
            $paths.GatewayDataDirectory,
            $paths.GatewayLogDirectory
        )) {
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
    }
    foreach ($path in @(
            $paths.GatewayExecutable,
            $paths.GatewayReleasePath,
            $paths.GatewayProvenancePath,
            $paths.GatewayLicensePath,
            $paths.GatewayNoticePath,
            $paths.GatewayConfigPath
        )) {
        Write-TestFile -Path $path -Content 'gateway ACL fixture'
    }
    $script:AclCalls.Clear()
    Set-InstallerAcls `
        -Paths $paths `
        -ManageGateway:$true `
        -GatewayProgramDataRootCreated:$true `
        -GatewayDataDirectoryCreated:$true `
        -GatewayLogDirectoryCreated:$true `
        -GatewayConfigCreated:$true
    foreach ($path in @(
            $paths.GatewayProgramDataRoot,
            $paths.GatewayDataDirectory,
            $paths.GatewayLogDirectory,
            $paths.GatewayConfigPath,
            $paths.GatewayInstallRoot,
            $paths.GatewayExecutable,
            $paths.GatewayReleasePath,
            $paths.GatewayProvenancePath,
            $paths.GatewayLicensePath,
            $paths.GatewayNoticePath
        )) {
        Assert-True -Condition (@($script:AclCalls | Where-Object { $_ -eq $path }).Count -eq 1) -Message "managed gateway ACL target '$path' is applied exactly once"
    }

    . $helperPath
    $script:WmiQueryResults = New-Object System.Collections.Queue
    [void]$script:WmiQueryResults.Enqueue('transient-error')
    [void]$script:WmiQueryResults.Enqueue([pscustomobject]@{
            Name        = 'BhtuneServer'
            State       = 'Running'
            StartMode   = 'Auto'
            StartName   = 'NT AUTHORITY\LocalService'
            PathName    = 'bhtune-server.exe --config bhtune.toml'
            DisplayName = 'BHTune Server'
            Description = 'BHTune HTTP API and embedded web GUI.'
        })
    $script:WmiQueryCalls = 0
    function Get-WmiObject {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Class,

            [Parameter(Mandatory = $true)]
            [string]$Filter
        )

        $script:WmiQueryCalls++
        $next = $script:WmiQueryResults.Dequeue()
        if ($next -eq 'transient-error') {
            throw 'temporary WMI query failure'
        }
        return $next
    }
    $retriedSnapshot = Get-ServiceSnapshot -Name 'BhtuneServer' -RetryCount 3 -RetryDelayMilliseconds 0
    Assert-Equal -Actual $script:WmiQueryCalls -Expected 2 -Message 'service snapshots retry transient WMI failures'
    Assert-Equal -Actual $retriedSnapshot.State -Expected 'Running' -Message 'service snapshots return the later WMI result'

    function Get-ServiceSnapshot {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Name
        )

        return $script:MockService
    }

    $script:ServiceQueryResults = New-Object System.Collections.Queue
    [void]$script:ServiceQueryResults.Enqueue($true)
    [void]$script:ServiceQueryResults.Enqueue($true)
    [void]$script:ServiceQueryResults.Enqueue($false)
    $script:ServiceQueryCalls = 0
    function Invoke-ServiceRegistrationQuery {
        param([string]$Name)
        $script:ServiceQueryCalls++
        return [bool]$script:ServiceQueryResults.Dequeue()
    }
    Wait-ServiceRegistrationGone -Name 'BhtuneServer' -TimeoutSeconds 2 -PollMilliseconds 0
    Assert-Equal -Actual $script:ServiceQueryCalls -Expected 3 -Message 'service removal waits for SCM registration to disappear'

    $script:ServiceQueryResults = New-Object System.Collections.Queue
    [void]$script:ServiceQueryResults.Enqueue('transient-error')
    [void]$script:ServiceQueryResults.Enqueue($false)
    $script:ServiceQueryCalls = 0
    function Invoke-ServiceRegistrationQuery {
        param([string]$Name)
        $script:ServiceQueryCalls++
        $next = $script:ServiceQueryResults.Dequeue()
        if ($next -eq 'transient-error') {
            throw 'temporary SCM query failure'
        }
        return [bool]$next
    }
    Wait-ServiceRegistrationGone -Name 'BhtuneServer' -TimeoutSeconds 2 -PollMilliseconds 0
    Assert-Equal -Actual $script:ServiceQueryCalls -Expected 2 -Message 'service removal retries transient SCM query failures'

    $script:ServiceQueryCalls = 0
    function Invoke-ServiceRegistrationQuery {
        param([string]$Name)
        $script:ServiceQueryCalls++
        return $true
    }
    $timeoutMessage = $null
    try {
        Wait-ServiceRegistrationGone -Name 'BhtuneServer' -TimeoutSeconds 1 -PollMilliseconds 25
    } catch {
        $timeoutMessage = $_.Exception.Message
    }
    Assert-True -Condition ($timeoutMessage -like "*did not disappear after 1 seconds*") -Message 'service removal times out when SCM registration remains'
    Assert-True -Condition ($script:ServiceQueryCalls -gt 0) -Message 'service removal polls SCM before timing out'

    $script:TcpListenerQueryUsedLocalPort = $false
    function Get-NetTCPConnection {
        param(
            [string]$State,
            [int]$LocalPort,
            [object]$ErrorAction
        )

        $script:TcpListenerQueryUsedLocalPort = $PSBoundParameters.ContainsKey('LocalPort')
        if ($script:TcpListenerQueryUsedLocalPort) {
            throw 'a LocalPort CIM filter reports no matching objects as an error'
        }
        return @(
            [pscustomobject]@{
                LocalAddress  = '127.0.0.1'
                LocalPort     = 5985
                OwningProcess = 1111
            },
            [pscustomobject]@{
                LocalAddress  = '0.0.0.0'
                LocalPort     = 7600
                OwningProcess = 4242
            }
        )
    }
    $filteredListeners = @(Get-TcpListenerSnapshots -Port 7600)
    Assert-True -Condition (-not $script:TcpListenerQueryUsedLocalPort) -Message 'TCP listener inspection avoids the no-match LocalPort CIM query'
    Assert-Equal -Actual $filteredListeners.Count -Expected 1 -Message 'TCP listener inspection filters the requested port in PowerShell'
    Assert-Equal -Actual $filteredListeners[0].OwningProcess -Expected 4242 -Message 'TCP listener inspection preserves the owning process'
    Remove-Item function:Get-NetTCPConnection

    $listenerService = [pscustomobject]@{
        Exists    = $true
        State     = 'Running'
        ProcessId = 4242
    }
    $script:ListenerSnapshots = @(
        [pscustomobject]@{
            LocalAddress  = '0.0.0.0'
            LocalPort     = 7600
            OwningProcess = 4242
        }
    )
    $script:GatewayProcessSnapshot = [pscustomobject]@{
        ProcessId      = 4242
        ExecutablePath = $paths.GatewayExecutable
        CommandLine    = 'gateway'
    }
    function Get-TcpListenerSnapshots {
        param([int]$Port)
        return @($script:ListenerSnapshots)
    }
    function Get-ProcessSnapshot {
        param([int]$ProcessId)
        return $script:GatewayProcessSnapshot
    }
    $listenerOwnership = Assert-GatewayListenerOwnership `
        -Paths $paths `
        -ServiceSnapshot $listenerService
    Assert-Equal -Actual $listenerOwnership.Process.ProcessId -Expected 4242 -Message 'gateway listener validation returns the exact service process'
    $script:ListenerSnapshots = @(
        [pscustomobject]@{
            LocalAddress  = '127.0.0.1'
            LocalPort     = 7600
            OwningProcess = 4242
        }
    )
    Assert-Throws -Action {
        Assert-GatewayListenerOwnership -Paths $paths -ServiceSnapshot $listenerService
    } -Message 'gateway listener validation rejects a listener that is not bound to all interfaces'
    $script:ListenerSnapshots = @(
        [pscustomobject]@{
            LocalAddress  = '0.0.0.0'
            LocalPort     = 7600
            OwningProcess = 4242
        },
        [pscustomobject]@{
            LocalAddress  = '127.0.0.1'
            LocalPort     = 7600
            OwningProcess = 4343
        }
    )
    Assert-Throws -Action {
        Assert-GatewayListenerOwnership -Paths $paths -ServiceSnapshot $listenerService
    } -Message 'gateway listener validation rejects any competing listener owner'
    $script:ListenerSnapshots = @(
        [pscustomobject]@{
            LocalAddress  = '0.0.0.0'
            LocalPort     = 7600
            OwningProcess = 4242
        }
    )
    $script:GatewayProcessSnapshot = [pscustomobject]@{
        ProcessId      = 4242
        ExecutablePath = Join-Path $script:WorkRoot 'unexpected\gateway.exe'
        CommandLine    = 'gateway'
    }
    Assert-Throws -Action {
        Assert-GatewayListenerOwnership -Paths $paths -ServiceSnapshot $listenerService
    } -Message 'gateway listener validation rejects the expected PID when its executable path differs'
    $script:ListenerSnapshots = @()
    Assert-True -Condition (Assert-GatewayPortAvailable -Port 7600) -Message 'an unused gateway port passes preflight'
    $script:ListenerSnapshots = @(
        [pscustomobject]@{
            LocalAddress  = '0.0.0.0'
            LocalPort     = 7600
            OwningProcess = 9999
        }
    )
    Assert-Throws -Action {
        Assert-GatewayPortAvailable -Port 7600
    } -Message 'a pre-existing gateway port owner fails preflight'

    Write-Host ("InstallerSupport self-tests passed: {0}" -f $script:Passed)
    exit 0
} finally {
    $env:ProgramData = $script:OriginalProgramData
    if (Test-Path -LiteralPath $script:WorkRoot) {
        Remove-Item -LiteralPath $script:WorkRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
