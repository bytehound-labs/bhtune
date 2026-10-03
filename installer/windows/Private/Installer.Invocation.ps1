# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Read-InstallerInvocationFile {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Installer invocation file is missing: $Path"
    }

    $allowedKeys = @(
        'Mode',
        'ExpectedVersion',
        'ReleaseTag',
        'PayloadRoot',
        'GatewayPayloadRoot',
        'InstallerScriptRoot',
        'UninstallerSource',
        'InstallRoot',
        'ProgramDataRoot',
        'AddToPath',
        'StartService',
        'InstallGateway',
        'StartGateway',
        'CustomDbBackupConfirmed',
        'TracePath',
        'TestOnly',
        'IsolatedLifecycleTest',
        'LifecycleTestId',
        'LifecycleTestRoot'
    )
    $requiredKeys = @(
        'Mode',
        'ExpectedVersion',
        'ReleaseTag',
        'PayloadRoot',
        'GatewayPayloadRoot',
        'InstallerScriptRoot',
        'UninstallerSource',
        'InstallRoot',
        'ProgramDataRoot',
        'AddToPath',
        'StartService',
        'InstallGateway',
        'StartGateway',
        'CustomDbBackupConfirmed',
        'TracePath'
    )
    $values = @{}
    $sectionFound = $false
    $reader = New-Object System.IO.StreamReader($Path, [System.Text.Encoding]::Default, $true)
    try {
        $contents = $reader.ReadToEnd()
    } finally {
        $reader.Dispose()
    }

    foreach ($line in ($contents -split "\r?\n")) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line -match '^\s*[;#]') {
            continue
        }
        if ($line -match '^\s*\[(?<section>[^\]]+)\]\s*$') {
            if ($sectionFound -or $Matches.section -cne 'Install') {
                throw 'Installer invocation file must contain one [Install] section.'
            }
            $sectionFound = $true
            continue
        }
        if (-not $sectionFound) {
            throw 'Installer invocation file must begin with an [Install] section.'
        }

        $separator = $line.IndexOf('=')
        if ($separator -le 0) {
            throw 'Installer invocation file contains a malformed setting.'
        }
        $name = $line.Substring(0, $separator).Trim()
        $value = $line.Substring($separator + 1)
        if ($name -notin $allowedKeys) {
            throw "Installer invocation file contains an unsupported setting: $name"
        }
        if ($values.ContainsKey($name)) {
            throw "Installer invocation file contains a duplicate setting: $name"
        }
        $values[$name] = $value
    }

    if (-not $sectionFound) {
        throw 'Installer invocation file is missing its [Install] section.'
    }
    foreach ($name in $requiredKeys) {
        if (-not $values.ContainsKey($name) -or [string]::IsNullOrWhiteSpace([string]$values[$name])) {
            throw "Installer invocation file is missing a required setting: $name"
        }
    }
    if ([string]$values['Mode'] -cne 'Install') {
        throw 'Installer invocation file must request Install mode.'
    }
    foreach ($name in @('AddToPath', 'StartService', 'InstallGateway', 'StartGateway', 'CustomDbBackupConfirmed')) {
        if ([string]$values[$name] -cnotin @('0', '1')) {
            throw "Installer invocation setting '$name' must be 0 or 1."
        }
    }
    foreach ($name in @('TestOnly', 'IsolatedLifecycleTest')) {
        if ($values.ContainsKey($name)) {
            if ([string]$values[$name] -cnotin @('true', 'false')) {
                throw "Installer invocation setting '$name' must be true or false."
            }
            $values[$name] = [string]$values[$name] -ceq 'true'
        }
    }

    $testOnly = $values.ContainsKey('TestOnly') -and [bool]$values['TestOnly']
    $isolatedTest = $values.ContainsKey('IsolatedLifecycleTest') -and [bool]$values['IsolatedLifecycleTest']
    if ($testOnly -ne $isolatedTest) {
        throw 'Lifecycle test invocation must enable both test-only switches.'
    }
    if ($testOnly) {
        foreach ($name in @('LifecycleTestId', 'LifecycleTestRoot')) {
            if (-not $values.ContainsKey($name) -or [string]::IsNullOrWhiteSpace([string]$values[$name])) {
                throw "Lifecycle test invocation is missing a required setting: $name"
            }
        }
        if ([string]$values['LifecycleTestId'] -cnotmatch '^[0-9a-f]{32}$') {
            throw 'Lifecycle test invocation ID must be 32 lowercase hexadecimal characters.'
        }
    } elseif ($values.ContainsKey('LifecycleTestId') -or $values.ContainsKey('LifecycleTestRoot')) {
        throw 'Lifecycle test paths are not valid in a production installer invocation.'
    }

    return ,$values
}
