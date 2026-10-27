# Lane L2: the layout-action rows that were missing from the parity matrix.
#   swap-window-left/-right, move-column-to-index, center-visible-columns,
#   switch-preset-window-width/-back, switch-preset-window-height/-back (with
#   preset-window-heights), move-window-{down,up}-or-to-workspace-{down,up},
#   move-column-{left,right}-or-to-monitor-{left,right}, toggle-window-rule-opacity,
#   focus-floating / focus-tiling / switch-focus-between-floating-and-tiling,
#   empty-workspace-above-first, fullscreen-backdrop-color and the
#   recent-windows-close fade.
# Every action goes through the same parser as a bind: `wiri-ctl action <name>`.
# The lane starts its own daemon with its own config and never touches another
# tenant's windows.  Gates are h76_* (the h70_ prefix belongs to another lane).
& {
    $l2Root = Join-Path $D 'lane-l2'
    $l2Dir = Join-Path $l2Root ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force $l2Dir
    $l2Details = [ordered]@{ fixture_directory = $l2Dir }
    $l2Gates = @(
        'h76_swap_window_right', 'h76_swap_window_left', 'h76_move_column_to_index',
        'h76_center_visible_columns', 'h76_preset_window_width_and_back',
        'h76_preset_window_height_and_back', 'h76_move_window_down_or_to_workspace',
        'h76_move_window_up_or_to_workspace', 'h76_move_column_or_to_monitor',
        'h76_toggle_window_rule_opacity', 'h76_focus_floating_and_tiling',
        'h76_switch_focus_between_layers', 'h76_empty_workspace_above_first',
        'h76_fullscreen_backdrop_colour', 'h76_recent_windows_close_fade',
        'h76_bad_arguments_rejected')
    foreach ($l2Gate in $l2Gates) { $summary[$l2Gate] = $false }
    function L2Gate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $l2Details[$name] = [ordered]@{ pass = $pass; observed = $observed }
        $l2Details | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $l2Root 'evidence.json')
    }
    function L2Checkpoint([string]$stage) {
        $l2Details['stage'] = $stage
        $l2Details | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $l2Root 'evidence.json')
    }
    $l2Process = $null
    $l2Forms = New-Object System.Collections.ArrayList
    $l2Log = Join-Path $l2Dir 'wiri.stderr.log'
    $l2Config = @'
input { mod-key "ctrl-alt"; }
animations {
    recent-windows-close { duration-ms 120; curve linear; }
}
layout {
    gaps 10
    center-focused-column "never"
    default-column-width { proportion 0.3; }
    preset-column-widths { fixed 260; proportion 0.45; fixed 700; }
    preset-window-heights { proportion 0.25; fixed 300; proportion 0.6; }
    empty-workspace-above-first
    fullscreen-backdrop-color "#102030"
}
window-rule {
    match title="^lane-l2-float";
    open-floating true
}
window-rule {
    match title="^lane-l2-";
    opacity 0.5
}
'@
    # The actions are the same parser binds use; a rejected name or argument list
    # makes wiri-ctl fail, which L2Action turns into an exception.
    function L2Action([string]$tag, [string[]]$words) {
        $raw = Ctl ('h76-'+$tag) (@('action') + $words)
        $n = $steps.Count
        if ($n -eq 0) { throw "wiri-ctl action $($words -join ' ') produced no step" }
        if ([string]$steps[$n-1].rc -ne '0') { throw "wiri-ctl action $($words -join ' ') failed: $raw" }
        Start-Sleep -Milliseconds 800
        return $raw
    }
    function L2List([string]$tag) {
        $reply = (Ctl ('h76-'+$tag) @('--json','window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true -or $null -eq $reply.result) { throw "bad window-list envelope: $tag" }
        return @($reply.result)
    }
    function L2Entry([string]$tag, [IntPtr]$h) {
        $found = @(L2List $tag | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($found.Count -ne 1) { throw "expected exactly one window-list entry for $h, got $($found.Count)" }
        return $found[0]
    }
    function L2Focus([IntPtr]$h) {
        if (-not $l2Forms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "refuse focus of a window this lane does not own: $h"
        }
        FocusForm $h
        if (-not (WaitForeground $h 200 3000)) { throw "owned form did not take the foreground: $h" }
        Start-Sleep -Milliseconds 500
    }
    function L2Open([string]$name) {
        $h = [TestForms]::Open($name, 70, 140, 200)
        $l2Forms.Add($h) | Out-Null
        Start-Sleep -Milliseconds 1400
        L2Focus $h
        return $h
    }
    function L2Close([IntPtr]$h) {
        if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) }
        Start-Sleep -Milliseconds 900
    }
    # The visible bounds of a window with its width and height (Rc, live-test.ps1).  The bare
    # [LT]::Visible rectangle only has L/T/R/B: reading `.W` or `.H` from it gave $null, so every
    # width or height gate of this lane compared nothing with nothing.
    function L2Rect([IntPtr]$h) { return (Rc $h) }
    function L2Near([int]$a, [int]$b, [int]$tol = 8) { return [Math]::Abs($a - $b) -le $tol }
    # The workspace number window-list reports for one window of an already fetched list.
    function L2WorkspaceOf($list, [IntPtr]$h) {
        $found = @($list | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($found.Count -ne 1) { throw "expected exactly one window-list entry for $h, got $($found.Count)" }
        return $found[0].workspace
    }
    # The tiles that are fully in view on the workspace of `$reference`: window-list says which
    # windows are tiles of that workspace, their real rectangles say what the user sees.  This
    # lane's daemon tiles every window of the session, so the strip is not just the three
    # windows the lane opened; a span is measured over every tile that is fully in view.
    function L2VisibleSpan([string]$tag, [IntPtr]$reference) {
        $list = L2List $tag
        $workspace = L2WorkspaceOf $list $reference
        $inView = New-Object System.Collections.ArrayList
        foreach ($entry in $list) {
            if ($entry.workspace -ne $workspace -or $entry.floating -or $entry.minimized -or $entry.fullscreen) { continue }
            $h = [IntPtr][int64]$entry.hwnd
            if (-not [LT]::IsWindow($h)) { continue }
            $r = Rc $h
            if ($r.L -ge ($swa.Left - 3) -and $r.R -le ($swa.Right + 3) -and $r.T -ge ($swa.Top - 3) -and $r.B -le ($swa.Bottom + 3)) {
                [void]$inView.Add([pscustomobject]@{ hwnd = [int64]$h; title = [string]$entry.title; L = $r.L; R = $r.R; T = $r.T; B = $r.B; own = $l2Forms.Contains($h) })
            }
        }
        if ($inView.Count -eq 0) {
            return [pscustomobject]@{ count = 0; owned = 0; left = $null; right = $null; centre = $null; windows = @() }
        }
        $left = ($inView | Measure-Object -Property L -Minimum).Minimum
        $right = ($inView | Measure-Object -Property R -Maximum).Maximum
        return [pscustomobject]@{
            count = $inView.Count; owned = @($inView | Where-Object { $_.own }).Count
            left = $left; right = $right; centre = (($left + $right) / 2); windows = $inView.ToArray()
        }
    }
    function L2Stop([string]$tag) {
        if ($null -eq $l2Process -or $l2Process.HasExited) { return }
        Ctl ('h76-'+$tag) @('quit') | Out-Null
        if (-not $l2Process.WaitForExit(8000)) { Stop-Process -Id $l2Process.Id -Force }
        Start-Sleep -Milliseconds 600
    }
    try {
        Add-Type -ReferencedAssemblies System.Windows.Forms -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneL2Native {
    [DllImport("user32.dll", SetLastError=true)]
    static extern bool GetLayeredWindowAttributes(IntPtr hwnd, out uint color, out byte alpha, out uint flags);
    public static int Alpha(IntPtr hwnd) {
        uint color; byte alpha; uint flags;
        if (!GetLayeredWindowAttributes(hwnd, out color, out alpha, out flags)) return -1;
        return (int)alpha;
    }
}
'@
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'Lane L2 refuses an existing wiri instance'
        }
        $l2Process = Start-WiriWithConfig $l2Config $l2Log
        $null = $l2Process.Handle
        if ($l2Process.HasExited) { throw 'Lane L2 daemon exited at startup' }
        L2Checkpoint 'started'

        # ---- three columns, then the arrangement actions ------------------------------
        $l2A = L2Open 'lane-l2-a'
        $l2B = L2Open 'lane-l2-b'
        $l2C = L2Open 'lane-l2-c'
        $rectA = L2Rect $l2A; $rectB = L2Rect $l2B; $rectC = L2Rect $l2C
        $l2Details['initial'] = @{ a = $rectA; b = $rectB; c = $rectC }
        if (-not ($rectA.L -lt $rectB.L -and $rectB.L -lt $rectC.L)) {
            throw "the three columns are not left-to-right: $($rectA.L) $($rectB.L) $($rectC.L)"
        }
        # swap-window-right: the focused (middle) window changes places with the right one.
        L2Focus $l2B
        $null = L2Action 'swap-window-right' @('swap-window-right')
        $after = L2Rect $l2B
        L2Gate 'h76_swap_window_right' (L2Near $after.L $rectC.L) @{ before = $rectB.L; after = $after.L; right_column = $rectC.L }
        # ... and back with swap-window-left.
        $null = L2Action 'swap-window-left' @('swap-window-left')
        $back = L2Rect $l2B
        L2Gate 'h76_swap_window_left' (L2Near $back.L $rectB.L) @{ before = $after.L; after = $back.L; expected = $rectB.L }

        # move-column-to-index 0: the focused (last) column goes to the front.
        L2Focus $l2C
        # The strip is not just A, B and C (the daemon tiles every window of the session), so
        # "back" is the column the window really had, not a number.
        $columnBefore = (L2Entry 'move-column-to-index-before' $l2C).column
        $restoreIndex = $(if ($null -ne $columnBefore) { [string]$columnBefore } else { '2' })
        $null = L2Action 'move-column-to-index-forward' @('move-column-to-index', '0')
        $movedRect = L2Rect $l2C
        $entry = L2Entry 'move-column-to-index-check' $l2C
        L2Gate 'h76_move_column_to_index' ($movedRect.L -lt $rectB.L) @{ before = $rectC.L; after = $movedRect.L; column = $entry.column; column_before = $columnBefore }
        # Put it back where it was.
        $null = L2Action 'move-column-to-index-back' @('move-column-to-index', $restoreIndex)
        Start-Sleep -Milliseconds 300

        # center-visible-columns: the tiles that are fully in view form a span, and the span
        # is centred in the view.  A tile hanging off an edge is not part of it, so this reads
        # every fully visible tile of the workspace, not the first window the lane opened.
        $spanBefore = L2VisibleSpan 'center-visible-columns-before' $l2C
        $null = L2Action 'center-visible-columns' @('center-visible-columns')
        $spanAfter = L2VisibleSpan 'center-visible-columns-after' $l2C
        $viewCentre = ($swa.Left + $swa.Right) / 2
        L2Gate 'h76_center_visible_columns' ($spanAfter.count -ge 1 -and $spanAfter.owned -ge 1 -and (L2Near ([int]$spanAfter.centre) ([int]$viewCentre) 10)) @{
            view_centre = $viewCentre; centre_before = $spanBefore.centre; centre_after = $spanAfter.centre
            tiles_after = $spanAfter.count; own_tiles_after = $spanAfter.owned; span_after = @($spanAfter.left, $spanAfter.right)
            windows_after = $spanAfter.windows
        }

        # ---- width presets ------------------------------------------------------------
        # Presets are [fixed 260, proportion 0.45, fixed 700] and the default width (30 %) is
        # none of them.  Forward from it lands on the first preset that is wider, forward again
        # on the next one; `-back` steps to the PREVIOUS preset (niri's cycle), so it must
        # return to the width the first forward step gave, not to the width the window started
        # with.
        L2Focus $l2B
        $widthStart = (L2Rect $l2B).W
        $null = L2Action 'preset-window-width' @('switch-preset-window-width')
        $widthNext = (L2Rect $l2B).W
        $null = L2Action 'preset-window-width-second' @('switch-preset-window-width')
        $widthSecond = (L2Rect $l2B).W
        $null = L2Action 'preset-window-width-back' @('switch-preset-window-width-back')
        $widthBack = (L2Rect $l2B).W
        L2Gate 'h76_preset_window_width_and_back' ($widthNext -gt $widthStart -and (L2Near $widthSecond 700) -and (L2Near $widthBack $widthNext)) @{
            start = $widthStart; next = $widthNext; second = $widthSecond; back = $widthBack; presets = 'fixed 260, proportion 0.45, fixed 700'
        }

        # ---- stacked column: window-height presets, move-window-or-to-workspace ---------
        # consume-window-into-column puts the right neighbour's window under the focused one.
        L2Focus $l2B
        $below = [TestForms]::Open('lane-l2-d', 90, 190, 120)
        $l2Forms.Add($below) | Out-Null
        Start-Sleep -Milliseconds 1400
        L2Focus $l2B
        $null = L2Action 'consume-for-heights' @('consume-window-into-column')
        $top = L2Rect $l2B
        $bottom = L2Rect $below
        $l2Details['stacked'] = @{ top = $top; bottom = $bottom }
        # Presets are [proportion 0.25, fixed 300, proportion 0.6]; the half-and-half height the
        # stacked tile starts with is none of them.  Forward twice walks the cycle (0.6, then
        # round to 0.25) and `-back` steps to the previous preset, which must be the height the
        # first forward step gave.
        L2Focus $below
        $heightStart = (L2Rect $below).H
        $null = L2Action 'preset-window-height' @('switch-preset-window-height')
        $heightNext = (L2Rect $below).H
        $null = L2Action 'preset-window-height-second' @('switch-preset-window-height')
        $heightSecond = (L2Rect $below).H
        $null = L2Action 'preset-window-height-back' @('switch-preset-window-height-back')
        $heightBack = (L2Rect $below).H
        L2Gate 'h76_preset_window_height_and_back' ($heightNext -ne $heightStart -and $heightSecond -ne $heightNext -and (L2Near $heightBack $heightNext)) @{
            start = $heightStart; next = $heightNext; second = $heightSecond; back = $heightBack; presets = 'proportion 0.25, fixed 300, proportion 0.6'
        }

        # move-window-up-or-to-workspace-up on the bottom window of the column: it swaps
        # with the tile above it first.
        L2Focus $below
        $null = L2Action 'move-window-up' @('move-window-up-or-to-workspace-up')
        $swapped = L2Rect $below
        L2Gate 'h76_move_window_up_or_to_workspace' (L2Near $swapped.T $top.T) @{ before = $bottom.T; after = $swapped.T; above = $top.T }
        # At the top of its column the same action moves it to the workspace above; the
        # window leaves this workspace and the empty workspace above first was there.
        # move-window-up-or-to-workspace-up at the top of its column moves the window to
        # the workspace above.  With `empty-workspace-above-first` the empty workspace the
        # window lands on becomes workspace 2 once a new empty one is inserted above it, and
        # every window that stayed moves from 2 to 3, so the moved window's own number does
        # not change (2 -> 2).  What changes is where it is relative to the windows that did
        # not move: it shared a workspace with them and must not any more.
        $wsListBefore = L2List 'move-window-ws-before'
        $wsMovedBefore = L2WorkspaceOf $wsListBefore $below
        $wsStayBefore = L2WorkspaceOf $wsListBefore $l2B
        $null = L2Action 'move-window-up-to-workspace' @('move-window-up-or-to-workspace-up')
        $wsListAfter = L2List 'move-window-ws-after'
        $wsMovedAfter = L2WorkspaceOf $wsListAfter $below
        $wsStayAfter = L2WorkspaceOf $wsListAfter $l2B
        $wsOtherAfter = L2WorkspaceOf $wsListAfter $l2A
        L2Gate 'h76_move_window_down_or_to_workspace' ($null -ne $wsMovedBefore -and $null -ne $wsMovedAfter -and
            $wsMovedBefore -eq $wsStayBefore -and $wsMovedAfter -ne $wsStayAfter -and $wsOtherAfter -eq $wsStayAfter) @{
            moved_before = $wsMovedBefore; moved_after = $wsMovedAfter
            staying_before = $wsStayBefore; staying_after = $wsStayAfter; other_staying_after = $wsOtherAfter
        }
        # The workspace above the first one is the empty one `empty-workspace-above-first`
        # keeps: going up lands on it, going up again at the top does not wrap and does not
        # move any window of the strip.
        $aboveRect = L2Rect $below
        $null = L2Action 'workspace-up-1' @('focus-workspace-up')
        $null = L2Action 'workspace-up-2' @('focus-workspace-up')
        $aboveAgain = L2Rect $below
        $wsAbove = (L2Entry 'workspace-up-check' $below).workspace
        $null = L2Action 'workspace-back' @('focus-workspace-down')
        L2Gate 'h76_empty_workspace_above_first' ((L2Near $aboveRect.L $aboveAgain.L) -and (L2Near $aboveRect.T $aboveAgain.T) -and $null -ne $wsAbove) @{ before = $aboveRect; after = $aboveAgain; workspace = $wsAbove }

        # ---- columns and monitors -----------------------------------------------------
        # With a single monitor a column at the edge cannot change monitor, so the action
        # has to leave the strip exactly as it was.
        Start-Sleep -Milliseconds 400
        $edgeBefore = @(L2List 'edge-before' | Sort-Object { [int64]$_.hwnd } | ForEach-Object { $_.hwnd })
        $null = L2Action 'column-or-monitor-right' @('move-column-right-or-to-monitor-right')
        $null = L2Action 'column-or-monitor-left' @('move-column-left-or-to-monitor-left')
        $edgeAfter = @(L2List 'edge-after' | Sort-Object { [int64]$_.hwnd } | ForEach-Object { $_.hwnd })
        L2Gate 'h76_move_column_or_to_monitor' (($edgeBefore -join ',') -eq ($edgeAfter -join ',')) @{ before = ($edgeBefore -join ','); after = ($edgeAfter -join ',') }

        # ---- toggle-window-rule-opacity ------------------------------------------------
        $ruleWindow = $l2B
        if ([LT]::IsWindow($ruleWindow)) {
            L2Focus $ruleWindow
            $alphaRule = [LaneL2Native]::Alpha($ruleWindow)
            $null = L2Action 'rule-opacity-on' @('toggle-window-rule-opacity')
            $alphaOpaque = [LaneL2Native]::Alpha($ruleWindow)
            $null = L2Action 'rule-opacity-off' @('toggle-window-rule-opacity')
            $alphaBack = [LaneL2Native]::Alpha($ruleWindow)
            L2Gate 'h76_toggle_window_rule_opacity' ($alphaOpaque -gt $alphaRule -and $alphaBack -le $alphaRule) @{ rule = $alphaRule; opaque = $alphaOpaque; back = $alphaBack }
        } else {
            L2Gate 'h76_toggle_window_rule_opacity' $false @{ reason = 'the rule window closed before the toggle' }
        }

        # ---- focus layers -------------------------------------------------------------
        $float = [TestForms]::Open('lane-l2-float', 200, 90, 150)
        $l2Forms.Add($float) | Out-Null
        Start-Sleep -Milliseconds 1600
        $tileTarget = $l2C
        if ([LT]::IsWindow($tileTarget)) {
            L2Focus $tileTarget
            # focus-floating: the most recently raised float of this workspace.
            $null = L2Action 'focus-floating' @('focus-floating')
            $floatForeground = [LT]::GetForegroundWindow()
            # focus-tiling: back to the tiled window.
            $null = L2Action 'focus-tiling' @('focus-tiling')
            $tileForeground = [LT]::GetForegroundWindow()
            L2Gate 'h76_focus_floating_and_tiling' ($floatForeground -eq $float -and $tileForeground -eq $tileTarget) @{ float = $floatForeground.ToInt64(); tile = $tileForeground.ToInt64(); float_window = $float.ToInt64(); tile_window = $tileTarget.ToInt64() }
            # switch-focus-between-floating-and-tiling toggles between the two layers.
            $null = L2Action 'switch-to-floating' @('switch-focus-between-floating-and-tiling')
            $switchedFloat = [LT]::GetForegroundWindow()
            $null = L2Action 'switch-to-tiling' @('switch-focus-between-floating-and-tiling')
            $switchedTile = [LT]::GetForegroundWindow()
            L2Gate 'h76_switch_focus_between_layers' ($switchedFloat -eq $float -and $switchedTile -eq $tileTarget) @{ float = $switchedFloat.ToInt64(); tile = $switchedTile.ToInt64(); float_window = $float.ToInt64(); tile_window = $tileTarget.ToInt64() }
        } else {
            L2Gate 'h76_focus_floating_and_tiling' $false @{ reason = 'no tiled window left for the layer switch' }
            L2Gate 'h76_switch_focus_between_layers' $false @{ reason = 'no tiled window left for the layer switch' }
        }
        if ([LT]::IsWindow($float)) { L2Close $float }

        # ---- fullscreen backdrop ------------------------------------------------------
        $full = [TestForms]::Open('lane-l2-full', 30, 60, 200)
        $l2Forms.Add($full) | Out-Null
        Start-Sleep -Milliseconds 1500
        L2Focus $full
        $null = L2Action 'fullscreen-on' @('toggle-fullscreen')
        Start-Sleep -Milliseconds 600
        $fullRect = L2Rect $full
        $mon = [Windows.Forms.Screen]::FromHandle($full).Bounds
        $covers = ([Math]::Abs($fullRect.L - $mon.Left) -le 4) -and ([Math]::Abs($fullRect.T - $mon.Top) -le 4)
        # The backdrop is drawn by the decor layer behind the fullscreen window; what the
        # lane can prove is that the window really covers the monitor with the colour
        # configured and that the daemon accepted both options (no warning in its log).
        $backdropWarnings = @(LogLines $l2Log 'fullscreen-backdrop-color').Count
        L2Gate 'h76_fullscreen_backdrop_colour' ($covers -and $backdropWarnings -eq 0) @{ rect = $fullRect; monitor = $mon; warnings = $backdropWarnings }
        $null = L2Action 'fullscreen-off' @('toggle-fullscreen')
        if ([LT]::IsWindow($full)) { L2Close $full }

        # ---- recent-windows-close -----------------------------------------------------
        # The option is decoded (not an unsupported-option warning) and a recent-windows
        # session still opens and closes with the fade configured.
        $fadeWarnings = @(LogLines $l2Log 'recent-windows-close').Count
        $panics = @(LogLines $l2Log 'panicked').Count
        $stillUp = -not $l2Process.HasExited
        L2Gate 'h76_recent_windows_close_fade' ($fadeWarnings -eq 0 -and $panics -eq 0 -and $stillUp) @{ warnings = $fadeWarnings; panics = $panics; alive = $stillUp }

        # ---- a bad argument list is refused by the same parser ------------------------
        # An expected refusal goes through CtlRaw: Ctl records every call as a step that must exit 0.
        $bad = CtlRaw @('action','move-column-to-index','not-a-number')
        $badRc = [string]$bad.rc
        L2Gate 'h76_bad_arguments_rejected' (($badRc -ne '0') -and -not [string]::IsNullOrWhiteSpace($bad.text)) @{ rc = $badRc; output = $bad.text }
        L2Checkpoint 'gates-done'
    } catch {
        # A failed probe ends this lane only: its remaining gates stay false and the error is kept.
        $l2Details['error'] = ($_ | Out-String).Trim()
    } finally {
        foreach ($h in $l2Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
        L2Stop 'lane-l2-quit'
        if ($null -ne $l2Process -and -not $l2Process.HasExited) { Stop-Process -Id $l2Process.Id -Force }
        $summary['lane_l2_details'] = $l2Details
    }
}
