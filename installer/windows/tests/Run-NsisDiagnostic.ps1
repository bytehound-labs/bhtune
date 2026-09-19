param(
    [Parameter(Mandatory = $true)]
    [string]$InstallerPath,

    [Parameter(Mandatory = $true)]
    [string]$LogPath,

    [Parameter(Mandatory = $false)]
    [AllowEmptyString()]
    [string]$ExpectedVersion = '',

    [int]$TimeoutSeconds = 30,

    [int]$OverallTimeoutSeconds = 1800,

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
        $gatewayService = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
        $installRootExists = Test-Path -LiteralPath $Paths.InstallRoot
        $markerExists = $null -ne (Get-RegistrySnapshot -Path $Paths.MarkerPath)
        $uninstallExists = $null -ne (Get-RegistrySnapshot -Path $Paths.UninstallKeyPath)
        $processes = @(Get-Process -Name 'bhtune-server' -ErrorAction SilentlyContinue)
        $gatewayProcesses = @(Get-Process -Name 'opcda-bridge-gateway' -ErrorAction SilentlyContinue)
        $gatewayListeners = @(Get-TcpListenerSnapshots -Port $Paths.GatewayPort)
        $finalizerRoots = @(
            if (Test-Path -LiteralPath $Paths.InstallerStateRoot -PathType Container) {
                Get-ChildItem -LiteralPath $Paths.InstallerStateRoot -Directory -Force -ErrorAction SilentlyContinue |
                    Where-Object { $_.Name -like 'uninstall-finalizer-*' }
            }
        )

        if ($null -eq $service -and
            $null -eq $gatewayService -and
            -not $installRootExists -and
            -not $markerExists -and
            -not $uninstallExists -and
            $processes.Count -eq 0 -and
            $gatewayProcesses.Count -eq 0 -and
            $gatewayListeners.Count -eq 0 -and
            $finalizerRoots.Count -eq 0) {
            return
        }

        $serviceState = if ($null -eq $service) { 'missing' } else { [string]$service.State }
        $gatewayServiceState = if ($null -eq $gatewayService) { 'missing' } else { [string]$gatewayService.State }
        $lastState = "service=$serviceState; gatewayService=$gatewayServiceState; installRoot=$installRootExists; marker=$markerExists; uninstall=$uninstallExists; processes=$($processes.Count); gatewayProcesses=$($gatewayProcesses.Count); gatewayListeners=$($gatewayListeners.Count); finalizerRoots=$($finalizerRoots.Count)"
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

function Invoke-InstallerDiagnostic {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Scenario,

        [Parameter(Mandatory = $false)]
        [string[]]$AdditionalArguments = @(),

        [Parameter(Mandatory = $false)]
        [bool]$ExpectSuccess = $true
    )

    $arguments = @('/S', '/NCRC', '/ADD_TO_PATH=0', '/START_SERVICE=0')
    if ($CustomDbBackupConfirmed) {
        $arguments += '/CUSTOM_DB_BACKUP_CONFIRMED=1'
    }
    $arguments += $AdditionalArguments

    Write-DiagnosticLog ("SCENARIO_BEGIN name={0} args={1}" -f $Scenario, ($arguments -join ' '))
    $process = Start-Process -FilePath $InstallerPath -ArgumentList $arguments -PassThru -ErrorAction Stop
    Write-DiagnosticLog "INSTALLER_PID scenario=$Scenario pid=$($process.Id)"
    Write-ProcessSnapshot -ProcessId $process.Id
    $exitCode = Wait-ProcessExit -Process $process -Timeout $TimeoutSeconds
    Write-DiagnosticLog "INSTALLER_EXIT_CODE scenario=$Scenario code=$exitCode"
    if ($ExpectSuccess) {
        Assert-Diagnostic -Condition ($exitCode -eq 0) -Message "Scenario '$Scenario' failed with installer exit code $exitCode."
    } else {
        Assert-Diagnostic -Condition ($exitCode -ne 0) -Message "Scenario '$Scenario' unexpectedly succeeded."
    }
    Write-DiagnosticLog "SCENARIO_INSTALL_RESULT name=$Scenario success=$ExpectSuccess"
    return $exitCode
}

function Invoke-UninstallerDiagnostic {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Scenario,

        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    Assert-Diagnostic -Condition (Test-Path -LiteralPath $Paths.UninstallerPath -PathType Leaf) -Message "Scenario '$Scenario' has no uninstaller."
    $process = Start-Process -FilePath $Paths.UninstallerPath -ArgumentList @('/S', '/NCRC') -PassThru -ErrorAction Stop
    Write-DiagnosticLog "UNINSTALLER_PID scenario=$Scenario pid=$($process.Id)"
    Write-ProcessSnapshot -ProcessId $process.Id
    $exitCode = Wait-ProcessExit -Process $process -Timeout $TimeoutSeconds
    Write-DiagnosticLog "UNINSTALLER_EXIT_CODE scenario=$Scenario code=$exitCode"
    Assert-Diagnostic -Condition ($exitCode -eq 0) -Message "Scenario '$Scenario' uninstall failed with exit code $exitCode."
    Wait-ForUninstallCleanup -Paths $Paths -Timeout $TimeoutSeconds
    Write-DiagnosticLog "SCENARIO_UNINSTALL_RESULT name=$Scenario success=True"
}

function Assert-NoInstalledState {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $false)]
        [bool]$AllowGatewayService = $false
    )

    Assert-Diagnostic -Condition ($null -eq (Get-ServiceSnapshot -Name $Paths.ServiceName)) -Message 'The BhtuneServer service exists without installer ownership.'
    if (-not $AllowGatewayService) {
        Assert-Diagnostic -Condition ($null -eq (Get-ServiceSnapshot -Name $Paths.GatewayServiceName)) -Message 'The OpcdaBridgeGateway service exists without installer ownership.'
        Assert-Diagnostic -Condition (@(Get-TcpListenerSnapshots -Port $Paths.GatewayPort).Count -eq 0) -Message 'TCP port 7600 remains occupied.'
    }
    Assert-Diagnostic -Condition (-not (Test-Path -LiteralPath $Paths.InstallRoot)) -Message 'The fixed Program Files root exists without installer ownership.'
    Assert-Diagnostic -Condition ($null -eq (Get-RegistrySnapshot -Path $Paths.MarkerPath)) -Message 'Installer ownership metadata exists unexpectedly.'
    Assert-Diagnostic -Condition ($null -eq (Get-RegistrySnapshot -Path $Paths.UninstallKeyPath)) -Message 'Uninstall metadata exists unexpectedly.'
}

function Assert-CoreInstallation {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$Version,

        [Parameter(Mandatory = $false)]
        [string]$ExpectedDatabasePath
    )

    if ([string]::IsNullOrWhiteSpace($ExpectedDatabasePath)) {
        $ExpectedDatabasePath = $Paths.DatabasePath
    }
    Assert-InstallerFixedPaths -Paths $Paths | Out-Null
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $Paths.InstallRoot -PathType Container) -Message 'The fixed Program Files install root was not created.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $Paths.ConfigPath -PathType Leaf) -Message 'The fixed ProgramData configuration was not created.'
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $Paths.UninstallerPath -PathType Leaf) -Message 'The uninstaller was not copied into the fixed install root.'

    $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
    $uninstall = Get-RegistrySnapshot -Path $Paths.UninstallKeyPath
    Assert-Diagnostic -Condition ($null -ne $marker) -Message 'Installer ownership metadata was not created.'
    Assert-Diagnostic -Condition ($null -ne $uninstall) -Message 'Uninstall metadata was not created.'
    Assert-Diagnostic -Condition ([int](Get-SnapshotValue -Snapshot $marker.Values -Name 'SchemaVersion') -eq 3) -Message 'The installer did not write schema-3 ownership metadata.'

    $service = Get-ServiceSnapshot -Name $Paths.ServiceName
    Assert-Diagnostic -Condition ($null -ne $service) -Message 'The BhtuneServer service was not created.'
    Assert-Diagnostic -Condition (Test-OwnedServiceSnapshot `
            -ServiceSnapshot $service `
            -ExecutablePath $Paths.ServiceExecutable `
            -ConfigPath $Paths.ConfigPath) -Message 'The created BhtuneServer service does not match the installer-owned definition.'

    $configText = Get-Content -LiteralPath $Paths.ConfigPath -Raw
    Assert-Diagnostic -Condition ($configText.Contains('bind = "127.0.0.1:8787"')) -Message 'The installed BHTune configuration is not loopback-only.'
    Assert-Diagnostic -Condition ($configText.Contains(('db = "{0}"' -f (ConvertTo-TomlPath -Path $ExpectedDatabasePath)))) -Message 'The installed BHTune configuration does not contain the expected database path.'

    if ([string]$service.State -ne 'Running') {
        Start-Service -Name $Paths.ServiceName
    }
    Wait-ForHealth -Uri 'http://127.0.0.1:8787/api/health' -Version $Version -Timeout $TimeoutSeconds
    Write-DiagnosticLog "HEALTH_OK version=$Version"
}

function Assert-GatewayInstallation {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [bool]$ExpectedManaged,

        [Parameter(Mandatory = $false)]
        [bool]$ExpectedRunning = $false
    )

    $marker = Get-RegistrySnapshot -Path $Paths.MarkerPath
    Assert-Diagnostic -Condition ($null -ne $marker) -Message 'Installer ownership metadata is missing.'
    $managedValue = [int](Get-SnapshotValue -Snapshot $marker.Values -Name 'GatewayManaged')
    Assert-Diagnostic -Condition ($managedValue -eq $(if ($ExpectedManaged) { 1 } else { 0 })) -Message 'Gateway ownership metadata does not match the expected component state.'

    $service = Get-ServiceSnapshot -Name $Paths.GatewayServiceName
    if (-not $ExpectedManaged) {
        Assert-Diagnostic -Condition ($null -eq $service) -Message 'The gateway service was installed despite component opt-out.'
        Assert-Diagnostic -Condition (-not (Test-Path -LiteralPath $Paths.GatewayInstallRoot)) -Message 'The gateway Program Files payload was installed despite component opt-out.'
        return
    }

    $payload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $Paths.GatewayInstallRoot
    Assert-GatewayPayloadBinary -GatewayPayload $payload | Out-Null
    Assert-Diagnostic -Condition ($null -ne $service) -Message 'The installer-managed gateway service is missing.'
    Assert-Diagnostic -Condition (Test-OwnedGatewayServiceSnapshot `
            -ServiceSnapshot $service `
            -ExecutablePath $Paths.GatewayExecutable `
            -ConfigPath $Paths.GatewayConfigPath `
            -LogDirectory $Paths.GatewayLogDirectory) -Message 'The gateway service does not match the installer-owned definition.'
    Assert-GatewayConfigPolicy `
        -ConfigPath $Paths.GatewayConfigPath `
        -ExpectedDatabasePath $Paths.GatewayDatabasePath `
        -ExpectedLogDirectory $Paths.GatewayLogDirectory | Out-Null

    $expectedState = if ($ExpectedRunning) { 'Running' } else { 'Stopped' }
    Assert-Diagnostic -Condition ([string]$service.State -eq $expectedState) -Message "The gateway service state is '$($service.State)', expected '$expectedState'."
    if ($ExpectedRunning) {
        Assert-GatewayListenerOwnership -Paths $Paths -ServiceSnapshot $service | Out-Null
        Invoke-GatewaySmokeCheck -Paths $Paths | Out-Null
    } else {
        Assert-Diagnostic -Condition (Test-TcpPortFree -Port $Paths.GatewayPort) -Message 'A stopped gateway still owns TCP port 7600.'
    }
}

function Get-GatewayFirewallFingerprint {
    $rules = New-Object System.Collections.ArrayList
    $portFilters = @(Get-NetFirewallPortFilter -PolicyStore ActiveStore -ErrorAction Stop |
            Where-Object { [string]$_.LocalPort -eq '7600' })
    foreach ($filter in $portFilters) {
        foreach ($rule in @(Get-NetFirewallRule -AssociatedNetFirewallPortFilter $filter -ErrorAction Stop)) {
            [void]$rules.Add($rule)
        }
    }
    foreach ($rule in @(Get-NetFirewallRule -PolicyStore ActiveStore -ErrorAction Stop |
            Where-Object {
                [string]$_.Name -match '(?i)bhtune|opcdabridgegateway' -or
                [string]$_.DisplayName -match '(?i)bhtune|opc da bridge'
            })) {
        [void]$rules.Add($rule)
    }

    return @($rules |
            Sort-Object -Property Name -Unique |
            ForEach-Object {
                '{0}|{1}|{2}|{3}|{4}|{5}' -f $_.Name, $_.DisplayName, $_.Enabled, $_.Direction, $_.Action, $_.Profile
            }) -join "`n"
}

function Remove-DiagnosticProgramData {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    Assert-NoInstalledState -Paths $Paths
    if (Test-Path -LiteralPath $Paths.ProgramDataRoot) {
        Assert-NoReparsePointInPath -Path $Paths.ProgramDataRoot -Name 'the diagnostic BHTune ProgramData root'
        Remove-Item -LiteralPath $Paths.ProgramDataRoot -Recurse -Force
    }
}

function Set-LegacySchema2Marker {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths
    )

    New-ItemProperty `
        -Path $Paths.MarkerPath `
        -Name SchemaVersion `
        -Value 2 `
        -PropertyType DWord `
        -Force | Out-Null
    foreach ($name in @(
            'GatewayManaged',
            'GatewayVersion',
            'GatewaySha256',
            'GatewayExecutable',
            'GatewayConfigPath',
            'GatewayServiceName',
            'GatewayPort'
        )) {
        Remove-ItemProperty -LiteralPath $Paths.MarkerPath -Name $name -ErrorAction SilentlyContinue
    }
}

function Start-DiagnosticPortListener {
    param(
        [Parameter(Mandatory = $true)]
        [int]$Port
    )

    $scriptPath = Join-Path $env:TEMP ('bhtune-listener-{0}.ps1' -f ([guid]::NewGuid().ToString('N')))
    Set-Content -LiteralPath $scriptPath -Encoding UTF8 -Value @"
`$listener = New-Object System.Net.Sockets.TcpListener([System.Net.IPAddress]::Any, $Port)
`$listener.Start()
try {
    while (`$true) {
        Start-Sleep -Seconds 1
    }
} finally {
    `$listener.Stop()
}
"@
    $arguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}"' -f $scriptPath
    $process = Start-Process `
        -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
        -ArgumentList $arguments `
        -PassThru `
        -ErrorAction Stop
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $listeners = @(Get-TcpListenerSnapshots -Port $Port)
        if ($listeners | Where-Object { $_.OwningProcess -eq $process.Id }) {
            return [pscustomobject]@{
                Process    = $process
                ScriptPath = $scriptPath
            }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    if ($null -ne (Get-Process -Id $process.Id -ErrorAction SilentlyContinue)) {
        Stop-Process -Id $process.Id -Force -ErrorAction Stop
    }
    Remove-Item -LiteralPath $scriptPath -Force -ErrorAction SilentlyContinue
    throw "The diagnostic listener did not bind port $Port."
}

function Stop-DiagnosticPortListener {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Listener,

        [Parameter(Mandatory = $true)]
        [int]$Port
    )

    $process = Get-Process -Id $Listener.Process.Id -ErrorAction SilentlyContinue
    if ($null -ne $process) {
        Stop-Process -Id $process.Id -Force -ErrorAction Stop
        $process.WaitForExit()
    }
    Wait-TcpPortFree -Port $Port -TimeoutSeconds $TimeoutSeconds
    Remove-Item -LiteralPath $Listener.ScriptPath -Force -ErrorAction SilentlyContinue
}

function Invoke-GatewayRollbackDiagnostic {
    param(
        [Parameter(Mandatory = $true)]
        [psobject]$Paths,

        [Parameter(Mandatory = $true)]
        [string]$Version
    )

    $sourceRoot = Join-Path $env:TEMP ('bhtune-gateway-rollback-source-{0}' -f ([guid]::NewGuid().ToString('N')))
    $payloadRoot = Join-Path $sourceRoot 'payload'
    $gatewayPayloadRoot = Join-Path $sourceRoot 'gateway'
    $installerScriptRoot = Join-Path $sourceRoot 'installer'
    $uninstallerSource = Join-Path $sourceRoot 'uninstall.exe'
    $tracePath = Join-Path $env:TEMP ('bhtune-gateway-rollback-{0}.jsonl' -f ([guid]::NewGuid().ToString('N')))
    try {
        New-Item -ItemType Directory -Path $payloadRoot, $gatewayPayloadRoot, $installerScriptRoot -Force | Out-Null
        foreach ($name in (Get-RequiredPayloadFiles)) {
            Copy-Item -LiteralPath (Join-Path $Paths.InstallRoot $name) -Destination (Join-Path $payloadRoot $name) -Force
        }
        foreach ($name in (Get-GatewayRequiredPayloadFiles)) {
            Copy-Item -LiteralPath (Join-Path $Paths.GatewayInstallRoot $name) -Destination (Join-Path $gatewayPayloadRoot $name) -Force
        }
        Copy-Item -LiteralPath (Join-Path $Paths.InstallerScriptRoot 'InstallerSupport.ps1') -Destination $installerScriptRoot -Force
        Copy-Item -LiteralPath (Join-Path $Paths.InstallerScriptRoot 'Install-Bhtune.ps1') -Destination $installerScriptRoot -Force
        Copy-Item -LiteralPath $Paths.UninstallerPath -Destination $uninstallerSource -Force

        $scriptPath = Join-Path $installerScriptRoot 'Install-Bhtune.ps1'
        $arguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -Mode Install -ExpectedVersion "{1}" -ReleaseTag "v{1}" -PayloadRoot "{2}" -GatewayPayloadRoot "{3}" -InstallerScriptRoot "{4}" -UninstallerSource "{5}" -InstallRoot "{6}" -ProgramDataRoot "{7}" -AddToPath 0 -StartService 0 -InstallGateway 1 -StartGateway 1 -CustomDbBackupConfirmed 1 -TestOnly -FailureInjection GatewaySmokeFailure -TracePath "{8}"' -f `
            $scriptPath,
            $Version,
            $payloadRoot,
            $gatewayPayloadRoot,
            $installerScriptRoot,
            $uninstallerSource,
            $Paths.InstallRoot,
            $Paths.ProgramDataRoot,
            $tracePath
        $process = Start-Process `
            -FilePath "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" `
            -ArgumentList $arguments `
            -PassThru `
            -ErrorAction Stop
        Write-DiagnosticLog "ROLLBACK_DIAGNOSTIC_PID=$($process.Id)"
        $exitCode = Wait-ProcessExit -Process $process -Timeout $TimeoutSeconds
        Write-DiagnosticLog "ROLLBACK_DIAGNOSTIC_EXIT_CODE=$exitCode"
        Assert-Diagnostic -Condition ($exitCode -ne 0) -Message 'The injected gateway smoke failure unexpectedly succeeded.'
        Assert-Diagnostic -Condition (Test-Path -LiteralPath $tracePath -PathType Leaf) -Message 'The gateway rollback diagnostic trace is missing.'
        $trace = Get-Content -LiteralPath $tracePath -Raw
        Assert-Diagnostic -Condition ($trace.Contains('"Stage":"gateway.service.start.end"')) -Message 'The rollback diagnostic failed before the candidate gateway started.'
        Assert-Diagnostic -Condition ($trace.Contains('"Stage":"rollback.end"')) -Message 'The injected gateway failure did not complete rollback.'
    } finally {
        Remove-Item -LiteralPath $tracePath -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $sourceRoot) {
            Remove-Item -LiteralPath $sourceRoot -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}

function Stop-DiagnosticScheduledTask {
    param(
        [Parameter(Mandatory = $true)]
        [string]$TaskName,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($null -eq $task) {
        return
    }

    $activeStates = @('Running', 'Queued')
    if ($activeStates -notcontains [string]$task.State) {
        return
    }

    Write-DiagnosticLog "TASK_STOP_REQUESTED_STATE=$($task.State)"
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction Stop
    $deadline = [DateTime]::Now.AddSeconds($TimeoutSeconds)
    do {
        Start-Sleep -Milliseconds 250
        $task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($null -eq $task -or $activeStates -notcontains [string]$task.State) {
            Write-DiagnosticLog "TASK_STOPPED_STATE=$(if ($null -eq $task) { 'Missing' } else { [string]$task.State })"
            return
        }
    } while ([DateTime]::Now -lt $deadline)

    throw "The SYSTEM lifecycle task remained '$($task.State)' after a stop request."
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
    $actionArguments = '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -Child -InstallerPath "{1}" -LogPath "{2}" -ExpectedVersion "{3}" -TimeoutSeconds {4} -OverallTimeoutSeconds {5}{6}' -f `
        $scriptPath, $InstallerPath, $LogPath, $ExpectedVersion, $TimeoutSeconds, $OverallTimeoutSeconds, $customDbArgument
    $action = New-ScheduledTaskAction -Execute "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument $actionArguments
    $principal = New-ScheduledTaskPrincipal -UserId 'SYSTEM' -LogonType ServiceAccount -RunLevel Highest

    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Register-ScheduledTask -TaskName $taskName -Action $action -Principal $principal -Force | Out-Null
    try {
        Start-ScheduledTask -TaskName $taskName
        $deadline = [DateTime]::Now.AddSeconds($OverallTimeoutSeconds + 30)
        $activeStates = @('Running', 'Queued')
        do {
            Start-Sleep -Milliseconds 500
            $state = (Get-ScheduledTask -TaskName $taskName).State
        } while ($activeStates -contains [string]$state -and [DateTime]::Now -lt $deadline)

        Write-DiagnosticLog "TASK_STATE=$state"
        if ($activeStates -contains [string]$state) {
            throw "The SYSTEM lifecycle task exceeded its $($OverallTimeoutSeconds + 30)-second parent deadline."
        }
        $info = Get-ScheduledTaskInfo -TaskName $taskName
        Write-DiagnosticLog ("TASK_LAST_RESULT={0} TASK_LAST_RUN={1} TASK_NEXT_RUN={2}" -f `
                $info.LastTaskResult, $info.LastRunTime, $info.NextRunTime)
        if ([int64]$info.LastTaskResult -ne 0) {
            throw "The SYSTEM lifecycle task failed with result $($info.LastTaskResult)."
        }
    } finally {
        Stop-DiagnosticScheduledTask -TaskName $taskName -TimeoutSeconds 30
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

    $paths = Get-InstallerPaths
    Assert-InstallerFixedPaths -Paths $paths | Out-Null
    Assert-NoInstalledState -Paths $paths
    Assert-Diagnostic -Condition (-not (Test-Path -LiteralPath $paths.ProgramDataRoot)) -Message 'The Windows runner contains pre-existing BHTune ProgramData; refusing to run a destructive lifecycle diagnostic.'
    $firewallBefore = Get-GatewayFirewallFingerprint

    # Silent clean installs require an explicit gateway opt-in because they
    # cannot display the interactive all-interface exposure warning.
    Invoke-InstallerDiagnostic -Scenario 'clean-silent-gateway-opt-in-required' | Out-Null
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$false
    Invoke-UninstallerDiagnostic -Scenario 'clean-silent-gateway-opt-in-required' -Paths $paths
    Assert-NoInstalledState -Paths $paths
    Write-DiagnosticLog 'SCENARIO_PASS name=clean-silent-gateway-opt-in-required'
    Remove-DiagnosticProgramData -Paths $paths

    # Explicit silent opt-in installs and starts the gateway by default.
    Invoke-InstallerDiagnostic `
        -Scenario 'clean-explicit-gateway' `
        -AdditionalArguments @('/INSTALL_GATEWAY=1') | Out-Null
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$true -ExpectedRunning:$true
    Assert-Diagnostic -Condition ((Get-GatewayFirewallFingerprint) -ceq $firewallBefore) -Message 'The installer changed a gateway-related Windows Firewall rule.'

    Stop-InstallerGatewayService -Paths $paths | Out-Null
    $defaultSentinels = @(
        $paths.GatewayDatabasePath,
        "$($paths.GatewayDatabasePath)-wal",
        "$($paths.GatewayDatabasePath)-shm",
        (Join-Path $paths.GatewayDataDirectory 'build.lock'),
        (Join-Path $paths.GatewayDataDirectory 'build.owner'),
        (Join-Path $paths.GatewayLogDirectory 'diagnostic.log'),
        (Join-Path $paths.GatewayProgramDataRoot 'rollback-evidence.json')
    )
    foreach ($sentinelPath in $defaultSentinels) {
        if (-not (Test-Path -LiteralPath $sentinelPath -PathType Leaf)) {
            Write-TestFile -Path $sentinelPath -Content ("preserve {0}" -f ([System.IO.Path]::GetFileName($sentinelPath)))
        }
    }
    $defaultHashes = @{}
    foreach ($sentinelPath in $defaultSentinels) {
        $defaultHashes[$sentinelPath] = Get-FileSha256 -Path $sentinelPath
    }
    $defaultConfigHash = Get-FileSha256 -Path $paths.GatewayConfigPath
    Invoke-UninstallerDiagnostic -Scenario 'clean-explicit-gateway' -Paths $paths
    Assert-NoInstalledState -Paths $paths
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.GatewayProgramDataRoot -PathType Container) -Message 'Gateway ProgramData was removed by uninstall.'
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.GatewayConfigPath) -eq $defaultConfigHash) -Message 'The gateway configuration changed during uninstall.'
    foreach ($sentinelPath in $defaultSentinels) {
        Assert-Diagnostic -Condition (Test-Path -LiteralPath $sentinelPath -PathType Leaf) -Message "Gateway ProgramData file '$sentinelPath' was removed during uninstall."
        Assert-Diagnostic -Condition ((Get-FileSha256 -Path $sentinelPath) -eq $defaultHashes[$sentinelPath]) -Message "Gateway ProgramData file '$sentinelPath' changed during uninstall."
    }
    Write-DiagnosticLog 'SCENARIO_PASS name=clean-explicit-gateway'
    Remove-DiagnosticProgramData -Paths $paths

    # A clean opt-out remains gateway-free, including a simulated schema-2
    # upgrade, until the operator explicitly opts in.
    Invoke-InstallerDiagnostic `
        -Scenario 'clean-gateway-opt-out' `
        -AdditionalArguments @('/INSTALL_GATEWAY=0', '/START_GATEWAY=0') | Out-Null
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$false
    Set-LegacySchema2Marker -Paths $paths
    Invoke-InstallerDiagnostic -Scenario 'legacy-schema2-preserves-gateway-absence' | Out-Null
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$false
    Write-DiagnosticLog 'SCENARIO_PASS name=legacy-schema2-preserves-gateway-absence'

    # Explicit add-on installs the gateway but honors a stopped final state.
    Invoke-InstallerDiagnostic `
        -Scenario 'explicit-gateway-add-on-stopped' `
        -AdditionalArguments @('/INSTALL_GATEWAY=1', '/START_GATEWAY=0') | Out-Null
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$true -ExpectedRunning:$false

    # Once owned, deselection cannot abandon the component and START_GATEWAY
    # cannot override the prior stopped/running state on an upgrade.
    Invoke-InstallerDiagnostic `
        -Scenario 'managed-stopped-upgrade' `
        -AdditionalArguments @('/INSTALL_GATEWAY=0', '/START_GATEWAY=1') | Out-Null
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$true -ExpectedRunning:$false
    Start-InstallerGatewayService -Paths $paths | Out-Null
    Invoke-GatewaySmokeCheck -Paths $paths | Out-Null
    Invoke-InstallerDiagnostic `
        -Scenario 'managed-running-upgrade' `
        -AdditionalArguments @('/INSTALL_GATEWAY=0', '/START_GATEWAY=0') | Out-Null
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$true -ExpectedRunning:$true
    Write-DiagnosticLog 'SCENARIO_PASS name=managed-upgrade-state-preservation'

    # Exercise a real Windows SCM rollback while BHTune uses an external
    # database. The gateway starts transiently, the injected smoke failure
    # occurs, and the verified prior stopped state plus ProgramData return.
    $externalRoot = Join-Path $env:ProgramData 'ByteHound\bhtune-installer-diagnostic-external'
    Assert-Diagnostic -Condition (-not (Test-Path -LiteralPath $externalRoot)) -Message "The external-database diagnostic root already exists: $externalRoot"
    New-Item -ItemType Directory -Path $externalRoot -Force | Out-Null
    $externalDatabase = Join-Path $externalRoot 'bhtune-external.db'
    Stop-InstallerService | Out-Null
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $paths.DatabasePath -PathType Leaf) -Message 'The managed database was not created before the external-database rollback scenario.'
    Copy-Item -LiteralPath $paths.DatabasePath -Destination $externalDatabase -Force
    $configText = Get-Content -LiteralPath $paths.ConfigPath -Raw
    $externalConfigLine = 'db = "{0}"' -f (ConvertTo-TomlPath -Path $externalDatabase)
    $updatedConfig = [regex]::Replace($configText, '(?m)^db\s*=\s*"[^"]*"\s*$', $externalConfigLine)
    Assert-Diagnostic -Condition ($updatedConfig -cne $configText) -Message 'The external database path could not be applied to bhtune.toml.'
    Write-TextFile -Path $paths.ConfigPath -Content $updatedConfig
    Start-InstallerService | Out-Null
    Wait-ForHealth -Uri 'http://127.0.0.1:8787/api/health' -Version $ExpectedVersion -Timeout $TimeoutSeconds
    Stop-InstallerGatewayService -Paths $paths | Out-Null

    $rollbackSentinels = @(
        (Join-Path $paths.GatewayProgramDataRoot 'rollback-sentinel.txt'),
        (Join-Path $paths.GatewayDataDirectory 'rollback-build.lock'),
        (Join-Path $paths.GatewayDataDirectory 'rollback-build.owner'),
        (Join-Path $paths.GatewayLogDirectory 'rollback.log')
    )
    foreach ($sentinelPath in $rollbackSentinels) {
        Write-TestFile -Path $sentinelPath -Content ("rollback preserve {0}" -f ([System.IO.Path]::GetFileName($sentinelPath)))
    }
    $rollbackHashes = @{}
    foreach ($sentinelPath in $rollbackSentinels) {
        $rollbackHashes[$sentinelPath] = Get-FileSha256 -Path $sentinelPath
    }
    $rollbackGatewayConfigHash = Get-FileSha256 -Path $paths.GatewayConfigPath
    $rollbackBhtuneConfigHash = Get-FileSha256 -Path $paths.ConfigPath
    Invoke-GatewayRollbackDiagnostic -Paths $paths -Version $ExpectedVersion
    Assert-CoreInstallation -Paths $paths -Version $ExpectedVersion -ExpectedDatabasePath $externalDatabase
    Assert-GatewayInstallation -Paths $paths -ExpectedManaged:$true -ExpectedRunning:$false
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.ConfigPath) -eq $rollbackBhtuneConfigHash) -Message 'The external-database BHTune configuration was not restored after gateway rollback.'
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.GatewayConfigPath) -eq $rollbackGatewayConfigHash) -Message 'The gateway configuration was not restored after rollback.'
    foreach ($sentinelPath in $rollbackSentinels) {
        Assert-Diagnostic -Condition ((Get-FileSha256 -Path $sentinelPath) -eq $rollbackHashes[$sentinelPath]) -Message "Gateway rollback did not restore '$sentinelPath'."
    }
    Start-InstallerGatewayService -Paths $paths | Out-Null
    Invoke-GatewaySmokeCheck -Paths $paths | Out-Null
    Write-DiagnosticLog 'SCENARIO_PASS name=gateway-smoke-failure-rollback'

    Invoke-UninstallerDiagnostic -Scenario 'gateway-programdata-preservation' -Paths $paths
    Assert-NoInstalledState -Paths $paths
    Assert-Diagnostic -Condition (Test-Path -LiteralPath $externalDatabase -PathType Leaf) -Message 'Uninstall removed the external BHTune database.'
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.ConfigPath) -eq $rollbackBhtuneConfigHash) -Message 'Uninstall changed the external-database BHTune configuration.'
    Assert-Diagnostic -Condition ((Get-FileSha256 -Path $paths.GatewayConfigPath) -eq $rollbackGatewayConfigHash) -Message 'Uninstall changed the gateway configuration.'
    foreach ($sentinelPath in $rollbackSentinels) {
        Assert-Diagnostic -Condition ((Get-FileSha256 -Path $sentinelPath) -eq $rollbackHashes[$sentinelPath]) -Message "Uninstall did not preserve '$sentinelPath'."
    }
    Write-DiagnosticLog 'SCENARIO_PASS name=gateway-programdata-preservation'
    Remove-DiagnosticProgramData -Paths $paths
    Remove-Item -LiteralPath $externalRoot -Recurse -Force

    # An unowned service blocks installation without being adopted or removed.
    $unownedCommand = '"' + (Join-Path $env:SystemRoot 'System32\cmd.exe') + '" /c exit 0'
    New-Service `
        -Name $paths.GatewayServiceName `
        -BinaryPathName $unownedCommand `
        -DisplayName 'Unowned OPC DA gateway diagnostic service' `
        -StartupType Manual | Out-Null
    try {
        Invoke-InstallerDiagnostic -Scenario 'unowned-gateway-service-conflict' -ExpectSuccess:$false | Out-Null
        Assert-NoInstalledState -Paths $paths -AllowGatewayService:$true
        Assert-Diagnostic -Condition ($null -ne (Get-ServiceSnapshot -Name $paths.GatewayServiceName)) -Message 'The installer removed the unowned gateway service.'
    } finally {
        Remove-ServiceByName -Name $paths.GatewayServiceName
    }
    Assert-NoInstalledState -Paths $paths
    Write-DiagnosticLog 'SCENARIO_PASS name=unowned-gateway-service-conflict'

    # An unrelated process listening on 7600 blocks installation and remains
    # untouched until the diagnostic stops that exact PID.
    $listener = Start-DiagnosticPortListener -Port $paths.GatewayPort
    try {
        Invoke-InstallerDiagnostic -Scenario 'unexpected-gateway-listener-conflict' -ExpectSuccess:$false | Out-Null
        Assert-Diagnostic -Condition ($null -eq (Get-ServiceSnapshot -Name $paths.ServiceName)) -Message 'The listener-conflict failure left BhtuneServer behind.'
        Assert-Diagnostic -Condition ($null -eq (Get-ServiceSnapshot -Name $paths.GatewayServiceName)) -Message 'The listener-conflict failure left OpcdaBridgeGateway behind.'
        Assert-Diagnostic -Condition (-not (Test-Path -LiteralPath $paths.InstallRoot)) -Message 'The listener-conflict failure left Program Files content behind.'
        Assert-Diagnostic -Condition ($null -eq (Get-RegistrySnapshot -Path $paths.MarkerPath)) -Message 'The listener-conflict failure left ownership metadata behind.'
        Assert-Diagnostic -Condition ($null -ne (Get-Process -Id $listener.Process.Id -ErrorAction SilentlyContinue)) -Message 'The installer terminated the unrelated port owner.'
    } finally {
        Stop-DiagnosticPortListener -Listener $listener -Port $paths.GatewayPort
    }
    Assert-NoInstalledState -Paths $paths
    Assert-Diagnostic -Condition ((Get-GatewayFirewallFingerprint) -ceq $firewallBefore) -Message 'The installer created or modified a gateway-related Windows Firewall rule.'
    Write-DiagnosticLog 'SCENARIO_PASS name=unexpected-gateway-listener-conflict'
    Write-DiagnosticLog 'LIFECYCLE_RESULT=PASS scenarios=silent-opt-in,explicit-gateway,opt-out,legacy-add-on,upgrade-state,rollback,uninstall,service-conflict,listener-conflict,firewall'
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
