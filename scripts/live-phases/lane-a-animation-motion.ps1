# H17 desktop observations: a bounded sample at the start/middle/end of real HWND motion.
# The parent harness owns the desktop mutex. This phase owns its daemon and forms only.
$h17Config = @'
input { mod-key "ctrl-alt"; }
animations { slowdown 2; }
layout {
    focus-ring { off; }
    border { off; }
}
window-rule { match title="^h17-"; default-column-width { fixed 350; }; }
'@
$h17Gates = @('h17_spring_resize_has_intermediate_frame', 'h17_resize_reaches_requested_width',
    'h17_column_move_has_intermediate_frame', 'h17_column_move_reaches_destination',
    'h17_workspace_moves_vertically', 'h17_workspace_reaches_rest',
    'h17_settled_geometry_stays_still', 'h17_reload_off_is_instant')
foreach ($gate in $h17Gates) { $summary[$gate] = $false }
$h17Daemon = $null; $h17Forms = @(); $h17Evidence = [ordered]@{}
try {
    $h17Daemon = Start-WiriWithConfig $h17Config (Join-Path $D 'wiri.stderr17.log')
    foreach ($i in 1..2) { $h17Forms += [TestForms]::Open("h17-$i", 120, (90 + $i * 30), 160); Start-Sleep -Milliseconds 1800 }
    $h = $h17Forms[-1]; FocusForm $h
    $before = Rc $h
    $null = Ctl 'h17-resize-spring' @('action', 'set-column-width', '650')
    Start-Sleep -Milliseconds 90; $middle = Rc $h
    Shot 'h17-spring-resize'
    Start-Sleep -Milliseconds 1700; $after = Rc $h
    $h17Evidence.resize = @($before, $middle, $after)
    $summary.h17_spring_resize_has_intermediate_frame = ($middle.W -gt ($before.W + 2)) -and ($middle.W -lt ($after.W - 2))
    $summary.h17_resize_reaches_requested_width = Near $after.W 650 3
    $before = Rc $h
    $null = Ctl 'h17-move-spring' @('action', 'move-column-left')
    Start-Sleep -Milliseconds 90; $middle = Rc $h
    Start-Sleep -Milliseconds 1700; $after = Rc $h
    $h17Evidence.move = @($before, $middle, $after)
    $summary.h17_column_move_has_intermediate_frame = ($middle.L -lt ($before.L - 2)) -and ($middle.L -gt ($after.L + 2))
    $summary.h17_column_move_reaches_destination = ($after.L -lt ($before.L - 100))
    $null = Ctl 'h17-move-ws2' @('action', 'move-to-workspace', '2')
    Start-Sleep -Milliseconds 1800
    $null = Ctl 'h17-focus-ws2' @('action', 'focus-workspace', '2')
    Start-Sleep -Milliseconds 90; $middle = Rc $h
    Shot 'h17-workspace-slide'
    Start-Sleep -Milliseconds 1800; $after = Rc $h
    $h17Evidence.workspace = @($middle, $after)
    $summary.h17_workspace_moves_vertically = ($middle.T -gt ($after.T + 2)) -and (Near $middle.L $after.L 3)
    $summary.h17_workspace_reaches_rest = [LT]::IsWindowVisible($h) -and ($after.T -ge 0) -and ($after.B -le 768)
    Start-Sleep -Milliseconds 600; $still = Rc $h
    $summary.h17_settled_geometry_stays_still = ($still.L -eq $after.L) -and ($still.T -eq $after.T) -and ($still.W -eq $after.W) -and ($still.H -eq $after.H)
    $off = $h17Config.Replace('animations { slowdown 2; }', 'animations { off; }')
    [IO.File]::WriteAllText($cfg, $off, (New-Object Text.UTF8Encoding($false)))
    $null = Ctl 'h17-reload-off' @('reload-config'); Start-Sleep -Milliseconds 800
    $null = Ctl 'h17-resize-off' @('action', 'set-column-width', '420')
    Start-Sleep -Milliseconds 90; $instant = Rc $h
    Start-Sleep -Milliseconds 500; $end = Rc $h
    $h17Evidence.off = @($instant, $end)
    $summary.h17_reload_off_is_instant = (Near $instant.W 420 3) -and ($instant.W -eq $end.W)
    Shot 'h17-off-settled'
} catch { $_ | Out-String | Set-Content (Join-Path $D 'h17-motion-error.txt') }
finally {
    foreach ($h in $h17Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
    if ($null -ne $h17Daemon) { Stop-WiriGracefully $h17Daemon 'h17' }
    $h17Evidence | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $D 'h17-motion.json')
}
