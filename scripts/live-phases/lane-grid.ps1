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
    'h98_without_the_flag_behaviour_is_niris'
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
function H98Visible($forms, $work) {
    $visible = @()
    foreach ($form in $forms) {
        $rect = Rc $form
        if ($rect.W -le 0) { continue }
        if (($rect.Y -lt ($work.Y + $work.H - 8)) -and ($rect.Y -ge ($work.Y - 8))) {
            $visible += ,@($form, $rect)
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

$h98Forms = @()
$h98Proc = $null
try {
    try {
        $h98Proc = Start-WiriWithConfig (H98Config $true $h98Binds) (Join-Path $D 'wiri.stderr98.log')
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
        $null = Ctl 'h98-home' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $h98Outputs = Ctl 'h98-outputs' @('list-monitors')
        $summary.h98_config_accepts_unified_grid = ($h98Outputs -match 'unified-grid: on')
        Set-Content (Join-Path $D 'h98-outputs.txt') ($h98Outputs | ConvertTo-Json -Depth 4 -Compress)

        $h98Work = H98Work $h98Outputs
        # Row 1 (first workspace), third column: 520 px wide, centred near x = 8 + 320 + 8 + 260.
        $null = Ctl 'h98-col2' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98AnchorRect = Rc $h98Forms[1]
        $h98AnchorCentre = $h98AnchorRect.X + [int]($h98AnchorRect.W / 2)
        $h98AnchorRight = $h98AnchorRect.X + $h98AnchorRect.W

        # 1. Rule 3: moving one row down keeps x and lands on the column under the anchor.
        $null = Ctl 'h98-down' @('action', 'focus-workspace-down')
        Start-Sleep -Milliseconds 700
        $h98Visible = H98Visible $h98Forms $h98Work
        $under = @($h98Visible | Where-Object {
            ($h98AnchorCentre -ge $_.X) -and ($h98AnchorCentre -lt ($_.X + $_.W))
        })
        $summary.h98_vertical_move_keeps_x_and_picks_the_column_under_it = ($under.Count -eq 1) -and ($h98Visible.Count -ge 1)
        Set-Content (Join-Path $D 'h98-vertical.txt') "anchor=$h98AnchorRect.X,$($h98AnchorRect.Y),$($h98AnchorRect.W) centre=$h98AnchorCentre visible=$(($h98Visible | ForEach-Object { "$($_.X),$($_.Y),$($_.W)" }) -join ' ')"

        # 2. An indexed workspace action lands on the x-matched column too.
        $null = Ctl 'h98-ws1' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-ws3' @('action', 'focus-workspace', '3')
        Start-Sleep -Milliseconds 700
        $h98Visible3 = H98Visible $h98Forms $h98Work
        $under3 = @($h98Visible3 | Where-Object {
            ($h98AnchorCentre -ge $_.X) -and ($h98AnchorCentre -lt ($_.X + $_.W))
        })
        $summary.h98_move_to_named_workspace_lands_on_the_x_matched_column = ($under3.Count -eq 1)

        # 3. `focus-up-right`: one row up and one column right, as one motion.
        $null = Ctl 'h98-home2' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2b' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98Before = Rc $h98Forms[1]
        $null = Ctl 'h98-upright' @('action', 'focus-up-right')
        Start-Sleep -Milliseconds 700
        $h98VisibleAfter = H98Visible $h98Forms $h98Work
        $h98Moved = @($h98VisibleAfter | Where-Object {
            ($_.X -ge ($h98Before.X + $h98Before.W)) -and
            ([math]::Abs($_.X - $h98Before.X) -gt 40)
        })
        $summary.h98_focus_up_right_lands_on_the_diagonal_target = ($h98Moved.Count -ge 1)

        # 4. `focus-down-left`: one row down and one column left.
        $null = Ctl 'h98-home3' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col3' @('action', 'focus-column', '3')
        Start-Sleep -Milliseconds 300
        $h98LeftBefore = Rc $h98Forms[2]
        $null = Ctl 'h98-downleft' @('action', 'focus-down-left')
        Start-Sleep -Milliseconds 700
        $h98LeftVisible = H98Visible $h98Forms $h98Work
        $h98LeftMoved = @($h98LeftVisible | Where-Object {
            ($_.X -lt $h98LeftBefore.X) -and ($_.Y -ge ($h98LeftBefore.Y - 4))
        })
        $summary.h98_focus_down_left_lands_on_the_diagonal_target = ($h98LeftMoved.Count -ge 1)

        # 5. `move-window-up-right`: the window itself arrives in the row above.
        $null = Ctl 'h98-home4' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col1' @('action', 'focus-column', '1')
        Start-Sleep -Milliseconds 300
        $h98Mover = $h98Forms[0]
        $h98MoverBefore = Rc $h98Mover
        $null = Ctl 'h98-moveupright' @('action', 'move-window-up-right')
        Start-Sleep -Milliseconds 800
        $h98MoverAfter = Rc $h98Mover
        # The moved window keeps focus, so it is the visible one of the row above and its x
        # is one column right of where it was.
        $h98MoverVisible = ($h98MoverAfter.Y -lt ($h98MoverBefore.Y - 4)) -or
                           ([math]::Abs($h98MoverAfter.Y - $h98MoverBefore.Y) -le 4)
        $summary.h98_move_window_up_right_moves_the_window_to_the_target_row =
            $h98MoverVisible -and (($h98MoverAfter.W -lt $h98MoverBefore.W) -or ($h98MoverAfter.X -ne $h98MoverBefore.X))
        Set-Content (Join-Path $D 'h98-move.txt') "before=$($h98MoverBefore.X),$($h98MoverBefore.Y),$($h98MoverBefore.W),$($h98MoverBefore.H) after=$($h98MoverAfter.X),$($h98MoverAfter.Y),$($h98MoverAfter.W),$($h98MoverAfter.H)"

        # 6. Rule 5: Mod+Up then Right with Up still held lands on the diagonal target.
        $null = Ctl 'h98-home5' @('action', 'focus-workspace', '1')
        Start-Sleep -Milliseconds 500
        $null = Ctl 'h98-col2c' @('action', 'focus-column', '2')
        Start-Sleep -Milliseconds 300
        $h98ChordBefore = Rc $h98Forms[1]
        H98Chord @(0x26, 0x27)
        Start-Sleep -Milliseconds 900
        $h98ChordVisible = H98Visible $h98Forms $h98Work
        $h98ChordTargets = @($h98ChordVisible | Where-Object {
            ($_.X -ge ($h98ChordBefore.X + $h98ChordBefore.W)) -and
            ([math]::Abs($_.X - $h98ChordBefore.X) -gt 40)
        })
        $summary.h98_two_arrows_together_land_on_the_diagonal_target = ($h98ChordTargets.Count -ge 1)
        Set-Content (Join-Path $D 'h98-chord.txt') "before=$($h98ChordBefore.X),$($h98ChordBefore.Y),$($h98ChordBefore.W) visible=$(($h98ChordVisible | ForEach-Object { "$($_.X),$($_.Y),$($_.W)" }) -join ' ')"
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
        Set-Content (Join-Path $D 'h98-off.txt') "off=$h98bOff anchor=$($h98bAnchor.X),$($h98bAnchor.W) visible=$(($h98bVisible | ForEach-Object { "$($_.X),$($_.W)" }) -join ' ')"
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
