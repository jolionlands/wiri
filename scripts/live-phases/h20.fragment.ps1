# H20 / Phase 2B recent-windows. Parent dot-sources this after harness helpers.
# Do not execute independently. No desktop activity is performed on import elsewhere.
if (-not ('H20Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H20Native {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string name);
}
'@
}
function H20Hosts { return @([LT]::TopLevel() | Where-Object { [LT]::Class($_) -eq 'WiriRecentWindowsHost' -and [LT]::Pid($_) -eq $h20Proc.Id }) }
function H20Host { $hs = @(H20Hosts); if ($hs.Count) { return $hs[0] }; return [IntPtr]::Zero }
function H20Prop([string]$name) { return [H20Native]::GetProp((H20Host), $name).ToInt64() }
function H20Tap([byte]$key) { [LT]::Key($key, $false); [LT]::Key($key, $true); Start-Sleep -Milliseconds 100 }
function H20Open { [LT]::Key(0x12, $false); H20Tap 0x09; Start-Sleep -Milliseconds 220 }
function H20Bounds($h) { $r = New-Object LT+RECT; [void][LT]::GetWindowRect($h, [ref]$r); return "$($r.L),$($r.T),$($r.R),$($r.B)" }
$h20Gates = @('real_dwm_previews','repeated_cycle','sources_unchanged','modifier_release_commit','cancel_restores_origin','scope_navigation','preview_height_cap','debounce_skips_intermediate','keyboard_input_commits','app_id_ignores_titles','no_hosts_after_quit')
foreach ($gate in $h20Gates) { $summary["h20_$gate"] = $false }
$h20Proc = $null; $h20Forms = @()
try {
    $h20Proc = Start-WiriWithConfig @'
layout { gaps 16; }
animations { off; }
recent-windows {
    debounce-ms 750
    open-delay-ms 150
    previews { max-height 320; max-scale 0.5; }
    binds {
        Alt+Tab { next-window; }
        Alt+Shift+Tab { previous-window; }
        Alt+grave { next-window filter="app-id"; }
    }
}
'@ (Join-Path $D 'h20-stderr.log')
    foreach ($i in 1..3) { $h20Forms += [TestForms]::Open("wiri-h20-different-title-$i", (60 * $i), 90, 120); Start-Sleep -Milliseconds 1100 }
    foreach ($h in $h20Forms) { FocusForm $h }
    $origin = $h20Forms[2]
    $before = @($h20Forms | ForEach-Object { H20Bounds $_ })
    H20Open
    $summary.h20_real_dwm_previews = ((H20Host) -ne [IntPtr]::Zero -and [LT]::IsWindowVisible((H20Host)) -and (H20Prop 'WiriRecentThumbnailCount') -ge 2)
    $first = H20Prop 'WiriRecentSelectedWindow'
    H20Tap 0x09
    $second = H20Prop 'WiriRecentSelectedWindow'
    $summary.h20_repeated_cycle = ($first -eq [int64]$h20Forms[1] -and $second -eq [int64]$h20Forms[0])
    $after = @($h20Forms | ForEach-Object { H20Bounds $_ })
    $summary.h20_sources_unchanged = (($before -join ';') -eq ($after -join ';') -and [LT]::GetForegroundWindow() -eq $origin)
    $ph = H20Prop 'WiriRecentPreviewHeight'
    $summary.h20_preview_height_cap = ($ph -gt 0 -and $ph -le 320)
    Shot 'h20-live-dwm-previews'
    # Any remaining modifier sustains the switcher, not just the opening Alt.
    [LT]::Key(0x10, $false); [LT]::Key(0x12, $true); Start-Sleep -Milliseconds 180
    $held = ((H20Host) -ne [IntPtr]::Zero -and [LT]::GetForegroundWindow() -eq $origin)
    [LT]::Key(0x10, $true); Start-Sleep -Milliseconds 450
    $summary.h20_modifier_release_commit = ($held -and [LT]::GetForegroundWindow() -eq $h20Forms[0] -and @(H20Hosts).Count -eq 0)
    FocusForm $origin
    H20Open
    H20Tap 0x4F; $outputScope = H20Prop 'WiriRecentScope'
    H20Tap 0x57; $workspaceScope = H20Prop 'WiriRecentScope'
    H20Tap 0x41; $allScope = H20Prop 'WiriRecentScope'
    H20Tap 0x53; $cycledScope = H20Prop 'WiriRecentScope'
    $summary.h20_scope_navigation = ($outputScope -eq 1 -and $workspaceScope -eq 2 -and $allScope -eq 0 -and $cycledScope -eq 1)
    H20Tap 0x1B; [LT]::Key(0x12, $true); Start-Sleep -Milliseconds 250
    $summary.h20_cancel_restores_origin = ([LT]::GetForegroundWindow() -eq $origin -and @(H20Hosts).Count -eq 0)
    # MRU is c,b,a. A transient focus of a must not reorder b behind it.
    foreach ($h in $h20Forms) { FocusForm $h }
    $null = Ctl 'h20-transient-a' @('focus-window', [string][int64]$h20Forms[0])
    $null = Ctl 'h20-back-c' @('focus-window', [string][int64]$origin)
    H20Open
    $summary.h20_debounce_skips_intermediate = ((H20Prop 'WiriRecentSelectedWindow') -eq [int64]$h20Forms[1])
    H20Tap 0x1B; [LT]::Key(0x12, $true)
    # Actual app input bypasses the focus deadline.
    $null = Ctl 'h20-input-focus-a' @('focus-window', [string][int64]$h20Forms[0])
    H20Tap 0x58
    $null = Ctl 'h20-input-back-c' @('focus-window', [string][int64]$origin)
    H20Open
    $summary.h20_keyboard_input_commits = ((H20Prop 'WiriRecentSelectedWindow') -eq [int64]$h20Forms[0])
    H20Tap 0x1B; [LT]::Key(0x12, $true)
    # All fixtures have one executable identity but intentionally different titles.
    FocusForm $origin
    [LT]::Key(0x12, $false); H20Tap 0xC0; Start-Sleep -Milliseconds 220
    $appFirst = H20Prop 'WiriRecentSelectedWindow'
    H20Tap 0xC0
    $appSecond = H20Prop 'WiriRecentSelectedWindow'
    $summary.h20_app_id_ignores_titles = ($appFirst -ne 0 -and $appSecond -ne 0 -and $appFirst -ne $appSecond)
    H20Tap 0x1B; [LT]::Key(0x12, $true)
} catch {
    $summary.h20_error = $_.Exception.Message
} finally {
    foreach ($key in @(0x09,0xC0,0x10,0x11,0x12,0x5B,0x5C)) { [LT]::Key([byte]$key, $true) }
    foreach ($h in $h20Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
    if ($null -ne $h20Proc) { Stop-WiriGracefully $h20Proc 'h20' }
    $summary.h20_no_hosts_after_quit = ($null -ne $h20Proc -and @(H20Hosts).Count -eq 0)
}
