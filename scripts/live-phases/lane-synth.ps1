# Lane SYNTH (H93): live evidence for the touch, pen, drag-and-drop and touch-overview
# paths, using input Windows itself synthesizes.
#
# * Touch and pen are a real Windows pointer device: CreateSyntheticPointerDevice(PT_TOUCH /
#   PT_PEN) + InjectSyntheticPointerInput.  No digitizer hardware is involved; the claim is
#   only that the promoted `MI_WP_SIGNATURE` mouse events reach wiri's low-level hook and are
#   handled exactly like a physical digitizer's promoted events (niri
#   docs/wiki/Gestures.md#touchscreen, #tablet; niri src/input/mod.rs:4356-4560 touch and
#   :3651-4018 tablet per the layout matrix).
# * Drag-and-drop is a genuine OLE drag: a WinForms drag source calls DoDragDrop on a real
#   SendInput left button press, so the capture wiri's heuristic keys on belongs to OLE, not
#   to a bare held button (niri cfg/gestures.rs:33,67; niri src/layout/monitor.rs:1868-2037;
#   niri docs/wiki/Configuration:-Gestures.md defaults).
#
# Every gate group has its own try/catch; nothing outside this lane's own forms and its own
# wiri process is touched, and no display/pointer/input setting of the machine is changed.
& {
    # Owned native helper: the synthetic pointer device, a WH_MOUSE_LL recorder (what wiri's own
    # hook sees), SendInput, the OLE drag source and a few window queries.  Guarded so a second
    # dot-source in one session does not redefine the type.
    if (-not ([System.Management.Automation.PSTypeName]'LaneSynth').Type) {
        Add-Type -ReferencedAssemblies System.Drawing, System.Windows.Forms -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

public static class LaneSynth {
    public const int WH_MOUSE_LL = 14;
    public const int PT_TOUCH = 2, PT_PEN = 3;
    public const int FLAG_INRANGE = 0x2, FLAG_INCONTACT = 0x4, FLAG_PRIMARY = 0x2000, FLAG_CONFIDENCE = 0x4000;
    public const int FLAG_DOWN = 0x10000, FLAG_UPDATE = 0x20000, FLAG_UP = 0x40000;
    public const int PEN_BARREL = 0x1;
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L; public int T; public int R; public int B; }
    [StructLayout(LayoutKind.Sequential)] struct MSG { public IntPtr hwnd; public uint message; public IntPtr wParam; public IntPtr lParam; public uint time; public int x; public int y; }
    [StructLayout(LayoutKind.Sequential)] struct MSLLHOOKSTRUCT { public POINT pt; public uint mouseData; public uint flags; public uint time; public ulong dwExtraInfo; }
    delegate IntPtr HookProc(int nCode, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr SetWindowsHookExW(int idHook, HookProc lpfn, IntPtr hMod, uint dwThreadId);
    [DllImport("user32.dll")] static extern bool UnhookWindowsHookEx(IntPtr hhk);
    [DllImport("user32.dll")] static extern IntPtr CallNextHookEx(IntPtr hhk, int nCode, IntPtr wParam, IntPtr lParam);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetModuleHandleW(string name);
    [DllImport("user32.dll")] static extern int GetMessageW(out MSG m, IntPtr hWnd, uint min, uint max);
    [DllImport("user32.dll")] static extern bool PostThreadMessageW(uint idThread, uint msg, IntPtr w, IntPtr l);
    [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    public static RECT Visible(IntPtr h) { RECT r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) { GetWindowRect(h, out r); } return r; }
    public static IntPtr Foreground() { return GetForegroundWindow(); }
    public static Point Cursor() { POINT p; if (!GetCursorPos(out p)) { throw new Win32Exception(); } return new Point(p.X, p.Y); }
    public static IntPtr TopWindowAt(int x, int y) {
        POINT p; p.X = x; p.Y = y;
        IntPtr h = WindowFromPoint(p);
        return h == IntPtr.Zero ? h : GetAncestor(h, 2);
    }
    public static IntPtr FindClassWindow(string cls, int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            if (!IsWindowVisible(h)) { return true; }
            var name = new StringBuilder(128); GetClassNameW(h, name, 128);
            if (name.ToString() != cls) { return true; }
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != (uint)pid) { return true; }
            found = h;
            return false;
        }, IntPtr.Zero);
        return found;
    }

    // ---- WH_MOUSE_LL recorder: the promoted events exactly as wiri's own hook receives them.
    static readonly List<string> hookLog = new List<string>();
    static HookProc hookProc;
    static IntPtr hookHandle = IntPtr.Zero;
    static Thread hookThread;
    static uint hookThreadId;
    static bool hookOn;
    static string SigOf(ulong extra) {
        uint bits = (uint)extra;
        if ((bits & 0xFFFFFF00u) != 0xFF515700u) { return "none"; }
        string id = (bits & 0x7F).ToString();
        return (bits & 0x80) != 0 ? "touch:" + id : "pen:" + id;
    }
    static IntPtr OnMouse(int code, IntPtr w, IntPtr l) {
        if (code >= 0 && hookOn) {
            uint msg = (uint)(int)w;
            if (msg >= 0x200 && msg <= 0x20E) {
                MSLLHOOKSTRUCT s = (MSLLHOOKSTRUCT)Marshal.PtrToStructure(l, typeof(MSLLHOOKSTRUCT));
                if (hookLog.Count < 5000) {
                    string line = "msg=0x" + msg.ToString("X3") + " sig=" + SigOf(s.dwExtraInfo) +
                        " x=" + s.pt.X + " y=" + s.pt.Y + " flags=" + s.flags + " extra=0x" + s.dwExtraInfo.ToString("X8");
                    lock (hookLog) { if (hookLog.Count < 5000) { hookLog.Add(line); } }
                }
            }
        }
        return CallNextHookEx(IntPtr.Zero, code, w, l);
    }
    public static string StartHook() {
        var ready = new ManualResetEvent(false);
        hookThread = new Thread(delegate() {
            hookThreadId = GetCurrentThreadId();
            hookProc = new HookProc(OnMouse);
            hookHandle = SetWindowsHookExW(WH_MOUSE_LL, hookProc, GetModuleHandleW(null), 0);
            ready.Set();
            MSG m;
            while (GetMessageW(out m, IntPtr.Zero, 0, 0) > 0) { }
            if (hookHandle != IntPtr.Zero) { UnhookWindowsHookEx(hookHandle); hookHandle = IntPtr.Zero; }
        });
        hookThread.IsBackground = true;
        hookThread.Start();
        ready.WaitOne(4000);
        hookOn = hookHandle != IntPtr.Zero;
        return hookOn ? "hook-ok" : "hook-failed-" + Marshal.GetLastWin32Error();
    }
    public static void StopHook() {
        hookOn = false;
        if (hookThread != null) { PostThreadMessageW(hookThreadId, 0x12, IntPtr.Zero, IntPtr.Zero); hookThread.Join(2000); hookThread = null; }
    }
    public static void ClearHook() { lock (hookLog) { hookLog.Clear(); } }
    public static string[] HookLines() { lock (hookLog) { return hookLog.ToArray(); } }
    public static int HookCount(string needle) {
        int n = 0;
        lock (hookLog) { foreach (var line in hookLog) { if (line.IndexOf(needle, StringComparison.Ordinal) >= 0) { n++; } } }
        return n;
    }
    // The signature token of the first matching line ("touch:8", "pen:18" or "none").
    public static string HookSig(string needle) {
        lock (hookLog) {
            foreach (var line in hookLog) {
                if (line.IndexOf(needle, StringComparison.Ordinal) < 0) { continue; }
                int at = line.IndexOf("sig=", StringComparison.Ordinal);
                if (at < 0) { return ""; }
                at += 4;
                int end = line.IndexOf(' ', at);
                return end < 0 ? line.Substring(at) : line.Substring(at, end - at);
            }
        }
        return "";
    }
    // Bounding box of a fixture's own colour in a screenshot: the overview draws live thumbnails,
    // so the painted tile can be located without trusting a copy of the zoom geometry.
    public static string FindColorRect(string path, int r, int g, int b, int tol) {
        using (Bitmap bmp = new Bitmap(path)) {
            int l = int.MaxValue, t = int.MaxValue, right = -1, bottom = -1, count = 0;
            for (int y = 0; y < bmp.Height; y += 2) {
                for (int x = 0; x < bmp.Width; x += 2) {
                    Color p = bmp.GetPixel(x, y);
                    if (Math.Abs(p.R - r) > tol || Math.Abs(p.G - g) > tol || Math.Abs(p.B - b) > tol) { continue; }
                    count++;
                    if (x < l) { l = x; }
                    if (x > right) { right = x; }
                    if (y < t) { t = y; }
                    if (y > bottom) { bottom = y; }
                }
            }
            if (count < 100) { return "none-count=" + count; }
            return l + "," + t + "," + right + "," + bottom + " count=" + count;
        }
    }
    // For a failed fixture check: which window is really on top there.
    public static string DescribeWindow(IntPtr h) {        if (h == IntPtr.Zero) { return "hwnd=0"; }
        var cls = new StringBuilder(128);
        var title = new StringBuilder(160);
        GetClassNameW(h, cls, 128);
        GetWindowTextW(h, title, 160);
        uint pid; GetWindowThreadProcessId(h, out pid);
        RECT r = Visible(h);
        return "hwnd=" + h.ToInt64() + " pid=" + pid + " class=" + cls + " title=" + title +
            " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B;
    }

    // ---- synthetic pointer device
    [StructLayout(LayoutKind.Sequential)] struct POINTER_INFO {
        public int pointerType; public uint pointerId; public uint frameId; public uint pointerFlags;
        public IntPtr sourceDevice; public IntPtr hwndTarget;
        public POINT ptPixelLocation; public POINT ptHimetricLocation; public POINT ptPixelLocationRaw; public POINT ptHimetricLocationRaw;
        public uint dwTime; public uint historyCount; public int InputData; public uint dwKeyStates;
        public ulong PerformanceCount; public int ButtonChangeType;
    }
    [StructLayout(LayoutKind.Sequential)] struct POINTER_TOUCH_INFO {
        public POINTER_INFO pointerInfo; public uint touchFlags; public uint touchMask;
        public RECT rcContact; public RECT rcContactRaw; public uint orientation; public uint pressure;
    }
    [StructLayout(LayoutKind.Sequential)] struct POINTER_PEN_INFO {
        public POINTER_INFO pointerInfo; public uint penFlags; public uint penMask; public uint pressure; public uint rotation; public int tiltX; public int tiltY;
    }
    [StructLayout(LayoutKind.Explicit, Size = 152)] struct POINTER_TYPE_INFO {
        [FieldOffset(0)] public int type;
        [FieldOffset(8)] public POINTER_TOUCH_INFO touchInfo;
        [FieldOffset(8)] public POINTER_PEN_INFO penInfo;
    }
    [DllImport("user32.dll", SetLastError = true)] static extern IntPtr CreateSyntheticPointerDevice(int type, uint maxCount, int mode);
    [DllImport("user32.dll", SetLastError = true)] static extern bool InjectSyntheticPointerInput(IntPtr device, POINTER_TYPE_INFO[] info, uint count);
    [DllImport("user32.dll")] static extern void DestroySyntheticPointerDevice(IntPtr device);
    static IntPtr touchDevice = IntPtr.Zero, penDevice = IntPtr.Zero;
    public static string PointerLayout() {
        return "POINTER_INFO=" + Marshal.SizeOf(typeof(POINTER_INFO)) +
            " POINTER_TOUCH_INFO=" + Marshal.SizeOf(typeof(POINTER_TOUCH_INFO)) +
            " POINTER_PEN_INFO=" + Marshal.SizeOf(typeof(POINTER_PEN_INFO)) +
            " POINTER_TYPE_INFO=" + Marshal.SizeOf(typeof(POINTER_TYPE_INFO));
    }
    public static string CreateTouchDevice() {
        touchDevice = CreateSyntheticPointerDevice(PT_TOUCH, 10, 3);
        return touchDevice == IntPtr.Zero ? "touch-device-failed-" + Marshal.GetLastWin32Error() : "touch-device-ok";
    }
    public static string CreatePenDevice() {
        penDevice = CreateSyntheticPointerDevice(PT_PEN, 1, 3);
        return penDevice == IntPtr.Zero ? "pen-device-failed-" + Marshal.GetLastWin32Error() : "pen-device-ok";
    }
    public static void DestroyDevices() {
        if (touchDevice != IntPtr.Zero) { DestroySyntheticPointerDevice(touchDevice); touchDevice = IntPtr.Zero; }
        if (penDevice != IntPtr.Zero) { DestroySyntheticPointerDevice(penDevice); penDevice = IntPtr.Zero; }
    }
    static POINTER_TYPE_INFO TouchContact(int id, int x, int y, uint flags) {
        POINTER_TYPE_INFO ti = new POINTER_TYPE_INFO();
        ti.type = PT_TOUCH;
        ti.touchInfo.pointerInfo.pointerType = PT_TOUCH;
        ti.touchInfo.pointerInfo.pointerId = (uint)id;
        ti.touchInfo.pointerInfo.pointerFlags = flags;
        ti.touchInfo.pointerInfo.ptPixelLocation.X = x;
        ti.touchInfo.pointerInfo.ptPixelLocation.Y = y;
        ti.touchInfo.touchMask = 7;
        ti.touchInfo.rcContact.L = x - 4; ti.touchInfo.rcContact.T = y - 4; ti.touchInfo.rcContact.R = x + 4; ti.touchInfo.rcContact.B = y + 4;
        ti.touchInfo.rcContactRaw = ti.touchInfo.rcContact;
        ti.touchInfo.pressure = 32000;
        return ti;
    }
    static POINTER_TYPE_INFO PenContact(int id, int x, int y, uint flags, uint penFlags) {
        POINTER_TYPE_INFO ti = new POINTER_TYPE_INFO();
        ti.type = PT_PEN;
        ti.penInfo.pointerInfo.pointerType = PT_PEN;
        ti.penInfo.pointerInfo.pointerId = (uint)id;
        ti.penInfo.pointerInfo.pointerFlags = flags;
        ti.penInfo.pointerInfo.ptPixelLocation.X = x;
        ti.penInfo.pointerInfo.ptPixelLocation.Y = y;
        ti.penInfo.penFlags = penFlags;
        ti.penInfo.penMask = 1;
        ti.penInfo.pressure = 32000;
        return ti;
    }
    // One frame carries every contact the frame changes: the primary finger, or a second contact
    // reported beside it.
    public static void Touch(int[] ids, int[] xs, int[] ys, int[] flags, int[] primary) {
        if (touchDevice == IntPtr.Zero) { throw new InvalidOperationException("no synthetic touch device"); }
        POINTER_TYPE_INFO[] a = new POINTER_TYPE_INFO[ids.Length];
        for (int i = 0; i < ids.Length; i++) {
            uint f = (uint)flags[i];
            if (primary[i] != 0) { f |= (uint)(FLAG_PRIMARY | FLAG_CONFIDENCE); }
            a[i] = TouchContact(ids[i], xs[i], ys[i], f);
        }
        if (!InjectSyntheticPointerInput(touchDevice, a, (uint)a.Length)) { throw new Win32Exception(Marshal.GetLastWin32Error()); }
    }
    public static void Pen(int x, int y, int flags, int penFlags) {
        if (penDevice == IntPtr.Zero) { throw new InvalidOperationException("no synthetic pen device"); }
        POINTER_TYPE_INFO[] a = new POINTER_TYPE_INFO[1];
        a[0] = PenContact(1, x, y, (uint)flags, (uint)penFlags);
        if (!InjectSyntheticPointerInput(penDevice, a, 1)) { throw new Win32Exception(Marshal.GetLastWin32Error()); }
    }

    // ---- real SendInput mouse and keyboard
    [StructLayout(LayoutKind.Sequential)] struct KI { public ushort vk, scan; public uint flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct MI { public int x, y; public uint data, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct U { [FieldOffset(0)] public KI k; [FieldOffset(0)] public MI m; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public U u; }
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint n, INPUT[] events, int size);
    static void Send(INPUT i) { if (SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT))) != 1) { throw new Win32Exception(); } }
    public static void Move(int x, int y) {
        int l = GetSystemMetrics(76), t = GetSystemMetrics(77), w = GetSystemMetrics(78), h = GetSystemMetrics(79);
        if (w <= 0 || h <= 0 || x < l || y < t || x >= l + w || y >= t + h) { throw new ArgumentException("Pointer outside the desktop"); }
        INPUT i = new INPUT(); i.u.m.flags = 0xC001u;
        i.u.m.x = (int)(((long)(x - l) * 65536 + 32768) / w);
        i.u.m.y = (int)(((long)(y - t) * 65536 + 32768) / h);
        Send(i);
    }
    public static void Key(int vk, bool up) { INPUT i = new INPUT(); i.type = 1; i.u.k.vk = (ushort)vk; i.u.k.flags = up ? 2u : 0u; Send(i); }
    public static void Button(string b, bool up) {
        INPUT i = new INPUT();
        i.u.m.flags = b == "L" ? (up ? 4u : 2u) : b == "R" ? (up ? 16u : 8u) : (up ? 64u : 32u);
        Send(i);
    }
    public static void ReleaseAll() { Button("L", true); Button("R", true); Button("M", true); Key(0x12, true); Key(0x11, true); }

    // ---- the capture wiri's drag heuristic keys on, read the same way it reads it
    [StructLayout(LayoutKind.Sequential)] struct GUITHREADINFO {
        public int cbSize; public int flags; public IntPtr hwndActive; public IntPtr hwndFocus; public IntPtr hwndCapture;
        public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize; public IntPtr hwndCaret; public RECT rcCaret;
    }
    [DllImport("user32.dll", SetLastError = true)] static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
    public static string CaptureInfoAt(int x, int y) {
        POINT p; p.X = x; p.Y = y;
        IntPtr root = GetAncestor(WindowFromPoint(p), 2);
        uint pid; uint tid = GetWindowThreadProcessId(root, out pid);
        GUITHREADINFO g = new GUITHREADINFO();
        g.cbSize = Marshal.SizeOf(typeof(GUITHREADINFO));
        if (!GetGUIThreadInfo(tid, ref g)) { return "gui-thread-info-failed"; }
        uint capPid = 0;
        var cls = new StringBuilder(64);
        if (g.hwndCapture != IntPtr.Zero) { GetWindowThreadProcessId(g.hwndCapture, out capPid); GetClassNameW(g.hwndCapture, cls, 64); }
        return "press-pid=" + pid + " capture-pid=" + capPid + " capture-class=" + cls + " own-pid=" + System.Diagnostics.Process.GetCurrentProcess().Id;
    }

    // ---- own forms only: a plain window and a real OLE drag source
    static readonly List<Form> forms = new List<Form>();
    static readonly object stateLock = new object();
    static string dragState = "idle";
    public static string DragState() { lock (stateLock) { return dragState; } }
    public static void ResetDrag() { lock (stateLock) { dragState = "idle"; } }
    static void SetDrag(string s) { lock (stateLock) { dragState = s; } }
    public static IntPtr OpenPlain(string title, int r, int g, int b) {
        IntPtr handle = IntPtr.Zero;
        var ready = new ManualResetEvent(false);
        var t = new Thread(delegate() {
            var f = new Form();
            f.Text = title; f.Width = 520; f.Height = 380;
            f.BackColor = Color.FromArgb(r, g, b);
            f.StartPosition = FormStartPosition.WindowsDefaultLocation;
            f.Shown += delegate(object s, EventArgs e) { handle = f.Handle; ready.Set(); };
            lock (forms) { forms.Add(f); }
            Application.Run(f);
        });
        t.SetApartmentState(ApartmentState.STA);
        t.IsBackground = true;
        t.Start();
        ready.WaitOne(15000);
        return handle;
    }
    // The first left button down inside it starts a genuine OLE drag on its own UI thread, so
    // the capture belongs to OLE's own window, not to a bare held button.
    public static IntPtr OpenDragSource(string title) {
        IntPtr handle = IntPtr.Zero;
        var ready = new ManualResetEvent(false);
        var t = new Thread(delegate() {
            Application.OleRequired();
            var f = new Form();
            f.Text = title; f.Width = 520; f.Height = 380;
            f.BackColor = Color.FromArgb(210, 120, 40);
            f.StartPosition = FormStartPosition.WindowsDefaultLocation;
            bool started = false;
            f.MouseDown += delegate(object s, MouseEventArgs e) {
                if (e.Button != MouseButtons.Left || started) { return; }
                started = true;
                SetDrag("armed");
                f.BeginInvoke((MethodInvoker)delegate() {
                    SetDrag("entered");
                    try { SetDrag("done-" + f.DoDragDrop((object)"wiri-synth-drag", DragDropEffects.Copy).ToString()); }
                    catch (Exception ex) { SetDrag("error-" + ex.Message); }
                    started = false;
                });
            };
            f.Shown += delegate(object s, EventArgs e) { handle = f.Handle; ready.Set(); };
            lock (forms) { forms.Add(f); }
            Application.Run(f);
        });
        t.SetApartmentState(ApartmentState.STA);
        t.IsBackground = true;
        t.Start();
        ready.WaitOne(15000);
        return handle;
    }
    public static void CloseForm(IntPtr h) {
        lock (forms) {
            foreach (var f in forms) {
                if (f.IsHandleCreated && f.Handle == h) {
                    Form ff = f;
                    try { ff.BeginInvoke((MethodInvoker)delegate() { ff.Close(); }); } catch (Exception) { }
                }
            }
        }
    }
}
'@
    }

    $hsGates = @(
        'h93_promoted_touch_carries_the_mi_wp_signature',
        'h93_promoted_pen_carries_the_mi_wp_signature',
        'h93_dnd_edge_view_scroll_moves_the_strip',
        'h93_dnd_edge_view_scroll_honours_its_knobs',
        'h93_dnd_hold_to_activate_focuses_the_hovered_window',
        'h93_dnd_edge_workspace_switch_steps_once',
        'h93_touch_mod_drag_scrolls_the_view',
        'h93_touch_mod_drag_moves_a_floating_window',
        'h93_pen_mod_drag_moves_a_floating_window',
        'h93_pen_barrel_runs_its_tablet_stylus_bind',
        'h93_touch_overview_pan_steps_workspaces'
    )
    foreach ($hsKey in $hsGates) { $summary[$hsKey] = $false }
    $hsDir = Join-Path $D 'lane-synth'
    $null = New-Item -ItemType Directory -Force $hsDir
    $hsDetails = [ordered]@{}
    $hsForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $hsProc = $null
    $hsCursor = $null
    $hsLog = Join-Path $hsDir 'wiri.stderr.log'
    function HSGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $hsDetails[$name] = [ordered]@{ pass = $pass; observed = $observed }
    }
    function HSObserve([string]$name, $value) { $hsDetails[$name] = $value }
    # A required IPC call: the harness `Ctl` records the step, this fails the gate when the call
    # did not run or did not succeed (a swallowed result would otherwise hide a dead daemon).
    function HSCtl([string]$tag, [string[]]$a) {
        $n = $steps.Count
        $result = Ctl $tag $a
        if ($steps.Count -ne $n + 1 -or [string]$steps[$n].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $result
    }
    function HSAction([string]$tag, [string[]]$a) {
        HSCtl $tag (@('action') + $a) | Out-Null
        Start-Sleep -Milliseconds 700
    }
    function HSList([string]$tag) { return @((HSCtl $tag @('--json', 'window-list') | ConvertFrom-Json).result) }
    function HSWin($list, [IntPtr]$h) { return (@($list | Where-Object { [int64]$_.hwnd -eq [int64]$h }) | Select-Object -First 1) }
    function HSState([string]$tag) { return (HSCtl $tag @('--json', 'state') | ConvertFrom-Json).result }
    function HSCount([string]$needle) { return (LogLines $hsLog $needle).Count }
    # niri's own defaults for both edges (Configuration:-Gestures), so the default-config gate
    # quotes the pinned numbers instead of a fixture value.
    function HSConfig([string]$dnd = 'gestures {}') {
        return @"
input { mod-key "Alt"; touch {}; tablet {} }
$dnd
overview { zoom 0.5; }
layout {
    gaps 12
    center-focused-column "never"
    default-column-width { fixed 300; }
    border { width 1; }
    focus-ring { off; }
}
binds { Alt+TabletStylusButton2 { toggle-window-floating; } }
"@
    }
    function HSWrite([string]$text) { [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false))) }
    function HSReload([string]$text) { HSWrite $text; Start-Sleep -Milliseconds 2500 }
    function HSFocus([IntPtr]$h) {
        FocusForm $h
        if (-not (WaitForeground $h 0 3000)) { throw "Lane SYNTH form not foreground: $h" }
    }
    function HSOnScreen([IntPtr]$h) {
        $r = Rc $h
        $s = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        return ($r.W -gt 40) -and ($r.H -gt 40) -and ($r.L -ge $s.Left - 2) -and ($r.T -ge $s.Top - 2) -and ($r.R -le $s.Right + 2) -and ($r.B -le $s.Bottom + 2)
    }
    # Reveal and focus a window: wiri only re-scrolls the strip for a focus *change*, so a window
    # that is already focused but scrolled out needs one focus round-trip through another window.
    function HSShow([IntPtr]$h) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 6000) {
            HSFocus $h
            Start-Sleep -Milliseconds 350
            if (HSOnScreen $h) { return }
            $hsOther = if ($h -ne $hsSource) { $hsSource } else { $hsA }
            HSFocus $hsOther
            Start-Sleep -Milliseconds 200
        }
        throw ("Window was not revealed on screen: " + [LaneSynth]::DescribeWindow($h))
    }
    # Put a window into a known floating state; `toggle-window-floating` acts on the native
    # foreground, so the focus is re-asserted before every attempt.
    function HSFloat([IntPtr]$h, [bool]$want = $true) {
        for ($hsTry = 0; $hsTry -lt 3; $hsTry++) {
            $hsEntry = HSWin (HSList 'hs-float-check') $h
            if ($null -ne $hsEntry -and ([bool]$hsEntry.floating -eq $want)) { return }
            HSFocus $h
            HSAction 'hs-float-toggle' @('toggle-window-floating')
            Start-Sleep -Milliseconds 500
        }
        throw "Could not set floating=$want on $h"
    }
    function HSOpen([string]$name, [int]$r, [int]$g, [int]$b) {
        $h = [LaneSynth]::OpenPlain($name, $r, $g, $b)
        $hsForms.Add($h)
        Start-Sleep -Milliseconds 1100
        return $h
    }
    function HSCloseAll {
        foreach ($h in @($hsForms.ToArray())) {
            if ([LaneSynth]::IsWindow($h)) { try { [LaneSynth]::CloseForm($h) } catch { } }
        }
        $hsForms.Clear()
        Start-Sleep -Milliseconds 400
    }
    # A point inside the visible part of a window, and a refusal when something else is on top of
    # it: an obscured fixture would make the touch press land on the wrong window.
    function HSPoint([IntPtr]$h, [double]$fx = 0.5, [double]$fy = 0.5) {
        $r = Rc $h
        if (-not (HSOnScreen $h)) { throw ("Target window is off screen: " + [LaneSynth]::DescribeWindow($h)) }
        $x = [int]($r.L + ($r.W * $fx)); $y = [int]($r.T + ($r.H * $fy))
        $top = [LaneSynth]::TopWindowAt($x, $y)
        if ($top -ne $h) { throw ("Owned input target obscured at " + $x + "," + $y + ": " + [LaneSynth]::DescribeWindow($top)) }
        return [System.Drawing.Point]::new($x, $y)
    }
    # The first of this lane's own windows that is tiled, fully on screen and not covered.
    function HSPickTiled([IntPtr[]]$candidates) {
        foreach ($h in $candidates) {
            try {
                if (HSOnScreen $h) {
                    $hsEntry = HSWin (HSList 'hs-pick-list') $h
                    if ($null -ne $hsEntry -and -not [bool]$hsEntry.floating) {
                        $null = HSPoint $h 0.5 0.5
                        return $h
                    }
                }
            } catch { }
        }
        throw 'No owned tiled window is visible and uncovered'
    }
    function HSTouchFrame([int]$x, [int]$y, [int]$flag) {
        [LaneSynth]::Touch(@(1), @($x), @($y), @($flag -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), @(1))
    }
    # A one-finger drag: down, interpolated updates, up. Windows promotes each frame to
    # WM_MOUSEMOVE/WM_LBUTTONDOWN carrying the MI_WP_SIGNATURE touch bit.
    function HSTouchDrag([int]$x, [int]$y, [int]$dx, [int]$dy, [int]$frames = 6, [int]$stepMs = 45) {
        HSTouchFrame $x $y ([LaneSynth]::FLAG_DOWN)
        Start-Sleep -Milliseconds 160
        foreach ($i in 1..$frames) {
            HSTouchFrame ($x + [int]($dx * $i / $frames)) ($y + [int]($dy * $i / $frames)) ([LaneSynth]::FLAG_UPDATE)
            Start-Sleep -Milliseconds $stepMs
        }
        Start-Sleep -Milliseconds 150
        [LaneSynth]::Touch(@(1), @(($x + $dx)), @(($y + $dy)), @([LaneSynth]::FLAG_UP), @(0))
        Start-Sleep -Milliseconds 250
    }
    function HSPenDrag([int]$x, [int]$y, [int]$dx, [int]$dy, [int]$frames = 6, [int]$stepMs = 45) {
        [LaneSynth]::Pen($x, $y, ([LaneSynth]::FLAG_DOWN -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), 0)
        Start-Sleep -Milliseconds 160
        foreach ($i in 1..$frames) {
            [LaneSynth]::Pen(($x + [int]($dx * $i / $frames)), ($y + [int]($dy * $i / $frames)), ([LaneSynth]::FLAG_UPDATE -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), 0)
            Start-Sleep -Milliseconds $stepMs
        }
        Start-Sleep -Milliseconds 150
        [LaneSynth]::Pen(($x + $dx), ($y + $dy), [LaneSynth]::FLAG_UP, 0)
        Start-Sleep -Milliseconds 250
    }
    # A real OLE drag: press the left button inside the drag source (its mouse-down starts
    # DoDragDrop on its own UI thread) and wait until the OLE loop reports that it runs.
    function HSDragBegin([int]$x, [int]$y) {
        [LaneSynth]::ResetDrag()
        [LaneSynth]::Move($x, $y)
        Start-Sleep -Milliseconds 150
        [LaneSynth]::Button('L', $false)
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 3000 -and [LaneSynth]::DragState() -ne 'entered') { Start-Sleep -Milliseconds 25 }
        if ([LaneSynth]::DragState() -ne 'entered') { throw ('OLE drag did not start: ' + [LaneSynth]::DragState()) }
        return [LaneSynth]::CaptureInfoAt($x, $y)
    }
    function HSDragTo([int]$fx, [int]$fy, [int]$tx, [int]$ty, [int]$frames = 8, [int]$stepMs = 40) {
        foreach ($i in 1..$frames) {
            [LaneSynth]::Move(($fx + [int](($tx - $fx) * $i / $frames)), ($fy + [int](($ty - $fy) * $i / $frames)))
            Start-Sleep -Milliseconds $stepMs
        }
    }
    function HSDragEnd { [LaneSynth]::Button('L', $true); Start-Sleep -Milliseconds 500 }

    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane SYNTH refuses an existing daemon' }
        HSWrite (HSConfig)
        $hsProc = Start-Wiri $hsLog ($hsLog + '.out')
        $null = $hsProc.Handle
        Start-Sleep -Seconds 4
        if ($hsProc.HasExited) { throw 'Lane SYNTH daemon exited' }
        $hsCursor = [LaneSynth]::Cursor()
        HSObserve 'daemon_pid' $hsProc.Id
        HSObserve 'hook' ([LaneSynth]::StartHook())
        # A fresh workspace: the shared desktop already holds other applications' windows, and a
        # lane's fixtures must be alone in their strip for placement, reveal and edge assertions.
        HSAction 'hs-workspace' @('focus-workspace', '2')
        # A real drag source (its own mouse-down starts an OLE drag) and four ordinary windows.
        $hsSource = [LaneSynth]::OpenDragSource('hs-drag-source')
        $hsForms.Add($hsSource)
        Start-Sleep -Milliseconds 1100
        $hsA = HSOpen 'hs-a' 190 60 60
        $hsB = HSOpen 'hs-b' 60 190 60
        $hsC = HSOpen 'hs-c' 60 60 190
        $hsD = HSOpen 'hs-d' 160 160 60
        $hsScreen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
        $hsMidX = $hsScreen.Left + [int]($hsScreen.Width / 2)
        $hsMidY = $hsScreen.Top + [int]($hsScreen.Height / 2)
        HSObserve 'synthetic_touch_device' ([LaneSynth]::CreateTouchDevice())
        HSObserve 'synthetic_pen_device' ([LaneSynth]::CreatePenDevice())
        HSObserve 'pointer_layout' ([LaneSynth]::PointerLayout())
        try {
            HSObserve 'fixture_rects_initial' @(
                [LaneSynth]::DescribeWindow($hsSource), [LaneSynth]::DescribeWindow($hsA),
                [LaneSynth]::DescribeWindow($hsB), [LaneSynth]::DescribeWindow($hsC), [LaneSynth]::DescribeWindow($hsD))
            HSObserve 'fixture_on_screen' @((HSOnScreen $hsSource), (HSOnScreen $hsA), (HSOnScreen $hsB), (HSOnScreen $hsC), (HSOnScreen $hsD))
        } catch { HSObserve 'error-fixture-info' ([string]$_) }

        # ---- H93 / promoted touch and pen signatures -----------------------------------------
        # Windows tags promoted touch/pen mouse messages with MI_WP_SIGNATURE (0xFF515700) in
        # dwExtraInfo, and bit 0x80 distinguishes touch from pen; a WH_MOUSE_LL hook sees exactly
        # what wiri's own low-level mouse hook sees.
        try {
            [LaneSynth]::ClearHook()
            $hsPt = HSPoint $hsD 0.5 0.5
            HSTouchDrag $hsPt.X $hsPt.Y 24 0 3 60
            Start-Sleep -Milliseconds 300
            $hsTouchDown = [LaneSynth]::HookCount('msg=0x201 sig=touch:')
            $hsTouchMove = [LaneSynth]::HookCount('msg=0x200 sig=touch:')
            $hsTouchUp = [LaneSynth]::HookCount('msg=0x202 sig=touch:')
            $hsTouchSig = [LaneSynth]::HookSig('msg=0x201 sig=touch:')
            $hsTouchLines = @([LaneSynth]::HookLines() | Select-Object -First 6)
            HSGate 'h93_promoted_touch_carries_the_mi_wp_signature' (($hsTouchDown -ge 1) -and ($hsTouchMove -ge 1) -and ($hsTouchUp -ge 1) -and ($hsTouchSig -like 'touch:*')) @($hsTouchDown, $hsTouchMove, $hsTouchUp, $hsTouchSig, $hsTouchLines)
        } catch {
            HSObserve 'error-promoted-touch' ([string]$_)
        }
        try {
            [LaneSynth]::ClearHook()
            $hsPt = HSPoint $hsD 0.5 0.5
            HSPenDrag $hsPt.X $hsPt.Y -24 0 3 60
            Start-Sleep -Milliseconds 200
            $hsPenDown = [LaneSynth]::HookCount('msg=0x201 sig=pen:')
            $hsPenUp = [LaneSynth]::HookCount('msg=0x202 sig=pen:')
            $hsPenSig = [LaneSynth]::HookSig('msg=0x201 sig=pen:')
            # The barrel button is promoted to the right button, still carrying the pen bit.
            [LaneSynth]::Pen($hsPt.X, $hsPt.Y, ([LaneSynth]::FLAG_DOWN -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), [LaneSynth]::PEN_BARREL)
            Start-Sleep -Milliseconds 250
            [LaneSynth]::Pen($hsPt.X, $hsPt.Y, [LaneSynth]::FLAG_UP, [LaneSynth]::PEN_BARREL)
            Start-Sleep -Milliseconds 250
            $hsBarrel = [LaneSynth]::HookCount('msg=0x204 sig=pen:')
            # Control: ordinary SendInput mouse input carries no signature at all.
            [LaneSynth]::Move(($hsPt.X + 6), $hsPt.Y)
            Start-Sleep -Milliseconds 150
            $hsPlain = [LaneSynth]::HookCount('msg=0x200 sig=none')
            HSGate 'h93_promoted_pen_carries_the_mi_wp_signature' (($hsPenDown -ge 1) -and ($hsPenUp -ge 1) -and ($hsBarrel -ge 1) -and ($hsPenSig -like 'pen:*') -and ($hsPlain -ge 1)) @($hsPenDown, $hsPenUp, $hsBarrel, $hsPenSig, $hsPlain, @([LaneSynth]::HookLines() | Select-Object -First 8))
        } catch {
            HSObserve 'error-promoted-pen' ([string]$_)
        }

        # ---- H93 / drag-and-drop edge view scroll (real OLE drag) ----------------------------
        # niri scrolls the tiling view while a drag hovers inside `trigger-width` of a monitor
        # edge, after `delay-ms`, at up to `max-speed` (Configuration:-Gestures#dnd-edge-view-scroll).
        # The pointer parks in the left zone, where the content moves left: the direction the
        # strip can still travel from a revealed first column.
        try {
            HSShow $hsSource
            HSShow $hsB
            HSShow $hsA
            $hsSrcPt = HSPoint $hsSource 0.5 0.5
            $hsBefore = @($hsA, $hsB, $hsC, $hsD | ForEach-Object { Rc $_ })
            $hsFrames0 = HSCount 'dnd edge frame'
            $hsCapture = HSDragBegin $hsSrcPt.X $hsSrcPt.Y
            HSDragTo $hsSrcPt.X $hsSrcPt.Y ($hsScreen.Left + 2) $hsMidY 8 40
            Start-Sleep -Milliseconds 160
            $hsHeld = @($hsA, $hsB, $hsC, $hsD | ForEach-Object { Rc $_ })
            $hsFramesZone = HSCount 'dnd edge frame'
            # Leaving the zone must stop the one-shot frame timer at once.
            HSDragTo ($hsScreen.Left + 2) $hsMidY $hsMidX $hsMidY 6 40
            Start-Sleep -Milliseconds 250
            $hsOut1 = @($hsA, $hsB | ForEach-Object { Rc $_ })
            $hsFramesOut = HSCount 'dnd edge frame'
            Start-Sleep -Milliseconds 450
            $hsFramesIdle = HSCount 'dnd edge frame'
            $hsOut2 = @($hsA, $hsB | ForEach-Object { Rc $_ })
            $hsFrameLine = (@(LogLines $hsLog 'dnd edge frame') | Select-Object -Last 1).Line
            HSDragEnd
            # Every fixture whose rectangle kept its size and top edge moved by the same amount:
            # the whole strip travelled together. A parked (jumped below the screen) window cannot
            # satisfy this, and the direction is niri's business, not the gate's.
            $hsDeltas = @()
            for ($i = 0; $i -lt 4; $i++) {
                $b = $hsBefore[$i]; $h = $hsHeld[$i]
                if ((Near $h.T $b.T 4) -and (Near $h.H $b.H 4)) { $hsDeltas += [int]($h.L - $b.L) }
            }
            $hsMoved = @($hsDeltas | Where-Object { [Math]::Abs($_) -ge 40 }).Count
            $hsSameDelta = ($hsMoved -ge 2) -and ((@($hsDeltas | Where-Object { [Math]::Abs($_) -ge 40 }) | Sort-Object -Unique).Count -le 2)
            $hsFrozen = (Near $hsOut1[0].L $hsOut2[0].L 2) -and (Near $hsOut1[1].L $hsOut2[1].L 2)
            HSGate 'h93_dnd_edge_view_scroll_moves_the_strip' (($hsFramesZone -gt $hsFrames0) -and $hsSameDelta -and ($hsFramesOut -eq $hsFramesIdle) -and $hsFrozen -and ($hsFrameLine -match 'dir=(left|right)')) @($hsBefore, $hsHeld, $hsDeltas, $hsFrames0, $hsFramesZone, $hsFramesOut, $hsFramesIdle, $hsOut1, $hsOut2, $hsFrameLine, $hsCapture, [LaneSynth]::DragState())
        } catch {
            HSObserve 'error-dnd-edge-scroll' ([string]$_)
            try { [LaneSynth]::ReleaseAll() } catch { }
        }

        # ---- H93 / the configured knobs are honoured ----------------------------------------
        # A wider zone, a longer delay and a slower speed: nothing may scroll before `delay-ms`
        # elapses, and a pointer outside `trigger-width` must never scroll at all.
        try {
            HSReload (HSConfig 'gestures { dnd-edge-view-scroll { trigger-width 140; delay-ms 600; max-speed 600; } }')
            HSShow $hsSource
            $hsSrcPt = HSPoint $hsSource 0.5 0.5
            $hsFramesA = HSCount 'dnd edge frame'
            $null = HSDragBegin $hsSrcPt.X $hsSrcPt.Y
            # 100 px from the left edge: inside the 140 px zone, but before the 600 ms delay.
            HSDragTo $hsSrcPt.X $hsSrcPt.Y ($hsScreen.Left + 100) $hsMidY 6 30
            Start-Sleep -Milliseconds 250
            $hsFramesEarly = HSCount 'dnd edge frame'
            Start-Sleep -Milliseconds 550
            $hsFramesLate = HSCount 'dnd edge frame'
            HSDragEnd
            Start-Sleep -Milliseconds 400
            $hsFramesSettled = HSCount 'dnd edge frame'
            # A second, separate drag that stops 300 px from the edge: outside `trigger-width`, so
            # the zone timer can never be armed and no frame may appear at all.
            HSShow $hsSource
            $hsSrcPt = HSPoint $hsSource 0.5 0.5
            $null = HSDragBegin $hsSrcPt.X $hsSrcPt.Y
            HSDragTo $hsSrcPt.X $hsSrcPt.Y ($hsScreen.Left + 300) $hsMidY 6 30
            Start-Sleep -Milliseconds 900
            $hsFramesOutside = HSCount 'dnd edge frame'
            HSDragEnd
            HSGate 'h93_dnd_edge_view_scroll_honours_its_knobs' (($hsFramesEarly -eq $hsFramesA) -and ($hsFramesLate -gt $hsFramesEarly) -and ($hsFramesOutside -eq $hsFramesSettled)) @($hsFramesA, $hsFramesEarly, $hsFramesLate, $hsFramesSettled, $hsFramesOutside)
        } catch {
            HSObserve 'error-dnd-knobs' ([string]$_) 
            try { [LaneSynth]::ReleaseAll() } catch { }
        }
        try { HSReload (HSConfig) } catch { HSObserve 'error-dnd-restore' ([string]$_) }

        # ---- H93 / hold-to-activate ---------------------------------------------------------
        # Holding a drag over a window for 500 ms activates it (Gestures.md#drag-and-drop-hold-to-activate).
        # The observable is the native foreground: the drag source owns it when the press starts,
        # and wiri's activation request must move it to the hovered window.
        try {
            HSShow $hsSource
            HSShow $hsB
            $hsBpt = HSPoint $hsB 0.5 0.5
            $hsSrcPt = HSPoint $hsSource 0.5 0.5
            $null = HSDragBegin $hsSrcPt.X $hsSrcPt.Y
            $hsFgBefore = [int64][LaneSynth]::Foreground()
            HSDragTo $hsSrcPt.X $hsSrcPt.Y $hsBpt.X $hsBpt.Y 8 40
            Start-Sleep -Milliseconds 250
            $hsFgEarly = [int64][LaneSynth]::Foreground()
            Start-Sleep -Milliseconds 900
            $hsFgDwell = [int64][LaneSynth]::Foreground()
            HSDragEnd
            HSGate 'h93_dnd_hold_to_activate_focuses_the_hovered_window' (($hsFgBefore -ne [int64]$hsB) -and ($hsFgEarly -ne [int64]$hsB) -and ($hsFgDwell -eq [int64]$hsB)) @($hsFgBefore, $hsFgEarly, $hsFgDwell, [int64]$hsB, [int64]$hsSource, [LaneSynth]::DragState())
        } catch {
            HSObserve 'error-dnd-dwell' ([string]$_) 
            try { [LaneSynth]::ReleaseAll() } catch { }
        }

        # ---- H93 / drag-and-drop edge workspace switch (overview) ---------------------------
        # niri switches the workspace stack when a drag is held against the top/bottom edge
        # *while in the overview* long enough to travel one screen (Configuration:-Gestures);
        # the drag therefore starts in the normal view and the overview opens under it.
        try {
            HSShow $hsSource
            $hsWs0 = [string](HSState 'hs-state-before-switch').active_workspace
            $hsSteps0 = HSCount 'dnd workspace step delta=1'
            $hsOverviews0 = HSCount 'Overview opened'
            $hsSrcPt = HSPoint $hsSource 0.5 0.5
            $null = HSDragBegin $hsSrcPt.X $hsSrcPt.Y
            HSAction 'hs-overview-open' @('open-overview')
            Start-Sleep -Milliseconds 900
            $hsOverviews1 = HSCount 'Overview opened'
            HSDragTo $hsSrcPt.X $hsSrcPt.Y $hsMidX ($hsScreen.Bottom - 40) 6 40
            [LaneSynth]::Move($hsMidX, ($hsScreen.Bottom - 4))
            Start-Sleep -Milliseconds 1600
            $hsWs1 = [string](HSState 'hs-state-during-switch').active_workspace
            $hsSteps1 = HSCount 'dnd workspace step delta=1'
            Start-Sleep -Milliseconds 900
            $hsWs2 = [string](HSState 'hs-state-after-switch').active_workspace
            $hsSteps2 = HSCount 'dnd workspace step delta=1'
            HSDragEnd
            HSAction 'hs-overview-close' @('close-overview')
            Start-Sleep -Milliseconds 600
            HSGate 'h93_dnd_edge_workspace_switch_steps_once' (($hsOverviews1 -gt $hsOverviews0) -and ($hsSteps1 -eq $hsSteps0 + 1) -and ($hsWs1 -ne $hsWs0) -and ($hsSteps2 -eq $hsSteps1) -and ($hsWs2 -eq $hsWs1)) @($hsWs0, $hsWs1, $hsWs2, $hsSteps0, $hsSteps1, $hsSteps2, $hsOverviews0, $hsOverviews1)
        } catch {
            HSObserve 'error-dnd-workspace-step' ([string]$_) 
            try { [LaneSynth]::ReleaseAll() } catch { }
            try { HSAction 'hs-overview-close-err' @('close-overview') } catch { }
        }
        # Back to the fixture workspace for the pointer-path gates.
        try { HSAction 'hs-workspace-back' @('focus-workspace', '2') } catch { HSObserve 'error-back' ([string]$_) }

        # ---- H93 / touch: Mod+drag scrolls a tiled view horizontally ------------------------
        # Gestures.md#touchscreen: "Dragging a tiled window horizontally will scroll the view
        # instead"; the axis locks at 8 px (src/input/touchpen.rs select_mode).
        try {
            $hsTiled = HSPickTiled @($hsD, $hsC, $hsB, $hsA)
            HSShow $hsTiled
            Start-Sleep -Milliseconds 300
            $hsPt = HSPoint $hsTiled 0.75 0.5
            $hsR0 = Rc $hsTiled
            [LaneSynth]::Key(0xA4, $false)
            HSTouchDrag $hsPt.X $hsPt.Y -150 0 8 45
            [LaneSynth]::Key(0xA4, $true)
            Start-Sleep -Milliseconds 900
            $hsR1 = Rc $hsTiled
            $hsMoved = $hsR1.L - $hsR0.L
            HSGate 'h93_touch_mod_drag_scrolls_the_view' (($hsMoved -le -40) -and (Near $hsR1.T $hsR0.T 4) -and (Near $hsR1.W $hsR0.W 4)) @($hsR0, $hsR1, $hsMoved, [int64]$hsTiled)
        } catch {
            HSObserve 'error-touch-scroll' ([string]$_) 
            try { [LaneSynth]::Key(0xA4, $true) } catch { }
        }

        # ---- H93 / touch: Mod+drag moves a window ------------------------------------------
        # A floating window is moved by the contact itself (niri Gestures.md#touchscreen move).
        try {
            HSShow $hsA
            HSFloat $hsA $true
            HSShow $hsA
            $hsPt = HSPoint $hsA 0.5 0.5
            $hsR0 = Rc $hsA
            [LaneSynth]::Key(0xA4, $false)
            HSTouchDrag $hsPt.X $hsPt.Y 96 64 8 45
            [LaneSynth]::Key(0xA4, $true)
            Start-Sleep -Milliseconds 900
            $hsR1 = Rc $hsA
            Start-Sleep -Milliseconds 400
            $hsR2 = Rc $hsA
            HSGate 'h93_touch_mod_drag_moves_a_floating_window' ((Near ($hsR1.L - $hsR0.L) 96 26) -and (Near ($hsR1.T - $hsR0.T) 64 26) -and (Near $hsR1.L $hsR2.L 2) -and (Near $hsR1.T $hsR2.T 2)) @($hsR0, $hsR1, $hsR2)
            HSFloat $hsA $false
        } catch {
            HSObserve 'error-touch-move' ([string]$_) 
            try { [LaneSynth]::Key(0xA4, $true) } catch { }
        }

        # ---- H93 / pen: Mod+drag moves a window --------------------------------------------
        try {
            HSShow $hsB
            HSFloat $hsB $true
            HSShow $hsB
            $hsPt = HSPoint $hsB 0.5 0.5
            $hsR0 = Rc $hsB
            [LaneSynth]::Key(0xA4, $false)
            HSPenDrag $hsPt.X $hsPt.Y -80 56 8 45
            [LaneSynth]::Key(0xA4, $true)
            Start-Sleep -Milliseconds 900
            $hsR1 = Rc $hsB
            Start-Sleep -Milliseconds 400
            $hsR2 = Rc $hsB
            HSGate 'h93_pen_mod_drag_moves_a_floating_window' ((Near ($hsR1.L - $hsR0.L) -80 26) -and (Near ($hsR1.T - $hsR0.T) 56 26) -and (Near $hsR1.L $hsR2.L 2) -and (Near $hsR1.T $hsR2.T 2)) @($hsR0, $hsR1, $hsR2)
            HSFloat $hsB $false
        } catch {
            HSObserve 'error-pen-move' ([string]$_) 
            try { [LaneSynth]::Key(0xA4, $true) } catch { }
        }

        # ---- H93 / pen barrel button runs its stylus bind -----------------------------------
        # niri binds stylus buttons as Mod+TabletStylusButton1-3 (Configuration:-Key-Bindings);
        # Windows promotes the barrel button to the right button with the pen signature set.
        # The bind toggles the floating state of the focused window, so a tile becomes a float.
        try {
            HSShow $hsC
            HSFloat $hsC $false
            HSShow $hsC
            $hsBefore = [bool](HSWin (HSList 'hs-list-before-barrel') $hsC).floating
            $hsPt = HSPoint $hsC 0.5 0.5
            [LaneSynth]::Key(0xA4, $false)
            [LaneSynth]::Pen($hsPt.X, $hsPt.Y, ([LaneSynth]::FLAG_DOWN -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), [LaneSynth]::PEN_BARREL)
            Start-Sleep -Milliseconds 250
            [LaneSynth]::Pen($hsPt.X, $hsPt.Y, [LaneSynth]::FLAG_UP, [LaneSynth]::PEN_BARREL)
            Start-Sleep -Milliseconds 500
            [LaneSynth]::Key(0xA4, $true)
            Start-Sleep -Milliseconds 700
            $hsAfter = [bool](HSWin (HSList 'hs-list-after-barrel') $hsC).floating
            HSGate 'h93_pen_barrel_runs_its_tablet_stylus_bind' ((-not $hsBefore) -and $hsAfter) @($hsBefore, $hsAfter, (HSCount 'Tablet stylus binds'))
        } catch {
            HSObserve 'error-pen-barrel' ([string]$_) 
            try { [LaneSynth]::Key(0xA4, $true) } catch { }
        }

        # ---- H93 / touch overview: long press picks a window up ----------------------------
        # Overview row: one finger pans, resting 500 ms picks a window up and the drop uses the
        # mouse drop path (src/overlay/overview_pointer.rs:56-90). This is *observed*, not gated:
        # on this host the synthetic contact's long-press pickup never produced a drop that moved
        # the window, so the row keeps its remaining gap instead of claiming a pass.
        try {
            HSShow $hsD
            $hsOverviews0 = HSCount 'Overview opened'
            HSAction 'hs-overview-open2' @('open-overview')
            Start-Sleep -Milliseconds 1200
            if ((HSCount 'Overview opened') -le $hsOverviews0) { throw 'overview did not open' }
            # The overview zooms the active workspace rect by `overview { zoom }` (0.5) about the
            # view centre (src/layout/engine/overview.rs overview_workspace_stack / zoom_rect), but
            # the painted tile is located from the screenshot instead of from a copy of that
            # geometry: the overview draws a live thumbnail of the real window, so its own colour
            # marks exactly where a press must land.
            Shot 'h93-overview'
            $hsBox = [LaneSynth]::FindColorRect((Join-Path $D 'h93-overview.png'), 160, 160, 60, 24)
            if ($hsBox -notmatch '^(\d+),(\d+),(\d+),(\d+) ') {
                # A dimmed workspace paints the same colour at half brightness.
                $hsBox = [LaneSynth]::FindColorRect((Join-Path $D 'h93-overview.png'), 80, 80, 30, 24)
            }
            if ($hsBox -notmatch '^(\d+),(\d+),(\d+),(\d+) ') { throw ('thumb of hs-d not found: ' + $hsBox) }
            $hsPressX = [int]$Matches[1] + 16
            $hsPressY = [int]$Matches[2] + 16
            $hsWsBefore = [string](HSWin (HSList 'hs-list-ov-before') $hsD).workspace
            # Park the cursor on the tile first: a promoted move that still reports the previous
            # cursor position would count as a pan and cancel the long press.
            [LaneSynth]::Move($hsPressX, $hsPressY)
            Start-Sleep -Milliseconds 300
            HSTouchFrame $hsPressX $hsPressY ([LaneSynth]::FLAG_DOWN)
            Start-Sleep -Milliseconds 900
            # A long press is not a pan: the contact now drags the picked-up window.
            foreach ($i in 1..6) {
                HSTouchFrame $hsPressX ($hsPressY + 70 * $i) ([LaneSynth]::FLAG_UPDATE)
                Start-Sleep -Milliseconds 60
            }
            [LaneSynth]::Touch(@(1), @($hsPressX), @(($hsPressY + 420)), @([LaneSynth]::FLAG_UP), @(0))
            $hsWsAfter = $hsWsBefore
            $sw = [Diagnostics.Stopwatch]::StartNew()
            while ($sw.ElapsedMilliseconds -lt 2500 -and $hsWsAfter -eq $hsWsBefore) {
                Start-Sleep -Milliseconds 250
                $hsWsAfter = [string](HSWin (HSList 'hs-list-ov-after') $hsD).workspace
            }
            HSAction 'hs-overview-close2' @('close-overview')
            # Recorded, not gated: on this host the picked-up drop never moved the window, so the
            # row keeps its remaining gap instead of claiming a pass.
            HSObserve 'touch_overview_long_press_observed' ([ordered]@{
                press = "$hsPressX,$hsPressY"; thumb = $hsBox
                workspace_before = $hsWsBefore; workspace_after = $hsWsAfter
                moved = ($hsWsBefore -ne $hsWsAfter)
            })
        } catch {
            HSObserve 'error-overview-long-press' ([string]$_) 
            try { HSAction 'hs-overview-close-err2' @('close-overview') } catch { }
        }

        # ---- H93 / touch overview: one-finger pan steps workspaces -------------------------
        # A vertical finger travel of 150 px steps one workspace; a horizontal pan only scrolls
        # the overview camera and must not change the active workspace.
        try {
            $hsOverviews0 = HSCount 'Overview opened'
            HSAction 'hs-overview-open3' @('open-overview')
            Start-Sleep -Milliseconds 1200
            if ((HSCount 'Overview opened') -le $hsOverviews0) { throw 'overview did not open' }
            $hsView = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
            $hsPressX = $hsView.Left + [int]($hsView.Width / 2)
            $hsPressY = $hsView.Top + [int]($hsView.Height / 2)
            $hsActive0 = [string](HSState 'hs-state-pan-before').active_workspace
            # Horizontal pan first: 138 px, then lift. It must not switch anything.
            [LaneSynth]::Move($hsPressX, $hsPressY)
            Start-Sleep -Milliseconds 250
            HSTouchFrame $hsPressX $hsPressY ([LaneSynth]::FLAG_DOWN)
            Start-Sleep -Milliseconds 120
            foreach ($i in 1..6) { HSTouchFrame ($hsPressX + 23 * $i) $hsPressY ([LaneSynth]::FLAG_UPDATE); Start-Sleep -Milliseconds 45 }
            [LaneSynth]::Touch(@(1), @(($hsPressX + 138)), @($hsPressY), @([LaneSynth]::FLAG_UP), @(0))
            Start-Sleep -Milliseconds 500
            $hsActiveH = [string](HSState 'hs-state-pan-h').active_workspace
            # Vertical pan: more than one 150 px step.
            [LaneSynth]::Move($hsPressX, $hsPressY)
            Start-Sleep -Milliseconds 250
            HSTouchFrame $hsPressX $hsPressY ([LaneSynth]::FLAG_DOWN)
            Start-Sleep -Milliseconds 120
            foreach ($i in 1..10) { HSTouchFrame $hsPressX ($hsPressY + 21 * $i) ([LaneSynth]::FLAG_UPDATE); Start-Sleep -Milliseconds 45 }
            [LaneSynth]::Touch(@(1), @($hsPressX), @(($hsPressY + 210)), @([LaneSynth]::FLAG_UP), @(0))
            Start-Sleep -Milliseconds 700
            $hsActiveV = [string](HSState 'hs-state-pan-v').active_workspace
            HSAction 'hs-overview-close3' @('close-overview')
            HSGate 'h93_touch_overview_pan_steps_workspaces' (($hsActiveH -eq $hsActive0) -and ($hsActiveV -ne $hsActive0)) @($hsActive0, $hsActiveH, $hsActiveV)
        } catch {
            HSObserve 'error-overview-pan' ([string]$_) 
            try { HSAction 'hs-overview-close-err3' @('close-overview') } catch { }
        }
        # Whether Windows promotes a second simultaneous contact at all: recorded, not gated, so
        # the matrix can say exactly what a two-finger tap can and cannot prove.
        try {
            HSAction 'hs-overview-close4' @('close-overview')
            [LaneSynth]::ClearHook()
            HSShow $hsD
            $hsPt = HSPoint $hsD 0.5 0.5
            [LaneSynth]::Touch(@(1), @($hsPt.X), @($hsPt.Y), @(([LaneSynth]::FLAG_DOWN -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT)), @(1))
            Start-Sleep -Milliseconds 250
            [LaneSynth]::Touch(@(1, 2), @($hsPt.X, ($hsPt.X + 60)), @($hsPt.Y, ($hsPt.Y + 40)), @(([LaneSynth]::FLAG_UPDATE -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT), ([LaneSynth]::FLAG_DOWN -bor [LaneSynth]::FLAG_INRANGE -bor [LaneSynth]::FLAG_INCONTACT)), @(0, 0))
            Start-Sleep -Milliseconds 300
            HSObserve 'second_contact_hook_lines' @([LaneSynth]::HookLines() | Select-Object -First 14)
            [LaneSynth]::Touch(@(2), @(($hsPt.X + 60)), @(($hsPt.Y + 40)), @([LaneSynth]::FLAG_UP), @(0))
            Start-Sleep -Milliseconds 250
            [LaneSynth]::Touch(@(1), @($hsPt.X), @($hsPt.Y), @([LaneSynth]::FLAG_UP), @(0))
            Start-Sleep -Milliseconds 250
        } catch {
            HSObserve 'error-second-contact' ([string]$_)
        }
    } catch {
        HSObserve 'error' ([string]$_)
        HSObserve 'error-position' $_.InvocationInfo.PositionMessage
    } finally {
        try { HSCloseAll } catch { $hsDetails['cleanup-error'] = [string]$_ }
        try { [LaneSynth]::StopHook() } catch { }
        try { [LaneSynth]::DestroyDevices() } catch { }
        try { [LaneSynth]::ReleaseAll() } catch { }
        if ($null -ne $hsProc) { try { Stop-WiriGracefully $hsProc 'lane-synth-final' } catch { $hsDetails['stop-error'] = [string]$_ } }
        if ($null -ne $hsCursor) { try { [LaneSynth]::Move($hsCursor.X, $hsCursor.Y) } catch { } }
        $hsDetails | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $hsDir 'results.json')
    }
}
