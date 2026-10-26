# Lane H / H35-H39: IPC wire, queries, events and automated hook-path picks.
# Dot-sourced by live-test.ps1 after $summary exists. No output-setting request
# is sent. Uses only this lane's retained daemon handle and probe windows.
# SendInput is guarded by owned foreground/target checks. Observational hooks
# run before the daemon's existing hooks; probe message filters verify that
# neither half of captured click/Escape pairs reaches either owned app.
& {
    $hDir = Join-Path $D 'lane-h'
    $null = New-Item -ItemType Directory -Path $hDir -Force
    $hDetails = [ordered]@{}
    $hOwned = $null
    $hClients = New-Object 'System.Collections.Generic.List[System.IDisposable]'
    $hSavedConfig = [IO.File]::ReadAllText($cfg)
    $hSavedPipe = $env:WIRI_PIPE
    $hGates = @(
        'h35_json_lines_persistent', 'h35_return_error_then_valid',
        'h35_unknown_then_valid', 'h35_legacy_wiri_undelimited',
        'h35_niri_external_requests', 'h35_cli_print_request_offline',
        'h35_cli_raw_request_argument', 'h35_cli_raw_request_stdin',
        'h36_outputs_structure', 'h36_focused_output_structure',
        'h36_keyboard_layouts_structure', 'h36_outputs_read_only',
        'h37_wiri_event_envelope', 'h37_niri_event_envelope',
        'h37_focus_timestamp_event', 'h37_window_layout_event',
        'h37_keyboard_layout_event',
        'h38_pick_window_next_click_hwnd', 'h38_pick_window_does_not_activate',
        'h38_pick_window_escape_null', 'h38_pick_window_consumes_click_pair',
        'h38_pick_window_consumes_escape_pair', 'h38_pick_disconnect_releases_slot',
        'h39_pick_color_matches_pixel', 'h39_pick_color_escape_null',
        'h39_pick_color_consumes_click_pair', 'h39_pick_color_consumes_escape_pair',
        'h39_cli_window_list_legacy_shape', 'h39_cli_windows_niri_shape'
    )
    foreach ($hGate in $hGates) {
        $summary[$hGate] = $false
        $hDetails[$hGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    function HGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $hDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
    }
    function HError([string]$phase, $err) {
        $hDetails["$phase-error"] = [string]$err
    }
    function HHas($value, [string]$key) {
        return ($null -ne $value -and $null -ne $value.PSObject.Properties[$key])
    }
    function HFields($value, [string[]]$keys) {
        foreach ($key in $keys) { if (-not (HHas $value $key)) { return $false } }
        return $true
    }
    function HWire([string]$label, [string]$direction, [string]$text) {
        [ordered]@{ label = $label; direction = $direction; text = $text } |
            ConvertTo-Json -Compress | Add-Content (Join-Path $hDir 'wire.ndjson')
    }
    function HConnect {
        $c = New-Object LaneHConnection $env:WIRI_PIPE, $hOwned.Id
        $hClients.Add($c)
        return $c
    }
    function HRead($c, [string]$label, [int]$ms = 3500) {
        $text = $c.ReadLine($ms)
        if ($null -eq $text) { throw "$label timed out after ${ms}ms" }
        HWire $label 'reply' $text
        return ($text | ConvertFrom-Json)
    }
    function HRequest($c, [string]$label, [string]$text) {
        HWire $label 'request' $text
        $c.Send($text + "`n")
        return (HRead $c $label)
    }
    function HWiriOK($reply) {
        return ((HFields $reply @('success', 'schema_version')) -and
            $reply.success -is [bool] -and $reply.success -and $reply.schema_version -eq '1.3')
    }
    function HNiriOK($reply, [string]$variant) {
        return ((HHas $reply 'Ok') -and (HHas $reply.Ok $variant) -and
            @($reply.PSObject.Properties).Count -eq 1 -and @($reply.Ok.PSObject.Properties).Count -eq 1)
    }
    function HCLI([string]$name, [string[]]$ctlArgs, $stdinText = $null) {
        $r = [LaneHNative]::RunCli($ctl, $ctlArgs, $stdinText, 5000)
        [IO.File]::WriteAllText((Join-Path $hDir "$name.stdout.txt"), $r.Out)
        [IO.File]::WriteAllText((Join-Path $hDir "$name.stderr.txt"), $r.Err)
        $steps.Add([pscustomobject]@{ step = "lane-h-$name"; args = ($ctlArgs -join ' '); rc = $r.Rc; ms = $r.Ms })
        return $r
    }
    function HEvents($c, [string]$label, [int]$ms = 1000) {
        $events = New-Object 'System.Collections.Generic.List[object]'
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt $ms -and $events.Count -lt 200) {
            $text = $c.ReadLine([Math]::Min(100, [Math]::Max(1, $ms - [int]$clock.ElapsedMilliseconds)))
            if ($null -ne $text) {
                HWire $label 'event' $text
                $events.Add(($text | ConvertFrom-Json))
            }
        }
        return ,($events.ToArray())
    }
    function HLayout($layout) {
        if (-not (HFields $layout @('pos_in_scrolling_layout', 'tile_size', 'window_size',
                    'tile_pos_in_workspace_view', 'window_offset_in_tile'))) { return $false }
        foreach ($key in @('tile_size', 'window_size', 'window_offset_in_tile')) {
            if ($layout.$key -isnot [Array] -or $layout.$key.Count -ne 2) { return $false }
            foreach ($v in $layout.$key) { if ($v -isnot [ValueType] -or [double]::IsNaN([double]$v) -or [double]::IsInfinity([double]$v)) { return $false } }
        }
        $pos = $layout.pos_in_scrolling_layout
        if ($null -ne $pos -and ($pos -isnot [Array] -or $pos.Count -ne 2 -or $pos[0] -lt 1 -or $pos[1] -lt 1)) { return $false }
        $tilePos = $layout.tile_pos_in_workspace_view
        return ($null -eq $tilePos -or ($tilePos -is [Array] -and $tilePos.Count -eq 2))
    }
    function HTimestamp($stamp) {
        return ((HFields $stamp @('secs', 'nanos')) -and $stamp.secs -ge 0 -and $stamp.nanos -ge 0 -and $stamp.nanos -lt 1000000000)
    }
    function HWindow($window) {
        return ((HFields $window @('id', 'title', 'app_id', 'pid', 'workspace_id', 'is_focused',
            'is_floating', 'is_urgent', 'layout', 'focus_timestamp', 'class_name', 'output',
            'is_fullscreen', 'is_maximized', 'is_minimized')) -and $window.id -gt 0 -and
            $window.is_focused -is [bool] -and $window.is_floating -is [bool] -and
            (HLayout $window.layout) -and ($null -eq $window.focus_timestamp -or (HTimestamp $window.focus_timestamp)))
    }
    function HKeyboard($layouts) {
        if (-not (HFields $layouts @('names', 'current_idx')) -or $layouts.names -isnot [Array]) { return $false }
        foreach ($name in $layouts.names) { if ($name -isnot [string] -or [string]::IsNullOrWhiteSpace($name)) { return $false } }
        return ($layouts.current_idx -ge 0 -and
            (($layouts.names.Count -eq 0 -and $layouts.current_idx -eq 0) -or $layouts.current_idx -lt $layouts.names.Count))
    }
    function HOutput($output) {
        if (-not (HFields $output @('name', 'make', 'model', 'serial', 'physical_size', 'modes',
                    'current_mode', 'vrr_supported', 'vrr_enabled', 'logical'))) { return $false }
        if ($output.name -isnot [string] -or $output.modes -isnot [Array]) { return $false }
        # Windows currently has no reliable per-output VRR query. Unknown is
        # null, not a fabricated false; tolerate bool if a backend learns it.
        foreach ($key in @('vrr_supported', 'vrr_enabled')) {
            if ($null -ne $output.$key -and $output.$key -isnot [bool]) { return $false }
        }
        foreach ($mode in $output.modes) {
            if (-not (HFields $mode @('width', 'height', 'refresh_rate', 'is_preferred')) -or
                $mode.width -le 0 -or $mode.height -le 0 -or $mode.refresh_rate -le 0 -or $mode.is_preferred -isnot [bool]) { return $false }
        }
        if ($null -ne $output.current_mode -and ($output.current_mode -lt 0 -or $output.current_mode -ge $output.modes.Count)) { return $false }
        if ($null -ne $output.logical -and (-not (HFields $output.logical @('x', 'y', 'width', 'height', 'scale', 'transform')) -or
            $output.logical.width -le 0 -or $output.logical.height -le 0 -or $output.logical.scale -le 0)) { return $false }
        return $true
    }
    function HFocus([IntPtr]$hwnd, [string]$label) {
        $req = '{"type":"focus_window","data":{"window_hwnd":' + $hwnd.ToInt64() + '}}'
        $r = HRequest $hRpc $label $req
        if (-not (HWiriOK $r)) { throw "$label failed: $($r | ConvertTo-Json -Compress)" }
        Start-Sleep -Milliseconds 1400
        if ([LT]::GetForegroundWindow() -ne $hwnd) { throw "$label did not focus the owned probe" }
    }
    function HPick([string]$request, [string]$variant, [int]$vk) {
        $c = $null; $cursor = $null
        try {
            HFocus ([LaneHNative]::A) "pick-$variant-focus"
            if ($vk -eq 1) { $cursor = [LaneHNative]::Cursor(); [LaneHNative]::MoveToClientB() }
            $c = HConnect
            $before = [LaneHNative]::BActivations
            [LaneHNative]::Arm()
            HWire $variant 'request' $request
            $c.Send($request + "`n")
            Start-Sleep -Milliseconds 500 # bounded server registration allowance
            try {
                [LaneHNative]::Input($vk, $false)
                Start-Sleep -Milliseconds 80
            } finally { [LaneHNative]::Input($vk, $true) }
            $reply = HRead $c $variant
            Start-Sleep -Milliseconds 350 # drain the matching release/any leaked app messages
            return [pscustomobject]@{ reply = $reply; input = [LaneHNative]::Observation();
                delivered = [LaneHNative]::Delivered(); before = $before;
                after = [LaneHNative]::BActivations; foreground = [LT]::GetForegroundWindow().ToInt64() }
        } finally {
            [LaneHNative]::Disarm()
            try { [LaneHNative]::Release() }
            finally {
                if ($null -ne $c) { $c.Dispose() }
                if ($null -ne $cursor) { [LaneHNative]::Move($cursor.X, $cursor.Y) }
            }
        }
    }
    function HConsumed($pick, [int]$vk) {
        if ($null -eq $pick.input -or -not $pick.input.Released -or $null -eq $pick.delivered) { return $false }
        if ($vk -eq 1) {
            return ($pick.input.Kind -ceq 'injected-left' -and $pick.input.Root -eq [LaneHNative]::B.ToInt64() -and
                $pick.input.Foreground -eq [LaneHNative]::A.ToInt64() -and
                $pick.delivered.LeftDown -eq 0 -and $pick.delivered.LeftUp -eq 0)
        }
        return ($pick.input.Kind -ceq 'injected-escape' -and $pick.input.Foreground -eq [LaneHNative]::A.ToInt64() -and
            $pick.delivered.EscapeDown -eq 0 -and $pick.delivered.EscapeUp -eq 0)
    }

    try {
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
using System.Threading.Tasks;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;
public sealed class LaneHConnection : IDisposable {
    readonly NamedPipeClientStream pipe;
    readonly StreamReader reader;
    Task<string> pending;
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle h, out uint pid);
    public LaneHConnection(string path, int ownedPid) {
        const string prefix = @"\\.\pipe\";
        if (path == null || !path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) throw new ArgumentException("Expected local Win32 named pipe path");
        pipe = new NamedPipeClientStream(".", path.Substring(prefix.Length), PipeDirection.InOut, PipeOptions.Asynchronous);
        try {
            pipe.Connect(1200);
            uint pid;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out pid)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (pid != (uint)ownedPid) throw new InvalidOperationException("Refused unowned pipe server PID " + pid);
            reader = new StreamReader(pipe, new UTF8Encoding(false, true), false, 4096, true);
        } catch { pipe.Dispose(); throw; }
    }
    public void Send(string text) {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        if (bytes.Length > 65536) throw new ArgumentException("Lane H does not send oversized/load traffic");
        Task write = pipe.WriteAsync(bytes, 0, bytes.Length);
        if (!write.Wait(1500)) { Dispose(); throw new TimeoutException("Pipe write timeout"); }
        write.GetAwaiter().GetResult();
    }
    // Keep one outstanding read after a timeout; never race another read on
    // persistent streams or the pending-pick disconnect checks.
    public string ReadLine(int ms) {
        if (pending == null) pending = reader.ReadLineAsync();
        if (!pending.Wait(ms)) return null;
        string line = pending.GetAwaiter().GetResult(); pending = null;
        if (line == null) throw new EndOfStreamException("Owned pipe closed");
        return line;
    }
    public void Dispose() { pipe.Dispose(); if (reader != null) reader.Dispose(); }
}
public sealed class LaneHCliResult { public string Out, Err, Rc; public long Ms; }
public sealed class LaneHObservation { public string Kind; public int X, Y; public long Root, Foreground; public int[] Rgb; public bool Released; }
public sealed class LaneHDelivered { public int LeftDown, LeftUp, EscapeDown, EscapeUp; }
public static class LaneHNative {
    public static IntPtr A, B;
    public static int BActivations { get { return Interlocked.CompareExchange(ref bActivations, 0, 0); } }
    static int bActivations;
    static Form a, b;
    static Thread ui;
    static IntPtr mouseHook, keyHook;
    static readonly object sync = new object();
    static bool armed;
    static LaneHObservation observation;
    static LaneHDelivered delivered = new LaneHDelivered();
    // This UI thread owns both probe forms and all their children. Observe
    // queued messages before WinForms preprocessing can suppress a lone up.
    sealed class ProbeInputFilter : IMessageFilter {
        public bool PreFilterMessage(ref Message m) {
            lock(sync) {
                if (armed) {
                    if (m.Msg == 0x201 || m.Msg == 0x203) ++delivered.LeftDown;
                    else if (m.Msg == 0x202) ++delivered.LeftUp;
                    else if (m.WParam.ToInt64() == 27) {
                        if (m.Msg == 0x100 || m.Msg == 0x104) ++delivered.EscapeDown;
                        else if (m.Msg == 0x101 || m.Msg == 0x105) ++delivered.EscapeUp;
                    }
                }
            }
            return false;
        }
    }
    static readonly HashSet<int> held = new HashSet<int>();
    delegate IntPtr Hook(int code, IntPtr w, IntPtr l);
    static readonly Hook mouseProc = OnMouse, keyProc = OnKey;
    [StructLayout(LayoutKind.Sequential)] struct MS { public Point pt; public uint data, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct KS { public uint vk, scan, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk, scan; public uint flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x, y; public uint data, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct U { [FieldOffset(0)] public KI k; [FieldOffset(0)] public MI m; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public U u; }
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct DM {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string device;
        public ushort spec, driver, size, extra; public uint fields;
        public int x, y; public uint orientation, fixedOutput;
        public short color, duplex, yRes, tt, collate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string form;
        public ushort logPixels; public uint bits, width, height, flags, frequency, icmMethod, icmIntent, media, dither, reserved1, reserved2, panWidth, panHeight;
    }
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SetWindowsHookEx(int id, Hook proc, IntPtr module, uint thread);
    [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr h);
    [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr h, int code, IntPtr w, IntPtr l);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode)] static extern IntPtr GetModuleHandle(string name);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(Point pt);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("gdi32.dll")] static extern uint GetPixel(IntPtr dc, int x, int y);
    [DllImport("user32.dll")] static extern short GetAsyncKeyState(int vk);
    [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint n, INPUT[] i, int size);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out Point p);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern IntPtr GetKeyboardLayout(uint tid);
    [DllImport("user32.dll")] static extern int GetKeyboardLayoutList(int n, [Out] IntPtr[] list);
    [DllImport("user32.dll")] static extern IntPtr ActivateKeyboardLayout(IntPtr hkl, uint flags);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool EnumDisplaySettings(string name, int index, ref DM mode);
    static bool Owned(IntPtr h) { return h != IntPtr.Zero && (h == A || h == B); }
    public static void Start() {
        var ready = new ManualResetEvent(false); Exception error = null;
        ui = new Thread(delegate() {
            try {
                a = new Form(); b = new Form();
                a.Text = "wiri-lane-h-focus-anchor"; b.Text = "wiri-lane-h-pick-target (blue client)";
                foreach (Form f in new Form[] { a, b }) {
                    f.Width = 500; f.Height = 360; f.BackColor = Color.FromArgb(48,136,216);
                    f.StartPosition = FormStartPosition.WindowsDefaultLocation;
                    var panel = new Panel(); panel.Dock = DockStyle.Fill; panel.BackColor = f.BackColor; f.Controls.Add(panel);
                }
                b.Activated += delegate { Interlocked.Increment(ref bActivations); };
                Application.AddMessageFilter(new ProbeInputFilter());
                a.Show(); b.Show(); A = a.Handle; B = b.Handle;
                // Installed after the daemon: newest low-level hooks run first,
                // so we see both halves even when wiri consumes the pair.
                mouseHook = SetWindowsHookEx(14, mouseProc, GetModuleHandle(null), 0);
                keyHook = SetWindowsHookEx(13, keyProc, GetModuleHandle(null), 0);
                if (mouseHook == IntPtr.Zero || keyHook == IntPtr.Zero) throw new Win32Exception(Marshal.GetLastWin32Error());
                ready.Set(); Application.Run();
            } catch(Exception e) { error = e; ready.Set(); }
            finally {
                if (mouseHook != IntPtr.Zero) UnhookWindowsHookEx(mouseHook);
                if (keyHook != IntPtr.Zero) UnhookWindowsHookEx(keyHook);
                if (a != null) a.Dispose(); if (b != null) b.Dispose(); A = B = IntPtr.Zero;
            }
        });
        ui.IsBackground = true; ui.SetApartmentState(ApartmentState.STA); ui.Start();
        if (!ready.WaitOne(8000)) throw new TimeoutException("Probe startup");
        ready.Dispose(); if (error != null) throw new InvalidOperationException("Probe startup", error);
    }
    public static void Close() {
        if (a != null && !a.IsDisposed) a.BeginInvoke((MethodInvoker)delegate { Application.ExitThread(); });
        if (ui != null && !ui.Join(4000)) throw new TimeoutException("Owned probe thread did not exit");
    }
    public static void Arm() {
        if (GetAsyncKeyState(1) < 0 || GetAsyncKeyState(27) < 0) throw new InvalidOperationException("Release left/Escape before arming");
        lock(sync) { observation = null; delivered = new LaneHDelivered(); armed = true; }
    }
    public static void Disarm() { lock(sync) { armed = false; } }
    public static LaneHObservation Observation() { lock(sync) { return observation; } }
    public static LaneHDelivered Delivered() {
        lock(sync) { return new LaneHDelivered { LeftDown = delivered.LeftDown, LeftUp = delivered.LeftUp, EscapeDown = delivered.EscapeDown, EscapeUp = delivered.EscapeUp }; }
    }
    static IntPtr OnMouse(int code, IntPtr w, IntPtr l) {
        try {
            if (code >= 0) {
                MS m = (MS)Marshal.PtrToStructure(l, typeof(MS));
                lock(sync) {
                    string kind = (m.flags & 3) != 0 ? "injected-left" : "physical-left";
                    if (armed && observation == null && w.ToInt64() == 0x201) {
                        var o = new LaneHObservation(); o.Kind = kind; o.X = m.pt.X; o.Y = m.pt.Y;
                        o.Root = GetAncestor(WindowFromPoint(m.pt), 2).ToInt64(); o.Foreground = GetForegroundWindow().ToInt64();
                        IntPtr dc = GetDC(IntPtr.Zero);
                        try { uint p = GetPixel(dc, m.pt.X, m.pt.Y); if (dc != IntPtr.Zero && p != 0xffffffff) o.Rgb = new int[] { (int)(p & 255), (int)((p >> 8) & 255), (int)((p >> 16) & 255) }; }
                        finally { if (dc != IntPtr.Zero) ReleaseDC(IntPtr.Zero, dc); }
                        observation = o;
                    } else if (armed && observation != null && observation.Kind == kind && w.ToInt64() == 0x202) observation.Released = true;
                }
            }
        } catch { } // An observational hook must never block the user's input.
        return CallNextHookEx(IntPtr.Zero, code, w, l);
    }
    static IntPtr OnKey(int code, IntPtr w, IntPtr l) {
        try {
            if (code >= 0) {
                KS k = (KS)Marshal.PtrToStructure(l, typeof(KS));
                if (k.vk == 27) lock(sync) {
                    string kind = (k.flags & 0x12) != 0 ? "injected-escape" : "physical-escape";
                    if (armed && observation == null && (w.ToInt64() == 0x100 || w.ToInt64() == 0x104)) observation = new LaneHObservation { Kind = kind, Foreground = GetForegroundWindow().ToInt64() };
                    else if (armed && observation != null && observation.Kind == kind && (w.ToInt64() == 0x101 || w.ToInt64() == 0x105)) observation.Released = true;
                }
            }
        } catch { }
        return CallNextHookEx(IntPtr.Zero, code, w, l);
    }
    public static Point Cursor() { Point p; if (!GetCursorPos(out p)) throw new Win32Exception(); return p; }
    public static void Move(int x, int y) { if (!SetCursorPos(x,y)) throw new Win32Exception(); }
    public static void MoveToClientB() {
        Point p = (Point)b.Invoke((Func<Point>)delegate { return b.PointToScreen(new Point(b.ClientSize.Width/2, b.ClientSize.Height/2)); });
        if (GetAncestor(WindowFromPoint(p),2) != B) throw new InvalidOperationException("Owned blue client is obscured/off-screen");
        Move(p.X,p.Y);
    }
    public static void Input(int vk, bool up) {
        lock(held) {
            if (up && !held.Contains(vk)) return;
            if (!up && (!Owned(GetForegroundWindow()) || GetAsyncKeyState(vk) < 0)) throw new InvalidOperationException("Input refused: foreground/key state");
            if (!up && vk == 1 && GetAncestor(WindowFromPoint(Cursor()), 2) != B) throw new InvalidOperationException("Input refused: pointer is not owned target");
            INPUT i = new INPUT();
            if (vk == 1) i.u.m.flags = up ? 4u : 2u;
            else if (vk == 27) { i.type = 1; i.u.k.vk = 27; i.u.k.flags = up ? 2u : 0u; }
            else throw new ArgumentException("Only left and Escape are allowed");
            if (SendInput(1,new INPUT[] { i },Marshal.SizeOf(typeof(INPUT))) != 1) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (up) held.Remove(vk); else held.Add(vk);
        }
    }
    public static void Release() { Input(1,true); Input(27,true); }
    public static IntPtr[] Layouts() {
        int n = GetKeyboardLayoutList(0,null); if (n < 0 || n > 256) throw new InvalidOperationException("Layout count");
        var layouts = new IntPtr[n]; int copied = GetKeyboardLayoutList(n, layouts);
        Array.Resize(ref layouts, Math.Max(0, Math.Min(n,copied))); return layouts;
    }
    public static IntPtr Layout() { uint pid; return GetKeyboardLayout(GetWindowThreadProcessId(A,out pid)); }
    public static void SetLayout(IntPtr hkl) {
        if (!Owned(GetForegroundWindow())) throw new InvalidOperationException("Layout change refused outside owned foreground");
        a.Invoke((MethodInvoker)delegate { if (ActivateKeyboardLayout(hkl,0) == IntPtr.Zero) throw new Win32Exception(); });
        if (Layout() != hkl) throw new InvalidOperationException("Owned thread did not switch keyboard layout");
    }
    public static string DisplayFingerprint() {
        var rows = new List<string>();
        foreach (Screen screen in Screen.AllScreens) {
            DM m = new DM(); m.size = (ushort)Marshal.SizeOf(typeof(DM));
            if (!EnumDisplaySettings(screen.DeviceName,-1,ref m)) throw new Win32Exception();
            rows.Add(screen.DeviceName + ":" + m.x + "," + m.y + "," + m.width + "," + m.height + "," + m.bits + "," + m.frequency + "," + m.orientation + "," + m.flags);
        }
        rows.Sort(StringComparer.Ordinal); return string.Join(";",rows.ToArray());
    }
    static string Quote(string s) {
        var q = new StringBuilder("\""); int slashes = 0;
        foreach (char c in s) {
            if (c == '\\') { ++slashes; continue; }
            if (c == '"') { q.Append('\\',slashes*2+1); q.Append(c); }
            else { q.Append('\\',slashes); q.Append(c); }
            slashes = 0;
        }
        q.Append('\\',slashes*2); q.Append('"'); return q.ToString();
    }
    public static LaneHCliResult RunCli(string exe, string[] args, string input, int timeout) {
        var words = new List<string>(); foreach (string arg in args) words.Add(Quote(arg));
        var info = new ProcessStartInfo(exe,string.Join(" ",words.ToArray()));
        info.UseShellExecute = false; info.CreateNoWindow = true; info.RedirectStandardOutput = true; info.RedirectStandardError = true; info.RedirectStandardInput = true;
        using (var p = new Process()) {
            p.StartInfo = info; var sw = Stopwatch.StartNew(); p.Start();
            try {
                var handle = p.Handle; // Retain this exact child, including exception paths.
                Task<string> stdout = p.StandardOutput.ReadToEndAsync(), stderr = p.StandardError.ReadToEndAsync();
                if (input != null) p.StandardInput.Write(input); p.StandardInput.Close();
                string rc;
                if (!p.WaitForExit(timeout)) { p.Kill(); if (!p.WaitForExit(2000)) throw new TimeoutException("Owned CLI process did not stop"); rc = "timeout"; }
                else rc = p.ExitCode.ToString();
                if (!Task.WaitAll(new Task[] { stdout, stderr },2000)) throw new TimeoutException("CLI output close timeout");
                return new LaneHCliResult { Out = stdout.Result, Err = stderr.Result, Rc = rc, Ms = sw.ElapsedMilliseconds };
            } finally {
                // Process.Dispose alone would leak a child if stdin/output throws.
                if (!p.HasExited) { p.Kill(); if (!p.WaitForExit(2000)) throw new TimeoutException("Owned CLI cleanup timeout"); }
            }
        }
    }
}
'@
        # Offline print-request must work without any daemon at the chosen pipe.
        # --print-request is tested before starting this lane's daemon.
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'Lane H refuses to run while an existing wiri instance is present'
        }
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_h_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $hPrint = HCLI 'print-request-offline' @('--print-request', 'msg', 'outputs')
        $hPrinted = $hPrint.Out | ConvertFrom-Json
        $hPrintRaw = HCLI 'print-raw-offline' @('raw-request', '"KeyboardLayouts"', '--print-request')
        HGate 'h35_cli_print_request_offline' ($hPrint.Rc -eq '0' -and $hPrinted.type -eq 'outputs' -and
            $hPrintRaw.Rc -eq '0' -and $hPrintRaw.Out.Trim() -ceq '"KeyboardLayouts"') 'No daemon started; unique pipe; direct/msg global flag and raw JSON preserved'

        # Empty/minimal isolated configuration: no output block, no display
        # actions, no pointer-follow or warp, and no test binds are needed.
        [IO.File]::WriteAllText($cfg, "input {`n    mod-key `"ctrl-alt`"`n}`n", (New-Object Text.UTF8Encoding($false)))
        $hOwned = Start-Wiri (Join-Path $hDir 'wiri.stderr.log') (Join-Path $hDir 'wiri.stdout.log')
        $null = $hOwned.Handle # Retain immediately; never rediscover by PID/name.
        Start-Sleep -Seconds 4
        if ($hOwned.HasExited) { throw "Lane H daemon exited (PID $($hOwned.Id))" }
        $hRpc = HConnect

        try {
            # One connection, a pipelined pair, and subsequent recovery requests.
            $hRpc.Send("{`"type`":`"version`"}`n{`"type`":`"windows`"}`n")
            HWire 'pipeline' 'request' '{"type":"version"}\n{"type":"windows"}\n'
            $hVersion = HRead $hRpc 'pipeline-version'
            $hWindows = HRead $hRpc 'pipeline-windows'
            HGate 'h35_json_lines_persistent' ((HWiriOK $hVersion) -and (HWiriOK $hWindows) -and
                $hVersion.result.version -is [string] -and $hWindows.result.windows -is [Array]) @($hVersion, $hWindows)
            $hErrorReply = HRequest $hRpc 'return-error' '{"type":"return_error"}'
            $hAfter = HRequest $hRpc 'after-return-error' '{"type":"version"}'
            HGate 'h35_return_error_then_valid' ($hErrorReply.success -eq $false -and
                $hErrorReply.schema_version -eq '1.3' -and $hErrorReply.error -ceq 'example error requested by ReturnError' -and (HWiriOK $hAfter)) @($hErrorReply, $hAfter)
            $hUnknown = HRequest $hRpc 'unknown' '{"type":"lane_h_unknown_request"}'
            $hAfter = HRequest $hRpc 'after-unknown' '{"type":"windows"}'
            HGate 'h35_unknown_then_valid' ($hUnknown.success -eq $false -and $hUnknown.error -is [string] -and
                $hUnknown.schema_version -eq '1.3' -and (HWiriOK $hAfter)) @($hUnknown, $hAfter)
            $hLegacy = HConnect
            try {
                HWire 'legacy' 'request' '{"type":"version"}'
                $hLegacy.Send('{"type":"version"}') # deliberately NO LF; wiri legacy only
                $hLegacyReply = HRead $hLegacy 'legacy'
                HGate 'h35_legacy_wiri_undelimited' (HWiriOK $hLegacyReply) $hLegacyReply
            } finally { $hLegacy.Dispose() }
            $hNiriVersion = HRequest $hRpc 'niri-string' '"Version"'
            $hNiriWindows = HRequest $hRpc 'niri-object' '{"Windows":null}'
            $hNiriError = HRequest $hRpc 'niri-return-error' '"ReturnError"'
            $hNiriUnknown = HRequest $hRpc 'niri-unknown' '"LaneHUnknownRequest"'
            $hNiriAfter = HRequest $hRpc 'niri-after-errors' '"Version"'
            HGate 'h35_niri_external_requests' ((HNiriOK $hNiriVersion 'Version') -and
                (HNiriOK $hNiriWindows 'Windows') -and $hNiriWindows.Ok.Windows -is [Array] -and
                $hNiriError.Err -ceq 'example error requested by ReturnError' -and
                $hNiriUnknown.Err -is [string] -and (HNiriOK $hNiriAfter 'Version')) @($hNiriVersion, $hNiriWindows, $hNiriError, $hNiriUnknown, $hNiriAfter)
        } catch { HError 'h35-wire' $_ }
        try {
            $hRaw = HCLI 'raw-argument' @('--json', 'raw-request', '{"Version":null}')
            $hRawReply = $hRaw.Out | ConvertFrom-Json
            HGate 'h35_cli_raw_request_argument' ($hRaw.Rc -eq '0' -and (HNiriOK $hRawReply 'Version') -and
                -not (HHas $hRawReply 'success')) $hRawReply
            $hStdin = HCLI 'raw-stdin-dash' @('msg', 'raw-request', '-') '"KeyboardLayouts"'
            $hStdinReply = $hStdin.Out | ConvertFrom-Json
            $hStdinOmitted = HCLI 'raw-stdin-omitted' @('raw-request') '{"type":"version"}'
            $hOmittedReply = $hStdinOmitted.Out | ConvertFrom-Json
            HGate 'h35_cli_raw_request_stdin' ($hStdin.Rc -eq '0' -and (HNiriOK $hStdinReply 'KeyboardLayouts') -and
                $hStdinOmitted.Rc -eq '0' -and (HWiriOK $hOmittedReply)) @($hStdinReply, $hOmittedReply)
        } catch { HError 'h35-cli' $_ }

        try {
            $hDisplayBefore = [LaneHNative]::DisplayFingerprint()
            $hOutputs = HRequest $hRpc 'outputs' '"Outputs"'
            $hFocusedOutput = HRequest $hRpc 'focused-output' '"FocusedOutput"'
            $hKeyboard = HRequest $hRpc 'keyboard-layouts' '"KeyboardLayouts"'
            $hWiriOutputs = HRequest $hRpc 'wiri-outputs' '{"type":"outputs"}'
            $hWiriFocused = HRequest $hRpc 'wiri-focused-output' '{"type":"focused_output"}'
            $hWiriKeyboard = HRequest $hRpc 'wiri-keyboard-layouts' '{"type":"keyboard_layouts"}'
            $hOutputProps = @($hOutputs.Ok.Outputs.PSObject.Properties)
            $hOutputValid = (HNiriOK $hOutputs 'Outputs') -and $hOutputProps.Count -gt 0
            foreach ($hOutputProp in $hOutputProps) {
                $hOutputValid = $hOutputValid -and (HOutput $hOutputProp.Value) -and $hOutputProp.Name -ceq $hOutputProp.Value.name
            }
            HGate 'h36_outputs_structure' ($hOutputValid -and (HWiriOK $hWiriOutputs) -and
                (($hWiriOutputs.result.outputs | ConvertTo-Json -Depth 12 -Compress) -ceq ($hOutputs.Ok.Outputs | ConvertTo-Json -Depth 12 -Compress))) @($hOutputs, $hWiriOutputs)
            $hFO = $hFocusedOutput.Ok.FocusedOutput
            HGate 'h36_focused_output_structure' ((HNiriOK $hFocusedOutput 'FocusedOutput') -and
                ($null -eq $hFO -or ((HOutput $hFO) -and (HHas $hOutputs.Ok.Outputs $hFO.name))) -and
                (HWiriOK $hWiriFocused) -and (HHas $hWiriFocused.result 'focused_output')) @($hFocusedOutput, $hWiriFocused)
            HGate 'h36_keyboard_layouts_structure' ((HNiriOK $hKeyboard 'KeyboardLayouts') -and
                (HKeyboard $hKeyboard.Ok.KeyboardLayouts) -and (HWiriOK $hWiriKeyboard) -and
                (HKeyboard $hWiriKeyboard.result.keyboard_layouts)) @($hKeyboard, $hWiriKeyboard)
            $hDisplayAfter = [LaneHNative]::DisplayFingerprint()
            HGate 'h36_outputs_read_only' ($hDisplayBefore -ceq $hDisplayAfter) @($hDisplayBefore, $hDisplayAfter, 'Only Outputs/FocusedOutput/KeyboardLayouts queries; no Output action sent')
        } catch { HError 'h36' $_ }

        [LaneHNative]::Start()
        Start-Sleep -Milliseconds 1800
        HFocus ([LaneHNative]::A) 'probe-focus'
        $hEventsNiri = HConnect
        $hNiriAck = HRequest $hEventsNiri 'niri-subscribe' '"EventStream"'
        $hEventsWiri = HConnect
        $hWiriAck = HRequest $hEventsWiri 'wiri-subscribe' '{"type":"subscribe_events","data":{"event_types":[]}}'
        $hReplayNiri = HEvents $hEventsNiri 'niri-replay'
        $hReplayWiri = HEvents $hEventsWiri 'wiri-replay'
        HGate 'h37_niri_event_envelope' ($hNiriAck.Ok -ceq 'Handled' -and
            @($hReplayNiri | Where-Object { (HHas $_ 'WindowsChanged') -and (HHas $_.WindowsChanged 'windows') -and -not (HHas $_ 'type') }).Count -eq 1) $hReplayNiri
        HGate 'h37_wiri_event_envelope' ((HWiriOK $hWiriAck) -and
            @($hReplayWiri | Where-Object { $_.type -ceq 'windows_changed' -and (HHas $_.data 'windows') }).Count -eq 1) $hReplayWiri
        try {
            $hBeforeFocus = HRequest $hRpc 'focus-baseline' '"Windows"'
            HFocus ([LaneHNative]::B) 'focus-b'
            HFocus ([LaneHNative]::A) 'focus-a'
            $hAfterFocus = HRequest $hRpc 'focus-final' '"Windows"'
            $hFocusNiri = HEvents $hEventsNiri 'focus-niri'
            $hFocusWiri = HEvents $hEventsWiri 'focus-wiri'
            $hAId = [LaneHNative]::A.ToInt64()
            $hOldA = @($hBeforeFocus.Ok.Windows | Where-Object { $_.id -eq $hAId })
            $hNewA = @($hAfterFocus.Ok.Windows | Where-Object { $_.id -eq $hAId })
            $hTimestampNiri = @($hFocusNiri | Where-Object { (HHas $_ 'WindowFocusTimestampChanged') -and $_.WindowFocusTimestampChanged.id -eq $hAId })
            $hTimestampWiri = @($hFocusWiri | Where-Object { $_.type -ceq 'window_focus_timestamp_changed' -and $_.data.id -eq $hAId })
            $hFocusValid = $hOldA.Count -eq 1 -and $hNewA.Count -eq 1 -and $hTimestampNiri.Count -gt 0 -and $hTimestampWiri.Count -gt 0
            if ($hFocusValid) {
                $hStamp = $hNewA[0].focus_timestamp
                $hOldStamp = $hOldA[0].focus_timestamp
                $hFocusValid = (HTimestamp $hStamp) -and (HTimestamp $hOldStamp) -and
                    ($hStamp.secs -gt $hOldStamp.secs -or ($hStamp.secs -eq $hOldStamp.secs -and $hStamp.nanos -gt $hOldStamp.nanos)) -and
                    (($hTimestampNiri[-1].WindowFocusTimestampChanged.focus_timestamp | ConvertTo-Json -Compress) -ceq ($hStamp | ConvertTo-Json -Compress)) -and
                    (($hTimestampWiri[-1].data.focus_timestamp | ConvertTo-Json -Compress) -ceq ($hStamp | ConvertTo-Json -Compress))
            }
            HGate 'h37_focus_timestamp_event' $hFocusValid @($hBeforeFocus, $hAfterFocus, $hFocusNiri, $hFocusWiri)
        } catch { HError 'h37-focus' $_ }
        try {
            HFocus ([LaneHNative]::A) 'layout-focus'
            $null = HRequest $hRpc 'layout-width-baseline' '{"type":"set_column_width","data":{"preset":"45%"}}'
            Start-Sleep -Milliseconds 1200
            $hLayoutBefore = HRequest $hRpc 'layout-before' '"Windows"'
            $null = HEvents $hEventsNiri 'layout-drain-niri' 300
            $null = HEvents $hEventsWiri 'layout-drain-wiri' 300
            $hResize = HRequest $hRpc 'layout-width-change' '{"type":"set_column_width","data":{"preset":"65%"}}'
            Start-Sleep -Milliseconds 1200
            $hLayoutAfter = HRequest $hRpc 'layout-after' '"Windows"'
            $hLayoutNiri = HEvents $hEventsNiri 'layout-niri'
            $hLayoutWiri = HEvents $hEventsWiri 'layout-wiri'
            $hBeforeA = @($hLayoutBefore.Ok.Windows | Where-Object { $_.id -eq [LaneHNative]::A.ToInt64() })
            $hAfterA = @($hLayoutAfter.Ok.Windows | Where-Object { $_.id -eq [LaneHNative]::A.ToInt64() })
            $hNiriMatch = $false; $hWiriMatch = $false
            foreach ($hEvent in $hLayoutNiri) {
                if (HHas $hEvent 'WindowLayoutsChanged') {
                    foreach ($hChange in $hEvent.WindowLayoutsChanged.changes) {
                        if ($hChange.Count -eq 2 -and $hChange[0] -eq [LaneHNative]::A.ToInt64() -and (HLayout $hChange[1])) { $hNiriMatch = $true }
                    }
                }
            }
            foreach ($hEvent in $hLayoutWiri) {
                if ($hEvent.type -ceq 'window_layouts_changed') {
                    foreach ($hChange in $hEvent.data.changes) {
                        if ($hChange.Count -eq 2 -and $hChange[0] -eq [LaneHNative]::A.ToInt64() -and (HLayout $hChange[1])) { $hWiriMatch = $true }
                    }
                }
            }
            HGate 'h37_window_layout_event' ((HWiriOK $hResize) -and $hBeforeA.Count -eq 1 -and $hAfterA.Count -eq 1 -and
                $hBeforeA[0].layout.tile_size[0] -ne $hAfterA[0].layout.tile_size[0] -and $hNiriMatch -and $hWiriMatch) @($hLayoutBefore, $hLayoutAfter, $hLayoutNiri, $hLayoutWiri)
        } catch { HError 'h37-layout' $_ }
        try {
            HFocus ([LaneHNative]::A) 'keyboard-focus'
            $hLayouts = @([LaneHNative]::Layouts())
            $hOriginalLayout = [LaneHNative]::Layout()
            $hOtherLayout = @($hLayouts | Where-Object { $_ -ne $hOriginalLayout })
            $summary['h37_keyboard_layout_event_skipped'] = ($hOtherLayout.Count -eq 0)
            if ($hOtherLayout.Count -eq 0) {
                # The only host-dependent skip. Null is not a passing gate.
                $summary['h37_keyboard_layout_event'] = $null
                $hDetails['h37_keyboard_layout_event'] = [ordered]@{ pass = $null; status = 'skipped'; reason = 'Host has no second already-loaded keyboard layout; none installed/loaded by this lane' }
            } else {
                try {
                    $hKBefore = HRequest $hRpc 'keyboard-before' '"KeyboardLayouts"'
                    $null = HEvents $hEventsNiri 'keyboard-drain-niri' 300
                    $null = HEvents $hEventsWiri 'keyboard-drain-wiri' 300
                    [LaneHNative]::SetLayout($hOtherLayout[0])
                    Start-Sleep -Milliseconds 1200
                    $hKAfter = HRequest $hRpc 'keyboard-after' '"KeyboardLayouts"'
                    $hKNiri = HEvents $hEventsNiri 'keyboard-niri'
                    $hKWiri = HEvents $hEventsWiri 'keyboard-wiri'
                    $hKN = @($hKNiri | Where-Object { (HHas $_ 'KeyboardLayoutSwitched') -and $_.KeyboardLayoutSwitched.idx -eq $hKAfter.Ok.KeyboardLayouts.current_idx })
                    $hKW = @($hKWiri | Where-Object { $_.type -ceq 'keyboard_layout_switched' -and $_.data.idx -eq $hKAfter.Ok.KeyboardLayouts.current_idx })
                    HGate 'h37_keyboard_layout_event' ((HKeyboard $hKAfter.Ok.KeyboardLayouts) -and
                        $hKBefore.Ok.KeyboardLayouts.current_idx -ne $hKAfter.Ok.KeyboardLayouts.current_idx -and $hKN.Count -gt 0 -and $hKW.Count -gt 0) @($hKBefore, $hKAfter, $hKNiri, $hKWiri)
                } finally {
                    HFocus ([LaneHNative]::A) 'keyboard-restore-focus'
                    [LaneHNative]::SetLayout($hOriginalLayout)
                }
            }
        } catch { HError 'h37-keyboard' $_ }

        # Withhold input past the normal response budget, disconnect, and
        # positively cancel a fresh pick through the real hooks. A busy/error
        # reply cannot masquerade as successful release of the old pick slot.
        $hDisconnectOK = $true
        $hDisconnectEvidence = New-Object 'System.Collections.Generic.List[object]'
        foreach ($hKind in @('window', 'color')) {
            $hPickConn = $null
            try {
                $hVariant = if ($hKind -eq 'window') { 'PickedWindow' } else { 'PickedColor' }
                $hPickRequest = if ($hKind -eq 'window') { '"PickWindow"' } else { '"PickColor"' }
                $hPickConn = HConnect
                HWire "pick-$hKind-disconnect" 'request' $hPickRequest
                $hPickConn.Send($hPickRequest + "`n")
                $hPremature = $hPickConn.ReadLine(1900) # beyond ordinary 1500ms response budget
                HWire "pick-$hKind-disconnect" 'reply-or-timeout' $(if ($null -eq $hPremature) { '<pending after 1900ms>' } else { $hPremature })
                $hAlive = HRequest $hRpc "pick-$hKind-daemon-responsive" '"Version"'
                $hPickConn.Dispose(); $hPickConn = $null
                Start-Sleep -Milliseconds 400
                $hRecovered = HPick $hPickRequest $hVariant 27
                $hPass = $null -eq $hPremature -and (HNiriOK $hAlive 'Version') -and
                    (HNiriOK $hRecovered.reply $hVariant) -and $null -eq $hRecovered.reply.Ok.$hVariant -and (HConsumed $hRecovered 27)
                $hDisconnectOK = $hDisconnectOK -and $hPass
                $hDisconnectEvidence.Add([pscustomobject]@{ kind = $hKind; pending = ($null -eq $hPremature);
                    premature = $hPremature; daemon = $hAlive; recovered = $hRecovered; pass = $hPass })
            } catch { $hDisconnectOK = $false; HError "pick-$hKind-disconnect" $_ }
            finally {
                if ($null -ne $hPickConn) { $hPickConn.Dispose() }
                Start-Sleep -Milliseconds 400
            }
        }
        HGate 'h38_pick_disconnect_releases_slot' $hDisconnectOK ($hDisconnectEvidence.ToArray())

        try {
            $hPickWindow = HPick '"PickWindow"' 'PickedWindow' 1
            $hPW = $hPickWindow.reply.Ok.PickedWindow
            $hWindowPair = HConsumed $hPickWindow 1
            HGate 'h38_pick_window_consumes_click_pair' $hWindowPair $hPickWindow
            HGate 'h38_pick_window_next_click_hwnd' ($hWindowPair -and (HNiriOK $hPickWindow.reply 'PickedWindow') -and
                (HWindow $hPW) -and $hPW.id -eq [LaneHNative]::B.ToInt64()) $hPickWindow
            HGate 'h38_pick_window_does_not_activate' ($hWindowPair -and
                $hPickWindow.foreground -eq [LaneHNative]::A.ToInt64() -and $hPickWindow.before -eq $hPickWindow.after) $hPickWindow
        } catch { HError 'pick-window-click' $_ }
        try {
            $hCancelWindow = HPick '"PickWindow"' 'PickedWindow' 27
            $hWindowEscapePair = HConsumed $hCancelWindow 27
            HGate 'h38_pick_window_consumes_escape_pair' $hWindowEscapePair $hCancelWindow
            HGate 'h38_pick_window_escape_null' ($hWindowEscapePair -and (HNiriOK $hCancelWindow.reply 'PickedWindow') -and
                $null -eq $hCancelWindow.reply.Ok.PickedWindow) $hCancelWindow
        } catch { HError 'pick-window-cancel' $_ }
        try {
            $hPickColor = HPick '"PickColor"' 'PickedColor' 1
            $hPC = $hPickColor.reply.Ok.PickedColor
            $hColorPair = HConsumed $hPickColor 1
            HGate 'h39_pick_color_consumes_click_pair' $hColorPair $hPickColor
            # Compare normalized channels with the actual SDR GetPixel sample
            # captured at the click, not a guessed color from the IPC reply.
            $hColorValid = $hColorPair -and (HNiriOK $hPickColor.reply 'PickedColor') -and (HHas $hPC 'rgb') -and
                $hPC.rgb -is [Array] -and $hPC.rgb.Count -eq 3 -and $null -ne $hPickColor.input.Rgb
            if ($hColorValid) {
                for ($hChannel = 0; $hChannel -lt 3; $hChannel++) {
                    $hActual = [double]$hPC.rgb[$hChannel]
                    $hExpected = $hPickColor.input.Rgb[$hChannel] / 255.0
                    $hColorValid = $hColorValid -and -not [double]::IsNaN($hActual) -and
                        $hActual -ge 0 -and $hActual -le 1 -and [Math]::Abs($hActual - $hExpected) -le 0.00001
                }
            }
            HGate 'h39_pick_color_matches_pixel' $hColorValid $hPickColor
        } catch { HError 'pick-color-click' $_ }
        try {
            $hCancelColor = HPick '"PickColor"' 'PickedColor' 27
            $hColorEscapePair = HConsumed $hCancelColor 27
            HGate 'h39_pick_color_consumes_escape_pair' $hColorEscapePair $hCancelColor
            HGate 'h39_pick_color_escape_null' ($hColorEscapePair -and (HNiriOK $hCancelColor.reply 'PickedColor') -and
                $null -eq $hCancelColor.reply.Ok.PickedColor) $hCancelColor
        } catch { HError 'pick-color-cancel' $_ }
        try {
            $hOldCli = HCLI 'window-list-legacy' @('--json', 'window-list')
            $hNewCli = HCLI 'windows-niri-shape' @('--json', 'windows')
            $hOldReply = $hOldCli.Out | ConvertFrom-Json
            $hNewReply = $hNewCli.Out | ConvertFrom-Json
            $hOldRows = @($hOldReply.result | Where-Object { $_.hwnd -eq [LaneHNative]::A.ToInt64() -or $_.hwnd -eq [LaneHNative]::B.ToInt64() })
            $hOldValid = $hOldCli.Rc -eq '0' -and (HWiriOK $hOldReply) -and $hOldReply.result -is [Array] -and $hOldRows.Count -eq 2
            foreach ($hRow in $hOldRows) {
                $hOldValid = $hOldValid -and (HFields $hRow @('hwnd', 'title', 'class_name', 'process_id', 'x', 'y', 'width', 'height')) -and
                    -not (HHas $hRow 'id') -and $hRow.process_id -eq $PID -and $hRow.width -gt 0 -and $hRow.height -gt 0
            }
            HGate 'h39_cli_window_list_legacy_shape' $hOldValid $hOldReply
            $hNewRows = @($hNewReply.result.windows | Where-Object { $_.id -eq [LaneHNative]::A.ToInt64() -or $_.id -eq [LaneHNative]::B.ToInt64() })
            $hNewValid = $hNewCli.Rc -eq '0' -and (HWiriOK $hNewReply) -and $hNewReply.result.windows -is [Array] -and $hNewRows.Count -eq 2
            foreach ($hRow in $hNewRows) { $hNewValid = $hNewValid -and (HWindow $hRow) -and -not (HHas $hRow 'hwnd') -and $hRow.pid -eq $PID }
            HGate 'h39_cli_windows_niri_shape' $hNewValid $hNewReply
        } catch { HError 'h39-cli-shapes' $_ }
        Shot 'h39-ipc-probes'
    } catch { HError 'lane-h' $_ }
    finally {
        if ('LaneHNative' -as [type]) {
            try { [LaneHNative]::Disarm(); [LaneHNative]::Release() } catch { HError 'cleanup-input' $_ }
        }
        foreach ($hClient in $hClients) { try { $hClient.Dispose() } catch { HError 'cleanup-pipe' $_ } }
        if ('LaneHNative' -as [type]) { try { [LaneHNative]::Close() } catch { HError 'cleanup-probes' $_ } }
        if ($null -ne $hOwned -and -not $hOwned.HasExited) {
            try {
                # Verify server PID and quit on that exact connected handle.
                # An inherited pipe override is never used for blind shutdown.
                $hQuit = New-Object LaneHConnection $env:WIRI_PIPE, $hOwned.Id
                try { $hQuit.Send("{`"type`":`"quit`"}`n") } finally { $hQuit.Dispose() }
            } catch { HError 'cleanup-quit' $_ }
            if (-not $hOwned.WaitForExit(2500)) {
                try { $hOwned.Kill(); $null = $hOwned.WaitForExit(5000); $hDetails['forced-owned-stop'] = $hOwned.Id }
                catch { HError 'cleanup-owned-stop' $_ }
            }
        }
        try { [IO.File]::WriteAllText($cfg, $hSavedConfig, (New-Object Text.UTF8Encoding($false))) }
        catch { HError 'cleanup-config-restore' $_ }
        finally { $env:WIRI_PIPE = $hSavedPipe }
        $hDetails['assumptions'] = @('IPC schema 1.3; user/session pipe or WIRI_PIPE; every connection verifies retained daemon PID',
            'niri requests are LF-delimited; only legacy wiri is tested without LF',
            'No display/output setting request, layout install, network, load or unrelated process shutdown',
            'Picks use SendInput with owned A foreground/B client guards; observer hooks see injected down/up before wiri; app message filters require zero leaked down/up messages',
            'Bounded registration/input/drain sleeps (500/80/350ms), 3500ms reply budget; disconnect cancels input-free picks pending beyond 1500ms',
            'Interactive unlocked SDR desktop with visible blue probe client; no display settings changed; keyboard-only unsupported-host skip is null')
        $hDetails | ConvertTo-Json -Depth 16 | Set-Content (Join-Path $hDir 'evidence.json')
        $summary['h35_h39_evidence'] = (Join-Path $hDir 'evidence.json')
    }
}
