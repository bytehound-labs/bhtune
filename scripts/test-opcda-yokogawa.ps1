#Requires -Version 7.4
[CmdletBinding()]
param(
    [string]$ArtifactsDirectory = 'target/opcda-yokogawa-smoke',
    [ValidateSet('User', 'Machine')]
    [string]$RegistrationScope = 'User'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows) {
    throw 'The native OPC DA smoke runner requires Windows and PowerShell 7.4 or later.'
}

$repositoryRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$artifactsRoot = [IO.Path]::GetFullPath($ArtifactsDirectory, $repositoryRoot)
if (-not $artifactsRoot.StartsWith("$repositoryRoot\", [StringComparison]::OrdinalIgnoreCase)) {
    throw 'ArtifactsDirectory must be a directory inside this checkout.'
}
for ($directory = $artifactsRoot; $directory -ne $repositoryRoot; $directory = [IO.Path]::GetDirectoryName($directory)) {
    if ((Test-Path -LiteralPath $directory) -and
        ((Get-Item -LiteralPath $directory -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing an artifacts path containing a reparse point: $directory"
    }
}

$principal = [Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
$isAdministrator = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$isGithubHosted = $env:GITHUB_ACTIONS -eq 'true' -and $env:RUNNER_ENVIRONMENT -eq 'github-hosted'
if ($RegistrationScope -eq 'User') {
    $policy = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
        [Microsoft.Win32.RegistryHive]::LocalMachine, [Microsoft.Win32.RegistryView]::Registry64
    )
    try {
        $uac = $policy.OpenSubKey('SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System')
        try {
            if ($isAdministrator -or ($null -ne $uac -and $uac.GetValue('EnableLUA', 1) -eq 0)) {
                throw 'User-scope COM activation requires a non-elevated shell with UAC enabled. No elevation or machine-scope fallback is performed.'
            }
        } finally {
            if ($null -ne $uac) { $uac.Dispose() }
        }
    } finally {
        $policy.Dispose()
    }
} elseif (-not $isAdministrator) {
    throw 'Explicit Machine registration requires administrator permissions and a disposable test host. No elevation is performed.'
}

$tools = @{}
foreach ($name in @('cargo', 'rustc', 'git', 'gh', 'cosign')) {
    $tools[$name] = (Get-Command -Name $name -CommandType Application -ErrorAction Stop |
        Select-Object -First 1).Source
}
Import-Module -Name (Join-Path $repositoryRoot 'installer/windows/BhtuneInstaller.psm1') -ErrorAction Stop

# Suspended creation closes the race between spawning a native child and assigning its job.
# Closing the runner also closes these non-inheritable jobs and kills their entire trees.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;
using System.Threading;
using Microsoft.Win32;
using Microsoft.Win32.SafeHandles;

public sealed class BhtuneSmokeProcess : IDisposable
{
    [StructLayout(LayoutKind.Sequential)]
    struct SecurityAttributes { public int Length; public IntPtr Descriptor; public int Inherit; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct StartupInfo {
        public int Size; public string Reserved, Desktop, Title;
        public uint X, Y, XSize, YSize, XChars, YChars, Fill, Flags;
        public ushort Show, ReservedSize;
        public IntPtr ReservedData, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct ProcessInfo { public IntPtr Process, Thread; public uint Id, ThreadId; }
    [StructLayout(LayoutKind.Sequential)]
    struct BasicLimits {
        public long ProcessTime, JobTime; public uint Flags;
        public UIntPtr MinWorkingSet, MaxWorkingSet; public uint ActiveLimit;
        public UIntPtr Affinity; public uint Priority, Scheduling;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct IoCounters { public ulong ReadOps, WriteOps, OtherOps, ReadBytes, WriteBytes, OtherBytes; }
    [StructLayout(LayoutKind.Sequential)]
    struct ExtendedLimits {
        public BasicLimits Basic; public IoCounters Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct Accounting {
        public long UserTime, KernelTime, PeriodUserTime, PeriodKernelTime;
        public uint PageFaults, TotalProcesses, ActiveProcesses, TerminatedProcesses;
    }
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr job, int kind, ref ExtendedLimits limits, int size);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool QueryInformationJobObject(IntPtr job, int kind, out Accounting info, int size, IntPtr length);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateJobObject(IntPtr job, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern IntPtr CreateFileW(string path, uint access, uint sharing, ref SecurityAttributes attributes,
        uint disposition, uint flags, IntPtr template);
    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    static extern bool CreateProcessW(string file, StringBuilder command, IntPtr processAttributes,
        IntPtr threadAttributes, bool inherit, uint flags, IntPtr environment, string directory,
        ref StartupInfo startup, out ProcessInfo process);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode)]
    static extern int RegCreateKeyExW(IntPtr hive, string path, int reserved, string keyClass, int options,
        int access, IntPtr attributes, out SafeRegistryHandle key, out int disposition);

    IntPtr job, process;
    public uint Id { get; private set; }
    public string CommandLine { get; private set; }

    static void Check(bool success, string operation) {
        if (!success) {
            int error = Marshal.GetLastWin32Error();
            throw new Win32Exception(error, operation + " failed (Win32 " + error + "): " +
                new Win32Exception(error).Message);
        }
    }
    static string Quote(string value) {
        if (value.Length != 0 && !Regex.IsMatch(value, "[\\s\"]")) return value;
        return "\"" + Regex.Replace(Regex.Replace(value, "(\\\\*)\"", "$1$1\\\""), "(\\\\+)$", "$1$1") + "\"";
    }
    public static RegistryKey CreateRegistry32Key(RegistryHive hive, string path) {
        SafeRegistryHandle handle;
        int disposition;
        int error = RegCreateKeyExW(new IntPtr((int)hive), path, 0, null, 0, 0x2021f,
            IntPtr.Zero, out handle, out disposition);
        if (error != 0) {
            if (handle != null) handle.Dispose();
            throw new Win32Exception(error, "RegCreateKeyExW(" + hive + "\\" + path +
                ") failed (Win32 " + error + "): " + new Win32Exception(error).Message);
        }
        if (disposition != 1) {
            handle.Dispose();
            throw new IOException("Refusing an existing COM key: " + hive + "\\" + path);
        }
        return RegistryKey.FromHandle(handle, RegistryView.Registry32);
    }
    public static BhtuneSmokeProcess Start(string file, string[] arguments, string directory, string stdout, string stderr) {
        var child = new BhtuneSmokeProcess();
        IntPtr input = IntPtr.Zero, output = IntPtr.Zero, error = IntPtr.Zero, thread = IntPtr.Zero;
        try {
            child.job = CreateJobObjectW(IntPtr.Zero, null);
            Check(child.job != IntPtr.Zero, "CreateJobObjectW");
            var limits = new ExtendedLimits();
            limits.Basic.Flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE; no breakaway.
            Check(SetInformationJobObject(child.job, 9, ref limits, Marshal.SizeOf<ExtendedLimits>()),
                "SetInformationJobObject");
            var attributes = new SecurityAttributes { Length = Marshal.SizeOf<SecurityAttributes>(), Inherit = 1 };
            input = CreateFileW("NUL", 0x80000000, 3, ref attributes, 3, 0, IntPtr.Zero);
            Check(input != new IntPtr(-1), "CreateFileW(stdin NUL)");
            output = CreateFileW(stdout, 0x40000000, 1, ref attributes, 2, 0, IntPtr.Zero);
            Check(output != new IntPtr(-1), "CreateFileW(stdout " + stdout + ")");
            error = CreateFileW(stderr, 0x40000000, 1, ref attributes, 2, 0, IntPtr.Zero);
            Check(error != new IntPtr(-1), "CreateFileW(stderr " + stderr + ")");
            child.CommandLine = Quote(file);
            foreach (string argument in arguments) child.CommandLine += " " + Quote(argument);
            var startup = new StartupInfo {
                Size = Marshal.SizeOf<StartupInfo>(), Flags = 0x100, Input = input, Output = output, Error = error
            };
            ProcessInfo created;
            Check(CreateProcessW(file, new StringBuilder(child.CommandLine), IntPtr.Zero, IntPtr.Zero, true,
                0x08000004, IntPtr.Zero, directory, ref startup, out created),
                "CreateProcessW(executable " + file + ", directory " + directory +
                ", command " + child.CommandLine + ")"); // NO_WINDOW | SUSPENDED.
            child.process = created.Process;
            child.Id = created.Id;
            thread = created.Thread;
            Check(AssignProcessToJobObject(child.job, child.process), "AssignProcessToJobObject");
            Check(ResumeThread(thread) != UInt32.MaxValue, "ResumeThread");
            return child;
        } catch (Exception launchError) {
            if (child.process != IntPtr.Zero) {
                TerminateProcess(child.process, 1);
                WaitForSingleObject(child.process, 10000);
            }
            try { child.Dispose(); }
            catch (Exception cleanupError) {
                throw new AggregateException("Native process launch and cleanup failed.", launchError, cleanupError);
            }
            throw;
        } finally {
            foreach (IntPtr handle in new[] { input, output, error, thread })
                if (handle != IntPtr.Zero && handle != new IntPtr(-1)) CloseHandle(handle);
        }
    }
    public bool Wait(int milliseconds) {
        uint result = WaitForSingleObject(process, (uint)milliseconds);
        Check(result != UInt32.MaxValue, "WaitForSingleObject");
        return result == 0;
    }
    public uint ExitCode {
        get { uint code; Check(GetExitCodeProcess(process, out code), "GetExitCodeProcess"); return code; }
    }
    public void Stop() {
        if (job == IntPtr.Zero) return;
        Accounting info;
        Check(QueryInformationJobObject(job, 1, out info, Marshal.SizeOf<Accounting>(), IntPtr.Zero),
            "QueryInformationJobObject");
        if (info.ActiveProcesses != 0) Check(TerminateJobObject(job, 1), "TerminateJobObject");
        var deadline = Stopwatch.StartNew();
        do {
            Check(QueryInformationJobObject(job, 1, out info, Marshal.SizeOf<Accounting>(), IntPtr.Zero),
                "QueryInformationJobObject");
            if (info.ActiveProcesses == 0) {
                if (process != IntPtr.Zero && !Wait(10000)) throw new TimeoutException("Owned root process was not reaped.");
                return;
            }
            Thread.Sleep(50);
        } while (deadline.ElapsedMilliseconds < 10000);
        throw new TimeoutException("Owned native process tree did not stop.");
    }
    public void Dispose() {
        try { Stop(); }
        finally {
            if (process != IntPtr.Zero) { CloseHandle(process); process = IntPtr.Zero; }
            if (job != IntPtr.Zero) { CloseHandle(job); job = IntPtr.Zero; }
        }
    }
}
'@

$runId = [guid]::NewGuid().ToString('N')
New-Item -ItemType Directory -Force -Path $artifactsRoot | Out-Null
$workRoot = Join-Path $artifactsRoot ".work-$runId"
$evidenceRoot = Join-Path $artifactsRoot "run-$runId"
$stateRoot = Join-Path $workRoot 'state'
$commandLogs = Join-Path $evidenceRoot 'commands'
New-Item -ItemType Directory -Path $workRoot, $evidenceRoot, $stateRoot, $commandLogs | Out-Null
Set-Content -LiteralPath (Join-Path $workRoot '.owner') -Value $runId -Encoding utf8NoBOM
$processes = [Collections.Generic.List[object]]::new()
$ownedKeys = [Collections.Generic.List[object]]::new()
$cleanupErrors = [Collections.Generic.List[string]]::new()
$firewall = [pscustomobject]@{
    name = "BHTuneOpcDaSmoke-$runId"
    owner = "BHTune native OPC DA smoke $runId"
    program = $null
    attempted = $false
    created = $false
    removed = $false
    protection = 'not_established'
    profiles = @()
    active_rule = $null
}
$environmentBefore = @{}
foreach ($name in @('BHTUNE_OPCDA_SMOKE_HOST', 'BHTUNE_OPCDA_SMOKE_DIR', 'BHTUNE_OPCDA_FIXTURE_AUDIT', 'BHTUNE_OPCDA_SMOKE_GATEWAY_VERSION')) {
    $environmentBefore[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
$evidence = [ordered]@{
    schema_version = 1
    run_id = $runId
    started_at_utc = [DateTime]::UtcNow.ToString('o')
    status = 'failed'
    registration_scope = $RegistrationScope
    registry_view = 'Registry32'
    github_hosted = $isGithubHosted
    firewall = $firewall
    processes = $processes
    registry_keys = $ownedKeys
    truncated_logs = [Collections.Generic.List[string]]::new()
}

function Start-SmokeProcess {
    param([string]$Name, [string]$File, [string[]]$Arguments)
    $native = [BhtuneSmokeProcess]::Start(
        $File, $Arguments, $repositoryRoot,
        (Join-Path $commandLogs "$Name.stdout"), (Join-Path $commandLogs "$Name.stderr")
    )
    $entry = [pscustomobject]@{
        name = $Name
        pid = $native.Id
        executable = $File
        command_line = $native.CommandLine
        sha256 = $null
        size_bytes = $null
        started_at_utc = [DateTime]::UtcNow.ToString('o')
        stopped = $false
        timed_out = $false
        exit_code = $null
        native = $native
    }
    $processes.Add($entry)
    $entry.sha256 = (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()
    $entry.size_bytes = (Get-Item -LiteralPath $File).Length
    return $entry
}

function Stop-SmokeProcess {
    param([object]$Entry)
    if ($Entry.stopped) { return }
    try {
        $Entry.native.Stop()
        $Entry.exit_code = $Entry.native.ExitCode
        $Entry.stopped = $true
    } finally {
        $Entry.native.Dispose()
    }
}

function Invoke-SmokeCommand {
    param([string]$Name, [string]$File, [string[]]$Arguments, [int]$TimeoutSeconds = 60)
    $entry = Start-SmokeProcess -Name $Name -File $File -Arguments $Arguments
    try {
        if (-not $entry.native.Wait($TimeoutSeconds * 1000)) {
            $entry.timed_out = $true
            throw "$Name exceeded its $TimeoutSeconds second deadline. See $commandLogs."
        }
        $exitCode = $entry.native.ExitCode
    } finally {
        Stop-SmokeProcess -Entry $entry
    }
    if ($exitCode -ne 0) {
        throw "$Name failed with exit code $exitCode. See $commandLogs."
    }
    return [IO.File]::ReadAllText((Join-Path $commandLogs "$Name.stdout")).Trim()
}

function Get-SmokeListeners {
    param([int]$Port)
    return @(Get-NetTCPConnection -State Listen -ErrorAction Stop | Where-Object { $_.LocalPort -eq $Port })
}

function Assert-NoFixtureRegistration {
    param([string]$ClsidPath, [string]$ProgIdPath)
    foreach ($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryHive]::LocalMachine)) {
        $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, [Microsoft.Win32.RegistryView]::Registry32)
        try {
            foreach ($path in @($ClsidPath, $ProgIdPath)) {
                $key = $base.OpenSubKey($path)
                if ($null -ne $key) {
                    $key.Dispose()
                    throw "Refusing colliding COM registration in $hive (Registry32): $path"
                }
            }
        } finally {
            $base.Dispose()
        }
    }
}

function Get-SmokeDiscoveryRegistration {
    $registrations = [Collections.Generic.List[object]]::new()
    foreach ($hive in @([Microsoft.Win32.RegistryHive]::CurrentUser, [Microsoft.Win32.RegistryHive]::LocalMachine)) {
        foreach ($view in @([Microsoft.Win32.RegistryView]::Registry32, [Microsoft.Win32.RegistryView]::Registry64)) {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey($hive, $view)
            $key = $null
            $mapping = $null
            try {
                $key = $base.OpenSubKey('Software\Classes\OPC.ServerList.1')
                if ($null -ne $key) {
                    $mapping = $key.OpenSubKey('CLSID')
                    $registrations.Add([pscustomobject]@{
                        hive = $hive.ToString()
                        view = $view.ToString()
                        clsid = if ($null -ne $mapping) { [string]$mapping.GetValue('', '') } else { '' }
                    })
                }
            } finally {
                if ($null -ne $mapping) { $mapping.Dispose() }
                if ($null -ne $key) { $key.Dispose() }
                $base.Dispose()
            }
        }
    }
    return @($registrations)
}

function Assert-SmokeFirewallRule {
    param([object]$Rule, [switch]$RequireEffective)
    if ($Rule.Name -cne $firewall.name -or $Rule.Description -cne $firewall.owner) {
        throw 'Firewall rule ownership changed; refusing to use or remove it.'
    }
    $applications = @($Rule | Get-NetFirewallApplicationFilter -ErrorAction Stop)
    if ($applications.Count -ne 1 -or [string]$applications[0].Program -ine $firewall.program) {
        throw 'Firewall rule executable scope changed; refusing to use or remove it.'
    }
    if ($RequireEffective) {
        # ActiveStore may return enum labels or raw CIM values (Full=1, InactiveProfile=5).
        $status = @($Rule.EnforcementStatus | ForEach-Object { [string]$_ })
        if ([string]$Rule.Enabled -ne 'True' -or [string]$Rule.Direction -ne 'Inbound' -or
            [string]$Rule.Action -ne 'Block' -or [string]$Rule.Profile -ne 'Any' -or
            ($status -notcontains 'Full' -and $status -notcontains '1') -or
            @($status | Where-Object { $_ -notin @('Full', '1', 'InactiveProfile', '5') }).Count -ne 0) {
            throw "The executable-scoped inbound block is not effective in the active firewall policy: enabled=$($Rule.Enabled), direction=$($Rule.Direction), action=$($Rule.Action), profile=$($Rule.Profile), enforcement=$($status -join ',')."
        }
        $ports = @($Rule | Get-NetFirewallPortFilter -ErrorAction Stop)
        $addresses = @($Rule | Get-NetFirewallAddressFilter -ErrorAction Stop)
        if ($ports.Count -ne 1 -or $addresses.Count -ne 1 -or
            [string]$ports[0].Protocol -notin @('Any', '256') -or
            [string]$ports[0].LocalPort -ne 'Any' -or [string]$ports[0].RemotePort -ne 'Any' -or
            [string]$addresses[0].LocalAddress -ne 'Any' -or [string]$addresses[0].RemoteAddress -ne 'Any') {
            throw 'The active gateway inbound block has an unexpected port or address restriction.'
        }
    }
}

$failure = $null
try {
    $progId = 'ByteHound.BHTuneOpcDaSmoke.1'
    $clsid = '{0E444026-A6C9-4AAF-BA49-1203649745B1}'
    $clsidPath = "Software\Classes\CLSID\$clsid"
    $progIdPath = "Software\Classes\$progId"
    Assert-NoFixtureRegistration -ClsidPath $clsidPath -ProgIdPath $progIdPath
    $evidence.source_commit = Invoke-SmokeCommand -Name source -File $tools.git -Arguments @('rev-parse', 'HEAD')
    $worktreeStatus = Invoke-SmokeCommand -Name source-status -File $tools.git -Arguments @('status', '--porcelain', '--untracked-files=normal')
    $evidence.source_worktree_dirty = -not [string]::IsNullOrWhiteSpace($worktreeStatus)
    $evidence.fixture_toolchain = Invoke-SmokeCommand -Name fixture-toolchain -File $tools.rustc -Arguments @('+stable-x86_64-pc-windows-msvc', '--version', '--verbose')
    $evidence.cli_toolchain = Invoke-SmokeCommand -Name cli-toolchain -File $tools.rustc -Arguments @('--version', '--verbose')
    $manifest = Join-Path $repositoryRoot 'tests/opcda-yokogawa-fixture/Cargo.toml'
    $fixtureTarget = Join-Path $repositoryRoot 'target/opcda-yokogawa-fixture'
    $auditPath = Join-Path $stateRoot 'fixture-writes.jsonl'
    [IO.File]::WriteAllText($auditPath, '')
    [Environment]::SetEnvironmentVariable('BHTUNE_OPCDA_FIXTURE_AUDIT', $auditPath, 'Process')
    Invoke-SmokeCommand -Name fixture-tests -File $tools.cargo -TimeoutSeconds 600 -Arguments @(
        '+stable-x86_64-pc-windows-msvc', 'test', '--manifest-path', $manifest, '--locked',
        '--target', 'i686-pc-windows-msvc', '--target-dir', $fixtureTarget, '--', '--test-threads=1'
    ) | Out-Null
    $evidence.fixture_component_tests = 'passed'
    Copy-Item -LiteralPath $auditPath -Destination (Join-Path $evidenceRoot 'component-fixture-writes.jsonl')
    Invoke-SmokeCommand -Name fixture-build -File $tools.cargo -TimeoutSeconds 600 -Arguments @(
        '+stable-x86_64-pc-windows-msvc', 'build', '--manifest-path', $manifest,
        '--target', 'i686-pc-windows-msvc', '--target-dir', $fixtureTarget, '--locked'
    ) | Out-Null
    $fixtureDll = Join-Path $fixtureTarget 'i686-pc-windows-msvc/debug/bhtune_opcda_yokogawa_fixture.dll'
    $evidence.fixture = [ordered]@{
        package = 'bhtune-opcda-yokogawa-fixture'
        target = 'i686-pc-windows-msvc'
        path = $fixtureDll
        size_bytes = (Get-Item -LiteralPath $fixtureDll).Length
        sha256 = (Get-FileHash -LiteralPath $fixtureDll -Algorithm SHA256).Hash.ToLowerInvariant()
    }

    $contractSource = Join-Path $repositoryRoot 'installer/windows/opcda-gateway-release.json'
    $contract = Assert-GatewayReleaseContract -ContractPath $contractSource
    $repository = [string]$contract.repository
    $tag = [string]$contract.tag
    $download = Join-Path $workRoot 'download'
    $extract = Join-Path $workRoot 'extract'
    $payload = Join-Path $workRoot 'gateway'
    New-Item -ItemType Directory -Path $download, $payload | Out-Null
    $release = Invoke-SmokeCommand -Name gateway-release -File $tools.gh -Arguments @('api', "repos/$repository/releases/tags/$tag") | ConvertFrom-Json
    if ([string]$release.tag_name -cne $tag -or $release.draft -or $release.prerelease) {
        throw 'The pinned gateway must be a published stable release.'
    }
    $requiredAssets = @(
        [string]$contract.archive.name,
        [string]$contract.upstream_evidence.archive_sigstore_bundle,
        [string]$contract.upstream_evidence.checksums,
        "$($contract.upstream_evidence.checksums).sigstore.json",
        [string]$contract.upstream_evidence.provenance
    )
    $publishedAssets = @($release.assets | ForEach-Object { [string]$_.name })
    foreach ($asset in $requiredAssets) {
        if ($publishedAssets -cnotcontains $asset) { throw "The pinned gateway release is missing '$asset'." }
    }
    $tagRef = Invoke-SmokeCommand -Name gateway-tag -File $tools.gh -Arguments @('api', "repos/$repository/git/ref/tags/$tag") | ConvertFrom-Json
    if ([string]$tagRef.object.type -eq 'tag') {
        $tagObject = Invoke-SmokeCommand -Name gateway-tag-object -File $tools.gh -Arguments @('api', "repos/$repository/git/tags/$($tagRef.object.sha)") | ConvertFrom-Json
        if ([string]$tagObject.object.type -ne 'commit') { throw 'The gateway tag does not resolve to a commit.' }
        $sourceCommit = [string]$tagObject.object.sha
    } elseif ([string]$tagRef.object.type -eq 'commit') {
        $sourceCommit = [string]$tagRef.object.sha
    } else {
        throw 'The gateway tag has an unsupported object type.'
    }
    if ($sourceCommit -cne [string]$contract.source_commit) { throw 'The gateway tag moved from its pinned source commit.' }
    $workflow = Invoke-SmokeCommand -Name gateway-workflow -File $tools.gh -Arguments @('api', "repos/$repository/contents/$($contract.release_workflow.path)?ref=$tag") | ConvertFrom-Json
    if ([string]$workflow.sha -cne [string]$contract.release_workflow.blob_sha) {
        throw 'The gateway release workflow does not match its pinned blob SHA.'
    }
    $downloadArguments = @('release', 'download', $tag, '--repo', $repository, '--dir', $download)
    foreach ($asset in $requiredAssets) { $downloadArguments += @('--pattern', $asset) }
    Invoke-SmokeCommand -Name gateway-download -File $tools.gh -Arguments $downloadArguments -TimeoutSeconds 180 | Out-Null
    $archive = Join-Path $download ([string]$contract.archive.name)
    $archiveBundle = Join-Path $download ([string]$contract.upstream_evidence.archive_sigstore_bundle)
    $checksums = Join-Path $download ([string]$contract.upstream_evidence.checksums)
    $attestation = Join-Path $download ([string]$contract.upstream_evidence.provenance)
    Invoke-SmokeCommand -Name gateway-checksum-signature -File $tools.cosign -Arguments @(
        'verify-blob', '--bundle', "$checksums.sigstore.json",
        '--certificate-identity', [string]$contract.release_workflow.builder_id,
        '--certificate-oidc-issuer', 'https://token.actions.githubusercontent.com', $checksums
    ) | Out-Null
    Invoke-SmokeCommand -Name gateway-archive-signature -File $tools.cosign -Arguments @(
        'verify-blob', '--bundle', $archiveBundle,
        '--certificate-identity', [string]$contract.release_workflow.builder_id,
        '--certificate-oidc-issuer', 'https://token.actions.githubusercontent.com', $archive
    ) | Out-Null
    Invoke-SmokeCommand -Name gateway-provenance -File $tools.gh -Arguments @(
        'attestation', 'verify', $archive, '--repo', $repository, '--bundle', $attestation,
        '--signer-workflow', "$repository/$($contract.release_workflow.path)",
        '--source-ref', "refs/tags/$tag", '--cert-oidc-issuer', 'https://token.actions.githubusercontent.com'
    ) | Out-Null
    $checksumLines = @(Get-Content -LiteralPath $checksums | Where-Object {
        [IO.Path]::GetFileName(($_ -split '\s+', 2)[-1].TrimStart('*')) -ceq [string]$contract.archive.name
    })
    if ($checksumLines.Count -ne 1 -or $checksumLines[0] -cnotmatch '^(?<hash>[0-9a-fA-F]{64})\s+\*?(?<name>.+)$') {
        throw 'The signed checksums must name the pinned gateway archive exactly once.'
    }
    $archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($archiveHash -cne $Matches.hash.ToLowerInvariant() -or $archiveHash -cne [string]$contract.archive.sha256) {
        throw 'The gateway archive does not match both signed and pinned checksums.'
    }
    Expand-Archive -LiteralPath $archive -DestinationPath $extract
    $archiveFiles = @(Get-ChildItem -LiteralPath $extract -File -Recurse | ForEach-Object {
        [IO.Path]::GetRelativePath($extract, $_.FullName)
    })
    if ($archiveFiles.Count -ne 1 -or $archiveFiles[0] -cne [string]$contract.executable.name) {
        throw 'The gateway archive does not contain exactly the expected executable.'
    }
    $executable = Join-Path $extract ([string]$contract.executable.name)
    $executableHash = (Get-FileHash -LiteralPath $executable -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($executableHash -cne [string]$contract.executable.sha256) { throw 'The gateway executable hash does not match the release contract.' }
    Copy-Item -LiteralPath $executable -Destination (Join-Path $payload 'opcda-bridge-gateway.exe')
    Copy-Item -LiteralPath $contractSource -Destination (Join-Path $payload 'opcda-gateway-release.json')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'installer/windows/third-party/opcda-bridge/LICENSE-MIT.txt') -Destination (Join-Path $payload 'LICENSE-opcda-bridge.txt')
    Copy-Item -LiteralPath (Join-Path $repositoryRoot 'installer/windows/third-party/opcda-bridge/NOTICE.txt') -Destination (Join-Path $payload 'NOTICE-opcda-bridge.txt')
    $provenance = [ordered]@{
        schema_version = 1
        verified_at_utc = [DateTime]::UtcNow.ToString('o')
        repository = $repository
        tag = $tag
        version = [string]$contract.version
        source_commit = $sourceCommit
        archive_name = [string]$contract.archive.name
        archive_sha256 = $archiveHash
        executable_name = [string]$contract.executable.name
        executable_sha256 = $executableHash
        builder_id = [string]$contract.release_workflow.builder_id
        release_workflow_blob_sha = [string]$workflow.sha
        release_assets_sha256_sha256 = (Get-FileHash -LiteralPath $checksums -Algorithm SHA256).Hash.ToLowerInvariant()
        archive_sigstore_bundle_sha256 = (Get-FileHash -LiteralPath $archiveBundle -Algorithm SHA256).Hash.ToLowerInvariant()
        provenance_asset_sha256 = (Get-FileHash -LiteralPath $attestation -Algorithm SHA256).Hash.ToLowerInvariant()
        sigstore_verified = $true
        github_provenance_verified = $true
    }
    $provenance | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $payload 'opcda-gateway-provenance.json') -Encoding utf8NoBOM
    $gatewayPayload = Assert-GatewayPayloadLayout -GatewayPayloadRoot $payload
    Assert-GatewayPayloadBinary -GatewayPayload $gatewayPayload | Out-Null
    $evidence.gateway = $provenance
    Copy-Item -LiteralPath (Join-Path $payload 'opcda-gateway-provenance.json'), $contractSource, $checksums, "$checksums.sigstore.json", $archiveBundle, $attestation -Destination $evidenceRoot

    $firewall.program = $gatewayPayload.Executable
    $profiles = @(Get-NetFirewallProfile -PolicyStore ActiveStore -ErrorAction Stop)
    if (($profiles.Name | Sort-Object) -join ',' -cne 'Domain,Private,Public') {
        throw 'Cannot establish the complete effective Windows firewall profile state.'
    }
    $firewall.profiles = @($profiles | ForEach-Object {
        [pscustomobject]@{ name = $_.Name; enabled = [string]$_.Enabled; allow_local_rules = [string]$_.AllowLocalFirewallRules }
    })
    $disabledProfiles = @($profiles | Where-Object { [string]$_.Enabled -ne 'True' })
    if ($disabledProfiles.Count -gt 0) {
        if (-not $isGithubHosted) {
            throw 'Local smoke requires an enabled firewall on every profile to protect the wildcard gateway listener. The runner will not enable it or change global policy.'
        }
        $firewall.protection = 'disposable_github_host_firewall_not_fully_enabled'
    } else {
        if (@($profiles | Where-Object { [string]$_.AllowLocalFirewallRules -eq 'False' }).Count -gt 0) {
            throw 'Effective firewall policy disallows the required local executable-scoped block.'
        }
        $existingRules = @(Get-NetFirewallRule -PolicyStore PersistentStore -ErrorAction Stop | Where-Object { $_.Name -ceq $firewall.name })
        if ($existingRules.Count -ne 0) { throw 'Refusing a colliding firewall rule name.' }
        $firewall.attempted = $true
        try {
            New-NetFirewallRule -Name $firewall.name -DisplayName $firewall.owner -Description $firewall.owner `
                -PolicyStore PersistentStore -Program $firewall.program -Direction Inbound -Action Block `
                -Profile Any -Enabled True -Protocol Any -LocalAddress Any -RemoteAddress Any `
                -EdgeTraversalPolicy Block -ErrorAction Stop | Out-Null
        } catch {
            throw "Cannot establish the owned gateway inbound block. Local firewall-rule creation/removal requires administrator permissions, independently of User COM scope. Use explicit Machine scope only on a disposable host; no elevation or fallback is performed. $($_.Exception.Message)"
        }
        $firewall.created = $true
        $activeRules = @(Get-NetFirewallRule -PolicyStore ActiveStore -Name $firewall.name -ErrorAction Stop)
        if ($activeRules.Count -ne 1) { throw 'The owned firewall block is missing or ambiguous in active policy.' }
        $firewall.active_rule = [ordered]@{
            enabled = [string]$activeRules[0].Enabled
            direction = [string]$activeRules[0].Direction
            action = [string]$activeRules[0].Action
            profile = [string]$activeRules[0].Profile
            enforcement = @($activeRules[0].EnforcementStatus | ForEach-Object { [string]$_ })
            status = [string]$activeRules[0].Status
        }
        Assert-SmokeFirewallRule -Rule $activeRules[0] -RequireEffective
        $firewall.protection = 'owned_executable_inbound_block'
    }

    Assert-NoFixtureRegistration -ClsidPath $clsidPath -ProgIdPath $progIdPath
    $discoveryProgId = 'OPC.ServerList.1'
    $discoveryClsid = '{65CF766F-DFDC-423E-9A67-E6FD1C1AB4F3}'
    $discoveryClsidPath = "Software\Classes\CLSID\$discoveryClsid"
    $discoveryProgIdPath = "Software\Classes\$discoveryProgId"
    $existingDiscovery = @(Get-SmokeDiscoveryRegistration)
    $useDiscoveryShim = $existingDiscovery.Count -eq 0
    $evidence.discovery = [ordered]@{
        prog_id = $discoveryProgId
        provider = if ($useDiscoveryShim) { 'fixture_shim' } else { 'existing_opcenum' }
        existing_registrations = $existingDiscovery
        shim_clsid = if ($useDiscoveryShim) { $discoveryClsid } else { $null }
    }
    if (@($existingDiscovery | Where-Object { $_.clsid -ieq $discoveryClsid }).Count -ne 0) {
        $evidence.discovery.provider = 'blocked_existing_fixture_shim'
        throw 'OPC.ServerList.1 belongs to another fixture registration. It is preserved; its owner must clean it up before this run.'
    }
    if (-not $useDiscoveryShim -and $RegistrationScope -eq 'User') {
        throw 'Existing OPCEnum may not discover HKCU fixture categories. Its registration is preserved; explicitly select Machine scope only on a disposable reproduction host.'
    }
    if ($useDiscoveryShim) {
        Assert-NoFixtureRegistration -ClsidPath $discoveryClsidPath -ProgIdPath $discoveryProgIdPath
    } elseif (@($existingDiscovery | Where-Object { $_.hive -eq 'LocalMachine' -and $_.view -eq 'Registry32' }).Count -eq 0) {
        throw 'Existing OPC.ServerList.1 has no 32-bit machine registration usable by this elevated run. It will not be replaced; provide a usable real OPCEnum on the disposable host.'
    }
    $hive = if ($RegistrationScope -eq 'User') { [Microsoft.Win32.RegistryHive]::CurrentUser } else { [Microsoft.Win32.RegistryHive]::LocalMachine }
    $registrations = @(
        @{ Path = $clsidPath; Values = @{ '' = 'BHTune synthetic OPC DA smoke fixture' } },
        @{ Path = "$clsidPath\InprocServer32"; Values = @{ '' = $fixtureDll; ThreadingModel = 'Both' } },
        @{ Path = "$clsidPath\ProgID"; Values = @{ '' = $progId } },
        @{ Path = "$clsidPath\Implemented Categories"; Values = @{} },
        @{ Path = "$clsidPath\Implemented Categories\{63D5F432-CFE4-11D1-B2C8-0060083BA1FB}"; Values = @{} },
        @{ Path = $progIdPath; Values = @{ '' = 'BHTune synthetic OPC DA smoke fixture' } },
        @{ Path = "$progIdPath\CLSID"; Values = @{ '' = $clsid } }
    )
    if ($useDiscoveryShim) {
        $registrations += @(
            @{ Path = $discoveryClsidPath; Values = @{ '' = 'BHTune synthetic OPC DA discovery shim' } },
            @{ Path = "$discoveryClsidPath\InprocServer32"; Values = @{ '' = $fixtureDll; ThreadingModel = 'Both' } },
            @{ Path = "$discoveryClsidPath\ProgID"; Values = @{ '' = $discoveryProgId } },
            @{ Path = $discoveryProgIdPath; Values = @{ '' = 'BHTune synthetic OPC DA discovery shim' } },
            @{ Path = "$discoveryProgIdPath\CLSID"; Values = @{ '' = $discoveryClsid } }
        )
    }
    foreach ($registration in $registrations) {
        if ($registration.Path -eq $discoveryProgIdPath -and @(Get-SmokeDiscoveryRegistration).Count -ne 0) {
            throw 'OPCEnum registration appeared during setup; it will not be overwritten.'
        }
        $key = [BhtuneSmokeProcess]::CreateRegistry32Key($hive, $registration.Path)
        try {
            $values = [ordered]@{}
            $ownedKeys.Add([pscustomobject]@{ hive = $hive.ToString(); view = 'Registry32'; path = $registration.Path; values = $values; removed = $false })
            $key.SetValue('BHTuneSmokeOwner', $runId, [Microsoft.Win32.RegistryValueKind]::String)
            $values['BHTuneSmokeOwner'] = $runId
            foreach ($name in $registration.Values.Keys) {
                $key.SetValue($name, $registration.Values[$name], [Microsoft.Win32.RegistryValueKind]::String)
                $values[$name] = $registration.Values[$name]
            }
        } finally {
            $key.Dispose()
        }
    }

    $reservation = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 0)
    try {
        $reservation.Start()
        $port = $reservation.LocalEndpoint.Port
    } finally {
        $reservation.Stop()
    }
    if ($port -in @(7600, 7602) -or @(Get-SmokeListeners -Port $port).Count -ne 0) {
        throw "Refusing occupied or protected gateway port $port."
    }
    $gatewayLogs = Join-Path $stateRoot 'gateway-logs'
    New-Item -ItemType Directory -Path $gatewayLogs | Out-Null
    $configPath = Join-Path $stateRoot 'gateway.toml'
    $indexPath = (Join-Path $stateRoot 'gateway-index.db').Replace('\', '/')
    $logPath = $gatewayLogs.Replace('\', '/')
    @"
port = $port

[log]
dir = "$logPath"
level = "info"

[index]
database_path = "$indexPath"
"@ | Set-Content -LiteralPath $configPath -Encoding utf8NoBOM
    $auditPath = Join-Path $stateRoot 'fixture-writes.jsonl'
    [IO.File]::WriteAllText($auditPath, '')
    [Environment]::SetEnvironmentVariable('BHTUNE_OPCDA_SMOKE_HOST', "127.0.0.1:$port", 'Process')
    [Environment]::SetEnvironmentVariable('BHTUNE_OPCDA_SMOKE_DIR', $stateRoot, 'Process')
    [Environment]::SetEnvironmentVariable('BHTUNE_OPCDA_FIXTURE_AUDIT', $auditPath, 'Process')
    [Environment]::SetEnvironmentVariable('BHTUNE_OPCDA_SMOKE_GATEWAY_VERSION', [string]$contract.version, 'Process')
    $evidence.endpoint = "127.0.0.1:$port"
    $evidence.gateway_config = $configPath
    $evidence.gateway_listen_address = '0.0.0.0'
    $gateway = Start-SmokeProcess -Name gateway -File $gatewayPayload.Executable -Arguments @(
        '--config', $configPath, '--port', "$port", '--log-dir', $gatewayLogs, '--log-level', 'info'
    )
    $listenerDeadline = [Diagnostics.Stopwatch]::StartNew()
    do {
        if ($gateway.native.Wait(0)) { throw 'The owned gateway exited before creating its test listener.' }
        $listeners = @(Get-SmokeListeners -Port $port)
        if (@($listeners | Where-Object { $_.OwningProcess -ne $gateway.pid }).Count -ne 0) {
            throw "Test port $port has an unexpected owner; no unrelated process will be stopped."
        }
        if ($listeners.Count -gt 0) { break }
        Start-Sleep -Milliseconds 100
    } while ($listenerDeadline.Elapsed.TotalSeconds -lt 20)
    if ($listeners.Count -eq 0) { throw 'The owned gateway listener did not appear within 20 seconds.' }
    $identity = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $($gateway.pid)" -ErrorAction Stop
    if ($null -eq $identity -or [string]$identity.ExecutablePath -ine $gatewayPayload.Executable -or
        [string]$identity.CommandLine -cne $gateway.command_line -or
        (Get-FileHash -LiteralPath $gatewayPayload.Executable -Algorithm SHA256).Hash.ToLowerInvariant() -cne $executableHash) {
        throw 'The gateway process identity, configuration, or executable hash changed.'
    }
    $evidence.listeners = @($listeners | Select-Object LocalAddress, LocalPort, OwningProcess)
    # The harness checks metadata readiness before any database-bootstrapping CLI command.
    Invoke-SmokeCommand -Name cli-smoke -File $tools.cargo -TimeoutSeconds 480 -Arguments @(
        'test', '-p', 'bhtune', '--locked', '--test', 'e2e_opcda_yokogawa', '--',
        '--ignored', '--test-threads=1'
    ) | Out-Null
    $evidence.status = 'passed'
} catch {
    $failure = $_
    $evidence.failure = $_.Exception.Message
} finally {
    for ($index = $processes.Count - 1; $index -ge 0; $index--) {
        $entry = $processes[$index]
        try { Stop-SmokeProcess -Entry $entry }
        catch { $cleanupErrors.Add("Process $($entry.pid): $($_.Exception.Message)") }
    }
    $allStopped = @($processes | Where-Object { -not $_.stopped }).Count -eq 0
    for ($index = $ownedKeys.Count - 1; $index -ge 0; $index--) {
        $owned = $ownedKeys[$index]
        $base = $null
        $key = $null
        try {
            $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey(
                [Microsoft.Win32.RegistryHive]([Enum]::Parse([Microsoft.Win32.RegistryHive], $owned.hive)),
                [Microsoft.Win32.RegistryView]::Registry32
            )
            $key = $base.OpenSubKey($owned.path)
            if ($null -ne $key) {
                if ($key.GetValue('BHTuneSmokeOwner', '') -cne $runId -or $key.SubKeyCount -ne 0 -or
                    $key.ValueCount -ne $owned.values.Count) {
                    throw 'COM key ownership or contents changed; refusing deletion.'
                }
                foreach ($name in $owned.values.Keys) {
                    if ($key.GetValue($name, $null) -cne $owned.values[$name] -or
                        $key.GetValueKind($name) -ne [Microsoft.Win32.RegistryValueKind]::String) {
                        throw 'COM key values changed; refusing deletion.'
                    }
                }
                $key.Dispose()
                $key = $null
                $base.DeleteSubKey($owned.path, $true)
            }
            $owned.removed = $true
        } catch {
            $cleanupErrors.Add("Registry $($owned.hive)\$($owned.path): $($_.Exception.Message)")
        } finally {
            if ($null -ne $key) { $key.Dispose() }
            if ($null -ne $base) { $base.Dispose() }
        }
    }
    if ($firewall.attempted) {
        try {
            if (-not $allStopped) { throw 'The owned firewall block is retained until every owned process tree is stopped.' }
            $rules = @(Get-NetFirewallRule -PolicyStore PersistentStore -ErrorAction Stop | Where-Object { $_.Name -ceq $firewall.name })
            if ($rules.Count -gt 1) { throw 'Firewall ownership is ambiguous; refusing deletion.' }
            if ($rules.Count -eq 1) {
                Assert-SmokeFirewallRule -Rule $rules[0]
                Remove-NetFirewallRule -InputObject $rules[0] -ErrorAction Stop
            }
            if (@(Get-NetFirewallRule -PolicyStore PersistentStore -ErrorAction Stop | Where-Object { $_.Name -ceq $firewall.name }).Count -ne 0 -or
                @(Get-NetFirewallRule -PolicyStore ActiveStore -ErrorAction Stop | Where-Object { $_.Name -ceq $firewall.name }).Count -ne 0) {
                throw 'The owned firewall block remains after removal.'
            }
            $firewall.removed = $true
        } catch {
            $cleanupErrors.Add("Firewall $($firewall.name): $($_.Exception.Message)")
        }
    }
    foreach ($name in $environmentBefore.Keys) {
        [Environment]::SetEnvironmentVariable($name, $environmentBefore[$name], 'Process')
    }
    try {
        if (-not $allStopped) { throw 'Cannot snapshot databases while an owned process tree may still be running.' }
        $stateFiles = @(Get-ChildItem -LiteralPath $stateRoot -Recurse -Force)
        if (@($stateFiles | Where-Object { $_.Attributes -band [IO.FileAttributes]::ReparsePoint }).Count -ne 0) {
            throw 'Refusing evidence containing a reparse point.'
        }
        foreach ($file in @(Get-ChildItem -LiteralPath $stateRoot, $commandLogs -File -Recurse)) {
            if ($file.Length -gt 1MB -and
                ($file.Extension -in @('.stdout', '.stderr', '.log') -or $file.DirectoryName -like '*-logs*')) {
                $stream = [IO.File]::OpenRead($file.FullName)
                try {
                    [void]$stream.Seek(-1MB, [IO.SeekOrigin]::End)
                    $tail = [byte[]]::new(1MB)
                    $stream.ReadExactly($tail)
                } finally {
                    $stream.Dispose()
                }
                [IO.File]::WriteAllBytes($file.FullName, $tail)
                $evidence.truncated_logs.Add($file.Name)
            }
        }
        $stateBytes = 0L
        foreach ($file in Get-ChildItem -LiteralPath $stateRoot -Recurse -File) {
            $stateBytes += $file.Length
        }
        if ($stateBytes -gt 64MB) {
            throw 'Owned smoke state exceeds the 64 MiB evidence bound.'
        }
        Copy-Item -LiteralPath $stateRoot -Destination (Join-Path $evidenceRoot 'state') -Recurse
    } catch {
        $cleanupErrors.Add("Evidence: $($_.Exception.Message)")
    }
    try {
        if (-not $allStopped -or @($ownedKeys | Where-Object { -not $_.removed }).Count -ne 0 -or
            ($firewall.attempted -and -not $firewall.removed)) {
            throw 'Scratch state is retained because process, COM, or scoped-firewall ownership cleanup is incomplete.'
        }
        if ((Get-Content -LiteralPath (Join-Path $workRoot '.owner') -Raw).Trim() -cne $runId -or
            (Get-Item -LiteralPath $workRoot -Force).Attributes -band [IO.FileAttributes]::ReparsePoint -or
            @(Get-ChildItem -LiteralPath $workRoot -Recurse -Force | Where-Object {
                $_.Attributes -band [IO.FileAttributes]::ReparsePoint
            }).Count -ne 0) {
            throw 'Scratch directory ownership changed; refusing deletion.'
        }
        Remove-Item -LiteralPath $workRoot -Recurse -Force
    } catch {
        $cleanupErrors.Add("Scratch: $($_.Exception.Message)")
    }
    $evidence.processes = @($processes | Select-Object * -ExcludeProperty native)
    $evidence.cleanup_errors = @($cleanupErrors)
    $evidence.finished_at_utc = [DateTime]::UtcNow.ToString('o')
    if ($cleanupErrors.Count -gt 0) { $evidence.status = 'failed' }
    $evidence | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $evidenceRoot 'evidence.json') -Encoding utf8NoBOM
}

Write-Host "Native OPC DA smoke evidence: $evidenceRoot"
if ($cleanupErrors.Count -gt 0) { throw "Smoke cleanup failed: $($cleanupErrors -join '; ')" }
if ($null -ne $failure) { throw $failure }
