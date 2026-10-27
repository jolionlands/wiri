# H17 desktop observations: a bounded sample at the start/middle/end of real HWND motion.
# The parent harness owns the desktop mutex. This phase owns its daemon and forms only.
# Workspace motion now lives on thumbnails, not the cloaked incoming HWND.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H17AnimationProbe {
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h,string key);
}
'@
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
    # Explicitly stay on workspace 1: niri's move-window-to-workspace follows focus
    # by default. The separately sampled focus below must start the vertical slide.
    $null = Ctl 'h17-move-ws2' @('action', 'move-window-to-workspace', '2', '--focus', 'false')
    Start-Sleep -Milliseconds 1800
    $null = Ctl 'h17-focus-ws2' @('action', 'focus-workspace', '2')
    # Sample until the slide is first observable (at most 700 ms, inside the 2x-slowed
    # transition): one fixed 90 ms sample missed the layer whenever the machine was busy.
    # The gate's conditions are unchanged; polling only finds the slide's first frame.
    $h17Clock = [Diagnostics.Stopwatch]::StartNew()
    $h17SeenMs = -1
    do {
        $h17Layers = @([LT]::TopLevel() | Where-Object { [LT]::Class($_) -eq 'WiriAnimationLayer' -and [LT]::IsWindowVisible($_) })
        $h17SlideY = if ($h17Layers.Count -eq 1) { [H17AnimationProbe]::GetProp($h17Layers[0], 'WiriAnimationIncomingY').ToInt64() } else { 0 }
        $h17SlideCloaked = [LT]::Cloaked($h)
        if ($h17Layers.Count -eq 1 -and $h17SlideY -gt 0 -and $h17SlideCloaked -gt 0) { $h17SeenMs = $h17Clock.ElapsedMilliseconds; break }
        Start-Sleep -Milliseconds 15
    } while ($h17Clock.ElapsedMilliseconds -lt 700)
    $middle = Rc $h
    # Zero IncomingY is ambiguous: it can also be a single host with no admitted
    # incoming tile. Keep the exact-one-layer gate, but retain enough evidence to
    # distinguish admission failure from absent/foreign/duplicate hosts.
    $h17LayerEvidence = @($h17Layers | ForEach-Object {
        @{ hwnd=$_.ToInt64(); pid=[LT]::Pid($_)
           kind=[H17AnimationProbe]::GetProp($_, 'WiriAnimationKind').ToInt64()
           incoming_y=[H17AnimationProbe]::GetProp($_, 'WiriAnimationIncomingY').ToInt64()
           outgoing_y=[H17AnimationProbe]::GetProp($_, 'WiriAnimationOutgoingY').ToInt64()
           thumbnails=[H17AnimationProbe]::GetProp($_, 'WiriAnimationThumbnailCount').ToInt64() }
    })
    Shot 'h17-workspace-slide'
    Start-Sleep -Milliseconds 1800; $after = Rc $h
    $h17Evidence.workspace = @($middle, $after, @{thumbnail_y=$h17SlideY; cloaked=$h17SlideCloaked; layer_count=$h17Layers.Count; seen_ms=$h17SeenMs; layers=$h17LayerEvidence})
    $summary.h17_workspace_moves_vertically = ($h17SlideCloaked -gt 0) -and ($h17SlideY -gt ($after.T + 2)) -and (Near $middle.L $after.L 3)
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
