# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

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
