param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ExpectedVersion = '',

    [int]$TimeoutSeconds = 30,

    [switch]$CustomDbBackupConfirmed,

    [switch]$Child
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version 2.0

$supportPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'InstallerSupport.ps1'
if (-not (Test-Path -LiteralPath $supportPath -PathType Leaf)) {
    throw "Installer helper is missing: $supportPath"
}
. $supportPath

function Write-DiagnosticLog {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    $Message | Add-Content -LiteralPath $LogPath -Encoding UTF8
}

function Write-TestFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    Set-Content -LiteralPath $Path -Value $Content -Encoding UTF8
}

function Write-ProcessSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId
    )

    $process = Get-CimInstance Win32_Process -Filter "ProcessId = $ProcessId"
    if ($null -eq $process) {
        Write-DiagnosticLog "PROCESS_SNAPSHOT pid=$ProcessId absent"
        return
    }

    Write-DiagnosticLog ("PROCESS_SNAPSHOT pid={0} name={1} parent={2} command={3}" -f `
            $process.ProcessId, $process.Name, $process.ParentProcessId, $process.CommandLine)
}

function Write-DescendantSnapshots {
    param(
        [Parameter(Mandatory = $true)]
        [int]$RootProcessId
    )

    $processes = @(Get-CimInstance Win32_Process)
    $pending = New-Object 'System.Collections.Generic.Queue[int]'
    $seen = New-Object 'System.Collections.Generic.HashSet[int]'
    $pending.Enqueue($RootProcessId)
    [void]$seen.Add($RootProcessId)

    while ($pending.Count -gt 0) {
        $parentId = $pending.Dequeue()
        foreach ($child in @($processes | Where-Object { $_.ParentProcessId -eq $parentId })) {
            if ($seen.Add([int]$child.ProcessId)) {
                Write-DiagnosticLog ("DESCENDANT pid={0} name={1} parent={2} command={3}" -f `
                        $child.ProcessId, $child.Name, $child.ParentProcessId, $child.CommandLine)
                $pending.Enqueue([int]$child.ProcessId)
            }
        }
    }
}

function Wait-ProcessExit {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process,

        [Parameter(Mandatory = $true)]
        [int]$Timeout
    )

    $deadline = [DateTime]::Now.AddSeconds($Timeout)
    while ([DateTime]::Now -lt $deadline) {
        $current = Get-Process -Id $Process.Id -ErrorAction SilentlyContinue
        if ($null -eq $current) {
            $Process.Refresh()
            return [int]$Process.ExitCode
        }
        Start-Sleep -Milliseconds 250
    }

    Write-DiagnosticLog "PROCESS_TIMEOUT pid=$($Process.Id)"
    Write-ProcessSnapshot -ProcessId $Process.Id
    Write-DescendantSnapshots -RootProcessId $Process.Id
    Stop-Process -Id $Process.Id -Force -ErrorAction Stop
    throw "Process $($Process.Id) did not exit within $Timeout seconds."
}

function Wait-ForHealth {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Uri,

        [Parameter(Mandatory = $true)]
        [string]$Version,

        [Parameter(Mandatory = $true)]
        [int]$Timeout
    )

    $deadline = [DateTime]::Now.AddSeconds($Timeout)
    $lastError = $null
    while ([DateTime]::Now -lt $deadline) {
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri $Uri -TimeoutSec 5
            if ([int]$response.StatusCode -ne 200) {
                throw "health endpoint returned HTTP $($response.StatusCode)"
            }
            $body = $response.Content | ConvertFrom-Json
            if ([string]$body.status -ne 'ok') {
                throw "health endpoint returned status '$($body.status)'"
            }
            if ([string]$body.version -ne $Version) {
                throw "health endpoint returned version '$($body.version)', expected '$Version'"
            }
            return
        } catch {
            $lastError = $_.Exception.Message
            Start-Sleep -Milliseconds 500
        }
    }
    throw "Timed out waiting for ${Uri}: $lastError"
}

function Wait-ForUninstallCleanup {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [int]$Timeout
    )

    $deadline = [DateTime]::Now.AddSeconds($Timeout)
    $lastState = $null
    while ([DateTime]::Now -lt $deadline) {
        $service = Get-ServiceSnapshot -Name $Paths.ServiceName
        $installRootExists = Test-Path -LiteralPath $Paths.InstallRoot
        $markerExists = $null -ne (Get-RegistrySnapshot -Path $Paths.MarkerPath)
        $uninstallExists = $null -ne (Get-RegistrySnapshot -Path $Paths.UninstallKeyPath)
        $processes = @(Get-Process -Name 'bhtune-server' -ErrorAction SilentlyContinue)
        $finalizerRoots = @(
            if (Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container) {
                Get-ChildItem -LiteralPath $Paths.InstallerStateRoot -Directory -Force -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -like 'uninstall-finalizer-*' }
            }
        )

        if ($null -eq $service -and
            -not $installRootExists -and
            -not $markerExists -and
            -not $uninstallExists -and
            $processes.Count -eq 0 -and
            $finalizerRoots.Count -eq 0) {
            return
        }

        $serviceState = if ($null -eq $service) { 'missing' } else { [string]$service.State }
        $lastState = "service=$serviceState; installRoot=$installRootExists; marker=$markerExists; uninstall=$uninstallExists; processes=$($processes.Count); finalizerRoots=$($finalizerRoots.Count)"
        Start-Sleep -Milliseconds 250
    }

    throw "Timed out waiting for uninstall cleanup: $lastState"
}

function Assert-Diagnostic {
    param(
        [Parameter(Mandatory = $true)]
        [bool]$Condition,

        [Parameter(Mandatory = $true)]
        [string]$Message
    )

    if (-not $Condition) {
        throw $Message
    }
}

if (-not $Child) {
    $parent = Split-Path -Parent $LogPath
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
    Set-Content -LiteralPath $LogPath -Value @(
        "START=$([DateTime]::Now.ToString('o'))"
        "USER=$env:USERNAME"
        "INSTALLER=$InstallerPath"
        "INSTALLER_EXISTS=$(Test-Path -LiteralPath $InstallerPath -PathType Leaf)"
    ) -Encoding UTF8

    if (-not (Test-Path -LiteralPath $InstallerPath -PathType Leaf)) {
        Write-DiagnosticLog 'HARNESS_ERROR=installer path does not exist'
        exit 2
    }

    $taskName = 'BhtuneNsisDiagnostic'
    $scriptPath = $PSCommandPath
    $customDbArgument = if ($CustomDbBackupConfirmed) { ' -CustomDbBackupConfirmed' } else { '' }
    $actionArguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -Child -InstallerPath "{1}" -LogPath "{2}" -ExpectedVersion "{3}" -TimeoutSeconds {4}{5}' -f `
        $scriptPath, $InstallerPath, $LogPath, $ExpectedVersion, $TimeoutSeconds, $customDbArgument
    $action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $actionArguments
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest

    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Force | Out-Null
    try {
        Start-ScheduledTask -TaskName $taskName
        $deadline = [DateTime]::Now.AddSeconds($TimeoutSeconds + 15)
        do {
            Start-Sleep -Milliseconds 500
            $state = (Get-ScheduledTask -TaskName $taskName).State
        } while ($state -eq 'Running' -and [DateTime]::Now -lt $deadline)

        Write-DiagnosticLog "TASK_STATE=$state"
        $info = Get-ScheduledTaskInfo -TaskName $taskName
        Write-DiagnosticLog ("TASK_LAST_RESULT={0} TASK_LAST_RUN={1} TASK_NEXT_RUN={2}" -f `
                $info.LastTaskResult, $info.LastRunTime, $info.NextRunTime)
        if ([int64]$info.LastTaskResult -ne 0) {
            throw "The SYSTEM lifecycle task failed with result $($info.LastTaskResult)."
        }
    } finally {
        Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    }

    Write-DiagnosticLog "END=$([DateTime]::Now.ToString('o'))"
    exit 0
}

Write-DiagnosticLog "CHILD_START=$([DateTime]::Now.ToString('o')) USER=$env:USERNAME"
try {
    if ([string]::IsNullOrWhiteSpace($ExpectedVersion)) {
        $installerName = [System.IO.Path]::GetFileName($InstallerPath)
        $versionMatch = [regex]::Match($installerName, 'bhtune-v(?<version>[0-9]+\.[0-9]+\.[0-9]+)-')
        Assert-Diagnostic -Condition $versionMatch.Success -Message "Could not determine the package version from '$installerName'."
        $ExpectedVersion = $versionMatch.Groups['version'].Value
    }

    $installerArguments = @('/S', '/NCRC', '/ADD_TO_PATH=0', '/START_SERVICE=0')
    if ($CustomDbBackupConfirmed) {
        $installerArguments += '/CUSTOM_DB_BACKUP_CONFIRMED=1'
    }
    $process = Start-Process -FilePath $InstallerPath -ArgumentList $installerArguments -PassThru -ErrorAction Stop
    Write-DiagnosticLog "INSTALLER_PID=$($process.Id)"
    Write-ProcessSnapshot -ProcessId $process.Id
    $installExitCode = Wait-ProcessExit -Process $process -Timeout $TimeoutSeconds
    Write-DiagnosticLog "INSTALLER_EXIT_CODE=$installExitCode"
    Assert-Diagnostic -Condition ($installExitCode -eq 0) -Message "Silent installation failed with exit code $installExitCode."

    $paths = Get-InstallerPaths
    Assert-InstallerFixedPaths -Paths $paths | Out-Null
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.InstallRoot -PathType Container) -Message 'The fixed Program Files install root was not created.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.ConfigPath -PathType Leaf) -Message 'The fixed ProgramData configuration was not created.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.UninstallerPath -PathType Leaf) -Message 'The uninstaller was not copied into the fixed install root.'

    $marker = Get-RegistrySnapshot -Path $paths.MarkerPath
    $uninstall = Get-RegistrySnapshot -Path $paths.UninstallKeyPath
    Assert-Diagnostic -Condition ($null -ne $marker) -Message 'Installer ownership metadata was not created.'
    Assert-Diagnostic -Condition ($null -ne $uninstall) -Message 'Uninstall metadata was not created.'
    $service = Get-ServiceSnapshot -Name $paths.ServiceName
    Assert-Diagnostic -Condition ($null -ne $service) -Message 'The BhtuneServer service was not created.'
    Assert-Diagnostic -Condition (Test-OwnedServiceSnapshot `
            -ServiceSnapshot $service `
            -ExecutablePath $paths.ServiceExecutable `
            -ConfigPath $paths.ConfigPath) -Message 'The created service does not match the installer-owned definition.'

    $configText = Get-Content -LiteralPath $paths.ConfigPath -Raw
    Assert-Diagnostic -Condition ($configText.Contains('bind = "127.0.0.1:8787"')) -Message 'The installed configuration is not loopback-only.'
    Assert-Diagnostic -Condition ($configText.Contains(('db = "{0}"' -f (ConvertTo-TomlPath -Path $paths.DatabasePath)))) -Message 'The installed configuration does not pin the managed database.'

    Start-Service -Name $paths.ServiceName
    Wait-ForHealth -Uri 'http://127.0.0.1:8787/api/health' -Version $ExpectedVersion -Timeout $TimeoutSeconds
    Write-DiagnosticLog "HEALTH_OK version=$ExpectedVersion"

    $sentinel = Join-Path $paths.ProgramDataRoot ('diagnostic-sentinel-{0}.txt' -f ([guid]::NewGuid().ToString('N')))
    $operatorFile = Join-Path $paths.DatabaseDirectory 'operator-owned.txt'
    Write-TestFile -Path $sentinel -Content 'installer lifecycle sentinel'
    Write-TestFile -Path $operatorFile -Content 'operator-owned database descendant'
    $configHash = Get-FileSha256 -Path $paths.ConfigPath

    $uninstallProcess = Start-Process -FilePath $paths.UninstallerPath -ArgumentList @('/S', '/NCRC') -PassThru -ErrorAction Stop
    Write-DiagnosticLog "UNINSTALLER_PID=$($uninstallProcess.Id)"
    Write-ProcessSnapshot -ProcessId $uninstallProcess.Id
    $uninstallExitCode = Wait-ProcessExit -Process $uninstallProcess -Timeout $TimeoutSeconds
    Write-DiagnosticLog "UNINSTALLER_EXIT_CODE=$uninstallExitCode"
    Assert-Diagnostic -Condition ($uninstallExitCode -eq 0) -Message "Silent uninstall failed with exit code $uninstallExitCode."

    Wait-ForUninstallCleanup -Paths $paths -Timeout $TimeoutSeconds
    Write-DiagnosticLog 'UNINSTALL_CLEANUP_OK'

    Assert-Diagnostic -Condition ($null -eq (Get-ServiceSnapshot -Name $paths.ServiceName)) -Message 'The BhtuneServer service remains after uninstall.'
    Assert-Diagnostic -Condition ($null -eq (Get-RegistrySnapshot -Path $paths.MarkerPath)) -Message 'Installer ownership metadata remains after complete uninstall.'
    Assert-Diagnostic -Condition ($null -eq (Get-RegistrySnapshot -Path $paths.UninstallKeyPath)) -Message 'Uninstall metadata remains after complete uninstall.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.ProgramDataRoot -PathType Container) -Message 'ProgramData was removed instead of being preserved.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $sentinel -PathType Leaf) -Message 'The ProgramData lifecycle sentinel was removed.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $operatorFile -PathType Leaf) -Message 'An operator-owned database descendant was removed.'
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.ConfigPath) -eq $configHash) -Message 'The preserved configuration changed during uninstall.'
    Write-DiagnosticLog 'LIFECYCLE_RESULT=PASS'
} catch {
    Write-DiagnosticLog "START_ERROR=$($_.Exception.ToString())"
    try {
        $paths = Get-InstallerPaths
        $service = Get-ServiceSnapshot -Name $paths.ServiceName
        if ($null -ne $service) {
            Write-DiagnosticLog "FAILURE_SERVICE_STATE=$($service.State)"
        }
    } catch {
        Write-DiagnosticLog "FAILURE_SNAPSHOT_ERROR=$($_.Exception.Message)"
    }
    exit 10
}

Write-DiagnosticLog "CHILD_END=$([DateTime]::Now.ToString('o'))"
exit 0
