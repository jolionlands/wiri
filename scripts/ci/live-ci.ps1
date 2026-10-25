# Experimental hosted-runner adapter. Never schedules a task, unlocks a session, or installs GUI software.
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
    reason = ''
}
$summary = [pscustomobject]@{ status = 'skipped'; ci = $preflight }
$oldAppData = $env:APPDATA
$oldLocalAppData = $env:LOCALAPPDATA
$failed = $false
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
}
'@
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
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File (Join-Path $root 'scripts\live-test.ps1') -Bin $Bin -Out $evidence
        if ($LASTEXITCODE -ne 0) { throw "Live harness exited with $LASTEXITCODE" }
        if (-not (Test-Path (Join-Path $evidence 'DONE'))) { throw 'Live harness did not finish; no DONE marker.' }
        $summary = Get-Content (Join-Path $evidence 'summary.json') -Raw | ConvertFrom-Json
        . (Join-Path $PSScriptRoot 'live-summary.ps1')
        $keys = @(Get-WiriLiveSummaryKeys -Harness (Join-Path $root 'scripts\live-test.ps1'))
        $failures = @(Get-WiriLiveSummaryFailures -Summary $summary -Keys $keys)
        $failed = $failures.Count -gt 0
        $status = if ($failed) { 'failed' } elseif (@($summary.ci_skips).Count) { 'passed-with-skips' } else { 'passed' }
        if ($failed) { Write-Warning ("Failed live gates: " + ($failures -join ', ')) }
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
