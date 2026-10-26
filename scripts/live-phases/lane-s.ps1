# Lane S: frozen screenshot UI and non-layout actions. Runs only under live-test's
# desktop lock. Every pipe connection verifies our retained daemon PID. Power
# actions are permanently dry-run in this child; never mutate the real desktop.
& {
    $sRoot = Join-Path $D 'lane-s'
    $sDir = Join-Path $sRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force $sDir
    $sDetails = [ordered]@{ fixture_directory=$sDir }
    # The shared Chord table deliberately stops at F9; keep this lane's new key local.
    $vk = $vk.Clone(); $vk['f10'] = 0x79
    $sOwned = $null; $sRpc = $null; $sEvents = $null; $sForm = [IntPtr]::Zero; $sOther = [IntPtr]::Zero
    $sSavedPipe = $env:WIRI_PIPE
    $sSavedDry = $env:WIRI_SYSTEM_ACTIONS_DRY_RUN
    $sSavedCursor = [Windows.Forms.Cursor]::Position
    $sGates = @('h46_selector_opens_without_activation', 'h46_selection_png_and_clipboard',
        'h46_space_drag_moves_then_resizes', 'h46_cancel_no_file_or_clipboard',
        'h46_ctrl_c_copy_only', 'h46_capture_button_saves', 'h46_frozen_pixels_without_ui',
        'h46_pointer_toggle_from_frozen_cursor', 'h46_space_saves',
        'h47_screen_pointer_flag_changes_pixels', 'h47_screen_copy_only_overrides_path',
        'h47_window_id_does_not_focus', 'h47_typed_window_flags_and_path',
        'h47_relative_path_is_rejected', 'h47_bind_properties_act',
        'h48_overview_open_close_idempotent', 'h48_urgency_set_unset_toggle_nonfocusing',
        'h48_urgency_events_and_focus_clear', 'h48_keyboard_layout_next_prev_index',
        'h49_load_config_file_and_reload_current', 'h49_bad_config_keeps_last_good',
        'h49_spawn_expands_program_only', 'h49_power_actions_dry_run_only',
        'h49_quit_confirmation_cancel', 'h49_quit_skip_confirmation_exits')
    foreach ($sGate in $sGates) { $summary[$sGate] = $false }
    function SGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $sDetails[$name] = [ordered]@{ pass=$pass; observed=$observed }
        $sDetails | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $sRoot 'evidence.json')
    }
    function SCheckpoint([string]$stage) {
        $sDetails['stage'] = $stage
        $sDetails | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $sRoot 'evidence.json')
    }
    function SRequest($request) {
        $text = $request | ConvertTo-Json -Depth 12 -Compress
        $sRpc.Send($text + "`n")
        $reply = $sRpc.ReadLine(4000)
        if ($null -eq $reply) { throw "Lane S IPC timeout: $text" }
        [ordered]@{ request=$request; reply=($reply | ConvertFrom-Json) } | ConvertTo-Json -Depth 12 -Compress |
            Add-Content -Encoding UTF8 (Join-Path $sDir 'wire.ndjson')
        return ($reply | ConvertFrom-Json)
    }
    function SDrainEvents([int]$waitMs = 250) {
        $records = @(); $until = [Diagnostics.Stopwatch]::StartNew()
        do {
            $line = $sEvents.ReadLine(60)
            if ($null -ne $line) {
                Add-Content -Encoding UTF8 (Join-Path $sDir 'events.ndjson') $line
                $records += ($line | ConvertFrom-Json)
            }
        } while ($until.ElapsedMilliseconds -lt $waitMs)
        return $records
    }
    function SAction([string]$name, [string[]]$words = @()) {
        $reply = SRequest @{type='run_action'; data=@{name=$name; args=$words; quoted=@()}}
        if (-not $reply.success) { throw "Action $name failed: $($reply | ConvertTo-Json -Compress)" }
        return $reply
    }
    function SOpen([string]$path) {
        FocusForm $sForm
        if (-not (WaitForeground $sForm 200 3000)) { throw 'Owned screenshot probe is not foreground' }
        $null = SAction 'screenshot' @('--path',$path,'--show-pointer','false')
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do { $overlay = [LaneSNative]::Overlay($sOwned.Id); if ($overlay -ne [IntPtr]::Zero) { break }; Start-Sleep -Milliseconds 40 } while ($clock.ElapsedMilliseconds -lt 4000)
        if ($overlay -eq [IntPtr]::Zero) { throw 'Screenshot overlay did not open' }
        Start-Sleep -Milliseconds 180
        return $overlay
    }
    function SKey([int]$code, [bool]$up = $false) {
        $fg = [LT]::GetForegroundWindow()
        if (-not $up -and $fg -ne $sForm -and $fg -ne [LaneSNative]::Overlay($sOwned.Id)) { throw 'Refused screenshot key: unrelated foreground' }
        [LT]::Key([byte]$code,$up)
        Start-Sleep -Milliseconds 80
    }
    function STap([int]$code) { SKey $code; SKey $code $true }
    function SDrag([int]$x1,[int]$y1,[int]$x2,[int]$y2,[bool]$spaceMove=$false) {
        [LaneSNative]::Move($x1,$y1); [LaneSNative]::Mouse($sOwned.Id,$sForm,$false)
        try {
            [LaneSNative]::Move($x2,$y2); Start-Sleep -Milliseconds 100
            if ($spaceMove) {
                SKey 32
                [LaneSNative]::Move(($x2+30),($y2+20)); Start-Sleep -Milliseconds 100
                SKey 32 $true
                [LaneSNative]::Move(($x2+60),($y2+50)); Start-Sleep -Milliseconds 100
            }
        } finally { [LaneSNative]::Mouse($sOwned.Id,$sForm,$true) }
        Start-Sleep -Milliseconds 120
    }
    try {
        Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows.Forms;
public static class LaneSNative {
    delegate bool EnumProc(IntPtr h,IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p,IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassNameW(IntPtr h,StringBuilder s,int n);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h,StringBuilder s,int n);
    [DllImport("user32.dll")] static extern bool PostMessageW(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h,uint ms);
    public static bool Exited(IntPtr h,uint ms) { return WaitForSingleObject(h,ms)==0; }
    public static IntPtr QuitDialog(int pid) {
        IntPtr found=IntPtr.Zero;
        EnumWindows(delegate(IntPtr h,IntPtr l) { uint p; GetWindowThreadProcessId(h,out p);
            if(p==(uint)pid) { var cls=new StringBuilder(128); var title=new StringBuilder(128);
                GetClassNameW(h,cls,cls.Capacity); GetWindowTextW(h,title,title.Capacity);
                if(cls.ToString()=="#32770" && title.ToString()=="wiri") { found=h; return false; } } return true; },IntPtr.Zero);
        return found;
    }
    public static bool CancelQuit(int pid) {
        var dialog=QuitDialog(pid);
        return dialog!=IntPtr.Zero && PostMessageW(dialog,0x111,new IntPtr(7),IntPtr.Zero);
    }
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point p);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern uint SendInput(uint n,INPUT[] i,int size);
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MI m; }
    static bool held;
    public static IntPtr Overlay(int pid) {
        IntPtr found=IntPtr.Zero;
        EnumWindows(delegate(IntPtr h,IntPtr l) { uint p; GetWindowThreadProcessId(h,out p);
            if(p==(uint)pid) { var s=new StringBuilder(128); GetClassNameW(h,s,s.Capacity); if(s.ToString()=="WiriScreenshotUI") { found=h; return false; } } return true; },IntPtr.Zero);
        return found;
    }
    public static void Move(int x,int y) { if(!SetCursorPos(x,y)) throw new Exception("SetCursorPos failed"); }
    public static void Mouse(int pid,IntPtr probe,bool up) {
        if(up && !held) return;
        if(!up) { var ui=Overlay(pid); var fg=GetForegroundWindow(); Point p; GetCursorPos(out p);
            if(ui==IntPtr.Zero || (fg!=probe && fg!=ui) || WindowFromPoint(p)!=ui) throw new Exception("Refused unowned screenshot click"); }
        var i=new INPUT(); i.m.flags=up?4u:2u;
        if(SendInput(1,new INPUT[]{i},Marshal.SizeOf(typeof(INPUT)))!=1) throw new Exception("SendInput failed");
        held=!up;
    }
    public static int Difference(string a,string b,int x,int y) {
        using(var first=new Bitmap(a)) using(var second=new Bitmap(b)) {
            int count=0;
            for(int cy=Math.Max(0,y-8);cy<Math.Min(first.Height,y+64);++cy)
                for(int cx=Math.Max(0,x-8);cx<Math.Min(first.Width,x+64);++cx)
                    if(first.GetPixel(cx,cy)!=second.GetPixel(cx,cy)) ++count;
            return count;
        }
    }
    public static void Recolor(IntPtr h,int r,int g,int b) {
        var c=Control.FromHandle(h); if(c==null) throw new Exception("Not our WinForms probe");
        c.Invoke((Action)delegate { c.BackColor=Color.FromArgb(r,g,b); c.Refresh(); });
    }
}
'@
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Refused existing unowned wiri process' }
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_s_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $env:WIRI_SYSTEM_ACTIONS_DRY_RUN = Join-Path $sDir ('power-dry-run-' + [Guid]::NewGuid().ToString('N') + '.ndjson')
        $sCfg = Join-Path $sDir 'config.kdl'
        $sConfig = "animations { off }`ninput { focus-follows-mouse false }`nscreenshot-path null`nbinds { Ctrl+Alt+F8 { screenshot; } }`n"
        [IO.File]::WriteAllText($sCfg,$sConfig,(New-Object Text.UTF8Encoding($false)))
        # A tiny owned program records literal argv and exits. Fake USERPROFILE
        # is child-local: no files or settings in the real profile are touched.
        $sProbeName = 'spawn-probe-' + [Guid]::NewGuid().ToString('N') + '.exe'
        Add-Type -OutputType ConsoleApplication -OutputAssembly (Join-Path $sDir $sProbeName) -TypeDefinition @'
using System;
using System.IO;
public static class LaneSSpawnProbe {
    public static void Main(string[] values) { File.WriteAllLines(values[0],new string[]{values[1],values[2]}); }
}
'@
        $sRoaming = $env:APPDATA; $sLocal = $env:LOCALAPPDATA; $sProfile = $env:USERPROFILE
        try {
            $env:APPDATA = Join-Path $sDir 'roaming'; $env:LOCALAPPDATA = Join-Path $sDir 'local'
            $env:USERPROFILE = $sDir
            $sOwned = Start-Process -FilePath $wiri -ArgumentList @('-v','--no-autostart','-c',('"'+$sCfg+'"')) -PassThru -RedirectStandardError (Join-Path $sDir 'daemon.stderr') -RedirectStandardOutput (Join-Path $sDir 'daemon.stdout')
            $null = $sOwned.Handle
        } finally { $env:APPDATA=$sRoaming; $env:LOCALAPPDATA=$sLocal; $env:USERPROFILE=$sProfile }
        Start-Sleep -Milliseconds 1500
        $sRpc = New-Object LaneHConnection $env:WIRI_PIPE,$sOwned.Id
        $sForm = [TestForms]::Open('lane-s-frozen-probe',43,177,89)
        Start-Sleep -Milliseconds 1400
        $list = Ctl 'h46-owned-window-list' @('--json','window-list') | ConvertFrom-Json
        $sDetails['window-list'] = $list
        FocusForm $sForm
        if (-not (WaitForeground $sForm 300 3000)) { throw 'Owned probe did not focus' }
        $sRect = [LT]::Visible($sForm)
        $sx = [Math]::Max($swa.Left,$sRect.L) + 30
        $sy = [Math]::Max($swa.Top,$sRect.T) + 80
        $ex = $sx + [Math]::Min([int]($swa.Width*0.12),[Math]::Min($swa.Right,$sRect.R)-$sx-80)
        $ey = $sy + [Math]::Min([int]($swa.Height*0.20),[Math]::Min($swa.Bottom,$sRect.B)-$sy-80)
        if ($ex -le $sx -or $ey -le $sy) { throw 'Owned probe has insufficient on-screen area for selection' }
        $sPath = Join-Path $sDir 'selection.png'
        $ui = SOpen $sPath
        SGate 'h46_selector_opens_without_activation' ([LT]::GetForegroundWindow() -eq $sForm) @{overlay=$ui.ToInt64(); foreground=[LT]::GetForegroundWindow().ToInt64()}
        [LaneSNative]::Recolor($sForm,210,40,90)
        SDrag $sx $sy $ex $ey
        STap 13
        Start-Sleep -Milliseconds 1200
        $png = DPng $sPath; $clip = [LaneD]::Clipboard()
        SGate 'h46_selection_png_and_clipboard' ($png[2] -eq 1 -and $png[0] -eq ($ex-$sx+1) -and $png[1] -eq ($ey-$sy+1) -and $clip[0] -eq $png[0] -and $clip[1] -eq $png[1] -and $clip[2] -eq 1) @{png=$png; clipboard=$clip}
        # The PNG must contain the frozen green pixels, never the post-open pink
        # form or dimmed selector/button pixels. A selection may intersect another
        # tenant's column, so require only the owned green part, not the whole image.
        $green = [LT]::CountColor($sPath,43,177,89); $pink = [LT]::CountColor($sPath,210,40,90)
        SGate 'h46_frozen_pixels_without_ui' ($green -gt 100 -and $pink -eq 0) @{green=$green; pink=$pink}
        $sMovePath = Join-Path $sDir 'space-move.png'
        $null = SOpen $sMovePath
        SDrag $sx $sy $ex $ey $true
        STap 13; Start-Sleep -Milliseconds 1100
        $moved = DPng $sMovePath
        SGate 'h46_space_drag_moves_then_resizes' ($moved[2] -eq 1 -and $moved[0] -eq ($ex-$sx+31) -and $moved[1] -eq ($ey-$sy+31)) $moved
        $sCancelPath = Join-Path $sDir 'cancel.png'; $seq = [LaneD]::GetClipboardSequenceNumber()
        $null = SOpen $sCancelPath
        STap 27; Start-Sleep -Milliseconds 600
        SGate 'h46_cancel_no_file_or_clipboard' (-not (Test-Path $sCancelPath) -and $seq -eq [LaneD]::GetClipboardSequenceNumber() -and [LaneSNative]::Overlay($sOwned.Id) -eq [IntPtr]::Zero) $null
        $sCopyPath = Join-Path $sDir 'copy-only.png'; $seq = [LaneD]::GetClipboardSequenceNumber()
        $null = SOpen $sCopyPath
        SKey 17; STap 67; SKey 17 $true; Start-Sleep -Milliseconds 1000
        SGate 'h46_ctrl_c_copy_only' (-not (Test-Path $sCopyPath) -and $seq -ne [LaneD]::GetClipboardSequenceNumber() -and [LaneD]::Clipboard()[2] -eq 1) ([LaneD]::Clipboard())
        $sButtonPath = Join-Path $sDir 'button.png'; $null = SOpen $sButtonPath
        $sBounds = [Windows.Forms.Screen]::FromHandle($sForm).Bounds
        $bx = $sBounds.Left + [Math]::Max(8,[int](($sBounds.Width-680)/2)) + 30
        $by = $sBounds.Bottom-45
        [LaneSNative]::Move($bx,$by); [LaneSNative]::Mouse($sOwned.Id,$sForm,$false); [LaneSNative]::Mouse($sOwned.Id,$sForm,$true)
        Start-Sleep -Milliseconds 1100
        SGate 'h46_capture_button_saves' ((DPng $sButtonPath)[2] -eq 1) (DPng $sButtonPath)

        FocusForm $sForm
        if (-not (WaitForeground $sForm 250 3000)) { throw 'Pointer fixture lost foreground' }
        $sRect = [LT]::Visible($sForm)
        $px = [Math]::Max($swa.Left,$sRect.L)+80; $py = [Math]::Max($swa.Top,$sRect.T)+140
        [LaneSNative]::Move($px,$py)
        # The selector freezes both clean and pointed frames on opening. Move the
        # real cursor after opening, toggle p, and save with Space (not Enter).
        $sTogglePath = Join-Path $sDir 'pointer-toggle.png'
        $null = SOpen $sTogglePath
        $tx = $px-20; $ty = $py-20
        SDrag $tx $ty ($px+60) ($py+60)
        STap 80; STap 32; Start-Sleep -Milliseconds 1100
        $togglePng = DPng $sTogglePath
        SGate 'h46_space_saves' ($togglePng[2] -eq 1 -and $togglePng[0] -eq 81 -and $togglePng[1] -eq 81) $togglePng
        $sToggleClean = Join-Path $sDir 'pointer-toggle-clean.png'
        [LaneSNative]::Move($px,$py)
        $null = SOpen $sToggleClean
        SDrag $tx $ty ($px+60) ($py+60)
        STap 13; Start-Sleep -Milliseconds 1100
        $toggleDifference = [LaneSNative]::Difference($sToggleClean,$sTogglePath,20,20)
        SGate 'h46_pointer_toggle_from_frozen_cursor' ($toggleDifference -gt 0) $toggleDifference
        [LaneSNative]::Move($px,$py)
        $sClean = Join-Path $sDir 'screen-no-pointer.png'; $sPointed = Join-Path $sDir 'screen-pointer.png'
        $null = SAction 'screenshot-screen' @('--show-pointer','false','--path',$sClean)
        $null = SAction 'screenshot-screen' @('--show-pointer','true','--path',$sPointed)
        $difference = [LaneSNative]::Difference($sClean,$sPointed,($px-$sBounds.Left),($py-$sBounds.Top))
        SGate 'h47_screen_pointer_flag_changes_pixels' ($difference -gt 0 -and (DPng $sPointed)[2] -eq 1) $difference
        $sNoWrite = Join-Path $sDir 'screen-copy-only.png'; $seq = [LaneD]::GetClipboardSequenceNumber()
        $null = SAction 'screenshot-screen' @('--write-to-disk','false','--path',$sNoWrite)
        SGate 'h47_screen_copy_only_overrides_path' (-not (Test-Path $sNoWrite) -and $seq -ne [LaneD]::GetClipboardSequenceNumber()) $null
        $bad = SRequest @{Action=@{ScreenshotScreen=@{path='relative.png'}}}
        SGate 'h47_relative_path_is_rejected' ($null -ne $bad.Err) $bad
        $sOther = [TestForms]::Open('lane-s-explicit-background',44,88,200)
        Start-Sleep -Milliseconds 1000
        FocusForm $sForm
        if (-not (WaitForeground $sForm 300 3000)) { throw 'Could not restore foreground before explicit-id capture' }
        $sExplicit = Join-Path $sDir 'explicit-window.png'; $fg = [LT]::GetForegroundWindow()
        $typed = SRequest @{Action=@{ScreenshotWindow=@{id=$sOther.ToInt64(); write_to_disk=$true; show_pointer=$true; path=$sExplicit}}}
        $windowPng = DPng $sExplicit
        SGate 'h47_window_id_does_not_focus' ([LT]::GetForegroundWindow() -eq $fg) @{before=$fg.ToInt64(); after=[LT]::GetForegroundWindow().ToInt64()}
        SGate 'h47_typed_window_flags_and_path' ($typed.Ok -eq 'Handled' -and $windowPng[2] -eq 1 -and [LT]::CountColor($sExplicit,44,88,200) -gt 1000) @{reply=$typed; png=$windowPng}
        # Reload a real niri bind with per-action properties; no global path.
        $sBindPath = (Join-Path $sDir 'bind-must-not-write.png').Replace('\','/')
        $sBindConfig = $sConfig + "`nbinds { Ctrl+Alt+F9 { screenshot-screen `"$sBindPath`" write-to-disk=false show-pointer=false; } }`n"
        [IO.File]::WriteAllText($sCfg,$sBindConfig,(New-Object Text.UTF8Encoding($false)))
        $null = SRequest @{type='reload_config'}
        Start-Sleep -Milliseconds 700
        FocusForm $sForm
        if (-not (WaitForeground $sForm 200 3000)) { throw 'Bind fixture lost foreground' }
        $seq=[LaneD]::GetClipboardSequenceNumber(); Chord 'ctrl+alt+f9'; Start-Sleep -Milliseconds 1000
        SGate 'h47_bind_properties_act' (-not (Test-Path $sBindPath) -and $seq -ne [LaneD]::GetClipboardSequenceNumber()) $null

        $sEvents = New-Object LaneHConnection $env:WIRI_PIPE,$sOwned.Id
        $sEvents.Send('"EventStream"' + "`n")
        $null = SDrainEvents 400
        $null = SAction 'close-overview'
        $null = SAction 'open-overview'; $open1 = SRequest 'OverviewState'
        $null = SAction 'open-overview'; $open2 = SRequest 'OverviewState'
        $null = SAction 'close-overview'; $close1 = SRequest 'OverviewState'
        $null = SAction 'close-overview'; $close2 = SRequest 'OverviewState'
        SGate 'h48_overview_open_close_idempotent' ($open1.Ok.OverviewState.is_open -and $open2.Ok.OverviewState.is_open -and -not $close1.Ok.OverviewState.is_open -and -not $close2.Ok.OverviewState.is_open) @($open1,$open2,$close1,$close2)
        FocusForm $sForm
        if (-not (WaitForeground $sForm 250 3000)) { throw 'Urgency fixture lost foreground' }
        $sUrgencySteps = @(); $sUrgentPass = $true; $fg = [LT]::GetForegroundWindow()
        foreach ($step in @(@('SetWindowUrgent',$true),@('SetWindowUrgent',$true),@('UnsetWindowUrgent',$false),@('ToggleWindowUrgent',$true),@('ToggleWindowUrgent',$false),@('SetWindowUrgent',$true))) {
            $typedAction = @{}; $typedAction[$step[0]] = @{id=$sOther.ToInt64()}
            $answer = SRequest @{Action=$typedAction}
            $windows = SRequest 'Windows'
            $observed = @($windows.Ok.Windows | Where-Object { $_.id -eq $sOther.ToInt64() })[0]
            $pass = $answer.Ok -eq 'Handled' -and $observed.is_urgent -eq $step[1] -and [LT]::GetForegroundWindow() -eq $fg
            $sUrgentPass = $sUrgentPass -and $pass
            $sUrgencySteps += @{action=$step[0]; expected=$step[1]; window=$observed; pass=$pass}
        }
        SGate 'h48_urgency_set_unset_toggle_nonfocusing' $sUrgentPass $sUrgencySteps
        FocusForm $sOther
        if (-not (WaitForeground $sOther 250 3000)) { throw 'Urgent owned form did not focus' }
        $focusedWindows = SRequest 'Windows'
        $focusedUrgent = @($focusedWindows.Ok.Windows | Where-Object { $_.id -eq $sOther.ToInt64() })[0].is_urgent
        $sUrgencyEvents = @(SDrainEvents 600 | Where-Object { $null -ne $_.WindowUrgencyChanged -and $_.WindowUrgencyChanged.id -eq $sOther.ToInt64() })
        $eventTrue = @($sUrgencyEvents | Where-Object { $_.WindowUrgencyChanged.urgent }).Count
        $eventFalse = @($sUrgencyEvents | Where-Object { -not $_.WindowUrgencyChanged.urgent }).Count
        SGate 'h48_urgency_events_and_focus_clear' (-not $focusedUrgent -and $eventTrue -ge 1 -and $eventFalse -ge 1) $sUrgencyEvents

        $layouts0 = SRequest 'KeyboardLayouts'; $initialIdx = [int]$layouts0.Ok.KeyboardLayouts.current_idx
        $layoutCount = @($layouts0.Ok.KeyboardLayouts.names).Count
        $nextLayout = SRequest @{Action=@{SwitchLayout=@{target='Next'}}}; Start-Sleep -Milliseconds 300
        $layoutsNext = SRequest 'KeyboardLayouts'
        $prevLayout = SRequest @{Action=@{SwitchLayout=@{target='Prev'}}}; Start-Sleep -Milliseconds 300
        $layoutsPrev = SRequest 'KeyboardLayouts'
        $indexLayout = SRequest @{Action=@{SwitchLayout=@{target=@{Index=$initialIdx}}}}; Start-Sleep -Milliseconds 300
        $layoutsIndex = SRequest 'KeyboardLayouts'
        SGate 'h48_keyboard_layout_next_prev_index' ($layoutCount -gt 0 -and $nextLayout.Ok -eq 'Handled' -and $prevLayout.Ok -eq 'Handled' -and $indexLayout.Ok -eq 'Handled' -and $layoutsNext.Ok.KeyboardLayouts.current_idx -eq (($initialIdx+1)%$layoutCount) -and $layoutsPrev.Ok.KeyboardLayouts.current_idx -eq $initialIdx -and $layoutsIndex.Ok.KeyboardLayouts.current_idx -eq $initialIdx) @($layouts0,$layoutsNext,$layoutsPrev,$layoutsIndex)

        $sAltCfg = Join-Path $sDir 'alternate.kdl'
        $sAltConfig = $sConfig + "`nbinds { Ctrl+Alt+F10 { screenshot-screen write-to-disk=false show-pointer=false; } }`n"
        [IO.File]::WriteAllText($sAltCfg,$sAltConfig,(New-Object Text.UTF8Encoding($false)))
        $loaded = SRequest @{Action=@{LoadConfigFile=@{path=$sAltCfg}}}; Start-Sleep -Milliseconds 700
        $reloaded = SRequest @{Action=@{LoadConfigFile=@{path=$null}}}; Start-Sleep -Milliseconds 700
        $configEvents = @(SDrainEvents 500 | Where-Object { $null -ne $_.ConfigLoaded })
        $altEvents = @($configEvents | Where-Object { -not $_.ConfigLoaded.failed })
        $seq=[LaneD]::GetClipboardSequenceNumber(); Chord 'ctrl+alt+f10'; Start-Sleep -Milliseconds 700
        $altBindWorked = $seq -ne [LaneD]::GetClipboardSequenceNumber()
        SGate 'h49_load_config_file_and_reload_current' ($loaded.Ok -eq 'Handled' -and $reloaded.Ok -eq 'Handled' -and $altEvents.Count -ge 2 -and $altBindWorked) $configEvents
        $sBadCfg = Join-Path $sDir 'invalid.kdl'; [IO.File]::WriteAllText($sBadCfg,'layout {',(New-Object Text.UTF8Encoding($false)))
        $badConfig = SRequest @{Action=@{LoadConfigFile=@{path=$sBadCfg}}}
        $seq=[LaneD]::GetClipboardSequenceNumber(); Chord 'ctrl+alt+f10'; Start-Sleep -Milliseconds 700
        SGate 'h49_bad_config_keeps_last_good' ($null -ne $badConfig.Err -and $seq -ne [LaneD]::GetClipboardSequenceNumber()) $badConfig
        $sSpawnMarker = Join-Path $sDir ('spawn-' + [Guid]::NewGuid().ToString('N') + '.txt')
        $literalOne = '~/argument with spaces'; $literalTwo = '& not-a-shell %USERPROFILE%'
        $null = SAction 'spawn' @(("~/"+$sProbeName),$sSpawnMarker,$literalOne,$literalTwo)
        Start-Sleep -Milliseconds 1000
        $spawnWords = if (Test-Path $sSpawnMarker) { [IO.File]::ReadAllLines($sSpawnMarker) } else { @() }
        SGate 'h49_spawn_expands_program_only' ($spawnWords.Count -eq 2 -and $spawnWords[0] -eq $literalOne -and $spawnWords[1] -eq $literalTwo) $spawnWords
        foreach ($powerVariant in @('PowerOffMonitors','PowerOnMonitors','Suspend')) { $null = SRequest @{Action=$powerVariant} }
        $powerRecords = [IO.File]::ReadAllLines($env:WIRI_SYSTEM_ACTIONS_DRY_RUN)
        SGate 'h49_power_actions_dry_run_only' (($powerRecords -join ',') -eq 'power-off-monitors,power-on-monitors,suspend') $powerRecords
        $null = SRequest @{Action=@{Quit=@{skip_confirmation=$false}}}
        Start-Sleep -Milliseconds 600
        $sDialog = [LaneSNative]::QuitDialog($sOwned.Id)
        $sCancelled = [LaneSNative]::CancelQuit($sOwned.Id)
        Start-Sleep -Milliseconds 300
        SGate 'h49_quit_confirmation_cancel' ($sDialog -ne [IntPtr]::Zero -and $sCancelled -and -not $sOwned.HasExited -and [LaneSNative]::QuitDialog($sOwned.Id) -eq [IntPtr]::Zero) $sDialog.ToInt64()
        $sEvents.Dispose(); $sEvents=$null
        $null = SRequest @{Action=@{Quit=@{skip_confirmation=$true}}}
        SCheckpoint 'waiting-for-owned-quit'
        SGate 'h49_quit_skip_confirmation_exits' ([LaneSNative]::Exited($sOwned.Handle,5000)) $null
    } catch {
        $sDetails['error'] = ($_ | Out-String)
    } finally {
        SCheckpoint 'disposing-pipes'
        if ($sEvents) { $sEvents.Dispose(); $sEvents=$null }
        if ($sRpc) { $sRpc.Dispose(); $sRpc=$null }
        SCheckpoint 'ensuring-owned-shutdown'
        if ($sOwned -and -not $sOwned.HasExited) {
            try {
                $sQuit = New-Object LaneHConnection $env:WIRI_PIPE,$sOwned.Id
                try { $sQuit.Send('{"type":"quit"}' + "`n"); $null=$sQuit.ReadLine(1200) } finally { $sQuit.Dispose() }
                $null = [LaneSNative]::Exited($sOwned.Handle,5000)
            } catch { $sDetails['shutdown-error'] = [string]$_ }
            if (-not $sOwned.HasExited) { Stop-Process -Id $sOwned.Id -Force }
        }
        SCheckpoint 'closing-owned-forms'
        if ($sForm -ne [IntPtr]::Zero -and [LT]::IsWindow($sForm)) { [LT]::Close($sForm) }
        if ($sOther -ne [IntPtr]::Zero -and [LT]::IsWindow($sOther)) { [LT]::Close($sOther) }
        [Windows.Forms.Cursor]::Position = $sSavedCursor
        $env:WIRI_PIPE=$sSavedPipe; $env:WIRI_SYSTEM_ACTIONS_DRY_RUN=$sSavedDry
        $sDetails | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $sDir 'evidence.json')
        $sDetails | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $sRoot 'evidence.json')
    }
}
