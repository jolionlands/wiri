# Lane A / Phase 2B. Dot-sourced after the existing summary is assembled.
# These checks own only their daemon and WinForms windows; the harness holds the desktop lock.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneA {
  [DllImport("user32.dll", EntryPoint="GetWindowLongW")] public static extern int Style(IntPtr h, int index);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out int v, int s);
  public static int CornerPreference(IntPtr h) { int v; return DwmGetWindowAttribute(h, 33, out v, 4) == 0 ? v : -1; }
}
'@
function ADecor($h) { return @([LT]::TopLevel() | Where-Object { [LT]::Class($_) -eq 'WiriDecor' -and [LT]::Title($_) -eq "WiriDecor Frame $([int64]$h)" }) }
function AFrameFits($h, [int]$margin) {
    $o = @(ADecor $h); if ($o.Count -ne 1) { return $false }
    $t = Rc $h; $r = Rc $o[0]
    return (Near $r.L ($t.L - $margin) 3) -and (Near $r.T ($t.T - $margin) 3) -and (Near $r.R ($t.R + $margin) 3) -and (Near $r.B ($t.B + $margin) 3)
}
$a16Config = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout {
    border { on; width 5; active-color "#ff8844"; inactive-color "#505050"; }
    focus-ring { on; width 6; active-gradient from="#80c8ff" to="#bbddff" angle=45; }
    shadow { off; }
    border-radius 8
    insert-hint { color "#ffc87f80"; }
}
'@
foreach ($key in @('h16_overlay_exists','h16_frame_surrounds_target','h16_overlay_excluded_from_tiles','h16_overlay_click_through_noactivate_toolwindow','h16_overlay_not_in_alt_tab','h16_frame_follows_move_and_resize','h16_target_keeps_keyboard_focus','h16_column_drag_has_insert_hint','h16_insert_hint_removed_after_drop','h16_dwm_round_corner_preference','h16_shadow_expands_same_layer','h16_no_decor_windows_after_quit')) { $summary[$key] = $false }
$a16 = $null; $a16Forms = @()
try {
    $a16 = Start-WiriWithConfig $a16Config (Join-Path $D 'wiri.stderr16.log')
    foreach ($i in 1..2) { $a16Forms += [TestForms]::Open("h16-$i", 110, (80 + $i * 40), 160); Start-Sleep -Milliseconds 1200 }
    $h = $a16Forms[-1]; FocusForm $h
    $ov = @(ADecor $h)
    $summary.h16_overlay_exists = ($ov.Count -eq 1)
    $summary.h16_frame_surrounds_target = AFrameFits $h 13
    $summary.h16_overlay_excluded_from_tiles = (@((WinList 'h16-windows') | Where-Object { $_.class_name -eq 'WiriDecor' }).Count -eq 0)
    $styleOk = $ov.Count -eq 1
    foreach ($o in $ov) {
        $style = [LaneA]::Style($o, -20)
        $styleOk = $styleOk -and (($style -band 0x080800A0) -eq 0x080800A0) -and (($style -band 0x00040000) -eq 0)
        $styleOk = $styleOk -and ([int64][LaneA]::SendMessage($o, 0x21, [IntPtr]::Zero, [IntPtr]::Zero) -eq 3)
    }
    $summary.h16_overlay_click_through_noactivate_toolwindow = $styleOk
    $summary.h16_overlay_not_in_alt_tab = ($ov.Count -eq 1) -and ([ShellViews]::InSwitchers($ov[0]) -le 0)
    $before = Rc $h
    $null = Ctl 'h16-resize' @('action', 'set-column-width', '370')
    Start-Sleep -Milliseconds 600
    $after = Rc $h
    $summary.h16_frame_follows_move_and_resize = (AFrameFits $h 13) -and (($after.W -ne $before.W) -or ($after.L -ne $before.L))
    $summary.h16_target_keeps_keyboard_focus = ([LT]::GetForegroundWindow() -eq $h)
    Shot 'h16-decorated'
    # Exercise the same real column reorder path that draws the insert hint.
    $r = Rc $h
    [void][LaneA]::SetCursorPos(($r.L + 30), ($r.T + 10))
    [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
    [LaneA]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 100
    # SetCursorPos alone does not feed WH_MOUSE_LL: inject a real relative movement.
    [LaneA]::mouse_event(1, 60, 10, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 200
    $summary.h16_column_drag_has_insert_hint = (@([LT]::TopLevel() | Where-Object { [LT]::Title($_) -eq 'WiriDecor Hint 0' }).Count -eq 1)
    Shot 'h16-insert-hint'
    [LaneA]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
    Start-Sleep -Milliseconds 300
    $summary.h16_insert_hint_removed_after_drop = (@([LT]::TopLevel() | Where-Object { [LT]::Title($_) -eq 'WiriDecor Hint 0' }).Count -eq 0)
    $summary.h16_dwm_round_corner_preference = ([LaneA]::CornerPreference($h) -eq 2)
    $shadowConfig = $a16Config.Replace('shadow { off; }', 'shadow { on; softness 18; spread 4; offset x=3 y=4; color "#00000099"; }')
    [IO.File]::WriteAllText($cfg, $shadowConfig, (New-Object Text.UTF8Encoding($false)))
    $null = Ctl 'h16-reload-shadow' @('reload-config'); Start-Sleep -Milliseconds 600
    $shadow = @(ADecor $h); $tile = Rc $h
    if ($shadow.Count -eq 1) {
        $s = Rc $shadow[0]
        $summary.h16_shadow_expands_same_layer = ($s.L -lt ($tile.L - 13)) -and ($s.R -gt ($tile.R + 13)) -and ($s.B -gt ($tile.B + 13))
    }
    Shot 'h16-shadow'
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h16-error.txt')
} finally {
    [LaneA]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
    foreach ($h in $a16Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
    if ($null -ne $a16) { Stop-WiriGracefully $a16 'h16' }
}
$summary.h16_no_decor_windows_after_quit = (@([LT]::TopLevel() | Where-Object { [LT]::Class($_) -eq 'WiriDecor' -and [LT]::Pid($_) -eq $a16.Id }).Count -eq 0)

# The single Lane A entry point keeps the parent harness unchanged apart from its dot-source.
. (Join-Path $PSScriptRoot 'lane-a-animations.ps1')
. (Join-Path $PSScriptRoot 'lane-a-animation-motion.ps1')
. (Join-Path $PSScriptRoot 'lane-a-tabs.ps1')
. (Join-Path $PSScriptRoot 'h19.fragment.ps1')
. (Join-Path $PSScriptRoot 'lane-a-recent.ps1')
. (Join-Path $PSScriptRoot 'lane-a-hotkeys.ps1')
