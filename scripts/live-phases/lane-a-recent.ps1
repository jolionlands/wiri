# Phase 2B / H20: recent-windows, adapted from h20.fragment.ps1 + h20.integration.md.
# Dot-sourced by live-test.ps1 after its helpers/$summary exist, with no daemon running.
# Do not also source h20.fragment.ps1. This is not a standalone script.
# Owns only its daemon and three in-process WinForms fixtures. No display settings are changed.
# Gates describe observations from a future harness run, not validation performed on authoring.
# A/O/W/S checks prove modal scope changes, NOT distinct-output/workspace filtering. The fixtures
# share one executable identity: title independence is tested, distinct-app exclusion is not.
# Optional off/empty-binds/precedence/reload/delayed-show/close/screenshot coverage is not included.
# If adding disabled/reload coverage, use custom Ctrl+Alt+F8/F9, NEVER shell/native Alt+Tab.

function H20RHosts {
    if ($h20rProcId -le 0) { return }
    foreach ($h in [LT]::TopLevel()) {
        if ([LT]::Class($h) -eq 'WiriRecentWindowsHost' -and [LT]::Pid($h) -eq $h20rProcId) { $h }
    }
}
function H20RHost {
    $hosts = @(H20RHosts)
    if ($hosts.Count -eq 1) { return $hosts[0] }
    return [IntPtr]::Zero
}
function H20RProp([string]$name) {
    $hostWindow = H20RHost
    if ($hostWindow -eq [IntPtr]::Zero) { return [long]0 }
    return [H20RecentNative]::GetProp($hostWindow, $name).ToInt64()
}
function H20RBounds([IntPtr]$h) {
    $r = New-Object LT+RECT
    if (-not [LT]::GetWindowRect($h, [ref]$r)) { return 'missing' }
    return "$($r.L),$($r.T),$($r.R),$($r.B)"
}
function H20RObserve {
    if ($null -eq $h20rWatch) { return }
    $now = @($h20rForms | ForEach-Object { H20RBounds $_ }) -join ';'
    $foreground = [LT]::GetForegroundWindow()
    $boundsChanged = $now -ne $h20rWatch.bounds
    $focusChanged = $foreground -ne $h20rWatch.origin
    if ($boundsChanged -or $focusChanged) {
        $h20rWatch.ok = $false
        if ($null -eq $h20rWatch.first_mismatch) {
            $h20rWatch.first_mismatch = [ordered]@{
                at_utc = [DateTime]::UtcNow.ToString('o'); bounds_changed = $boundsChanged
                focus_changed = $focusChanged; observed_bounds = $now; foreground = [int64]$foreground
                foreground_title = [LT]::Title($foreground); foreground_class = [LT]::Class($foreground)
                foreground_pid = [LT]::Pid($foreground)
            }
        }
    }
    $h20rWatch.samples++
}
function H20RWait([scriptblock]$condition, [int]$timeoutMs = 4000) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    do {
        H20RObserve
        if (& $condition) { return $true }
        if ($clock.ElapsedMilliseconds -ge $timeoutMs) { return $false }
        Start-Sleep -Milliseconds 25
    } while ($true)
}
function H20RRequire([bool]$ok, [string]$message) {
    if (-not $ok) { throw $message }
}
function H20RTap([byte]$key) {
    try { [LT]::Key($key, $false); Start-Sleep -Milliseconds 30 }
    finally { [LT]::Key($key, $true) }
    # No drainer-completion assumption here: callers poll the actual host/selection/focus.
}
function H20RRelease {
    # Include both physical sides as well as generic keys, including keys used in failure paths.
    foreach ($key in @(0x09,0xC0,0x58,0x1B,0x25,0x27,0x24,0x23,0x41,0x4F,0x57,0x53,
                       0x10,0x11,0x12,0xA0,0xA1,0xA2,0xA3,0xA4,0xA5,0x5B,0x5C)) {
        try { [LT]::Key([byte]$key, $true) }
        catch { $h20rIssues.Add("Key release $key : $($_.Exception.Message)") }
    }
}
function H20RShown {
    $hostWindow = H20RHost
    return ($hostWindow -ne [IntPtr]::Zero -and [LT]::IsWindowVisible($hostWindow) -and
        [LT]::Cloaked($hostWindow) -eq 0 -and (H20RProp 'WiriRecentThumbnailCount') -gt 0 -and
        (H20RProp 'WiriRecentSelectedWindow') -ne 0)
}
function H20ROpen([byte]$trigger = 0x09) {
    [LT]::Key(0x12, $false)
    H20RTap $trigger
    return (H20RWait { H20RShown })
}
function H20RSelected([long]$hwnd) {
    return (H20RWait { (H20RShown) -and (H20RProp 'WiriRecentSelectedWindow') -eq $hwnd })
}
function H20RGone { return (H20RWait { @(H20RHosts).Count -eq 0 }) }
function H20REndSession {
    try {
        if (@(H20RHosts).Count -gt 0) { H20RTap 0x1B; $null = H20RGone }
    } finally { H20RRelease }
}
function H20RHold([int]$durationMs, [IntPtr]$origin, [long]$selected, [string]$heldModifier) {
    # A negative assertion is observed continuously, not inferred from a single 100ms sleep.
    $started = [DateTime]::UtcNow.ToString('o')
    $clock = [Diagnostics.Stopwatch]::StartNew(); $ok = $true; $samples = 0; $firstFailure = $null
    do {
        H20RObserve
        $shown = H20RShown; $foreground = [LT]::GetForegroundWindow()
        $actualSelection = H20RProp 'WiriRecentSelectedWindow'
        if (-not $shown -or $foreground -ne $origin -or $actualSelection -ne $selected) {
            $ok = $false
            if ($null -eq $firstFailure) {
                $firstFailure = [ordered]@{
                    at_utc = [DateTime]::UtcNow.ToString('o'); elapsed_ms = $clock.ElapsedMilliseconds
                    host_shown = $shown; foreground = [int64]$foreground; selected = $actualSelection
                    foreground_title = [LT]::Title($foreground); foreground_class = [LT]::Class($foreground)
                }
            }
        }
        $samples++
        Start-Sleep -Milliseconds 25
    } while ($clock.ElapsedMilliseconds -lt $durationMs)
    $h20rEvidence['modifier_holds'] += [ordered]@{
        held_modifier = $heldModifier; started_at_utc = $started; duration_ms = $durationMs
        expected_foreground = [int64]$origin; expected_selection = $selected
        samples = $samples; ok = $ok; first_failure = $firstFailure
    }
    return $ok
}
function H20RHoldCancelled([int]$durationMs, [IntPtr]$origin) {
    $clock = [Diagnostics.Stopwatch]::StartNew(); $ok = $true
    do {
        if ([LT]::GetForegroundWindow() -ne $origin -or @(H20RHosts).Count -ne 0) { $ok = $false }
        Start-Sleep -Milliseconds 25
    } while ($clock.ElapsedMilliseconds -lt $durationMs)
    return $ok
}
function H20RSeed {
    # Establish C,B,A MRU with actual focus acknowledgement and >750ms residence per window.
    # Deliberately NOT used in either sub-debounce race below.
    foreach ($h in $h20rForms) {
        $h20rState.sequence++
        $null = Ctl "h20-seed-$($h20rState.sequence)" @('focus-window', [string][int64]$h)
        H20RRequire (H20RWait { [LT]::GetForegroundWindow() -eq $h }) 'Cannot focus H20 seed fixture.'
        Start-Sleep -Milliseconds 1050
    }
}
function H20RFastFocus([IntPtr]$h, [string]$tag, $clock, [int]$deadlineMs) {
    if ($clock.ElapsedMilliseconds -ge $deadlineMs) { return $false }
    $null = Ctl $tag @('focus-window', [string][int64]$h)
    # Ctl itself can take up to five seconds: a slow CLI is a FAILED timing precondition,
    # never evidence that debounce or keyboard-input bypass passed.
    $remaining = $deadlineMs - [int]$clock.ElapsedMilliseconds
    if ($remaining -le 0) { return $false }
    return ((H20RWait { [LT]::GetForegroundWindow() -eq $h } $remaining) -and
        $clock.ElapsedMilliseconds -lt $deadlineMs)
}
function H20RScope([byte]$key, [long]$scope) {
    H20RTap $key
    return (H20RWait { (H20RShown) -and (H20RProp 'WiriRecentScope') -eq $scope })
}

function H20RReloadConfig([string]$text, [string]$tag) {
    # The daemon watches its config file, so the write can trigger a reload of its own.
    # The gate waits for the daemon's own confirmation line: the IPC reply can be sent
    # before the new config is in use, so the reply alone proves nothing.
    [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
    $before = @(LogLines $h20rLog 'Config reloaded successfully').Count
    $null = Ctl $tag @('reload-config')
    return (H20RWait { @(LogLines $h20rLog 'Config reloaded successfully').Count -gt $before } 2500)
}
function H20RHostMax([int]$durationMs, [int]$intervalMs = 25) {
    # A host that lives for less than one poll interval must still be seen, so the
    # count is sampled all the time and not read once at the end.
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $max = 0; $samples = 0
    do {
        $count = @(H20RHosts).Count
        if ($count -gt $max) { $max = $count }
        $samples++
        Start-Sleep -Milliseconds $intervalMs
    } while ($clock.ElapsedMilliseconds -lt $durationMs)
    return [pscustomobject]@{ max = $max; samples = $samples; elapsed_ms = $clock.ElapsedMilliseconds }
}
function H20RStartEvents([string]$tag) {
    # A live subscriber to the daemon's event stream.  Every event type is taken and
    # filtered here, so the gate does not depend on the daemon's filter names.
    $out = Join-Path $D "$tag.jsonl"
    $err = Join-Path $D "$tag.err"
    Remove-Item -Force $out, $err -ErrorAction SilentlyContinue
    $p = Start-Process -FilePath $ctl -ArgumentList @('event-stream','--stream') `
        -RedirectStandardOutput $out -RedirectStandardError $err -NoNewWindow -PassThru
    Start-Sleep -Milliseconds 600   # the subscription must be live before the focus changes
    return [pscustomobject]@{ proc = $p; out = $out; err = $err }
}
function H20RStopEvents($stream) {
    $events = @()
    try {
        if (-not $stream.proc.HasExited) { $stream.proc.Kill() }
        $null = $stream.proc.WaitForExit(3000)
    } catch { }
    foreach ($line in @(Get-Content $stream.out -ErrorAction SilentlyContinue)) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        try { $events += ($line | ConvertFrom-Json) } catch { }
    }
    return $events
}
function H20RStampNs($event) {
    # The event carries {"secs":N,"nanos":M}: compare the stamps as nanoseconds.
    if ($null -eq $event -or $null -eq $event.data -or $null -eq $event.data.focus_timestamp) { return -1 }
    return ([double]$event.data.focus_timestamp.secs * 1e9) + [double]$event.data.focus_timestamp.nanos
}
$h20rGates = @('real_dwm_previews','repeated_cycle','sources_unchanged','modifier_release_commit',
    'cancel_restores_origin','scope_navigation','preview_height_cap','debounce_skips_intermediate',
    'keyboard_input_commits','app_id_ignores_titles','no_hosts_after_quit','fragment_completed',
    'recent_windows_off_creates_no_host','recent_windows_off_still_timestamps_focus')
foreach ($gate in $h20rGates) { $summary["h20_$gate"] = $false }
$h20rProc = $null; $h20rProcId = 0; $h20rForms = @(); $h20rWatch = $null
$h20rLog = Join-Path $D 'h20-stderr.log'
$h20rOldDpi = [IntPtr]::Zero
$h20rIssues = New-Object 'System.Collections.Generic.List[string]'
$h20rState = @{ sequence = 0 }
$h20rEvidence = [ordered]@{
    scope_coverage = 'A/O/W/S mode changes only; no distinct-output/workspace filtering claim'
    identity_coverage = 'same executable, different/changed titles only; no distinct-app exclusion claim'
    debounce_ms = 750; configured_max_height = 96; max_scale = 0.5; padding = 30
    modifier_holds = @()
}
$h20rConfig = @'
input { mod-key "ctrl-alt"; }
layout { gaps 16; }
animations { off; }
recent-windows {
    debounce-ms 750
    open-delay-ms 150
    highlight { padding 30; }
    previews { max-height 96; max-scale 0.5; }
    binds {
        Alt+Tab { next-window; }
        Alt+Shift+Tab { previous-window; }
        Alt+grave { next-window filter="app-id"; }
    }
}
'@
try {
    if (-not ('H20RecentNative' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H20RecentNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct MONITORINFO {
        public uint cbSize; public RECT monitor, work; public uint flags;
    }
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr hwnd, string name);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool GetMonitorInfo(IntPtr monitor, ref MONITORINFO info);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int kind, out uint x, out uint y);
    public static MONITORINFO Info(IntPtr monitor) {
        MONITORINFO info = new MONITORINFO(); info.cbSize = (uint)Marshal.SizeOf(typeof(MONITORINFO));
        if (!GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed");
        return info;
    }
}
'@ -ErrorAction Stop
    }
    $h20rOldDpi = [H20RecentNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
    H20RRequire ($h20rOldDpi -ne [IntPtr]::Zero) 'Cannot establish physical-pixel, per-monitor-v2 observation context.'
    $h20rProc = Start-WiriWithConfig $h20rConfig $h20rLog
    $h20rProcId = $h20rProc.Id
    H20RRequire (-not $h20rProc.HasExited) 'H20 daemon exited during startup.'
    # Cache the live process handle, as Ctl does: otherwise Start-Process's object
    # can report an empty ExitCode after exit even when WaitForExit succeeds.
    $null = $h20rProc.Handle
    $h20rForms += [TestForms]::OpenKeys('wiri-h20-title-alpha-key-recorder')
    $h20rForms += [TestForms]::Open('wiri-h20-title-bravo', 130, 90, 120)
    $h20rForms += [TestForms]::Open('wiri-h20-title-charlie', 70, 150, 120)
    foreach ($h in $h20rForms) { H20RRequire ($h -ne [IntPtr]::Zero -and [LT]::IsWindow($h)) 'H20 fixture creation failed.' }
    H20RRequire (H20RWait {
        $h20rState.sequence++
        $list = WinList "h20-managed-$($h20rState.sequence)"
        $managed = @($h20rForms | Where-Object { $null -ne (WinOf $list $_) })
        $managed.Count -eq 3
    } 6000) 'H20 fixtures were not all managed.'
    H20RSeed
    $h20rOrigin = $h20rForms[2]
    $h20rWatch = [pscustomobject]@{
        bounds = (@($h20rForms | ForEach-Object { H20RBounds $_ }) -join ';')
        origin = $h20rOrigin; ok = $true; samples = 0; first_mismatch = $null
    }
    $h20rEvidence['source_watch'] = $h20rWatch
    try {
        H20RRequire (H20ROpen) 'No visible live recent-windows host after Alt+Tab.'
        $summary.h20_real_dwm_previews = H20RWait { (H20RShown) -and (H20RProp 'WiriRecentThumbnailCount') -ge 2 }
        # Thumbnail count is successful DWM registrations, NEVER the candidate-list length.
        $h20rEvidence['registered_thumbnails'] = H20RProp 'WiriRecentThumbnailCount'
        $cycle1 = H20RSelected ([int64]$h20rForms[1])
        H20RTap 0x09; $cycle2 = H20RSelected ([int64]$h20rForms[0])
        try {
            [LT]::Key(0x10, $false); H20RTap 0x09
            $cycle3 = H20RSelected ([int64]$h20rForms[1])
        } finally { [LT]::Key(0x10, $true) }
        H20RTap 0x27; $cycle4 = H20RSelected ([int64]$h20rForms[0])
        $summary.h20_repeated_cycle = ($cycle1 -and $cycle2 -and $cycle3 -and $cycle4)
        Shot 'h20-live-dwm-previews'

        $hostWindow = H20RHost
        $monitor = [H20RecentNative]::MonitorFromWindow($hostWindow, 2)
        $info = [H20RecentNative]::Info($monitor)
        [uint32]$dpiX = 0; [uint32]$dpiY = 0
        $dpiResult = [H20RecentNative]::GetDpiForMonitor($monitor, 0, [ref]$dpiX, [ref]$dpiY)
        $windowDpi = [H20RecentNative]::GetDpiForWindow($hostWindow)
        $mw = $info.monitor.R - $info.monitor.L; $mh = $info.monitor.B - $info.monitor.T
        $scale = $dpiX / 96.0
        $pad = [Math]::Floor(30 * $scale + 0.5)
        $cap = [Math]::Min(96 * $scale, [Math]::Min($mh * 0.5, [Math]::Max(1, $mh - 2 * $pad - 70)))
        $previewHeight = H20RProp 'WiriRecentPreviewHeight'
        $source = [LT]::Visible([IntPtr](H20RProp 'WiriRecentSelectedWindow'))
        $sw = [Math]::Max(1, $source.R - $source.L); $sh = [Math]::Max(1, $source.B - $source.T)
        # Require the height limit to bind, so a small source cannot accidentally mask a broken cap.
        $capBinds = ($cap -lt $sh * 0.5 -and $cap -lt $sh * [Math]::Max(1, $mw - 2 * $pad) / $sw)
        $summary.h20_preview_height_cap = ($dpiResult -eq 0 -and $dpiX -gt 0 -and $dpiX -eq $dpiY -and
            $windowDpi -eq $dpiX -and $capBinds -and $previewHeight -gt 0 -and
            $previewHeight -le [Math]::Floor($cap) -and $previewHeight -ge [Math]::Floor($cap) - 1)
        $h20rEvidence['geometry'] = [ordered]@{
            host = [int64]$hostWindow; host_bounds = (H20RBounds $hostWindow)
            monitor_bounds = "$($info.monitor.L),$($info.monitor.T),$($info.monitor.R),$($info.monitor.B)"
            monitor_dpi_x = $dpiX; monitor_dpi_y = $dpiY; dpi_query_result = $dpiResult; host_dpi = $windowDpi
            physical_height_cap = $cap; observed_preview_height = $previewHeight; cap_is_binding = $capBinds
            source_visible_bounds = "$($source.L),$($source.T),$($source.R),$($source.B)"
            observed_1366x768 = ($mw -eq 1366 -and $mh -eq 768)
        }

        # Pairwise hand-offs avoid Ctrl+Alt+Shift+Win (the shell's Office-key chord).
        # Each OTHER modifier family must sustain the session ALONE after opening Alt is up.
        [LT]::Key(0x10, $false); [LT]::Key(0x12, $true)
        $held1 = H20RHold 1000 $h20rOrigin ([int64]$h20rForms[0]) 'Shift'
        [LT]::Key(0x5B, $false); [LT]::Key(0x10, $true)
        $held2 = H20RHold 500 $h20rOrigin ([int64]$h20rForms[0]) 'Win'
        # Keep Ctrl held across Win-up rather than finish on a bare Win release.
        # Any shell focus/layout change still fails the same strict gates.
        [LT]::Key(0x11, $false); [LT]::Key(0x5B, $true)
        $held3 = H20RHold 500 $h20rOrigin ([int64]$h20rForms[0]) 'Ctrl'
        H20RObserve
        $summary.h20_sources_unchanged = ($h20rWatch.ok -and $h20rWatch.samples -gt 0)
        $h20rEvidence['source_watch'] = $h20rWatch
        $h20rWatch = $null # Focus may change only AFTER the last physical modifier is released.
        $releaseAt = [DateTime]::UtcNow.ToString('o')
        [LT]::Key(0x11, $true)
        $committed = H20RWait { [LT]::GetForegroundWindow() -eq $h20rForms[0] -and @(H20RHosts).Count -eq 0 }
        $summary.h20_modifier_release_commit = ($held1 -and $held2 -and $held3 -and $committed)
        $h20rEvidence['held_modifier_observations'] = @($held1, $held2, $held3)
        $h20rEvidence['modifier_commit'] = [ordered]@{
            final_modifier = 'Ctrl'; released_at_utc = $releaseAt; committed = $committed
            foreground = [int64][LT]::GetForegroundWindow(); remaining_hosts = @(H20RHosts).Count
        }
    } finally { $h20rWatch = $null; H20REndSession }

    H20RSeed
    try {
        H20RRequire (H20ROpen) 'Cannot open for scope/cancel checks.'
        $scopeO = H20RScope 0x4F 1; $scopeW = H20RScope 0x57 2; $scopeA = H20RScope 0x41 0
        $scopeS1 = H20RScope 0x53 1; $scopeS2 = H20RScope 0x53 2; $scopeS3 = H20RScope 0x53 0
        $summary.h20_scope_navigation = ($scopeO -and $scopeW -and $scopeA -and $scopeS1 -and $scopeS2 -and $scopeS3)
        H20RTap 0x1B; $escaped = H20RGone
        H20RRelease
        $summary.h20_cancel_restores_origin = ($escaped -and (H20RHoldCancelled 800 $h20rOrigin))
    } finally { H20REndSession }

    H20RSeed
    try {
        $race = [Diagnostics.Stopwatch]::StartNew()
        $focusedA = H20RFastFocus $h20rForms[0] 'h20-transient-a' $race 450
        $atA = $race.ElapsedMilliseconds
        $backC = $false
        if ($focusedA) { $backC = H20RFastFocus $h20rOrigin 'h20-transient-back-c' $race 650 }
        $atC = $race.ElapsedMilliseconds
        $h20rEvidence['transient_focus_timing'] = @{ focus_a_ms = $atA; back_c_ms = $atC; valid = ($focusedA -and $backC) }
        if ($focusedA -and $backC) {
            H20RRequire (H20ROpen) 'Cannot inspect transient-focus MRU.'
            $summary.h20_debounce_skips_intermediate = H20RSelected ([int64]$h20rForms[1])
        } else { $h20rIssues.Add('Transient-focus timing precondition missed (<650ms); debounce gate remains false.') }
    } finally { H20REndSession }

    H20RSeed
    try {
        [TestForms]::ClearKeys()
        $race = [Diagnostics.Stopwatch]::StartNew()
        $focusedA = H20RFastFocus $h20rForms[0] 'h20-input-focus-a' $race 450
        $atA = $race.ElapsedMilliseconds; $atX = -1L; $receivedX = $false; $backC = $false
        # Critical ordering: acknowledge ACTUAL foreground, then type an application X before
        # the 750ms debounce deadline. Never replace this with the harness's slow FocusForm.
        if ($focusedA -and [LT]::GetForegroundWindow() -eq $h20rForms[0] -and $race.ElapsedMilliseconds -lt 500) {
            H20RTap 0x58
            $atX = $race.ElapsedMilliseconds
            $remaining = 550 - [int]$race.ElapsedMilliseconds
            if ($remaining -gt 0) { $receivedX = H20RWait { [TestForms]::KeyLog() -match '(^|,)X(,|$)' } $remaining }
            if ($receivedX -and $race.ElapsedMilliseconds -lt 550) {
                $backC = H20RFastFocus $h20rOrigin 'h20-input-back-c' $race 650
            }
        }
        $atC = $race.ElapsedMilliseconds
        $validInputRace = ($focusedA -and $atX -ge 0 -and $atX -lt 550 -and $receivedX -and $backC -and $atC -lt 650)
        $h20rEvidence['application_input_timing'] = @{
            focus_a_ms = $atA; x_key_up_ms = $atX; back_c_ms = $atC
            key_log = [TestForms]::KeyLog(); valid = $validInputRace
        }
        if ($validInputRace) {
            H20RRequire (H20ROpen) 'Cannot inspect application-input MRU.'
            $summary.h20_keyboard_input_commits = H20RSelected ([int64]$h20rForms[0])
        } else { $h20rIssues.Add('Application-input timing/foreground/delivery precondition missed; bypass gate remains false.') }
    } finally { H20REndSession }

    [TestForms]::SetTitle($h20rForms[1], 'H20 RENAMED unrelated document - different from alpha and charlie')
    $renamed = H20RWait { [LT]::Title($h20rForms[1]) -eq 'H20 RENAMED unrelated document - different from alpha and charlie' }
    H20RSeed
    try {
        H20RRequire (H20ROpen 0xC0) 'Cannot open application-filtered session.'
        $appFirst = H20RSelected ([int64]$h20rForms[1])
        H20RTap 0xC0; $appSecond = H20RSelected ([int64]$h20rForms[0])
        $fixturePids = @($h20rForms | ForEach-Object { [LT]::Pid($_) } | Select-Object -Unique)
        $summary.h20_app_id_ignores_titles = ($renamed -and $appFirst -and $appSecond -and $fixturePids.Count -eq 1)
        $h20rEvidence['fixture_titles'] = @($h20rForms | ForEach-Object { [LT]::Title($_) })
        Shot 'h20-same-app-different-titles'
    } finally { H20REndSession }

    # ---- recent-windows { off }: the switcher must not intercept -------------------------
    # The chord is a custom one (Ctrl+Alt+F8): while the switcher is off the key is NOT
    # swallowed, and a native Alt+Tab would reach the shell and switch the desktop behind
    # the lane's back.  The lane's own config is restored at the end of this section, so
    # the remaining gates run against the config they were written for.
    $h20rDefaultsFile = Join-Path $D 'legacy-default-binds.kdl'
    $h20rDefaultsInclude = ''
    if (Test-Path $h20rDefaultsFile) { $h20rDefaultsInclude = 'include r#"' + $h20rDefaultsFile + '"#' + "`n" }
    $h20rOffConfig = $h20rDefaultsInclude + @'
input { mod-key "ctrl-alt"; }
layout { gaps 16; }
animations { off; }
recent-windows {
    off
    debounce-ms 750
    open-delay-ms 150
    highlight { padding 30; }
    previews { max-height 96; max-scale 0.5; }
    binds {
        Ctrl+Alt+F8 { next-window; }
        Ctrl+Alt+F9 { previous-window; }
    }
}
'@
    $h20rOffEvidence = [ordered]@{
        off_config = 'recent-windows { off; binds { Ctrl+Alt+F8/F9 } }'
        reload_applied = $false
        hosts_before = -1
        max_hosts_while_off = -1
        host_samples = 0
        key_log_while_off = ''
        chord_passed_through = $false
        opened_closed_while_off = $false
        max_hosts_after_open_close = -1
        focus_events = 0
        first_stamp_ns = -1
        last_stamp_ns = -1
        stamp_advanced = $false
        restored = $false
        host_after_restore = $false
        restore_ms = -1
        daemon_alive = $false
    }
    try {
        $h20rOffEvidence.reload_applied = H20RReloadConfig $h20rOffConfig 'h20-recent-off-reload'
        H20RRequire $h20rOffEvidence.reload_applied 'The daemon did not apply the recent-windows { off; } config.'
        $h20rOffEvidence.hosts_before = @(H20RHosts).Count
        # The recorder must own the keyboard: the pass-through is only observable when
        # the chord reaches the application.
        $recorder = $h20rForms[0]
        $null = Ctl 'h20-off-focus-recorder' @('focus-window', [string][int64]$recorder)
        H20RRequire (H20RWait { [LT]::GetForegroundWindow() -eq $recorder } 3000) 'Cannot focus the key recorder.'
        [TestForms]::ClearKeys()
        # The chord of the disabled switcher: no host may appear while it is off.
        [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
        Start-Sleep -Milliseconds 40
        [LT]::Key(0x77, $true)      # F8
        Start-Sleep -Milliseconds 80
        [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
        $watch = H20RHostMax 2500
        $h20rOffEvidence.max_hosts_while_off = $watch.max
        $h20rOffEvidence.host_samples = $watch.samples
        $h20rOffEvidence.key_log_while_off = [TestForms]::KeyLog()
        $h20rOffEvidence.chord_passed_through = ([TestForms]::KeyLog() -match '(^|,)F8(,|$)')
        # Open and close an owned window while the switcher is off: neither may make a host.
        $opened = [TestForms]::Open('wiri-h20-off-window', 120, 60, 150)
        $h20rForms += $opened
        $null = Ctl 'h20-off-focus-new' @('focus-window', [string][int64]$opened)
        $null = H20RWait { [LT]::GetForegroundWindow() -eq $opened } 3000
        [TestForms]::CloseForm($opened)
        $h20rOffEvidence.opened_closed_while_off = (H20RWait { -not [LT]::IsWindow($opened) } 3000)
        $watch2 = H20RHostMax 1200
        $h20rOffEvidence.max_hosts_after_open_close = $watch2.max
        # The focus timestamps do not depend on the switcher: they must still be published.
        $stream = H20RStartEvents 'h20-off-focus-events'
        try {
            $focusOrder = @($h20rForms[1], $recorder, $h20rForms[1], $recorder)
            for ($i = 0; $i -lt $focusOrder.Count; $i++) {
                $null = Ctl "h20-off-focus-$i" @('focus-window', [string][int64]$focusOrder[$i])
                $null = H20RWait { [LT]::GetForegroundWindow() -eq $focusOrder[$i] } 3000
                Start-Sleep -Milliseconds 350
            }
        } finally { $events = H20RStopEvents $stream }
        $recorderId = [int64]$recorder
        $mine = @($events | Where-Object {
            $_.type -ceq 'window_focus_timestamp_changed' -and $null -ne $_.data -and [int64]$_.data.id -eq $recorderId })
        $stamps = @($mine | ForEach-Object { H20RStampNs $_ } | Where-Object { $_ -ge 0 })
        $h20rOffEvidence.focus_events = $mine.Count
        if ($stamps.Count -ge 2) {
            $h20rOffEvidence.first_stamp_ns = $stamps[0]
            $h20rOffEvidence.last_stamp_ns = $stamps[-1]
            $h20rOffEvidence.stamp_advanced = ($stamps[-1] -gt $stamps[0])
        }
        # The lane's own config back: the switcher must work again.
        $h20rOffEvidence.restored = H20RReloadConfig ($h20rDefaultsInclude + $h20rConfig) 'h20-recent-on-reload'
        H20RRequire $h20rOffEvidence.restored 'The daemon did not restore the default recent-windows config.'
        $restoreClock = [Diagnostics.Stopwatch]::StartNew()
        $h20rOffEvidence.host_after_restore = H20ROpen
        $h20rOffEvidence.restore_ms = $restoreClock.ElapsedMilliseconds
        H20REndSession
        $h20rOffEvidence.daemon_alive = (-not $h20rProc.HasExited)
        $summary.h20_recent_windows_off_creates_no_host = ($h20rOffEvidence.reload_applied -and
            $h20rOffEvidence.hosts_before -eq 0 -and $h20rOffEvidence.max_hosts_while_off -eq 0 -and
            $h20rOffEvidence.chord_passed_through -and $h20rOffEvidence.opened_closed_while_off -and
            $h20rOffEvidence.max_hosts_after_open_close -eq 0 -and $h20rOffEvidence.restored -and
            $h20rOffEvidence.host_after_restore -and $h20rOffEvidence.daemon_alive)
        $summary.h20_recent_windows_off_still_timestamps_focus = ($h20rOffEvidence.focus_events -ge 2 -and
            $h20rOffEvidence.stamp_advanced -and $h20rOffEvidence.max_hosts_while_off -eq 0 -and
            $h20rOffEvidence.daemon_alive)
        $h20rEvidence['recent_windows_off'] = $h20rOffEvidence
    } catch {
        $h20rIssues.Add("recent-windows off: $($_.Exception.Message)")
        $h20rEvidence['recent_windows_off'] = $h20rOffEvidence
    } finally { H20RRelease }

    # Quit WHILE the real host exists, not just after a cancellation already destroyed it.
    try {
        H20RRequire (H20ROpen) 'Cannot create an active preview host for quit teardown.'
        $quitHosts = @(H20RHosts)
        $liveBeforeQuit = H20RShown
        # Preserve evidence for every conjunct; a false gate must not imply a host leak
        # when it was actually a missing exit code or a failed live-host precondition.
        $quitEvidence = [ordered]@{
            started_at_utc = [DateTime]::UtcNow.ToString('o'); live_before_quit = $liveBeforeQuit
            hosts_before = @($quitHosts | ForEach-Object { [int64]$_ }); single_host_before = ($quitHosts.Count -eq 1)
            wait_timeout_ms = 10000; exited = $null; exit_code = $null; hosts_gone = $null
            captured_hosts_destroyed = $null; surviving_captured_hosts = $null; remaining_hosts = $null
        }
        $h20rEvidence['quit'] = $quitEvidence
        $null = Ctl 'h20-quit-active' @('quit')
        $exited = $h20rProc.WaitForExit(10000)
        $quitEvidence.exited = $exited
        if ($exited) { $quitEvidence.exit_code = $h20rProc.ExitCode }
        $gone = H20RGone
        $quitEvidence.hosts_gone = $gone
        $survivors = @($quitHosts | Where-Object { [LT]::IsWindow($_) })
        $oldHostsDestroyed = $survivors.Count -eq 0
        $quitEvidence.captured_hosts_destroyed = $oldHostsDestroyed
        $quitEvidence.surviving_captured_hosts = @($survivors | ForEach-Object { [int64]$_ })
        $quitEvidence.remaining_hosts = @(H20RHosts | ForEach-Object { [int64]$_ })
        $summary.h20_no_hosts_after_quit = ($liveBeforeQuit -and $quitHosts.Count -eq 1 -and
            $exited -and $quitEvidence.exit_code -eq 0 -and $gone -and $oldHostsDestroyed)
    } finally { H20RRelease }
    $summary.h20_fragment_completed = $true
} catch {
    $summary['h20_error'] = $_.Exception.Message
    $h20rIssues.Add($_.Exception.Message)
    $_ | Out-String | Set-Content -Encoding UTF8 (Join-Path $D 'h20-error.txt')
} finally {
    $h20rWatch = $null
    H20RRelease
    try {
        if ($null -ne $h20rProc -and -not $h20rProc.HasExited) { Stop-WiriGracefully $h20rProc 'h20-cleanup' }
        $summary['h20_cleanup_no_hosts'] = H20RGone
    } catch { $h20rIssues.Add("Daemon cleanup: $($_.Exception.Message)") }
    finally { if ($null -ne $h20rProc) { $h20rProc.Dispose() } }
    foreach ($h in $h20rForms) {
        try { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
        catch { $h20rIssues.Add("Fixture cleanup: $($_.Exception.Message)") }
    }
    try { $summary['h20_cleanup_forms_closed'] = H20RWait { @($h20rForms | Where-Object { [LT]::IsWindow($_) }).Count -eq 0 } }
    finally {
        if ($h20rOldDpi -ne [IntPtr]::Zero) { [void][H20RecentNative]::SetThreadDpiAwarenessContext($h20rOldDpi) }
    }
    $h20rEvidence['issues'] = @($h20rIssues.ToArray())
    $h20rEvidence | ConvertTo-Json -Depth 7 | Set-Content -Encoding UTF8 (Join-Path $D 'h20-evidence.json')
}
