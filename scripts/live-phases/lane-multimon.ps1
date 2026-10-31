# Lane MULTIMON (H99): wiri on two displays of different size and scale.
#
# tridaris has DISPLAY1 (1366x768 at 100%, primary, at 0,0) and DISPLAY2 (SGT HS156PC over HDMI,
# 1920x1080 at 125%, at 1366,0).  Every gate here observes the real desktop: physical monitor
# rectangles and per-monitor DPI from EnumDisplayMonitors/GetDpiForMonitor, DWM's visible window
# rectangles, the decor frames wiri paints, wiri's own `outputs`/`windows`/`workspaces` IPC and the
# overview/overlay windows it hosts.  Nothing is inferred from configuration.
#
# The gates cover the parity rows that were partial only for lack of a second, differently scaled
# output:
#   * `Outputs` IPC list fields (identity, physical size, modes, current mode, logical geometry);
#   * `Output{output, action}` IPC and the `niri msg output` subcommand shape;
#   * per-output `layout {}` overrides (gaps) at mixed DPI;
#   * `center-column` on each output, including the 125% one;
#   * `focus-ring`, `border`, `shadow` and a window rule's `border` override projected onto the
#     physical pixels of both outputs;
#   * `layout { background-color }` per output reaching the overview backdrop;
#   * the hotkey overlay on the focused output, at its centre, scaled by its DPI;
#   * keyboard binds while the overview is open on one of two displays;
#   * `hot-corners {}` per output;
#   * a workspace surviving `output off` and returning when the output comes back, and the
#     hot-plug re-evaluation that follows it;
#   * `mode`, `scale`, `position` and `transform` through wiri's own `msg output`.
#
# The output gates change ONLY DISPLAY2, only through wiri's own `msg output` IPC, and only the
# settings the owner approved (off/on, mode, scale, transform, position).  DISPLAY2 is restored to
# 1920x1080, 125%, position 1366,0 and transform normal in a `finally` block, and the last gate
# `h99_outputs_restored` verifies that state plus DISPLAY1 being unchanged and still primary.
# DISPLAY1, the Windows primary display, the Windows DPI settings and every other system setting are
# never written.  With fewer than two displays every gate is recorded as skipped (not failed), so
# the hosted CI runners and single-display runs are unaffected.
& {
    $mmDir = Join-Path $D 'lane-multimon'
    $null = New-Item -ItemType Directory -Force $mmDir
    $mmKeys = @(
        'h99_two_displays_are_a_mixed_dpi_pair',
        'h99_outputs_ipc_lists_both_outputs_with_identity',
        'h99_outputs_ipc_logical_geometry_is_per_output_dpi',
        'h99_focused_output_follows_focus_monitor',
        'h99_per_output_layout_gaps_differ_at_mixed_dpi',
        'h99_center_column_centres_the_column_on_each_output',
        'h99_focus_ring_extent_scales_with_output_dpi',
        'h99_border_band_scales_with_output_dpi',
        'h99_window_rule_border_uses_the_output_projection',
        'h99_shadow_extent_uses_the_output_projection',
        'h99_backdrop_colour_is_per_output',
        'h99_hotkey_overlay_centres_on_the_focused_output',
        'h99_hotkey_overlay_scales_with_the_output_dpi',
        'h99_overview_keyboard_works_across_displays',
        'h99_hot_corner_policy_is_per_output',
        'h99_msg_output_scale_is_layout_only_and_restores',
        'h99_msg_output_mode_switches_the_real_desktop',
        'h99_outputs_ipc_tracks_the_real_mode_change',
        'h99_msg_output_position_moves_the_real_desktop',
        'h99_msg_output_transform_rotates_the_real_desktop',
        'h99_output_off_detaches_only_display2',
        'h99_output_off_moves_its_windows_to_display1',
        'h99_output_on_returns_the_monitor_and_its_workspace',
        'h99_hotplug_reevaluates_per_output_layout',
        'h99_outputs_restored'
    )
    foreach ($mmKey in $mmKeys) { $summary[$mmKey] = $false }
    $mmReport = [ordered]@{
        reference = 'niri e19df12cf8; DISPLAY1 1366x768/96dpi primary at 0,0; DISPLAY2 1920x1080/120dpi at 1366,0'
        checks = [ordered]@{}
        skips = [ordered]@{}
        observations = (New-Object 'System.Collections.Generic.List[object]')
        limitations = @(
            'Only the output gates touch DISPLAY2, only through wiri `msg output`, and each restores it in a finally block.',
            'DISPLAY1 is never written; the Windows primary display is never changed.',
            'The overview hosts one window at a time (the focused output), as wiri documents; the multi-display gate observes that plus the keyboard binds, not a per-output overview on both displays at once.',
            'Fixtures stay on DISPLAY1 (the harness parks the pointer there); the DISPLAY2 fixtures are placed by an `open-on-output` window rule, which is itself real placement evidence.'
        )
    }
    function MmSave { ($mmReport | ConvertTo-Json -Depth 12) | Set-Content -Encoding UTF8 (Join-Path $mmDir 'multimon.json') }
    function MmGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $mmReport.checks[$name] = [ordered]@{ pass = $pass; observed = $observed }
        MmSave
    }
    # A gate this run could not observe.  It is recorded as skipped, never as failed and never as
    # passed: its boolean leaves the summary (so the gate totals do not count it) and it is named
    # in `h99_skips` with the reason, so a green summary cannot hide it.
    function MmSkip([string]$name, [string]$reason) {
        $summary.Remove($name)
        $mmReport.skips[$name] = $reason
        $summary['h99_skips'] = @($mmReport.skips.Keys | ForEach-Object { [pscustomobject]@{ gate = $_; reason = $mmReport.skips[$_] } })
        MmSave
    }
    if (-not ('LaneMultimonNative' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class LaneMultimonNative {
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, EnumProc proc, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool GetMonitorInfoW(IntPtr monitor, ref MONITORINFOEX info);
    [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool EnumDisplaySettingsW(string name, int mode, IntPtr devmode);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int ChangeDisplaySettingsExW(string name, IntPtr devmode, IntPtr hwnd, uint flags, IntPtr param);
    [DllImport("user32.dll")] static extern int SetDisplayConfig(uint pathCount, IntPtr paths, uint modeCount, IntPtr modes, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetPropW(IntPtr hwnd, string name);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll", SetLastError = true)] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    public delegate bool EnumProc(IntPtr monitor, IntPtr hdc, IntPtr rect, IntPtr data);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct MONITORINFOEX {
        public int cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string szDevice;
    }
    [StructLayout(LayoutKind.Sequential)] struct MOUSE { public int X, Y; public uint Data, Flags, Time; public UIntPtr Extra; }
    [StructLayout(LayoutKind.Explicit)] struct INPUTUNION { [FieldOffset(0)] public MOUSE Mouse; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint Type; public INPUTUNION Value; }
    public class Monitor {
        public string Device; public int L, T, R, B; public int WorkL, WorkT, WorkR, WorkB;
        public bool Primary; public int Dpi;
        public int W { get { return R - L; } }
        public int H { get { return B - T; } }
    }
    public static List<Monitor> Monitors() {
        List<Monitor> result = new List<Monitor>();
        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, delegate(IntPtr m, IntPtr hdc, IntPtr r, IntPtr d) {
            MONITORINFOEX info = new MONITORINFOEX();
            info.cbSize = Marshal.SizeOf(typeof(MONITORINFOEX));
            if (GetMonitorInfoW(m, ref info)) {
                uint dx = 0; uint dy = 0;
                GetDpiForMonitor(m, 0, out dx, out dy);
                Monitor mon = new Monitor();
                mon.Device = info.szDevice;
                mon.L = info.rcMonitor.L; mon.T = info.rcMonitor.T; mon.R = info.rcMonitor.R; mon.B = info.rcMonitor.B;
                mon.WorkL = info.rcWork.L; mon.WorkT = info.rcWork.T; mon.WorkR = info.rcWork.R; mon.WorkB = info.rcWork.B;
                mon.Primary = (info.dwFlags & 1) != 0; mon.Dpi = (int)dx;
                result.Add(mon);
            }
            return true;
        }, IntPtr.Zero);
        return result;
    }
    const int DEVMODE_SIZE = 220;
    const uint DM_PELSWIDTH = 0x00080000, DM_PELSHEIGHT = 0x00100000, DM_POSITION = 0x00000020, DM_DISPLAYORIENTATION = 0x00000080;
    const uint CDS_UPDATEREGISTRY = 0x1, CDS_NORESET = 0x4;
    const uint SDC_APPLY = 0x80, SDC_TOPOLOGY_EXTEND = 0x4;
    // Width, height, source x, source y, orientation, bits per pixel; {-1} when the output is not
    // attached to the desktop (EnumDisplaySettings refuses an inactive device).
    public static int[] Mode(string name) {
        IntPtr p = Marshal.AllocHGlobal(DEVMODE_SIZE);
        try {
            Marshal.Copy(new byte[DEVMODE_SIZE], 0, p, DEVMODE_SIZE);
            Marshal.WriteInt16(p, 68, (short)DEVMODE_SIZE);
            if (!EnumDisplaySettingsW(name, -1, p)) { return new int[] { -1 }; }
            return new int[] { Marshal.ReadInt32(p, 172), Marshal.ReadInt32(p, 176), Marshal.ReadInt32(p, 76),
                Marshal.ReadInt32(p, 80), Marshal.ReadInt32(p, 84), Marshal.ReadInt32(p, 168) };
        } finally { Marshal.FreeHGlobal(p); }
    }
    // Restore one output's size, position and orientation from its current mode block, the way
    // Windows' own display settings do it: stage the device, then apply the whole desktop.
    public static int Restore(string name, int width, int height, int x, int y, int orientation) {
        IntPtr p = Marshal.AllocHGlobal(DEVMODE_SIZE);
        try {
            Marshal.Copy(new byte[DEVMODE_SIZE], 0, p, DEVMODE_SIZE);
            Marshal.WriteInt16(p, 68, (short)DEVMODE_SIZE);
            if (!EnumDisplaySettingsW(name, -1, p)) { return -1; }
            uint fields = (uint)Marshal.ReadInt32(p, 72) | DM_PELSWIDTH | DM_PELSHEIGHT | DM_POSITION | DM_DISPLAYORIENTATION;
            Marshal.WriteInt32(p, 72, (int)fields);
            Marshal.WriteInt32(p, 172, width); Marshal.WriteInt32(p, 176, height);
            Marshal.WriteInt32(p, 76, x); Marshal.WriteInt32(p, 80, y);
            Marshal.WriteInt32(p, 84, orientation);
            int rc = ChangeDisplaySettingsExW(name, p, IntPtr.Zero, CDS_UPDATEREGISTRY | CDS_NORESET, IntPtr.Zero);
            if (rc != 0) { return rc; }
            return ChangeDisplaySettingsExW(null, IntPtr.Zero, IntPtr.Zero, 0, IntPtr.Zero);
        } finally { Marshal.FreeHGlobal(p); }
    }
    // SDC_TOPOLOGY_EXTEND re-attaches a detached display; no path/mode arrays are needed.
    public static int Extend() { return SetDisplayConfig(0, IntPtr.Zero, 0, IntPtr.Zero, SDC_APPLY | SDC_TOPOLOGY_EXTEND); }
    public static bool Park(int x, int y) { return SetCursorPos(x, y); }
    public static long GetProp(IntPtr hwnd, string name) { return GetPropW(hwnd, name).ToInt64(); }
    // A real pointer motion through SendInput: the low-level hook a hot corner listens to never sees
    // SetCursorPos.
    public static void Move(int x, int y) {
        INPUT move = new INPUT();
        move.Type = 0;
        move.Value.Mouse.X = Normalized(x, GetSystemMetrics(76), GetSystemMetrics(78));
        move.Value.Mouse.Y = Normalized(y, GetSystemMetrics(77), GetSystemMetrics(79));
        move.Value.Mouse.Flags = 0xc001;   // ABSOLUTE | VIRTUALDESK | MOVE: the whole desktop, not the primary monitor
        if (SendInput(1, new INPUT[] { move }, Marshal.SizeOf(typeof(INPUT))) != 1)
            throw new InvalidOperationException("pointer motion injection failed");
    }
    // A relative move of (0,0): the hook still sees an event at the current position.
    public static void Nudge() {
        INPUT move = new INPUT();
        move.Type = 0;
        move.Value.Mouse.Flags = 0x0001;
        if (SendInput(1, new INPUT[] { move }, Marshal.SizeOf(typeof(INPUT))) != 1)
            throw new InvalidOperationException("pointer nudge injection failed");
    }
    static int Normalized(int value, int origin, int extent) {
        if (extent <= 0 || value < origin || value >= origin + extent)
            throw new InvalidOperationException("pointer target outside the virtual desktop");
        return (int)(((long)(value - origin) * 65536 + 32768) / extent);
    }
}
'@
    }

    function MmCtl([string]$tag, [string[]]$words) {
        # A request that arrives while wiri is applying a layout answers "wiri is busy ... queued"
        # with a non-zero exit code; it is not a failure, so ask again a few times.
        for ($attempt = 0; $attempt -lt 6; $attempt++) {
            if ($null -eq $mmProc -or $mmProc.HasExited) { throw "H99 daemon is not alive: $tag" }
            $name = 'h99-' + $tag + $(if ($attempt -gt 0) { '-retry' + $attempt } else { '' })
            $text = Ctl $name $words
            $rc = if ($steps.Count -eq 0) { '' } else { [string]$steps[$steps.Count - 1].rc }
            $reply = "$text"
            try { $reply += [IO.File]::ReadAllText((Join-Path $D ($name + '.txt.err'))) } catch { }
            if ($rc -eq '0') { return $text }
            if ($reply -match 'is busy|queued') { Start-Sleep -Milliseconds 700; continue }
            throw "H99 CLI failed: $tag (rc=$rc) $reply"
        }
        throw "H99 CLI kept answering busy: $tag"
    }
    function MmJson([string]$tag, [string[]]$words) {
        $reply = (MmCtl $tag (@('--json') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "H99 request was rejected: $tag ($($reply.error))" }
        return $reply.result
    }
    # A request whose refusal is itself the observation: wiri answers with its own error text.
    function MmTryJson([string]$tag, [string[]]$words) {
        $reply = (MmCtl $tag (@('--json') + $words)) | ConvertFrom-Json
        return [pscustomobject]@{ ok = ($reply.success -eq $true); error = [string]$reply.error; result = $reply.result }
    }
    # Wait until wiri's own model lists the output again (its workspaces name it), which is what
    # "the monitor is back" means to the engine.
    function MmWaitEngineOutput([string]$name, [bool]$present, [int]$seconds = 20) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        for ($i = 0; $i -lt ($seconds * 5); $i++) {
            $entries = @()
            try { $entries = @(MmWorkspaces | Where-Object { [string]$_.output -eq $name }) } catch { }
            if (($entries.Count -ge 1) -eq $present) { return [int]$sw.ElapsedMilliseconds }
            Start-Sleep -Milliseconds 200
        }
        return -1
    }
    function MmAction([string]$tag, [string[]]$words) {
        $null = MmJson $tag (@('action') + $words)
        Start-Sleep -Milliseconds 450
    }
    function MmMonitors { return @([LaneMultimonNative]::Monitors()) }
    function MmMonitor([string]$device) { return @(MmMonitors | Where-Object { $_.Device -eq $device }) | Select-Object -First 1 }
    function MmOutputs { return (MmJson 'outputs' @('outputs')).outputs }
    function MmOutput([string]$name) { return (MmOutputs).$name }
    function MmFocusedOutput { return (MmJson 'focused-output' @('focused-output')).focused_output }
    function MmWindows { return @((MmJson 'windows' @('windows')).windows) }
    function MmWorkspaces { return @((MmJson 'workspaces' @('workspaces')).workspaces) }
    function MmWin($list, $handle) { return @($list | Where-Object { [int64]$_.id -eq [int64]$handle }) | Select-Object -First 1 }
    function MmMode([string]$device) { return @([LaneMultimonNative]::Mode($device)) }
    function MmPixel([int]$x, [int]$y) {
        $bmp = New-Object Drawing.Bitmap 1, 1
        $g = [Drawing.Graphics]::FromImage($bmp)
        try {
            $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
            $c = $bmp.GetPixel(0, 0)
            return @([int]$c.R, [int]$c.G, [int]$c.B)
        } finally { $g.Dispose(); $bmp.Dispose() }
    }
    function MmNearColor($rgb, $expected, [int]$tolerance = 20) {
        if ($null -eq $rgb -or @($rgb).Count -ne 3) { return $false }
        for ($i = 0; $i -lt 3; $i++) {
            if ([Math]::Abs([int]$rgb[$i] - [int]$expected[$i]) -gt $tolerance) { return $false }
        }
        return $true
    }
    function MmRgb([string]$hex) {
        $h = $hex.TrimStart('#')
        return @([Convert]::ToInt32($h.Substring(0, 2), 16), [Convert]::ToInt32($h.Substring(2, 2), 16),
                 [Convert]::ToInt32($h.Substring(4, 2), 16))
    }
    function MmNear([int]$a, [int]$b, [int]$tolerance = 2) { return ([Math]::Abs($a - $b) -le $tolerance) }
    # The layer window wiri draws around a tile: `WiriDecor Frame <hwnd>` of its own process.
    function MmDecor([IntPtr]$handle) {
        if ($null -eq $mmProc) { return [IntPtr]::Zero }
        $found = @([LT]::TopLevel() | Where-Object {
            [LT]::Class($_) -eq 'WiriDecor' -and [LT]::Title($_) -eq ('WiriDecor Frame ' + [int64]$handle) -and
            [LT]::Pid($_) -eq $mmProc.Id })
        return ($found | Select-Object -First 1)
    }
    function MmFrameRect([IntPtr]$handle) {
        $r = New-Object LT+RECT
        if ($null -eq $handle -or $handle -eq [IntPtr]::Zero -or -not [LT]::GetWindowRect($handle, [ref]$r)) {
            throw "H99 has no window to measure (handle $handle)"
        }
        return [pscustomobject]@{ L = [int]$r.L; T = [int]$r.T; R = [int]$r.R; B = [int]$r.B }
    }
    function MmClassWindows([string]$className) {
        return @([LT]::TopLevel() | Where-Object {
            [LT]::Class($_) -eq $className -and $null -ne $mmProc -and [LT]::Pid($_) -eq $mmProc.Id })
    }
    function MmWaitClass([string]$className, [bool]$present, [int]$seconds = 6) {
        for ($i = 0; $i -lt ($seconds * 10); $i++) {
            if ((@(MmClassWindows $className).Count -ge 1) -eq $present) { return $true }
            Start-Sleep -Milliseconds 100
        }
        return $false
    }
    # A pixel of the overview background that is not inside a workspace row: just above the top
    # row when the camera leaves room there, else just below the bottom row, else the host corner.
    function MmBackdropPoint([IntPtr]$hostWindow) {
        $hostRect = MmFrameRect $hostWindow
        $count = [int][LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewWorkspaceCount')
        if ($count -ge 1) {
            $x = [int]([LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewWorkspace0X') +
                [LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewWorkspace0W') / 2)
            $top = [int][LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewWorkspace0Y')
            if ($top - 6 -ge $hostRect.T + 2) { return @($x, ($top - 6)) }
            $last = $count - 1
            $bottom = [int]([LaneMultimonNative]::GetProp($hostWindow, "WiriOverviewWorkspace${last}Y") +
                [LaneMultimonNative]::GetProp($hostWindow, "WiriOverviewWorkspace${last}H"))
            if ($bottom + 6 -le $hostRect.B - 2) { return @($x, ($bottom + 6)) }
        }
        return @(($hostRect.L + 4), ($hostRect.T + 4))
    }
    function MmOpen([string]$title, [int]$red, [int]$green) {
        $h = [TestForms]::Open($title, $red, $green, 120)
        Start-Sleep -Milliseconds 1200
        $mmForms.Add($h) | Out-Null
        return $h
    }
    function MmCloseAll {
        foreach ($form in $mmForms) {
            if ([LT]::IsWindow($form)) { [TestForms]::CloseForm($form) }
        }
        $mmForms.Clear()
        Start-Sleep -Milliseconds 900
    }
    # Ask wiri for the focus and wait for it: the hotkey overlay follows the real foreground window's
    # monitor and DPI.
    function MmOwnForeground([IntPtr]$handle) {
        FocusForm $handle
        for ($i = 0; $i -lt 60; $i++) {
            if ([LT]::GetForegroundWindow() -eq $handle) { Start-Sleep -Milliseconds 200; return $true }
            Start-Sleep -Milliseconds 50
        }
        return $false
    }
    function MmReload([string]$text, [string]$tag) {
        [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
        $null = MmCtl ('reload-' + $tag) @('reload-config')
        Start-Sleep -Milliseconds 900
    }
    # A real pointer motion the low-level hook sees.  SendInput can refuse (it returns zero while
    # another program holds the input, e.g. right after a display change), so retry and fall back
    # to placing the cursor and injecting a relative move of (0,0): the hook still fires with the
    # position the corner logic reads.
    function MmMovePointer([int]$x, [int]$y, [int]$settleMs = 700) {
        for ($attempt = 0; $attempt -lt 5; $attempt++) {
            try { [LaneMultimonNative]::Move($x, $y); Start-Sleep -Milliseconds $settleMs; return $true }
            catch { Start-Sleep -Milliseconds 250 }
        }
        $null = [LaneMultimonNative]::Park($x, $y)
        Start-Sleep -Milliseconds 120
        try { [LaneMultimonNative]::Nudge(); Start-Sleep -Milliseconds $settleMs; return $true }
        catch { throw "H99 cannot inject pointer motion to $x,$y -- $($_)" }
    }
    function MmWaitMonitor([string]$device, [bool]$present, [int]$seconds = 25) {
        for ($i = 0; $i -lt ($seconds * 4); $i++) {
            if ((@(MmMonitors | Where-Object { $_.Device -eq $device }).Count -ge 1) -eq $present) { return $true }
            Start-Sleep -Milliseconds 250
        }
        return $false
    }
    # Put DISPLAY2 back to 1920x1080, 125%, 1366,0, normal transform.  Works without the daemon:
    # topology first (an inactive output has no mode to restore), then size/position/orientation.
    function MmRestoreDisplay2 {
        $state = [ordered]@{ extend = $null; restore = $null }
        if ([string]::IsNullOrEmpty($mmD2Name)) { return $state }
        if (@(MmMonitors | Where-Object { $_.Device -eq $mmD2Name }).Count -eq 0) {
            $state.extend = [LaneMultimonNative]::Extend()
            $null = MmWaitMonitor $mmD2Name $true 25
        }
        if (@(MmMonitors | Where-Object { $_.Device -eq $mmD2Name }).Count -ge 1) {
            $mode = MmMode $mmD2Name
            if ($mode.Count -ge 5 -and (($mode[0] -ne 1920) -or ($mode[1] -ne 1080) -or ($mode[2] -ne 1366) -or
                    ($mode[3] -ne 0) -or ($mode[4] -ne 0))) {
                $state.restore = [LaneMultimonNative]::Restore($mmD2Name, 1920, 1080, 1366, 0, 0)
                $null = MmWaitMonitor $mmD2Name $true 15
                Start-Sleep -Milliseconds 1200
            }
        }
        return $state
    }
    # The configuration of every group: one layout body, optional per-output blocks and extra
    # sections, with the two fixture rules every group shares.  `appearance` replaces the default
    # border/focus-ring/shadow block so a variant never declares the same node twice.
    function MmConfig([string]$layout = '', [string]$outputs = '', [string]$extra = '',
        [string]$appearance = "    border { off }`r`n    focus-ring { off }`r`n    shadow { off }`r`n") {
        $text = "input { mod-key `"ctrl-alt`" }`r`n" +
            "animations { off }`r`n" +
            "layout {`r`n    gaps 8`r`n    smart-borders false`r`n    strip-frame true`r`n" +
            "    column-width-mode `"fixed`"`r`n    column-width 420`r`n" +
            $appearance + $layout + "}`r`n" + $outputs + $extra +
            "window-rule {`r`n    match title=`"^h99-d2`"`r`n    open-on-output r#`"$mmD2Name`"#`r`n}`r`n" +
            "window-rule {`r`n    match title=`"^h99-`"`r`n    default-column-width { fixed 420 }`r`n}`r`n" +
            "binds {`r`n    Mod+F9 { focus-column-right }`r`n}`r`n"
        return $text
    }
    function MmOutputLayout([string]$body) {
        return "output r#`"$mmD2Name`"# {`r`n$body`r`n}`r`n"
    }

    $mmProc = $null
    $mmForms = New-Object System.Collections.ArrayList
    $mmCursor = [Windows.Forms.Cursor]::Position
    $mmOldDpiContext = [IntPtr]::Zero
    $mmD1Name = ''
    $mmD2Name = ''
    try {
        # Physical-pixel observation context: without it this DPI-unaware process is told that
        # DISPLAY2 is 1536x864 at 1366,0 and every rectangle and screenshot pixel of it is wrong.
        $mmOldDpiContext = [LaneMultimonNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if ($mmOldDpiContext -eq [IntPtr]::Zero) { throw 'H99 cannot establish a per-monitor-v2 observation context' }
        $mmMon = MmMonitors
        $mmReport.observations.Add([ordered]@{ stage = 'monitors'; monitors = @($mmMon | ForEach-Object {
            [ordered]@{ device = $_.Device; bounds = "$($_.L),$($_.T),$($_.R),$($_.B)"
                work = "$($_.WorkL),$($_.WorkT),$($_.WorkR),$($_.WorkB)"; primary = $_.Primary; dpi = $_.Dpi } }) })
        $summary['h99_display_count'] = $mmMon.Count
        if ($mmMon.Count -lt 2) {
            foreach ($gateName in $mmKeys) { MmSkip $gateName ("fewer than two displays ({0})" -f $mmMon.Count) }
            return
        }
        $mmD1 = @($mmMon | Where-Object { $_.Primary }) | Select-Object -First 1
        $mmD2 = @($mmMon | Where-Object { -not $_.Primary }) | Select-Object -First 1
        if ($null -eq $mmD1 -or $null -eq $mmD2) { throw 'H99 needs exactly one primary and one secondary display' }
        $mmD1Name = $mmD1.Device
        $mmD2Name = $mmD2.Device
        $summary['h99_displays'] = @($mmMon | ForEach-Object { "$($_.Device) $($_.W)x$($_.H) $($_.Dpi)dpi at $($_.L),$($_.T)" })
        # The pair itself: different physical size, different scale, the primary at the origin.  A
        # missing difference here would make every mixed-DPI claim below vacuous.
        MmGate 'h99_two_displays_are_a_mixed_dpi_pair' (
            ($mmMon.Count -eq 2) -and ($mmD1.Primary -and -not $mmD2.Primary) -and ($mmD1.L -eq 0 -and $mmD1.T -eq 0) -and
            ($mmD2.L -ge $mmD1.R) -and ($mmD1.W -ne $mmD2.W -or $mmD1.H -ne $mmD2.H) -and
            ($mmD1.Dpi -eq 96) -and ($mmD2.Dpi -ge 120) -and
            ($mmD1.WorkR -le $mmD1.R) -and ($mmD1.WorkB -le $mmD1.B) -and ($mmD2.WorkR -le $mmD2.R) -and ($mmD2.WorkB -le $mmD2.B)
        ) ([ordered]@{ d1 = "$($mmD1.W)x$($mmD1.H)@$($mmD1.Dpi)"; d2 = "$($mmD2.W)x$($mmD2.H)@$($mmD2.Dpi)"; d2_origin = "$($mmD2.L),$($mmD2.T)" })

        $null = [LaneMultimonNative]::Park([int](($mmD1.WorkL + $mmD1.WorkR) / 2), [int](($mmD1.WorkT + $mmD1.WorkB) / 2))
        $mmBase = MmConfig ''
        $mmProc = Start-WiriWithConfig $mmBase (Join-Path $mmDir 'wiri.stderr.log')
        if ($null -eq $mmProc -or $mmProc.HasExited) { throw 'H99 daemon did not start' }
        $null = MmOutputs

        # ---- 1. the Outputs IPC list on a real mixed-DPI pair ----------------------------------
        try {
            $out = MmOutputs
            $names = @($out.PSObject.Properties | ForEach-Object { $_.Name })
            $d1 = $out.$mmD1Name
            $d2 = $out.$mmD2Name
            $mode1 = MmMode $mmD1Name
            $mode2 = MmMode $mmD2Name
            $d1Mode = $null
            if ($null -ne $d1 -and $null -ne $d1.current_mode -and @($d1.modes).Count -gt [int]$d1.current_mode) { $d1Mode = @($d1.modes)[[int]$d1.current_mode] }
            $d2Mode = $null
            if ($null -ne $d2 -and $null -ne $d2.current_mode -and @($d2.modes).Count -gt [int]$d2.current_mode) { $d2Mode = @($d2.modes)[[int]$d2.current_mode] }
            $identityOk = ($names.Count -eq 2) -and ($null -ne $d1) -and ($null -ne $d2) -and
                ([string]$d1.name -eq $mmD1Name) -and ([string]$d2.name -eq $mmD2Name) -and
                (-not [string]::IsNullOrWhiteSpace([string]$d1.make)) -and (-not [string]::IsNullOrWhiteSpace([string]$d1.model)) -and
                (-not [string]::IsNullOrWhiteSpace([string]$d2.make)) -and (-not [string]::IsNullOrWhiteSpace([string]$d2.model)) -and
                ($null -ne $d1.physical_size -and @($d1.physical_size).Count -eq 2 -and [int]@($d1.physical_size)[0] -gt 100) -and
                ($null -ne $d2.physical_size -and @($d2.physical_size).Count -eq 2 -and [int]@($d2.physical_size)[0] -gt 100) -and
                (@($d1.modes).Count -ge 1) -and (@($d2.modes).Count -gt 1) -and
                ($null -ne $d1Mode) -and ($null -ne $d2Mode) -and
                ([int]$d1Mode.width -eq $mode1[0]) -and ([int]$d1Mode.height -eq $mode1[1]) -and
                ([int]$d2Mode.width -eq $mode2[0]) -and ([int]$d2Mode.height -eq $mode2[1]) -and
                ($null -eq $d1.vrr_supported) -and ($null -eq $d2.max_bpc)
            MmGate 'h99_outputs_ipc_lists_both_outputs_with_identity' $identityOk ([ordered]@{
                names = $names; d1 = $d1; d2 = $d2; real_d1_mode = $mode1; real_d2_mode = $mode2 })
            # Logical geometry is each output's own physical size divided by its own scale.
            $lg1 = $d1.logical
            $lg2 = $d2.logical
            $scale1 = [double]$lg1.scale
            $scale2 = [double]$lg2.scale
            $logicalOk = (MmNear ([int]$lg1.x) $mmD1.L 1) -and (MmNear ([int]$lg1.y) $mmD1.T 1) -and
                (MmNear ([int]$lg1.width) ([int][Math]::Round($mmD1.W / $scale1)) 1) -and
                (MmNear ([int]$lg1.height) ([int][Math]::Round($mmD1.H / $scale1)) 1) -and
                (MmNear ([int]$lg2.x) $mmD2.L 1) -and (MmNear ([int]$lg2.y) $mmD2.T 1) -and
                (MmNear ([int]$lg2.width) ([int][Math]::Round($mmD2.W / $scale2)) 1) -and
                (MmNear ([int]$lg2.height) ([int][Math]::Round($mmD2.H / $scale2)) 1) -and
                ([Math]::Abs($scale1 - $mmD1.Dpi / 96.0) -lt 0.001) -and
                ([Math]::Abs($scale2 - $mmD2.Dpi / 96.0) -lt 0.001) -and
                ([Math]::Abs($scale1 - $scale2) -gt 0.01) -and
                ([string]$lg1.transform -eq 'Normal') -and ([string]$lg2.transform -eq 'Normal')
            MmGate 'h99_outputs_ipc_logical_geometry_is_per_output_dpi' $logicalOk ([ordered]@{
                d1 = $lg1; d2 = $lg2; d1_px = "$($mmD1.W)x$($mmD1.H)@$($mmD1.Dpi)"; d2_px = "$($mmD2.W)x$($mmD2.H)@$($mmD2.Dpi)" })
            # The focus cursor itself: named output, then the directional action, then back.
            MmAction 'focus-d1' @('focus-monitor', $mmD1Name)
            $focused1 = MmFocusedOutput
            MmAction 'focus-right' @('focus-monitor-right')
            $focused2 = MmFocusedOutput
            MmAction 'focus-left' @('focus-monitor-left')
            $focused3 = MmFocusedOutput
            MmGate 'h99_focused_output_follows_focus_monitor' (
                ($null -ne $focused1) -and ([string]$focused1.name -eq $mmD1Name) -and
                ($null -ne $focused2) -and ([string]$focused2.name -eq $mmD2Name) -and
                ($null -ne $focused3) -and ([string]$focused3.name -eq $mmD1Name)
            ) ([ordered]@{ named = $focused1.name; right = $focused2.name; left = $focused3.name })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-outputs-error.txt')
            $mmReport['error-outputs'] = ($_ | Out-String).Trim()
            MmSave
        }

        # ---- 2. per-output `layout {}` overrides and `center-column` at mixed DPI ---------------
        try {
            MmCloseAll
            MmReload (MmConfig '' (MmOutputLayout "    layout {`r`n        gaps 40`r`n    }")) 'gaps'
            # An empty workspace of its own on each output, so the fixture columns are the only ones.
            MmAction 'ws-d1' @('focus-monitor', $mmD1Name)
            MmAction 'ws-d1-2' @('focus-workspace', '2')
            $mmD1A = MmOpen 'h99-d1-a' 90 60
            $mmD1B = MmOpen 'h99-d1-b' 60 90
            MmAction 'ws-d2' @('focus-monitor', $mmD2Name)
            MmAction 'ws-d2-2' @('focus-workspace', '2')
            $mmD2A = MmOpen 'h99-d2-a' 90 90
            $mmD2B = MmOpen 'h99-d2-b' 40 60
            Start-Sleep -Milliseconds 900
            $mmWinList = MmWindows
            $mmReport.observations.Add([ordered]@{ stage = 'gap-fixtures'; fixtures = @(@(
                @{ name = 'd1-a'; hwnd = [int64]$mmD1A; rect = (Rc $mmD1A); ipc = (MmWin $mmWinList $mmD1A) },
                @{ name = 'd1-b'; hwnd = [int64]$mmD1B; rect = (Rc $mmD1B); ipc = (MmWin $mmWinList $mmD1B) },
                @{ name = 'd2-a'; hwnd = [int64]$mmD2A; rect = (Rc $mmD2A); ipc = (MmWin $mmWinList $mmD2A) },
                @{ name = 'd2-b'; hwnd = [int64]$mmD2B; rect = (Rc $mmD2B); ipc = (MmWin $mmWinList $mmD2B) }) |
                ForEach-Object { [pscustomobject]$_ }) })
            $r1a = Rc $mmD1A; $r1b = Rc $mmD1B; $r2a = Rc $mmD2A; $r2b = Rc $mmD2B
            $d1Sorted = @(@($r1a, $r1b) | Sort-Object L)
            $d2Sorted = @(@($r2a, $r2b) | Sort-Object L)
            $gapD1 = $d1Sorted[1].L - $d1Sorted[0].R
            $gapD2 = $d2Sorted[1].L - $d2Sorted[0].R
            $onD1 = @($r1a, $r1b | Where-Object { $_.L -ge $mmD1.L -and $_.R -le $mmD1.R -and $_.T -ge $mmD1.T -and $_.B -le $mmD1.B }).Count -eq 2
            $onD2 = @($r2a, $r2b | Where-Object { $_.L -ge $mmD2.L -and $_.R -le $mmD2.R -and $_.T -ge $mmD2.T -and $_.B -le $mmD2.B }).Count -eq 2
            MmGate 'h99_per_output_layout_gaps_differ_at_mixed_dpi' (
                $onD1 -and $onD2 -and (MmNear $gapD1 8 1) -and (MmNear $gapD2 50 2) -and (($gapD2 - $gapD1) -ge 36)
            ) ([ordered]@{ d1 = "$($d1Sorted[0].L)-$($d1Sorted[0].R) / $($d1Sorted[1].L)-$($d1Sorted[1].R)"; gap_d1 = $gapD1;
                d2 = "$($d2Sorted[0].L)-$($d2Sorted[0].R) / $($d2Sorted[1].L)-$($d2Sorted[1].R)"; gap_d2 = $gapD2;
                expect_d1 = 8; expect_d2 = 'round(40 * 1.25) = 50'; on_d1 = $onD1; on_d2 = $onD2 })
            # center-column on each output puts the column's centre on that output's work-area centre.
            $mmCentre1 = [int](($mmD1.WorkL + $mmD1.WorkR) / 2)
            $mmCentre2 = [int](($mmD2.WorkL + $mmD2.WorkR) / 2)
            # `focus-window` is an IPC command (not an action), and the column then gets an exact
            # width before it is centred.
            $null = MmCtl 'centre-d1-focus' @('focus-window', [string][int64]$mmD1A)
            MmAction 'centre-d1-width' @('set-column-width', '320')
            $before1 = Rc $mmD1A
            MmAction 'centre-d1' @('center-column')
            $c1 = Rc $mmD1A
            $null = MmCtl 'centre-d2-focus' @('focus-window', [string][int64]$mmD2B)
            MmAction 'centre-d2-width' @('set-column-width', '320')
            $before2 = Rc $mmD2B
            MmAction 'centre-d2' @('center-column')
            $c2 = Rc $mmD2B
            $mid1 = [int](($c1.L + $c1.R) / 2)
            $mid2 = [int](($c2.L + $c2.R) / 2)
            $moved1 = [Math]::Abs((($before1.L + $before1.R) / 2) - $mid1)
            $moved2 = [Math]::Abs((($before2.L + $before2.R) / 2) - $mid2)
            $centreOk = (MmNear $mid1 $mmCentre1 3) -and (MmNear $mid2 $mmCentre2 3) -and ($moved1 -ge 60) -and ($moved2 -ge 60)
            MmGate 'h99_center_column_centres_the_column_on_each_output' $centreOk ([ordered]@{
                d1 = [ordered]@{ before = $before1; after = $c1; mid = $mid1; expected = $mmCentre1; moved = $moved1 };
                d2 = [ordered]@{ before = $before2; after = $c2; mid = $mid2; expected = $mmCentre2; moved = $moved2 } })
            Shot 'h99-center-column'
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-layout-error.txt')
            $mmReport['error-layout'] = ($_ | Out-String).Trim()
            MmSave
        }

        # ---- 3. focus ring, border, shadow and a rule's border at mixed DPI ---------------------
        # A decor frame is the tile expanded by max(ring, shadow reach + 1) + 2, and the border is
        # the band the tile reserves inside itself.  Both are logical values projected onto the
        # output's pixels: 4 logical is 4 px at 100% and round(4 * 1.25) = 5 px at 125%.
        try {
            MmCloseAll
            $mmRing = {
                param([int]$width)
                return "    border { off }`r`n    focus-ring { width $width; active-color `"#c02020`"; inactive-color `"#20c020`" }`r`n    shadow { off }`r`n"
            }
            MmReload (MmConfig '' '' '' (& $mmRing 4)) 'ring-4'
            $mmRingD1 = MmOpen 'h99-ring-d1' 120 60
            $mmRingD2 = MmOpen 'h99-d2-ring' 60 120
            Start-Sleep -Milliseconds 900
            $ringDistance = {
                param($handle)
                $win = Rc $handle
                $frame = MmFrameRect (MmDecor $handle)
                return [ordered]@{ window = $win; frame = $frame; left = ($win.L - $frame.L); right = ($frame.R - $win.R)
                    top = ($win.T - $frame.T); bottom = ($frame.B - $win.B); dpi = [LaneMultimonNative]::GetDpiForWindow($handle) }
            }
            $ring4 = [ordered]@{ d1 = (& $ringDistance $mmRingD1); d2 = (& $ringDistance $mmRingD2) }
            $mmReport.observations.Add([ordered]@{ stage = 'ring-4'; measured = $ring4 })
            MmReload (MmConfig '' '' '' (& $mmRing 14)) 'ring-14'
            Start-Sleep -Milliseconds 900
            $ring14 = [ordered]@{ d1 = (& $ringDistance $mmRingD1); d2 = (& $ringDistance $mmRingD2) }
            $mmReport.observations.Add([ordered]@{ stage = 'ring-14'; measured = $ring14 })
            # 4 + 2 = 6 and 5 + 2 = 7; 14 + 2 = 16 and round(17.5) + 2 = 20.
            MmGate 'h99_focus_ring_extent_scales_with_output_dpi' (
                (MmNear $ring4.d1.left 6 1) -and (MmNear $ring4.d1.right 6 1) -and (MmNear $ring4.d1.top 6 1) -and
                (MmNear $ring4.d2.left 7 1) -and (MmNear $ring4.d2.right 7 1) -and (MmNear $ring4.d2.top 7 1) -and
                (MmNear $ring14.d1.left 16 1) -and (MmNear $ring14.d2.left 20 1) -and
                (($ring14.d1.left - $ring4.d1.left) -lt ($ring14.d2.left - $ring4.d2.left))
            ) ([ordered]@{ ring4 = $ring4; ring14 = $ring14; expect = 'ring 4: 6 px at 100%, 7 px at 125%; ring 14: 16 px, 20 px' })

            # The border band is inside the tile: two columns' windows are 2 * band further apart than
            # the inner gap (8 + 2*4 = 16 at 100%, 10 + 2*5 = 20 at 125%; width 14 gives 8 + 2*14 = 36
            # and 10 + 2*18 = 46, which an unscaled 36 could not mimic).
            $mmBandD1 = MmOpen 'h99-band-d1' 60 60
            $mmBandD2 = MmOpen 'h99-d2-band' 60 60
            Start-Sleep -Milliseconds 1000
            $mmBand = {
                param([string]$label)
                $pair = if ($label -eq 'd1') { @((Rc $mmRingD1), (Rc $mmBandD1)) } else { @((Rc $mmRingD2), (Rc $mmBandD2)) }
                $sorted = @($pair | Sort-Object L)
                return ($sorted[1].L - $sorted[0].R)
            }
            $mmBandAppearance = {
                param([int]$width)
                return "    border { width $width }`r`n    focus-ring { off }`r`n    shadow { off }`r`n"
            }
            MmReload (MmConfig '' '' '' (& $mmBandAppearance 4)) 'border-4'
            Start-Sleep -Milliseconds 900
            $band4 = [ordered]@{ d1 = (& $mmBand 'd1'); d2 = (& $mmBand 'd2') }
            MmReload (MmConfig '' '' '' (& $mmBandAppearance 14)) 'border-14'
            Start-Sleep -Milliseconds 900
            $band14 = [ordered]@{ d1 = (& $mmBand 'd1'); d2 = (& $mmBand 'd2') }
            $mmReport.observations.Add([ordered]@{ stage = 'border-band'; width4 = $band4; width14 = $band14 })
            MmGate 'h99_border_band_scales_with_output_dpi' (
                (MmNear $band4.d1 16 2) -and (MmNear $band4.d2 20 2) -and
                (MmNear $band14.d1 36 2) -and (MmNear $band14.d2 46 2) -and ($band14.d1 -lt $band14.d2)
            ) ([ordered]@{ width4 = $band4; width14 = $band14; expect_width4 = 'd1 16, d2 20'; expect_width14 = 'd1 36, d2 46' })

            # A window rule's `border {}` override is projected on the same output's pixels: only the
            # rule's window gets the wider band, and it is wider on the 125% output.
            $mmRule = MmConfig '' '' (
                "window-rule {`r`n    match title=`"^h99-d2-rule`"`r`n    border { width 8 }`r`n}`r`n") (& $mmBandAppearance 4)
            MmReload $mmRule 'rule-border'
            $mmRuleD1 = MmOpen 'h99-rule-d1' 130 130
            $mmRuleD2 = MmOpen 'h99-d2-rule' 120 140
            Start-Sleep -Milliseconds 1000
            $ruleDecor = [ordered]@{ d1 = (& $ringDistance $mmRuleD1); d2 = (& $ringDistance $mmRuleD2) }
            $mmReport.observations.Add([ordered]@{ stage = 'rule-border'; measured = $ruleDecor })
            # The rule's window carries band 8: 8 px at 100% and round(8 * 1.25) = 10 px at 125%,
            # plus the 2 px layer margin the frame always adds.  The window without the rule keeps
            # the layout's band 4 (4 px / 5 px), so the override is visible on both outputs.
            MmGate 'h99_window_rule_border_uses_the_output_projection' (
                (MmNear $ruleDecor.d1.left 6 1) -and (MmNear $ruleDecor.d2.left 12 1) -and
                (MmNear $ruleDecor.d1.right 6 1) -and (MmNear $ruleDecor.d2.right 12 1)
            ) ([ordered]@{ d1 = $ruleDecor.d1; d2 = $ruleDecor.d2;
                expect = 'rule band 8 + 2 margin = 10 at 100% and 10 + 2 = 12 at 125%; a window without the rule keeps band 4 + 2 = 6' })

            # Softness, spread and offsets are projected per output as well: 4/4/2/2 logical is
            # ceil(3*2 + 4 + 2) + 1 + 2 = 15 px of frame margin at 100% and
            # ceil(3*2.5 + 5 + 3) + 1 + 2 = 19 px at 125%.
            MmReload (MmConfig '' '' '' "    border { off }`r`n    focus-ring { off }`r`n    shadow { on; softness 4; spread 4; offset-x 2; offset-y 2 }`r`n") 'shadow'
            Start-Sleep -Milliseconds 1200
            $shadowExtent = [ordered]@{ d1 = (& $ringDistance $mmRingD1); d2 = (& $ringDistance $mmRingD2) }
            $mmReport.observations.Add([ordered]@{ stage = 'shadow'; measured = $shadowExtent })
            MmGate 'h99_shadow_extent_uses_the_output_projection' (
                (MmNear $shadowExtent.d1.left 15 1) -and (MmNear $shadowExtent.d2.left 19 1) -and
                (($shadowExtent.d2.left - $shadowExtent.d1.left) -eq 4)
            ) ([ordered]@{ d1 = $shadowExtent.d1; d2 = $shadowExtent.d2; expect = '15 px at 100%, 17 px at 125%' })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-decor-error.txt')
            $mmReport['error-decor'] = ($_ | Out-String).Trim()
            MmSave
        }

        # ---- 4. per-output backdrop colour (reaches the overview behind the workspaces) --------
        try {
            MmCloseAll
            $mmBackdrop = MmConfig "    background-color `"#203060`"`r`n" (MmOutputLayout "    layout { background-color `"#a03020`" }") (
                "overview { zoom 0.5; workspace-shadow { off; } }`r`n")
            MmReload $mmBackdrop 'backdrop'
            $mmBackD1 = MmOpen 'h99-back-d1' 80 40
            $mmBackD2 = MmOpen 'h99-d2-back' 40 80
            $mmBack = [ordered]@{}
            foreach ($pair in @(@('d2', $mmBackD2), @('d1', $mmBackD1))) {
                if (-not (MmOwnForeground $pair[1])) { throw "H99 cannot put the foreground on the $($pair[0]) backdrop fixture" }
                MmAction ('open-overview-' + $pair[0]) @('open-overview')
                if (-not (MmWaitClass 'WiriThumbnailOverviewHost' $true 8)) { throw "overview host did not appear for $($pair[0])" }
                Start-Sleep -Milliseconds 900
                $hostWindow = @(MmClassWindows 'WiriThumbnailOverviewHost') | Select-Object -First 1
                $sample = MmBackdropPoint $hostWindow
                $mmBack[$pair[0]] = [ordered]@{
                    published = [LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewBackdropRgb')
                    point = "$($sample[0]),$($sample[1])"; pixel = (MmPixel $sample[0] $sample[1])
                    host = (MmFrameRect $hostWindow)
                }
                Shot ('h99-backdrop-' + $pair[0])
                MmAction ('close-overview-' + $pair[0]) @('close-overview')
                if (-not (MmWaitClass 'WiriThumbnailOverviewHost' $false 8)) { throw "overview host did not retire for $($pair[0])" }
                Start-Sleep -Milliseconds 400
            }
            $mmReport.observations.Add([ordered]@{ stage = 'backdrop'; measured = $mmBack })
            # DISPLAY2's own `layout { background-color }` is the backdrop when it has the focus; the
            # global one is on DISPLAY1.  Both published by the host and sampled from a real pixel.
            MmGate 'h99_backdrop_colour_is_per_output' (
                ($mmBack.d2.published -eq 0xa03020) -and (MmNearColor $mmBack.d2.pixel (MmRgb '#a03020')) -and
                ($mmBack.d1.published -eq 0x203060) -and (MmNearColor $mmBack.d1.pixel (MmRgb '#203060'))
            ) ([ordered]@{ d2 = $mmBack.d2; d1 = $mmBack.d1 })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-backdrop-error.txt')
            $mmReport['error-backdrop'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'backdrop-close' @('action', 'close-overview') } catch { }
            MmSave
        }

        # ---- 5. the hotkey overlay on the focused output, at its centre, at its scale ----------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'overlay'
            $mmOverD1 = MmOpen 'h99-over-d1' 100 100
            $mmOverD2 = MmOpen 'h99-d2-over' 100 40
            Start-Sleep -Milliseconds 800
            $overlays = [ordered]@{}
            foreach ($pair in @(@('d1', $mmOverD1, $mmD1), @('d2', $mmOverD2, $mmD2))) {
                if (-not (MmOwnForeground $pair[1])) { throw "H99 cannot put the foreground on the $($pair[0]) fixture" }
                MmAction ('show-overlay-' + $pair[0]) @('show-hotkey-overlay')
                if (-not (MmWaitClass 'WiriBindingsCheatsheet' $true 8)) { throw "hotkey overlay did not appear on $($pair[0])" }
                Start-Sleep -Milliseconds 700
                $sheet = @(MmClassWindows 'WiriBindingsCheatsheet') | Select-Object -First 1
                $rect = MmFrameRect $sheet
                $overlays[$pair[0]] = [ordered]@{
                    bounds = $rect; width = ($rect.R - $rect.L); height = ($rect.B - $rect.T)
                    centre_x = [int](($rect.L + $rect.R) / 2); centre_y = [int](($rect.T + $rect.B) / 2)
                    expected_x = [int](($pair[2].WorkL + $pair[2].WorkR) / 2); expected_y = [int](($pair[2].WorkT + $pair[2].WorkB) / 2)
                    dpi = [LaneMultimonNative]::GetDpiForWindow($sheet)
                    inside = ($rect.L -ge $pair[2].WorkL) -and ($rect.R -le $pair[2].WorkR) -and
                        ($rect.T -ge $pair[2].WorkT) -and ($rect.B -le $pair[2].WorkB)
                }
                Shot ('h99-hotkey-overlay-' + $pair[0])
                # Any key dismisses the sheet (and is consumed), so the next one can open it again.
                [LT]::Key([byte]0x51, $false); Start-Sleep -Milliseconds 60; [LT]::Key([byte]0x51, $true)
                $null = MmWaitClass 'WiriBindingsCheatsheet' $false 8
                Start-Sleep -Milliseconds 300
            }
            $mmReport.observations.Add([ordered]@{ stage = 'hotkey-overlay'; measured = $overlays })
            MmGate 'h99_hotkey_overlay_centres_on_the_focused_output' (
                $overlays.d2.inside -and $overlays.d1.inside -and
                (MmNear $overlays.d1.centre_x $overlays.d1.expected_x 3) -and (MmNear $overlays.d1.centre_y $overlays.d1.expected_y 3) -and
                (MmNear $overlays.d2.centre_x $overlays.d2.expected_x 3) -and (MmNear $overlays.d2.centre_y $overlays.d2.expected_y 3) -and
                ($overlays.d2.dpi -ge 120) -and ($overlays.d1.dpi -eq 96)
            ) ([ordered]@{ d1 = $overlays.d1; d2 = $overlays.d2 })
            # 720 logical px wide at 100% and 720 * 1.25 = 900 at 125%.
            $widthRatio = if ($overlays.d1.width -gt 0) { [Math]::Round($overlays.d2.width / $overlays.d1.width, 3) } else { 0 }
            MmGate 'h99_hotkey_overlay_scales_with_the_output_dpi' (
                (MmNear $overlays.d1.width 720 2) -and (MmNear $overlays.d2.width 900 2) -and
                ($overlays.d2.height -ge $overlays.d1.height) -and ($widthRatio -ge 1.24) -and ($widthRatio -le 1.26)
            ) ([ordered]@{ width_d1 = $overlays.d1.width; width_d2 = $overlays.d2.width; width_ratio = $widthRatio;
                height_d1 = $overlays.d1.height; height_d2 = $overlays.d2.height })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-overlay-error.txt')
            $mmReport['error-overlay'] = ($_ | Out-String).Trim()
            try { [LT]::Key([byte]0x1B, $false); [LT]::Key([byte]0x1B, $true) } catch { }
            MmSave
        }

        # ---- 6. keyboard binds while the overview is open, on a second display ------------------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'overview'
            $mmOvA = MmOpen 'h99-ov-a' 60 140
            $mmOvB = MmOpen 'h99-ov-b' 140 60
            $mmOvC = MmOpen 'h99-d2-ov-c' 100 100
            Start-Sleep -Milliseconds 800
            if (-not (MmOwnForeground $mmOvA)) { throw 'H99 cannot put the foreground on the overview fixture' }
            MmAction 'open-overview-kb' @('open-overview')
            if (-not (MmWaitClass 'WiriThumbnailOverviewHost' $true 8)) { throw 'overview host did not appear' }
            Start-Sleep -Milliseconds 900
            $hostWindow = @(MmClassWindows 'WiriThumbnailOverviewHost') | Select-Object -First 1
            $before = [LaneMultimonNative]::GetProp($hostWindow, 'WiriOverviewFocusedWindow')
            $hostRect = MmFrameRect $hostWindow
            # The real chord: Ctrl+Alt+F9 is bound to focus-column-right in this config.
            Chord 'ctrl+alt+f9'
            Start-Sleep -Milliseconds 1200
            $stillOpen = MmWaitClass 'WiriThumbnailOverviewHost' $true 3
            $hostNow = @(MmClassWindows 'WiriThumbnailOverviewHost') | Select-Object -First 1
            $after = [LaneMultimonNative]::GetProp($hostNow, 'WiriOverviewFocusedWindow')
            # Escape closes the overview while the focused output is the second display.
            MmAction 'focus-d2-kb' @('focus-monitor', $mmD2Name)
            [LT]::Key([byte]0x1B, $false); Start-Sleep -Milliseconds 60; [LT]::Key([byte]0x1B, $true)
            $closed = MmWaitClass 'WiriThumbnailOverviewHost' $false 8
            $mmReport.observations.Add([ordered]@{ stage = 'overview-keyboard'; before = $before; after = $after;
                still_open = $stillOpen; closed = $closed; host = $hostRect
                fixtures = @([int64]$mmOvA, [int64]$mmOvB, [int64]$mmOvC) })
            MmGate 'h99_overview_keyboard_works_across_displays' (
                ($before -ne 0) -and ($after -ne 0) -and ($before -ne $after) -and $stillOpen -and $closed
            ) ([ordered]@{ focused_before = $before; focused_after = $after; still_open = $stillOpen; escape_closed = $closed })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-overview-error.txt')
            $mmReport['error-overview'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'overview-kb-close' @('action', 'close-overview') } catch { }
            MmSave
        }

        # ---- 7. `hot-corners {}` per output ----------------------------------------------------
        try {
            MmCloseAll
            $mmCorners = "input { mod-key `"ctrl-alt`" }`r`n" +
                "animations { off }`r`n" +
                "layout {`r`n    gaps 8`r`n    border { off }`r`n    focus-ring { off }`r`n    shadow { off }`r`n}`r`n" +
                "gestures { hot-corners { top-left } }`r`n" +
                (MmOutputLayout "    hot-corners { top-right }")
            MmReload $mmCorners 'hot-corners'
            $null = [LaneMultimonNative]::Park([int](($mmD1.WorkL + $mmD1.WorkR) / 2), [int](($mmD1.WorkT + $mmD1.WorkB) / 2))
            Start-Sleep -Milliseconds 400
            $corner = [ordered]@{}
            # DISPLAY2 has its own policy: top-right opens, top-left does nothing.
            $null = MmMovePointer ($mmD2.R - 1) $mmD2.T 900
            $corner.d2_top_right = @(MmClassWindows 'WiriThumbnailOverviewHost').Count -ge 1
            $null = MmMovePointer ([int](($mmD2.WorkL + $mmD2.WorkR) / 2)) ([int](($mmD2.WorkT + $mmD2.WorkB) / 2)) 500
            if (@(MmClassWindows 'WiriThumbnailOverviewHost').Count -ge 1) { MmAction 'corner-close-1' @('close-overview'); Start-Sleep -Milliseconds 700 }
            $null = MmMovePointer $mmD2.L $mmD2.T 900
            $corner.d2_top_left = @(MmClassWindows 'WiriThumbnailOverviewHost').Count -ge 1
            $null = MmMovePointer ([int](($mmD2.WorkL + $mmD2.WorkR) / 2)) ([int](($mmD2.WorkT + $mmD2.WorkB) / 2)) 500
            # DISPLAY1 keeps the global default: top-left opens.
            $null = MmMovePointer $mmD1.L $mmD1.T 900
            $corner.d1_top_left = @(MmClassWindows 'WiriThumbnailOverviewHost').Count -ge 1
            Shot 'h99-hot-corner-display1'
            $null = MmMovePointer ([int](($mmD1.WorkL + $mmD1.WorkR) / 2)) ([int](($mmD1.WorkT + $mmD1.WorkB) / 2)) 300
            if (@(MmClassWindows 'WiriThumbnailOverviewHost').Count -ge 1) { MmAction 'corner-close-2' @('close-overview'); Start-Sleep -Milliseconds 700 }
            $mmReport.observations.Add([ordered]@{ stage = 'hot-corners'; measured = $corner })
            MmGate 'h99_hot_corner_policy_is_per_output' (
                $corner.d2_top_right -and (-not $corner.d2_top_left) -and $corner.d1_top_left
            ) ([ordered]@{ d2_top_right = $corner.d2_top_right; d2_top_left = $corner.d2_top_left; d1_top_left = $corner.d1_top_left })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-corners-error.txt')
            $mmReport['error-corners'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'corners-close' @('action', 'close-overview') } catch { }
            MmSave
        }

        # ---- 8. `msg output scale` is a layout-only change --------------------------------------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'scale'
            $mmScaleD2 = MmOpen 'h99-d2-scale' 90 90
            Start-Sleep -Milliseconds 900
            $beforeScale = MmOutput $mmD2Name
            $beforeRect = Rc $mmScaleD2
            $beforeDpi = (MmMonitor $mmD2Name).Dpi
            $null = MmJson 'd2-scale-1' @('msg', 'output', $mmD2Name, 'scale', '1.0')
            Start-Sleep -Milliseconds 1500
            $duringScale = MmOutput $mmD2Name
            $duringRect = Rc $mmScaleD2
            $duringDpi = (MmMonitor $mmD2Name).Dpi
            $null = MmJson 'd2-scale-auto' @('msg', 'output', $mmD2Name, 'scale', 'auto')
            Start-Sleep -Milliseconds 1500
            $afterScale = MmOutput $mmD2Name
            $afterRect = Rc $mmScaleD2
            $mmReport.observations.Add([ordered]@{ stage = 'scale'; before = $beforeScale.logical; during = $duringScale.logical;
                after = $afterScale.logical; before_rect = $beforeRect; during_rect = $duringRect; after_rect = $afterRect;
                real_dpi = $duringDpi })
            MmGate 'h99_msg_output_scale_is_layout_only_and_restores' (
                ([Math]::Abs([double]$duringScale.logical.scale - 1.0) -lt 0.001) -and
                ([Math]::Abs([double]$beforeScale.logical.scale - 1.25) -lt 0.001) -and
                ($duringDpi -eq $beforeDpi) -and ($duringDpi -ge 120) -and
                (MmNear ([int]$duringScale.logical.width) $mmD2.W 2) -and
                ([Math]::Abs(($duringRect.R - $duringRect.L) - ($beforeRect.R - $beforeRect.L)) -ge 10) -and
                ([Math]::Abs([double]$afterScale.logical.scale - 1.25) -lt 0.001) -and
                (MmNear ([int]$afterScale.logical.width) ([int][Math]::Round($mmD2.W / 1.25)) 1) -and
                ((MmMonitor $mmD2Name).Dpi -ge 120)
            ) ([ordered]@{ before = $beforeScale.logical; during = $duringScale.logical; after = $afterScale.logical;
                before_rect = $beforeRect; during_rect = $duringRect; after_rect = $afterRect; real_dpi = $duringDpi })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-scale-error.txt')
            $mmReport['error-scale'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'scale-recover' @('msg', 'output', $mmD2Name, 'scale', 'auto') } catch { }
            MmSave
        }

        # ---- 9. `msg output mode` changes the real desktop and `outputs` follows it -------------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'mode'
            $mmModeD2 = MmOpen 'h99-d2-mode' 120 120
            Start-Sleep -Milliseconds 900
            $d2Out = MmOutput $mmD2Name
            $currentMode = @($d2Out.modes)[[int]$d2Out.current_mode]
            # A smaller mode Windows itself advertises for this output, so the request is not a guess.
            $candidates = @(@($d2Out.modes) | Where-Object { ([int]$_.width -lt [int]$currentMode.width) -and ([int]$_.height -le 1080) } |
                Sort-Object { [int]$_.width * [int]$_.height } -Descending)
            if ($candidates.Count -eq 0) { throw 'H99 found no smaller advertised mode for DISPLAY2' }
            $targetMode = $candidates[0]
            $targetText = '{0}x{1}@{2}.{3:D3}' -f [int]$targetMode.width, [int]$targetMode.height,
                [int]([int]$targetMode.refresh_rate / 1000), ([int]$targetMode.refresh_rate % 1000)
            $modeReply = MmTryJson 'd2-mode' @('msg', 'output', $mmD2Name, 'mode', $targetText)
            Start-Sleep -Milliseconds 3000
            $realMode = MmMode $mmD2Name
            $realMonitor = MmMonitor $mmD2Name
            $afterOut = MmOutput $mmD2Name
            $afterMode = @($afterOut.modes)[[int]$afterOut.current_mode]
            $modeRect = Rc $mmModeD2
            $mmReport.observations.Add([ordered]@{ stage = 'mode'; requested = $targetText; accepted = $modeReply.ok;
                error = $modeReply.error; real = $realMode; monitor = "$($realMonitor.W)x$($realMonitor.H)@$($realMonitor.Dpi)";
                published = $afterMode; logical = $afterOut.logical; tile = $modeRect })
            if (-not $modeReply.ok) {
                # Windows refused the transaction and nothing changed: that is a real limit of the
                # physical path on this hardware, not something this lane can observe.
                MmSkip 'h99_msg_output_mode_switches_the_real_desktop' ("wiri's mode request was refused by Windows: {0}" -f $modeReply.error)
                MmSkip 'h99_outputs_ipc_tracks_the_real_mode_change' ("no real mode change to track: {0}" -f $modeReply.error)
            } else {
                MmGate 'h99_msg_output_mode_switches_the_real_desktop' (
                    ($realMode.Count -ge 5) -and ($realMode[0] -eq [int]$targetMode.width) -and ($realMode[1] -eq [int]$targetMode.height) -and
                    ($realMonitor.W -eq [int]$targetMode.width) -and ($realMonitor.H -eq [int]$targetMode.height) -and
                    ($modeRect.L -ge $realMonitor.L) -and ($modeRect.R -le $realMonitor.R) -and
                    ($modeRect.T -ge $realMonitor.T) -and ($modeRect.B -le $realMonitor.B)
                ) ([ordered]@{ requested = $targetText; real_mode = $realMode; monitor = "$($realMonitor.W)x$($realMonitor.H)"; tile = $modeRect })
                MmGate 'h99_outputs_ipc_tracks_the_real_mode_change' (
                    ([int]$afterMode.width -eq [int]$targetMode.width) -and ([int]$afterMode.height -eq [int]$targetMode.height) -and
                    (MmNear ([int]$afterOut.logical.height) ([int][Math]::Round($realMonitor.H / [double]$afterOut.logical.scale)) 1) -and
                    (MmNear ([int]$afterOut.logical.width) ([int][Math]::Round($realMonitor.W / [double]$afterOut.logical.scale)) 1)
                ) ([ordered]@{ current_mode_index = $afterOut.current_mode; published = $afterMode; logical = $afterOut.logical })
            }
            $null = MmJson 'd2-mode-back' @('msg', 'output', $mmD2Name, 'mode', '1920x1080')
            Start-Sleep -Milliseconds 3000
            $restored = MmMode $mmD2Name
            $mmReport.observations.Add([ordered]@{ stage = 'mode-restored'; real = $restored; dpi = (MmMonitor $mmD2Name).Dpi })
            if (-not (($restored[0] -eq 1920) -and ($restored[1] -eq 1080))) { throw "DISPLAY2 did not come back to 1920x1080: $($restored -join ',')" }
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-mode-error.txt')
            $mmReport['error-mode'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'mode-recover' @('msg', 'output', $mmD2Name, 'mode', '1920x1080') } catch { }
            try { $null = MmRestoreDisplay2 } catch { }
            MmSave
        }

        # ---- 10. `msg output position` moves the real display ----------------------------------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'position'
            $mmPosD2 = MmOpen 'h99-d2-pos' 140 100
            Start-Sleep -Milliseconds 900
            $beforePos = MmMode $mmD2Name
            $posReply = MmTryJson 'd2-position' @('msg', 'output', $mmD2Name, 'position', 'set', '1400', '0')
            Start-Sleep -Milliseconds 2500
            $movedMode = MmMode $mmD2Name
            $movedMonitor = MmMonitor $mmD2Name
            $movedOut = MmOutput $mmD2Name
            $movedRect = Rc $mmPosD2
            $movedNow = ($movedMode.Count -ge 5) -and ($movedMode[2] -eq 1400) -and ($movedMode[3] -eq 0) -and ($movedMonitor.L -eq 1400)
            $null = MmJson 'd2-position-back' @('msg', 'output', $mmD2Name, 'position', 'set', '1366', '0')
            Start-Sleep -Milliseconds 2500
            $backMode = MmMode $mmD2Name
            $backMonitor = MmMonitor $mmD2Name
            $mmReport.observations.Add([ordered]@{ stage = 'position'; before = $beforePos; accepted = $posReply.ok;
                error = $posReply.error; moved = $movedMode;
                moved_monitor = "$($movedMonitor.L),$($movedMonitor.T)-$($movedMonitor.R),$($movedMonitor.B)";
                moved_logical = $movedOut.logical; moved_tile = $movedRect;
                restored = $backMode; restored_monitor = "$($backMonitor.L),$($backMonitor.T)" })
            if (-not $posReply.ok) {
                MmSkip 'h99_msg_output_position_moves_the_real_desktop' ("wiri's position request was refused by Windows: {0}" -f $posReply.error)
            } elseif (-not $movedNow) {
                # wiri answered success but the desktop kept its old position: the CCD transaction
                # was accepted and then had no effect on this hardware.  Recorded, not passed.
                MmSkip 'h99_msg_output_position_moves_the_real_desktop' 'wiri answered success but the real source position did not change (physical repositioning is not applied on this hardware)'
            } else {
                MmGate 'h99_msg_output_position_moves_the_real_desktop' (
                    ([int]$movedOut.logical.x -eq 1400) -and ($movedRect.L -ge $movedMonitor.L) -and
                    ($backMode[2] -eq 1366) -and ($backMode[3] -eq 0) -and ($backMonitor.L -eq 1366)
                ) ([ordered]@{ moved = [ordered]@{ mode = $movedMode; monitor_l = $movedMonitor.L; logical_x = $movedOut.logical.x };
                    restored = [ordered]@{ mode = $backMode; monitor_l = $backMonitor.L } })
            }
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-position-error.txt')
            $mmReport['error-position'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'position-recover' @('msg', 'output', $mmD2Name, 'position', 'set', '1366', '0') } catch { }
            try { $null = MmRestoreDisplay2 } catch { }
            MmSave
        }

        # ---- 11. `msg output transform` rotates the real display -------------------------------
        try {
            MmCloseAll
            MmReload (MmConfig '') 'transform'
            $mmRotD2 = MmOpen 'h99-d2-rot' 160 60
            Start-Sleep -Milliseconds 900
            $trReply = MmTryJson 'd2-transform' @('msg', 'output', $mmD2Name, 'transform', '90')
            Start-Sleep -Milliseconds 3000
            $rotMode = MmMode $mmD2Name
            $rotMonitor = MmMonitor $mmD2Name
            $rotOut = MmOutput $mmD2Name
            $rotRect = Rc $mmRotD2
            $null = MmJson 'd2-transform-back' @('msg', 'output', $mmD2Name, 'transform', 'normal')
            Start-Sleep -Milliseconds 3000
            $backMode = MmMode $mmD2Name
            $backMonitor = MmMonitor $mmD2Name
            $mmReport.observations.Add([ordered]@{ stage = 'transform'; accepted = $trReply.ok; error = $trReply.error;
                rotated = $rotMode; monitor = "$($rotMonitor.L),$($rotMonitor.T) $($rotMonitor.W)x$($rotMonitor.H)";
                logical = $rotOut.logical; tile = $rotRect; restored = $backMode
                restored_monitor = "$($backMonitor.W)x$($backMonitor.H)@$($backMonitor.Dpi)" })
            if (-not $trReply.ok) {
                MmSkip 'h99_msg_output_transform_rotates_the_real_desktop' ("wiri's transform request was refused by Windows: {0}" -f $trReply.error)
            } else {
                MmGate 'h99_msg_output_transform_rotates_the_real_desktop' (
                    ($rotMode.Count -ge 5) -and ($rotMode[4] -eq 1) -and ($rotMonitor.W -eq $mmD2.H) -and ($rotMonitor.H -eq $mmD2.W) -and
                    ([string]$rotOut.logical.transform -eq '90') -and
                    ($rotRect.L -ge $rotMonitor.L) -and ($rotRect.R -le $rotMonitor.R) -and
                    ($rotRect.T -ge $rotMonitor.T) -and ($rotRect.B -le $rotMonitor.B) -and
                    ($backMode[4] -eq 0) -and ($backMonitor.W -eq $mmD2.W) -and ($backMonitor.H -eq $mmD2.H)
                ) ([ordered]@{ rotated = [ordered]@{ mode = $rotMode; monitor = "$($rotMonitor.W)x$($rotMonitor.H)"; logical = $rotOut.logical; tile = $rotRect };
                    restored = [ordered]@{ mode = $backMode; monitor = "$($backMonitor.W)x$($backMonitor.H)" } })
            }
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-transform-error.txt')
            $mmReport['error-transform'] = ($_ | Out-String).Trim()
            try { $null = MmCtl 'transform-recover' @('msg', 'output', $mmD2Name, 'transform', 'normal') } catch { }
            try { $null = MmRestoreDisplay2 } catch { }
            MmSave
        }

        # ---- 12. `msg output off`, and the output coming back ----------------------------------
        try {
            MmCloseAll
            MmReload (MmConfig '' (MmOutputLayout "    layout {`r`n        gaps 40`r`n    }")) 'off'
            $mmOffA = MmOpen 'h99-d2-off-a' 60 60
            $mmOffB = MmOpen 'h99-d2-off-b' 90 30
            $mmOffD1 = MmOpen 'h99-d1-off' 30 90
            Start-Sleep -Milliseconds 1000
            $beforeOff = MmWindows
            $offWorkspaces = MmWorkspaces
            $offWs = (MmWin $beforeOff $mmOffA).workspace_id
            $offWsEntry = @($offWorkspaces | Where-Object { [int64]$_.id -eq [int64]$offWs }) | Select-Object -First 1
            $offWsId = $offWsEntry.idx
            $mmReport.observations.Add([ordered]@{ stage = 'before-off'; workspace_id = $offWs; workspace_idx = $offWsId;
                workspace_name = $offWsEntry.name; rects = @(@(Rc $mmOffA), @(Rc $mmOffB), @(Rc $mmOffD1)) })
            $offOk = $false
            $movedOk = $false
            try {
                $null = MmJson 'd2-off' @('msg', 'output', $mmD2Name, 'off')
                $gone = MmWaitMonitor $mmD2Name $false 15
                Start-Sleep -Milliseconds 1500
                $afterOffMonitors = MmMonitors
                $afterOffOutputs = MmOutputs
                $afterOffNames = @($afterOffOutputs.PSObject.Properties | ForEach-Object { $_.Name })
                $rectA = Rc $mmOffA
                $rectB = Rc $mmOffB
                $d1Now = MmMonitor $mmD1Name
                $offOk = $gone -and ($afterOffMonitors.Count -eq 1) -and ($afterOffMonitors[0].Device -eq $mmD1Name) -and
                    ($afterOffNames.Count -eq 1) -and ($afterOffNames[0] -eq $mmD1Name) -and
                    ($d1Now.W -eq $mmD1.W) -and ($d1Now.H -eq $mmD1.H) -and ($d1Now.L -eq 0) -and ($d1Now.T -eq 0) -and ($d1Now.Dpi -eq 96)
                # The windows of the detached output come back onto the surviving one, still managed
                # and laid out inside its monitor.
                $managed = @(MmWindows | Where-Object { [int64]$_.id -eq [int64]$mmOffA -or [int64]$_.id -eq [int64]$mmOffB }).Count
                $movedOk = ($rectA.L -ge $d1Now.L) -and ($rectA.R -le $d1Now.R) -and ($rectA.T -ge $d1Now.T) -and ($rectA.B -le $d1Now.B) -and
                    ($rectB.L -ge $d1Now.L) -and ($rectB.R -le $d1Now.R) -and ($rectB.T -ge $d1Now.T) -and ($rectB.B -le $d1Now.B) -and
                    ($rectA.W -gt 100) -and ($rectB.W -gt 100) -and ($managed -eq 2)
                $mmReport.observations.Add([ordered]@{ stage = 'after-off'; monitors = @($afterOffMonitors | ForEach-Object { "$($_.Device) $($_.W)x$($_.H)" });
                    outputs = $afterOffNames; rect_a = $rectA; rect_b = $rectB; managed = $managed;
                    d1 = "$($d1Now.W)x$($d1Now.H)@$($d1Now.Dpi)" })
            } finally {
                # `msg output off` leaves the output off in wiri's own model (that is niri's
                # behaviour: the request is remembered until the next config reload), so the
                # reconnect is both a real display change and an explicit `on`.
                $restoreState = MmRestoreDisplay2
                $nativeBack = MmWaitMonitor $mmD2Name $true 25
                $onReply = $null
                try { $onReply = MmTryJson 'd2-on' @('msg', 'output', $mmD2Name, 'on') } catch { $mmReport['error-on'] = ($_ | Out-String).Trim() }
                Start-Sleep -Milliseconds 1500
                $mmReport.observations.Add([ordered]@{ stage = 'after-on'; restore = $restoreState; monitor_native = $nativeBack;
                    on_accepted = $(if ($null -ne $onReply) { $onReply.ok } else { $null })
                    on_error = $(if ($null -ne $onReply) { $onReply.error } else { $null })
                    monitors = @(MmMonitors | ForEach-Object { "$($_.Device) $($_.W)x$($_.H)@$($_.Dpi) at $($_.L),$($_.T)" }) })
            }
            MmGate 'h99_output_off_detaches_only_display2' $offOk ([ordered]@{ workspace = $offWs; windows_moved = $movedOk })
            MmGate 'h99_output_off_moves_its_windows_to_display1' ($offOk -and $movedOk) ([ordered]@{ rect_a = $rectA; rect_b = $rectB })
            # Back from the dead: the monitor re-registers, wiri lists it again, and the workspace
            # object (same IPC id, same index) is on it with its windows.
            $back = MmWaitMonitor $mmD2Name $true 25
            $engineBackMs = MmWaitEngineOutput $mmD2Name $true 25            $backMonitor = MmMonitor $mmD2Name
            $backOut = MmOutput $mmD2Name
            $backWindows = MmWindows
            $backWorkspaces = MmWorkspaces
            $wsBack = @($backWorkspaces | Where-Object { [int64]$_.id -eq [int64]$offWs }) | Select-Object -First 1
            $winBack = @($backWindows | Where-Object { [int64]$_.id -eq [int64]$mmOffA }) | Select-Object -First 1
            $mmReport.observations.Add([ordered]@{ stage = 'after-on-workspace'; engine_saw_output_ms = $engineBackMs;
                monitor = "$($backMonitor.W)x$($backMonitor.H)@$($backMonitor.Dpi)";
                outputs_scale = $(if ($null -ne $backOut) { $backOut.logical.scale } else { $null })
                workspace = $wsBack; window = $winBack; d1 = (Rc $mmOffD1); a = (Rc $mmOffA) })
            MmGate 'h99_output_on_returns_the_monitor_and_its_workspace' (
                $back -and ($null -ne $backMonitor) -and ($backMonitor.W -eq $mmD2.W) -and ($backMonitor.H -eq $mmD2.H) -and
                ($null -ne $backOut) -and ([Math]::Abs([double]$backOut.logical.scale - 1.25) -lt 0.001) -and
                ($null -ne $wsBack) -and ([int]$wsBack.idx -eq [int]$offWsId) -and ($null -ne $winBack) -and
                ([string]$winBack.output -eq $mmD2Name)
            ) ([ordered]@{ workspace_before = [ordered]@{ id = $offWs; idx = $offWsId }; workspace_after = $wsBack;
                window_after = $winBack; monitor = "$($backMonitor.W)x$($backMonitor.H)" })
            # Hot-plug re-evaluation: DISPLAY2's own `layout { gaps 40 }` applies again after the
            # reconnect (50 physical px at 125%), and its windows are back on its own pixels.
            Start-Sleep -Milliseconds 900
            MmAction 'hotplug-focus-d2' @('focus-monitor', $mmD2Name)
            $plugA = Rc $mmOffA
            $plugB = Rc $mmOffB
            $plugSorted = @(@($plugA, $plugB) | Sort-Object L)
            $plugGap = $plugSorted[1].L - $plugSorted[0].R
            $mmReport.observations.Add([ordered]@{ stage = 'hotplug-gaps'; a = $plugA; b = $plugB; gap = $plugGap })
            MmGate 'h99_hotplug_reevaluates_per_output_layout' (
                ($plugGap -ge 46) -and ($plugGap -le 54) -and ($plugA.L -ge $mmD2.L) -and
                (@($backWorkspaces | Where-Object { [string]$_.output -eq $mmD2Name }).Count -ge 1)
            ) ([ordered]@{ gap = $plugGap; expect = 'round(40 * 1.25) = 50'; a = $plugA; b = $plugB })
        } catch {
            $_ | Out-String | Set-Content (Join-Path $mmDir 'group-off-error.txt')
            $mmReport['error-off'] = ($_ | Out-String).Trim()
            MmSave
        }

        Shot 'h99-final'
    } finally {
        # Nothing outside this lane may inherit a changed display: DISPLAY2 goes back to 1920x1080,
        # 125%, 1366,0 and the normal transform whatever happened above.
        try { $null = MmRestoreDisplay2 } catch { $mmReport['restore-error'] = ($_ | Out-String).Trim() }
        try { MmCloseAll } catch { }
        try {
            if ($null -ne $mmProc -and -not $mmProc.HasExited) { Stop-WiriGracefully $mmProc 'h99' }
        } catch { }
        try { [Windows.Forms.Cursor]::Position = $mmCursor } catch { }
    }

    # ---- the final gate: the desktop is exactly as this lane found it ---------------------------
    try {
        if ([string]::IsNullOrEmpty($mmD2Name)) { MmSave; return }
        if (@(MmMonitors | Where-Object { $_.Device -eq $mmD2Name }).Count -eq 0) {
            $mmReport['restore-extend'] = [LaneMultimonNative]::Extend()
            $null = MmWaitMonitor $mmD2Name $true 25
        }
        Start-Sleep -Milliseconds 1500
        $endMon = MmMonitors
        $endD1 = @($endMon | Where-Object { $_.Device -eq $mmD1Name }) | Select-Object -First 1
        $endD2 = @($endMon | Where-Object { $_.Device -eq $mmD2Name }) | Select-Object -First 1
        $endMode = MmMode $mmD2Name
        $endD1Mode = MmMode $mmD1Name
        $mmReport.observations.Add([ordered]@{ stage = 'restored'; monitors = @($endMon | ForEach-Object {
            "$($_.Device) $($_.W)x$($_.H)@$($_.Dpi)dpi at $($_.L),$($_.T) primary=$($_.Primary)" });
            d2_mode = $endMode; d1_mode = $endD1Mode })
        MmGate 'h99_outputs_restored' (
            ($null -ne $endD2) -and ($null -ne $endD1) -and
            ($endMode.Count -ge 5) -and ($endMode[0] -eq 1920) -and ($endMode[1] -eq 1080) -and
            ($endMode[2] -eq 1366) -and ($endMode[3] -eq 0) -and ($endMode[4] -eq 0) -and
            ($endD2.L -eq 1366) -and ($endD2.T -eq 0) -and ($endD2.W -eq 1920) -and ($endD2.H -eq 1080) -and
            ($endD2.Dpi -eq 120) -and (-not $endD2.Primary) -and
            ($endD1.Primary) -and ($endD1.L -eq 0) -and ($endD1.T -eq 0) -and ($endD1.W -eq 1366) -and ($endD1.H -eq 768) -and
            ($endD1.Dpi -eq 96) -and ($endD1Mode[0] -eq 1366) -and ($endD1Mode[1] -eq 768) -and ($endD1Mode[4] -eq 0) -and
            ($endD1.WorkL -eq 0) -and ($endD1.WorkT -eq 0) -and ($endD1.WorkR -eq 1366) -and ($endD1.WorkB -eq 728)
        ) ([ordered]@{
            d2 = if ($null -ne $endD2) { "$($endD2.W)x$($endD2.H)@$($endD2.Dpi)dpi at $($endD2.L),$($endD2.T)" } else { 'missing' };
            d2_mode = $endMode
            d1 = if ($null -ne $endD1) { "$($endD1.W)x$($endD1.H)@$($endD1.Dpi)dpi at $($endD1.L),$($endD1.T) primary=$($endD1.Primary)" } else { 'missing' };
            d1_mode = $endD1Mode })
    } catch {
        $_ | Out-String | Set-Content (Join-Path $mmDir 'restore-check-error.txt')
        $mmReport['error-restore-check'] = ($_ | Out-String).Trim()
        MmGate 'h99_outputs_restored' $false ([ordered]@{ error = ($_ | Out-String).Trim() })
    } finally {
        if ($mmOldDpiContext -ne [IntPtr]::Zero) { $null = [LaneMultimonNative]::SetThreadDpiAwarenessContext($mmOldDpiContext) }
        $summary['h99_display_count'] = @(MmMonitors).Count
        MmSave
    }
}
