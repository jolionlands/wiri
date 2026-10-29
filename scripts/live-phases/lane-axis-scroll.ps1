# H87: application-delivered horizontal/vertical wheel scaling on one owned HWND.
# Dot-source only under live-test.ps1's desktop mutex. Synthetic SendInput with
# extra-info zero traverses the normal WH_MOUSE_LL route; it is NOT hardware or
# per-device evidence. No system pointer settings, modifiers or buttons are sent.
# This phase is intentionally not wired into live-test.ps1 until native review.
# Oracle sources at niri e19df12cf8 (the local pinned checkout):
# - niri-config/src/input.rs:168-183: explicit H/V replaces base, absent axis
#   falls back to base/1. src/input/mod.rs:3588-3618 multiplies device and rule.
# - src/input/mod.rs:5067-5078 applies config.mouse.natural_scroll to libinput;
#   :3181-3191 reads both device axes and :3614-3640 forwards both to the app.
#   Both-axis application reversal is inferred from that device-wide natural
#   setting; it is not inferred from pointer-bind direction or scroll UI pixels.
# - Wheel bind handling at :3200-3397 precedes device/rule factors :3588-3618.
# Windows adapter conventions, not niri protocol values:
# - src/input/low_level_hook.rs:760-763 bypasses only rule_scroll::REINJECTED;
#   :936-954 dispatches binds before unbound application scaling.
# - src/input/rule_scroll.rs REINJECTED=0x57495257 is measured via the receiver's
#   GetMessageExtraInfo, never set by this fixture's SendInput calls.
& {
    $ErrorActionPreference = 'Stop'
    $h87Dir = Join-Path $D ('h87-axis-scroll-' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $h87Dir
    $h87Utf8 = New-Object Text.UTF8Encoding($false)
    $h87Gates = @(
        'h87_unconfigured_both_axes_positive_control',
        'h87_scalar_positive_both_axes',
        'h87_property_only_axis_overrides',
        'h87_negative_axis_reverses_application_delta',
        'h87_zero_axis_suppresses_only_that_axis',
        'h87_mixed_base_axis_override_replaces_base',
        'h87_interleaved_low_delta_residual_is_axis_local',
        'h87_window_rule_multiplies_axis_factors',
        'h87_natural_scroll_reverses_application_axes',
        'h87_bound_wheel_dispatches_before_zero_factor',
        'h87_replacement_tag_bypasses_recursion',
        'h87_owned_cleanup'
    )
    $h87Evidence = [ordered]@{
        scope = 'Owned application wheel messages from untagged synthetic SendInput; no hardware/device identity claim'
        fixture_directory = $h87Dir
        touchpad_routing_info = [ordered]@{
            status = 'unverified: no physical touchpad reports or device-specific routing exercised'
            reason = 'WH_MOUSE_LL wheel packets do not establish the physical source device'
            counted_as_pass = $false
        }
        gates = [ordered]@{}
        cleanup_errors = @()
    }
    foreach ($gate in $h87Gates) {
        $summary[$gate] = $false
        $h87Evidence.gates[$gate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $h87State = @{ daemon = $null; rpc = $null; native_ready = $false; forms_ready = $false }
    $h87Forms = New-Object System.Collections.ArrayList
    $h87Prefix = 'lane-h87-' + $PID + '-' + [Guid]::NewGuid().ToString('N') + '-'
    $h87Title = $h87Prefix + 'wheel-receiver'
    $h87Form = [IntPtr]::Zero
    $h87SavedCursor = $null
    $h87OldDpi = [IntPtr]::Zero
    $h87SavedPipe = $env:WIRI_PIPE
    $h87SavedConfig = $env:WIRI_CONFIG
    $h87SavedNiriConfig = $env:NIRI_CONFIG
    $h87Pipe = '\\.\pipe\wiri_live_h87_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $testRoaming = Join-Path $h87Dir 'profile\roaming'
    $testLocal = Join-Path $h87Dir 'profile\local'
    $cfg = Join-Path $testRoaming 'wiri\config.kdl'
    foreach ($path in @((Split-Path $cfg), $testLocal)) { $null = New-Item -ItemType Directory -Force -Path $path }

    function H87Save {
        [IO.File]::WriteAllText((Join-Path $h87Dir 'evidence.json'),
            ($h87Evidence | ConvertTo-Json -Depth 18), $h87Utf8)
    }
    function H87Gate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $h87Evidence.gates[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        H87Save
    }
    function H87Wire([string]$tag, [string]$direction, [string]$text) {
        # Pipe methods return fresh .NET strings, not ETS-decorated Get-Content results.
        [IO.File]::AppendAllText((Join-Path $h87Dir 'wire.ndjson'),
            (([ordered]@{ utc = [DateTime]::UtcNow.ToString('o'); tag = $tag; direction = $direction; text = $text } |
                ConvertTo-Json -Compress) + "`n"), $h87Utf8)
    }
    function H87Connect {
        if ($null -eq $h87State.daemon -or $h87State.daemon.HasExited -or $env:WIRI_PIPE -ne $h87Pipe) {
            throw 'H87 retained daemon/pipe ownership is no longer valid'
        }
        return (New-Object H87AxisPipe $h87Pipe, $h87State.daemon.Id)
    }
    function H87Read($connection, [string]$tag, [int]$milliseconds = 5000) {
        $line = $connection.ReadLine($milliseconds)
        if ($null -eq $line) { throw "H87 $tag reply timed out" }
        H87Wire $tag 'reply' $line
        return ($line | ConvertFrom-Json)
    }
    function H87Request([string]$tag, $request) {
        if ($null -eq $h87State.rpc -or $h87State.daemon.HasExited -or $env:WIRI_PIPE -ne $h87Pipe) {
            throw 'H87 command connection is unavailable'
        }
        $json = $request | ConvertTo-Json -Depth 8 -Compress
        H87Wire $tag 'request' $json
        try {
            $h87State.rpc.Send($json + "`n")
            return (H87Read $h87State.rpc $tag)
        } catch {
            # Never resend an action, or consume a late reply as the next request.
            $h87State.rpc.Dispose(); $h87State.rpc = $null
            throw
        }
    }
    function H87Config([string]$mouse, [string]$binds = '', [string]$rule = '') {
        return @"
animations { off; }
input { mod-key "ctrl-alt"; focus-follows-mouse false; mouse { $mouse } }
hotkey-overlay { skip-at-startup; }
window-rule { match title="^$h87Title`$"; open-floating true; $rule }
binds { $binds }
"@
    }
    function H87Reload([string]$tag, [string]$text) {
        $events = H87Connect
        try {
            # Observe the initial replay before changing the file. One outstanding
            # read survives a timeout, as in LaneHConnection; no overlapping reads.
            $events.Send("{`"type`":`"subscribe_events`",`"data`":{`"event_types`": [`"config_loaded`"]}}`n")
            $ack = H87Read $events ($tag + '-subscribe')
            if ($ack.success -ne $true) { throw 'H87 config event subscription rejected' }
            $initial = H87Read $events ($tag + '-initial-config')
            if ($initial.type -ne 'config_loaded') { throw 'H87 expected config replay before reload' }
            # Drain any previous reload notifications. No dispatch occurs here.
            $drain = [Diagnostics.Stopwatch]::StartNew()
            do {
                $old = $events.ReadLine(200)
                if ($null -eq $old) { break }
                H87Wire ($tag + '-pre-reload-drain') 'event' $old
            } while ($drain.ElapsedMilliseconds -lt 2000)
            if ($null -ne $old) { throw 'H87 prior config event stream did not settle' }
            [IO.File]::WriteAllText($cfg, $text, $h87Utf8)
            [IO.File]::WriteAllText((Join-Path $h87Dir ($tag + '.kdl')), $text, $h87Utf8)
            $reply = H87Request ($tag + '-reload') @{ type = 'reload_config' }
            if ($reply.success -ne $true) { throw "H87 config rejected: $($reply | ConvertTo-Json -Compress)" }
            $loaded = H87Read $events ($tag + '-loaded') 6000
            if ($loaded.type -ne 'config_loaded' -or $loaded.data.failed -ne $false -or
                $loaded.data.config_path -ne $cfg) { throw "H87 reload completion was not successful for $cfg" }
            # File watcher (200 ms trailing debounce) and explicit reload can
            # both complete. Observe a 700 ms quiet interval so a second reload
            # cannot reset fractional residuals in the middle of a sequence.
            $completions = New-Object System.Collections.ArrayList
            [void]$completions.Add($loaded)
            $settle = [Diagnostics.Stopwatch]::StartNew()
            do {
                $line = $events.ReadLine(700)
                if ($null -eq $line) { break }
                H87Wire ($tag + '-reload-settle') 'event' $line
                $event = $line | ConvertFrom-Json
                if ($event.type -ne 'config_loaded' -or $event.data.failed -ne $false -or
                    $event.data.config_path -ne $cfg) { throw 'H87 reload changed or failed during settling' }
                [void]$completions.Add($event)
            } while ($settle.ElapsedMilliseconds -lt 6000)
            if ($null -ne $line) { throw 'H87 config completions did not settle' }
            return [ordered]@{ requested_config = $text; reload_reply = $reply; completions = @($completions) }
        } finally { $events.Dispose() }
    }
    function H87Focus {
        if (-not $h87Forms.Contains($h87Form) -or -not [H87AxisProbe]::Owns($h87Form)) {
            throw 'H87 receiver ownership lost'
        }
        # One explicit focus request; waits observe only and never redispatch it.
        $reply = H87Request 'focus-owned-receiver' @{ Action = @{ FocusWindow = @{ id = [int64]$h87Form } } }
        if ($reply.Ok -ne 'Handled') { throw 'H87 owned focus action rejected' }
        if (-not (WaitForeground $h87Form 200 4000)) { throw 'H87 receiver did not gain native foreground' }
        [H87AxisProbe]::CenterPointer($h87Form)
        Start-Sleep -Milliseconds 180
        [H87AxisProbe]::RequirePointer($h87Form)
    }
    function H87Observe([string]$tag, [int]$minimumMs = 550) {
        # Poll only observations. Injections happen once in H87Sequence.
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $changedAt = 0L; $previous = -1
        do {
            Start-Sleep -Milliseconds 50
            $count = [H87AxisProbe]::Count($h87Form)
            if ($count -ne $previous) { $previous = $count; $changedAt = $clock.ElapsedMilliseconds }
        } while ($clock.ElapsedMilliseconds -lt 2500 -and
            ($clock.ElapsedMilliseconds -lt $minimumMs -or $clock.ElapsedMilliseconds - $changedAt -lt 400))
        $native = [H87AxisProbe]::Snapshot($h87Form)
        $observed = [ordered]@{
            utc = [DateTime]::UtcNow.ToString('o'); elapsed_ms = $clock.ElapsedMilliseconds
            quiet = ($clock.ElapsedMilliseconds - $changedAt -ge 400)
            native = $native; events = @([H87AxisProbe]::Events($h87Form))
            dropped = [H87AxisProbe]::Dropped($h87Form)
        }
        [IO.File]::WriteAllText((Join-Path $h87Dir ($tag + '.json')),
            ($observed | ConvertTo-Json -Depth 8), $h87Utf8)
        return $observed
    }
    function H87Sequence([string]$tag, $inputs) {
        H87Focus
        # Drain old delivery before the UI-thread clear; no synthetic sentinel is
        # mixed into a suppression or low-delta case.
        Start-Sleep -Milliseconds 250
        [H87AxisProbe]::Clear($h87Form)
        $before = [H87AxisProbe]::Snapshot($h87Form)
        $sent = New-Object System.Collections.ArrayList
        foreach ($packet in $inputs) {
            $native = [H87AxisProbe]::Snapshot($h87Form)
            [H87AxisProbe]::Wheel($h87Form, ($packet.axis -eq 'H'), [int]$packet.delta)
            [void]$sent.Add([ordered]@{
                utc = [DateTime]::UtcNow.ToString('o'); axis = $packet.axis; delta = [int]$packet.delta
                extra_info = 0; before = $native
            })
            Start-Sleep -Milliseconds 90
        }
        $observation = H87Observe $tag
        $observation['before'] = $before
        $observation['sent_once'] = @($sent)
        return $observation
    }
    function H87Exact($observation, [string[]]$expected) {
        $actual = @($observation.events | ForEach-Object { $_.Axis + ':' + $_.Delta })
        return $observation.quiet -and $observation.dropped -eq 0 -and
            $observation.native.Foreground -eq [int64]$h87Form -and $observation.native.PointerInside -and
            $actual.Count -eq $expected.Count -and (($actual -join ',') -ceq ($expected -join ','))
    }
    function H87Case([string]$gate, [string]$tag, [string]$mouse, $inputs, [string[]]$expected,
        [string]$rule = '', [switch]$requireReplacementTag) {
        $h87Evidence['stage'] = $tag
        try {
            $config = H87Reload $tag (H87Config $mouse '' $rule)
            $observed = H87Sequence $tag $inputs
            $pass = H87Exact $observed $expected
            if ($requireReplacementTag) {
                $pass = $pass -and @($observed.events).Count -eq 1 -and $observed.events[0].ExtraInfo -eq 0x57495257
            }
            H87Gate $gate $pass ([ordered]@{
                expected = $expected; config = $config; observation = $observed
                replacement_tag_required = [bool]$requireReplacementTag
            })
            return $observed
        } catch {
            $h87Evidence.gates[$gate] = [ordered]@{
                pass = $false; status = 'error'; requested_mouse = $mouse; requested_rule = $rule
                error = [string]$_; utc = [DateTime]::UtcNow.ToString('o')
            }
            H87Save
            # A rejected config remains a RED and later cases can reload a valid
            # file. Lost ownership/connection aborts further dispatch instead.
            if ($null -eq $h87State.rpc -or $h87State.daemon.HasExited -or
                -not [H87AxisProbe]::Owns($h87Form)) { throw }
            return $null
        }
    }

    try {
        if (-not $ownsLiveLock) { throw 'H87 requires the parent Global\wiri-live-test mutex' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H87 refuses to start beside any existing daemon'
        }
        . (Join-Path (Split-Path $PSScriptRoot -Parent) 'live-fixtures\owned-forms.ps1')
        $h87State.forms_ready = $true
        if (-not ('H87AxisProbe' -as [type])) {
            Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing,System.Core -TypeDefinition @'
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
using System.Threading.Tasks;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;

// Same retained-read and server-PID checks as LaneHConnection; standalone so a
// focused runner need not execute Lane H to load its support type.
public sealed class H87AxisPipe : IDisposable {
    readonly NamedPipeClientStream pipe;
    readonly StreamReader reader;
    Task<string> pending;
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle h, out uint pid);
    public H87AxisPipe(string path, int expectedPid) {
        const string prefix = @"\\.\pipe\";
        if (path == null || !path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("Expected local pipe");
        pipe = new NamedPipeClientStream(".", path.Substring(prefix.Length), PipeDirection.InOut, PipeOptions.Asynchronous);
        try {
            pipe.Connect(1500); uint actualPid;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out actualPid)) throw new Win32Exception();
            if (actualPid != (uint)expectedPid) throw new InvalidOperationException("Refused unowned H87 pipe server");
            reader = new StreamReader(pipe, new UTF8Encoding(false,true), false, 4096, true);
        } catch { pipe.Dispose(); throw; }
    }
    public void Send(string text) {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        if (bytes.Length > 65536) throw new ArgumentException("H87 request too large");
        Task write = pipe.WriteAsync(bytes,0,bytes.Length);
        if (!write.Wait(1500)) { Dispose(); throw new TimeoutException("H87 pipe write"); }
        write.GetAwaiter().GetResult();
    }
    public string ReadLine(int milliseconds) {
        if (pending == null) pending = reader.ReadLineAsync();
        if (!pending.Wait(milliseconds)) return null;
        string line = pending.GetAwaiter().GetResult(); pending = null;
        if (line == null) throw new EndOfStreamException("H87 owned pipe closed");
        return line;
    }
    public void Dispose() { pipe.Dispose(); if (reader != null) reader.Dispose(); }
}
public sealed class H87WheelEvent {
    public string Axis, Utc; public int Delta; public long Hwnd, ExtraInfo, Tick; public uint Pid;
}
public sealed class H87NativeSnapshot {
    public long Hwnd, Foreground, PointerRoot; public uint Pid, Dpi; public bool Visible, Iconic, PointerInside;
    public int CursorX, CursorY, ClientLeft, ClientTop, ClientRight, ClientBottom; public string Utc;
}
public static class H87AxisProbe {
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x,y; public uint data,flags,time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public MI mouse; }
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int left,top,right,bottom; }
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetCursorPos(out Point point);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point point);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h,uint flags);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
    [DllImport("user32.dll", SetLastError=true)] static extern bool GetClientRect(IntPtr h,out RECT rect);
    [DllImport("user32.dll", SetLastError=true)] static extern bool ClientToScreen(IntPtr h,ref Point point);
    [DllImport("user32.dll")] static extern IntPtr GetMessageExtraInfo();
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    static readonly object Sync = new object();
    static readonly Dictionary<IntPtr,Receiver> Receivers = new Dictionary<IntPtr,Receiver>();
    sealed class Receiver : NativeWindow {
        public readonly IntPtr Target; public readonly Form Form;
        public readonly List<H87WheelEvent> Events = new List<H87WheelEvent>();
        public int Dropped;
        public Receiver(IntPtr target,Form form) { Target=target; Form=form; }
        protected override void WndProc(ref Message m) {
            if (m.Msg==0x20A || m.Msg==0x20E) {
                uint pid; GetWindowThreadProcessId(Target,out pid);
                var observed = new H87WheelEvent {
                    Axis=m.Msg==0x20A?"V":"H", Delta=unchecked((short)((m.WParam.ToInt64()>>16)&65535)),
                    Hwnd=Target.ToInt64(), Pid=pid, ExtraInfo=GetMessageExtraInfo().ToInt64(),
                    Utc=DateTime.UtcNow.ToString("o"), Tick=Stopwatch.GetTimestamp()
                };
                lock(Events) { if(Events.Count<256) Events.Add(observed); else Dropped++; }
            }
            bool destroyed=m.Msg==0x82;
            base.WndProc(ref m);
            if(destroyed) lock(Sync) Receivers.Remove(Target);
        }
    }
    static void RequireOwnedWindow(IntPtr h) {
        uint pid; GetWindowThreadProcessId(h,out pid);
        if(!IsWindow(h) || pid!=(uint)Process.GetCurrentProcess().Id) throw new InvalidOperationException("H87 refused foreign/missing HWND");
    }
    static Receiver Find(IntPtr h) {
        RequireOwnedWindow(h);
        lock(Sync) { Receiver receiver; if(!Receivers.TryGetValue(h,out receiver)) throw new InvalidOperationException("H87 receiver not registered"); return receiver; }
    }
    // Monitor owns completion for late callbacks; no disposed event can race UI.
    static void OnUI(Form form, MethodInvoker action) {
        var gate=new object(); bool done=false; Exception error=null;
        form.BeginInvoke((MethodInvoker)delegate {
            try { action(); } catch(Exception failure) { error=failure; }
            finally { lock(gate) { done=true; Monitor.PulseAll(gate); } }
        });
        var clock=Stopwatch.StartNew();
        lock(gate) {
            while(!done && clock.ElapsedMilliseconds<3000) Monitor.Wait(gate,(int)Math.Max(1,3000-clock.ElapsedMilliseconds));
            if(!done) throw new TimeoutException("H87 owned UI observation timed out");
            if(error!=null) throw new InvalidOperationException("H87 owned UI callback failed",error);
        }
    }
    public static void Attach(IntPtr h) {
        RequireOwnedWindow(h);
        Form form=Control.FromHandle(h) as Form;
        if(form==null) throw new InvalidOperationException("H87 expected its owned WinForms Form");
        var receiver=new Receiver(h,form);
        // Registered before queueing attachment; late work remains bounded by the
        // shared form registry, whose UI-thread disposal also destroys this HWND.
        lock(Sync) { if(Receivers.ContainsKey(h)) throw new InvalidOperationException("H87 duplicate receiver"); Receivers.Add(h,receiver); }
        OnUI(form,delegate { RequireOwnedWindow(h); receiver.AssignHandle(h); });
    }
    public static bool Owns(IntPtr h) {
        uint pid; GetWindowThreadProcessId(h,out pid);
        lock(Sync) return IsWindow(h) && pid==(uint)Process.GetCurrentProcess().Id && Receivers.ContainsKey(h);
    }
    public static int ReceiverCount { get { lock(Sync) return Receivers.Count; } }
    public static Point Cursor() { Point p; if(!GetCursorPos(out p)) throw new Win32Exception(); return p; }
    public static void RestoreCursor(Point p) { if(!SetCursorPos(p.X,p.Y)) throw new Win32Exception(); }
    static void ClientBounds(IntPtr h,out Point a,out Point b) {
        RECT rect; if(!GetClientRect(h,out rect)) throw new Win32Exception();
        a=new Point(rect.left,rect.top); b=new Point(rect.right,rect.bottom);
        if(!ClientToScreen(h,ref a) || !ClientToScreen(h,ref b)) throw new Win32Exception();
    }
    public static H87NativeSnapshot Snapshot(IntPtr h) {
        Find(h); Point p=Cursor(),a,b; ClientBounds(h,out a,out b); uint pid;
        GetWindowThreadProcessId(h,out pid); IntPtr root=GetAncestor(WindowFromPoint(p),2);
        return new H87NativeSnapshot {
            Hwnd=h.ToInt64(),Pid=pid,Dpi=GetDpiForWindow(h),Foreground=GetForegroundWindow().ToInt64(),
            PointerRoot=root.ToInt64(),Visible=IsWindowVisible(h),Iconic=IsIconic(h),
            PointerInside=root==h && p.X>a.X+2 && p.Y>a.Y+2 && p.X<b.X-2 && p.Y<b.Y-2,
            CursorX=p.X,CursorY=p.Y,ClientLeft=a.X,ClientTop=a.Y,ClientRight=b.X,ClientBottom=b.Y,Utc=DateTime.UtcNow.ToString("o")
        };
    }
    public static void RequirePointer(IntPtr h) {
        var observed=Snapshot(h);
        if(observed.Foreground!=h.ToInt64() || !observed.Visible || observed.Iconic || !observed.PointerInside)
            throw new InvalidOperationException("H87 input refused: owned foreground and unobscured client pointer required");
        foreach(int vk in new int[] {1,2,4,5,6,16,17,18,91,92})
            if(GetAsyncKeyState(vk)<0) throw new InvalidOperationException("H87 refuses an already held button/modifier");
    }
    public static void CenterPointer(IntPtr h) {
        Find(h); Point a,b; ClientBounds(h,out a,out b); Point p=new Point(a.X+(b.X-a.X)/2,a.Y+(b.Y-a.Y)/2);
        if(GetForegroundWindow()!=h || GetAncestor(WindowFromPoint(p),2)!=h) throw new InvalidOperationException("H87 receiver center obscured");
        RestoreCursor(p);
    }
    public static void Wheel(IntPtr h,bool horizontal,int delta) {
        RequirePointer(h);
        if(delta==0 || delta<short.MinValue || delta>short.MaxValue) throw new ArgumentOutOfRangeException("delta");
        INPUT input=new INPUT(); input.mouse.flags=horizontal?0x1000u:0x800u;
        input.mouse.data=unchecked((uint)delta); input.mouse.extra=UIntPtr.Zero;
        if(SendInput(1,new INPUT[] {input},Marshal.SizeOf(typeof(INPUT)))!=1) throw new Win32Exception(Marshal.GetLastWin32Error(),"H87 wheel SendInput rejected");
    }
    public static void Clear(IntPtr h) {
        Receiver receiver=Find(h);
        OnUI(receiver.Form,delegate { lock(receiver.Events) { receiver.Events.Clear(); receiver.Dropped=0; } });
    }
    public static int Count(IntPtr h) { var receiver=Find(h); lock(receiver.Events) return receiver.Events.Count+receiver.Dropped; }
    public static int Dropped(IntPtr h) { var receiver=Find(h); lock(receiver.Events) return receiver.Dropped; }
    public static H87WheelEvent[] Events(IntPtr h) { var receiver=Find(h); lock(receiver.Events) return receiver.Events.ToArray(); }
    public static bool WindowExists(IntPtr h) { return IsWindow(h); }
    public static void ForgetClosed() {
        lock(Sync) {
            var gone=new List<IntPtr>(); foreach(var entry in Receivers) if(!IsWindow(entry.Key)) gone.Add(entry.Key);
            foreach(var key in gone) Receivers.Remove(key);
        }
    }
}
'@ -ErrorAction Stop
        }
        $h87State.native_ready = $true
        $h87OldDpi = [H87AxisProbe]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if ($h87OldDpi -eq [IntPtr]::Zero) { throw 'H87 could not establish physical cursor observation context' }
        $h87SavedCursor = [H87AxisProbe]::Cursor()
        $env:WIRI_PIPE = $h87Pipe; $env:WIRI_CONFIG = ''; $env:NIRI_CONFIG = ''
        [IO.File]::WriteAllText($cfg, (H87Config ''), $h87Utf8)
        $oldRoaming = $env:APPDATA; $oldLocal = $env:LOCALAPPDATA
        $oldDryRun = $env:WIRI_POINTER_SETTINGS_DRY_RUN
        try {
            $env:APPDATA = $testRoaming; $env:LOCALAPPDATA = $testLocal
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = Join-Path $h87Dir 'pointer-settings-dry-run.ndjson'
            $h87State.daemon = Start-Process -FilePath $wiri -ArgumentList @('-v', '--no-startup-overlay', '-c', (Q $cfg)) `
                -WindowStyle Hidden -PassThru -RedirectStandardError (Join-Path $h87Dir 'daemon.stderr.log') `
                -RedirectStandardOutput (Join-Path $h87Dir 'daemon.stdout.log')
            $null = $h87State.daemon.Handle
        } finally {
            $env:APPDATA = $oldRoaming; $env:LOCALAPPDATA = $oldLocal
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = $oldDryRun
        }
        $h87Evidence['daemon_pid'] = $h87State.daemon.Id
        $h87Evidence['pipe'] = $h87Pipe
        $readyClock = [Diagnostics.Stopwatch]::StartNew()
        while ($null -eq $h87State.rpc -and $readyClock.ElapsedMilliseconds -lt 20000) {
            if ($h87State.daemon.HasExited) { throw 'H87 daemon exited during startup' }
            try { $h87State.rpc = H87Connect } catch { Start-Sleep -Milliseconds 100 }
        }
        if ($null -eq $h87State.rpc) { throw 'H87 owned pipe startup timed out' }
        $null = H87Request 'ready-state' @{ type = 'get_state' }
        $h87Form = [OwnedLiveForms]::Open($h87Title, 54, 94, 128)
        [void]$h87Forms.Add($h87Form)
        [H87AxisProbe]::Attach($h87Form)
        Start-Sleep -Milliseconds 1100
        $h87Evidence['receiver_hwnd'] = [int64]$h87Form
        $h87Evidence['receiver_pid'] = $PID
        $h87Evidence['injection'] = 'SendInput WHEEL/HWHEEL, dwExtraInfo=0; no key/button downs and no direct SendMessage/PostMessage'
        $pair = @(@{ axis = 'V'; delta = 120 }, @{ axis = 'H'; delta = 120 })

        $baseline = H87Case 'h87_unconfigured_both_axes_positive_control' 'unconfigured' '' $pair @('V:120', 'H:120')
        if (-not $summary.h87_unconfigured_both_axes_positive_control) {
            throw 'H87 unconfigured delivery precondition failed; no scaling claims are valid'
        }
        $null = H87Case 'h87_scalar_positive_both_axes' 'scalar-two' 'scroll-factor 2;' $pair @('V:240', 'H:240')
        $null = H87Case 'h87_property_only_axis_overrides' 'axis-only' 'scroll-factor horizontal=2 vertical=0.5;' $pair @('V:60', 'H:240')
        $null = H87Case 'h87_negative_axis_reverses_application_delta' 'negative-axis' 'scroll-factor horizontal=-0.5 vertical=1;' $pair @('V:120', 'H:-60')
        $null = H87Case 'h87_zero_axis_suppresses_only_that_axis' 'zero-axis' 'scroll-factor horizontal=0 vertical=2;' $pair @('V:240')
        $null = H87Case 'h87_mixed_base_axis_override_replaces_base' 'mixed-base' 'scroll-factor 3 horizontal=0.25;' $pair @('V:360', 'H:30')
        $low = @(
            @{ axis = 'H'; delta = 1 }, @{ axis = 'V'; delta = 1 },
            @{ axis = 'H'; delta = 1 }, @{ axis = 'V'; delta = 1 },
            @{ axis = 'H'; delta = 1 }, @{ axis = 'H'; delta = 1 }
        )
        $null = H87Case 'h87_interleaved_low_delta_residual_is_axis_local' 'interleaved-low' `
            'scroll-factor horizontal=0.25 vertical=0.5;' $low @('V:1', 'H:1')
        $null = H87Case 'h87_window_rule_multiplies_axis_factors' 'window-rule' `
            'scroll-factor horizontal=0.5 vertical=1.5;' $pair @('V:360', 'H:120') 'scroll-factor 2;'
        $null = H87Case 'h87_natural_scroll_reverses_application_axes' 'natural' `
            'natural-scroll true; scroll-factor 0.5;' $pair @('V:-60', 'H:-60')

        $h87Evidence['stage'] = 'bound-wheel-before-zero'
        try {
            $marker = Join-Path $h87Dir 'bound-wheel.marker.txt'
            [IO.File]::WriteAllText($marker, '', $h87Utf8)
            $bind = 'WheelScrollUp { spawn-sh r#"echo hit>> "' + $marker + '""#; }'
            $config = H87Reload 'bound-wheel-before-zero' (H87Config 'scroll-factor 0;' $bind)
            $bound = H87Sequence 'bound-wheel-before-zero' @(@{ axis = 'V'; delta = 120 })
            $markerClock = [Diagnostics.Stopwatch]::StartNew()
            do {
                $hits = @([IO.File]::ReadAllLines($marker) | Where-Object { $_ -eq 'hit' }).Count
                if ($hits -ge 1) { break }
                Start-Sleep -Milliseconds 100
            } while ($markerClock.ElapsedMilliseconds -lt 4000)
            Start-Sleep -Milliseconds 400
            $hits = @([IO.File]::ReadAllLines($marker) | Where-Object { $_ -eq 'hit' }).Count
            H87Gate 'h87_bound_wheel_dispatches_before_zero_factor' ($hits -eq 1 -and (H87Exact $bound @())) @{
                config = $config; observation = $bound; marker_hits = $hits; expected_hits = 1
            }
        } catch {
            $h87Evidence.gates['h87_bound_wheel_dispatches_before_zero_factor'] = @{
                pass = $false; status = 'error'; error = [string]$_
            }
            H87Save
            if ($null -eq $h87State.rpc) { throw }
        }
        $null = H87Case 'h87_replacement_tag_bypasses_recursion' 'replacement-once' `
            'scroll-factor 1.5;' @(@{ axis = 'V'; delta = 120 }) @('V:180') -requireReplacementTag
        $h87Evidence['stage'] = 'gates-complete'
    } catch {
        $h87Evidence['error'] = ($_ | Out-String).Trim()
        $h87Evidence['status'] = 'error'
    } finally {
        # No keys or buttons were pressed by H87, so cleanup must not release
        # another actor's held input. Only its saved cursor and DPI are restored.
        try {
            if ($h87State.forms_ready) {
                [OwnedLiveForms]::ClosePrefix($h87Prefix)
                $clock = [Diagnostics.Stopwatch]::StartNew()
                do {
                    $allClosed = [OwnedLiveForms]::AllClosedForPrefix($h87Prefix)
                    $open = @($h87Forms | Where-Object { [H87AxisProbe]::WindowExists($_) })
                    if ($allClosed -and $open.Count -eq 0) { break }
                    Start-Sleep -Milliseconds 100
                } while ($clock.ElapsedMilliseconds -lt 5000)
                $h87Evidence['pending_openings'] = [OwnedLiveForms]::PendingForPrefix($h87Prefix)
                if (-not $allClosed -or $open.Count -ne 0) { throw 'H87 owned HWNDs/opening UI threads did not finish' }
            }
            if ($h87State.native_ready) {
                [H87AxisProbe]::ForgetClosed()
                if ([H87AxisProbe]::ReceiverCount -ne 0) { throw 'H87 receiver registrations remain after owned form closure' }
            }
        } catch { $h87Evidence.cleanup_errors += 'owned-forms: ' + [string]$_ }
        if ($null -ne $h87State.rpc) {
            try { $h87State.rpc.Dispose() } catch { $h87Evidence.cleanup_errors += 'pipe: ' + [string]$_ }
        }
        if ($null -ne $h87State.daemon) {
            try {
                if (-not $h87State.daemon.HasExited) {
                    $quit = $null
                    try { $quit = H87Connect; $quit.Send("{`"type`":`"quit`"}`n") }
                    catch { $h87Evidence['quit_error'] = [string]$_ }
                    finally { if ($null -ne $quit) { $quit.Dispose() } }
                    if (-not $h87State.daemon.WaitForExit(4000)) {
                        $h87Evidence.cleanup_errors += 'Retained owned daemon required forced stop'
                        $h87State.daemon.Kill(); $null = $h87State.daemon.WaitForExit(5000)
                    }
                }
                if (-not $h87State.daemon.HasExited) { throw 'H87 owned daemon is still running' }
            } catch { $h87Evidence.cleanup_errors += 'owned-daemon: ' + [string]$_ }
        }
        if ($null -ne $h87SavedCursor) {
            try { [H87AxisProbe]::RestoreCursor($h87SavedCursor) }
            catch { $h87Evidence.cleanup_errors += 'cursor-restore: ' + [string]$_ }
        }
        if ($h87OldDpi -ne [IntPtr]::Zero) {
            try {
                if ([H87AxisProbe]::SetThreadDpiAwarenessContext($h87OldDpi) -eq [IntPtr]::Zero) { throw 'DPI restore failed' }
            } catch { $h87Evidence.cleanup_errors += 'dpi-restore: ' + [string]$_ }
        }
        $env:WIRI_PIPE = $h87SavedPipe; $env:WIRI_CONFIG = $h87SavedConfig; $env:NIRI_CONFIG = $h87SavedNiriConfig
        H87Gate 'h87_owned_cleanup' ($h87Evidence.cleanup_errors.Count -eq 0) @{
            errors = $h87Evidence.cleanup_errors; injected_key_or_button_downs = 0
        }
        $summary['lane_axis_scroll_details'] = $h87Evidence
        H87Save
        if ($h87Evidence.cleanup_errors.Count -gt 0) {
            # Root integration must treat this sentinel as fatal, not merely log
            # it through LanePhaseError and continue starting desktop daemons.
            $summary['lane_axis_scroll_fatal_cleanup'] = @{ stop_further_daemons = $true; errors = $h87Evidence.cleanup_errors }
            throw 'H87 HARD CLEANUP FAILURE: stop further desktop phases; inspect retained owned resources'
        }
    }
}
