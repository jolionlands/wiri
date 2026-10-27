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
        $dndBlock = if ($dnd) { 'gestures {}' } else { '' }
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
        TFocus $h; [LaneTNative]::Move($x,$y)
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
        [LaneTNative]::Move($x,$y)
        try {
            [LaneTNative]::Key(0xA4,$false)
            [LaneTNative]::Button('R',$false); Start-Sleep -Milliseconds 40
            [LaneTNative]::Button('R',$true); Start-Sleep -Milliseconds 60
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
        $a = TOpen 'a'; $b = TOpen 'b'; $c = TOpen 'c'; $d = TOpen 'd'

        # H58: direct manipulation, axis lock, release and ordered workspace selection.
        $pt = TPoint $b 0.65 0.5
        $initial = Rc $b
        try {
            [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('M',$false)
            [LaneTNative]::Move(($pt.X-3),$pt.Y); Start-Sleep -Milliseconds 150
            $threshold = Rc $b
            TGate 'h58_middle_waits_for_axis_threshold' (Near $threshold.L $initial.L 3) @($initial,$threshold)
            foreach ($i in 1..6) { [LaneTNative]::Move(($pt.X-30*$i),$pt.Y); Start-Sleep -Milliseconds 30 }
            Start-Sleep -Milliseconds 80
            $held = Rc $b
            TGate 'h58_horizontal_follows_pointer_while_held' ((Near $held.L ($initial.L-180) 10) -and (Near $held.T $initial.T 3)) @($initial,$held)
            Shot 'h58-middle-held'
            [LaneTNative]::Key(0xA4,$true) # paired up must still be captured
            [LaneTNative]::Button('M',$true)
        } finally { [LaneTNative]::ReleaseAll() }
        $samples = @()
        foreach ($i in 1..12) { $samples += Rc $b; Start-Sleep -Milliseconds 120 }
        $settled = Rc $b; Start-Sleep -Milliseconds 350; $still = Rc $b
        TGate 'h58_release_snaps_and_settles' (([Math]::Abs($settled.L-$held.L) -gt 8) -and (Near $settled.L $still.L 2)) @($held,$samples,$settled,$still)
        [LaneTNative]::Move(($pt.X-120),$pt.Y); Start-Sleep -Milliseconds 200
        TGate 'h58_modifier_first_release_ends_grab' (Near (Rc $b).L $still.L 2) @($still,(Rc $b))
        $pt = TPoint $b 0.5 0.9
        TDrag $b 'M' $pt.X $pt.Y 0 -420
        $away = (VisibleCount @($a,$b,$c,$d)) -eq 0
        TAction 't-return' @('focus-workspace','2')
        TGate 'h58_vertical_switches_workspace' ($away -and (VisibleCount @($a,$b,$c,$d)) -eq 4) @($away,(VisibleCount @($a,$b,$c,$d)))
        Shot 'h58-returned'

        # H59: stack a,b and close only our unused fixtures through the harness helpers.
        foreach ($h in @($c,$d)) { TFocus $h; $null = CloseFocused $h 't-reduce'; Start-Sleep -Milliseconds 500 }
        TFocus $a; TAction 't-stack' @('consume-window-into-column')
        TAction 't-height' @('set-window-height','200')
        $heightBefore = Rc $a; TDouble $a $false; $heightAfter = Rc $a
        TGate 'h59_top_edge_double_resets_height' (($heightAfter.H -gt $heightBefore.H+50) -and (Near $heightAfter.H (Rc $b).H 5)) @($heightBefore,$heightAfter,(Rc $b))
        $widthBefore = Rc $a; TDouble $a $true; $wide = Rc $a; TDouble $a $true; $normal = Rc $a
        TGate 'h59_side_edge_double_toggles_full_width' (($wide.W -gt $widthBefore.W+200) -and (Near $normal.W $widthBefore.W 5)) @($widthBefore,$wide,$normal)
        TFocus $a; $r = Rc $a; $x=$r.R-2; $y=$r.T+[int]($r.H/2)
        [LaneTNative]::Move($x,$y)
        try {
            [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('R',$false)
            Start-Sleep -Milliseconds 100
            [LaneTNative]::Move(($x+90),$y); Start-Sleep -Milliseconds 150; $preview = Rc $a
            TAction 't-refresh-held' @('set-workspace-layout','scrolling'); $heldRefresh = Rc $a
            TGate 'h59_resize_preview_survives_held_refresh' (($preview.W -gt $r.W+60) -and (Near $heldRefresh.W $preview.W 4)) @($r,$preview,$heldRefresh)
        } finally { [LaneTNative]::ReleaseAll() }
        Start-Sleep -Milliseconds 1500
        $drop = Rc $a; TAction 't-refresh-drop' @('set-workspace-layout','scrolling'); $refresh = Rc $a
        TGate 'h59_resize_persists_after_drop' ((Near $drop.W $preview.W 5) -and (Near $refresh.W $drop.W 3)) @($preview,$drop,$refresh)
        TAction 't-source-width' @('set-column-width','450')
        $c = TOpen 'destination'; TAction 't-target-width' @('set-column-width','350')
        $pt = TPoint $a 0.6 0.4; $target = Rc $c
        # W compares vertical tile slots with column boundaries. Aim just inside
        # the target's top slot, not its centre (which may request a new column).
        TDrag $a 'L' $pt.X $pt.Y ([int]($target.L+$target.W/2)-$pt.X) ($target.T+2-$pt.Y)
        $list = @((TCtl 't-drop-list' @('--json','window-list') | ConvertFrom-Json).result)
        TGate 'h59_move_drops_into_actual_destination_column' (((ColOf $list $a) -eq (ColOf $list $c)) -and ((ColOf $list $a) -ne (ColOf $list $b))) $list
        $pt = TPoint $a
        try {
            [LaneTNative]::Key(0xA4,$false); [LaneTNative]::Button('L',$false)
            [LaneTNative]::Move(($pt.X+12),($pt.Y+12)); Start-Sleep -Milliseconds 100
            [LaneTNative]::Button('R',$false); [LaneTNative]::Button('R',$true)
            Start-Sleep -Milliseconds 200
        } finally { [LaneTNative]::ReleaseAll() }
        Start-Sleep -Milliseconds 1500
        $list = @((TCtl 't-toggle-list' @('--json','window-list') | ConvertFrom-Json).result)
        TGate 'h59_right_click_during_move_toggles_float' ([bool](WinOf $list $a).floating) (WinOf $list $a)
        Shot 'h59-final'
        TClose

        # H60-H61: lifecycle evidence only, deliberately no shell settings/HID injection/OLE claims.
        TReload (TConfig $true $true)
        $raw = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
        $dragHost = [LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id)
        $registered = (LogLines $tLog 'Precision touchpad Raw Input registered').Count
        TGate 'h61_raw_inputsink_registered_on_opt_in' (($raw -ne [IntPtr]::Zero) -and $registered -gt 0) @([int64]$raw,$registered)
        TReload (TConfig $false $false)
        $rawOff = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
        $dragOff = [LaneTNative]::MessageWindow('WiriDragGestures',$tProcess.Id)
        TGate 'h60_dnd_host_configured_only' (($dragHost -ne [IntPtr]::Zero) -and $dragOff -eq [IntPtr]::Zero) @([int64]$dragHost,[int64]$dragOff)
        TGate 'h61_raw_removed_on_off_reload' (($rawOff -eq [IntPtr]::Zero) -and (-not [LT]::IsWindow($raw)) -and (LogLines $tLog 'Precision touchpad Raw Input removed').Count -gt 0) @([int64]$rawOff,(LogLines $tLog 'Precision touchpad Raw Input removed').Count)
        TReload (TConfig $true $false)
        $rawQuit = [LaneTNative]::MessageWindow('WiriTouchpadRawInput',$tProcess.Id)
        TStop $tProcess
        TGate 'h61_raw_removed_on_quit' (($rawQuit -ne [IntPtr]::Zero) -and (-not [LT]::IsWindow($rawQuit)) -and $tProcess.HasExited) @([int64]$rawQuit,$tProcess.HasExited)
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
