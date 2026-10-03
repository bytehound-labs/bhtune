# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Get-SnapshotValue {
    param(
        [Parameter(Mandatory = $false)]
        [psobject]$Snapshot,

        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    if ($null -eq $Snapshot) {
        return $null
    }

    if ($Snapshot -is [System.Collections.IDictionary]) {
        return $Snapshot[$Name]
    }

    $property = $Snapshot.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function Write-TextFile {
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
    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
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
        [bool]$TestOnly = $false,

        [Parameter(Mandatory = $false)]
        [bool]$AllowNoFailureInjection = $false
    )

    if ($FailureInjection -ne 'None' -and -not $TestOnly) {
        throw "Failure-injection '$FailureInjection' is test-only and is rejected in normal installer mode."
    }

    if ($TestOnly -and $FailureInjection -eq 'None' -and -not $AllowNoFailureInjection) {
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
