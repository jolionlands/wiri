# Lane G / H40-H44: strip policies, width intent, presets, floats and native grabs.
# Runs under live-test.ps1's desktop mutex and isolated child profile. Never changes display modes.
& {
    $gDir = Join-Path $D 'lane-g'
    $null = New-Item -ItemType Directory -Force $gDir
    $gDetails = [ordered]@{}
    $gOwned = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $gForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $gGates = @(
        'h40_never_keeps_visible_columns_still', 'h40_always_centres_first_middle_last',
        'h40_on_overflow_only_centres_when_needed', 'h40_always_center_single_column',
        'h40_gaps_and_signed_struts_compose',
        'h41_inserts_right_of_active_middle_end_empty', 'h41_widths_survive_open_close',
        'h41_column_remembers_active_stacked_tile', 'h41_workspace_remembers_active_column_and_tile',
        'h42_mixed_presets_forward_and_wrap', 'h42_mixed_presets_backward_and_wrap',
        'h42_nonpreset_steps_to_next_wider',
        'h43_normal_toplevels_stay_tiled', 'h43_owned_dialog_auto_floats_over_owner',
        'h43_float_does_not_scroll_with_tiles', 'h43_float_rectangle_survives_workspace_and_retile',
        'h44_caption_enters_native_loop', 'h44_caption_not_repositioned_during_loop',
        'h44_caption_drop_snaps_back_to_same_slot', 'h44_edge_enters_native_loop',
        'h44_edge_not_repositioned_during_loop', 'h44_edge_width_survives_refresh',
        'h44_stacked_edge_height_and_siblings_survive_refresh'
    )
    foreach ($gate in $gGates) { $summary[$gate] = $false }
    function GGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $gDetails[$name] = [ordered]@{ pass=$pass; observed=$observed }
    }
    function GCtl([string]$tag, [string[]]$a) {
        $n = $steps.Count
        $result = Ctl $tag $a
        if ($steps.Count -ne $n+1 -or [string]$steps[$n].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $result
    }
    function GAction([string]$tag, [string[]]$a) {
        GCtl $tag (@('action')+$a) | Out-Null
        Start-Sleep -Milliseconds 650
    }
    function GWindows([string]$tag) { return @((GCtl $tag @('--json','window-list') | ConvertFrom-Json).result) }
    function GFocus([IntPtr]$h) {
        GCtl ('g-focus-'+[int64]$h) @('focus-window',[string][int64]$h) | Out-Null
        Start-Sleep -Milliseconds 700
        if ([LT]::GetForegroundWindow() -ne $h) { throw "Owned form is not foreground: $h" }
    }
    function GOpen([string]$suffix) {
        $h = [TestForms]::Open(('lane-g-'+$suffix),65,140,200)
        $gForms.Add($h)
        Start-Sleep -Milliseconds 1100
        GFocus $h
        return $h
    }
    function GClose {
        foreach ($h in $gForms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
        $gForms.Clear()
        [LaneGNative]::CloseAll()
        Start-Sleep -Milliseconds 900
    }
    function GStop($p) {
        if ($null -eq $p -or -not $gOwned.Contains($p) -or $p.HasExited) { return }
        try { $gDetails['quit-'+$p.Id] = [LaneGNative]::QuitOwned($p.Id) }
        finally { if (-not $p.WaitForExit(3500)) { $p.Kill(); $null = $p.WaitForExit(5000) } }
        Start-Sleep -Milliseconds 500
    }
    function GConfig([string]$mode='never', [bool]$single=$false) {
        $singleSetting = if ($single) { 'always-center-single-column' } else { '' }
        return @"
animations { off }
workspace "lane-g-outside"
workspace "lane-g"
workspace "lane-g-return"
layout {
    gaps 12
    struts { left -6; right 30; top 14; bottom 20; }
    center-focused-column "$mode"
    $singleSetting
    default-column-width { fixed $gFixed; }
    preset-column-widths { fixed $gSmall; proportion 0.5; fixed $gLarge; }
    border { width 1; }
    focus-ring { off; }
}
window-rule {
    match title="^lane-g-"
    open-on-workspace "lane-g"
}
"@
    }
    function GStart([string]$mode='never', [bool]$single=$false) {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane G refuses an existing wiri instance' }
        [IO.File]::WriteAllText($cfg,(GConfig $mode $single),(New-Object Text.UTF8Encoding($false)))
        $log = Join-Path $gDir ($mode+'-'+$single+'.stderr.log')
        $p = Start-Wiri $log ($log+'.out')
        $gOwned.Add($p)
        $null = $p.Handle
        Start-Sleep -Seconds 4
        if ($p.HasExited) { throw 'Lane G wiri exited at startup' }
        GAction 'g-workspace-start' @('focus-workspace','2')
        return $p
    }
    function GSame($a,$b,[int]$tol=4) { return (Near $a.L $b.L $tol) -and (Near $a.T $b.T $tol) -and (Near $a.W $b.W $tol) -and (Near $a.H $b.H $tol) }
    function GCentered($r) { return (Near ($r.L+$r.W/2) ($gLeft+$gView/2) 4) }
    $gCursor = $null
    try {
        Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing,System.Core -TypeDefinition @'
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
public static class LaneGNative {
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct U { [FieldOffset(0)] public MI m; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public U u; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint n, INPUT[] i, int size);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int key);
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SendMessageTimeout(IntPtr h,uint m,IntPtr w,IntPtr l,uint flags,uint ms,out UIntPtr result);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe,out uint pid);
    [StructLayout(LayoutKind.Sequential)] struct R { public int l,t,r,b; }
    [StructLayout(LayoutKind.Sequential)] struct WP { public IntPtr hwnd,after; public int x,y,cx,cy; public uint flags; }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out R r);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h,int attr,out R r,int size);
    [DllImport("user32.dll")] static extern bool SystemParametersInfo(uint action,uint p,out int value,uint flags);
    public static int FullWindowDragging() { int value; return SystemParametersInfo(0x26,0,out value,0) ? value : -1; }
    sealed class Probe : Form {
        public volatile bool inLoop; public int starts,ends,sizes,moves,positions;
        public int left,top,width,height,ml,mt,mr,mb;
        protected override void WndProc(ref Message m) {
            if (m.Msg == 0x231) {
                R raw,visible;
                if (!GetWindowRect(Handle,out raw)) throw new Win32Exception();
                if (DwmGetWindowAttribute(Handle,9,out visible,Marshal.SizeOf(typeof(R)))!=0) visible=raw;
                ml=visible.l-raw.l; mt=visible.t-raw.t; mr=raw.r-visible.r; mb=raw.b-visible.b;
                left=visible.l; top=visible.t; width=visible.r-visible.l; height=visible.b-visible.t;
                positions=0; inLoop=true; Interlocked.Increment(ref starts);
            }
            if (m.Msg == 0x232) { inLoop=false; Interlocked.Increment(ref ends); }
            if (m.Msg == 0x214 || m.Msg == 0x216) {
                R r=(R)Marshal.PtrToStructure(m.LParam,typeof(R));
                left=r.l+ml; top=r.t+mt; width=r.r-r.l-ml-mr; height=r.b-r.t-mt-mb;
                if (m.Msg == 0x214) Interlocked.Increment(ref sizes);
                else Interlocked.Increment(ref moves);
            }
            if (inLoop && m.Msg == 0x46) {
                WP p=(WP)Marshal.PtrToStructure(m.LParam,typeof(WP));
                if ((p.flags & 3)!=3) Interlocked.Increment(ref positions);
            }
            base.WndProc(ref m);
        }
    }
    static readonly Dictionary<IntPtr,Probe> forms = new Dictionary<IntPtr,Probe>();
    static bool down;
    public static Point Cursor() { Point p; if (!GetCursorPos(out p)) throw new Exception("GetCursorPos"); return p; }
    public static void Move(int x,int y) { if (!SetCursorPos(x,y)) throw new Exception("SetCursorPos"); }
    public static void Button(bool up) {
        if (!up && (GetAsyncKeyState(1)<0 || down)) throw new Exception("Refuse already-held left button");
        if (up && !down) return;
        INPUT i=new INPUT(); i.u.m.flags=up ? 4u : 2u;
        if (SendInput(1,new INPUT[]{i},Marshal.SizeOf(typeof(INPUT)))!=1) throw new Win32Exception(Marshal.GetLastWin32Error());
        down=!up;
    }
    public static int Hit(IntPtr h,int x,int y) {
        UIntPtr result;
        if (SendMessageTimeout(h,0x84,IntPtr.Zero,new IntPtr((y<<16)|(x&65535)),2,500,out result)==IntPtr.Zero) return -1;
        return unchecked((int)result.ToUInt64());
    }
    public static IntPtr Open(string name) {
        IntPtr h=IntPtr.Zero; var ready=new ManualResetEvent(false);
        var t=new Thread(delegate() {
            var f=new Probe(); f.Text=name; f.Size=new Size(480,360); f.BackColor=Color.CadetBlue;
            f.Shown+=delegate { h=f.Handle; lock(forms) { forms.Add(h,f); } ready.Set(); };
            Application.Run(f);
        });
        t.IsBackground=true; t.SetApartmentState(ApartmentState.STA); t.Start();
        if (!ready.WaitOne(15000)) throw new TimeoutException("Native probe did not open");
        return h;
    }
    public static int[] State(IntPtr h) {
        lock(forms) { var f=forms[h]; return new int[]{f.inLoop?1:0,f.starts,f.ends,f.moves,f.sizes,f.positions}; }
    }
    public static int[] Proposal(IntPtr h) {
        lock(forms) { var f=forms[h]; return new int[]{f.left,f.top,f.width,f.height,f.ml,f.mt,f.mr,f.mb}; }
    }
    public static void CloseAll() {
        Button(true);
        lock(forms) { foreach(var f in forms.Values) { if (!f.IsDisposed) f.BeginInvoke((MethodInvoker)delegate { f.Close(); }); } }
    }
    public static string QuitOwned(int pid) {
        string name=Environment.GetEnvironmentVariable("WIRI_PIPE");
        if (String.IsNullOrWhiteSpace(name)) name="wiri_control_"+
            System.Text.RegularExpressions.Regex.Replace(Environment.UserName,"[^a-zA-Z0-9_.-]","_")+"_"+
            System.Diagnostics.Process.GetCurrentProcess().SessionId;
        if (name.StartsWith(@"\\.\pipe\")) name=name.Substring(9);
        using(var p=new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            p.Connect(800); uint server;
            if (!GetNamedPipeServerProcessId(p.SafePipeHandle,out server)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (server!=(uint)pid) return "refused-unowned-server-"+server;
            byte[] bytes=Encoding.UTF8.GetBytes("{\"type\":\"quit\"}");
            var write=p.BeginWrite(bytes,0,bytes.Length,null,null);
            using(var done=write.AsyncWaitHandle) { if (!done.WaitOne(1500)) return "owned-quit-timeout"; p.EndWrite(write); }
            return "owned-quit-sent";
        }
    }
}
'@
        $gCursor = [LaneGNative]::Cursor()
        $gDetails['full-window-dragging'] = [LaneGNative]::FullWindowDragging()
        # All expected geometry derives from this display's current work area, not its resolution.
        $gScale = [LaneD]::Scale([IntPtr]::Zero)
        $gView = $swa.Width - (-6+30) - 24
        $gLeft = $swa.Left - 6 + 12
        $gFixed = [int][Math]::Floor($gView*0.29/$gScale)
        $gSmall = [int][Math]::Floor($gView*0.22/$gScale)
        $gLarge = [int][Math]::Floor($gView*0.72/$gScale)
        # The explicit one-pixel tile border above is independent of decoration defaults.
        $gExtra = 2
        $gProportionWidth = [int][Math]::Round(($gView+12)*0.5)-12-$gExtra
        $gExpected = @([int][Math]::Round($gSmall*$gScale),$gProportionWidth,[int][Math]::Round($gLarge*$gScale))

        # H40: policy matrix, first/middle/last, and the single-column override.
        foreach ($mode in @('never','always','on-overflow')) {
            $p = $null
            try {
                $p = GStart $mode
                $a = GOpen ($mode+'-a'); $ra = Rc $a
                $b = GOpen ($mode+'-b'); $rb = Rc $b
                $c = GOpen ($mode+'-c'); $rc = Rc $c
                if ($mode -eq 'never') {
                    $start = @(Rc $a; Rc $b; Rc $c)
                    GFocus $b; $after = @(Rc $a; Rc $b; Rc $c)
                    GGate 'h40_never_keeps_visible_columns_still' ((GSame $start[0] $after[0]) -and (GSame $start[1] $after[1]) -and (GSame $start[2] $after[2])) @($start,$after)
                    GGate 'h40_gaps_and_signed_struts_compose' ((Near $start[0].L ($gLeft+1) 3) -and (Near ($start[1].L-$start[0].R) 14 3) -and (Near $start[0].T ($swa.Top+14+12+1) 3) -and (Near $start[0].B ($swa.Bottom-20-12-1) 3)) $start
                } elseif ($mode -eq 'always') {
                    $centres = @()
                    foreach ($h in @($a,$b,$c)) { GFocus $h; $centres += Rc $h }
                    GGate 'h40_always_centres_first_middle_last' ((GCentered $ra) -and (GCentered $centres[0]) -and (GCentered $centres[1]) -and (GCentered $centres[2])) @($ra,$centres)
                } else {
                    $before = Rc $b; GFocus $b; $still = Rc $b
                    GFocus $c; $overflow = GOpen 'overflow-d'; $rd = Rc $overflow
                    GGate 'h40_on_overflow_only_centres_when_needed' ((GSame $before $still) -and (GCentered $rd)) @($before,$still,$rd)
                }
                Shot ('h40-'+$mode)
            } finally { GClose; GStop $p }
        }
        $p = $null
        try {
            $p = GStart 'never' $true
            $a = GOpen 'single'; $r = Rc $a
            GGate 'h40_always_center_single_column' (GCentered $r) $r
            Shot 'h40-single'
        } finally { GClose; GStop $p }

        # H41-H43: keep one strip alive while inserting, cycling, stacking and floating.
        $p = $null
        try {
            $p = GStart
            $a = GOpen 'strip-a'; $wa = (Rc $a).W
            $b = GOpen 'strip-b'; $c = GOpen 'strip-c'
            GFocus $a; GAction 'g-width-proportion' @('set-column-width','40%'); $wp = (Rc $a).W
            $inserted = GOpen 'insert-middle'
            $list = GWindows 'g-insert-middle'
            $middle = ((ColOf $list $inserted) -eq ((ColOf $list $a)+1)) -and ((ColOf $list $b) -eq ((ColOf $list $inserted)+1))
            GFocus $c; $e = GOpen 'insert-end'
            $list = GWindows 'g-insert-end'
            $end = ((ColOf $list $e) -eq ((ColOf $list $c)+1))
            $gDetails['admission-width'] = $wa
            $persistent = (Near (Rc $a).W $wp 3) -and (Near (Rc $b).W ($gFixed*$gScale) 3)
            [TestForms]::CloseForm($e); [TestForms]::CloseForm($inserted); Start-Sleep -Milliseconds 1000
            GFocus $a
            GGate 'h41_widths_survive_open_close' ($persistent -and (Near (Rc $a).W $wp 3) -and (Near (Rc $b).W ($gFixed*$gScale) 3)) @($wp,(Rc $a),(Rc $b))
            GGate 'h41_inserts_right_of_active_middle_end_empty' ($middle -and $end -and (ColOf $list $a) -eq 0) $list
            # a | b | c -> (a,b) | c. Remember b, not the first tile a.
            GAction 'g-stack' @('consume-window-into-column')
            GFocus $b; GAction 'g-focus-next-column' @('focus-column-right'); GAction 'g-focus-previous-column' @('focus-column-left')
            GGate 'h41_column_remembers_active_stacked_tile' ([LT]::GetForegroundWindow() -eq $b) ([int64][LT]::GetForegroundWindow())
            GAction 'g-workspace-away' @('focus-workspace','3'); GAction 'g-workspace-back' @('focus-workspace','2')
            GGate 'h41_workspace_remembers_active_column_and_tile' ([LT]::GetForegroundWindow() -eq $b) ([int64][LT]::GetForegroundWindow())
            GFocus $c
            $forward=@(); $backward=@()
            GAction 'g-preset-start' @('set-column-width',[string]$gSmall)
            foreach ($i in 0..2) { GAction ('g-preset-forward-'+$i) @('switch-preset-column-width'); $forward += (Rc $c).W }
            foreach ($i in 0..2) { GAction ('g-preset-back-'+$i) @('switch-preset-column-width-back'); $backward += (Rc $c).W }
            GGate 'h42_mixed_presets_forward_and_wrap' ((Near $forward[0] $gExpected[1] 3) -and (Near $forward[1] $gExpected[2] 3) -and (Near $forward[2] $gExpected[0] 3)) @{expected=$gExpected; actual=$forward}
            GGate 'h42_mixed_presets_backward_and_wrap' ((Near $backward[0] $gExpected[2] 3) -and (Near $backward[1] $gExpected[1] 3) -and (Near $backward[2] $gExpected[0] 3)) @{expected=$gExpected; actual=$backward}
            $between = [int][Math]::Floor(($gExpected[0]+$gExpected[1])/2/$gScale)
            GAction 'g-nonpreset-width' @('set-column-width',[string]$between)
            GAction 'g-nonpreset-next' @('switch-preset-column-width')
            GGate 'h42_nonpreset_steps_to_next_wider' (Near (Rc $c).W $gExpected[1] 3) (Rc $c)
            $list = GWindows 'g-normal-toplevels'
            GGate 'h43_normal_toplevels_stay_tiled' ((WinOf $list $a).floating -eq $false -and (WinOf $list $b).floating -eq $false -and (WinOf $list $c).floating -eq $false) $list
            # A true owned top-level, not a tool/helper. It should float above its owner.
            GFocus $a
            $dialog = [TestForms]::OpenOwned($a,'lane-g-owned-dialog'); $gForms.Add($dialog)
            Start-Sleep -Milliseconds 1500
            $list = GWindows 'g-dialog'; $dr = Rc $dialog; $ownerRect = Rc $a
            GGate 'h43_owned_dialog_auto_floats_over_owner' ((WinOf $list $dialog).floating -eq $true -and (WinOf $list $dialog).workspace -eq (WinOf $list $a).workspace -and (Near ($dr.L+$dr.W/2) ($ownerRect.L+$ownerRect.W/2) 8) -and [LT]::TopWindowAt(($dr.L+$dr.W/2),($dr.T+$dr.H/2)) -eq $dialog) @($dr,$ownerRect,$list)
            [TestForms]::CloseForm($dialog); Start-Sleep -Milliseconds 800
            GFocus $b; GAction 'g-float' @('toggle-window-floating'); $fr = Rc $b
            GFocus $c; GAction 'g-scroll-wide' @('set-column-width','90%')
            GFocus $a
            GGate 'h43_float_does_not_scroll_with_tiles' (GSame $fr (Rc $b)) @($fr,(Rc $b))
            GAction 'g-float-away' @('focus-workspace','3'); GAction 'g-float-back' @('focus-workspace','2')
            $roundtrip = GSame $fr (Rc $b)
            GFocus $b; GAction 'g-retile' @('toggle-window-floating'); GAction 'g-refloat' @('toggle-window-floating')
            GGate 'h43_float_rectangle_survives_workspace_and_retile' ($roundtrip -and (GSame $fr (Rc $b))) @($fr,(Rc $b))
            Shot 'h43-floats'
        } finally { GClose; GStop $p }

        # H44: real caption and border mouse drags, NOT synthetic MOVESIZE events.
        $p = $null
        try {
            $p = GStart
            $h = [LaneGNative]::Open('lane-g-native'); Start-Sleep -Milliseconds 1200; GFocus $h
            $before = Rc $h; $list = GWindows 'g-native-before'; $slot = ColOf $list $h
            $x = [int]($before.L+80); $y = [int]($before.T+14)
            if ([LaneGNative]::Hit($h,$x,$y) -ne 2 -or [LT]::TopWindowAt($x,$y) -ne $h) { throw 'Native caption target is not our owned caption' }
            try {
                [LaneGNative]::Move($x,$y); [LaneGNative]::Button($false); Start-Sleep -Milliseconds 100
                foreach ($i in 1..8) { [LaneGNative]::Move(($x+7*$i),($y+3*$i)); Start-Sleep -Milliseconds 45 }
                Start-Sleep -Milliseconds 350
                $during = Rc $h; $state = [LaneGNative]::State($h); $proposal = [LaneGNative]::Proposal($h)
                GGate 'h44_caption_enters_native_loop' ($state[0] -eq 1 -and $state[1] -ge 1 -and $state[3] -ge 1) $state
                GCtl 'g-caption-refresh-during-loop' @('set-workspace-layout','scrolling') | Out-Null; Start-Sleep -Milliseconds 650
                $held = Rc $h; $heldState = [LaneGNative]::State($h)
                # Windows may show only an outline until release. The native proposal
                # proves movement; no new placement messages may arrive after relayout.
                GGate 'h44_caption_not_repositioned_during_loop' ((GSame $during $held) -and $proposal[0] -gt $before.L+30 -and $heldState[0] -eq 1 -and $heldState[5] -eq $state[5]) @($before,$during,$held,$proposal,$state,$heldState)
                Shot 'h44-caption-held'
            } finally { [LaneGNative]::Button($true) }
            Start-Sleep -Milliseconds 1300
            $after = Rc $h; $list = GWindows 'g-caption-drop'
            GGate 'h44_caption_drop_snaps_back_to_same_slot' ((GSame $before $after) -and (ColOf $list $h) -eq $slot -and [LaneGNative]::State($h)[2] -ge 1) @($before,$after,$list)
            # Add a sibling to prove both a column width and a tile height are reconciled.
            $sibling = GOpen 'native-sibling'; GFocus $h; GAction 'g-native-stack' @('consume-window-into-column')
            $before = Rc $h; $otherBefore = Rc $sibling
            $point = $null
            foreach ($dx in 0..7) { foreach ($dy in 0..7) {
                $px=[int]($before.R-1-$dx); $py=[int]($before.B-1-$dy)
                if ([LaneGNative]::Hit($h,$px,$py) -eq 17 -and [LT]::TopWindowAt($px,$py) -eq $h) { $point=@($px,$py); break }
            }; if ($point) { break } }
            if (-not $point) { throw 'No unobscured native bottom-right sizing border' }
            try {
                [LaneGNative]::Move($point[0],$point[1]); [LaneGNative]::Button($false); Start-Sleep -Milliseconds 100
                foreach ($i in 1..8) { [LaneGNative]::Move(($point[0]+8*$i),($point[1]-6*$i)); Start-Sleep -Milliseconds 45 }
                Start-Sleep -Milliseconds 350
                $during = Rc $h; $state = [LaneGNative]::State($h); $proposal = [LaneGNative]::Proposal($h)
                GGate 'h44_edge_enters_native_loop' ($state[0] -eq 1 -and $state[4] -ge 1) $state
                GCtl 'g-edge-refresh-during-loop' @('set-workspace-layout','scrolling') | Out-Null; Start-Sleep -Milliseconds 650
                $held = Rc $h; $heldState = [LaneGNative]::State($h)
                GGate 'h44_edge_not_repositioned_during_loop' ((GSame $during $held) -and $proposal[2] -gt $before.W+30 -and $heldState[0] -eq 1 -and $heldState[5] -eq $state[5]) @($before,$during,$held,$proposal,$state,$heldState)
                Shot 'h44-edge-held'
            } finally { [LaneGNative]::Button($true) }
            Start-Sleep -Milliseconds 1300
            $drop = Rc $h; $otherDrop = Rc $sibling
            GCtl 'g-edge-refresh-after-drop' @('set-workspace-layout','scrolling') | Out-Null; Start-Sleep -Milliseconds 750
            $after = Rc $h; $otherAfter = Rc $sibling
            GGate 'h44_edge_width_survives_refresh' ((Near $after.W $proposal[2] 4) -and (Near $otherAfter.W $after.W 4) -and $after.W -gt $before.W+30 -and (GSame $drop $after)) @($before,$during,$drop,$after,$otherAfter,$proposal)
            GGate 'h44_stacked_edge_height_and_siblings_survive_refresh' ((Near $after.H $proposal[3] 4) -and $after.H -lt $before.H-25 -and $otherAfter.H -gt $otherBefore.H+25 -and (GSame $otherDrop $otherAfter)) @($before,$during,$after,$otherBefore,$otherAfter,$proposal)
            Shot 'h44-edge-drop'
        } finally { [LaneGNative]::Button($true); GClose; GStop $p }
    } catch {
        $gDetails['error'] = ($_ | Out-String)
        ($_ | Out-String) | Set-Content (Join-Path $gDir 'error.txt')
    } finally {
        if ('LaneGNative' -as [type]) { [LaneGNative]::Button($true); [LaneGNative]::CloseAll() }
        foreach ($h in $gForms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
        foreach ($p in $gOwned) { GStop $p }
        if ($null -ne $gCursor) { [LaneGNative]::Move($gCursor.X,$gCursor.Y) }
        $gDetails | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $gDir 'values.json')
    }
}
