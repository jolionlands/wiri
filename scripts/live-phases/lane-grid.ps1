# Lane GRID (H98): the wiri `layout { unified-grid }` extension.
#
# One owned wiri instance per group with a temporary config of our own, owned WinForms
# windows in 3 workspaces x 4 columns of varied widths, and assertions on the rectangles
# the tiles actually get.  Each gate group has its own try/catch; nothing outside the owned
# fixture forms and the wiri instance started here is touched.  No `$d` or `$pid`: the
# harness owns those names.
$h98Keys = @(
    'h98_config_accepts_unified_grid',
    'h98_vertical_move_keeps_x_and_picks_the_column_under_it',
    'h98_move_to_named_workspace_lands_on_the_x_matched_column',
    'h98_focus_up_right_lands_on_the_diagonal_target',
    'h98_focus_down_left_lands_on_the_diagonal_target',
    'h98_move_window_up_right_moves_the_window_to_the_target_row',
    'h98_two_arrows_together_land_on_the_diagonal_target',
    'h98_without_the_flag_behaviour_is_niris',
    'h98_overview_rows_share_one_camera',
    'h98_overview_diagonal_focus_pans_the_camera'
)
foreach ($key in $h98Keys) { $summary[$key] = $false }

# Four columns of varied widths per row: 320, 520, 400, 620 px.  `column-width` is the
# fallback for a column with no explicit width, so every column is given one by the fixture.
function H98Config([bool]$unified, [string]$binds) {
    $flag = if ($unified) { 'unified-grid' } else { '' }
    $text = "layout {`n    gaps 8`n    unified-grid-chord-ms 150`n    $flag`n}`n"
    $text += "animations { window-movement { duration-ms 100; curve `"linear`"; }; workspace-switch { duration-ms 100; curve `"linear`"; }; }`n"
    $text += "binds {`n$binds`n}`n"
    return $text
}

# The visible (on-screen) columns of the active workspace: a parked column sits below the
# monitor, so "inside the work area" is the observable that says which row is showing.
# `Rc` reports L/T/R/B/W/H (the harness helper's fields); a plain `.X`/`.Y` read silently
# yielded $null, which made every positional comparison below vacuously true.
function H98Visible($forms, $work) {
    $visible = @()
    foreach ($form in $forms) {
        $rect = Rc $form
        if ($rect.W -le 0) { continue }
        if (($rect.T -lt ($work.Y + $work.H - 8)) -and ($rect.T -ge ($work.Y - 8))) {
            $visible += [pscustomobject]@{ hwnd = $form; L = $rect.L; T = $rect.T; W = $rect.W; H = $rect.H }
        }
    }
    return $visible
}

function H98Work($ignored) {
    # The tiles of the active workspace sit inside the primary screen's working area; a
    # parked column is moved below it by the layout.
    $area = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
    return [pscustomobject]@{ X = $area.X; Y = $area.Y; W = $area.Width; H = $area.Height }
}

# Every fixture as the engine sees it (workspace, column, workspace-view position) next to its
# screen rectangle: a failing geometric gate then shows which row the fixtures really are in.
function H98Dump([string]$tag) {
    $rows = @()
    try {
        $list = @(WinList ("h98-dump-" + $tag))
        $workspaces = @((Ctl ("h98-dumpws-" + $tag) @('--json', 'workspaces') | ConvertFrom-Json).result.workspaces)
        $entries = @()
        foreach ($form in $h98Forms) {
            $entry = @($list | Where-Object { [int64]$_.hwnd -eq [int64]$form }) | Select-Object -First 1
            $rect = Rc $form
            $entries += [ordered]@{
                hwnd = [int64]$form
                workspace = $(if ($null -ne $entry) { $entry.workspace } else { $null })
                column = $(if ($null -ne $entry) { $entry.column } else { $null })
                tile = $(if ($null -ne $entry) { $entry.tile } else { $null })
                floating = $(if ($null -ne $entry) { $entry.floating } else { $null })
                pos = $(if ($null -ne $entry) { @($entry.layout.tile_pos_in_workspace_view) } else { @() })
                size = $(if ($null -ne $entry) { @($entry.layout.tile_size) } else { @() })
                rect = @($rect.L, $rect.T, $rect.W, $rect.H)
            }
        }
        $rows = [ordered]@{ stage = $tag; workspaces = $workspaces; fixtures = $entries }
    } catch {
        $rows = [ordered]@{ stage = $tag; error = ($_ | Out-String).Trim() }
    }
    ($rows | ConvertTo-Json -Depth 8) | Set-Content (Join-Path $D ("h98-dump-$tag.json"))
}

$h98Binds = @'
    Mod+Up { focus-window-or-workspace-up; }
    Mod+Down { focus-window-or-workspace-down; }
    Mod+Right { focus-column-right; }
    Mod+Left { focus-column-left; }
'@

# Ctrl+Alt is wiri's default `Mod`.  Press the modifier chord, then a key, then release
# everything: SendInput only, no timers, no polling loop.
function H98Chord([int[]]$keys) {
    [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
    foreach ($vk in $keys) {
        [LT]::Key($vk, $false)
        Start-Sleep -Milliseconds 40
    }
    $reversed = @($keys)
    [array]::Reverse($reversed)
    foreach ($vk in $reversed) { [LT]::Key($vk, $true) }
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
}

# The unified grid hides and parks rows and columns, so a screen rectangle - or the published
# view position - of a window on an inactive workspace, or of a column parked out of the view,
# is stale.  The movement gates therefore read niri's own window state and rebuild the strip
# themselves: a row's column starts are the running sum of its tiles' `tile_size`, which is the
# space `src/layout/engine/unified_grid.rs` x-matches in (rule 3: a row change lands on the
# column whose strip span holds the focus anchor; rule 4: the diagonal actions take the column
# one right/left of it, clamped to the row).
function H98State {
    $windows = @((Ctl 'h98-windows' @('--json', 'windows') | ConvertFrom-Json).result.windows)
    $workspaces = @((Ctl 'h98-workspaces' @('--json', 'workspaces') | ConvertFrom-Json).result.workspaces)
    return [pscustomobject]@{ windows = $windows; workspaces = $workspaces }
}
function H98Of($state, $handle) {
    return @($state.windows | Where-Object { [int64]$_.id -eq [int64]$handle }) | Select-Object -First 1
}
function H98RowOf($state, $window) {
    if ($null -eq $window) { return -1 }
    $workspace = @($state.workspaces | Where-Object { [int64]$_.id -eq [int64]$window.workspace_id }) | Select-Object -First 1
    if ($null -eq $workspace) { return -1 }
    return [int]$workspace.idx
}
# The window the engine acted on.  `is_focused` is niri's own focus flag; when the OS
# foreground is held by another (non-wiri) window, wiri cannot activate the window and
# reports no focused window at all, so the active row's remembered active window is the
# fallback.
function H98ActiveOf($state) {
    $focused = @($state.windows | Where-Object { $_.is_focused -and ($h98Forms -contains [IntPtr][int64]$_.id) }) | Select-Object -First 1
    if ($null -ne $focused) { return $focused }
    $workspace = @($state.workspaces | Where-Object { $_.is_active }) | Select-Object -First 1
    if ($null -eq $workspace -or $null -eq $workspace.active_window_id) { return $null }
    return H98Of $state ([int64]$workspace.active_window_id)
}
function H98Column($window) {
    if ($null -eq $window -or $null -eq $window.layout.pos_in_scrolling_layout) { return -1 }
    return [int]$window.layout.pos_in_scrolling_layout[0]
}
# One entry per column of a row: its 1-based column number, strip start and tile width.
function H98Strip($state, [int]$row) {
    $strip = @()
    $start = 0.0
    $seen = 0
    $windows = @($state.windows | Where-Object {
        ($null -ne $_.layout.pos_in_scrolling_layout) -and ((H98RowOf $state $_) -eq $row)
    } | Sort-Object { [int]$_.layout.pos_in_scrolling_layout[0] })
    foreach ($window in $windows) {
        $column = H98Column $window
        if ($column -le $seen) { continue }
        $width = [double]$window.layout.tile_size[0]
        $strip += [pscustomobject]@{ Column = $column; Start = $start; Width = $width }
        $start += $width
        $seen = $column
    }
    return $strip
}
# The 0-based index of the row's column whose strip span holds `anchor`, nearest otherwise
# (the engine's x_matched_column tie rule).
function H98Matched($strip, [double]$anchor) {
    $best = -1
    $bestDistance = [double]::MaxValue
    for ($i = 0; $i -lt $strip.Count; $i++) {
        $start = $strip[$i].Start
        $end = $strip[$i].Start + $strip[$i].Width
        $distance = if ($anchor -lt $start) { $start - $anchor } elseif ($anchor -ge $end) { $anchor - $end } else { 0.0 }
        if ($distance -lt $bestDistance) { $bestDistance = $distance; $best = $i }
    }
    return $best
}
# The strip x of the centre of a window's column.
function H98Anchor($state, $window) {
    $column = H98Column $window
    foreach ($entry in (H98Strip $state (H98RowOf $state $window))) {
        if ($entry.Column -eq $column) { return [double]($entry.Start + $entry.Width / 2) }
    }
    return $null
}
# The 1-based column a grid step lands on in `row`: the x-match moved by `offset` columns and
# clamped to the row, exactly as the engine resolves it before anything moves.
function H98ExpectedColumn($state, [int]$row, [double]$anchor, [int]$offset) {
    $strip = @(H98Strip $state $row)
    if ($strip.Count -eq 0) { return -1 }
    $index = (H98Matched $strip $anchor) + $offset
    if ($index -lt 0) { $index = 0 }
    if ($index -gt ($strip.Count - 1)) { $index = $strip.Count - 1 }
    return [int]$strip[$index].Column
}
# The window is on the monitor's view (its active row), not parked above or below it.
function H98OnScreen($handle, $work) {
    $rect = Rc $handle
    return ($rect.W -gt 0) -and ($rect.T -ge ($work.Y - 8)) -and ($rect.T -lt ($work.Y + $work.H - 8))
}
$h98Forms = @()
$h98Proc = $null
try {
    try {
        $h98Proc = Start-WiriWithConfig (H98Config $true $h98Binds) (Join-Path $D 'wiri.stderr98.log')
        # The desktop's own windows (this cycle's console and anything an earlier phase left
        # open) are adopted into the first workspace, so the fixture rows start at workspace 2:
        # a row the lane counts columns in holds only its own four forms, and a vertical move
        # from a fixture row lands on another fixture row.
        $null = Ctl 'h98-workspace' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 600
        # Three workspaces, four columns of varied widths each, on the unity grid.
        for ($rowIndex = 0; $rowIndex -lt 3; $rowIndex++) {
            if ($rowIndex -gt 0) {
                $null = Ctl "h98-mk$rowIndex" @('action', 'focus-workspace-down')
                Start-Sleep -Milliseconds 250
            }
            foreach ($width in @(320, 520, 400, 620)) {
                $h98Forms += [TestForms]::Open("wiri-h98-$rowIndex-$width", 40, 60, 120)
                Start-Sleep -Milliseconds 220
                $null = Ctl "h98-w$rowIndex-$width" @('action', 'set-column-width', "$width")
                Start-Sleep -Milliseconds 120
            }
        }
        $null = Ctl 'h98-home' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 500
        $h98Outputs = Ctl 'h98-outputs' @('list-monitors')
        $summary.h98_config_accepts_unified_grid = ($h98Outputs -match 'unified-grid: on')
        Set-Content (Join-Path $D 'h98-outputs.txt') ($h98Outputs | ConvertTo-Json -Depth 4 -Compress)
        # Which fixture is which row/column, as the engine reports it: a failure then names the
        # window it expected instead of a bare rectangle.
        Set-Content (Join-Path $D 'h98-rows.txt') "$(($h98Forms | ForEach-Object { '{0}:{1}' -f $_.ToInt64(), (Rc $_).L }) -join ' ')"

        H98Dump 'rows-built'
        $h98Work = H98Work $h98Outputs
        # Row 0 is workspace 2, row 1 workspace 3, row 2 workspace 4.  Each row's columns are
        # 320, 520, 400, 620 px wide in that order.
        $h98Top = @{ ws = 2; narrow = $h98Forms[0]; mid = $h98Forms[1]; wide = $h98Forms[2]; last = $h98Forms[3] }
        $h98Row = @{ ws = 3; narrow = $h98Forms[4]; mid = $h98Forms[5]; wide = $h98Forms[6]; last = $h98Forms[7] }
        $h98Low = @{ ws = 4; narrow = $h98Forms[8]; mid = $h98Forms[9]; wide = $h98Forms[10]; last = $h98Forms[11] }
        # Mid row, second column: the 520 px anchor one row above another fixture row.
        $null = Ctl 'h98-col2' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300

        # 1. Rule 3: moving one row down keeps x and lands on the column under the anchor.
        $null = Ctl 'h98-home1' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2a' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98State = H98State
        $h98AnchorEntry = H98Of $h98State $h98Top.mid
        $h98AnchorRow = H98RowOf $h98State $h98AnchorEntry
        $h98AnchorX = H98Anchor $h98State $h98AnchorEntry
        $h98Expected = H98ExpectedColumn $h98State ($h98AnchorRow + 1) $h98AnchorX 0
        $null = Ctl 'h98-down' @('action', 'focus-workspace-down')
        Start-Sleep -Milliseconds 700
        $h98State = H98State
        $h98Active = H98ActiveOf $h98State
        $h98RowIdx = H98RowOf $h98State $h98Active
        $h98Under = ($null -ne $h98Active) -and ($h98Forms -contains [IntPtr][int64]$h98Active.id) -and
            ($h98RowIdx -eq ($h98AnchorRow + 1)) -and
            ((H98Column $h98Active) -eq $h98Expected) -and
            (H98OnScreen ([IntPtr][int64]$h98Active.id) $h98Work)
        $summary.h98_vertical_move_keeps_x_and_picks_the_column_under_it = $h98Under
        Set-Content (Join-Path $D 'h98-vertical.txt') "anchor_row=$h98AnchorRow anchor_x=$h98AnchorX active=$($h98Active.id) row=$h98RowIdx column=$((H98Column $h98Active)) expected=$h98Expected under=$h98Under"

        # 2. An indexed workspace action lands on the x-matched column too.
        $null = Ctl 'h98-ws2' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2d' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98State = H98State
        $h98AnchorEntry = H98Of $h98State $h98Top.mid
        $h98AnchorRow = H98RowOf $h98State $h98AnchorEntry
        $h98AnchorX = H98Anchor $h98State $h98AnchorEntry
        $h98Expected = H98ExpectedColumn $h98State 4 $h98AnchorX 0
        $null = Ctl 'h98-ws4' @('action', 'focus-workspace', '4')
        Start-Sleep -Milliseconds 700
        $h98State = H98State
        $h98Active = H98ActiveOf $h98State
        $h98RowIdx = H98RowOf $h98State $h98Active
        $summary.h98_move_to_named_workspace_lands_on_the_x_matched_column =
            ($null -ne $h98Active) -and ($h98Forms -contains [IntPtr][int64]$h98Active.id) -and
            ($h98RowIdx -eq 4) -and ((H98Column $h98Active) -eq $h98Expected) -and
            (H98OnScreen ([IntPtr][int64]$h98Active.id) $h98Work)
        Set-Content (Join-Path $D 'h98-indexed.txt') "anchor_row=$h98AnchorRow anchor_x=$h98AnchorX row=$h98RowIdx column=$((H98Column $h98Active)) expected=$h98Expected"

        # 3. `focus-up-right`: one row up and one column right, as one motion.
        $null = Ctl 'h98-home2' @('action', 'focus-workspace', '3')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2b' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98State = H98State
        $h98AnchorEntry = H98Of $h98State $h98Row.mid
        $h98AnchorRow = H98RowOf $h98State $h98AnchorEntry
        $h98AnchorX = H98Anchor $h98State $h98AnchorEntry
        $h98Expected = H98ExpectedColumn $h98State ($h98AnchorRow - 1) $h98AnchorX 1
        $null = Ctl 'h98-upright' @('action', 'focus-up-right')
        Start-Sleep -Milliseconds 700
        $h98State = H98State
        $h98Active = H98ActiveOf $h98State
        $h98RowIdx = H98RowOf $h98State $h98Active
        $summary.h98_focus_up_right_lands_on_the_diagonal_target =
            ($null -ne $h98Active) -and ($h98Forms -contains [IntPtr][int64]$h98Active.id) -and
            ($h98RowIdx -eq ($h98AnchorRow - 1)) -and ((H98Column $h98Active) -eq $h98Expected) -and
            (H98OnScreen ([IntPtr][int64]$h98Active.id) $h98Work)

        # 4. `focus-down-left`: one row down and one column left.
        $null = Ctl 'h98-home3' @('action', 'focus-workspace', '3')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col3' @('action', 'focus-column', '3')
        Start-Sleep -Milliseconds 300
        $h98State = H98State
        $h98AnchorEntry = H98Of $h98State $h98Row.wide
        $h98AnchorRow = H98RowOf $h98State $h98AnchorEntry
        $h98AnchorX = H98Anchor $h98State $h98AnchorEntry
        $h98Expected = H98ExpectedColumn $h98State ($h98AnchorRow + 1) $h98AnchorX -1
        $null = Ctl 'h98-downleft' @('action', 'focus-down-left')
        Start-Sleep -Milliseconds 700
        $h98State = H98State
        $h98Active = H98ActiveOf $h98State
        $h98RowIdx = H98RowOf $h98State $h98Active
        $summary.h98_focus_down_left_lands_on_the_diagonal_target =
            ($null -ne $h98Active) -and ($h98Forms -contains [IntPtr][int64]$h98Active.id) -and
            ($h98RowIdx -eq ($h98AnchorRow + 1)) -and ((H98Column $h98Active) -eq $h98Expected) -and
            (H98OnScreen ([IntPtr][int64]$h98Active.id) $h98Work)

        # 5. `move-window-up-right`: the window itself arrives in the row above, one column
        #    right of the column the x-matching rule picks under it.
        $null = Ctl 'h98-home4' @('action', 'focus-workspace', '3')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col1' @('action', 'focus-column', '1')
        Start-Sleep -Milliseconds 300
        $h98Mover = $h98Row.narrow
        $h98State = H98State
        $h98MoverRow = H98RowOf $h98State (H98Of $h98State $h98Mover)
        $h98MoverX = H98Anchor $h98State (H98Of $h98State $h98Mover)
        $h98Expected = H98ExpectedColumn $h98State ($h98MoverRow - 1) $h98MoverX 1
        $null = Ctl 'h98-moveupright' @('action', 'move-window-up-right')
        Start-Sleep -Milliseconds 800
        $h98State = H98State
        $h98MoverAfter = H98Of $h98State $h98Mover
        $h98RowIdx = H98RowOf $h98State $h98MoverAfter
        $summary.h98_move_window_up_right_moves_the_window_to_the_target_row =
            ($null -ne $h98MoverAfter) -and ($h98RowIdx -eq ($h98MoverRow - 1)) -and
            ((H98Column $h98MoverAfter) -eq $h98Expected) -and (H98OnScreen $h98Mover $h98Work)
        Set-Content (Join-Path $D 'h98-move.txt') "from_row=$h98MoverRow x=$h98MoverX expected_col=$h98Expected row_now=$h98RowIdx col_now=$((H98Column $h98MoverAfter))"

        # 6. Rule 5: Mod+Up then Right with Up still held lands on the diagonal target.
        $null = Ctl 'h98-home5' @('action', 'focus-workspace', '3')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2c' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98State = H98State
        $h98AnchorEntry = H98Of $h98State $h98Row.mid
        $h98AnchorRow = H98RowOf $h98State $h98AnchorEntry
        $h98AnchorX = H98Anchor $h98State $h98AnchorEntry
        $h98Expected = H98ExpectedColumn $h98State ($h98AnchorRow - 1) $h98AnchorX 1
        H98Chord @(0x26, 0x27)
        Start-Sleep -Milliseconds 900
        $h98State = H98State
        $h98Active = H98ActiveOf $h98State
        $h98RowIdx = H98RowOf $h98State $h98Active
        $summary.h98_two_arrows_together_land_on_the_diagonal_target =
            ($null -ne $h98Active) -and ($h98Forms -contains [IntPtr][int64]$h98Active.id) -and
            ($h98RowIdx -eq ($h98AnchorRow - 1)) -and ((H98Column $h98Active) -eq $h98Expected) -and
            (H98OnScreen ([IntPtr][int64]$h98Active.id) $h98Work)
        Set-Content (Join-Path $D 'h98-chord.txt') "from_row=$h98AnchorRow anchor_x=$h98AnchorX row_now=$h98RowIdx col_now=$((H98Column $h98Active)) expected=$h98Expected id=$($h98Active.id)"
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h98-unified-error.txt')
    } finally {
        if ($null -ne $h98Proc) { Stop-WiriGracefully $h98Proc 'h98-unified' }
        foreach ($h98Form in $h98Forms) { if ([LT]::IsWindow($h98Form)) { [TestForms]::CloseForm($h98Form) } }
        $h98Forms = @()
        $h98Proc = $null
        Start-Sleep -Milliseconds 400
    }

    # 7. The same fixture without the flag: niri's per-row camera and remembered column.
    try {
        $h98Proc = Start-WiriWithConfig (H98Config $false $h98Binds) (Join-Path $D 'wiri.stderr98b.log')
        for ($rowIndex = 0; $rowIndex -lt 2; $rowIndex++) {
            if ($rowIndex -gt 0) {
                $null = Ctl "h98b-mk$rowIndex" @('action', 'focus-workspace-down')
                Start-Sleep -Milliseconds 250
            }
            foreach ($width in @(320, 520, 400)) {
                $h98Forms += [TestForms]::Open("wiri-h98b-$rowIndex-$width", 40, 60, 120)
                Start-Sleep -Milliseconds 220
                $null = Ctl "h98b-w$rowIndex-$width" @('action', 'set-column-width', "$width")
                Start-Sleep -Milliseconds 120
            }
        }
        $null = Ctl 'h98b-home' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $h98bOutputs = Ctl 'h98b-outputs' @('list-monitors')
        $h98bOff = -not ($h98bOutputs -match 'unified-grid: on')
        # Row 2's remembered column is the last one added (the 400 px column).  A vertical
        # move must land there, not on the column under the anchor.
        $null = Ctl 'h98b-col0' @('action', 'focus-column', '1')
        Start-Sleep -Milliseconds 250
        $h98bAnchor = Rc $h98Forms[0]
        $null = Ctl 'h98b-down' @('action', 'focus-workspace-down')
        Start-Sleep -Milliseconds 700
        $h98bWork = H98Work $h98bOutputs
        $h98bVisible = H98Visible $h98Forms $h98bWork
        $h98bNarrow = @($h98bVisible | Where-Object { [math]::Abs($_.W - 400) -le 6 })
        $summary.h98_without_the_flag_behaviour_is_niris = $h98bOff -and ($h98bNarrow.Count -ge 1)
        Set-Content (Join-Path $D 'h98-off.txt') "off=$h98bOff anchor=$($h98bAnchor.L),$($h98bAnchor.W) visible=$(($h98bVisible | ForEach-Object { "$($_.L),$($_.W)" }) -join ' ')"
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h98-off-error.txt')
    } finally {
        if ($null -ne $h98Proc) { Stop-WiriGracefully $h98Proc 'h98-off' }
        foreach ($h98Form in $h98Forms) { if ([LT]::IsWindow($h98Form)) { [TestForms]::CloseForm($h98Form) } }
        $h98Forms = @()
        $h98Proc = $null
    }
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h98-error.txt')
} finally {
    if ($null -ne $h98Proc) { Stop-WiriGracefully $h98Proc 'h98-final' }
    foreach ($h98Form in $h98Forms) { if ([LT]::IsWindow($h98Form)) { [TestForms]::CloseForm($h98Form) } }
}

# ---- The zoomed-out view: the overview draws the same one shared camera ---------------
#
# With `unified-grid` on, every workspace row of the overview must be drawn through the
# monitor's one camera, so column k of three rows with different column counts sits at the
# same on-screen x, and `focus-up-right` inside the overview has to move the camera in x
# and y together and land on the window diagonally up-and-right.  The overview host
# publishes the per-tile destination rects (`WiriOverviewTileCount`,
# `WiriOverviewTile{i}{Window,X,Y,W,H}`, in draw order) plus the workspace stack, so the
# gates read the geometry the DWM thumbnails really get.
if (-not ('H98Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H98Native {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string name);
}
'@
}
function H98oHost { return [H98Native]::FindWindow('WiriThumbnailOverviewHost', $null) }
function H98oProp($handle, [string]$name) {
    if ($handle -eq [IntPtr]::Zero) { return [int64]-1 }
    return [H98Native]::GetProp($handle, ('WiriOverview' + $name)).ToInt64()
}
# One array per workspace row, each sorted left to right (the host publishes tiles top to
# bottom, then left to right, but a row is what the gate compares).
function H98oRows($handle) {
    $groups = @{}
    $count = H98oProp $handle 'TileCount'
    for ($index = 0; $index -lt $count; $index++) {
        $prefix = 'Tile' + $index
        $y = H98oProp $handle ($prefix + 'Y')
        $key = [string]$y
        if (-not $groups.ContainsKey($key)) { $groups[$key] = New-Object System.Collections.ArrayList }
        [void]$groups[$key].Add([pscustomobject]@{
                window = H98oProp $handle ($prefix + 'Window')
                x = H98oProp $handle ($prefix + 'X')
                y = $y
                w = H98oProp $handle ($prefix + 'W')
            })
    }
    $rows = New-Object System.Collections.ArrayList
    foreach ($key in ($groups.Keys | Sort-Object { [int64]$_ })) {
        [void]$rows.Add(@($groups[$key] | Sort-Object x))
    }
    return , $rows.ToArray()
}
# The y of the row the camera is centred on (the active workspace's stack entry).
function H98oActiveY($handle) {
    $active = H98oProp $handle 'ActiveWorkspace'
    for ($index = 0; $index -lt (H98oProp $handle 'WorkspaceCount'); $index++) {
        if ((H98oProp $handle ('Workspace' + $index + 'Id')) -eq $active) {
            return H98oProp $handle ('Workspace' + $index + 'Y')
        }
    }
    return [int64]::MinValue
}

$h98oConfig = @'
input {
    mod-key "ctrl-alt"
}
layout {
    gaps 8
    unified-grid
    unified-grid-chord-ms 150
    focus-ring { off; }
    border { off; }
}
window-rule {
    match title="^h98o-"
    default-column-width { fixed 350; }
}
'@
$h98oForms = @()
$h98oProc = $null
try {
    try {
        $h98oProc = Start-WiriWithConfig $h98oConfig (Join-Path $D 'wiri.stderr98o.log')
        # Three rows of different column counts (5, 3 and 2) of equal 350 px columns, so
        # column k starts at the same strip x in every row.
        $h98oPlan = @(5, 3, 2)
        for ($h98oRow = 0; $h98oRow -lt $h98oPlan.Count; $h98oRow++) {
            if ($h98oRow -gt 0) {
                $null = Ctl ("h98o-mk{0}" -f $h98oRow) @('action', 'focus-workspace-down')
                Start-Sleep -Milliseconds 300
            }
            for ($h98oCol = 0; $h98oCol -lt $h98oPlan[$h98oRow]; $h98oCol++) {
                $h98oForms += [TestForms]::Open(("h98o-{0}-{1}" -f ($h98oRow + 1), ($h98oCol + 1)), 40, 60, 120)
                Start-Sleep -Milliseconds 240
            }
        }
        # Back to the first row with the camera on its first column, then open the camera.
        $null = Ctl 'h98o-home' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 800
        $h98oOutputs = Ctl 'h98o-outputs' @('list-monitors')
        $h98oGridOn = [bool]($h98oOutputs -match 'unified-grid: on')
        $null = Ctl 'h98o-open' @('action', 'toggle-overview')
        Start-Sleep -Milliseconds 1500
        $h98oHost = H98oHost
        $h98oRows = H98oRows $h98oHost
        Set-Content (Join-Path $D 'h98o-rows.txt') (($h98oRows | ForEach-Object {
                    (@($_ | ForEach-Object { "$($_.x),$($_.y),$($_.w)" }) -join ' ')
                }) -join ' | ')
        # 1. One shared camera: the leftmost thumbnail of every row is at the same x (every
        #    row's first column starts at strip x 0), two rows whose first column has the same
        #    width also agree on the second column's x, and the rows hold different counts, so
        #    this cannot pass on one row.
        $h98oLeftmost = @($h98oRows | ForEach-Object { @($_)[0].x })
        $h98oWidthPairs = @{}
        foreach ($h98oRowTiles in $h98oRows) {
            $h98oRowTiles = @($h98oRowTiles)
            if ($h98oRowTiles.Count -ge 2) {
                $h98oKey = [string]$h98oRowTiles[0].w
                if (-not $h98oWidthPairs.ContainsKey($h98oKey)) {
                    $h98oWidthPairs[$h98oKey] = @($h98oRowTiles[1].x)
                } else {
                    $h98oWidthPairs[$h98oKey] = @($h98oWidthPairs[$h98oKey]) + @($h98oRowTiles[1].x)
                }
            }
        }
        $h98oAgreeing = @($h98oWidthPairs.Values | Where-Object {
                @($_).Count -ge 2 -and (@($_ | Sort-Object -Unique).Count -eq 1)
            })
        $h98oCounts = @($h98oRows | ForEach-Object { @($_).Count })
        $summary.h98_overview_rows_share_one_camera = (
            $h98oGridOn -and ($h98oHost -ne [IntPtr]::Zero) -and
            ($h98oRows.Count -eq 3) -and (@($h98oCounts | Sort-Object -Unique).Count -ge 2) -and
            (@($h98oLeftmost | Sort-Object -Unique).Count -eq 1) -and
            (@($h98oLeftmost)[0] -gt 0) -and ($h98oAgreeing.Count -ge 1))
        Set-Content (Join-Path $D 'h98o-camera.txt') "grid=$h98oGridOn counts=$($h98oCounts -join ',') leftmost=$($h98oLeftmost -join ',') agreeing=$($h98oAgreeing.Count)"

        # 2. `focus-up-right` inside the overview: the row moves up (the camera's y follows the
        #    new active workspace), the shared camera pans right (every row's left edge moves,
        #    by the same amount), and the focus lands on a window of the destination row that is
        #    not its leftmost column.
        $null = Ctl 'h98o-row2' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 1200
        $null = Ctl 'h98o-right1' @('action', 'focus-column-right')
        Start-Sleep -Milliseconds 400
        $null = Ctl 'h98o-right2' @('action', 'focus-column-right')
        Start-Sleep -Milliseconds 900
        $h98oHost = H98oHost
        $h98oActiveBefore = H98oProp $h98oHost 'ActiveWorkspace'
        $h98oYBefore = H98oActiveY $h98oHost
        $h98oFocusBefore = H98oProp $h98oHost 'FocusedWindow'
        $h98oBefore = H98oRows $h98oHost
        $h98oXBefore = @(@($h98oBefore)[0])[0].x
        $null = Ctl 'h98o-upright' @('action', 'focus-up-right')
        Start-Sleep -Milliseconds 1400
        $h98oHost = H98oHost
        $h98oActiveAfter = H98oProp $h98oHost 'ActiveWorkspace'
        $h98oYAfter = H98oActiveY $h98oHost
        $h98oFocusAfter = H98oProp $h98oHost 'FocusedWindow'
        $h98oAfter = H98oRows $h98oHost
        $h98oLefts = @($h98oAfter | ForEach-Object { @($_)[0].x })
        $h98oXAfter = @($h98oLefts)[0]
        $h98oDestRow = @($h98oAfter | Where-Object { @($_)[0].y -eq $h98oYAfter })
        $h98oFocusIndex = -1
        if ($h98oDestRow.Count -eq 1) {
            $h98oDestinationTiles = @($h98oDestRow[0])
            for ($h98oTile = 0; $h98oTile -lt $h98oDestinationTiles.Count; $h98oTile++) {
                if ($h98oDestinationTiles[$h98oTile].window -eq $h98oFocusAfter) { $h98oFocusIndex = $h98oTile }
            }
        }
        Set-Content (Join-Path $D 'h98o-diagonal.txt') "before=x$h98oXBefore active=$h98oActiveBefore y=$h98oYBefore focus=$h98oFocusBefore after=x$h98oXAfter active=$h98oActiveAfter y=$h98oYAfter focus=$h98oFocusAfter index=$h98oFocusIndex lefts=$($h98oLefts -join ',')"
        $summary.h98_overview_diagonal_focus_pans_the_camera = (
            ($h98oHost -ne [IntPtr]::Zero) -and
            ($h98oActiveBefore -ne $h98oActiveAfter) -and
            ($h98oFocusAfter -ne $h98oFocusBefore) -and
            ($h98oFocusIndex -ge 1) -and
            ($h98oXAfter -lt $h98oXBefore) -and
            (@($h98oLefts | Sort-Object -Unique).Count -eq 1) -and
            ([math]::Abs($h98oYAfter - $h98oYBefore) -le 2))
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h98o-error.txt')
    } finally {
        if ($null -ne $h98oProc) { Stop-WiriGracefully $h98oProc 'h98o-overview' }
        foreach ($h98oForm in $h98oForms) { if ([LT]::IsWindow($h98oForm)) { [TestForms]::CloseForm($h98oForm) } }
        $h98oForms = @()
        $h98oProc = $null
        Start-Sleep -Milliseconds 400
    }
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h98o-outer-error.txt')
} finally {
    if ($null -ne $h98oProc) { Stop-WiriGracefully $h98oProc 'h98o-final' }
    foreach ($h98oForm in $h98oForms) { if ([LT]::IsWindow($h98oForm)) { [TestForms]::CloseForm($h98oForm) } }
}
