# Lane T / H58-H61. Parent owns Global\wiri-live-test and the isolated child profile.
# Mouse checks are real SendInput on our forms. Raw PTP is registration-only;
# DnD capture/kinetics and promoted touch/pen are unit-tested, NOT hardware/OLE evidence.
& {
    $tDir = Join-Path $D 'lane-t'
    $null = New-Item -ItemType Directory -Force $tDir
    $tDetails = [ordered]@{}
    $tOwned = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $tForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $tGates = @(
        'h58_middle_waits_for_axis_threshold', 'h58_horizontal_follows_pointer_while_held',
        'h58_release_snaps_and_settles', 'h58_vertical_switches_workspace',
        'h58_modifier_first_release_ends_grab',
        'h59_top_edge_double_resets_height', 'h59_side_edge_double_toggles_full_width',
        'h59_resize_preview_survives_held_refresh', 'h59_resize_persists_after_drop',
        'h59_move_drops_into_actual_destination_column', 'h59_right_click_during_move_toggles_float',
        'h60_dnd_host_absent_without_gestures', 'h60_dnd_host_configured_only',
        'h60_dnd_edge_scroll_moves_the_strip', 'h60_dnd_edge_switch_fires_once_after_the_delay',
        'h60_dnd_no_frame_timer_when_idle',
        'h61_raw_host_absent_without_touchpad', 'h61_raw_inputsink_registered_on_opt_in',
        'h61_raw_removed_on_off_reload', 'h61_raw_removed_on_quit'
    )
    foreach ($gate in $tGates) { $summary[$gate] = $false }
    $summary['h60_ole_evidence'] = 'unit-only capture heuristic; no real OLE drag injected'
    $summary['h61_hardware_evidence'] = 'PTP registration only; touch/pen synthetic unit tests only'
    function TGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $tDetails[$name] = [ordered]@{ pass=$pass; observed=$observed }
    }
    function TCtl([string]$tag, [string[]]$a) {
        $n = $steps.Count
        $result = Ctl $tag $a
        if ($steps.Count -ne $n+1 -or [string]$steps[$n].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $result
    }
    function TAction([string]$tag, [string[]]$a) {
        TCtl $tag (@('action')+$a) | Out-Null
        Start-Sleep -Milliseconds 850
    }
    function TFocus([IntPtr]$h) {
        if (-not $tForms.Contains($h)) { throw 'Not a Lane T form' }
        FocusForm $h
        if (-not (WaitForeground $h 0 3000)) { throw "Lane T form not foreground: $h" }
    }
    function TOpen([string]$name) {
        $h = [TestForms]::Open(('lane-t-'+$name),70,130,210)
        $tForms.Add($h); [LaneTNative]::Own($h)
        Start-Sleep -Milliseconds 1200
        TFocus $h
        return $h
    }
    function TClose {
        [LaneTNative]::ReleaseAll()
        foreach ($h in @($tForms.ToArray())) {
            if ([LT]::IsWindow($h)) {
                try { TFocus $h; $null = CloseFocused $h 't-close'; Start-Sleep -Milliseconds 350 }
                finally { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
            }
        }
        $tForms.Clear()
        Start-Sleep -Milliseconds 600
    }
    function TStop($p) {
        if ($null -eq $p -or -not $tOwned.Contains($p) -or $p.HasExited) { return }
        try { $tDetails['quit-'+$p.Id] = [LaneTNative]::QuitOwned($p.Id,(WiriPipeName)) }
        finally { if (-not $p.WaitForExit(3500)) { $p.Kill(); $null = $p.WaitForExit(5000) } }
    }
    function TConfig([bool]$raw=$false, [bool]$dnd=$false) {
        $rawBlock = if ($raw) { 'touchpad {}' } else { 'touchpad { off; }' }
        # Explicit zones and dwell, not the defaults: the H60 edge gates quote these numbers, so
        # a changed default must fail them instead of silently re-timing the fixture.
        $dndBlock = if ($dnd) {
            (@(
                'gestures {',
                '    dnd-edge-view-scroll { trigger-width 30; delay-ms 100; max-speed 1500; }',
                '    dnd-edge-workspace-switch { trigger-height 50; delay-ms 100; max-speed 1500; }',
                '}'
            ) -join "`n")
        } else { '' }
        return @"
input { mod-key "Alt"; $rawBlock }
animations {}
workspace "lane-t-outside"
workspace "lane-t"
workspace "lane-t-return"
layout {
    gaps 12
    center-focused-column "never"
    default-column-width { fixed 600; }
    border { width 1; }
    focus-ring { off; }
}
binds { Ctrl+Alt+Q { close-window; } }
window-rule { match title="^lane-t-"; open-on-workspace "lane-t"; }
$dndBlock
"@
    }
    function TWrite([string]$text) {
        [IO.File]::WriteAllText($cfg,$text,(New-Object Text.UTF8Encoding($false)))
    }
    function TReload([string]$text) { TWrite $text; Start-Sleep -Milliseconds 2800 }
    function TPoint([IntPtr]$h, [double]$x=0.6, [double]$y=0.5) {
        TFocus $h
        $r = Rc $h
        $pt = [Drawing.Point]::new([int]($r.L+$r.W*$x),[int]($r.T+$r.H*$y))
        [LaneTNative]::Move($pt.X,$pt.Y)
        Start-Sleep -Milliseconds 100
        if ([LT]::TopWindowAt($pt.X,$pt.Y) -ne $h) { throw 'Owned input target obscured' }
        return $pt
    }
    function TDrag([IntPtr]$h, [string]$button, [int]$x, [int]$y, [int]$dx, [int]$dy) {
        TFocus $h; [LaneTNative]::Move($x,$y); Start-Sleep -Milliseconds 80
        try {
            [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button($button,$false)
            Start-Sleep -Milliseconds 100
            foreach ($i in 1..8) {
                [LaneTNative]::Move(($x+[int]($dx*$i/8)),($y+[int]($dy*$i/8)))
                Start-Sleep -Milliseconds 35
            }
        } finally { [LaneTNative]::ReleaseAll() }
        Start-Sleep -Milliseconds 1600
    }
    function TDouble([IntPtr]$h, [bool]$side) {
        TFocus $h
        $r = Rc $h
        $x = if ($side) { $r.L+1 } else { $r.L+[int]($r.W/2) }
        $y = if ($side) { $r.T+[int]($r.H/2) } else { $r.T+1 }
        [LaneTNative]::Move($x,$y); Start-Sleep -Milliseconds 80
        try {
            [LaneTNative]::Key(0xA4,$false)
            [LaneTNative]::Button('R',$false); Start-Sleep -Milliseconds 40
            [LaneTNative]::Button('R',$true); Start-Sleep -Milliseconds 130 # the first click's grab release is queued on the dispatcher; stay far inside GetDoubleClickTime
            [LaneTNative]::Button('R',$false); Start-Sleep -Milliseconds 40
            [LaneTNative]::Button('R',$true)
        } finally { [LaneTNative]::ReleaseAll() }
        Start-Sleep -Milliseconds 1800
    }
    $tCursor = $null; $tProcess = $null
    try {
        Add-Type -ReferencedAssemblies System.Drawing,System.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class LaneTNative {
    [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk,scan; public uint flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct U { [FieldOffset(0)] public KI k; [FieldOffset(0)] public MI m; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public U u; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint n,INPUT[] events,int size);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h,uint flags);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr parent,IntPtr after,string cls,string title);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe,out uint pid);
    static readonly HashSet<IntPtr> owned = new HashSet<IntPtr>();
    static readonly List<int> keys = new List<int>();
    static readonly List<string> buttons = new List<string>();
    public static void Own(IntPtr h) { owned.Add(h); }
    static void Send(INPUT i) { if (SendInput(1,new INPUT[]{i},Marshal.SizeOf(typeof(INPUT)))!=1) throw new Win32Exception(); }
    static void RequireFocus() { if (!owned.Contains(GetForegroundWindow())) throw new Exception("Lane T refuses foreign foreground"); }
    public static Point Cursor() { Point p; if (!GetCursorPos(out p)) throw new Win32Exception(); return p; }
    public static void Key(int vk,bool up) {
        if (up && !keys.Contains(vk)) return;
        if (!up) { RequireFocus(); if (keys.Contains(vk)||GetAsyncKeyState(vk)<0) throw new Exception("Key already held"); }
        INPUT i=new INPUT(); i.type=1; i.u.k.vk=(ushort)vk; i.u.k.flags=up?2u:0u; Send(i);
        if (up) keys.Remove(vk); else keys.Add(vk);
    }
    public static void Button(string b,bool up) {
        if (up && !buttons.Contains(b)) return;
        int vk=b=="L"?1:b=="R"?2:b=="M"?4:0;
        if (vk==0) throw new ArgumentException("button");
        if (!up) {
            RequireFocus();
            if (!owned.Contains(GetAncestor(WindowFromPoint(Cursor()),2))) throw new Exception("Pointer not on owned form");
            if (buttons.Contains(b)||GetAsyncKeyState(vk)<0) throw new Exception("Button already held");
        }
        INPUT i=new INPUT(); i.u.m.flags=b=="L"?(up?4u:2u):b=="R"?(up?16u:8u):(up?64u:32u); Send(i);
        if (up) buttons.Remove(b); else buttons.Add(b);
    }
    public static void ReleaseAll() {
        Exception error=null;
        string[] bs=buttons.ToArray(); int[] ks=keys.ToArray();
        for (int j=bs.Length-1;j>=0;--j) try { Button(bs[j],true); } catch(Exception ex) { error=ex; }
        for (int j=ks.Length-1;j>=0;--j) try { Key(ks[j],true); } catch(Exception ex) { error=ex; }
        if (error!=null) throw error;
    }
    public static void Move(int x,int y) {
        int l=GetSystemMetrics(76),t=GetSystemMetrics(77),w=GetSystemMetrics(78),h=GetSystemMetrics(79);
        if (w<=0||h<=0||x<l||y<t||x>=l+w||y>=t+h) throw new Exception("Pointer outside existing desktop");
        INPUT i=new INPUT(); i.u.m.flags=0xC001u;
        i.u.m.x=(int)(((long)(x-l)*65536+32768)/w); i.u.m.y=(int)(((long)(y-t)*65536+32768)/h); Send(i);
    }
    public static IntPtr MessageWindow(string cls,int pid) {
        IntPtr previous=IntPtr.Zero;
        for (int j=0;j<512;++j) {
            IntPtr h=FindWindowEx(new IntPtr(-3),previous,cls,null);
            if (h==IntPtr.Zero) return IntPtr.Zero;
            uint owner; GetWindowThreadProcessId(h,out owner); if (owner==(uint)pid) return h;
            previous=h;
        }
        throw new Exception("Too many message windows");
    }
    public static string QuitOwned(int pid,string name) {
        using(var p=new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            p.Connect(800); uint server;
            if (!GetNamedPipeServerProcessId(p.SafePipeHandle,out server)) throw new Win32Exception();
            if (server!=(uint)pid) return "refused-unowned-server-"+server;
            byte[] bytes=Encoding.UTF8.GetBytes("{\"type\":\"quit\"}");
            var write=p.BeginWrite(bytes,0,bytes.Length,null,null);
            using(var done=write.AsyncWaitHandle) { if (!done.WaitOne(1500)) return "owned-quit-timeout"; p.EndWrite(write); }
            return "owned-quit-sent";
        }
    }
}
'@
        $tCursor = [LaneTNative]::Cursor()
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane T refuses an existing daemon' }
        TWrite (TConfig)
        $tLog = Join-Path $tDir 'stderr.log'
        $tProcess = Start-Wiri $tLog ($tLog+'.out')
        $tOwned.Add($tProcess); $null = $tProcess.Handle
        Start-Sleep -Seconds 4
        if ($tProcess.HasExited) { throw 'Lane T daemon exited' }
        TAction 't-workspace' @('focus-workspace','2')
        TGate 'h60_dnd_host_absent_without_gestures' ([LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id) -eq [IntPtr]::Zero) 'no gestures block'
        TGate 'h61_raw_host_absent_without_touchpad' ([LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id) -eq [IntPtr]::Zero) 'touchpad off'
        # Variable names: the harness helpers (Ctl, Shot, CloseFocused) read the caller's $D by
        # dynamic scope, and PowerShell variables are case-insensitive, so a form handle called
        # $d would turn every later Ctl output path into "<hwnd>\<step>.txt" (the first live run
        # died exactly there). Every handle below is therefore $tA..$tD.
        $tA = TOpen 'a'; $tB = TOpen 'b'; $tC = TOpen 'c'; $tD = TOpen 'd'

        # H58: direct manipulation, axis lock, release and ordered workspace selection.
        # Each section has its own try/catch: a failed step leaves its own and later gates false
        # with the reason in results.json, and still lets the H60/H61 lifecycle gates run.
        try {
            $tPt = TPoint $tB 0.65 0.5
            $tInitial = Rc $tB
            try {
                [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('M',$false)
                [LaneTNative]::Move(($tPt.X-3),$tPt.Y); Start-Sleep -Milliseconds 150
                $tThreshold = Rc $tB
                foreach ($i in 1..6) { [LaneTNative]::Move(($tPt.X-30*$i),$tPt.Y); Start-Sleep -Milliseconds 30 }
                Start-Sleep -Milliseconds 80
                $tHeld = Rc $tB
                # Below 8 px nothing may move, but an inert grab would pass that alone: the same
                # grab must then follow the pointer, or the threshold was never armed.
                TGate 'h58_middle_waits_for_axis_threshold' ((Near $tThreshold.L $tInitial.L 3) -and ($tHeld.L -lt $tInitial.L-100)) @($tInitial,$tThreshold,$tHeld)
                TGate 'h58_horizontal_follows_pointer_while_held' ((Near $tHeld.L ($tInitial.L-180) 10) -and (Near $tHeld.T $tInitial.T 3)) @($tInitial,$tHeld)
                Shot 'h58-middle-held'
                [LaneTNative]::Key(0xA4,$true) # paired up must still be captured
                [LaneTNative]::Button('M',$true)
            } finally { [LaneTNative]::ReleaseAll() }
            $tSamples = @()
            foreach ($i in 1..12) { $tSamples += Rc $tB; Start-Sleep -Milliseconds 120 }
            $tSettled = Rc $tB; Start-Sleep -Milliseconds 350; $tStill = Rc $tB
            TGate 'h58_release_snaps_and_settles' (([Math]::Abs($tSettled.L-$tHeld.L) -gt 8) -and (Near $tSettled.L $tStill.L 2)) @($tHeld,$tSamples,$tSettled,$tStill)
            [LaneTNative]::Move(($tPt.X-120),$tPt.Y); Start-Sleep -Milliseconds 200
            TGate 'h58_modifier_first_release_ends_grab' (Near (Rc $tB).L $tStill.L 2) @($tStill,(Rc $tB))
            $tPt = TPoint $tB 0.5 0.9
            TDrag $tB 'M' $tPt.X $tPt.Y 0 -420
            $tAway = (VisibleCount @($tA,$tB,$tC,$tD)) -eq 0
            TAction 't-return' @('focus-workspace','2')
            TGate 'h58_vertical_switches_workspace' ($tAway -and (VisibleCount @($tA,$tB,$tC,$tD)) -eq 4) @($tAway,(VisibleCount @($tA,$tB,$tC,$tD)))
            Shot 'h58-returned'
        } catch {
            $tDetails['error-h58'] = [string]$_
            $tDetails['error-h58-position'] = $_.InvocationInfo.PositionMessage
        }

        # H59: stack a,b and close only our unused fixtures through the harness helpers.
        try {
            # H58 may have stopped on another workspace; the fixtures live on lane-t (2).
            TAction 't-h59-workspace' @('focus-workspace','2')
            foreach ($h in @($tC,$tD)) { TFocus $h; $null = CloseFocused $h 't-reduce'; Start-Sleep -Milliseconds 500 }
            TFocus $tA; TAction 't-stack' @('consume-window-into-column')
            TAction 't-height' @('set-window-height','200')
            $tHeightBefore = Rc $tA; TDouble $tA $false; $tHeightAfter = Rc $tA
            TGate 'h59_top_edge_double_resets_height' (($tHeightAfter.H -gt $tHeightBefore.H+50) -and (Near $tHeightAfter.H (Rc $tB).H 5)) @($tHeightBefore,$tHeightAfter,(Rc $tB))
            $tWidthBefore = Rc $tA; TDouble $tA $true; $tWide = Rc $tA; TDouble $tA $true; $tNormal = Rc $tA
            TGate 'h59_side_edge_double_toggles_full_width' (($tWide.W -gt $tWidthBefore.W+200) -and (Near $tNormal.W $tWidthBefore.W 5)) @($tWidthBefore,$tWide,$tNormal)
            TFocus $tA; $tR = Rc $tA; $tX = $tR.R-2; $tY = $tR.T+[int]($tR.H/2)
            [LaneTNative]::Move($tX,$tY); Start-Sleep -Milliseconds 80
            try {
                [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('R',$false)
                Start-Sleep -Milliseconds 100
                [LaneTNative]::Move(($tX+90),$tY); Start-Sleep -Milliseconds 150; $tPreview = Rc $tA
                TAction 't-refresh-held' @('set-workspace-layout','scrolling'); $tHeldRefresh = Rc $tA
                TGate 'h59_resize_preview_survives_held_refresh' (($tPreview.W -gt $tR.W+60) -and (Near $tHeldRefresh.W $tPreview.W 4)) @($tR,$tPreview,$tHeldRefresh)
            } finally { [LaneTNative]::ReleaseAll() }
            Start-Sleep -Milliseconds 1500
            $tDrop = Rc $tA; TAction 't-refresh-drop' @('set-workspace-layout','scrolling'); $tRefresh = Rc $tA
            TGate 'h59_resize_persists_after_drop' ((Near $tDrop.W $tPreview.W 5) -and (Near $tRefresh.W $tDrop.W 3)) @($tPreview,$tDrop,$tRefresh)
            TAction 't-source-width' @('set-column-width','450')
            $tC = TOpen 'destination'; TAction 't-target-width' @('set-column-width','350')
            $tPt = TPoint $tA 0.6 0.4; $tTarget = Rc $tC
            # W compares vertical tile slots with column boundaries. Aim just inside
            # the target's top slot, not its centre (which may request a new column).
            TDrag $tA 'L' $tPt.X $tPt.Y ([int]($tTarget.L+$tTarget.W/2)-$tPt.X) ($tTarget.T+2-$tPt.Y)
            $tList = @((TCtl 't-drop-list' @('--json','window-list') | ConvertFrom-Json).result)
            TGate 'h59_move_drops_into_actual_destination_column' (((ColOf $tList $tA) -eq (ColOf $tList $tC)) -and ((ColOf $tList $tA) -ne (ColOf $tList $tB))) $tList
            $tPt = TPoint $tA
            try {
                [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('L',$false)
                [LaneTNative]::Move(($tPt.X+12),($tPt.Y+12)); Start-Sleep -Milliseconds 100
                [LaneTNative]::Button('R',$false); [LaneTNative]::Button('R',$true)
                Start-Sleep -Milliseconds 200
            } finally { [LaneTNative]::ReleaseAll() }
            Start-Sleep -Milliseconds 1500
            $tList = @((TCtl 't-toggle-list' @('--json','window-list') | ConvertFrom-Json).result)
            TGate 'h59_right_click_during_move_toggles_float' ([bool](WinOf $tList $tA).floating) (WinOf $tList $tA)
            Shot 'h59-final'
        } catch {
            $tDetails['error-h59'] = [string]$_
            $tDetails['error-h59-position'] = $_.InvocationInfo.PositionMessage
        }
        try { TClose } catch { $tDetails['error-close'] = [string]$_ }

        # H60: real drag-and-drop edge evidence. A held left button pressed on one of our own
        # forms and moved with SendInput (no Mod, so wiri sees a foreign capture drag like an
        # application's) drives the configured edges: the strip scrolls the way niri's does, a
        # dwell at the top/bottom edge steps the workspace once, and the one-shot frame timer
        # stops the moment the pointer leaves the zone and stays stopped. Direction and
        # once-only-ness are asserted; frame cadence never is, because the dwell timing is the
        # flaky part. Still no OLE claim: the capture heuristic is wiri's own.
        try {
            TReload (TConfig $false $true)
            $tDragOn = [LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id)
            $tScreen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
            $tEdgeX = $tScreen.Right - 4
            $tEdgeY = $tScreen.Bottom - 4
            $tMidX = $tScreen.Left + [int]($tScreen.Width/2)
            $tMidY = $tScreen.Top + [int]($tScreen.Height/2)
            $tE = TOpen 'edge-a'; $tF = TOpen 'edge-b'; $tG = TOpen 'edge-c'
            TAction 't-dnd-workspace' @('focus-workspace','2')
            $tStrip0 = Rc $tG
            $tFrames0 = (LogLines $tLog 'dnd edge frame').Count
            $tRight0 = (LogLines $tLog 'dnd edge frame dir=right').Count
            $tLeft0 = (LogLines $tLog 'dnd edge frame dir=left').Count
            $tPt = TPoint $tE 0.25 0.5
            try {
                [LaneTNative]::Button('L',$false)
                foreach ($i in 1..10) {
                    [LaneTNative]::Move(($tPt.X + [int](($tEdgeX-$tPt.X)*$i/10)), $tPt.Y)
                    Start-Sleep -Milliseconds 25
                }
                Start-Sleep -Milliseconds 160
                # Still inside the zone, but re-arm the one-shot timer a few times with a real
                # frame delta: a first frame shorter than half a pixel would otherwise latch the
                # axis off without the strip ever moving (the pointer itself stays in the zone).
                foreach ($i in 1..3) {
                    [LaneTNative]::Move($tEdgeX, ($tPt.Y + 2*$i))
                    Start-Sleep -Milliseconds 120
                }
                Start-Sleep -Milliseconds 320
                $tStripHeld = Rc $tG
                Shot 'h60-dnd-edge-scroll'
                $tFramesZone = (LogLines $tLog 'dnd edge frame').Count
                $tRightZone = (LogLines $tLog 'dnd edge frame dir=right').Count
                $tLeftZone = (LogLines $tLog 'dnd edge frame dir=left').Count
                # Same held button, away from every edge zone: the timer must be gone already.
                [LaneTNative]::Move($tMidX, $tMidY)
                Start-Sleep -Milliseconds 160
                $tFramesOut = (LogLines $tLog 'dnd edge frame').Count
                $tOut1 = Rc $tG; Start-Sleep -Milliseconds 140; $tOut2 = Rc $tG
            } finally { [LaneTNative]::ReleaseAll() }
            Start-Sleep -Milliseconds 300
            $tFramesIdle = (LogLines $tLog 'dnd edge frame').Count
            $tOut3 = Rc $tG
            TGate 'h60_dnd_edge_scroll_moves_the_strip' (($tDragOn -ne [IntPtr]::Zero) -and ($tStripHeld.L -lt $tStrip0.L-40) -and ($tRightZone -gt $tRight0) -and ($tLeftZone -eq $tLeft0)) @($tStrip0,$tStripHeld,$tRight0,$tRightZone,$tLeftZone)
            # The one-shot frame timer armed by the last in-zone move can fire
            # once after the leave-move is processed (a scheduling race between
            # the tick and the leave), so allow exactly one trailing frame.
            # The invariant that matters: no frames keep coming while the
            # pointer stays away, and the strip does not move on its own.
            TGate 'h60_dnd_no_frame_timer_when_idle' (($tFramesZone -gt $tFrames0) -and ($tFramesOut -le ($tFramesZone + 1)) -and ($tFramesIdle -eq $tFramesOut) -and (Near $tOut1.L $tOut2.L 2) -and (Near $tOut2.L $tOut3.L 2) -and (Near $tOut2.L $tStripHeld.L 2)) @($tFramesZone,$tFramesOut,$tFramesIdle,$tOut1,$tOut2,$tOut3)

            # A fresh drag straight down to the bottom edge: past delay-ms the workspace steps
            # once, in the down direction, and a stationary pointer never repeats it.
            $tWs0 = [string](TCtl 't-dnd-state' @('--json','state') | ConvertFrom-Json).result.active_workspace
            $tSteps0 = (LogLines $tLog 'dnd workspace step delta=1').Count
            $tBack0 = (LogLines $tLog 'dnd workspace step delta=-1').Count
            $tPt = TPoint $tG 0.4 0.5
            try {
                [LaneTNative]::Button('L',$false)
                foreach ($i in 1..10) {
                    [LaneTNative]::Move($tPt.X, ($tPt.Y + [int](($tEdgeY-$tPt.Y)*$i/10)))
                    Start-Sleep -Milliseconds 25
                }
                Start-Sleep -Milliseconds 1500
                $tWs1 = [string](TCtl 't-dnd-state-after' @('--json','state') | ConvertFrom-Json).result.active_workspace
                $tSteps1 = (LogLines $tLog 'dnd workspace step delta=1').Count
                $tAway = (VisibleCount @($tE,$tF,$tG)) -eq 0
            } finally { [LaneTNative]::ReleaseAll() }
            TAction 't-dnd-back' @('focus-workspace','2')
            $tWsBack = [string](TCtl 't-dnd-state-back' @('--json','state') | ConvertFrom-Json).result.active_workspace
            $tNext = 'workspace-' + ([int]($tWs0 -replace '^workspace-','') + 1)
            TGate 'h60_dnd_edge_switch_fires_once_after_the_delay' (($tSteps1-$tSteps0 -eq 1) -and ((LogLines $tLog 'dnd workspace step delta=-1').Count -eq $tBack0) -and ($tWs0 -ne $tWs1) -and ($tWs1 -eq $tNext) -and $tAway -and ($tWsBack -eq $tWs0) -and ((VisibleCount @($tE,$tF,$tG)) -eq 3)) @($tWs0,$tWs1,$tNext,$tSteps0,$tSteps1,$tAway,$tWsBack)
            Shot 'h60-dnd-back'
        } catch {
            $tDetails['error-h60-dnd'] = [string]$_
            $tDetails['error-h60-dnd-position'] = $_.InvocationInfo.PositionMessage
        }
        try { TClose } catch { $tDetails['error-h60-dnd-close'] = [string]$_ }

        # H60 lifecycle / H61: HWND lifecycle only, deliberately no shell settings/HID injection/
        # OLE claims.
        try {
            TReload (TConfig $true $true)
            $tRaw = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
            $tDragHost = [LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id)
            $tRegistered = (LogLines $tLog 'Precision touchpad Raw Input registered').Count
            TGate 'h61_raw_inputsink_registered_on_opt_in' (($tRaw -ne [IntPtr]::Zero) -and $tRegistered -gt 0) @([int64]$tRaw,$tRegistered)
            TReload (TConfig $false $false)
            $tRawOff = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
            $tDragOff = [LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id)
            TGate 'h60_dnd_host_configured_only' (($tDragHost -ne [IntPtr]::Zero) -and $tDragOff -eq [IntPtr]::Zero) @([int64]$tDragHost,[int64]$tDragOff)
            TGate 'h61_raw_removed_on_off_reload' (($tRawOff -eq [IntPtr]::Zero) -and (-not [LT]::IsWindow($tRaw)) -and (LogLines $tLog 'Precision touchpad Raw Input removed').Count -gt 0) @([int64]$tRawOff,(LogLines $tLog 'Precision touchpad Raw Input removed').Count)
            TReload (TConfig $true $false)
            $tRawQuit = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
            TStop $tProcess
            TGate 'h61_raw_removed_on_quit' (($tRawQuit -ne [IntPtr]::Zero) -and (-not [LT]::IsWindow($tRawQuit)) -and $tProcess.HasExited) @([int64]$tRawQuit,$tProcess.HasExited)
        } catch {
            $tDetails['error-h60-h61'] = [string]$_
            $tDetails['error-h60-h61-position'] = $_.InvocationInfo.PositionMessage
        }
    } catch {
        $tDetails['error'] = [string]$_
        $tDetails['error-position'] = $_.InvocationInfo.PositionMessage
    } finally {
        try { TClose } catch { $tDetails['cleanup-error'] = [string]$_ }
        foreach ($p in $tOwned) { try { TStop $p } catch { $tDetails['stop-error-'+$p.Id] = [string]$_ } }
        if ($null -ne $tCursor) { [LaneTNative]::Move($tCursor.X,$tCursor.Y) }
        $tDetails | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $tDir 'results.json')
    }
}
