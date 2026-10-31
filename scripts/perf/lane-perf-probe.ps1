# Lane PERF probe: measures wiri's thread/memory profile on the live desktop.
#
# It takes the machine-wide live mutex (`Global\wiri-live-test`), starts ONE wiri
# from the caller's build, creates only its own ordinary WinForms windows, measures,
# and cleans up in a `finally` block.  It never stops a foreign process and never
# writes outside the worktree.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\perf\lane-perf-probe.ps1 `
#       -Exe target\release\wiri.exe -Windows 2 -Out .perfprobe\run2.json
#
# Measurements per window count:
#   * threads (count, and the per-thread description) at rest and after actions
#   * private bytes / working set / handles / GDI+USER objects
#   * idle CPU (kernel+user) and idle context switches over a single 30 s sleep
param(
    [string]$Exe = '',
    [string]$Out = '',
    [int]$Windows = 2,
    [int]$IdleSeconds = 30,
    [int]$LockWaitMinutes = 120,
    [string]$Label = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $Exe) { $Exe = Join-Path $repoRoot 'target\release\wiri.exe' }
$Exe = [IO.Path]::GetFullPath($Exe)
if (-not (Test-Path $Exe)) { throw "wiri executable not found: $Exe" }
if (-not $Out) { $Out = Join-Path $repoRoot ('.perfprobe\probe-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.json') }
$Out = [IO.Path]::GetFullPath($Out)

Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs')
. (Join-Path $PSScriptRoot '..\live-fixtures\owned-forms.ps1')

if (-not ('PerfProbe.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace PerfProbe {
    public static class Native {
        [DllImport("kernel32.dll", SetLastError = true)]
        static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
        static extern int GetThreadDescription(IntPtr thread, out IntPtr description);
        [DllImport("kernel32.dll")]
        static extern IntPtr LocalFree(IntPtr memory);
        [DllImport("kernel32.dll")]
        static extern bool CloseHandle(IntPtr handle);
        const uint THREAD_QUERY_LIMITED_INFORMATION = 0x0800;
        // The thread description is what SetThreadDescription stores; Rust's
        // std::thread names and tokio's thread_name both use it.
        public static string Describe(long tid) {
            IntPtr thread = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, (uint)tid);
            if (thread == IntPtr.Zero) return "<unavailable>";
            try {
                IntPtr text;
                if (GetThreadDescription(thread, out text) != 0 || text == IntPtr.Zero) return "<none>";
                try { return Marshal.PtrToStringUni(text) ?? "<none>"; }
                finally { LocalFree(text); }
            } finally { CloseHandle(thread); }
        }
    }
}
'@
}

$mutex = New-Object System.Threading.Mutex($false, 'Global\wiri-live-test')
$held = $false
$child = $null
$forms = @()
$work = $null
try {
    $held = $mutex.WaitOne($LockWaitMinutes * 60 * 1000)
    if (-not $held) { throw 'Could not take the live-test mutex' }

    $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
    $work = Join-Path $repoRoot (Join-Path '.perfprobe' ("run-$stamp"))
    New-Item -ItemType Directory -Force $work | Out-Null
    foreach ($name in @('local', 'roaming')) { New-Item -ItemType Directory -Force (Join-Path $work $name) | Out-Null }
    $config = Join-Path $work 'config.kdl'
    Copy-Item (Join-Path $repoRoot 'resources\default_config.kdl') $config

    # Only the child's environment changes; every file it writes stays in the worktree.
    [LaneE.Native]::PipeName = 'wiri_probe_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $envVars = @{
        LOCALAPPDATA = (Join-Path $work 'local')
        APPDATA = (Join-Path $work 'roaming')
        WIRI_CONFIG = $config
        WIRI_PIPE = '\\.\pipe\' + [LaneE.Native]::PipeName
    }

    # Ordinary windows, exactly like the live harness's own fixtures.
    for ($i = 1; $i -le $Windows; $i++) {
        $forms += [OwnedLiveForms]::Open("perf-probe-$i", 40 + ($i * 7 % 180), 110, 160)
    }
    Start-Sleep -Milliseconds 800

    $info = New-Object Diagnostics.ProcessStartInfo
    $info.FileName = $Exe
    $info.Arguments = '-v --no-autostart --config "' + $config + '"'
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardError = $true
    $info.RedirectStandardOutput = $true
    $info.WorkingDirectory = $work
    foreach ($key in $envVars.Keys) { $info.EnvironmentVariables[$key] = $envVars[$key] }
    $child = [Diagnostics.Process]::Start($info)
    $childId = $child.Id

    # Wait for the daemon to answer a first request.
    $ready = $false
    for ($i = 0; $i -lt 120; $i++) {
        Start-Sleep -Milliseconds 250
        try {
            $reply = [LaneE.Native]::Request('{"type":"get_state"}', 2000, $childId) | ConvertFrom-Json
            if ($reply.success -eq $true) { $ready = $true; break }
        } catch { }
    }
    if (-not $ready) { throw 'wiri did not answer a get_state request' }
    Start-Sleep -Seconds 5 # let the startup indicator and the first location events settle

    function Snapshot([string]$when) {
        $r = [LaneE.Native]::ReadResources($childId)
        $t = [LaneE.Native]::ReadThreads($childId)
        $rows = @()
        foreach ($thread in $t.Threads) {
            $rows += [pscustomobject]@{ tid = $thread.Id; description = [PerfProbe.Native]::Describe($thread.Id); switches = $thread.Switches; cpu_ms = $thread.CpuMs }
        }
        return [pscustomobject]@{
            when = $when
            private_bytes = $r.PrivateBytes
            working_set_bytes = $r.WorkingSetBytes
            threads = $r.Threads
            handles = $r.Handles
            gdi_objects = $r.GdiObjects
            user_objects = $r.UserObjects
            thread_rows = $rows
        }
    }
    function Idle([string]$when) {
        $idle = [LaneE.Native]::Idle($childId, $IdleSeconds * 1000)
        $rows = @()
        foreach ($d in $idle.Threads) {
            $rows += [pscustomobject]@{ tid = $d.Id; description = [PerfProbe.Native]::Describe($d.Id); switches = $d.Switches; cpu_ms = $d.CpuMs }
        }
        return [pscustomobject]@{
            when = $when
            seconds = $idle.Seconds
            cpu_ms = $idle.CpuMs
            one_core_percent = $idle.OneCorePercent
            switches_per_second = $idle.SwitchesPerSecond
            new_threads = $idle.NewThreads
            exited_threads = $idle.ExitedThreads
            input_changed = ($idle.InputTickBefore -ne $idle.InputTickAfter)
            thread_rows = $rows
        }
    }
    function Action([string]$name, [string[]]$arguments) {
        $json = @{ type = 'run_action'; data = @{ name = $name; args = @($arguments) } } | ConvertTo-Json -Compress -Depth 4
        $reply = [LaneE.Native]::Request($json, 5000, $childId) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "action $name rejected: $($reply | ConvertTo-Json -Compress)" }
        Start-Sleep -Milliseconds 500
    }

    $rest = Snapshot 'rest'
    $idleRest = Idle 'rest'

    # A short sequence of ordinary actions, then the same idle observation again.
    Action 'focus-column-right' @()
    Action 'set-column-width' @('600')
    Action 'fullscreen-window' @()
    Action 'fullscreen-window' @()
    Action 'move-column-left' @()
    $after = Snapshot 'after'
    $idleAfter = Idle 'after'

    $result = [ordered]@{
        label = $Label
        utc = [DateTime]::UtcNow.ToString('o')
        machine = $env:COMPUTERNAME
        logical_processors = [Environment]::ProcessorCount
        exe = $Exe
        exe_sha256 = (Get-FileHash $Exe -Algorithm SHA256).Hash
        pid = $childId
        windows = $Windows
        idle_seconds = $IdleSeconds
        rest = $rest
        idle_rest = $idleRest
        after = $after
        idle_after = $idleAfter
    }
    New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
    $result | ConvertTo-Json -Depth 8 | Set-Content -Encoding utf8 $Out
    Write-Host "probe result: $Out"
} finally {
    if ($null -ne $child) {
        try {
            if (-not $child.HasExited) {
                try { $null = [LaneE.Native]::Request('{"type":"quit"}', 3000, $child.Id) } catch { }
                if (-not $child.WaitForExit(10000)) { $child.Kill(); $null = $child.WaitForExit(5000) }
            }
        } catch { }
        try { $child.Dispose() } catch { }
    }
    foreach ($h in $forms) { try { [OwnedLiveForms]::Close($h) } catch { } }
    if ($held) { try { $mutex.ReleaseMutex() } catch { } }
    $mutex.Dispose()
}
