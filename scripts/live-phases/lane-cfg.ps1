# Lane CFG (H91): a config reload re-applies gaps, window rules, binds and animation settings
# without restarting wiri.  Each gate group has its own try/catch; nothing outside the owned
# fixture forms and the wiri instance started here is touched.
$h91Keys = @(
    'h91_gap_reload_moves_the_window',
    'h91_window_rule_reload_affects_a_new_window',
    'h91_bind_reload_registers_the_new_chord',
    'h91_animation_reload_changes_motion'
)
foreach ($key in $h91Keys) { $summary[$key] = $false }

# The whole config is rewritten for each reload: one `binds {}` and one `window-rule {}` per
# physical file, or the reload is refused as a duplicate.
function H91Config([int]$gaps, [string]$rule, [string]$extraBind, [string]$animations) {
    $text = "layout { gaps $gaps; }`n"
    $text += "animations { $animations }`n"
    $text += "binds {`n    Ctrl+Alt+F8 { toggle-window-floating; }`n    $extraBind`n}`n"
    $text += "window-rule {`n    $rule`n}`n"
    return $text
}

function H91Reload([string]$text, [string]$tag) {
    [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
    $null = Ctl "h91-$tag-reload" @('reload-config')
    Start-Sleep -Milliseconds 900
}

function H91Chord([byte]$key) {
    [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
    [LT]::Key($key, $false); Start-Sleep -Milliseconds 90; [LT]::Key($key, $true)
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
}

# Rectangles sample motion; a couple of pixels of rounding must not fail a gate.
# `Rc` reports niri's L/T/R/B/W/H (the harness helper's own fields), so the position
# fields are L and T; `.X`/`.Y` do not exist and silently compared $null with $null.
function H91Near($a, $b, [int]$tolerance = 3) {
    return ([math]::Abs($a.L - $b.L) -le $tolerance) -and
           ([math]::Abs($a.T - $b.T) -le $tolerance) -and
           ([math]::Abs($a.W - $b.W) -le $tolerance) -and
           ([math]::Abs($a.H - $b.H) -le $tolerance)
}

$h91Proc = $null
$h91Forms = @()
$h91Spawned = Join-Path $D 'h91-bind-ran.txt'
try {
    if (Test-Path $h91Spawned) { Remove-Item $h91Spawned -Force }
    # `window-resize` drives the deltas probe below and `window-movement` the gap reload, so
    # both are configured here: a width change animates from any strip position, while a
    # column-focus scroll does not (the focused window can be the last column).
    $h91Long = 'window-movement { duration-ms 1200; curve "linear"; }' + "`n" + 'window-resize { duration-ms 1200; curve "linear"; }'
    $h91Proc = Start-WiriWithConfig (H91Config 16 '' 'Ctrl+Alt+F9 { toggle-window-floating; }' $h91Long) (Join-Path $D 'wiri.stderr91.log')
    $h91Forms += [TestForms]::Open('wiri-h91-a', 60, 80, 190)
    Start-Sleep -Milliseconds 1200

    # 1. `layout { gaps }`: a reload moves the tiled window by the difference of the gaps.
    try {
        FocusForm $h91Forms[0]
        Start-Sleep -Milliseconds 300
        $h91Before = Rc $h91Forms[0]
        H91Reload (H91Config 60 '' 'Ctrl+Alt+F9 { toggle-window-floating; }' $h91Long) 'gap'
        # The re-placement animates (window-movement 1200 ms): sample the settled rect.
        Start-Sleep -Milliseconds 900
        $h91After = Rc $h91Forms[0]
        $deltaX = $h91After.L - $h91Before.L
        $deltaY = $h91After.T - $h91Before.T
        # niri keeps the scrolled view's horizontal offset, so only the vertical placement is a
        # fixed function of the gap difference: the top edge moves by `60 - 16 = 44` px and the
        # view shrinks by twice that. The horizontal delta depends on where the strip happens
        # to be scrolled (measured -16 for a right-aligned last column, +44 for the first), so
        # it is only used as evidence, not asserted.
        $summary.h91_gap_reload_moves_the_window = ($deltaY -ge 30) -and ($deltaY -le 60) -and
            (($h91Before.H - $h91After.H) -ge 60) -and ($h91After.W -eq $h91Before.W)
        Set-Content (Join-Path $D 'h91-gap.txt') "before=$($h91Before.L),$($h91Before.T),$($h91Before.W),$($h91Before.H) after=$($h91After.L),$($h91After.T),$($h91After.W),$($h91After.H) dx=$deltaX dy=$deltaY"
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h91-gap-error.txt')
    }

    # 2. `window-rule`: a rule added by the reload governs the windows that open afterwards.
    try {
        $h91BeforeRule = @([LT]::TopLevel() | Where-Object { [LT]::Title($_) -like 'wiri-h91-*' }).Count
        H91Reload (H91Config 60 'match title="^wiri-h91"; open-floating true;' 'Ctrl+Alt+F9 { toggle-window-floating; }' $h91Long) 'rule'
        $h91Forms += [TestForms]::Open('wiri-h91-b', 60, 90, 120)
        Start-Sleep -Milliseconds 1200
        $h91Monitor = Rc $h91Forms[0]
        $h91Floating = Rc $h91Forms[1]
        # The window that opened after the reload keeps its own size (floating, 520x380 from
        # the fixture) instead of being stretched to the view (a tiled window spans the whole
        # work-area height). The width is not a discriminator: a tiled column is ~500 px wide
        # and the floating fixture is ~520, so the height is asserted.
        $summary.h91_window_rule_reload_affects_a_new_window = ($h91Floating.H -lt ($h91Monitor.H - 100)) -and ($h91Floating.W -gt 400) -and ($h91Monitor.W -gt 400) -and ($h91BeforeRule -ge 1)
        Set-Content (Join-Path $D 'h91-rule.txt') "tiled=$($h91Monitor.L),$($h91Monitor.T),$($h91Monitor.W),$($h91Monitor.H) opened=$($h91Floating.L),$($h91Floating.T),$($h91Floating.W),$($h91Floating.H)"
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h91-rule-error.txt')
    }

    # 3. `binds`: the chord added by the reload is registered without a restart.
    try {
        FocusForm $h91Forms[0]
        Start-Sleep -Milliseconds 300
        $h91Bind = 'Ctrl+Alt+F9 { spawn-sh "cmd /c echo h91 > ' + $h91Spawned.Replace('\', '\\') + '"; }'
        H91Reload (H91Config 60 'match title="^wiri-h91"; open-floating true;' $h91Bind $h91Long) 'bind'
        H91Chord 0x78
        Start-Sleep -Milliseconds 300
        $summary.h91_bind_reload_registers_the_new_chord = WaitFile $h91Spawned 4
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h91-bind-error.txt')
    }

    # 4. `animations`: a reload from a long window-resize curve to `off` makes the same width
    #    change land immediately. A width change is used instead of a column-focus scroll,
    #    because a scroll needs a column in that direction (the focused window can be the
    #    last one, where `focus-column-right` has nothing to move and no motion is visible).
    try {
        FocusForm $h91Forms[0]
        Start-Sleep -Milliseconds 200
        $h91From = Rc $h91Forms[0]
        $null = Ctl 'h91-resize-long' @('action', 'set-column-width', '650')
        Start-Sleep -Milliseconds 150
        $h91Mid = Rc $h91Forms[0]
        Start-Sleep -Milliseconds 1600
        $h91End = Rc $h91Forms[0]
        $h91Animated = -not (H91Near $h91Mid $h91End)
        H91Reload (H91Config 60 'match title="^wiri-h91"; open-floating true;' 'Ctrl+Alt+F9 { toggle-window-floating; }' 'off;') 'anim'
        FocusForm $h91Forms[0]
        Start-Sleep -Milliseconds 200
        $null = Ctl 'h91-resize-off' @('action', 'set-column-width', '420')
        Start-Sleep -Milliseconds 150
        $h91InstantMid = Rc $h91Forms[0]
        Start-Sleep -Milliseconds 600
        $h91InstantEnd = Rc $h91Forms[0]
        $h91Instant = (H91Near $h91InstantMid $h91InstantEnd) -and ([math]::Abs($h91InstantEnd.W - $h91End.W) -gt 20)
        $summary.h91_animation_reload_changes_motion = $h91Animated -and $h91Instant
        Set-Content (Join-Path $D 'h91-anim.txt') "from=$($h91From.W) mid=$($h91Mid.W) end=$($h91End.W) instantMid=$($h91InstantMid.W) instantEnd=$($h91InstantEnd.W)"
    } catch {
        $_ | Out-String | Set-Content (Join-Path $D 'h91-anim-error.txt')
    }
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h91-error.txt')
} finally {
    if ($null -ne $h91Proc) { Stop-WiriGracefully $h91Proc 'h91-final' }
    foreach ($h91Form in $h91Forms) { if ([LT]::IsWindow($h91Form)) { [TestForms]::CloseForm($h91Form) } }
    if (Test-Path $h91Spawned) { Remove-Item $h91Spawned -Force -ErrorAction SilentlyContinue }
}
