# Lane S: frozen screenshot UI and non-layout actions. Runs only under live-test's
# desktop lock. Every pipe connection verifies our retained daemon PID. Power
# actions are permanently dry-run in this child; never mutate the real desktop.
& {
    $sDir = Join-Path $D 'lane-s'
    $null = New-Item -ItemType Directory -Force $sDir
    $sDetails = [ordered]@{}
    $sOwned = $null; $sRpc = $null; $sForm = [IntPtr]::Zero
    $sSavedPipe = $env:WIRI_PIPE
    $sSavedDry = $env:WIRI_SYSTEM_ACTIONS_DRY_RUN
    $sSavedCursor = [Windows.Forms.Cursor]::Position
    $sGates = @('h46_selector_opens_without_activation', 'h46_selection_png_and_clipboard',
        'h46_space_drag_moves_then_resizes', 'h46_cancel_no_file_or_clipboard',
        'h46_ctrl_c_copy_only', 'h46_capture_button_saves', 'h46_frozen_pixels_without_ui')
    foreach ($sGate in $sGates) { $summary[$sGate] = $false }
    function SGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $sDetails[$name] = [ordered]@{ pass=$pass; observed=$observed }
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
    public static void Recolor(IntPtr h,int r,int g,int b) {
        var c=Control.FromHandle(h); if(c==null) throw new Exception("Not our WinForms probe");
        c.Invoke((Action)delegate { c.BackColor=Color.FromArgb(r,g,b); c.Refresh(); });
    }
}
'@
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Refused existing unowned wiri process' }
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_s_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $env:WIRI_SYSTEM_ACTIONS_DRY_RUN = Join-Path $sDir 'power-dry-run.ndjson'
        $sCfg = Join-Path $sDir 'config.kdl'
        $sConfig = "animations { off }`ninput { focus-follows-mouse false }`nscreenshot-path null`nbinds { Ctrl+Alt+F8 { screenshot; } }`n"
        [IO.File]::WriteAllText($sCfg,$sConfig,(New-Object Text.UTF8Encoding($false)))
        $sRoaming = $env:APPDATA; $sLocal = $env:LOCALAPPDATA
        try {
            $env:APPDATA = Join-Path $sDir 'roaming'; $env:LOCALAPPDATA = Join-Path $sDir 'local'
            $sOwned = Start-Process -FilePath $wiri -ArgumentList @('-v','--no-autostart','-c',('"'+$sCfg+'"')) -PassThru -RedirectStandardError (Join-Path $sDir 'daemon.stderr') -RedirectStandardOutput (Join-Path $sDir 'daemon.stdout')
            $null = $sOwned.Handle
        } finally { $env:APPDATA=$sRoaming; $env:LOCALAPPDATA=$sLocal }
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
    } catch {
        $sDetails['error'] = ($_ | Out-String)
    } finally {
        if ($sRpc) { $sRpc.Dispose(); $sRpc=$null }
        if ($sOwned -and -not $sOwned.HasExited) {
            try {
                $sQuit = New-Object LaneHConnection $env:WIRI_PIPE,$sOwned.Id
                try { $sQuit.Send('{"type":"quit"}' + "`n"); $null=$sQuit.ReadLine(1200) } finally { $sQuit.Dispose() }
                $null = $sOwned.WaitForExit(5000)
            } catch { $sDetails['shutdown-error'] = [string]$_ }
            if (-not $sOwned.HasExited) { Stop-Process -Id $sOwned.Id -Force }
        }
        if ($sForm -ne [IntPtr]::Zero -and [LT]::IsWindow($sForm)) { [TestForms]::CloseForm($sForm) }
        [Windows.Forms.Cursor]::Position = $sSavedCursor
        $env:WIRI_PIPE=$sSavedPipe; $env:WIRI_SYSTEM_ACTIONS_DRY_RUN=$sSavedDry
        $sDetails | ConvertTo-Json -Depth 14 | Set-Content -Encoding UTF8 (Join-Path $sDir 'evidence.json')
    }
}
