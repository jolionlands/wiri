# Lane OVERVIEW / H97: the overview is niri's fixed-zoom camera over the infinite 2D grid
# (x = each workspace's own strip of columns, y = the workspaces stacked vertically), not a
# "fit every column and every workspace on one screen" zoom.
#
# niri source pinned in docs/parity/niri-gaps-layout-behaviour.md section 9:
#  * compute_overview_zoom (src/layout/mod.rs:5034): zoom = clamp(overview{zoom}, 0.0001, 0.75),
#    animated as 1 - progress*(1-zoom).
#  * Monitor::workspaces_render_geo (src/layout/monitor.rs:1467): workspace size = view*zoom,
#    gap = 10% of the view height * zoom, the focused workspace centred, neighbours above and
#    below, one entry per workspace (empty ones included), the whole stack a camera.
#  * Monitor::render_workspaces (src/layout/monitor.rs:1735): every workspace relocated to its
#    stack rectangle, rescaled by the zoom (so a tile is exactly zoom * its normal size) and
#    cropped inside that workspace.
#
# The parent harness owns the desktop mutex. This phase owns its daemon and its forms, only
# reads the geometry the host publishes as window properties plus the typed `focused-window`
# request, and changes no display or input setting. Gates are enforced; the p_overview_* keys
# are information only. The ctl forms are the ones verified by hand on the desktop:
# `focus-window <hwnd>`, `move-window-to-workspace <id>`, `switch-workspace <id>` and
# `action <niri name>`.
$h97Gates = @(
    'h97_thumbnails_are_the_configured_zoom',
    'h97_active_workspace_is_centred',
    'h97_workspace_gap_and_stack',
    'h97_focus_column_right_pans_camera',
    'h97_focus_workspace_down_pans_camera',
    'h97_zoom_config_after_reload',
    'h97_click_on_panned_thumbnail_selects',
    'h97_overview_geometry_cleans_up'
)
foreach ($gate in $h97Gates) { $summary[$gate] = $false }
foreach ($key in @('p_overview_open_ms', 'p_overview_close_ms', 'p_overview_publish_interval_ms_mean',
        'p_overview_publish_interval_ms_max', 'p_overview_cpu_ms', 'p_overview_thumbnails')) {
    $summary[$key] = -1
}
if (-not ('H97Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H97Native {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string name);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@
}
function H97Host { return [H97Native]::FindWindow('WiriThumbnailOverviewHost', $null) }
function H97Prop($h, [string]$name) {
    if ($h -eq [IntPtr]::Zero) { return -1 }
    return [H97Native]::GetProp($h, ('WiriOverview' + $name)).ToInt64()
}
# The active workspace's rectangle in the stack (x/y/w/h) and its visible index.
function H97Active($h) {
    $active = H97Prop $h 'ActiveWorkspace'
    $count = H97Prop $h 'WorkspaceCount'
    for ($i = 0; $i -lt $count; $i++) {
        if ((H97Prop $h ("Workspace{0}Id" -f $i)) -eq $active) {
            return [pscustomobject]@{
                index = $i; id = $active
                x = H97Prop $h ("Workspace{0}X" -f $i); y = H97Prop $h ("Workspace{0}Y" -f $i)
                w = H97Prop $h ("Workspace{0}W" -f $i); h = H97Prop $h ("Workspace{0}H" -f $i)
            }
        }
    }
    return $null
}
function H97Stack($h) {
    $rows = @()
    for ($i = 0; $i -lt (H97Prop $h 'WorkspaceCount'); $i++) {
        $rows += [pscustomobject]@{
            id = H97Prop $h ("Workspace{0}Id" -f $i)
            x = H97Prop $h ("Workspace{0}X" -f $i); y = H97Prop $h ("Workspace{0}Y" -f $i)
            w = H97Prop $h ("Workspace{0}W" -f $i); h = H97Prop $h ("Workspace{0}H" -f $i)
        }
    }
    return $rows
}
# Wait for the overview host, then until the camera zoom stops moving. Returns the samples of
# (ms, height) changes so the p_ keys can report the animation's publish cadence.
# The overview request is asynchronous: the IPC reply arrives before the main loop opens the
# host window, so a host that is not there yet must be *waited for* (bounded by limitMs, as
# before); it used to break out at once and hand every gate a null handle.
function H97Settle([int]$limitMs = 6000) {
    $samples = New-Object System.Collections.ArrayList
    $watch = [Diagnostics.Stopwatch]::StartNew()
    $lastH = [int64]::MinValue; $same = 0; $hostHandle = [IntPtr]::Zero
    while ($watch.ElapsedMilliseconds -lt $limitMs) {
        $hostHandle = H97Host
        if ($hostHandle -eq [IntPtr]::Zero) { $hostHandle = [IntPtr]::Zero; Start-Sleep -Milliseconds 20; continue }
        $active = H97Active $hostHandle
        if ($null -ne $active -and $active.h -gt 0) {
            if ($active.h -ne $lastH) {
                [void]$samples.Add(@($watch.ElapsedMilliseconds, $active.h))
                $lastH = $active.h; $same = 0
            } else { $same++ }
            if ($same -ge 4) { break }
        }
        Start-Sleep -Milliseconds 20
    }
    return @{ ms = $watch.ElapsedMilliseconds; samples = $samples; hwnd = $hostHandle }
}
function H97WaitClosed([int]$limitMs = 6000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $limitMs) {
        if ((H97Host) -eq [IntPtr]::Zero) { return $watch.ElapsedMilliseconds }
        Start-Sleep -Milliseconds 20
    }
    return -1
}
function H97Rect($handle) {
    $r = New-Object H97Native+RECT
    $null = [H97Native]::GetWindowRect($handle, [ref]$r)
    return $r
}
$h97Forms = @(); $h97Daemon = $null; $h97Checks = [ordered]@{}
$h97Dir = Join-Path $D 'lane-overview'
$null = New-Item -ItemType Directory -Path $h97Dir -Force
$h97Saved = $null
if (Test-Path $cfg) { $h97Saved = [IO.File]::ReadAllBytes($cfg) }
# KDL needs one child per line: a whole `layout { a { }; b { }; }` on one line is rejected.
$h97Base = @'
input {
    mod-key "ctrl-alt"
}
layout {
    focus-ring { off; }
    border { off; }
}
window-rule {
    match title="^h97-"
    default-column-width { fixed 350; }
}
'@
try {
    if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count) { throw 'H97 refuses an existing daemon' }
    $h97Daemon = Start-WiriWithConfig $h97Base (Join-Path $h97Dir 'wiri.stderr.log')
    # 30 ordinary owned windows over 4 workspaces: 8 + 8 + 8 + 6. The first workspace keeps
    # eight columns so its strip is far wider than the screen.
    $h97Plan = @(1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4)
    for ($i = 0; $i -lt $h97Plan.Count; $i++) {
        $h97Forms += [TestForms]::Open(('h97-{0:d2}' -f $i), (40 + $i % 20) * 6 % 240, (60 + $i * 7) % 240, (90 + $i * 11) % 240)
        Start-Sleep -Milliseconds 120
    }
    Start-Sleep -Milliseconds 3000
    # Workspace numbers are 1-based (niri's `move-to-workspace 2` is the second workspace).
    for ($i = 1; $i -lt $h97Plan.Count; $i++) {
        if ($h97Plan[$i] -eq 1) { continue }
        $null = Ctl "h97-focus-$i" @('focus-window', [string][int64]$h97Forms[$i])
        Start-Sleep -Milliseconds 120
        $null = Ctl "h97-move-$i" @('move-window-to-workspace', [string]$h97Plan[$i])
        Start-Sleep -Milliseconds 120
    }
    Start-Sleep -Milliseconds 1200
    $null = Ctl 'h97-workspace-1' @('switch-workspace', '1')
    Start-Sleep -Milliseconds 700
    FocusForm $h97Forms[0]
    $null = WaitForeground $h97Forms[0] 100 3000
    $null = Ctl 'h97-focus-first' @('focus-window', [string][int64]$h97Forms[0])
    Start-Sleep -Milliseconds 900
    $work = [Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    $h97Tile = 350

    # ---- gate group A: fixed zoom from config, niri's stack geometry ----
    try {
        $normalRect = H97Rect $h97Forms[0]
        # One expression per element: `a, $b - $c, $d` parses as `a, ($b - ($c, $d))` in
        # PowerShell (the comma binds tighter than the arithmetic), which throws on Object[].
        $h97Checks.normal = @($normalRect.L, $normalRect.T, ($normalRect.R - $normalRect.L), ($normalRect.B - $normalRect.T))
        $null = Ctl 'h97-open' @('action', 'toggle-overview')
        $settled = H97Settle
        $hostHwnd = $settled.hwnd
        $active = H97Active $hostHwnd
        $rects = H97Stack $hostHwnd
        $focusW = H97Prop $hostHwnd 'FocusW'; $focusH = H97Prop $hostHwnd 'FocusH'
        $h97Checks.stack = $rects
        $h97Checks.focus = @($focusW, $focusH)
        # The configured `default-column-width { fixed 350 }` is drawn at exactly
        # zoom*tile: 175 px at the default 0.5 (the old fit-everything camera used ~0.19
        # for five stacked workspaces, i.e. 66 px).
        $expectedTile = [math]::Round($h97Tile * 0.5)
        $h97Checks.expected = @($expectedTile, [math]::Ceiling($work.Height * 0.5))
        $summary.h97_thumbnails_are_the_configured_zoom = (
            ($hostHwnd -ne [IntPtr]::Zero) -and
            ($null -ne $active) -and
            ([math]::Abs($active.h - [math]::Ceiling($work.Height * 0.5)) -le 1) -and
            ([math]::Abs($focusW - $expectedTile) -le 1) -and
            ($focusW -gt 120))
        # niri centres the focused workspace in the view: (view.h - ws.h)/2 above it.
        $summary.h97_active_workspace_is_centred = (
            ($null -ne $active) -and
            ([math]::Abs(($active.y + $active.h / 2) - ($work.Top + $work.Height / 2)) -le 2))
        # The gap between workspaces is 10% of the view height, times the zoom.
        $gap = [math]::Round($work.Height * 0.1 * 0.5)
        $strides = @()
        for ($i = 1; $i -lt $rects.Count; $i++) { $strides += ($rects[$i].y - $rects[$i - 1].y) }
        $expectStride = $active.h + $gap
        $h97Checks.stride = @($expectStride, $strides)
        $summary.h97_workspace_gap_and_stack = (
            ($rects.Count -ge 4) -and
            (@($strides | Where-Object { [math]::Abs($_ - $expectStride) -gt 1 }).Count -eq 0) -and
            (@($rects | Where-Object { $_.h -ne $active.h }).Count -eq 0) -and
            (@($rects | Where-Object { $_.w -ne $active.w }).Count -eq 0))
        Shot 'h97-overview-stack'
    } catch { $h97Checks.groupA = $_ | Out-String }

    # ---- gate group B: keyboard focus pans the camera in x and y ----
    try {
        $hostHwnd = H97Host
        $ws = H97Active $hostHwnd
        $wsX = $ws.x
        $h97Samples = @()
        for ($i = 0; $i -lt 7; $i++) {
            $null = Ctl "h97-right-$i" @('action', 'focus-column-right')
            # The strip camera animates (`animations.view`); let it finish.
            Start-Sleep -Milliseconds 700
            $hostHwnd = H97Host
            $h97Samples += [pscustomobject]@{ x = H97Prop $hostHwnd 'FocusX'; w = H97Prop $hostHwnd 'FocusW'; ws = (H97Active $hostHwnd).x }
        }
        $h97Checks.panX = @($h97Samples | ForEach-Object { $_.x })
        # The camera moves inside a fixed workspace frame: the workspace rectangle never
        # changes, the focused tile keeps the zoomed width, and its place inside the
        # workspace changes on most steps (the strip is panned, not fitted).
        $moved = 0; $previous = $h97Samples[0].x
        for ($i = 1; $i -lt $h97Samples.Count; $i++) { if ($h97Samples[$i].x -ne $previous) { $moved++ }; $previous = $h97Samples[$i].x }
        $summary.h97_focus_column_right_pans_camera = (
            ($hostHwnd -ne [IntPtr]::Zero) -and
            (@($h97Samples | Where-Object { $_.ws -ne $wsX }).Count -eq 0) -and
            (@($h97Samples | Where-Object { [math]::Abs($_.w - [math]::Round($h97Tile * 0.5)) -gt 1 }).Count -eq 0) -and
            ($moved -ge 2))
        # Vertical: a workspace change moves the whole stack up by exactly one stride and
        # the camera follows the new focused workspace (niri's `workspace_render_idx`).
        $before = @{}
        $hostHwnd = H97Host
        foreach ($row in (H97Stack $hostHwnd)) { $before[[string]$row.id] = $row.y }
        $beforeActive = H97Active $hostHwnd
        $focusedBefore = H97Prop $hostHwnd 'FocusedWindow'
        $null = Ctl 'h97-down' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 1500
        $hostHwnd = H97Host
        $afterActive = H97Active $hostHwnd
        $stride = $beforeActive.h + [math]::Round($work.Height * 0.1 * 0.5)
        $deltas = @{}
        foreach ($row in (H97Stack $hostHwnd)) {
            if ($before.ContainsKey([string]$row.id)) { $deltas[[string]$row.id] = $row.y - $before[[string]$row.id] }
        }
        $h97Checks.panY = @($stride, $deltas)
        $summary.h97_focus_workspace_down_pans_camera = (
            ($hostHwnd -ne [IntPtr]::Zero) -and
            ($null -ne $afterActive) -and ($afterActive.index -eq $beforeActive.index + 1) -and
            ($deltas.Count -ge 3) -and
            (@($deltas.Values | Where-Object { [math]::Abs($_ + $stride) -gt 1 }).Count -eq 0) -and
            ((H97Prop $hostHwnd 'FocusedWindow') -ne $focusedBefore))
        Shot 'h97-overview-panned'
    } catch { $h97Checks.groupB = $_ | Out-String }

    # ---- gate group C: overview { zoom 0.3 } after a reload changes the scale ----
    try {
        [IO.File]::WriteAllText($cfg, ($h97Base + "`r`noverview {`r`n    zoom 0.3`r`n}`r`n"), (New-Object Text.UTF8Encoding($false)))
        $reloaded = CtlRaw @('reload-config')
        Start-Sleep -Milliseconds 900
        $settled = H97Settle 6000
        $hostHwnd = $settled.hwnd
        $active = H97Active $hostHwnd
        $focusW = H97Prop $hostHwnd 'FocusW'
        $h97Checks.reload = @($active.h, [math]::Ceiling($work.Height * 0.3), $focusW, [math]::Round($h97Tile * 0.3), $reloaded.text.Trim())
        $summary.h97_zoom_config_after_reload = (
            ($reloaded.rc -eq 0) -and
            ($hostHwnd -ne [IntPtr]::Zero) -and
            ([math]::Abs($active.h - [math]::Ceiling($work.Height * 0.3)) -le 1) -and
            ([math]::Abs($focusW - [math]::Round($h97Tile * 0.3)) -le 1))
        Shot 'h97-overview-zoom-03'
    } catch { $h97Checks.groupC = $_ | Out-String }

    # ---- gate group D: clicking a panned thumbnail selects its window ----
    try {
        $hostHwnd = H97Host
        $selected = H97Prop $hostHwnd 'FocusedWindow'
        $fx = H97Prop $hostHwnd 'FocusX'; $fy = H97Prop $hostHwnd 'FocusY'
        $fw = H97Prop $hostHwnd 'FocusW'; $fh = H97Prop $hostHwnd 'FocusH'
        $hostRect = H97Rect $hostHwnd
        $clickX = [int]($fx + $fw / 2 - $hostRect.L)
        $clickY = [int]($fy + $fh / 2 - $hostRect.T)
        $point = [IntPtr](($clickY -shl 16) -bor ($clickX -band 0xffff))
        $null = [H97Native]::PostMessage($hostHwnd, 0x201, [IntPtr]1, $point)
        $null = [H97Native]::PostMessage($hostHwnd, 0x202, [IntPtr]0, $point)
        $closedIn = H97WaitClosed 5000
        Start-Sleep -Milliseconds 900
        $h97Checks.click = @($selected, [int64][LT]::GetForegroundWindow(), $closedIn, $fx, $fy, $fw, $fh)
        $summary.h97_click_on_panned_thumbnail_selects = (
            ($selected -gt 0) -and ($closedIn -ge 0) -and
            ([int64][LT]::GetForegroundWindow() -eq $selected))
    } catch { $h97Checks.groupD = $_ | Out-String }

    # ---- information only: open/close latency, camera publish cadence, CPU, thumbnails ----
    try {
        if ((H97Host) -ne [IntPtr]::Zero) { $null = Ctl 'h97-info-close-first' @('action', 'close-overview'); $null = H97WaitClosed 4000 }
        if ($null -ne $h97Daemon) { $h97Daemon.Refresh() }
        $cpuBefore = if ($null -ne $h97Daemon) { $h97Daemon.TotalProcessorTime.TotalMilliseconds } else { 0 }
        $null = Ctl 'h97-info-open' @('action', 'toggle-overview')
        $settled = H97Settle 8000
        $summary.p_overview_open_ms = [int]$settled.ms
        $intervals = @()
        $previous = $null
        foreach ($sample in $settled.samples) {
            if ($null -ne $previous) { $intervals += ($sample[0] - $previous) }
            $previous = $sample[0]
        }
        if ($intervals.Count -gt 0) {
            $summary.p_overview_publish_interval_ms_mean = [int][math]::Round(($intervals | Measure-Object -Average).Average)
            $summary.p_overview_publish_interval_ms_max = [int](($intervals | Measure-Object -Maximum).Maximum)
        }
        $summary.p_overview_thumbnails = [int](H97Prop (H97Host) 'ThumbnailCount')
        $null = Ctl 'h97-info-close' @('action', 'close-overview')
        $summary.p_overview_close_ms = [int](H97WaitClosed 8000)
        if ($null -ne $h97Daemon) {
            $h97Daemon.Refresh()
            $summary.p_overview_cpu_ms = [int]($h97Daemon.TotalProcessorTime.TotalMilliseconds - $cpuBefore)
        }
    } catch { $h97Checks.info = $_ | Out-String }
} catch { $h97Checks.error = $_ | Out-String } finally {
    $summary.h97_overview_geometry_cleans_up = $false
    if ($null -ne $h97Daemon) {
        Stop-WiriGracefully $h97Daemon 'h97'
        $summary.h97_overview_geometry_cleans_up = (
            $h97Daemon.HasExited -and ((H97Host) -eq [IntPtr]::Zero) -and
            (-not (Get-Process -Id $h97Daemon.Id -ErrorAction SilentlyContinue)))
    }
    foreach ($form in $h97Forms) { if ($form -ne [IntPtr]::Zero -and [LT]::IsWindow($form)) { [TestForms]::CloseForm($form) } }
    if ($null -ne $h97Saved) { [IO.File]::WriteAllBytes($cfg, $h97Saved) }
    $h97Checks | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $h97Dir 'checks.json')
}
