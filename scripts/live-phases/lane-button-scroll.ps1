# H83: application-delivered input from an owned WinForms HWND. Dot-source only
# under live-test.ps1's desktop mutex and private profile. No hardware/device
# identity claim: SendInput exercises WH_MOUSE_LL and its Windows wheel adapter.
& {
    $bsRoot = Join-Path $D 'lane-button-scroll'
    $bsDir = Join-Path $bsRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $bsDir
    $bsGates = @(
        'h83_unconfigured_middle_click_pair_passes',
        'h83_hold_consumes_both_button_edges',
        'h83_hold_motion_delivers_both_wheel_axes',
        'h83_hold_freezes_cursor_anchor',
        'h83_release_stops_scrolling',
        'h83_unconfigured_click_after_release_passes',
        'h83_short_tap_latches_scroll_lock',
        'h83_second_tap_unlocks_scroll_lock',
        'h83_reload_cancels_latched_scroll',
        'h83_reload_keeps_consumed_release_paired',
        'h83_explicit_middle_button_bind_wins',
        'h83_physical_wheel_bind_positive_control',
        'h83_emulated_wheel_bypasses_physical_bind_and_global_factor',
        'h83_touchpad_only_config_does_not_enable_mouse_scroll',
        'h83_cleanup'
    )
    $bsDetails = [ordered]@{
        fixture_directory = $bsDir
        scope = 'Owned synthetic input and application message delivery; not physical device identity, libinput distance calibration, or hardware qualification'
        checks = [ordered]@{}
        cleanup_errors = @()
    }
    foreach ($gate in $bsGates) {
        $summary[$gate] = $false
        $bsDetails.checks[$gate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $bsProcess = $null
    $bsForm = [IntPtr]::Zero
    $bsSavedCursor = $null
    $bsSavedPipe = $env:WIRI_PIPE
    $bsSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    $bsPipe = '\\.\pipe\wiri_live_h83_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $bsOldDpi = [IntPtr]::Zero
    $bsNativeReady = $false
    function BSEvidence {
        $bsDetails | ConvertTo-Json -Depth 18 | Set-Content -Encoding UTF8 (Join-Path $bsRoot 'evidence.json')
    }
    function BSGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $bsDetails.checks[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        BSEvidence
    }
    function BSRequest([string]$tag, $request) {
        if ($null -eq $bsProcess -or $bsProcess.HasExited) { throw 'H83 owned daemon is unavailable' }
        $text = $request | ConvertTo-Json -Depth 8 -Compress
        $raw = [H83ButtonScrollProbe]::Request($bsProcess.Id, $bsPipe, $text)
        $reply = $raw | ConvertFrom-Json
        [ordered]@{ tag = $tag; request = $request; reply = $reply } |
            ConvertTo-Json -Depth 12 -Compress | Add-Content -Encoding UTF8 (Join-Path $bsDir 'wire.ndjson')
        if ($reply.success -ne $true) { throw "H83 request failed: $tag $raw" }
        return $reply
    }
    function BSConfig([string]$mouse, [string]$binds = '', [string]$extra = '') {
        return @"
input { mod-key "ctrl-alt"; $mouse }
animations { off; }
hotkey-overlay { skip-at-startup; }
window-rule { match title="^lane-h83-button-scroll$"; open-floating true; }
binds { $binds }
$extra
"@
    }
    function BSReload([string]$tag, [string]$text) {
        [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
        $null = BSRequest ("reload-$tag") @{ type = 'reload_config' }
        # Config reload queues work. Observe a bounded settle interval before
        # injecting; each subsequent gate also needs real positive delivery.
        Start-Sleep -Milliseconds 1400
        [IO.File]::WriteAllText((Join-Path $bsDir ("$tag.kdl")), $text, (New-Object Text.UTF8Encoding($false)))
    }
    function BSFocus {
        if (-not [H83ButtonScrollProbe]::Owns($bsForm)) { throw 'H83 form ownership lost' }
        FocusForm $bsForm
        [H83ButtonScrollProbe]::Activate($bsForm)
        Start-Sleep -Milliseconds 250
        if ([H83ButtonScrollProbe]::Foreground() -ne $bsForm) { throw 'H83 owned probe did not gain foreground' }
        [H83ButtonScrollProbe]::PlaceAtCenter($bsForm)
        Start-Sleep -Milliseconds 150
        [H83ButtonScrollProbe]::RequirePointer($bsForm)
    }
    function BSClear {
        Start-Sleep -Milliseconds 150
        [H83ButtonScrollProbe]::Clear($bsForm)
    }
    function BSObserve([string]$tag, [int]$waitMs = 350) {
        Start-Sleep -Milliseconds $waitMs
        $events = @([H83ButtonScrollProbe]::Events($bsForm))
        $snapshot = [ordered]@{
            events = $events
            cursor = [H83ButtonScrollProbe]::Cursor()
            foreground = [int64][H83ButtonScrollProbe]::Foreground()
        }
        $snapshot | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 (Join-Path $bsDir ("$tag.json"))
        return $snapshot
    }
    function BSClick {
        try {
            [H83ButtonScrollProbe]::Middle($bsForm, $false)
            Start-Sleep -Milliseconds 40
        } finally { [H83ButtonScrollProbe]::Middle($bsForm, $true) }
        Start-Sleep -Milliseconds 150
    }
    function BSMotion([int]$dx, [int]$dy) {
        [H83ButtonScrollProbe]::Relative($bsForm, $dx, $dy)
        Start-Sleep -Milliseconds 150
    }
    function BSWheels($snapshot) {
        return @($snapshot.events | Where-Object { $_ -like 'W:*' }).Count
    }
    function BSPair($snapshot) {
        return @($snapshot.events | Where-Object { $_ -eq 'MD' }).Count -eq 1 -and
            @($snapshot.events | Where-Object { $_ -eq 'MU' }).Count -eq 1
    }
    function BSNoEdges($snapshot) {
        return @($snapshot.events | Where-Object { $_ -eq 'MD' -or $_ -eq 'MU' }).Count -eq 0
    }
    function BSNearPoint($a, $b) { return [Math]::Abs($a.X - $b.X) -le 1 -and [Math]::Abs($a.Y - $b.Y) -le 1 }
    function BSBind([string]$name) {
        $path = Join-Path $bsDir ("$name.marker.txt")
        [IO.File]::WriteAllText($path, '')
        return ('spawn-sh r#"echo hit>> "' + $path + '""#')
    }
    function BSCount([string]$name) {
        return @(Get-Content -LiteralPath (Join-Path $bsDir ("$name.marker.txt")) -ErrorAction SilentlyContinue |
            Where-Object { $_ -eq 'hit' }).Count
    }
    function BSCleanupError([string]$stage, $errorValue) {
        $bsDetails.cleanup_errors += "$stage`: $errorValue"
    }
    try {
        if (-not $ownsLiveLock -or -not $testRoaming -or -not $testLocal) {
            throw 'H83 requires the parent desktop mutex and isolated profile'
        }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H83 refuses to start beside any existing daemon'
        }
        if (-not ('H83ButtonScrollProbe' -as [type])) {
            Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing, System.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;
public static class H83ButtonScrollProbe {
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    // INPUT's native union is 32 bytes on 64-bit and 24 on 32-bit. MI is the
    // largest member on these hosts; sequential nesting supplies native padding.
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MI mouse; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, INPUT[] input, int size);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
    sealed class Probe : Form {
        public readonly List<string> InputEvents = new List<string>();
        protected override void WndProc(ref Message m) {
            string e = null;
            if (m.Msg == 0x207 || m.Msg == 0x209) e = "MD";
            else if (m.Msg == 0x208) e = "MU";
            else if (m.Msg == 0x20A || m.Msg == 0x20E) {
                int delta = unchecked((short)((m.WParam.ToInt64() >> 16) & 65535));
                e = (m.Msg == 0x20A ? "W:V:" : "W:H:") + delta;
            }
            if (e != null) lock(InputEvents) InputEvents.Add(e);
            base.WndProc(ref m);
        }
    }
    sealed class Owner {
        public Probe Form; public Thread Thread; public IntPtr Handle;
        public volatile bool Cancelled; public Exception Error;
        public readonly ManualResetEvent Ready = new ManualResetEvent(false);
    }
    static readonly object Sync = new object();
    static readonly List<Owner> Owners = new List<Owner>();
    static readonly Dictionary<IntPtr,Owner> Forms = new Dictionary<IntPtr,Owner>();
    static readonly object HeldSync = new object();
    static bool HeldMiddle;
    static Owner Find(IntPtr h) { lock(Sync) { Owner o; if (!Forms.TryGetValue(h,out o)) throw new InvalidOperationException("Unknown H83 HWND"); return o; } }
    public static bool Owns(IntPtr h) { lock(Sync) return Forms.ContainsKey(h); }
    public static IntPtr Foreground() { return GetForegroundWindow(); }
    public static Point Cursor() { Point p; if (!GetCursorPos(out p)) throw new Win32Exception(); return p; }
    public static void RestoreCursor(Point point) { if (!SetCursorPos(point.X,point.Y)) throw new Win32Exception(); }
    public static void RequirePointer(IntPtr h) {
        Find(h);
        if (GetForegroundWindow() != h || GetAncestor(WindowFromPoint(Cursor()),2) != h)
            throw new InvalidOperationException("H83 input refused: pointer/foreground not owned probe");
    }
    static void Send(uint flags, int dx, int dy, int data) {
        INPUT i = new INPUT(); i.mouse.flags=flags; i.mouse.x=dx; i.mouse.y=dy;
        i.mouse.data=unchecked((uint)data); i.mouse.extra=new UIntPtr(0x48383333u);
        if (SendInput(1,new INPUT[] { i },Marshal.SizeOf(typeof(INPUT))) != 1)
            throw new Win32Exception(Marshal.GetLastWin32Error(),"H83 SendInput rejected");
    }
    public static void Middle(IntPtr h, bool up) {
        lock(HeldSync) {
            if (up && !HeldMiddle) return;
            if (!up) {
                RequirePointer(h);
                if (!HeldMiddle && GetAsyncKeyState(4) < 0) throw new InvalidOperationException("H83 refuses an already held middle button");
            }
            Send(up ? 0x40u : 0x20u,0,0,0);
            HeldMiddle=!up;
        }
    }
    public static void Relative(IntPtr h, int dx, int dy) {
        RequirePointer(h);
        if (Math.Abs(dx)>16 || Math.Abs(dy)>16) throw new ArgumentOutOfRangeException("bounded relative motion only");
        Point p=Cursor();
        // Leave ample room for the OS's current acceleration without writing SPI.
        if (GetAncestor(WindowFromPoint(new Point(p.X+dx*5,p.Y+dy*5)),2)!=h)
            throw new InvalidOperationException("H83 relative motion lacks owned client clearance");
        Send(0x2001u,dx,dy,0); // MOVE | MOVE_NOCOALESCE; never SetCursorPos as evidence
    }
    public static void Wheel(IntPtr h, bool horizontal, int delta) { RequirePointer(h); Send(horizontal?0x1000u:0x800u,0,0,delta); }
    public static string[] ReleaseAll() {
        var errors=new List<string>();
        try { Middle(IntPtr.Zero,true); } catch(Exception e) { errors.Add(e.ToString()); }
        return errors.ToArray();
    }
    static void OnUI(Owner o, MethodInvoker action) {
        IAsyncResult call=o.Form.BeginInvoke(action);
        using(var wait=call.AsyncWaitHandle) { if(!wait.WaitOne(3000)) throw new TimeoutException("H83 UI timeout"); o.Form.EndInvoke(call); }
    }
    static void Cancel(Owner o) {
        o.Cancelled=true; var f=o.Form;
        try { if(f!=null && f.IsHandleCreated && !f.IsDisposed) f.BeginInvoke((MethodInvoker)delegate { f.Close(); }); }
        catch(InvalidOperationException) { }
    }
    public static IntPtr Open() {
        var o=new Owner();
        o.Thread=new Thread(delegate() {
            try {
                SetThreadDpiAwarenessContext(new IntPtr(-4));
                if(o.Cancelled) return;
                var f=new Probe(); o.Form=f;
                f.Text="lane-h83-button-scroll"; f.Size=new Size(640,440);
                f.StartPosition=FormStartPosition.CenterScreen; f.BackColor=Color.FromArgb(42,76,96);
                f.Shown+=delegate {
                    o.Handle=f.Handle;
                    lock(Sync) { if(!o.Cancelled) Forms[o.Handle]=o; }
                    o.Ready.Set(); if(o.Cancelled) f.Close();
                };
                if(o.Cancelled) return;
                Application.Run(f);
            } catch(Exception e) { o.Error=e; }
            finally { if(o.Form!=null) o.Form.Dispose(); lock(Sync) Forms.Remove(o.Handle); o.Ready.Set(); }
        });
        o.Thread.IsBackground=true; o.Thread.SetApartmentState(ApartmentState.STA);
        lock(Sync) Owners.Add(o);
        o.Thread.Start();
        if(!o.Ready.WaitOne(8000)) { Cancel(o); throw new TimeoutException("H83 form startup timeout"); }
        if(o.Error!=null || o.Handle==IntPtr.Zero || !Owns(o.Handle)) { Cancel(o); throw new InvalidOperationException("H83 form startup failed",o.Error); }
        return o.Handle;
    }
    public static void Activate(IntPtr h) { var o=Find(h); OnUI(o,delegate { o.Form.Activate(); o.Form.Focus(); }); }
    public static void PlaceAtCenter(IntPtr h) {
        var o=Find(h); Point pt=new Point();
        OnUI(o,delegate { pt=o.Form.PointToScreen(new Point(o.Form.ClientSize.Width/2,o.Form.ClientSize.Height/2)); });
        if(GetForegroundWindow()!=h || GetAncestor(WindowFromPoint(pt),2)!=h) throw new InvalidOperationException("H83 center obscured");
        RestoreCursor(pt);
    }
    public static void Clear(IntPtr h) { var f=Find(h).Form; lock(f.InputEvents) f.InputEvents.Clear(); }
    public static string[] Events(IntPtr h) { var f=Find(h).Form; lock(f.InputEvents) return f.InputEvents.ToArray(); }
    public static void CloseAll() {
        Owner[] owners; lock(Sync) owners=Owners.ToArray();
        foreach(var o in owners) Cancel(o);
        foreach(var o in owners) if(!o.Thread.Join(5000)) throw new TimeoutException("H83 owned form did not close");
        lock(Sync) Owners.Clear();
    }
    public static string Request(int expectedPid,string fullPipe,string json) {
        string name=fullPipe.StartsWith(@"\\.\pipe\")?fullPipe.Substring(9):fullPipe;
        using(var pipe=new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            pipe.Connect(2000); uint actualPid;
            if(!GetNamedPipeServerProcessId(pipe.SafePipeHandle,out actualPid)) throw new Win32Exception();
            if(actualPid!=(uint)expectedPid) throw new InvalidOperationException("H83 refuses unowned pipe PID "+actualPid);
            byte[] bytes=Encoding.UTF8.GetBytes(json+"\n");
            var write=pipe.BeginWrite(bytes,0,bytes.Length,null,null);
            using(var wait=write.AsyncWaitHandle) { if(!wait.WaitOne(2000)) throw new TimeoutException("H83 pipe write"); pipe.EndWrite(write); }
            using(var reader=new StreamReader(pipe,Encoding.UTF8)) {
                var read=reader.ReadLineAsync();
                if(!read.Wait(4000) || read.Result==null) throw new TimeoutException("H83 pipe reply");
                return read.Result;
            }
        }
    }
}
'@ -ErrorAction Stop
        }
        $bsNativeReady = $true
        $bsOldDpi = [H83ButtonScrollProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if ($bsOldDpi -eq [IntPtr]::Zero) { throw 'H83 cannot establish physical-pixel observation context' }
        $bsSavedCursor = [H83ButtonScrollProbe]::Cursor()
        $env:WIRI_PIPE = $bsPipe
        $bsInitial = BSConfig ''
        [IO.File]::WriteAllText($cfg, $bsInitial, (New-Object Text.UTF8Encoding($false)))
        $bsOldRoaming = $env:APPDATA; $bsOldLocal = $env:LOCALAPPDATA
        $bsOldPointerDry = $env:WIRI_POINTER_SETTINGS_DRY_RUN
        try {
            $env:APPDATA = $testRoaming; $env:LOCALAPPDATA = $testLocal
            # No configured SPI options; retain a dry-run backend as an extra
            # barrier against future fixture changes acquiring global settings.
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = Join-Path $bsDir 'pointer-audit.ndjson'
            $bsProcess = Start-Process -FilePath $wiri -ArgumentList @('-v', '--no-startup-overlay', '-c', (Q $cfg)) `
                -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $bsDir 'wiri.stderr.log') `
                -RedirectStandardOutput (Join-Path $bsDir 'wiri.stdout.log')
            $null = $bsProcess.Handle
        } finally {
            $env:APPDATA = $bsOldRoaming; $env:LOCALAPPDATA = $bsOldLocal
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = $bsOldPointerDry
        }
        $bsDetails['daemon_pid'] = $bsProcess.Id; $bsDetails['pipe'] = $bsPipe
        $bsReady = $false
        for ($attempt = 0; $attempt -lt 30 -and -not $bsReady; $attempt++) {
            if ($bsProcess.HasExited) { throw 'H83 daemon exited during startup' }
            try { $null = BSRequest 'ready' @{ type = 'get_state' }; $bsReady = $true }
            catch { if ($attempt -eq 29) { throw }; Start-Sleep -Milliseconds 200 }
        }
        Start-Sleep -Milliseconds 800
        $bsForm = [H83ButtonScrollProbe]::Open()
        Start-Sleep -Milliseconds 1200
        BSFocus; BSClear; BSClick
        $bsBaseline = BSObserve 'unconfigured-click'
        BSGate 'h83_unconfigured_middle_click_pair_passes' ((BSPair $bsBaseline) -and (BSWheels $bsBaseline) -eq 0) $bsBaseline

        $bsHoldConfig = BSConfig 'mouse { scroll-method "on-button-down"; scroll-button 274; }'
        BSReload 'hold' $bsHoldConfig
        BSFocus; BSClear
        $bsAnchor = [H83ButtonScrollProbe]::Cursor()
        try {
            [H83ButtonScrollProbe]::Middle($bsForm, $false)
            BSMotion 0 10; BSMotion 10 0
            $bsHeld = BSObserve 'held-both-axes'
        } finally { [H83ButtonScrollProbe]::Middle($bsForm, $true) }
        $bsHeldEdges = BSObserve 'hold-after-release'
        $bsBothAxes = @($bsHeld.events | Where-Object { $_ -like 'W:V:*' }).Count -gt 0 -and
            @($bsHeld.events | Where-Object { $_ -like 'W:H:*' }).Count -gt 0
        BSGate 'h83_hold_motion_delivers_both_wheel_axes' $bsBothAxes $bsHeld
        BSGate 'h83_hold_consumes_both_button_edges' ($bsBothAxes -and (BSNoEdges $bsHeldEdges)) $bsHeldEdges
        BSGate 'h83_hold_freezes_cursor_anchor' ($bsBothAxes -and (BSNearPoint $bsAnchor $bsHeld.cursor)) @{ anchor = $bsAnchor; observed = $bsHeld }
        BSClear; BSMotion 0 10
        $bsReleased = BSObserve 'released-motion'
        BSGate 'h83_release_stops_scrolling' ($bsBothAxes -and (BSWheels $bsReleased) -eq 0 -and -not (BSNearPoint $bsHeld.cursor $bsReleased.cursor)) $bsReleased

        BSReload 'disabled-after-hold' (BSConfig 'mouse { scroll-method "on-button-down"; scroll-button 0; }')
        BSFocus; BSClear; BSClick
        $bsAfterDisable = BSObserve 'disabled-click'
        BSGate 'h83_unconfigured_click_after_release_passes' ((BSPair $bsAfterDisable) -and (BSWheels $bsAfterDisable) -eq 0) $bsAfterDisable

        $bsLockConfig = BSConfig 'mouse { scroll-method "on-button-down"; scroll-button 274; scroll-button-lock; }'
        BSReload 'lock' $bsLockConfig
        BSFocus; BSClear; BSClick
        $bsLockAnchor = [H83ButtonScrollProbe]::Cursor()
        BSMotion 0 10
        $bsLatched = BSObserve 'latched-motion'
        $bsLatchPass = (BSWheels $bsLatched) -gt 0 -and (BSNoEdges $bsLatched) -and (BSNearPoint $bsLockAnchor $bsLatched.cursor)
        BSGate 'h83_short_tap_latches_scroll_lock' $bsLatchPass $bsLatched
        BSClear; BSClick
        $bsUnlockTap = BSObserve 'unlock-tap-edges'
        BSClear; BSMotion 10 0
        $bsUnlocked = BSObserve 'unlocked-motion'
        BSGate 'h83_second_tap_unlocks_scroll_lock' ($bsLatchPass -and (BSNoEdges $bsUnlockTap) -and
            (BSWheels $bsUnlocked) -eq 0 -and -not (BSNearPoint $bsLockAnchor $bsUnlocked.cursor)) @{
                unlock_tap = $bsUnlockTap; motion_after_unlock = $bsUnlocked }

        BSFocus; BSClick; BSClear; BSMotion 0 10
        $bsBeforeReload = BSObserve 'latch-before-reload'
        BSReload 'cancel-lock' (BSConfig '')
        BSClear; BSMotion 0 10
        $bsCancelled = BSObserve 'latch-after-reload'
        BSGate 'h83_reload_cancels_latched_scroll' ((BSWheels $bsBeforeReload) -gt 0 -and (BSWheels $bsCancelled) -eq 0 -and
            -not (BSNearPoint $bsBeforeReload.cursor $bsCancelled.cursor)) @{ before = $bsBeforeReload; after = $bsCancelled }

        BSReload 'paired-hold' $bsHoldConfig
        BSFocus; BSClear
        try {
            [H83ButtonScrollProbe]::Middle($bsForm, $false)
            BSMotion 0 10
            $bsBeforeHeldReload = BSObserve 'held-before-reload'
            BSReload 'cancel-held' (BSConfig '')
        } finally { [H83ButtonScrollProbe]::Middle($bsForm, $true) }
        $bsPairedReload = BSObserve 'release-after-reload'
        BSClear; BSClick
        $bsReloadNextClick = BSObserve 'next-click-after-held-reload'
        BSGate 'h83_reload_keeps_consumed_release_paired' ((BSWheels $bsBeforeHeldReload) -gt 0 -and
            (BSNoEdges $bsPairedReload) -and (BSPair $bsReloadNextClick)) @{ captured = $bsPairedReload; next_click = $bsReloadNextClick }

        $bsMiddleBind = BSBind 'middle-bind'
        BSReload 'middle-bind' (BSConfig 'mouse { scroll-method "on-button-down"; scroll-button 274; scroll-button-lock; }' ("MouseMiddle { $bsMiddleBind; }"))
        BSFocus; BSClear; BSClick; BSMotion 0 10
        $bsBound = BSObserve 'explicit-middle-bind' 3400
        BSGate 'h83_explicit_middle_button_bind_wins' ((BSCount 'middle-bind') -eq 1 -and (BSNoEdges $bsBound) -and
            (BSWheels $bsBound) -eq 0) @{ observed = $bsBound; marker_count = (BSCount 'middle-bind') }

        $bsWheelBind = BSBind 'wheel-bind'
        BSReload 'wheel-binds' (BSConfig 'mouse { scroll-method "on-button-down"; scroll-button 274; scroll-factor 0; }' ("WheelScrollDown { $bsWheelBind; }"))
        BSFocus; BSClear
        [H83ButtonScrollProbe]::Wheel($bsForm, $false, -120)
        $bsPhysical = BSObserve 'physical-wheel-bound' 3400
        $bsPhysicalCount = BSCount 'wheel-bind'
        BSGate 'h83_physical_wheel_bind_positive_control' ($bsPhysicalCount -eq 1 -and (BSWheels $bsPhysical) -eq 0) @{
            observed = $bsPhysical; marker_count = $bsPhysicalCount }
        BSClear
        try {
            [H83ButtonScrollProbe]::Middle($bsForm, $false)
            BSMotion 0 10; BSMotion 10 0
            $bsBypass = BSObserve 'continuous-wheel-bypass' 3400
        } finally { [H83ButtonScrollProbe]::Middle($bsForm, $true) }
        $bsBypassBoth = @($bsBypass.events | Where-Object { $_ -like 'W:V:*' }).Count -gt 0 -and
            @($bsBypass.events | Where-Object { $_ -like 'W:H:*' }).Count -gt 0
        BSGate 'h83_emulated_wheel_bypasses_physical_bind_and_global_factor' ($bsPhysicalCount -eq 1 -and
            (BSCount 'wheel-bind') -eq 1 -and $bsBypassBoth -and (BSNoEdges $bsBypass)) @{
                observed = $bsBypass; physical_scroll_factor = 0; marker_count = (BSCount 'wheel-bind') }

        BSReload 'touchpad-only' (BSConfig 'touchpad { scroll-method "on-button-down"; scroll-button 274; scroll-button-lock; }')
        BSFocus; BSClear
        try {
            [H83ButtonScrollProbe]::Middle($bsForm, $false)
            BSMotion 0 10
        } finally { [H83ButtonScrollProbe]::Middle($bsForm, $true) }
        $bsTouchpad = BSObserve 'touchpad-only-middle-motion'
        BSGate 'h83_touchpad_only_config_does_not_enable_mouse_scroll' ((BSPair $bsTouchpad) -and (BSWheels $bsTouchpad) -eq 0) $bsTouchpad
        $bsDetails['stage'] = 'gates-complete'
    } catch {
        $bsDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        # Release only successful downs injected by this lane. This precedes
        # form closure, daemon shutdown, and restoration of the saved cursor.
        if ($bsNativeReady) {
            foreach ($releaseError in [H83ButtonScrollProbe]::ReleaseAll()) { BSCleanupError 'release-owned-middle' $releaseError }
            try { [H83ButtonScrollProbe]::CloseAll() } catch { BSCleanupError 'owned-form-close' $_ }
        }
        if ($null -ne $bsProcess) {
            try {
                if (-not $bsProcess.HasExited) {
                    try { $null = BSRequest 'quit-owned' @{ type = 'quit' } }
                    catch { $bsDetails['quit_reply_error'] = [string]$_ }
                    if (-not $bsProcess.WaitForExit(8000)) {
                        BSCleanupError 'owned-daemon' 'graceful quit timed out; killing retained owned process'
                        $bsProcess.Kill()
                        if (-not $bsProcess.WaitForExit(5000)) { throw 'owned daemon did not exit' }
                    }
                }
            } catch { BSCleanupError 'owned-daemon-stop' $_ }
        }
        try { if ($null -ne $bsSavedCursor) { [H83ButtonScrollProbe]::RestoreCursor($bsSavedCursor) } }
        catch { BSCleanupError 'cursor-restore' $_ }
        try {
            if ($null -ne $bsSavedConfig) { [IO.File]::WriteAllText($cfg, $bsSavedConfig, (New-Object Text.UTF8Encoding($false))) }
            elseif (Test-Path -LiteralPath $cfg) { Remove-Item -LiteralPath $cfg -Force }
        } catch { BSCleanupError 'config-restore' $_ }
        $env:WIRI_PIPE = $bsSavedPipe
        if ($bsOldDpi -ne [IntPtr]::Zero) {
            try {
                if ([H83ButtonScrollProbe]::SetThreadDpiAwarenessContext($bsOldDpi) -eq [IntPtr]::Zero) { throw 'DPI context restore failed' }
            } catch { BSCleanupError 'dpi-context-restore' $_ }
        }
        BSGate 'h83_cleanup' ($bsDetails.cleanup_errors.Count -eq 0 -and ($null -eq $bsProcess -or $bsProcess.HasExited)) @{
            errors = $bsDetails.cleanup_errors; daemon_started = ($null -ne $bsProcess); cursor_saved = ($null -ne $bsSavedCursor) }
        $summary['lane_button_scroll_details'] = $bsDetails
        BSEvidence
    }
}
