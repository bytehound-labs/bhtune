# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Get-InstallerModulePayloadRelativePaths {
    return @(
        'BhtuneInstaller.psm1'
        'Install-Bhtune.ps1'
    ) + @($script:InstallerPrivateFiles)
}

function Assert-InstallerModulePayload {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceRoot
    )

    if (-not (Test-Path -LiteralPath $SourceRoot -PathType Container)) {
        throw "Installer module source root is missing: $SourceRoot"
    }

    foreach ($relativePath in @(Get-InstallerModulePayloadRelativePaths)) {
        $path = Join-Path $SourceRoot $relativePath
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Installer module payload is missing '$relativePath' under '$SourceRoot'."
        }
    }

    $privateRoot = Join-Path $SourceRoot 'Private'
    $actualPrivateFiles = @(
        if (Test-Path -LiteralPath $privateRoot -PathType Container) {
            Get-ChildItem -LiteralPath $privateRoot -File -Recurse -Force |
                ForEach-Object {
                    $_.FullName.Substring($SourceRoot.Length).TrimStart([char[]]@('\', '/')).Replace('\', '/')
                }
        }
    )
    $expectedPrivateFiles = @($script:InstallerPrivateFiles | Sort-Object -CaseSensitive)
    $actualPrivateFiles = @($actualPrivateFiles | Sort-Object -CaseSensitive)
    if (($expectedPrivateFiles -join '|') -cne ($actualPrivateFiles -join '|')) {
        throw "Installer private source files do not match the module manifest under '$SourceRoot'."
    }
}

function Copy-InstallerModulePayload {
    param(
        [Parameter(Mandatory = $true)]
        [string]$SourceRoot,

        [Parameter(Mandatory = $true)]
        [string]$DestinationRoot
    )

    Assert-InstallerModulePayload -SourceRoot $SourceRoot
    foreach ($relativePath in @(Get-InstallerModulePayloadRelativePaths)) {
        $sourcePath = Join-Path $SourceRoot $relativePath
        $destinationPath = Join-Path $DestinationRoot $relativePath
        $destinationParent = Split-Path -Parent $destinationPath
        $sourceHash = Get-FileSha256 -Path $sourcePath
        New-Item -ItemType Directory -Path $destinationParent -Force | Out-Null
        Copy-Item -LiteralPath $sourcePath -Destination $destinationPath -Force
        if ((Get-FileSha256 -Path $destinationPath) -cne $sourceHash) {
            throw "Installer module copy failed to verify for '$relativePath'."
        }
    }
    Assert-InstallerModulePayload -SourceRoot $DestinationRoot
}
