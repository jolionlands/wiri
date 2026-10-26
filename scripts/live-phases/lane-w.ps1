# Lane W / H50-H53. Dot-source after $summary exists, under live-test.ps1's desktop mutex:
# . (Join-Path $PSScriptRoot 'live-phases\lane-w.ps1')
# Only the parent adds that line and runs the FINAL FULL SUITE; this file is not a standalone runner.
# References: DOCUMENTATION.md (input/layout/window-rule), IPC_PROTOCOL.md section 4.11,
# parity/niri-gaps-layout-behaviour.md sections 12-13; lane-a.ps1 and lane-g.ps1.
# No display changes, external applications, synthetic urgency events, or dependencies.
& {
    # Child scope keeps the parent's config/profile/output variables untouched. The parent's
    # Ctl, Shot and Start-Wiri helpers resolve these lane-local paths when called below.
    $D = Join-Path $D 'lane-w'
    $null = New-Item -ItemType Directory -Force -Path $D
    $wSavedPipe = $env:WIRI_PIPE
    $wSavedConfig = $env:WIRI_CONFIG
    $wOwned = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $wForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $wState = @{ daemon = $null; cursorRestored = $true }
    $wPhases = @('h50', 'h51', 'h51-scroll', 'h52', 'h52-activation', 'h53')
    $wDetails = [ordered]@{
        gates = [ordered]@{}; errors = [ordered]@{}; screenshots = [ordered]@{}
        daemons = @(); cleanup = [ordered]@{}
        assumptions = @(
            'Interactive unlocked SDR desktop; colour tolerance 12/255; straight-edge width tolerance 1 px.',
            'Pixel oracles read saved SCREENSHOT PNGs, not config values or decoration-window metadata.',
            'The ring belongs to the active window only; inactive/urgent background windows use their border.',
            'clip-to-geometry is a binary four-corner HWND region; PtInRegion uses DWM-visible offsets within GetWindowRect.',
            'DWM preference is square (1) on both the opt-in clipped target and the global square control; it is not proof of clipping.',
            'Wheel probes own their Form/WndProc counters and use SendInput tag 0x4c414e57, NOT Wiri replay tag 0x57495257.',
            'Windows activation requests here mean actual own-application FlashWindowEx -> shell flash/ALERT, never synthetic NotifyWinEvent.',
            'Windows cannot veto an already-granted foreground or supply a Wayland activation serial; explicit focus is not an application request.',
            'Activation policy is latched at admission; the unmatched default is set-urgent.',
            'No output setters; floating offsets are measured against the parent work area ($swa).'
        )
    }
    $wGates = @(
        'h50_rule_active_border_colour_and_width', 'h50_rule_inactive_border_colour_and_width',
        'h50_rule_active_focus_ring_colour_and_width', 'h50_unmatched_control_keeps_global_colours_and_widths',
        'h50_rule_native_clip_region_is_target_only', 'h50_own_flashwindowex_makes_urgent_border_red',
        'h50_foreground_clears_urgent_colours', 'h50_title_change_restores_original_region',
        'h51_rule_tabbed_column_hides_other_window_after_consume',
        'h51_selecting_other_tab_swaps_real_visibility', 'h51_unmatched_consumed_column_stays_normal',
        'h51_scroll_factor_zero_suppresses_own_wheel', 'h51_scroll_factor_two_doubles_own_wheel',
        'h51_scroll_unmatched_control_receives_unscaled_wheel',
        'h52_rule_float_has_bottom_right_work_area_offsets', 'h52_float_position_and_control_survive_focus',
        'h52_activation_ignore_keeps_control_and_inactive_border', 'h52_activation_set_urgent_marks_only_target',
        'h52_activation_focus_promotes_own_target', 'h52_default_activation_is_set_urgent',
        'h52_activation_policy_is_latched_across_title_change', 'h52_explicit_focus_is_not_an_activation_request',
        'h53_rule_border_gradient_has_red_blue_endpoints', 'h53_rule_focus_ring_gradient_has_red_blue_endpoints',
        'h53_gradient_midpoints_and_unmatched_control', 'h53_owned_cleanup_complete'
    )
    foreach ($gate in $wGates) {
        $summary[$gate] = $false
        $wDetails.gates[$gate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    function WGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $wDetails.gates[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
    }
    function WError([string]$name, $err) {
        $text = $err | Out-String
        $wDetails.errors[$name] = $text
        $text | Set-Content -Encoding UTF8 (Join-Path $D ($name+'-error.txt'))
    }
    function WAssertDaemon {
        $p = $wState.daemon
        if ($null -eq $p -or -not $wOwned.Contains($p) -or $p.HasExited) { throw 'Lane W has no retained live daemon' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $p.Id }).Count -ne 0) {
            throw 'Lane W refuses to drive the desktop while a foreign wiri daemon is present'
        }
        [LaneWNative]::VerifyPipe($env:WIRI_PIPE, $p.Id)
    }
    function WCtl([string]$tag, [string[]]$arguments) {
        WAssertDaemon
        $n = $steps.Count
        $text = Ctl ('w-'+$tag) $arguments
        if ($steps.Count -ne ($n+1) -or [string]$steps[$n].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $text
    }
    function WAction([string]$tag, [string[]]$arguments) {
        $null = WCtl $tag (@('action') + $arguments)
        Start-Sleep -Milliseconds 650
    }
    function WWindows([string]$tag) {
        $reply = (WCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true -or $null -eq $reply.result) { throw "Bad window-list envelope: $tag" }
        return @($reply.result)
    }
    function WEntry($list, [IntPtr]$h) {
        $entries = @($list | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($entries.Count -ne 1) { throw "Expected exactly one owned window-list entry: $h" }
        return $entries[0]
    }
    function WFocus([IntPtr]$h) {
        if (-not $wForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "Refuse focus of a non-owned form: $h"
        }
        WAssertDaemon
        FocusForm $h
        if (-not (WaitForeground $h 200 2500)) { throw "Owned form did not take foreground: $h" }
    }
    function WOpen([string]$suffix) {
        WAssertDaemon
        # Lane-owned subclass: no parent TestForms counters/handlers or API changes.
        $h = [LaneWForms]::Open(('lane-w-'+$suffix))
        if ($h -eq [IntPtr]::Zero) { throw "Test form did not open: $suffix" }
        $wForms.Add($h)
        if ([LT]::Pid($h) -ne $PID) { throw 'Unexpected test-form PID' }
        Start-Sleep -Milliseconds 1100
        WFocus $h
        return $h
    }
    function WShot([string]$tag) {
        $bounds = [Windows.Forms.SystemInformation]::VirtualScreen
        Shot $tag
        $shot = [pscustomobject]@{
            path = (Join-Path $D ($tag+'.png')); left = $bounds.Left; top = $bounds.Top
            width = $bounds.Width; height = $bounds.Height; utc = [DateTime]::UtcNow.ToString('o')
        }
        $wDetails.screenshots[$tag] = $shot
        return $shot
    }
    function WBand($shot, $r, [string]$colour, [int]$width, [int]$firstMin, [int]$firstMax) {
        # Six outward scanlines: three along each straight left/top edge, away from corners.
        # Longest contiguous colour runs must have the requested WIDTH and be next to this form.
        $rays = [LaneWNative]::Bands($shot.path, $shot.left, $shot.top, $r.L, $r.T, $r.R, $r.B, $colour)
        $pass = ($rays.Count -eq 6)
        foreach ($ray in $rays) {
            $pass = $pass -and ([Math]::Abs($ray.Length-$width) -le 1) -and
                ($ray.First -ge $firstMin) -and ($ray.First -le $firstMax)
        }
        return [pscustomobject]@{ pass = [bool]$pass; png = $shot.path; rect = $r; expected = $colour; width = $width; rays = $rays }
    }
    function WVisible([IntPtr]$h) {
        return [LT]::IsWindow($h) -and [LT]::IsWindowVisible($h) -and -not [LT]::IsIconic($h) -and
            ([LT]::Cloaked($h) -eq 0) -and (OnScreen $h)
    }
    function WHidden([IntPtr]$h) {
        return [LT]::IsWindow($h) -and -not [LT]::IsIconic($h) -and
            ((-not [LT]::IsWindowVisible($h)) -or ([LT]::Cloaked($h) -gt 0))
    }
    function WSameRect($a, $b) {
        return (Near $a.L $b.L 2) -and (Near $a.T $b.T 2) -and (Near $a.W $b.W 2) -and (Near $a.H $b.H 2)
    }
    function WRegionEqual($a, $b) {
        return ($a.Error -eq 0 -and $b.Error -eq 0 -and $a.Kind -eq $b.Kind -and $a.Bytes -ceq $b.Bytes)
    }
    function WClipPass($r) {
        # A square DWM preference alone cannot pass. Require all FOUR clipped corners,
        # retained straight-edge midpoints and centre in the actual complex HWND region.
        return ($r.Kind -eq 3 -and $r.Error -eq 0 -and $r.Measured -and $r.Center -and
            @($r.Corners | Where-Object { $_ }).Count -eq 0 -and $r.Corners.Count -eq 4 -and
            @($r.EdgeMidpoints | Where-Object { $_ }).Count -eq 4 -and $r.Preference -eq 1)
    }
    function WWheelProbe([IntPtr]$h, [int]$factor, [string]$tag) {
        WFocus $h
        [LaneWForms]::ResetWheel($h)
        $before = [LaneWForms]::WheelLog($h)
        WAssertDaemon
        $verticalInput = [LaneWNative]::WheelOwn($h,120,$false)
        Start-Sleep -Milliseconds 750
        # Recheck ownership, foreground, pointer and modifiers before EVERY injection.
        WAssertDaemon
        $horizontalInput = [LaneWNative]::WheelOwn($h,-120,$true)
        Start-Sleep -Milliseconds 750
        $after = [LaneWForms]::WheelLog($h)
        $foreground = ([LT]::GetForegroundWindow() -eq $h)
        $pass = ($before.Events.Count -eq 0 -and $foreground -and $after.VerticalDelta -eq (120*$factor) -and
            $after.HorizontalDelta -eq (-120*$factor))
        if ($factor -eq 0) { $pass = $pass -and $after.Events.Count -eq 0 }
        else { $pass = $pass -and $after.VerticalCount -gt 0 -and $after.HorizontalCount -gt 0 }
        $shot = WShot $tag
        return [pscustomobject]@{
            pass=[bool]$pass; factor=$factor; hwnd=[int64]$h; foreground=$foreground
            input=@($verticalInput,$horizontalInput); before=$before; after=$after; png=$shot.path
        }
    }
    function WActivationProbe([IntPtr]$target, [IntPtr]$control, [string]$policy, [string]$tag) {
        # Showing/focusing is NOT the request. Settle both forms and explicitly focus
        # the control first, then let the own target ask via Windows FlashWindowEx.
        WFocus $target
        $activeShot = WShot ($tag+'-explicit-initial-focus')
        $initialActive = WBand $activeShot (Rc $target) '#00e080' 7 0 2
        WFocus $control
        $shot = WShot ($tag+'-before-flash')
        $before = WBand $shot (Rc $target) '#8020e0' 7 0 2
        $controlBefore = WBand $shot (Rc $control) '#40b080' 2 0 2
        $baseline = ($initialActive.pass -and $before.pass -and $controlBefore.pass -and
            [LT]::GetForegroundWindow() -eq $control)
        if (-not $baseline) { throw "Activation probe lacks settled own background target: $tag" }
        WAssertDaemon
        $previousFlash = [LaneWNative]::FlashOwn($target,$true)
        $attempts = @(); $pass = $false; $safe = $true; $stable = 0
        try {
            for ($attempt=0; $attempt -lt 12; $attempt++) {
                Start-Sleep -Milliseconds 250
                $shot = WShot ($tag+'-flash-'+$attempt)
                $fg = [LT]::GetForegroundWindow()
                if ($fg -ne $target -and $fg -ne $control) { $safe=$false; break }
                $colour = '#8020e0'
                if ($policy -eq 'set-urgent') { $colour='#ff0000' }
                elseif ($policy -eq 'focus') { $colour='#00e080' }
                $band = WBand $shot (Rc $target) $colour 7 0 2
                $cb = WBand $shot (Rc $control) $(if ($policy -eq 'focus') { '#405060' } else { '#40b080' }) 2 0 2
                $expectedForeground = $control
                if ($policy -eq 'focus') { $expectedForeground=$target }
                $ok = ($fg -eq $expectedForeground -and $band.pass -and $cb.pass)
                $attempts += [ordered]@{ foreground=[int64]$fg; expected=$policy; pass=$ok; target=$band; control=$cb; png=$shot.path }
                if ($policy -eq 'ignore') {
                    # Negative policy: never focus/mark urgent during the FULL interval.
                    if (-not $ok) { $safe=$false; break }
                    $pass = ($attempt -eq 11)
                } else {
                    if ($ok) { $stable++ } else { $stable=0 }
                    if ($stable -ge 2) { $pass=$true; break }
                }
            }
        } finally { $null = [LaneWNative]::FlashOwn($target,$false) }
        # Do not apply an application-request policy to ordinary explicit focus or
        # foreground notifications: ignore must not block focus; urgent must clear.
        WFocus $target
        $shot = WShot ($tag+'-explicit-focus-after-flash')
        $explicitActive = WBand $shot (Rc $target) '#00e080' 7 0 2
        WFocus $control
        Start-Sleep -Milliseconds 750
        $shot = WShot ($tag+'-explicit-control-after-flash')
        $explicitInactive = WBand $shot (Rc $target) '#8020e0' 7 0 2
        $explicitControl = WBand $shot (Rc $control) '#40b080' 2 0 2
        $explicit = ($explicitActive.pass -and $explicitInactive.pass -and $explicitControl.pass -and
            [LT]::GetForegroundWindow() -eq $control)
        return [pscustomobject]@{
            pass=[bool]($baseline -and $safe -and $pass); explicitFocus=[bool]$explicit; policy=$policy
            previousFlash=$previousFlash; initialActive=$initialActive; before=$before; controlBefore=$controlBefore
            attempts=$attempts; after=@($explicitActive,$explicitInactive,$explicitControl); windows=@(WWindows ($tag+'-windows'))
        }
    }
    function WStart([string]$phase, [string]$rules) {
        if (@($wForms | Where-Object { [LT]::IsWindow($_) }).Count -ne 0) {
            throw 'Lane W refuses a new scenario while previous owned forms remain unclosed'
        }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'Lane W refuses an existing daemon; it never stops a foreign process'
        }
        # Fresh, lane-owned child environment/config/state for EACH scenario; no parent config rewrite.
        $testRoaming = Join-Path $D ($phase+'\profile\roaming')
        $testLocal = Join-Path $D ($phase+'\profile\local')
        $cfg = Join-Path $testRoaming 'wiri\config.kdl'
        $null = New-Item -ItemType Directory -Force -Path (Split-Path $cfg), $testLocal
        $env:WIRI_CONFIG = $cfg
        $device = [Windows.Forms.Screen]::PrimaryScreen.DeviceName
        $text = @"
input { mod-key "alt"; }
animations { off; }
workspace "lane-w-outside"
workspace "lane-w"
layout {
    gaps 32
    center-focused-column "never"
    default-column-width { proportion 0.30; }
    default-column-display "normal"
    border { on; width 2; active-color "#40b080"; inactive-color "#405060"; urgent-color "#ff8800"; }
    focus-ring { on; width 3; gap 2; active-color "#b0b0b0"; inactive-color "#606060"; urgent-color "#ff8800"; }
    shadow { off; }
    border-radius 0
}
// CloseFocused intentionally uses Ctrl+Alt+Q, independently of Mod=Alt.
binds {
    extend-defaults false
    Ctrl+Alt+Q { close-window; }
}
window-rule { open-on-workspace "lane-w-outside"; }
window-rule {
    match title="^lane-w-"
    open-on-workspace "lane-w"
    open-on-output r"$device"
}
$rules
"@
        [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
        $p = Start-Wiri (Join-Path $D ($phase+'.stderr.log')) (Join-Path $D ($phase+'.stdout.log'))
        $wOwned.Add($p); $wState.daemon = $p
        $null = $p.Handle # Retain this exact process before any further operation.
        $wDetails.daemons += [ordered]@{ phase = $phase; pid = $p.Id; config = $cfg; roaming = $testRoaming; local = $testLocal }
        Start-Sleep -Seconds 4
        WAssertDaemon
        WAction ($phase+'-workspace') @('focus-workspace', '2')
    }
    function WClose([IntPtr]$h, [string]$tag) {
        if (-not [LT]::IsWindow($h)) { return }
        if (-not $wForms.Contains($h) -or [LT]::Pid($h) -ne $PID -or [LT]::Title($h) -notlike 'lane-w-*') {
            throw "Refuse close of a non-owned HWND: $h"
        }
        # No CloseForm, WM_CLOSE, direct Close(), Alt+F4, or unguarded close IPC fallback.
        # In particular hidden tabs must be brought forward through the parent's helpers.
        WFocus $h
        WAssertDaemon
        if (-not (WaitForeground $h 100 2500)) { throw "Refuse close without owned foreground: $h" }
        if (-not (CloseFocused $h ('w-'+$tag))) { throw "Parent refused close: $h" }
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ([LT]::IsWindow($h) -and $clock.ElapsedMilliseconds -lt 3500) { Start-Sleep -Milliseconds 100 }
        if ([LT]::IsWindow($h)) { throw "Owned form did not close: $h" }
    }
    function WStop($p, [string]$tag) {
        if ($null -eq $p -or -not $wOwned.Contains($p) -or $p.HasExited) { return }
        try {
            # Like Lane G, verify the server PID and send quit on THAT SAME connected handle.
            [LaneWNative]::QuitOwned($env:WIRI_PIPE, $p.Id)
            $wDetails.cleanup[$tag+'-quit'] = 'sent on PID-validated connection'
        } catch { WError ($tag+'-quit') $_ }
        finally {
            if (-not $p.WaitForExit(4000)) {
                $wDetails.cleanup[$tag+'-forced-owned-stop'] = $p.Id
                $p.Kill(); $null = $p.WaitForExit(5000)
            }
        }
        if (-not $p.HasExited) { throw "Retained daemon did not stop: $($p.Id)" }
    }
    function WFinish([string]$tag) {
        # Also account for a form whose Show/Open failed after its HWND was created.
        if ('LaneWForms' -as [type]) {
            foreach ($h in [LaneWForms]::Handles()) { if (-not $wForms.Contains($h)) { $wForms.Add($h) } }
        }
        foreach ($h in @($wForms.ToArray())) {
            try { WClose $h ($tag+'-close-'+[int64]$h) } catch { WError ($tag+'-close-'+[int64]$h) $_ }
            if (-not [LT]::IsWindow($h)) { $null = $wForms.Remove($h) }
        }
        try { WStop $wState.daemon $tag } catch { WError ($tag+'-stop') $_ }
        $wState.daemon = $null
    }
    function WGradient($shot, $r, [int]$distance) {
        # angle=90 is left -> right. Read 3x3 pixel patches at BOTH straight horizontal edges.
        $inset = [int][Math]::Max(16, [Math]::Round($r.W*0.04))
        $xs = @(($r.L+$inset), ([int][Math]::Floor(($r.L+$r.R)/2)), ($r.R-1-$inset))
        $ys = @(($r.T-$distance), ($r.B+$distance-1))
        $rows = @(); $ends = $true; $middle = $true
        foreach ($y in $ys) {
            $samples = @()
            foreach ($x in $xs) { $samples += [pscustomobject]@{ x=$x; y=$y; rgb=[LaneWNative]::Patch($shot.path,$shot.left,$shot.top,$x,$y) } }
            $red = $samples[0].rgb; $purple = $samples[1].rgb; $blue = $samples[2].rgb
            $ends = $ends -and ($red[0] -ge 220 -and $red[1] -le 20 -and $red[2] -le 40) -and
                ($blue[2] -ge 220 -and $blue[1] -le 20 -and $blue[0] -le 40)
            $middle = $middle -and ($purple[0] -ge 90 -and $purple[0] -le 165 -and $purple[1] -le 20 -and
                $purple[2] -ge 90 -and $purple[2] -le 165 -and [Math]::Abs($purple[0]-$purple[2]) -le 30)
            $rows += ,$samples
        }
        return [pscustomobject]@{ endpoints=[bool]$ends; midpoint=[bool]$middle; png=$shot.path; rect=$r; distance=$distance; rows=$rows }
    }

    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane W refuses a foreign daemon' }
        if (-not ('LaneWNative' -as [type])) {
            Add-Type -ReferencedAssemblies System.Drawing,System.Core,System.Windows.Forms -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;
public sealed class LaneWRegion {
    public int Kind, Error, Preference;
    public string Bytes = "";
    public bool Measured, Center;
    public bool[] Corners = new bool[0], EdgeMidpoints = new bool[0];
    public int NativeLeft, NativeTop, NativeRight, NativeBottom;
    public int VisibleLeft, VisibleTop, VisibleRight, VisibleBottom;
    public int OffsetX, OffsetY, Width, Height;
}
public sealed class LaneWWheelEvent {
    public int Message, Delta;
    public long ExtraInfo;
}
public sealed class LaneWWheelLog {
    public int VerticalCount, VerticalDelta, HorizontalCount, HorizontalDelta;
    public LaneWWheelEvent[] Events;
}
public sealed class LaneWWheelInput {
    public uint Inserted;
    public int X, Y, Delta;
    public bool Horizontal;
    public long ExtraInfo;
}
[StructLayout(LayoutKind.Sequential)] public struct LaneWPoint { public int X, Y; }
sealed class LaneWProbeForm : Form {
    internal LaneWRegion InitialRegion;
    readonly object sync = new object();
    readonly List<LaneWWheelEvent> wheel = new List<LaneWWheelEvent>();
    [DllImport("user32.dll")] static extern IntPtr GetMessageExtraInfo();
    protected override void WndProc(ref Message m) {
        if (m.Msg==0x020a || m.Msg==0x020e) {
            int delta=unchecked((short)((m.WParam.ToInt64()>>16)&0xffff));
            lock (sync) wheel.Add(new LaneWWheelEvent { Message=m.Msg, Delta=delta, ExtraInfo=GetMessageExtraInfo().ToInt64() });
        }
        base.WndProc(ref m);
    }
    internal void ResetWheel() { lock (sync) wheel.Clear(); }
    internal LaneWWheelLog WheelLog() {
        lock (sync) {
            var result=new LaneWWheelLog { Events=wheel.ToArray() };
            foreach (var e in wheel) {
                if (e.Message==0x020a) { result.VerticalCount++; result.VerticalDelta+=e.Delta; }
                else { result.HorizontalCount++; result.HorizontalDelta+=e.Delta; }
            }
            return result;
        }
    }
}
public static class LaneWForms {
    static readonly object sync = new object();
    static readonly Dictionary<IntPtr,LaneWProbeForm> forms = new Dictionary<IntPtr,LaneWProbeForm>();
    static LaneWProbeForm Own(IntPtr h) {
        LaneWNative.RequireOwn(h);
        lock (sync) {
            LaneWProbeForm form;
            if (!forms.TryGetValue(h,out form)) throw new InvalidOperationException("Not a lane-owned probe form");
            return form;
        }
    }
    public static IntPtr[] Handles() { lock (sync) { var keys=new List<IntPtr>(forms.Keys); return keys.ToArray(); } }
    public static IntPtr Open(string title) {
        if (!title.StartsWith("lane-w-")) throw new InvalidOperationException("Probe title must remain lane-owned");
        IntPtr handle=IntPtr.Zero; Exception error=null; bool shown=false;
        var ready=new ManualResetEvent(false);
        var thread=new Thread(delegate() {
            IntPtr oldDpi=IntPtr.Zero;
            try {
                oldDpi=LaneWNative.PhysicalCoordinates();
                var form=new LaneWProbeForm { Text=title, Width=520, Height=380,
                    BackColor=Color.FromArgb(61,113,157), StartPosition=FormStartPosition.WindowsDefaultLocation };
                handle=form.Handle;
                lock (sync) forms.Add(handle,form);
                // Snapshot before Show/admission, not after Wiri may already have clipped it.
                form.InitialRegion=LaneWNative.Region(handle,false);
                form.Shown+=delegate { shown=true; ready.Set(); };
                Application.Run(form);
            } catch (Exception ex) { if (!shown) { error=ex; ready.Set(); } }
            finally {
                // A UI-thread failure is not permission to forget a still-live HWND.
                // The PowerShell cleanup must report it or use the parent's guarded close.
                if (!LaneWNative.LiveOwn(handle)) { lock (sync) forms.Remove(handle); }
                if (oldDpi!=IntPtr.Zero) LaneWNative.RestoreCoordinates(oldDpi);
            }
        });
        thread.SetApartmentState(ApartmentState.STA); thread.IsBackground=true; thread.Start();
        if (!ready.WaitOne(15000)) throw new TimeoutException("Lane W probe did not show");
        ready.Dispose();
        if (error!=null) throw new InvalidOperationException("Lane W probe failed",error);
        return handle;
    }
    public static LaneWRegion OriginalRegion(IntPtr h) { return Own(h).InitialRegion; }
    public static void ResetWheel(IntPtr h) { Own(h).ResetWheel(); }
    public static LaneWWheelLog WheelLog(IntPtr h) { return Own(h).WheelLog(); }
    public static void Rename(IntPtr h,string title) {
        var form=Own(h);
        if (!title.StartsWith("lane-w-")) throw new InvalidOperationException("Rename must retain lane-owned title");
        var done=new ManualResetEvent(false);
        form.BeginInvoke(new MethodInvoker(delegate { form.Text=title; done.Set(); }));
        if (!done.WaitOne(2500)) throw new TimeoutException("Lane W title change timed out");
        done.Dispose();
    }
}
public sealed class LaneWRay {
    public string Side, Pixels;
    public int AnchorX, AnchorY, First, Length;
}
public static class LaneWNative {
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attr, out int value, int size);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    public static IntPtr PhysicalCoordinates() {
        IntPtr old=SetThreadDpiAwarenessContext(new IntPtr(-4)); // PER_MONITOR_AWARE_V2
        if (old==IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        return old;
    }
    public static void RestoreCoordinates(IntPtr old) { SetThreadDpiAwarenessContext(old); }
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetWindowRect(IntPtr h,out RECT r);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetClientRect(IntPtr h,out RECT r);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h,int attr,out RECT value,int size);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h,ref LaneWPoint point);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(LaneWPoint point);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out LaneWPoint point);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetCursorPos(int x,int y);
    [DllImport("gdi32.dll", SetLastError=true)] static extern IntPtr CreateRectRgn(int l,int t,int r,int b);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] static extern int GetWindowRgn(IntPtr h,IntPtr region);
    [DllImport("gdi32.dll", SetLastError=true)] static extern uint GetRegionData(IntPtr region,uint size,byte[] bytes);
    [DllImport("gdi32.dll")] static extern bool PtInRegion(IntPtr region,int x,int y);
    [DllImport("kernel32.dll", EntryPoint="SetLastError")] static extern void ClearLastError(uint error);
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT {
        public int dx,dy; public uint mouseData,flags,time; public UIntPtr extra;
    }
    [StructLayout(LayoutKind.Explicit)] struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mouse; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public INPUTUNION data; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count,INPUT[] input,int size);
    public static bool LiveOwn(IntPtr h) {
        uint pid; GetWindowThreadProcessId(h,out pid);
        return IsWindow(h) && pid==(uint)System.Diagnostics.Process.GetCurrentProcess().Id;
    }
    public static void RequireOwn(IntPtr h) {
        if (!LiveOwn(h)) throw new InvalidOperationException("Refuse non-owned live HWND");
    }
    public static LaneWRegion Region(IntPtr h,bool measure) {
        // GetWindowRect can be DPI-virtualized whereas DWM bounds are physical.
        // Scope the calling thread, then restore it; never change the parent process.
        IntPtr old=PhysicalCoordinates();
        try { return PhysicalRegion(h,measure); }
        finally { RestoreCoordinates(old); }
    }
    static LaneWRegion PhysicalRegion(IntPtr h,bool measure) {
        RequireOwn(h);
        IntPtr region=CreateRectRgn(0,0,0,0);
        if (region==IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
        try {
            ClearLastError(0);
            int kind=GetWindowRgn(h,region), error=Marshal.GetLastWin32Error();
            var result=new LaneWRegion { Kind=kind, Error=kind==0 ? error : 0, Preference=CornerPreference(h) };
            if (kind!=0) {
                uint size=GetRegionData(region,0,null);
                if (size==0 || size>1024*1024) throw new InvalidOperationException("Invalid/oversized own region data");
                var bytes=new byte[(int)size];
                if (GetRegionData(region,size,bytes)!=size) throw new Win32Exception(Marshal.GetLastWin32Error());
                result.Bytes=Convert.ToBase64String(bytes);
            }
            if (!measure) return result;
            RECT native,visible;
            if (!GetWindowRect(h,out native)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (DwmGetWindowAttribute(h,9,out visible,Marshal.SizeOf(typeof(RECT)))!=0)
                throw new InvalidOperationException("No DWM visible frame for region oracle");
            result.NativeLeft=native.Left; result.NativeTop=native.Top; result.NativeRight=native.Right; result.NativeBottom=native.Bottom;
            result.VisibleLeft=visible.Left; result.VisibleTop=visible.Top; result.VisibleRight=visible.Right; result.VisibleBottom=visible.Bottom;
            int x=visible.Left-native.Left, y=visible.Top-native.Top, w=visible.Right-visible.Left, height=visible.Bottom-visible.Top;
            if (w<4 || height<4) throw new InvalidOperationException("Empty own visible frame");
            // GetWindowRgn/PtInRegion are native-window-relative, not DWM-frame-relative.
            result.OffsetX=x; result.OffsetY=y; result.Width=w; result.Height=height; result.Measured=true;
            if (kind!=0) {
                result.Corners=new bool[] { PtInRegion(region,x,y), PtInRegion(region,x+w-1,y),
                    PtInRegion(region,x+w-1,y+height-1), PtInRegion(region,x,y+height-1) };
                result.Center=PtInRegion(region,x+w/2,y+height/2);
                result.EdgeMidpoints=new bool[] { PtInRegion(region,x+w/2,y), PtInRegion(region,x+w-1,y+height/2),
                    PtInRegion(region,x+w/2,y+height-1), PtInRegion(region,x,y+height/2) };
            }
            return result;
        } finally { DeleteObject(region); }
    }
    public static LaneWPoint Cursor() {
        IntPtr old=PhysicalCoordinates();
        try {
            LaneWPoint point;
            if (!GetCursorPos(out point)) throw new Win32Exception(Marshal.GetLastWin32Error());
            return point;
        } finally { RestoreCoordinates(old); }
    }
    public static void RestoreCursor(LaneWPoint point) {
        IntPtr old=PhysicalCoordinates();
        try { if (!SetCursorPos(point.X,point.Y)) throw new Win32Exception(Marshal.GetLastWin32Error()); }
        finally { RestoreCoordinates(old); }
    }
    public static LaneWWheelInput WheelOwn(IntPtr h,int delta,bool horizontal) {
        IntPtr old=PhysicalCoordinates();
        try { return PhysicalWheelOwn(h,delta,horizontal); }
        finally { RestoreCoordinates(old); }
    }
    static LaneWWheelInput PhysicalWheelOwn(IntPtr h,int delta,bool horizontal) {
        RequireOwn(h);
        if (GetForegroundWindow()!=h) throw new InvalidOperationException("Refuse wheel without own foreground");
        foreach (int key in new int[] { 0x10,0x11,0x12,0x5b,0x5c })
            if ((GetAsyncKeyState(key)&0x8000)!=0) throw new InvalidOperationException("Refuse wheel while a modifier is held");
        RECT client;
        if (!GetClientRect(h,out client) || client.Right-client.Left<8 || client.Bottom-client.Top<8)
            throw new InvalidOperationException("No own client interior for wheel");
        var point=new LaneWPoint { X=(client.Left+client.Right)/2, Y=(client.Top+client.Bottom)/2 };
        if (!ClientToScreen(h,ref point) || !SetCursorPos(point.X,point.Y)) throw new Win32Exception(Marshal.GetLastWin32Error());
        if (WindowFromPoint(point)!=h || GetForegroundWindow()!=h)
            throw new InvalidOperationException("Refuse wheel over non-owned client/foreground");
        // Wiri must bypass ONLY its own replay marker. This other injected tag must
        // still be scaled; blanket LLMHF_INJECTED rejection would fail these gates.
        const uint tag=0x4c414e57;
        INPUT move=new INPUT(), wheel=new INPUT();
        move.data.mouse.dx=1; move.data.mouse.flags=1; move.data.mouse.extra=new UIntPtr(tag);
        wheel.data.mouse.mouseData=unchecked((uint)delta); wheel.data.mouse.flags=horizontal ? 0x1000u : 0x0800u;
        wheel.data.mouse.extra=new UIntPtr(tag);
        uint inserted=SendInput(2,new INPUT[] { move,wheel },Marshal.SizeOf(typeof(INPUT)));
        if (inserted!=2) throw new Win32Exception(Marshal.GetLastWin32Error());
        return new LaneWWheelInput { Inserted=inserted, X=point.X, Y=point.Y, Delta=delta, Horizontal=horizontal, ExtraInfo=tag };
    }
    [StructLayout(LayoutKind.Sequential)] struct FLASHWINFO {
        public uint cbSize; public IntPtr hwnd; public uint flags, count, timeout;
    }
    [DllImport("user32.dll")] static extern bool FlashWindowEx(ref FLASHWINFO info);
    public static int CornerPreference(IntPtr h) {
        int value; return DwmGetWindowAttribute(h,33,out value,4)==0 ? value : -1;
    }
    public static bool FlashOwn(IntPtr h, bool start) {
        uint pid; GetWindowThreadProcessId(h,out pid);
        if (pid!=(uint)System.Diagnostics.Process.GetCurrentProcess().Id) throw new InvalidOperationException("Refuse foreign FlashWindowEx");
        FLASHWINFO info=new FLASHWINFO(); info.cbSize=(uint)Marshal.SizeOf(typeof(FLASHWINFO)); info.hwnd=h;
        info.flags=start ? 15u : 0u; // FLASHW_ALL | FLASHW_TIMERNOFG; otherwise FLASHW_STOP.
        info.count=start ? 8u : 0u; info.timeout=500;
        // The return value is the PREVIOUS flashing state, not a success flag.
        return FlashWindowEx(ref info);
    }
    static void OwnedPipe(string name, int pid, bool quit) {
        if (String.IsNullOrWhiteSpace(name)) throw new InvalidOperationException("Lane W requires its private pipe");
        if (name.StartsWith(@"\\.\pipe\")) name=name.Substring(9);
        using (var pipe=new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            pipe.Connect(1000); uint actual;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle,out actual)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (actual!=(uint)pid) throw new InvalidOperationException("Refuse foreign pipe server PID "+actual+", expected "+pid);
            if (!quit) return;
            byte[] bytes=Encoding.UTF8.GetBytes("{\"type\":\"quit\"}\n");
            var write=pipe.BeginWrite(bytes,0,bytes.Length,null,null);
            using (var done=write.AsyncWaitHandle) {
                if (!done.WaitOne(1500)) throw new TimeoutException("Owned quit write timeout");
                pipe.EndWrite(write);
            }
        }
    }
    public static void VerifyPipe(string name,int pid) { OwnedPipe(name,pid,false); }
    public static void QuitOwned(string name,int pid) { OwnedPipe(name,pid,true); }
    static Color Pixel(Bitmap image,int x,int y) {
        if (x<0 || y<0 || x>=image.Width || y>=image.Height) throw new InvalidOperationException("Screenshot sample outside virtual screen");
        return image.GetPixel(x,y);
    }
    public static LaneWRay[] Bands(string path,int ox,int oy,int l,int t,int r,int b,string hex) {
        Color wanted=ColorTranslator.FromHtml(hex); var rays=new List<LaneWRay>();
        using (var image=new Bitmap(path)) {
            for (int side=0;side<2;side++) for (int part=1;part<=3;part++) {
                int ax=side==0 ? l : l+(r-l)*part/4;
                int ay=side==0 ? t+(b-t)*part/4 : t;
                int start=-1, length=0, bestStart=-1, bestLength=0;
                var pixels=new StringBuilder();
                for (int d=0;d<=28;d++) {
                    int x=ax-(side==0 ? d : 0), y=ay-(side==1 ? d : 0);
                    Color c=Pixel(image,x-ox,y-oy);
                    if (d!=0) pixels.Append(' ');
                    pixels.AppendFormat("#{0:x2}{1:x2}{2:x2}",c.R,c.G,c.B);
                    bool match=Math.Abs(c.R-wanted.R)<=12 && Math.Abs(c.G-wanted.G)<=12 && Math.Abs(c.B-wanted.B)<=12;
                    if (match) {
                        if (length==0) start=d;
                        length++;
                        if (length>bestLength) { bestLength=length; bestStart=start; }
                    } else length=0;
                }
                rays.Add(new LaneWRay { Side=side==0 ? "left" : "top", AnchorX=ax, AnchorY=ay,
                    First=bestStart, Length=bestLength, Pixels=pixels.ToString() });
            }
        }
        return rays.ToArray();
    }
    public static int[] Patch(string path,int ox,int oy,int x,int y) {
        int r=0,g=0,b=0;
        using (var image=new Bitmap(path)) for (int dy=-1;dy<=1;dy++) for (int dx=-1;dx<=1;dx++) {
            Color c=Pixel(image,x-ox+dx,y-oy+dy); r+=c.R; g+=c.G; b+=c.B;
        }
        return new int[] { (r+4)/9,(g+4)/9,(b+4)/9 };
    }
}
'@
        }
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_w_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $wDetails['pipe'] = $env:WIRI_PIPE
        $wDetails['work-area'] = [ordered]@{ left=$swa.Left; top=$swa.Top; right=$swa.Right; bottom=$swa.Bottom; width=$swa.Width; height=$swa.Height }

        # H50: actual on-screen solid colours and contiguous band widths, scoped to ONE title.
        try {
            WStart 'h50' @'
window-rule {
    match title="^lane-w-h50-target$"
    border { on; width 7; active-color "#00e080"; inactive-color "#8020e0"; urgent-color "#ff0000"; }
    focus-ring { on; width 6; active-color "#00c8ff"; inactive-color "#e0c000"; urgent-color "#ff0000"; }
    geometry-corner-radius 8
    clip-to-geometry true
}
'@
            $target = WOpen 'h50-target'; $control = WOpen 'h50-control'
            WFocus $target
            $shot = WShot 'h50-target-active'; $tr = Rc $target; $cr = Rc $control
            $activeBorder = WBand $shot $tr '#00e080' 7 0 2
            $activeRing = WBand $shot $tr '#00c8ff' 6 7 12
            $controlInactive = WBand $shot $cr '#405060' 2 0 2
            WGate 'h50_rule_active_border_colour_and_width' $activeBorder.pass $activeBorder
            WGate 'h50_rule_active_focus_ring_colour_and_width' $activeRing.pass $activeRing
            $originalTarget = [LaneWForms]::OriginalRegion($target)
            $originalControl = [LaneWForms]::OriginalRegion($control)
            $targetRegion = [LaneWNative]::Region($target,$true)
            $controlRegion = [LaneWNative]::Region($control,$true)
            $nativeClip = (WClipPass $targetRegion) -and $originalTarget.Kind -eq 0 -and $originalTarget.Error -eq 0 -and
                $controlRegion.Kind -eq 0 -and $controlRegion.Preference -eq 1 -and (WRegionEqual $originalControl $controlRegion)
            WGate 'h50_rule_native_clip_region_is_target_only' $nativeClip ([ordered]@{
                target=$targetRegion; control=$controlRegion; originalTarget=$originalTarget; originalControl=$originalControl; png=$shot.path
            })
            WFocus $control
            $shot = WShot 'h50-control-active'; $tr = Rc $target; $cr = Rc $control
            $inactiveBorder = WBand $shot $tr '#8020e0' 7 0 2
            $controlActive = WBand $shot $cr '#40b080' 2 0 2
            $controlRing = WBand $shot $cr '#b0b0b0' 3 2 7
            WGate 'h50_rule_inactive_border_colour_and_width' $inactiveBorder.pass $inactiveBorder
            WGate 'h50_unmatched_control_keeps_global_colours_and_widths' ($controlInactive.pass -and $controlActive.pass -and $controlRing.pass) @($controlInactive,$controlActive,$controlRing)

            # Flash only this process's OWN background test form. No IPC mark-urgent or
            # NotifyWinEvent shortcut: the application -> Windows -> wiri path must work.
            if (-not (WaitForeground $control 0 1000)) { throw 'Refuse flash without owned control foreground' }
            $previousFlash = [LaneWNative]::FlashOwn($target,$true)
            $urgent = $null; $urgentShots = @()
            for ($attempt=0; $attempt -lt 10; $attempt++) {
                Start-Sleep -Milliseconds 350
                $shot = WShot ('h50-urgent-'+$attempt)
                $urgent = WBand $shot (Rc $target) '#ff0000' 7 0 2
                $urgentShots += [ordered]@{ png=$shot.path; red=$urgent.pass }
                if ($urgent.pass) { break }
            }
            $stillControl = WBand $shot (Rc $control) '#40b080' 2 0 2
            WGate 'h50_own_flashwindowex_makes_urgent_border_red' ($inactiveBorder.pass -and $urgent.pass -and $stillControl.pass -and [LT]::GetForegroundWindow() -eq $control) ([ordered]@{ previousFlash=$previousFlash; attempts=$urgentShots; target=$urgent; control=$stillControl; windows=@(WWindows 'h50-urgent-windows') })
            # TIMERNOFG must stop on focus, and wiri must clear its own urgent state.
            # Do not call FLASHW_STOP first: that could mask a broken foreground-clear path.
            WFocus $target
            $shot = WShot 'h50-focus-clears-urgent'
            $clearBorder = WBand $shot (Rc $target) '#00e080' 7 0 2
            $clearRing = WBand $shot (Rc $target) '#00c8ff' 6 7 12
            WGate 'h50_foreground_clears_urgent_colours' ($urgent.pass -and $clearBorder.pass -and $clearRing.pass) @($clearBorder,$clearRing)
            $null = [LaneWNative]::FlashOwn($target,$false)

            # Leave the rule by changing ONLY our own form's title. Do not clear/set
            # its region ourselves: rule reconciliation must restore the pre-show snapshot.
            [LaneWForms]::Rename($target,'lane-w-h50-restored')
            $restoreAttempts = @(); $restoredRegion = $null
            for ($attempt=0; $attempt -lt 12; $attempt++) {
                Start-Sleep -Milliseconds 250
                $restoredRegion = [LaneWNative]::Region($target,$true)
                $restoreAttempts += $restoredRegion
                if (WRegionEqual $originalTarget $restoredRegion) { break }
            }
            $shot = WShot 'h50-title-change-restores-region'
            $restoredBorder = WBand $shot (Rc $target) '#40b080' 2 0 2
            $controlAfter = [LaneWNative]::Region($control,$true)
            WGate 'h50_title_change_restores_original_region' ($nativeClip -and
                (WRegionEqual $originalTarget $restoredRegion) -and $restoredRegion.Preference -eq 1 -and
                (WRegionEqual $originalControl $controlAfter) -and $restoredBorder.pass -and
                [LT]::Title($target) -eq 'lane-w-h50-restored') ([ordered]@{
                    original=$originalTarget; attempts=$restoreAttempts; restored=$restoredRegion; control=$controlAfter
                    border=$restoredBorder; windows=@(WWindows 'h50-restored-windows'); png=$shot.path
                })
        } catch { WError 'h50' $_ }
        finally { WFinish 'h50' }

        # H51: a per-rule default must remain tabbed after consuming a second window.
        # An indicator on a single-window column alone is NOT evidence of tab visibility.
        try {
            WStart 'h51' @'
window-rule {
    match title="^lane-w-h51-tabbed$"
    default-column-display "tabbed"
}
'@
            $tab = WOpen 'h51-tabbed'; $mate = WOpen 'h51-tabmate'
            WFocus $tab
            WAction 'h51-consume-into-rule-column' @('consume-window-into-column')
            WFocus $tab
            $list = @(WWindows 'h51-consumed-tabbed'); $te = WEntry $list $tab; $me = WEntry $list $mate
            $sameColumn = ($null -ne $te.column -and $null -ne $me.column -and $te.column -ge 0 -and $te.column -eq $me.column -and $null -ne $te.tile -and $null -ne $me.tile -and $te.tile -ne $me.tile)
            $firstVisible = (WVisible $tab) -and (WHidden $mate) -and ([LT]::GetForegroundWindow() -eq $tab)
            $shot = WShot 'h51-consumed-tabbed-selected-first'
            WGate 'h51_rule_tabbed_column_hides_other_window_after_consume' ($sameColumn -and $firstVisible) ([ordered]@{ windows=$list; forms=@(FormInfo @($tab,$mate)); png=$shot.path })
            WFocus $mate
            $secondVisible = (WVisible $mate) -and (WHidden $tab) -and ([LT]::GetForegroundWindow() -eq $mate)
            $shot = WShot 'h51-selected-second-tab'
            WGate 'h51_selecting_other_tab_swaps_real_visibility' ($sameColumn -and $firstVisible -and $secondVisible) ([ordered]@{ forms=@(FormInfo @($tab,$mate)); windows=@(WWindows 'h51-selected-second'); png=$shot.path })
            $a = WOpen 'h51-normal-a'; $b = WOpen 'h51-normal-b'
            WFocus $a
            WAction 'h51-consume-unmatched-column' @('consume-window-into-column')
            $list = @(WWindows 'h51-consumed-normal'); $ae = WEntry $list $a; $be = WEntry $list $b
            $ar = Rc $a; $br = Rc $b
            $normal = ($null -ne $ae.column -and $null -ne $be.column -and $ae.column -ge 0 -and $ae.column -eq $be.column -and $ae.column -ne $te.column -and $null -ne $ae.tile -and $null -ne $be.tile -and $ae.tile -ne $be.tile)
            $separate = ($ar.B -le $br.T) -or ($br.B -le $ar.T)
            $shot = WShot 'h51-unmatched-consumed-normal'
            WGate 'h51_unmatched_consumed_column_stays_normal' ($normal -and $separate -and (WVisible $a) -and (WVisible $b) -and (WHidden $tab) -and (WVisible $mate)) ([ordered]@{ windows=$list; forms=@(FormInfo @($tab,$mate,$a,$b)); rects=@($ar,$br); png=$shot.path })
        } catch { WError 'h51' $_ }
        finally { WFinish 'h51' }

        # Item 2 / H51: real application-delivered wheel deltas, not a config/IPC
        # readback. The subclass above owns the only counters; no parent form edits.
        $wheelCursor = $null
        try {
            WStart 'h51-scroll' @'
window-rule {
    match title="^lane-w-h51-scroll-zero$"
    scroll-factor 0
}
window-rule {
    match title="^lane-w-h51-scroll-double$"
    scroll-factor 2
}
'@
            $wheelCursor = [LaneWNative]::Cursor()
            $wState.cursorRestored = $false
            $control = WOpen 'h51-scroll-control'
            $zero = WOpen 'h51-scroll-zero'; $double = WOpen 'h51-scroll-double'
            $controlBefore = WWheelProbe $control 1 'h51-wheel-control-before'
            $zeroProbe = WWheelProbe $zero 0 'h51-wheel-zero'
            $doubleProbe = WWheelProbe $double 2 'h51-wheel-double'
            $controlAfter = WWheelProbe $control 1 'h51-wheel-control-after'
            $unaffected = ($controlBefore.pass -and $controlAfter.pass)
            WGate 'h51_scroll_factor_zero_suppresses_own_wheel' ($unaffected -and $zeroProbe.pass -and $doubleProbe.pass) ([ordered]@{
                target=$zeroProbe; positiveDelivery=$doubleProbe; control=@($controlBefore,$controlAfter)
            })
            WGate 'h51_scroll_factor_two_doubles_own_wheel' ($unaffected -and $doubleProbe.pass) ([ordered]@{
                target=$doubleProbe; control=@($controlBefore,$controlAfter)
            })
            WGate 'h51_scroll_unmatched_control_receives_unscaled_wheel' $unaffected ([ordered]@{
                probes=@($controlBefore,$controlAfter); harnessTag='0x4c414e57'; wiriReplayTag='0x57495257'
                windows=@(WWindows 'h51-scroll-windows')
            })
        } catch { WError 'h51-scroll' $_ }
        finally {
            try {
                if ($null -ne $wheelCursor) {
                    [LaneWNative]::RestoreCursor($wheelCursor)
                    $wState.cursorRestored = $true
                    $wDetails.cleanup['h51-scroll-cursor'] = [ordered]@{ restored=$true; x=$wheelCursor.X; y=$wheelCursor.Y }
                }
            } catch { WError 'h51-scroll-cursor-restore' $_ }
            finally { WFinish 'h51-scroll' }
        }

        # H52: the visible floating rectangle, not its invisible Win32 resize frame,
        # must be inset x=32/y=24 from THIS work area's bottom-right corner.
        try {
            $floatWidth = [int][Math]::Max(180, [Math]::Floor($swa.Width*0.28))
            $floatHeight = [int][Math]::Max(140, [Math]::Floor($swa.Height*0.30))
            WStart 'h52' @"
window-rule {
    match title="^lane-w-h52-float-"
    open-floating true
    default-column-width { fixed $floatWidth; }
    default-window-height { fixed $floatHeight; }
}
window-rule {
    match title="^lane-w-h52-float-target$"
    default-floating-position x=32 y=24 relative-to="bottom-right"
}
"@
            $control = WOpen 'h52-float-control'; $controlBefore = Rc $control
            $target = WOpen 'h52-float-target'; $r = Rc $target
            $list = @(WWindows 'h52-positioned-float'); $entry = WEntry $list $target
            $offsets = (Near ($swa.Right-$r.R) 32 3) -and (Near ($swa.Bottom-$r.B) 24 3) -and
                (Near $r.L ($swa.Right-$r.W-32) 3) -and (Near $r.T ($swa.Bottom-$r.H-24) 3)
            $inside = ($r.L -ge $swa.Left -and $r.T -ge $swa.Top -and $r.R -le $swa.Right -and $r.B -le $swa.Bottom)
            $shot = WShot 'h52-bottom-right'
            WGate 'h52_rule_float_has_bottom_right_work_area_offsets' ($entry.floating -eq $true -and $offsets -and $inside -and (WVisible $target)) ([ordered]@{ rect=$r; rightOffset=($swa.Right-$r.R); bottomOffset=($swa.Bottom-$r.B); windows=$list; png=$shot.path })
            WFocus $control
            $after = Rc $target; $controlAfter = Rc $control
            $controlNotPositioned = -not ((Near ($swa.Right-$controlBefore.R) 32 3) -and (Near ($swa.Bottom-$controlBefore.B) 24 3))
            $shot = WShot 'h52-control-focused'
            WGate 'h52_float_position_and_control_survive_focus' ($offsets -and (WSameRect $r $after) -and (WSameRect $controlBefore $controlAfter) -and $controlNotPositioned) ([ordered]@{ targetBefore=$r; targetAfter=$after; controlBefore=$controlBefore; controlAfter=$controlAfter; png=$shot.path })
        } catch { WError 'h52' $_ }
        finally { WFinish 'h52' }

        # Item 2 / H52: on-xdg-activate maps ONLY a real application request
        # (shell flash/ALERT from our FlashWindowEx), never arbitrary foreground
        # changes or explicit user/IPC focus. Windows has no Wayland serial and
        # cannot veto foreground already granted by the OS; we do not claim either.
        try {
            WStart 'h52-activation' @'
window-rule {
    match title="^lane-w-h52-activation-(ignore|urgent|focus|default|latched-ignore|latched-focus)$"
    border { on; width 7; active-color "#00e080"; inactive-color "#8020e0"; urgent-color "#ff0000"; }
    focus-ring { on; width 6; active-color "#00c8ff"; }
}
window-rule {
    match title="^lane-w-h52-activation-(ignore|latched-ignore)$"
    on-xdg-activate "ignore"
}
window-rule {
    match title="^lane-w-h52-activation-urgent$"
    on-xdg-activate "set-urgent"
}
window-rule {
    match title="^lane-w-h52-activation-(focus|latched-focus)$"
    on-xdg-activate "focus"
}
'@
            $cases = @(
                @{ suffix='ignore'; policy='ignore'; gate='h52_activation_ignore_keeps_control_and_inactive_border' },
                @{ suffix='urgent'; policy='set-urgent'; gate='h52_activation_set_urgent_marks_only_target' },
                @{ suffix='focus'; policy='focus'; gate='h52_activation_focus_promotes_own_target' },
                @{ suffix='default'; policy='set-urgent'; gate='h52_default_activation_is_set_urgent' },
                @{ suffix='latched-ignore'; policy='ignore'; gate='h52_activation_policy_is_latched_across_title_change'; rename='lane-w-h52-activation-latched-focus' }
            )
            $activation = [ordered]@{}; $explicitFocus = $true
            foreach ($case in $cases) {
                # Only two columns at a time, so a background target is genuinely
                # on-screen for its pixel oracle, independent of the desktop width.
                $target = WOpen ('h52-activation-'+$case.suffix)
                $control = WOpen ('h52-activation-control-'+$case.suffix)
                if ($case.ContainsKey('rename')) {
                    [LaneWForms]::Rename($target,$case.rename)
                    Start-Sleep -Milliseconds 800
                    if ([LT]::Title($target) -ne $case.rename) { throw 'Latched activation probe failed to rename its own form' }
                }
                $probe = WActivationProbe $target $control $case.policy ('h52-activation-'+$case.suffix)
                $activation[$case.suffix] = [ordered]@{
                    initialTitle=('lane-w-h52-activation-'+$case.suffix); titleAtRequest=[LT]::Title($target); result=$probe
                }
                $explicitFocus = $explicitFocus -and $probe.explicitFocus
                WGate $case.gate $probe.pass $activation[$case.suffix]
                WClose $target ('activation-'+$case.suffix+'-target')
                WClose $control ('activation-'+$case.suffix+'-control')
            }
            WGate 'h52_explicit_focus_is_not_an_activation_request' ($explicitFocus -and $activation.Count -eq $cases.Count) $activation
        } catch { WError 'h52-activation' $_ }
        finally { WFinish 'h52-activation' }

        # H53: both rule paints have real red -> blue endpoints AND purple middles.
        # A solid red/blue fallback, metadata-only gradient, or global override cannot pass.
        try {
            WStart 'h53' @'
window-rule {
    match title="^lane-w-h53-gradient$"
    border {
        on; width 8
        active-gradient from="#ff0000" to="#0000ff" angle=90
        inactive-color "#8020e0"; urgent-color "#ff0000"
    }
    focus-ring {
        on; width 6
        active-gradient from="#ff0000" to="#0000ff" angle=90
        inactive-color "#e0c000"; urgent-color "#ff0000"
    }
    geometry-corner-radius 8
    clip-to-geometry true
}
'@
            $target = WOpen 'h53-gradient'; $control = WOpen 'h53-control'
            WFocus $target
            $shot = WShot 'h53-red-blue-gradient'; $r = Rc $target
            $border = WGradient $shot $r 4
            $ring = WGradient $shot $r 13 # border 8 + gap 2 + half of ring 6.
            $controlInactive = WBand $shot (Rc $control) '#405060' 2 0 2
            WGate 'h53_rule_border_gradient_has_red_blue_endpoints' $border.endpoints $border
            WGate 'h53_rule_focus_ring_gradient_has_red_blue_endpoints' $ring.endpoints $ring
            WFocus $control
            $shot = WShot 'h53-unmatched-control'
            $controlBorder = WBand $shot (Rc $control) '#40b080' 2 0 2
            $controlRing = WBand $shot (Rc $control) '#b0b0b0' 3 2 7
            WGate 'h53_gradient_midpoints_and_unmatched_control' ($border.midpoint -and $ring.midpoint -and $controlInactive.pass -and $controlBorder.pass -and $controlRing.pass) ([ordered]@{ border=$border; ring=$ring; control=@($controlInactive,$controlBorder,$controlRing); windows=@(WWindows 'h53-windows') })
        } catch { WError 'h53' $_ }
        finally { WFinish 'h53' }
    } catch { WError 'lane-w' $_ }
    finally {
        try {
            # Close through the parent's guarded path while our daemon still serves the private pipe.
            # If it died, report unclosed forms rather than sending a close to an unrelated foreground.
            try { WFinish 'final' } catch { WError 'final-close' $_ }
            foreach ($p in $wOwned) { try { WStop $p ('final-'+$p.Id) } catch { WError ('final-stop-'+$p.Id) $_ } }
        } finally {
            # Restore FIRST, even if cleanup itself or writing an error file threw.
            $env:WIRI_PIPE = $wSavedPipe
            $env:WIRI_CONFIG = $wSavedConfig
            $restored = ($env:WIRI_PIPE -eq $wSavedPipe -and $env:WIRI_CONFIG -eq $wSavedConfig)
            $remainingForms = @($wForms | Where-Object { [LT]::IsWindow($_) } | ForEach-Object { [int64]$_ })
            $remainingDaemons = @($wOwned | Where-Object { -not $_.HasExited } | ForEach-Object { $_.Id })
            $actualPhases = @($wDetails.daemons | ForEach-Object { $_.phase })
            $allPhases = ($actualPhases.Count -eq $wPhases.Count -and @($wPhases | Where-Object { $actualPhases -notcontains $_ }).Count -eq 0)
            WGate 'h53_owned_cleanup_complete' ($remainingForms.Count -eq 0 -and $remainingDaemons.Count -eq 0 -and $allPhases -and $restored -and $wState.cursorRestored) ([ordered]@{
                remainingForms=$remainingForms; remainingDaemons=$remainingDaemons; started=$wOwned.Count
                plannedPhases=$wPhases; actualPhases=$actualPhases; envRestored=$restored; cursorRestored=$wState.cursorRestored
            })
            try {
                $wDetails | ConvertTo-Json -Depth 18 | Set-Content -Encoding UTF8 (Join-Path $D 'evidence.json')
                $summary['lane_w_evidence'] = (Join-Path $D 'evidence.json')
            } finally { foreach ($p in $wOwned) { $p.Dispose() } }
        }
    }
}
