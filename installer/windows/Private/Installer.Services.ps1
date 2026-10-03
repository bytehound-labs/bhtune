# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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

function ConvertFrom-ScQueryExOutput {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$Output = ''
    )

    $stateMatch = [regex]::Match(
        $Output,
        '(?im)^\s*STATE\s*:\s*(?<code>\d+)\s+(?<name>[A-Z_]+)'
    )
    if (-not $stateMatch.Success) {
        throw "sc.exe queryex did not return a service state for '$Name'. Output: $Output"
    }

    $state = switch ([int]$stateMatch.Groups['code'].Value) {
        1 { 'Stopped' }
        2 { 'StartPending' }
        3 { 'StopPending' }
        4 { 'Running' }
        5 { 'ContinuePending' }
        6 { 'PausePending' }
        7 { 'Paused' }
        default { $stateMatch.Groups['name'].Value }
    }

    $parseScNumber = {
        param([string]$Value)
        if ($Value.StartsWith('0x', [System.StringComparison]::OrdinalIgnoreCase)) {
            return [Convert]::ToInt32($Value.Substring(2), 16)
        }
        return [int]$Value
    }

    $processId = 0
    $processMatch = [regex]::Match($Output, '(?im)^\s*PID\s*:\s*(?<value>0x[0-9a-f]+|\d+)')
    if ($processMatch.Success) {
        $processId = & $parseScNumber $processMatch.Groups['value'].Value
    }

    $win32ExitCode = 0
    $win32Match = [regex]::Match($Output, '(?im)^\s*WIN32_EXIT_CODE\s*:\s*(?<value>0x[0-9a-f]+|\d+)')
    if ($win32Match.Success) {
        $win32ExitCode = & $parseScNumber $win32Match.Groups['value'].Value
    }

    $serviceExitCode = 0
    $serviceMatch = [regex]::Match($Output, '(?im)^\s*SERVICE_EXIT_CODE\s*:\s*(?<value>0x[0-9a-f]+|\d+)')
    if ($serviceMatch.Success) {
        $serviceExitCode = & $parseScNumber $serviceMatch.Groups['value'].Value
    }

    $checkpoint = 0
    $checkpointMatch = [regex]::Match($Output, '(?im)^\s*CHECKPOINT\s*:\s*(?<value>0x[0-9a-f]+|\d+)')
    if ($checkpointMatch.Success) {
        $checkpoint = & $parseScNumber $checkpointMatch.Groups['value'].Value
    }

    $waitHint = 0
    $waitHintMatch = [regex]::Match($Output, '(?im)^\s*WAIT_HINT\s*:\s*(?<value>0x[0-9a-f]+|\d+)')
    if ($waitHintMatch.Success) {
        $waitHint = & $parseScNumber $waitHintMatch.Groups['value'].Value
    }

    return [pscustomobject]@{
        Exists           = $true
        Name             = $Name
        State            = $state
        ProcessId        = $processId
        Win32ExitCode    = $win32ExitCode
        ServiceExitCode  = $serviceExitCode
        Checkpoint       = $checkpoint
        WaitHint         = $waitHint
    }
}

function Get-ServiceControlStatus {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutMilliseconds = 5000
    )

    $scPath = Join-Path $env:SystemRoot 'System32\sc.exe'
    if (-not (Test-Path -LiteralPath $scPath -PathType Leaf)) {
        throw 'The Windows Service Control Manager command (sc.exe) is unavailable.'
    }

    Write-InstallerTrace `
        -Stage 'service.state.query' `
        -Detail ("name={0}; timeout_ms={1}" -f $Name, $TimeoutMilliseconds)
    $result = Invoke-CapturedProcess `
        -FilePath $scPath `
        -Arguments ('queryex "{0}"' -f $Name) `
        -TimeoutMilliseconds $TimeoutMilliseconds
    $output = [string]::Join("`n", @($result.StdOut, $result.StdErr))
    if ($result.ExitCode -eq 1060) {
        return $null
    }
    if ($result.ExitCode -ne 0) {
        throw "sc.exe queryex $Name failed with exit code $($result.ExitCode). $output"
    }

    return ConvertFrom-ScQueryExOutput -Name $Name -Output $output
}

function Invoke-BoundedServiceControlCommand {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('start', 'stop')]
        [string]$Command,

        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutMilliseconds = 15000
    )

    $scPath = Join-Path $env:SystemRoot 'System32\sc.exe'
    if (-not (Test-Path -LiteralPath $scPath -PathType Leaf)) {
        throw 'The Windows Service Control Manager command (sc.exe) is unavailable.'
    }

    $result = Invoke-CapturedProcess `
        -FilePath $scPath `
        -Arguments ('{0} "{1}"' -f $Command, $Name) `
        -TimeoutMilliseconds $TimeoutMilliseconds
    $allowedExitCodes = if ($Command -eq 'start') {
        @(0, 1056)
    } else {
        @(0, 1062)
    }
    if ($result.ExitCode -notin $allowedExitCodes) {
        $output = [string]::Join("`n", @($result.StdOut, $result.StdErr))
        throw "sc.exe $Command $Name failed with exit code $($result.ExitCode). $output"
    }

    return $result
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
        $listeners = @(
            Get-NetTCPConnection -State Listen -ErrorAction Stop |
                Where-Object { [int]$_.LocalPort -eq $Port }
        )
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
        -Arguments 'opc --output json gateway-info --bridge-host 127.0.0.1:7600' `
        -TimeoutMilliseconds $TimeoutMilliseconds
    if ($result.ExitCode -ne 0) {
        throw "The OPC DA gateway smoke check failed with exit code $($result.ExitCode): $($result.StdOut) $($result.StdErr)"
    }

    $contract = Assert-GatewayReleaseContract -ContractPath $Paths.GatewayReleasePath
    return Assert-GatewayInfoPayload -Json $result.StdOut -Contract $contract
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
    $lastError = $null
    do {
        if ([DateTime]::UtcNow -ge $deadline) {
            break
        }
        $remainingMilliseconds = [int][Math]::Max(
            1000,
            [Math]::Min(5000, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        )
        try {
            $snapshot = Get-ServiceControlStatus `
                -Name $Name `
                -TimeoutMilliseconds $remainingMilliseconds
            $lastError = $null
        } catch {
            $lastError = $_.Exception
            Write-InstallerTrace `
                -Stage 'service.state.retry' `
                -Detail ("name={0}; target={1}; error={2}" -f $Name, $State, $lastError.Message)
            Start-Sleep -Milliseconds 250
            continue
        }
        if ($null -eq $snapshot) {
            throw "The service '$Name' disappeared while waiting for state '$State'."
        }
        Write-InstallerTrace `
            -Stage 'service.state.result' `
            -Detail ("name={0}; target={1}; actual={2}; pid={3}; win32_exit={4}; service_exit={5}; checkpoint={6}; wait_hint={7}" -f
                $Name,
                $State,
                $snapshot.State,
                $snapshot.ProcessId,
                $snapshot.Win32ExitCode,
                $snapshot.ServiceExitCode,
                $snapshot.Checkpoint,
                $snapshot.WaitHint)
        if ($snapshot.State -eq $State) {
            return $snapshot
        }
        if ($snapshot.State -eq 'Stopped' -and
            ($snapshot.Win32ExitCode -ne 0 -or $snapshot.ServiceExitCode -ne 0)) {
            throw (
                "The service '{0}' stopped while waiting for state '{1}' " +
                "(Win32ExitCode={2}, ServiceExitCode={3})." -f
                $Name,
                $State,
                $snapshot.Win32ExitCode,
                $snapshot.ServiceExitCode
            )
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    if ($null -ne $lastError) {
        throw "The service '$Name' did not reach state '$State' within $TimeoutSeconds seconds. Last query error: $($lastError.Message)"
    }
    throw "The service '$Name' did not reach state '$State' within $TimeoutSeconds seconds."
}

function Start-ServiceByName {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,

        [Parameter(Mandatory = $false)]
        [int]$TimeoutSeconds = 30
    )

    $current = Get-ServiceControlStatus -Name $Name
    if ($null -eq $current) {
        throw "The service '$Name' is not registered."
    }
    if ($current.State -ne 'Running') {
        Write-InstallerTrace -Stage 'service.start.begin' -Detail $Name
        Invoke-BoundedServiceControlCommand `
            -Command 'start' `
            -Name $Name `
            -TimeoutMilliseconds ([Math]::Max(1000, $TimeoutSeconds * 1000)) | Out-Null
        Write-InstallerTrace -Stage 'service.start.requested' -Detail $Name
    }
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

    Write-InstallerTrace -Stage 'service.stop.begin' -Detail $Name
    Invoke-BoundedServiceControlCommand `
        -Command 'stop' `
        -Name $Name `
        -TimeoutMilliseconds ([Math]::Max(1000, $TimeoutSeconds * 1000)) | Out-Null
    Write-InstallerTrace -Stage 'service.stop.requested' -Detail $Name
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
