# Windows PowerShell 5.1 private implementation; dot-sourced by BhtuneInstaller.psm1.

function Invoke-BhtuneInstaller {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $false)]
        [ValidateSet('Install', 'Uninstall')]
        [string]$Mode = 'Install',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ExpectedVersion = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ReleaseTag = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$PayloadRoot = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$GatewayPayloadRoot = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$InstallerScriptRoot = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$UninstallerSource = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$InstallRoot = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$ProgramDataRoot = '',

        [Parameter(Mandatory = $false)]
        [object]$AddToPath = '1',

        [Parameter(Mandatory = $false)]
        [object]$StartService = '1',

        [Parameter(Mandatory = $false)]
        [object]$InstallGateway = '1',

        [Parameter(Mandatory = $false)]
        [object]$StartGateway = '1',

        [Parameter(Mandatory = $false)]
        [object]$CustomDbBackupConfirmed = '0',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$TracePath = '',

        [Parameter(Mandatory = $false)]
        [switch]$TestOnly,

        [Parameter(Mandatory = $false)]
        [ValidateSet('None', 'HealthMismatch', 'GatewaySmokeFailure', 'CommitFailure')]
        [string]$FailureInjection = 'None',

        [Parameter(Mandatory = $false)]
        [switch]$LeaveInstallRoot,

        [Parameter(Mandatory = $false)]
        [switch]$FinalizeUninstall,

        [Parameter(Mandatory = $false)]
        [int]$WaitForProcessId = 0,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$WaitForProcessPath = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$FinalizerRoot = '',

        [Parameter(Mandatory = $false)]
        [switch]$IsolatedLifecycleTest,

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$LifecycleTestId = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$LifecycleTestRoot = '',

        [Parameter(Mandatory = $false)]
        [AllowEmptyString()]
        [string]$EntryScriptPath = '',

        [Parameter(Mandatory = $true)]
        [ref]$ExitCode
    )

    $ErrorActionPreference = 'Stop'
    $previousTracePath = $script:InstallerTracePath
    $previousScriptPath = $script:InstallerScriptPath
    $previousLifecycleTestId = $script:InstallerLifecycleTestId
    $previousLifecycleTestRoot = $script:InstallerLifecycleTestRoot
    $previousLifecycleTestActive = $script:InstallerLifecycleTestActive
    $previousServiceName = $script:InstallerServiceName
    $previousGatewayServiceName = $script:GatewayServiceName
    $previousMarkerPath = $script:InstallerMarkerPath
    $previousUninstallPath = $script:InstallerUninstallPath
    $script:InstallerTracePath = $TracePath
    $script:InstallerScriptPath = if ([string]::IsNullOrWhiteSpace($EntryScriptPath)) {
        Join-Path $script:InstallerModuleRoot 'Install-Bhtune.ps1'
    } else {
        $EntryScriptPath
    }

    try {
        $AddToPath = ConvertTo-InstallerBoolean -Value $AddToPath -Name 'AddToPath'
        $StartService = ConvertTo-InstallerBoolean -Value $StartService -Name 'StartService'
        $InstallGateway = ConvertTo-InstallerBoolean -Value $InstallGateway -Name 'InstallGateway'
        $StartGateway = ConvertTo-InstallerBoolean -Value $StartGateway -Name 'StartGateway'
        $CustomDbBackupConfirmed = ConvertTo-InstallerBoolean -Value $CustomDbBackupConfirmed -Name 'CustomDbBackupConfirmed'

        $transactionLock = $null
        try {
            Write-InstallerTrace -Stage 'script.begin' -Detail ("mode={0}; testOnly={1}" -f $Mode, $TestOnly)
            Assert-FailureInjectionPolicy `
                -FailureInjection $FailureInjection `
                -TestOnly ([bool]$TestOnly) `
                -AllowNoFailureInjection ([bool]$IsolatedLifecycleTest) | Out-Null
            if ($IsolatedLifecycleTest) {
                if (-not $TestOnly) {
                    throw 'The isolated lifecycle context is valid only in test-only mode.'
                }
                Set-InstallerLifecycleTestContext `
                    -LifecycleTestId $LifecycleTestId `
                    -LifecycleTestRoot $LifecycleTestRoot `
                    -InstallRoot $InstallRoot `
                    -ProgramDataRoot $ProgramDataRoot
            } elseif (-not [string]::IsNullOrWhiteSpace($LifecycleTestId) -or
                -not [string]::IsNullOrWhiteSpace($LifecycleTestRoot)) {
                throw 'Lifecycle test identity and root require -IsolatedLifecycleTest.'
            }

            Write-InstallerTrace -Stage 'script.options.validated' -Detail ("mode={0}; finalize={1}" -f $Mode, $FinalizeUninstall)
            if ($FinalizeUninstall -and $Mode -ne 'Uninstall') {
                throw 'The uninstall finalization switch is valid only with -Mode Uninstall.'
            }
            if ($WaitForProcessId -gt 0) {
                if ($Mode -ne 'Uninstall' -or -not $FinalizeUninstall) {
                    throw 'Waiting for a parent process is valid only for uninstall finalization.'
                }
                Write-InstallerTrace -Stage 'uninstall.finalizer.wait-begin' -Detail ("pid={0}; expected={1}" -f `
                        $WaitForProcessId, $WaitForProcessPath)
                Wait-ForParentProcessExit `
                    -ProcessId $WaitForProcessId `
                    -ExpectedPath $WaitForProcessPath
                Write-InstallerTrace -Stage 'uninstall.finalizer.wait-end' -Detail ([string]$WaitForProcessId)
            }
            if ($Mode -eq 'Install') {
                if ([string]::IsNullOrWhiteSpace($PayloadRoot)) {
                    throw 'Install mode requires -PayloadRoot.'
                }
                if ([string]::IsNullOrWhiteSpace($InstallerScriptRoot)) {
                    throw 'Install mode requires -InstallerScriptRoot.'
                }
                if ([string]::IsNullOrWhiteSpace($UninstallerSource)) {
                    throw 'Install mode requires -UninstallerSource.'
                }
                if (-not [bool]$TestOnly -and [string]::IsNullOrWhiteSpace($ReleaseTag)) {
                    throw 'Install mode requires -ReleaseTag=vX.Y.Z.'
                }

                $versionContract = Assert-InstallerVersionContract -ExpectedVersion $ExpectedVersion -ReleaseTag $ReleaseTag
                $paths = Get-InstallerPaths -InstallRoot $InstallRoot -ProgramDataRoot $ProgramDataRoot
                Write-InstallerTrace -Stage 'script.paths.resolved' -Detail $paths.InstallRoot
                if (-not [bool]$TestOnly) {
                    Assert-InstallerFixedPaths -Paths $paths | Out-Null
                }
                $transactionLock = Enter-InstallerTransactionLock
                Write-InstallerTrace -Stage 'transaction.lock.acquired' -Detail $paths.InstallerStateRoot
                Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
                $recovery = Recover-InterruptedTransaction -Paths $paths -FinalizeUninstall:$false
                Write-InstallerTrace -Stage 'transaction.recovery.completed' -Detail $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Action })
                if ($null -ne $recovery -and [string]$recovery.Action -eq 'ResumeUninstall') {
                    throw "An uninstall transaction is still finalizing for '$($paths.InstallRoot)'. Wait for the uninstall finalizer to finish, or rerun the uninstaller before installing BHTune again."
                }
                # NSIS removes the old service and Program Files tree before
                # this transaction on upgrade. Existing ownership metadata in
                # ProgramData authorizes reuse of that state.
                $priorState = Assert-InstallerState `
                    -Paths $paths `
                    -AllowMissingService:$true `
                    -AllowMissingInstallRoot:$true
                Invoke-InstallTransaction `
                    -Paths $paths `
                    -PriorState $priorState `
                    -VersionContract $versionContract `
                    -PayloadRoot $PayloadRoot `
                    -GatewayPayloadRoot $GatewayPayloadRoot `
                    -InstallerScriptRoot $InstallerScriptRoot `
                    -UninstallerSource $UninstallerSource `
                    -AddToPath $AddToPath `
                    -StartService $StartService `
                    -InstallGateway $InstallGateway `
                    -StartGateway $StartGateway `
                    -CustomDbBackupConfirmed $CustomDbBackupConfirmed `
                    -TestOnly ([bool]$TestOnly) `
                    -FailureInjection $FailureInjection
            } else {
                $paths = Get-InstallerPaths -InstallRoot $InstallRoot -ProgramDataRoot $ProgramDataRoot
                Write-InstallerTrace -Stage 'script.paths.resolved' -Detail $paths.InstallRoot
                if (-not [bool]$TestOnly) {
                    Assert-InstallerFixedPaths -Paths $paths | Out-Null
                }
                $transactionLock = Enter-InstallerTransactionLock
                Write-InstallerTrace -Stage 'transaction.lock.acquired' -Detail $paths.InstallerStateRoot
                Assert-InstallerEnvironmentPolicy -Paths $paths | Out-Null
                $recovery = Recover-InterruptedTransaction `
                    -Paths $paths `
                    -FinalizeUninstall:([bool]$FinalizeUninstall)
                Write-InstallerTrace -Stage 'transaction.recovery.completed' -Detail $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Action })
                if ($null -eq $recovery -or [string]$recovery.Action -ne 'Finalized') {
                    Write-InstallerTrace `
                        -Stage 'uninstall.orchestration.begin' `
                        -Detail ("phase={0}; finalize={1}" -f `
                            $(if ($null -eq $recovery) { 'none' } else { [string]$recovery.Phase }), $FinalizeUninstall)
                    Invoke-ConservativeUninstall `
                        -Paths $paths `
                        -LeaveInstallRoot ([bool]$LeaveInstallRoot) `
                        -FinalizeUninstall ([bool]$FinalizeUninstall)
                    Write-InstallerTrace -Stage 'uninstall.orchestration.end' -Detail $paths.InstallRoot
                }
            }

            if ($FinalizeUninstall -and -not [string]::IsNullOrWhiteSpace($FinalizerRoot)) {
                Remove-Item -LiteralPath $FinalizerRoot -Recurse -Force -ErrorAction SilentlyContinue
                if (Test-Path -LiteralPath $FinalizerRoot) {
                    Write-Warning "The uninstall finalizer completed, but its temporary staging root remains at '$FinalizerRoot'."
                    Write-InstallerTrace -Stage 'uninstall.finalizer.cleanup-incomplete' -Detail $FinalizerRoot
                } else {
                    Write-InstallerTrace -Stage 'uninstall.finalizer.cleaned' -Detail $FinalizerRoot
                }
            }
            $ExitCode.Value = 0
            return
        } catch {
            Write-InstallerTrace -Stage 'script.failure' -Detail $_.Exception.Message
            Write-Error -Message $_.Exception.Message -ErrorAction Continue
            $ExitCode.Value = 1
            return
        } finally {
            Exit-InstallerTransactionLock -Lock $transactionLock
        }
    } finally {
        $script:InstallerTracePath = $previousTracePath
        $script:InstallerScriptPath = $previousScriptPath
        $script:InstallerLifecycleTestId = $previousLifecycleTestId
        $script:InstallerLifecycleTestRoot = $previousLifecycleTestRoot
        $script:InstallerLifecycleTestActive = $previousLifecycleTestActive
        $script:InstallerServiceName = $previousServiceName
        $script:GatewayServiceName = $previousGatewayServiceName
        $script:InstallerMarkerPath = $previousMarkerPath
        $script:InstallerUninstallPath = $previousUninstallPath
    }
}
