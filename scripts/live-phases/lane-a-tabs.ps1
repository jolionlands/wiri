# Phase 2B / H18: real tab visibility, focus and indicator; no external processes.
function H18Chord([byte]$key) {
    [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
    [LT]::Key($key, $false); Start-Sleep -Milliseconds 90; [LT]::Key($key, $true)
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
}
$h18Keys = @('h18_toggle_keeps_focused_tab','h18_only_active_tab_visible','h18_up_down_change_tab_and_focus','h18_tab_keys_cycle_focus','h18_indicator_is_column_local','h18_toggle_normal_shows_all_tabs','h18_default_column_display_tabbed')
foreach ($key in $h18Keys) { $summary[$key] = $false }
$h18Proc = $null; $h18Forms = @()
try {
    $h18Proc = Start-WiriWithConfig @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout {
    shadow { off; }
    tab-indicator { position "left"; width 7; gap 3; hide-when-single-tab false; }
}
binds {
    Mod+F10 { focus-window-down; }
    Mod+F11 { focus-window-up; }
}
'@ (Join-Path $D 'wiri.stderr18.log')
    foreach ($i in 1..2) { $h18Forms += [TestForms]::Open("wiri-h18-$i", 60, (80 + 40*$i), 190); Start-Sleep -Milliseconds 1000 }
    $a = $h18Forms[0]; $b = $h18Forms[1]
    FocusForm $a
    $null = Ctl 'h18-consume' @('action','consume-window-into-column')
    Start-Sleep -Milliseconds 500
    FocusForm $b
    $null = Ctl 'h18-tabbed' @('action','toggle-column-tabbed-display')
    Start-Sleep -Milliseconds 500
    $summary.h18_toggle_keeps_focused_tab = ([LT]::GetForegroundWindow() -eq $b) -and [LT]::IsWindowVisible($b)
    $summary.h18_only_active_tab_visible = [LT]::IsWindowVisible($b) -and -not [LT]::IsWindowVisible($a)
    $ind = @([LT]::TopLevel() | Where-Object { [LT]::Class($_) -eq 'WiriDecor' -and [LT]::Title($_) -like 'WiriDecor Tabs *' })
    # The raster host includes a 2px transparent antialiasing margin on every side.
    $summary.h18_indicator_is_column_local = ($ind.Count -eq 1) -and ((Rc $ind[0]).W -eq 11) -and ((Rc $ind[0]).H -gt 50)
    Shot 'h18-tabbed-selected-second'
    H18Chord 0x7A
    Start-Sleep -Milliseconds 600
    $up = ([LT]::GetForegroundWindow() -eq $a) -and [LT]::IsWindowVisible($a) -and -not [LT]::IsWindowVisible($b)
    H18Chord 0x79
    Start-Sleep -Milliseconds 600
    $summary.h18_up_down_change_tab_and_focus = $up -and ([LT]::GetForegroundWindow() -eq $b) -and -not [LT]::IsWindowVisible($a)
    $null = Ctl 'h18-tab-next' @('action','tab-next'); Start-Sleep -Milliseconds 500
    $next = ([LT]::GetForegroundWindow() -eq $a) -and -not [LT]::IsWindowVisible($b)
    $null = Ctl 'h18-tab-prev' @('action','tab-prev'); Start-Sleep -Milliseconds 500
    $summary.h18_tab_keys_cycle_focus = $next -and ([LT]::GetForegroundWindow() -eq $b) -and -not [LT]::IsWindowVisible($a)
    $null = Ctl 'h18-normal' @('action','toggle-column-tabbed-display'); Start-Sleep -Milliseconds 600
    $summary.h18_toggle_normal_shows_all_tabs = [LT]::IsWindowVisible($a) -and [LT]::IsWindowVisible($b) -and -not (OverlapsRect $a (Rc $b))
    Shot 'h18-normal'
    Stop-WiriGracefully $h18Proc 'h18'; $h18Proc = $null
    foreach ($h in $h18Forms) { [TestForms]::CloseForm($h) }; $h18Forms = @()
    $h18Proc = Start-WiriWithConfig @'
animations { off; }
layout {
    default-column-display "tabbed"
    tab-indicator { hide-when-single-tab false; }
}
'@ (Join-Path $D 'wiri.stderr18-default.log')
    $h18Forms += [TestForms]::Open('wiri-h18-default', 70, 100, 150)
    Start-Sleep -Milliseconds 1000
    $summary.h18_default_column_display_tabbed = (@([LT]::TopLevel() | Where-Object { [LT]::Title($_) -eq "WiriDecor Tabs $([int64]$h18Forms[0])" }).Count -eq 1)
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h18-error.txt')
} finally {
    if ($null -ne $h18Proc) { Stop-WiriGracefully $h18Proc 'h18-final' }
    foreach ($h in $h18Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
}
