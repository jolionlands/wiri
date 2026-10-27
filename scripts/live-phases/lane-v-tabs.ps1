# Lane V / H65: item 4B, configurable clickable tab indicators (niri e19df12cf8).
# Parent integration: invoke under the existing desktop lock, after Lane H and
# with no daemon running. Uses D, cfg, summary, steps, Ctl, WiriPipeName,
# Start-Wiri, LT, Rc, TestForms, FocusForm and WaitForeground from live-test.ps1.
# Indicator button pairs go ONLY to the retained daemon's exact
# WiriDecor Tabs <owned-first-HWND> host. Bind/close chords use the harness's
# foreground guards on owned probes. Hit-test,
# WindowFromPoint, NOACTIVATE and MOUSEACTIVATE checks accompany those pairs.
# p_* are informational measured numbers, never pass/fail inputs; -1 is unknown.
& {
    $vtDir = Join-Path $D 'lane-v-tabs'
    $vtGates = @(
        'h65_owned_two_tab_column', 'h65_set_display_bind', 'h65_set_display_ctl_idempotent',
        'h65_set_display_typed_ipc', 'h65_click_hidden_tab_and_focus',
        'h65_decor_never_activates', 'h65_gap_and_margin_ignore_clicks',
        'h65_rounded_corners_select_tab',
        'h65_left_indicator_geometry', 'h65_right_indicator_geometry',
        'h65_top_indicator_geometry', 'h65_bottom_indicator_geometry',
        'h65_place_within_column_reserves_each_side',
        'h65_reload_updates_indicator_and_reservation',
        'h65_off_removes_indicator_and_reservation', 'h65_negative_gap_reservation',
        'h65_active_inactive_colors', 'h65_gradients_reload',
        'h65_fullscreen_suppresses_across_tab_switches',
        'h65_maximize_edges_suppresses_across_tab_switches',
        'h65_maximize_column_keeps_indicator_and_reservation',
        'h65_hide_single_removes_indicator_and_reservation',
        'h65_single_tab_can_be_shown', 'h65_cleanup')
    foreach ($key in $vtGates) { $summary[$key] = $false }
    foreach ($key in @('p_h65_last_reload_roundtrip_ms', 'p_h65_click_focus_max_ms',
            'p_h65_daemon_peak_working_set_bytes')) { $summary[$key] = -1 }
    $vtDetails = [ordered]@{ reference='niri e19df12cf8'; checks=[ordered]@{};
        observations=(New-Object 'System.Collections.Generic.List[object]');
        limitations=@('Posted owned-host clicks, not physical input.',
            'Urgent paints are configured/reloaded, but urgency is not induced or claimed verified.') }
    $vt = [pscustomobject]@{ Proc=$null; First=[IntPtr]::Zero; Serial=0;
        Saved=$null; Touched=$false; DecorActivated=$false; Graceful=$false }
    $vtForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    function VTGate([string]$key,[bool]$pass,$observed) {
        $summary[$key]=$pass
        $vtDetails.checks[$key]=[ordered]@{pass=$pass; observed=$observed}
    }
    function VTOwnForm([IntPtr]$h) {
        if (-not $vtForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw 'Refused a non-owned or destroyed probe HWND'
        }
    }
    function VTAlive {
        if ($null -eq $vt.Proc -or $vt.Proc.HasExited) { throw 'Retained H65 daemon is not alive' }
    }
    function VTCtl([string]$tag,[string[]]$words) {
        VTAlive
        # Verify the pipe server identity before EVERY CLI request, including reload.
        $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$vt.Proc.Id
        try { } finally { $wire.Dispose() }
        $vt.Serial++
        $name=('lane-v-tabs-{0:d4}-{1}' -f $vt.Serial,$tag)
        $text=Ctl $name $words
        VTAlive
        if ($steps.Count -eq 0 -or $steps[$steps.Count-1].step -ne $name -or
                [string]$steps[$steps.Count-1].rc -ne '0') { throw "CLI failed: $name" }
        return $text
    }
    function VTActive {
        VTAlive
        $shown=@($vtForms | Where-Object { [LT]::IsWindow($_) -and [LT]::IsWindowVisible($_) })
        if ($shown.Count -ne 1 -or [LT]::GetForegroundWindow() -ne $shown[0]) {
            throw 'Expected exactly one visible, foreground owned tab'
        }
        VTOwnForm $shown[0]
        return $shown[0]
    }
    function VTAction([string]$name) {
        $null=VTActive
        $null=VTCtl $name @('action',$name)
        Start-Sleep -Milliseconds 500
    }
    function VTConfig([string]$side='left',[bool]$within=$true,[bool]$off=$false,
            [bool]$hide=$false,[int]$gap=8,[string]$paint='solid') {
        $flag=if ($off) { 'off;' } else { '' }
        $inside=if ($within) { 'true' } else { 'false' }
        $single=if ($hide) { 'true' } else { 'false' }
        $paints=if ($paint -eq 'gradient') {
            'active-gradient from="#e03040" to="#f09030" angle=90; inactive-gradient from="#20b050" to="#20a0c0" angle=90; urgent-gradient from="#3050f0" to="#a030f0" angle=90;'
        } else { 'active-color "#e03040"; inactive-color "#20b050"; urgent-color "#3050f0";' }
        return @"
input { mod-key "ctrl-alt"; }
animations { off; }
binds { Mod+F8 { set-column-display "tabbed"; }; Mod+F9 { set-column-display "normal"; }; Mod+Q { close-window; }; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
    tab-indicator {
        $flag
        hide-when-single-tab $single
        place-within-column $inside
        position "$side"
        gap $gap
        width 12
        length total-proportion=0.64
        gaps-between-tabs 14
        corner-radius 4
        $paints
    }
}
"@
    }
    function VTReload([string]$text,[string]$tag) {
        VTAlive
        [IO.File]::WriteAllText($cfg,$text,(New-Object Text.UTF8Encoding($false)))
        $sw=[Diagnostics.Stopwatch]::StartNew()
        $null=VTCtl ('reload-'+$tag) @('reload-config')
        $sw.Stop(); $summary['p_h65_last_reload_roundtrip_ms']=$sw.Elapsed.TotalMilliseconds
        Start-Sleep -Milliseconds 650
    }
    function VTDecor {
        VTAlive
        $hosts=@([LT]::TopLevel() | Where-Object {
            [LT]::Pid($_) -eq $vt.Proc.Id -and [LT]::Class($_) -eq 'WiriDecor' -and
            [LT]::Title($_) -eq ('WiriDecor Tabs '+[string][int64]$vt.First) -and
            [LT]::IsWindowVisible($_)
        })
        if ($hosts.Count -gt 1) { throw 'Ambiguous owned tab indicator' }
        if ($hosts.Count -eq 0) { return [IntPtr]::Zero }
        return $hosts[0]
    }
    function VTOwnDecor([IntPtr]$h) {
        VTAlive
        if ($h -eq [IntPtr]::Zero -or $h -ne (VTDecor)) { throw 'Refused a stale/unowned indicator' }
    }
    function VTRect([IntPtr]$h) {
        $r=[LaneVTabsNative]::Rect($h)
        return [pscustomobject]@{ L=$r[0]; T=$r[1]; R=$r[2]; B=$r[3]; W=($r[2]-$r[0]); H=($r[3]-$r[1]) }
    }
    function VTSnapshot([string]$tag) {
        $active=VTActive; $h=VTDecor
        $r=if ($h -ne [IntPtr]::Zero) { VTRect $h } else { $null }
        $s=[pscustomobject]@{tag=$tag; foreground=[int64][LT]::GetForegroundWindow();
            active=[int64]$active; window=(Rc $active); nativeWindow=(VTRect $active);
            host=[int64]$h; indicator=$r;
            visible=@($vtForms | ForEach-Object { [pscustomobject]@{ hwnd=[int64]$_; visible=[LT]::IsWindowVisible($_) } }) }
        $vtDetails.observations.Add($s)
        return $s
    }
    function VTEqual($a,$b,[int]$tol=2) {
        if ($null -eq $a -or $null -eq $b) { return $false }
        return ([Math]::Abs($a.L-$b.L) -le $tol -and [Math]::Abs($a.T-$b.T) -le $tol -and
            [Math]::Abs($a.R-$b.R) -le $tol -and [Math]::Abs($a.B-$b.B) -le $tol)
    }
    function VTReserved($base,$inset,[string]$side,[int]$space) {
        $expected=[pscustomobject]@{L=$base.L; T=$base.T; R=$base.R; B=$base.B}
        switch ($side) {
            left { $expected.L += $space }; right { $expected.R -= $space }
            top { $expected.T += $space }; bottom { $expected.B -= $space }
        }
        return (VTEqual $expected $inset)
    }
    function VTPoints($r,[string]$side) {
        $horizontal=$side -in @('top','bottom')
        $long=if ($horizontal) { $r.W-4 } else { $r.H-4 }
        $first=[int][Math]::Ceiling(($long-14)/2.0)
        $second=$long-14-$first
        if ($first -lt 8 -or $second -lt 8) { throw 'Indicator too short for unambiguous owned probes' }
        $cross=if ($horizontal) { $r.T+8 } else { $r.L+8 }
        $start=if ($horizontal) { $r.L+2 } else { $r.T+2 }
        $values=@($start+[int]($first/2),$start+$first+14+[int]($second/2),$start+$first+7)
        $points=@(foreach ($v in $values) {
            if ($horizontal) { [pscustomobject]@{X=$v;Y=$cross} }
            else { [pscustomobject]@{X=$cross;Y=$v} }
        })
        $margin=if ($horizontal) { [pscustomobject]@{X=$points[0].X;Y=$r.T} }
            else { [pscustomobject]@{X=$r.L;Y=$points[0].Y} }
        return [pscustomobject]@{Tabs=@($points[0],$points[1]); Gap=$points[2]; Margin=$margin;
            Corner=[pscustomobject]@{X=($r.L+2);Y=($r.T+2)}}
    }
    function VTSelected([IntPtr]$expected) {
        if (-not [LT]::IsWindow($expected) -or -not [LT]::IsWindowVisible($expected) -or
                [LT]::GetForegroundWindow() -ne $expected) { return $false }
        foreach ($other in $vtForms) {
            if ($other -ne $expected -and [LT]::IsWindow($other) -and [LT]::IsWindowVisible($other)) { return $false }
        }
        return $true
    }
    function VTClick($point,[IntPtr]$expected,[bool]$painted,[string]$tag) {
        VTOwnForm $expected
        $h=VTDecor; VTOwnDecor $h
        $r=VTRect $h; $x=[int]$point.X; $y=[int]$point.Y
        $before=VTActive
        $hit=[LaneVTabsNative]::Send($h,0x84,[IntPtr]::Zero,[LaneVTabsNative]::Pack($x,$y))
        $root=[LaneVTabsNative]::RootAt($x,$y)
        $noActivate=([LaneVTabsNative]::ExStyle($h) -band 0x08000000) -ne 0
        $ma=[LaneVTabsNative]::Send($h,0x21,$h,[IntPtr]0x02010001)
        $sw=[Diagnostics.Stopwatch]::StartNew()
        $packed=[LaneVTabsNative]::Pack(($x-$r.L),($y-$r.T))
        if (-not [LaneVTabsNative]::PostMessage($h,0x201,[IntPtr]1,$packed)) { throw 'Owned button-down failed' }
        try {
            foreach ($i in 1..3) {
                if ([LT]::GetForegroundWindow() -eq $h) { $vt.DecorActivated=$true }
                Start-Sleep -Milliseconds 15
            }
        } finally {
            # Complete the pair even if an observation fails; never redirect it elsewhere.
            if ([LT]::IsWindow($h) -and [LT]::Pid($h) -eq $vt.Proc.Id) {
                if (-not [LaneVTabsNative]::PostMessage($h,0x202,[IntPtr]::Zero,$packed)) { throw 'Owned button-up failed' }
            }
        }
        $selected=$false
        for ($i=0; $i -lt 120; $i++) {
            if ([LT]::GetForegroundWindow() -eq $h) { $vt.DecorActivated=$true }
            if (VTSelected $expected) { $selected=$true; break }
            Start-Sleep -Milliseconds 25
        }
        $sw.Stop()
        Start-Sleep -Milliseconds 200
        $selected=$selected -and (VTSelected $expected)
        if ($painted -and $selected) {
            $summary['p_h65_click_focus_max_ms']=[Math]::Max($summary['p_h65_click_focus_max_ms'],$sw.Elapsed.TotalMilliseconds)
        }
        $o=[pscustomobject]@{tag=$tag; point=$point; before=[int64]$before; expected=[int64]$expected;
            foreground=[int64][LT]::GetForegroundWindow(); hit=$hit; root=[int64]$root;
            selected=$selected; noActivateStyle=$noActivate; mouseActivate=$ma;
            elapsedMs=$sw.Elapsed.TotalMilliseconds;
            inputRegionCorrect=$(if ($painted) { $hit -eq 1 -and $root -eq $h } else { $hit -eq -1 }) }
        $vtDetails.observations.Add($o)
        return $o
    }
    function VTRgb($point) {
        $bmp=New-Object Drawing.Bitmap 1,1
        $g=[Drawing.Graphics]::FromImage($bmp)
        try {
            $g.CopyFromScreen([int]$point.X,[int]$point.Y,0,0,$bmp.Size)
            $c=$bmp.GetPixel(0,0)
            return @([int]$c.R,[int]$c.G,[int]$c.B)
        } finally { $g.Dispose(); $bmp.Dispose() }
    }
    function VTColorNear($rgb,$expected) {
        if ($rgb.Count -ne 3) { return $false }
        for ($i=0; $i -lt 3; $i++) { if ([Math]::Abs($rgb[$i]-$expected[$i]) -gt 10) { return $false } }
        return $true
    }
    function VTMode([string]$action,[bool]$suppressed,[string]$gate) {
        VTReload (VTConfig) ('before-'+$action)
        $entry=VTActive
        VTAction $action
        $checks=@()
        foreach ($step in @('entry','tab-next','tab-prev')) {
            if ($step -ne 'entry') { VTAction $step }
            $on=VTSnapshot ($action+'-'+$step+'-on')
            VTReload (VTConfig 'left' $true $true) ($action+'-'+$step+'-off')
            $off=VTSnapshot ($action+'-'+$step+'-off')
            VTReload (VTConfig) ($action+'-'+$step+'-restore')
            $again=VTSnapshot ($action+'-'+$step+'-restore')
            $expected=if ($step -eq 'tab-next') {
                @($vtForms | Where-Object { $_ -ne $entry })[0]
            } else { $entry }
            $geometry=if ($suppressed) { VTEqual $off.window $on.window }
                else { VTReserved $off.window $on.window 'left' 20 }
            $checks += ($on.active -eq [int64]$expected -and $off.active -eq $on.active -and
                $again.active -eq $on.active -and $off.host -eq 0 -and
                (($on.host -eq 0) -eq $suppressed) -and (($again.host -eq 0) -eq $suppressed) -and
                $geometry -and (VTEqual $on.window $again.window))
        }
        VTAction $action
        $restored=VTSnapshot ($action+'-normal-again')
        VTGate $gate (-not ($checks -contains $false) -and $restored.host -ne 0) $checks
    }
    function VTStop {
        if ($null -eq $vt.Proc) { return }
        if (-not $vt.Proc.HasExited) {
            try {
                $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$vt.Proc.Id
                try { $wire.Send('{"type":"quit"}'+"`n") } finally { $wire.Dispose() }
                $vt.Graceful=$vt.Proc.WaitForExit(3500)
            } catch { $vtDetails['quitError']=[string]$_ }
            if (-not $vt.Proc.HasExited) {
                $vtDetails['forcedStop']=$true
                $vt.Proc.Kill(); $null=$vt.Proc.WaitForExit(5000)
            }
        }
    }
    try {
        $null=New-Item -ItemType Directory -Path $vtDir -Force
        if ($null -eq ('LaneHConnection' -as [type])) { throw 'H65 requires Lane H identity-checked IPC helper' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H65 refuses an existing daemon; parent must stop its daemon first'
        }
        $vt.Saved=[IO.File]::ReadAllBytes($cfg)
        if ($null -eq ('LaneVTabsNative' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneVTabsNative {
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int X,Y; }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out RECT r);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h,uint f);
    [DllImport("user32.dll",EntryPoint="GetWindowLongW")] static extern int GetWindowLong32(IntPtr h,int i);
    [DllImport("user32.dll",EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetWindowLong64(IntPtr h,int i);
    [DllImport("user32.dll",SetLastError=true)] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll",SetLastError=true)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,IntPtr l,uint f,uint ms,out UIntPtr result);
    public static int[] Rect(IntPtr h) { RECT r; if(!GetWindowRect(h,out r)) throw new InvalidOperationException("GetWindowRect failed"); return new[]{r.L,r.T,r.R,r.B}; }
    public static IntPtr Pack(int x,int y) { return new IntPtr(unchecked((int)(((uint)(ushort)y<<16)|(ushort)x))); }
    public static long ExStyle(IntPtr h) { return IntPtr.Size==8 ? GetWindowLong64(h,-20).ToInt64() : GetWindowLong32(h,-20); }
    public static IntPtr RootAt(int x,int y) { return GetAncestor(WindowFromPoint(new POINT{X=x,Y=y}),2); }
    public static long Send(IntPtr h,uint m,IntPtr w,IntPtr l) {
        UIntPtr result; if(SendMessageTimeout(h,m,w,l,2,500,out result)==IntPtr.Zero) throw new TimeoutException("Owned decor message timeout");
        return IntPtr.Size==8 ? unchecked((long)result.ToUInt64()) : unchecked((int)result.ToUInt32());
    }
}
'@
        }
        [IO.File]::WriteAllText($cfg,(VTConfig),(New-Object Text.UTF8Encoding($false)))
        $vt.Touched=$true
        $vt.Proc=Start-Wiri (Join-Path $vtDir 'daemon.stderr.log') (Join-Path $vtDir 'daemon.stdout.log')
        $null=$vt.Proc.Handle # Retain the process identity/handle until cleanup is complete.
        VTAlive
        Start-Sleep -Seconds 2
        $prefix='wiri-h65-'+[Guid]::NewGuid().ToString('N').Substring(0,8)
        foreach ($i in 0..1) {
            $h=[TestForms]::Open(($prefix+'-'+$i),(70+60*$i),100,170)
            $vtForms.Add($h); VTOwnForm $h
            Start-Sleep -Milliseconds 1000
        }
        $vt.First=$vtForms[0]
        # Refuse consume if it could take a foreign tile. Inspect the legacy schema,
        # not niri's Windows result (which has id and no legacy column field).
        $list=@((VTCtl 'before-consume' @('--json','window-list') | ConvertFrom-Json).result)
        $a=WinOf $list $vtForms[0]; $b=WinOf $list $vtForms[1]
        if ($null -eq $a -or $null -eq $b -or (ColOf $list $vtForms[0]) -lt 0 -or
                (ColOf $list $vtForms[1]) -ne ((ColOf $list $vtForms[0])+1) -or $a.workspace -ne $b.workspace) {
            throw 'Owned probes are not adjacent tiled columns in the same workspace'
        }
        $foreign=@($list | Where-Object { $_.workspace -eq $a.workspace -and
            $null -ne $_.column -and [int]$_.column -ge 0 -and
            [int64]$_.hwnd -notin @([int64]$vtForms[0],[int64]$vtForms[1]) })
        if ($foreign.Count -ne 0) { throw 'Refused consume in a workspace containing foreign tiles' }
        FocusForm $vtForms[0]
        if (-not (WaitForeground $vtForms[0] 100 3000)) { throw 'First owned probe did not become foreground' }
        Start-Sleep -Milliseconds 350
        # Setup is stacked, so VTAction's one-visible-tab invariant does not apply yet.
        $null=VTCtl 'consume' @('action','consume-window-into-column')
        Start-Sleep -Milliseconds 500
        FocusForm $vtForms[1]
        if (-not (WaitForeground $vtForms[1] 100 3000)) { throw 'Second owned probe did not become foreground' }
        Start-Sleep -Milliseconds 350
        Chord 'ctrl+alt+f8'
        Start-Sleep -Milliseconds 650
        $boundTabbed=VTSelected $vtForms[1]
        if (-not (WaitForeground $vtForms[1] 100 3000)) { throw 'Owned probe lost foreground before normal bind' }
        Chord 'ctrl+alt+f9'; Start-Sleep -Milliseconds 500
        $boundNormal=@($vtForms | Where-Object { [LT]::IsWindowVisible($_) }).Count -eq 2
        if (-not (WaitForeground $vtForms[1] 100 3000)) { throw 'Owned probe lost foreground before tabbed bind' }
        Chord 'ctrl+alt+f8'; Start-Sleep -Milliseconds 500
        VTGate 'h65_set_display_bind' ($boundTabbed -and $boundNormal -and (VTSelected $vtForms[1])) @($boundTabbed,$boundNormal)
        $null=VTCtl 'set-tabbed' @('set-column-display','tabbed')
        $null=VTCtl 'set-tabbed-again' @('set-column-display','tabbed')
        Start-Sleep -Milliseconds 500
        VTGate 'h65_set_display_ctl_idempotent' (VTSelected $vtForms[1]) ([int64][LT]::GetForegroundWindow())
        $typedResults=@()
        foreach ($mode in @('Normal','Tabbed','Tabbed')) {
            $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$vt.Proc.Id
            try {
                $wire.Send(('{"Action":{"SetColumnDisplay":{"display":"'+$mode+'"}}}')+"`n")
                $typedReply=$wire.ReadLine(2000) | ConvertFrom-Json
                $typedResults += $null -ne $typedReply.PSObject.Properties['Ok']
            } finally { $wire.Dispose() }
            Start-Sleep -Milliseconds 400
            if ($mode -eq 'Normal') { $typedResults += @($vtForms | Where-Object { [LT]::IsWindowVisible($_) }).Count -eq 2 }
            else { $typedResults += VTSelected $vtForms[1] }
        }
        VTGate 'h65_set_display_typed_ipc' (-not ($typedResults -contains $false)) $typedResults
        $list=@((VTCtl 'paired' @('--json','window-list') | ConvertFrom-Json).result)
        $same=(ColOf $list $vtForms[0]) -ge 0 -and (ColOf $list $vtForms[0]) -eq (ColOf $list $vtForms[1])
        $paired=VTSnapshot 'paired'
        VTGate 'h65_owned_two_tab_column' ($same -and $paired.active -eq [int64]$vtForms[1] -and $paired.host -ne 0) $list
        if (-not $summary['h65_owned_two_tab_column']) { throw 'Failed to establish the owned two-tab column' }
        $sideResults=@(); $clickResults=@(); $ignoredResults=@(); $cornerResults=@(); $activationResults=@()
        foreach ($side in @('left','right','top','bottom')) {
            VTReload (VTConfig $side $false) ($side+'-outside')
            $outside=VTSnapshot ($side+'-outside')
            VTReload (VTConfig $side $true) ($side+'-inside')
            $inside=VTSnapshot ($side+'-inside')
            $reservation=VTReserved $outside.window $inside.window $side 20
            $horizontal=$side -in @('top','bottom')
            $band=if ($horizontal) { $inside.indicator.H } else { $inside.indicator.W }
            $along=if ($horizontal) { $inside.indicator.W } else { $inside.indicator.H }
            $areaLength=if ($horizontal) { $inside.window.W } else { $inside.window.H }
            $expectedLength=[Math]::Round(($areaLength*0.64),[MidpointRounding]::AwayFromZero)+4
            $position=switch ($side) {
                left { [Math]::Abs(($inside.indicator.R-2)-($inside.window.L-8)) -le 3 }
                right { [Math]::Abs(($inside.indicator.L+2)-($inside.window.R+8)) -le 3 }
                top { [Math]::Abs(($inside.indicator.B-2)-($inside.window.T-8)) -le 3 }
                bottom { [Math]::Abs(($inside.indicator.T+2)-($inside.window.B+8)) -le 3 }
            }
            $centered=if ($horizontal) {
                [Math]::Abs(($inside.indicator.L+$inside.indicator.R)-($inside.window.L+$inside.window.R)) -le 4
            } else { [Math]::Abs(($inside.indicator.T+$inside.indicator.B)-($inside.window.T+$inside.window.B)) -le 4 }
            VTGate ('h65_'+$side+'_indicator_geometry') ($inside.host -ne 0 -and $band -eq 16 -and
                [Math]::Abs($along-$expectedLength) -le 2 -and $position -and $centered) $inside
            $sideResults += $reservation
            $points=VTPoints $inside.indicator $side
            $current=VTActive
            # Only inter-tab gaps and the 2px AA margin reject clicks.
            foreach ($region in @('Gap','Margin')) {
                $ignored=VTClick $points.$region $current $false ($side+'-'+$region)
                $ignoredResults += ($ignored.selected -and $ignored.inputRegionCorrect)
                $activationResults += ($ignored.noActivateStyle -and $ignored.mouseActivate -eq 3)
            }
            # Pinned niri hit-tests rectangular tabs, including rounded cutouts.
            # Native alpha flooring keeps this first-tab corner hittable; it must
            # select the previously hidden first tab, not behave like the AA margin.
            $corner=VTClick $points.Corner $vtForms[0] $true ($side+'-corner-selects-tab')
            $cornerResults += ($corner.before -ne $corner.expected -and $corner.selected -and $corner.inputRegionCorrect)
            $activationResults += ($corner.noActivateStyle -and $corner.mouseActivate -eq 3)
            $current=VTActive
            $target=if ($current -eq $vtForms[0]) { 1 } else { 0 }
            $clicked=VTClick $points.Tabs[$target] $vtForms[$target] $true ($side+'-hidden-tab')
            $clickResults += ($clicked.before -ne $clicked.expected -and $clicked.selected -and $clicked.inputRegionCorrect)
            $activationResults += ($clicked.noActivateStyle -and $clicked.mouseActivate -eq 3)
        }
        VTGate 'h65_place_within_column_reserves_each_side' (-not ($sideResults -contains $false)) $sideResults
        VTGate 'h65_click_hidden_tab_and_focus' (-not ($clickResults -contains $false)) $clickResults
        VTGate 'h65_gap_and_margin_ignore_clicks' (-not ($ignoredResults -contains $false)) $ignoredResults
        VTGate 'h65_rounded_corners_select_tab' (-not ($cornerResults -contains $false)) $cornerResults
        VTGate 'h65_decor_never_activates' (-not $vt.DecorActivated -and -not ($activationResults -contains $false)) $activationResults
        $reloadPassed=$summary['h65_place_within_column_reserves_each_side']
        foreach ($side in @('left','right','top','bottom')) { $reloadPassed=$reloadPassed -and $summary['h65_'+$side+'_indicator_geometry'] }
        VTGate 'h65_reload_updates_indicator_and_reservation' $reloadPassed $sideResults
        VTReload (VTConfig) 'off-baseline'
        $on=VTSnapshot 'before-off'
        VTReload (VTConfig 'left' $true $true) 'off'
        $off=VTSnapshot 'off'
        VTReload (VTConfig) 'on-again'
        $again=VTSnapshot 'on-again'
        VTGate 'h65_off_removes_indicator_and_reservation' ($off.host -eq 0 -and $on.host -ne 0 -and
            $again.host -ne 0 -and $off.active -eq $on.active -and
            (VTReserved $off.window $on.window 'left' 20) -and (VTEqual $on.window $again.window)) @($on,$off,$again)
        $negativeChecks=@()
        foreach ($gap in @(-4,-20)) {
            VTReload (VTConfig 'left' $true $true $false $gap) ('negative-off-'+$gap)
            $base=VTSnapshot ('negative-off-'+$gap)
            VTReload (VTConfig 'left' $true $false $false $gap) ('negative-on-'+$gap)
            $inset=VTSnapshot ('negative-on-'+$gap)
            $negativeChecks += ($inset.host -ne 0 -and (VTReserved $base.window $inset.window 'left' ([Math]::Max(12+$gap,0))))
        }
        VTGate 'h65_negative_gap_reservation' (-not ($negativeChecks -contains $false)) $negativeChecks
        VTReload (VTConfig) 'solid-paints'
        $solid=VTSnapshot 'solid-paints'; $points=VTPoints $solid.indicator 'left'
        $solidChecks=@(); $solidSamples=@()
        foreach ($i in 0..1) {
            $rgb=@(VTRgb $points.Tabs[$i]); $solidSamples += ,$rgb
            $expected=if ([int64]$vtForms[$i] -eq $solid.active) { @(224,48,64) } else { @(32,176,80) }
            $solidChecks += (VTColorNear $rgb $expected)
        }
        VTGate 'h65_active_inactive_colors' (-not ($solidChecks -contains $false)) $solidSamples
        VTReload (VTConfig 'left' $true $false $false 8 'gradient') 'gradient-paints'
        $gradient=VTSnapshot 'gradient-paints'; $gp=VTPoints $gradient.indicator 'left'
        $gradientSamples=@(); $gradientChecks=@()
        foreach ($i in 0..1) {
            $c=$gp.Tabs[$i]
            $samples=@(foreach ($dx in @(-3,3)) { foreach ($dy in @(-5,5)) {
                ,@(VTRgb ([pscustomobject]@{X=($c.X+$dx);Y=($c.Y+$dy)}))
            } })
            $gradientSamples += ,$samples
            $varies=$false; $changed=$false
            foreach ($rgb in $samples) {
                for ($channel=0; $channel -lt 3; $channel++) {
                    if ([Math]::Abs($rgb[$channel]-$samples[0][$channel]) -gt 10) { $varies=$true }
                    if ([Math]::Abs($rgb[$channel]-$solidSamples[$i][$channel]) -gt 10) { $changed=$true }
                }
            }
            $gradientChecks += ($varies -and $changed)
        }
        VTGate 'h65_gradients_reload' (-not ($gradientChecks -contains $false) -and
            $gradient.active -eq $solid.active -and (VTEqual $gradient.window $solid.window)) $gradientSamples
        VTMode 'fullscreen-window' $true 'h65_fullscreen_suppresses_across_tab_switches'
        VTMode 'maximize-window-to-edges' $true 'h65_maximize_edges_suppresses_across_tab_switches'
        VTMode 'maximize-column' $false 'h65_maximize_column_keeps_indicator_and_reservation'
        VTReload (VTConfig 'left' $true $false $true) 'hide-single-before-close'
        $victim=$vtForms[1]; VTOwnForm $victim
        FocusForm $victim
        if (-not (WaitForeground $victim 100 3000)) { throw 'Owned second probe is not foreground for close' }
        if (-not (CloseFocused $victim 'lane-v-tabs-close')) { throw 'Owned second probe close chord refused' }
        for ($i=0; $i -lt 60 -and [LT]::IsWindow($victim); $i++) { Start-Sleep -Milliseconds 50 }
        if ([LT]::IsWindow($victim)) { throw 'Owned second probe did not close' }
        FocusForm $vtForms[0]
        if (-not (WaitForeground $vtForms[0] 100 3000)) { throw 'Surviving owned probe is not foreground' }
        Start-Sleep -Milliseconds 650
        $hidden=VTSnapshot 'single-hidden'
        VTReload (VTConfig 'left' $true $true $true) 'single-off-reference'
        $singleOff=VTSnapshot 'single-off-reference'
        VTGate 'h65_hide_single_removes_indicator_and_reservation' ($hidden.host -eq 0 -and
            $singleOff.host -eq 0 -and (VTEqual $hidden.window $singleOff.window)) @($hidden,$singleOff)
        VTReload (VTConfig) 'show-single'
        $shown=VTSnapshot 'single-shown'
        VTGate 'h65_single_tab_can_be_shown' ($shown.host -ne 0 -and $shown.active -eq [int64]$vtForms[0] -and
            (VTReserved $hidden.window $shown.window 'left' 20)) $shown
        $vt.Proc.Refresh()
        $summary['p_h65_daemon_peak_working_set_bytes']=[int64]$vt.Proc.PeakWorkingSet64
    } catch {
        $vtDetails['error']=$_ | Out-String
    } finally {
        $cleanupErrors=New-Object 'System.Collections.Generic.List[string]'
        try { VTStop } catch { $cleanupErrors.Add([string]$_) }
        foreach ($h in $vtForms) {
            try {
                if ([LT]::IsWindow($h)) { VTOwnForm $h; [TestForms]::CloseForm($h) }
            } catch { $cleanupErrors.Add([string]$_) }
        }
        for ($i=0; $i -lt 60; $i++) {
            if (@($vtForms | Where-Object { [LT]::IsWindow($_) }).Count -eq 0) { break }
            Start-Sleep -Milliseconds 50
        }
        try {
            if ($vt.Touched -and $null -ne $vt.Saved) { [IO.File]::WriteAllBytes($cfg,$vt.Saved) }
        } catch { $cleanupErrors.Add([string]$_) }
        $restored=$false
        try {
            if ($null -ne $vt.Saved) {
                $restored=[Convert]::ToBase64String([IO.File]::ReadAllBytes($cfg)) -eq [Convert]::ToBase64String($vt.Saved)
            }
        } catch { $cleanupErrors.Add([string]$_) }
        $daemonGone=$null -ne $vt.Proc -and $vt.Proc.HasExited
        $formsGone=@($vtForms | Where-Object { [LT]::IsWindow($_) }).Count -eq 0
        $hostsGone=$false
        if ($null -ne $vt.Proc) {
            $hostsGone=@([LT]::TopLevel() | Where-Object { [LT]::Pid($_) -eq $vt.Proc.Id -and
                [LT]::Class($_) -eq 'WiriDecor' -and [LT]::Title($_) -eq ('WiriDecor Tabs '+[string][int64]$vt.First) }).Count -eq 0
        }
        VTGate 'h65_cleanup' ($vt.Graceful -and $daemonGone -and $formsGone -and $hostsGone -and
            $restored -and $cleanupErrors.Count -eq 0) ([ordered]@{graceful=$vt.Graceful;
                daemonGone=$daemonGone; formsGone=$formsGone; hostsGone=$hostsGone;
                configRestoredByteForByte=$restored; errors=@($cleanupErrors.ToArray())})
        # Retain all initially-false gates in the artifact, even after an early refusal.
        $vtDetails['summary']=[ordered]@{}
        foreach ($key in $vtGates) { $vtDetails.summary[$key]=$summary[$key] }
        foreach ($key in @('p_h65_last_reload_roundtrip_ms','p_h65_click_focus_max_ms',
                'p_h65_daemon_peak_working_set_bytes')) { $vtDetails.summary[$key]=$summary[$key] }
        $vtDetails | ConvertTo-Json -Depth 18 | Set-Content -Encoding UTF8 (Join-Path $vtDir 'checks.json')
        if ($daemonGone) { $vt.Proc.Dispose() }
    }
}
