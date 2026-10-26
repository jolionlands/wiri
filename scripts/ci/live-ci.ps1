# Experimental hosted-runner adapter. Never schedules a task, unlocks a session, or installs GUI software.
# On a disposable runner it may close Windows' first-run privacy screen when that screen holds the foreground.
[CmdletBinding()]
param(
    [string]$Bin = 'target\release',
    [string]$Out = '.live',
    [switch]$ProbeOnly
)

$ErrorActionPreference = 'Stop'
$root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$Out = [IO.Path]::GetFullPath($Out)
$Bin = [IO.Path]::GetFullPath($Bin)
$evidence = Join-Path $Out 'harness'
New-Item -ItemType Directory -Force $Out | Out-Null
$preflight = [ordered]@{
    session_id = (Get-Process -Id $PID).SessionId
    user_interactive = [Environment]::UserInteractive
    runner_arch = $env:RUNNER_ARCH
    windows_terminal_on_path = [bool](Get-Command wt.exe -ErrorAction SilentlyContinue)
    input_desktop = $null
    thread_desktop = $null
    interactive_desktop = $false
    foreground_process = $null
    first_run_screen_dismissed = $false
    reason = ''
}
$summary = [pscustomobject]@{ status = 'skipped'; ci = $preflight }
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$failed = $false
$wslGuard = $null
try {
    if (-not $ProbeOnly -and ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted' -or $env:WIRI_LIVE_CI -ne '1')) {
        throw 'Run mode is restricted to disposable GitHub runners with WIRI_LIVE_CI=1. Use -ProbeOnly for a read-only desktop probe.'
    }
    Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class WiriCiDesktop {
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr OpenInputDesktop(uint flags, bool inherit, uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool GetUserObjectInformation(IntPtr handle, int index, StringBuilder name, int bytes, out int needed);
    [DllImport("user32.dll")] static extern IntPtr GetThreadDesktop(uint threadId);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    static string Name(IntPtr desktop) {
        var name = new StringBuilder(256); int needed;
        return desktop != IntPtr.Zero && GetUserObjectInformation(desktop, 2, name, name.Capacity * 2, out needed) ? name.ToString() : "";
    }
    public static string ThreadName() { return Name(GetThreadDesktop(GetCurrentThreadId())); }
    public static string InputName() {
        IntPtr desktop = OpenInputDesktop(0, false, 0x0101); // READOBJECTS | SWITCHDESKTOP; no actual switch
        if (desktop == IntPtr.Zero) return "";
        try { return Name(desktop); } finally { CloseDesktop(desktop); }
    }
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    public static uint ForegroundPid() {
        uint pid = 0;
        IntPtr hwnd = GetForegroundWindow();
        if (hwnd != IntPtr.Zero) GetWindowThreadProcessId(hwnd, out pid);
        return pid;
    }
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    public static void PressEscape() { keybd_event(0x1B, 0, 0, UIntPtr.Zero); keybd_event(0x1B, 0, 2, UIntPtr.Zero); }
}
'@
    # The hosted windows-11-arm image (2026-10) boots into Windows' first-run privacy screen, which holds the
    # foreground: every SetForegroundWindow, from wiri or the harness, is refused and about 60 focus gates fail.
    function Get-ForegroundProcessName {
        $fgPid = [WiriCiDesktop]::ForegroundPid()
        if ($fgPid -eq 0) { return '' }
        $p = Get-Process -Id $fgPid -ErrorAction SilentlyContinue
        if ($p) { $p.ProcessName } else { '' }
    }
    $firstRunProcesses = @('WWAHost', 'CloudExperienceHostBroker', 'UserOOBEBroker', 'msoobe')
    $preflight.input_desktop = [WiriCiDesktop]::InputName()
    $preflight.thread_desktop = [WiriCiDesktop]::ThreadName()
    if (-not $preflight.user_interactive -or $preflight.session_id -eq 0 -or
        $preflight.input_desktop -ne 'Default' -or $preflight.thread_desktop -ne $preflight.input_desktop) {
        $preflight.reason = 'No accessible, unlocked interactive Default desktop in the runner session. No live phases ran (this is not a pass).'
    } else {
        # Access to a desktop handle alone does not prove screen capture works.
        Add-Type -AssemblyName System.Drawing, System.Windows.Forms
        try {
            $screen = [System.Windows.Forms.SystemInformation]::VirtualScreen
            if ($screen.Width -le 0 -or $screen.Height -le 0) { throw 'No screen is available for GUI automation.' }
            $bitmap = New-Object System.Drawing.Bitmap 1, 1
            try {
                $graphics = [Drawing.Graphics]::FromImage($bitmap)
                try { $graphics.CopyFromScreen($screen.Left, $screen.Top, 0, 0, $bitmap.Size) }
                finally { $graphics.Dispose() }
            } finally { $bitmap.Dispose() }
            $preflight.interactive_desktop = $true
        } catch {
            $preflight.reason = "Desktop capture unavailable: $($_.Exception.Message). No live phases ran (this is not a pass)."
        }
        $preflight.foreground_process = Get-ForegroundProcessName
        if ($preflight.interactive_desktop -and -not $ProbeOnly -and $firstRunProcesses -contains $preflight.foreground_process) {
            # Disposable runner only (checked above): turn the privacy experience off and close the screen by PID.
            try {
                New-Item -Force 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE' | Out-Null
                Set-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE' -Name DisablePrivacyExperience -Value 1 -Type DWord
            } catch { Write-Warning "Could not set DisablePrivacyExperience: $($_.Exception.Message)" }
            Stop-Process -Id ([WiriCiDesktop]::ForegroundPid()) -Force -ErrorAction SilentlyContinue
            Start-Sleep -Seconds 3
            $after = Get-ForegroundProcessName
            $preflight.first_run_screen_dismissed = -not ($firstRunProcesses -contains $after)
            $preflight.foreground_process = "$($preflight.foreground_process) -> $after"
            if (-not $preflight.first_run_screen_dismissed) {
                $preflight.interactive_desktop = $false
                $preflight.reason = "Windows' first-run privacy screen ($after) holds the foreground and could not be closed. No live phases ran (this is not a pass)."
            }
        }
        # Consoles open in the classic console host, one window each: with Windows Terminal as the default terminal
        # (both hosted images) the harness's three cmd.exe windows become tabs of one window (h9).
        if ($preflight.interactive_desktop -and -not $ProbeOnly) {
            $conhost = '{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}'
            try {
                New-Item -Force 'HKCU:\Console\%%Startup' | Out-Null
                Set-ItemProperty 'HKCU:\Console\%%Startup' -Name DelegationConsole -Value $conhost
                Set-ItemProperty 'HKCU:\Console\%%Startup' -Name DelegationTerminal -Value $conhost
                $preflight['default_terminal'] = 'conhost'
            } catch { Write-Warning "Could not make conhost the default terminal: $($_.Exception.Message)" }
        }
        # Then the Start menu / search it leaves open, which takes the first test keys.
        if ($preflight.interactive_desktop -and -not $ProbeOnly) {
            foreach ($i in 1..5) {
                if (@('SearchHost', 'StartMenuExperienceHost') -notcontains (Get-ForegroundProcessName)) { break }
                [WiriCiDesktop]::PressEscape()
                Start-Sleep -Milliseconds 700
            }
            $preflight.foreground_process = "$($preflight.foreground_process) -> $(Get-ForegroundProcessName)"
        }
    }
    if ($ProbeOnly) {
        $summary.status = 'probe-only'
        $preflight.reason += ' Probe only: no wiri, windows or key presses were started.'
    } elseif ($preflight.interactive_desktop) {
        foreach ($exe in 'wiri.exe', 'wiri-ctl.exe') {
            if (-not (Test-Path (Join-Path $Bin $exe) -PathType Leaf)) { throw "Missing live-test binary: $exe" }
        }
        # The original harness has a clean-slate process stop. Refuse rather than let it stop an unrelated instance.
        if (Get-Process wiri -ErrorAction SilentlyContinue) { throw 'A wiri process already exists; refusing to disturb it.' }
        # Keep the harness config/log/hidden-window records inside the disposable checkout too.
        $env:APPDATA = Join-Path $root '.scratch\live-profile\Roaming'
        $env:LOCALAPPDATA = Join-Path $root '.scratch\live-profile\Local'
        New-Item -ItemType Directory -Force $env:APPDATA, $env:LOCALAPPDATA | Out-Null
        # The harness recreates its output directory. Keep an outer result/preflight even if it times out.
        $summary.status = 'running'
        $summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $Out 'summary.json') -Encoding UTF8
        $preflight | ConvertTo-Json | Set-Content (Join-Path $Out 'ci-preflight.json') -Encoding UTF8
        # The hosted ARM64 image keeps relaunching a console "Press any key to install ... (WSL)" prompt, and shows
        # a "Windows created a temporary paging file" message box mid-run; both take the foreground in the middle
        # of phases.  Disposable runner: stop each prompt and close that message box while the harness runs.
        $wslGuard = Start-Job -ScriptBlock {
            Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Text;
using System.Runtime.InteropServices;
public static class WiriCiNoise {
    delegate bool EnumProc(IntPtr hwnd, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr hwnd, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr hwnd, uint msg, IntPtr w, IntPtr l);
    static string Text(IntPtr h, bool cls) { var s = new StringBuilder(512); if (cls) GetClassName(h, s, 512); else GetWindowText(h, s, 512); return s.ToString(); }
    // Closes visible message boxes (#32770) whose text mentions the paging file; returns how many.
    public static int ClosePagingFileNotices() {
        var found = new List<IntPtr>();
        EnumWindows((h, l) => {
            if (IsWindowVisible(h) && Text(h, true) == "#32770") {
                bool hit = false;
                EnumChildWindows(h, (c, l2) => { if (Text(c, false).IndexOf("paging file", StringComparison.OrdinalIgnoreCase) >= 0) hit = true; return true; }, IntPtr.Zero);
                if (hit) found.Add(h);
            }
            return true;
        }, IntPtr.Zero);
        foreach (var h in found) PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero); // WM_CLOSE
        return found.Count;
    }
}
'@
            # The prompt comes back every 30 s and wiri tiles and activates it within about 70 ms, faster than
            # any poll: stop it when the process starts (WMI process-start trace), and poll as a fallback.
            # The parent is recorded to find what relaunches it.
            $traced = $true
            try {
                Register-CimIndicationEvent -Query "SELECT * FROM Win32_ProcessStartTrace WHERE ProcessName='wsl.exe'" -SourceIdentifier WiriWslStart -ErrorAction Stop
            } catch { $traced = $false; "process-start trace unavailable: $($_.Exception.Message)" }
            function Parent([int]$parentId) {
                $p = Get-CimInstance Win32_Process -Filter "ProcessId=$parentId" -ErrorAction SilentlyContinue
                if ($p) { "$parentId $($p.Name) $($p.CommandLine)" } else { "$parentId (gone)" }
            }
            while ($true) {
                if ($traced) {
                    $e = Wait-Event -SourceIdentifier WiriWslStart -Timeout 1
                    if ($e) {
                        $n = $e.SourceEventArgs.NewEvent
                        Stop-Process -Id $n.ProcessID -Force -ErrorAction SilentlyContinue
                        "stopped wsl.exe pid $($n.ProcessID) at start $(Get-Date -Format HH:mm:ss.fff); parent $(Parent $n.ParentProcessID)"
                        Remove-Event -EventIdentifier $e.EventIdentifier -ErrorAction SilentlyContinue
                    }
                } else {
                    Start-Sleep -Milliseconds 500
                }
                foreach ($p in @(Get-CimInstance Win32_Process -Filter "Name='wsl.exe'" -ErrorAction SilentlyContinue)) {
                    Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
                    "stopped wsl.exe pid $($p.ProcessId) by poll at $(Get-Date -Format HH:mm:ss.fff); parent $(Parent $p.ParentProcessId)"
                }
                $closed = [WiriCiNoise]::ClosePagingFileNotices()
                if ($closed -gt 0) { "closed $closed paging-file notice(s) at $(Get-Date -Format HH:mm:ss.fff)" }
            }
        }
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\live-test.ps1') -Bin $Bin -Out $evidence
        if ($LASTEXITCODE -ne 0) { throw "Live harness exited with $LASTEXITCODE" }
        if (-not (Test-Path (Join-Path $evidence 'DONE'))) { throw 'Live harness did not finish; no DONE marker.' }
        $summary = Get-Content (Join-Path $evidence 'summary.json') -Raw | ConvertFrom-Json
        . (Join-Path $PSScriptRoot 'live-summary.ps1')
        $keys = @(Get-WiriLiveSummaryKeys -Harness (Join-Path $root 'scripts\live-test.ps1'))
        $failures = @(Get-WiriLiveSummaryFailures -Summary $summary -Keys $keys)
        $failed = $failures.Count -gt 0
        $status = if ($failed) { 'failed' } elseif (@($summary.ci_skips).Count) { 'passed-with-skips' } else { 'passed' }
        if ($failed) {
            Write-Warning ("Failed live gates: " + ($failures -join ', '))
            # Print each failed phase's values and its small evidence files, so a hosted-runner failure can be
            # read from the job log without downloading the artifact (screenshots make it tens of MB).
            $printed = 0
            foreach ($p in @($failures | ForEach-Object { if ("$_" -match '^([a-z]+[0-9]+)_') { $Matches[1] } } | Sort-Object -Unique)) {
                Write-Output "---- $p values"
                foreach ($prop in $summary.PSObject.Properties) {
                    if ($prop.Name -like "${p}_*") { Write-Output ('  {0} = {1}' -f $prop.Name, (ConvertTo-Json $prop.Value -Compress -Depth 4)) }
                }
                $small = Get-ChildItem $evidence -Filter "$p-*" -File -ErrorAction SilentlyContinue |
                    Where-Object { $_.Extension -in '.json', '.txt' -and $_.Length -le 8192 } | Sort-Object LastWriteTime
                foreach ($f in @($small)) {
                    if ($printed + $f.Length -gt 65536) { Write-Output "---- (evidence output capped at 64 KB)"; break }
                    $printed += $f.Length
                    Write-Output "---- $($f.Name)"
                    Write-Output (Get-Content $f.FullName -Raw)
                }
            }
        }
        $summary | Add-Member -NotePropertyName status -NotePropertyValue $status -Force
        $summary | Add-Member -NotePropertyName ci_failed_gates -NotePropertyValue $failures -Force
        $summary | Add-Member -NotePropertyName ci -NotePropertyValue $preflight -Force
    } else {
        Write-Warning $preflight.reason
    }
} catch {
    $failed = $true
    $preflight.reason = $_.Exception.Message
    $summary | Add-Member -NotePropertyName status -NotePropertyValue 'failed' -Force
    Write-Warning $preflight.reason
} finally {
    if ($wslGuard) {
        Stop-Job $wslGuard -ErrorAction SilentlyContinue
        $preflight['wsl_prompts_stopped'] = @(Receive-Job $wslGuard -ErrorAction SilentlyContinue | ForEach-Object { "$_" })
        Remove-Job $wslGuard -Force -ErrorAction SilentlyContinue
    }
    $env:APPDATA = $oldAppData
    $env:LOCALAPPDATA = $oldLocalAppData
    # Also leave an explicit result when the desktop probe skipped everything.
    New-Item -ItemType Directory -Force $Out | Out-Null
    $summary | Add-Member -NotePropertyName ci -NotePropertyValue $preflight -Force
    $summary | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $Out 'summary.json') -Encoding UTF8
    $preflight | ConvertTo-Json | Set-Content (Join-Path $Out 'ci-preflight.json') -Encoding UTF8
    $message = "Experimental live test: $($summary.status). $($preflight.reason) See the live artifact's summary.json, ci_skips, ci_failed_gates and any harness screenshots."
    Write-Output $message
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content $env:GITHUB_STEP_SUMMARY $message }
}
if ($failed) { exit 1 }
