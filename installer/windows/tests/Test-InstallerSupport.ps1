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
    Assert-Null -Actual (Get-VersionFromProcessOutput -Output 'warning: bhtune 3.2.1 payload') -Message 'unstructured version output is rejected'

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

    foreach ($functionName in @(
            'Get-SnapshotValue',
            'Write-TextFile',
            'Write-JsonFile',
            'Enter-InstallerTransactionLock',
            'Exit-InstallerTransactionLock',
            'Read-TransactionJournal',
            'Remove-TransactionJournal',
            'Recover-InterruptedTransaction',
            'Assert-InstallerOwnershipMarker',
            'Assert-InstallerMetadataForRecovery',
            'Assert-InstallerState',
            'Assert-DatabasePolicy',
            'Write-TransactionJournal',
            'Test-TransactionJournalPath',
            'Assert-TransactionJournalShape',
            'Remove-InstallerCreatedConfig',
            'Get-TransactionPhaseRank',
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

        return $script:MockService
    }
    function Invoke-ServiceRegistrationQuery {
        param(
            [Parameter(Mandatory = $true)]
            [string]$Name
        )

        if ($null -ne $script:MockServiceRegistration) {
            return [bool]$script:MockServiceRegistration
        }
        return $null -ne $script:MockService
    }
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

    Write-Host ("InstallerSupport self-tests passed: {0}" -f $script:Passed)
    exit 0
} finally {
    $env:ProgramData = $script:OriginalProgramData
    if (Test-Path -LiteralPath $script:WorkRoot) {
        Remove-Item -LiteralPath $script:WorkRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
