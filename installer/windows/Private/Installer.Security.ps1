# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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

    $installerScriptTargets = @(
        $Paths.InstallerScriptRoot,
        (Join-Path $Paths.InstallerScriptRoot 'Private')
    ) + @(
        Get-InstallerModulePayloadRelativePaths |
            ForEach-Object {
                Join-Path $Paths.InstallerScriptRoot $_
            }
    )
    foreach ($path in @(
            (Join-Path $Paths.InstallRoot 'bhtune.exe'),
            (Join-Path $Paths.InstallRoot 'bhtune-server.exe'),
            (Join-Path $Paths.InstallRoot 'LICENSE'),
            (Join-Path $Paths.InstallRoot 'README.md')
        ) + $installerScriptTargets) {
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
