# H88: floating windows versus fullscreen and maximized-to-edges (niri workspace.rs:1288-1476).
# A floating window cannot be fullscreen or maximized: the action moves it into the strip, and
# leaving the state floats it again at the geometry it floated at. The parent owns the global
# live-test mutex and isolated APPDATA. Only lane-owned forms are focused, floated, resized or
# closed; the retained daemon owns its pipe. Every gate group has its own try/catch.
& {
    $fsRoot = Join-Path $D 'lane-float-sizing'
    $fsDir = Join-Path $fsRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $fsDir
    $fsDetails = [ordered]@{ fixture_directory = $fsDir }
    $fsGates = @(
        'h88_float_fullscreen_enters_the_strip',
        'h88_float_fullscreen_keeps_foreground',
        'h88_float_fullscreen_exit_restores_floating_geometry',
        'h88_float_maximize_enters_the_strip_and_fills_the_work_area',
        'h88_float_maximize_exit_restores_floating_geometry',
        'h88_explicit_inactive_float_fullscreen_keeps_foreground',
        'h88_explicit_inactive_float_returns_to_floating',
        'h88_tile_fullscreen_exit_stays_a_tile',
        'h88_float_both_states_float_only_after_both_leave',
        'h88_toggle_window_floating_on_a_fullscreen_float_tile'
    )
    foreach ($fsGate in $fsGates) {
        $summary[$fsGate] = $false
        $fsDetails[$fsGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $fsProcess = $null
    $fsForms = New-Object System.Collections.ArrayList
    $fsSavedPipe = $env:WIRI_PIPE
    $fsPipe = '\\.\pipe\wiri_live_h88_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $fsSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    $fsOldDpi = [IntPtr]::Zero
    function FSGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $fsDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $fsDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fsRoot 'evidence.json')
    }
    function FSOwned([IntPtr]$h) {
        if (-not $fsForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "H88 refuses an unowned form: $h"
        }
    }
    function FSCtl([string]$tag, [string[]]$words) {
        if ($null -eq $fsProcess -or $fsProcess.HasExited -or $env:WIRI_PIPE -ne $fsPipe) {
            throw 'H88 retained daemon/pipe ownership is no longer valid'
        }
        $text = Ctl ('h88-' + $tag) $words
        if ($steps.Count -eq 0 -or [string]$steps[$steps.Count - 1].rc -ne '0') {
            throw "H88 CLI failed: $tag $text"
        }
        return $text
    }
    function FSAction([string]$tag, [string[]]$words) {
        $reply = (FSCtl $tag (@('--json', 'action') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) {
            throw "H88 action was rejected: $tag $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 1500
    }
    function FSFocus([IntPtr]$h) {
        FSOwned $h
        foreach ($attempt in 1..2) {
            $null = FSCtl ('focus-' + $h.ToInt64() + '-' + $attempt) @('--json', 'focus-window', [string][int64]$h)
            if (WaitForeground $h 900 4000) { Start-Sleep -Milliseconds 300; return }
        }
        throw "H88 owned form did not gain focus: $h"
    }
    function FSOpen([string]$name) {
        $h = [TestForms]::Open($name, 80, 130, 200)
        [void]$fsForms.Add($h)
        Start-Sleep -Milliseconds 1200
        return $h
    }
    function FSEntry([string]$tag, [IntPtr]$h) {
        FSOwned $h
        $reply = (FSCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "H88 window-list failed: $tag" }
        $entry = @($reply.result | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($entry.Count -ne 1) { throw "H88 expected one managed entry for $h, got $($entry.Count)" }
        return $entry[0]
    }
    function FSNear([double]$a, [double]$b, [double]$tol = 6) { return [Math]::Abs($a - $b) -le $tol }
    function FSSame($a, $b) {
        return (FSNear $a.L $b.L) -and (FSNear $a.T $b.T) -and (FSNear $a.W $b.W) -and (FSNear $a.H $b.H)
    }
    function FSFills($rect, $area) {
        return (FSNear $rect.L $area.L 8) -and (FSNear $rect.T $area.T 8) -and
            (FSNear $rect.R $area.R 8) -and (FSNear $rect.B $area.B 8)
    }
    function FSIsFloat($entry) { return ($entry.floating -eq $true) }
    function FSInStrip($entry) { return ($entry.floating -eq $false -and $null -ne $entry.column -and [int]$entry.column -ge 0) }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H88 refuses an existing wiri instance'
        }
        if (-not ('LaneFloatSizingNative' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneFloatSizingNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct INFO {
        public uint cbSize; public RECT monitor, work; public uint flags;
    }
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetMonitorInfo(IntPtr monitor, ref INFO info);
    public static INFO Info(IntPtr monitor) {
        INFO info = new INFO(); info.cbSize = (uint)Marshal.SizeOf(typeof(INFO));
        if (!GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed");
        return info;
    }
}
'@ -ErrorAction Stop
        }
        $fsOldDpi = [LaneFloatSizingNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if ($fsOldDpi -eq [IntPtr]::Zero) { throw 'H88 cannot establish physical-pixel observation context' }
        $env:WIRI_PIPE = $fsPipe
        $fsConfig = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
window-rule { match title="^lane-h88-(float|other)$"; open-floating true; }
'@
        $fsProcess = Start-WiriWithConfig $fsConfig (Join-Path $fsDir 'wiri.stderr.log') -NoDefaultBinds
        $null = $fsProcess.Handle
        if ($fsProcess.HasExited) { throw 'H88 daemon exited during startup' }
        $fsDetails['daemon_pid'] = $fsProcess.Id
        $fsDetails['pipe'] = $fsPipe
        $fsA = FSOpen 'lane-h88-float'
        $fsB = FSOpen 'lane-h88-other'
        $fsT = FSOpen 'lane-h88-tile'
        if (-not (FSIsFloat (FSEntry 'a-entry' $fsA)) -or -not (FSIsFloat (FSEntry 'b-entry' $fsB)) -or
            (FSIsFloat (FSEntry 't-entry' $fsT))) { throw 'H88 fixture layers do not match the configuration' }
        $fsInfo = [LaneFloatSizingNative]::Info([LaneFloatSizingNative]::MonitorFromWindow($fsA, 2))
        $fsMonitor = [pscustomobject]@{ L = $fsInfo.monitor.L; T = $fsInfo.monitor.T; R = $fsInfo.monitor.R; B = $fsInfo.monitor.B }
        $fsWork = [pscustomobject]@{ L = $fsInfo.work.L; T = $fsInfo.work.T; R = $fsInfo.work.R; B = $fsInfo.work.B }
        $fsDetails['geometry_context'] = @{ monitor = $fsMonitor; work = $fsWork }
        $fsHome = Rc $fsA
        $fsDetails['float_geometry_before'] = $fsHome

        # Group 1: fullscreen-window on the focused float moves it into the strip, covering the
        # monitor with the focus kept; the same action again floats it where it was.
        try {
            FSFocus $fsA
            $fsBeforeEntry = FSEntry 'g1-before' $fsA
            FSAction 'g1-fullscreen-on' @('fullscreen-window')
            $fsOnRect = Rc $fsA
            $fsOnEntry = FSEntry 'g1-on' $fsA
            $fsOnForeground = ([LT]::GetForegroundWindow() -eq $fsA)
            FSGate 'h88_float_fullscreen_enters_the_strip' (
                (FSIsFloat $fsBeforeEntry) -and (FSInStrip $fsOnEntry) -and $fsOnEntry.fullscreen -eq $true -and
                (FSFills $fsOnRect $fsMonitor)) @{ before = $fsBeforeEntry; after = $fsOnEntry; rect = $fsOnRect; monitor = $fsMonitor }
            FSGate 'h88_float_fullscreen_keeps_foreground' $fsOnForeground @{ foreground_is_target = $fsOnForeground }
            FSAction 'g1-fullscreen-off' @('fullscreen-window')
            $fsOffRect = Rc $fsA
            $fsOffEntry = FSEntry 'g1-off' $fsA
            FSGate 'h88_float_fullscreen_exit_restores_floating_geometry' (
                (FSIsFloat $fsOffEntry) -and $fsOffEntry.fullscreen -eq $false -and (FSSame $fsOffRect $fsHome)) @{
                    entry = $fsOffEntry; before = $fsHome; after = $fsOffRect }
        } catch { $fsDetails['group_fullscreen_error'] = ($_ | Out-String).Trim() }

        # Group 2: maximize-window-to-edges does the same with the work area.
        try {
            FSFocus $fsA
            FSAction 'g2-maximize-on' @('maximize-window-to-edges')
            $fsMaxRect = Rc $fsA
            $fsMaxEntry = FSEntry 'g2-on' $fsA
            FSGate 'h88_float_maximize_enters_the_strip_and_fills_the_work_area' (
                (FSInStrip $fsMaxEntry) -and $fsMaxEntry.maximized -eq $true -and $fsMaxEntry.fullscreen -eq $false -and
                (FSFills $fsMaxRect $fsWork)) @{ entry = $fsMaxEntry; rect = $fsMaxRect; work = $fsWork }
            FSAction 'g2-maximize-off' @('maximize-window-to-edges')
            $fsMaxOffRect = Rc $fsA
            $fsMaxOffEntry = FSEntry 'g2-off' $fsA
            FSGate 'h88_float_maximize_exit_restores_floating_geometry' (
                (FSIsFloat $fsMaxOffEntry) -and $fsMaxOffEntry.maximized -eq $false -and (FSSame $fsMaxOffRect $fsHome)) @{
                    entry = $fsMaxOffEntry; before = $fsHome; after = $fsMaxOffRect }
        } catch { $fsDetails['group_maximize_error'] = ($_ | Out-String).Trim() }

        # Group 3: an explicit id that is not the foreground window does not take the focus.
        try {
            FSFocus $fsB
            $fsOtherBefore = Rc $fsB
            FSAction 'g3-by-id-on' @('fullscreen-window', '--id', [string][int64]$fsA)
            $fsIdOnEntry = FSEntry 'g3-on' $fsA
            $fsIdOnRect = Rc $fsA
            $fsIdOnForeground = ([LT]::GetForegroundWindow() -eq $fsB)
            # The column is fullscreen-sized, but the view does not scroll to it: niri activates (and
            # moves the camera to) the window only when it already was the active one
            # (workspace.rs:1420, 1433-1443). So check the monitor's size, not its position.
            FSGate 'h88_explicit_inactive_float_fullscreen_keeps_foreground' (
                (FSInStrip $fsIdOnEntry) -and $fsIdOnEntry.fullscreen -eq $true -and $fsIdOnForeground -and
                (FSNear ($fsIdOnRect.R - $fsIdOnRect.L) ($fsMonitor.R - $fsMonitor.L) 8) -and
                (FSNear ($fsIdOnRect.B - $fsIdOnRect.T) ($fsMonitor.B - $fsMonitor.T) 8)) @{
                    entry = $fsIdOnEntry; rect = $fsIdOnRect; other_foreground = $fsIdOnForeground }
            FSAction 'g3-by-id-off' @('fullscreen-window', '--id', [string][int64]$fsA)
            $fsIdOffEntry = FSEntry 'g3-off' $fsA
            $fsIdOffRect = Rc $fsA
            $fsOtherAfter = Rc $fsB
            FSGate 'h88_explicit_inactive_float_returns_to_floating' (
                (FSIsFloat $fsIdOffEntry) -and $fsIdOffEntry.fullscreen -eq $false -and (FSSame $fsIdOffRect $fsHome) -and
                (FSSame $fsOtherBefore $fsOtherAfter) -and [LT]::GetForegroundWindow() -eq $fsB) @{
                    entry = $fsIdOffEntry; before = $fsHome; after = $fsIdOffRect
                    other_before = $fsOtherBefore; other_after = $fsOtherAfter }
        } catch { $fsDetails['group_explicit_error'] = ($_ | Out-String).Trim() }

        # Group 4: a tile that never floated stays a tile when it leaves fullscreen.
        try {
            FSFocus $fsT
            $fsTileBefore = Rc $fsT
            $fsTileBeforeEntry = FSEntry 'g4-before' $fsT
            FSAction 'g4-fullscreen-on' @('fullscreen-window')
            $fsTileOnEntry = FSEntry 'g4-on' $fsT
            FSAction 'g4-fullscreen-off' @('fullscreen-window')
            $fsTileOffEntry = FSEntry 'g4-off' $fsT
            $fsTileAfter = Rc $fsT
            FSGate 'h88_tile_fullscreen_exit_stays_a_tile' (
                (FSInStrip $fsTileBeforeEntry) -and $fsTileOnEntry.fullscreen -eq $true -and (FSInStrip $fsTileOffEntry) -and
                $fsTileOffEntry.fullscreen -eq $false -and $fsTileOffEntry.column -eq $fsTileBeforeEntry.column -and
                (FSSame $fsTileBefore $fsTileAfter)) @{
                    before = $fsTileBeforeEntry; on = $fsTileOnEntry; after = $fsTileOffEntry
                    rect_before = $fsTileBefore; rect_after = $fsTileAfter }
        } catch { $fsDetails['group_tile_error'] = ($_ | Out-String).Trim() }

        # Group 5: maximize and fullscreen together: only leaving both floats it again.
        try {
            FSFocus $fsA
            FSAction 'g5-maximize' @('maximize-window-to-edges')
            FSAction 'g5-fullscreen' @('fullscreen-window')
            $fsBothEntry = FSEntry 'g5-both' $fsA
            FSAction 'g5-unmaximize-while-fullscreen' @('maximize-window-to-edges')
            $fsStillFullscreenEntry = FSEntry 'g5-still-fullscreen' $fsA
            FSAction 'g5-unfullscreen' @('fullscreen-window')
            $fsBothOffEntry = FSEntry 'g5-off' $fsA
            $fsBothOffRect = Rc $fsA
            FSGate 'h88_float_both_states_float_only_after_both_leave' (
                (FSInStrip $fsBothEntry) -and $fsBothEntry.fullscreen -eq $true -and $fsBothEntry.maximized -eq $true -and
                (FSInStrip $fsStillFullscreenEntry) -and $fsStillFullscreenEntry.fullscreen -eq $true -and
                $fsStillFullscreenEntry.maximized -eq $false -and (FSIsFloat $fsBothOffEntry) -and
                (FSSame $fsBothOffRect $fsHome)) @{
                    both = $fsBothEntry; after_unmaximize = $fsStillFullscreenEntry; after_unfullscreen = $fsBothOffEntry
                    before = $fsHome; after = $fsBothOffRect }
        } catch { $fsDetails['group_both_error'] = ($_ | Out-String).Trim() }

        # Group 6: toggle-window-floating on the fullscreen tile that came from the floating layer.
        try {
            FSFocus $fsA
            FSAction 'g6-fullscreen' @('fullscreen-window')
            $fsToggleOnEntry = FSEntry 'g6-on' $fsA
            FSAction 'g6-toggle-floating' @('toggle-window-floating')
            $fsToggleEntry = FSEntry 'g6-toggled' $fsA
            $fsToggleRect = Rc $fsA
            FSGate 'h88_toggle_window_floating_on_a_fullscreen_float_tile' (
                (FSInStrip $fsToggleOnEntry) -and $fsToggleOnEntry.fullscreen -eq $true -and (FSIsFloat $fsToggleEntry) -and
                $fsToggleEntry.fullscreen -eq $false -and $fsToggleEntry.maximized -eq $false -and
                (FSSame $fsToggleRect $fsHome)) @{
                    on = $fsToggleOnEntry; toggled = $fsToggleEntry; before = $fsHome; after = $fsToggleRect }
        } catch { $fsDetails['group_toggle_error'] = ($_ | Out-String).Trim() }
        $fsDetails['stage'] = 'gates-complete'
    } catch {
        $fsDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        foreach ($fsForm in $fsForms) {
            try {
                if ([LT]::IsWindow($fsForm) -and [LT]::Pid($fsForm) -eq $PID) { [TestForms]::CloseForm($fsForm) }
            } catch { $fsDetails['form_cleanup_error'] = [string]$_ }
        }
        try {
            if ($null -ne $fsProcess -and -not $fsProcess.HasExited) {
                if ($env:WIRI_PIPE -eq $fsPipe) { Stop-WiriGracefully $fsProcess 'h88-owned' }
                elseif (-not $fsProcess.HasExited) { $fsProcess.Kill(); $null = $fsProcess.WaitForExit(5000) }
            }
        } catch {
            $fsDetails['daemon_cleanup_error'] = [string]$_
            if ($null -ne $fsProcess -and -not $fsProcess.HasExited) {
                try { $fsProcess.Kill(); $null = $fsProcess.WaitForExit(5000) } catch { $fsDetails['daemon_kill_error'] = [string]$_ }
            }
        } finally {
            $env:WIRI_PIPE = $fsSavedPipe
            if ($null -ne $fsSavedConfig) { [IO.File]::WriteAllText($cfg, $fsSavedConfig, (New-Object Text.UTF8Encoding($false))) }
            if ($fsOldDpi -ne [IntPtr]::Zero) { [void][LaneFloatSizingNative]::SetThreadDpiAwarenessContext($fsOldDpi) }
            $fsDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fsRoot 'evidence.json')
            $summary['lane_float_sizing_details'] = $fsDetails
        }
    }
}
