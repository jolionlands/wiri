// Demo fixtures for scripts/demo/record-demo.ps1.
//
// Two jobs, both owned by the recording script's own process:
//   DemoNative — the Win32 calls the choreography needs (real key chords, a real
//                click, window enumeration, the overview host's tile geometry).
//   DemoApps   — the windows the video shows.  They are ordinary WinForms windows
//                (wiri tiles them like any other application) painted to look like
//                an editor, a terminal, notes, a browser, a file manager, a chat and
//                a music player.  Nothing here reads a user file or opens a network
//                connection: every pixel is drawn from the constants below.
//
// Loaded with Add-Type -Path from record-demo.ps1; requires System.Windows.Forms and
// System.Drawing.  No state survives the script's process.
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

public static class DemoNative
{
    public delegate bool EnumProc(IntPtr hwnd, IntPtr param);

    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; public int Width { get { return Right - Left; } } public int Height { get { return Bottom - Top; } } }

    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr param);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr hwnd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int command);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
    [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
    [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string className, string title);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr hwnd, string name);
    [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT point);
    [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr hwnd, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wparam, IntPtr lparam);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr hwnd, int index);
    [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] static extern int GetWindowLong(IntPtr hwnd, int index);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hwnd, int attribute, out RECT rect, int size);
    [DllImport("dwmapi.dll", EntryPoint = "DwmGetWindowAttribute")] public static extern int DwmGetInt(IntPtr hwnd, int attribute, out int value, int size);
    [DllImport("dwmapi.dll", EntryPoint = "DwmSetWindowAttribute")] public static extern int DwmSetInt(IntPtr hwnd, int attribute, ref int value, int size);

    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct HARDWAREINPUT { public uint uMsg; public ushort wParamL, wParamH; }
    [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public HARDWAREINPUT hi; }
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);

    const uint INPUT_KEYBOARD = 1, INPUT_MOUSE = 0;
    const uint KEYEVENTF_KEYUP_SEND = 0x0002;
    const uint MOUSEEVENTF_LEFTDOWN_SEND = 0x0002, MOUSEEVENTF_LEFTUP_SEND = 0x0004;
    const int WS_EX_TOPMOST_BIT = 0x00000008;
    const uint WM_KEYDOWN = 0x0100, WM_KEYUP = 0x0101;

    /// One visible top-level window, as the pre-flight log wants it.
    public struct WindowInfo
    {
        public IntPtr Hwnd;
        public uint Pid;
        public string Process;
        public string Class;
        public string Title;
        public RECT Rect;
        public bool Topmost;
        public bool Cloaked;
        public bool Visible;
    }

    static readonly Dictionary<uint, string> ProcessNames = new Dictionary<uint, string>();

    static string ProcessNameOf(uint pid)
    {
        lock (ProcessNames)
        {
            string name;
            if (ProcessNames.TryGetValue(pid, out name)) return name;
            try { name = System.Diagnostics.Process.GetProcessById((int)pid).ProcessName; }
            catch { name = "?"; }
            ProcessNames[pid] = name;
            return name;
        }
    }

    public static string ProcessOf(IntPtr hwnd) { return ProcessNameOf(PidOf(hwnd)); }

    public static long ExStyleOf(IntPtr hwnd)
    {
        return IntPtr.Size == 8 ? GetWindowLongPtr(hwnd, -20).ToInt64() : GetWindowLong(hwnd, -20);
    }

    // Every top-level window, described for the log and for the stray-popup checks.
    public static List<WindowInfo> TopLevelWindows(bool visibleOnly)
    {
        var found = new List<WindowInfo>();
        EnumWindows(delegate(IntPtr hwnd, IntPtr param)
        {
            var info = new WindowInfo();
            info.Hwnd = hwnd;
            info.Pid = PidOf(hwnd);
            info.Process = ProcessNameOf(info.Pid);
            info.Class = ClassOf(hwnd);
            info.Title = Title(hwnd);
            info.Rect = Bounds(hwnd);
            info.Topmost = (ExStyleOf(hwnd) & WS_EX_TOPMOST_BIT) != 0;
            info.Cloaked = Cloaked(hwnd);
            info.Visible = IsWindowVisible(hwnd) && !IsIconic(hwnd);
            if (visibleOnly && !(info.Visible && !info.Cloaked)) return true;
            found.Add(info);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string Describe(WindowInfo info)
    {
        return string.Format("hwnd=0x{0:X} pid={1} process={2} class={3} rect={4},{5} {6}x{7} topmost={8} cloaked={9} visible={10} title={11}",
            info.Hwnd.ToInt64(), info.Pid, info.Process, info.Class,
            info.Rect.Left, info.Rect.Top, info.Rect.Width, info.Rect.Height,
            info.Topmost, info.Cloaked, info.Visible, info.Title);
    }

    public static bool StillOnScreen(IntPtr hwnd)
    {
        return IsWindow(hwnd) && IsWindowVisible(hwnd) && !IsIconic(hwnd) && !Cloaked(hwnd);
    }

    // A menu or flyout closes on Escape.  A posted key first (cheap, no input injected into
    // the desktop), then a real SendInput Escape with the popup in the foreground.
    public static void PostEscape(IntPtr hwnd)
    {
        PostMessage(hwnd, WM_KEYDOWN, (IntPtr)0x1B, IntPtr.Zero);
        PostMessage(hwnd, WM_KEYUP, (IntPtr)0x1B, IntPtr.Zero);
    }

    public static void SendInputEscape()
    {
        var inputs = new INPUT[2];
        inputs[0].type = INPUT_KEYBOARD;
        inputs[0].u.ki.wVk = 0x1B;
        inputs[1].type = INPUT_KEYBOARD;
        inputs[1].u.ki.wVk = 0x1B;
        inputs[1].u.ki.dwFlags = KEYEVENTF_KEYUP_SEND;
        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    // A left click on an empty part of the desktop: menus dismiss on any outside click.
    public static void SendInputClick(int x, int y)
    {
        SetCursorPos(x, y);
        var inputs = new INPUT[2];
        inputs[0].type = INPUT_MOUSE;
        inputs[0].u.mi.dwFlags = MOUSEEVENTF_LEFTDOWN_SEND;
        inputs[1].type = INPUT_MOUSE;
        inputs[1].u.mi.dwFlags = MOUSEEVENTF_LEFTUP_SEND;
        SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    }

    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }

    const int DWMWA_CLOAKED = 14;
    const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;
    const int SW_MINIMIZE = 6;
    const int SW_RESTORE = 9;
    const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
    const uint MOUSEEVENTF_LEFTUP = 0x0004;
    const uint KEYEVENTF_EXTENDEDKEY = 0x0001;
    const uint KEYEVENTF_KEYUP = 0x0002;

    public static string Title(IntPtr hwnd)
    {
        var text = new StringBuilder(512);
        GetWindowText(hwnd, text, text.Capacity);
        return text.ToString();
    }

    public static string ClassOf(IntPtr hwnd)
    {
        var text = new StringBuilder(256);
        GetClassName(hwnd, text, text.Capacity);
        return text.ToString();
    }

    public static uint PidOf(IntPtr hwnd)
    {
        uint pid;
        GetWindowThreadProcessId(hwnd, out pid);
        return pid;
    }

    // What the user sees: DWM's frame bounds, without the invisible resize border.
    public static RECT Bounds(IntPtr hwnd)
    {
        RECT rect;
        if (DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, out rect, Marshal.SizeOf(typeof(RECT))) != 0)
            GetWindowRect(hwnd, out rect);
        return rect;
    }

    public static bool Cloaked(IntPtr hwnd)
    {
        int value;
        return DwmGetInt(hwnd, DWMWA_CLOAKED, out value, 4) == 0 && value != 0;
    }

    // Visible top-level windows that carry a title, minus the shell's own furniture.
    public static List<IntPtr> VisibleWindows()
    {
        var found = new List<IntPtr>();
        EnumWindows(delegate(IntPtr hwnd, IntPtr param)
        {
            if (!IsWindowVisible(hwnd) || IsIconic(hwnd) || Cloaked(hwnd)) return true;
            string title = Title(hwnd);
            if (title.Length == 0) return true;
            string cls = ClassOf(hwnd);
            if (cls == "Progman" || cls == "Shell_TrayWnd" || cls == "Button" || cls.StartsWith("Wiri")) return true;
            found.Add(hwnd);
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static string Describe(IntPtr hwnd)
    {
        RECT rect = Bounds(hwnd);
        return string.Format("hwnd=0x{0:X} pid={1} class={2} iconic={3} cloaked={4} rect={5},{6} {7}x{8} title={9}",
            hwnd.ToInt64(), PidOf(hwnd), ClassOf(hwnd), IsIconic(hwnd), Cloaked(hwnd),
            rect.Left, rect.Top, rect.Width, rect.Height, Title(hwnd));
    }

    public static void Minimize(IntPtr hwnd) { ShowWindow(hwnd, SW_MINIMIZE); }
    public static void Restore(IntPtr hwnd) { ShowWindow(hwnd, SW_RESTORE); }

    public static void ParkCursor()
    {
        // The outer gap at the top right: not over a window, not over the tray (hovering the
        // tray pops tooltips into the frame), and outside the caption band's left-aligned mark.
        SetCursorPos(System.Windows.Forms.Screen.PrimaryScreen.Bounds.Right - 10, 6);
    }

    public static void Click(int x, int y)
    {
        SetCursorPos(x, y);
        Thread.Sleep(220);
        mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, UIntPtr.Zero);
        Thread.Sleep(70);
        mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, UIntPtr.Zero);
    }

    static byte VirtualKey(string name)
    {
        switch (name)
        {
            case "ctrl": return 0x11;
            case "alt": return 0x12;
            case "shift": return 0x10;
            case "win": return 0x5B;
            case "left": return 0x25;
            case "up": return 0x26;
            case "right": return 0x27;
            case "down": return 0x28;
            case "home": return 0x24;
            case "end": return 0x23;
            case "pageup": return 0x21;
            case "pagedown": return 0x22;
            case "space": return 0x20;
            case "escape": return 0x1B;
            case "esc": return 0x1B;
            case "return": return 0x0D;
            case "tab": return 0x09;
            case "backslash": return 0xDC;
            case "comma": return 0xBC;
            case "period": return 0xBE;
            default:
                if (name.Length == 1)
                {
                    char c = char.ToUpperInvariant(name[0]);
                    if (c >= 'A' && c <= 'Z') return (byte)c;
                    if (c >= '0' && c <= '9') return (byte)c;
                }
                if (name.StartsWith("f") && name.Length <= 3)
                {
                    int n;
                    if (int.TryParse(name.Substring(1), out n) && n >= 1 && n <= 24) return (byte)(0x70 + n - 1);
                }
                throw new ArgumentException("unknown key '" + name + "' in chord");
        }
    }

    public static void Key(byte vk, bool up)
    {
        uint flags = 0;
        // Arrows, Home/End, Page up/down, Insert/Delete carry the extended flag.
        if ((vk >= 0x21 && vk <= 0x28) || vk == 0x2D || vk == 0x2E) flags |= KEYEVENTF_EXTENDEDKEY;
        if (up) flags |= KEYEVENTF_KEYUP;
        keybd_event(vk, 0, flags, UIntPtr.Zero);
    }

    // "ctrl+alt+left" -> hold each key in order, then release in reverse.  The waits
    // match the live harness: wiri's low-level hook must see the chord as one press.
    public static void Chord(string spec)
    {
        string[] parts = spec.ToLowerInvariant().Split('+');
        var keys = new byte[parts.Length];
        for (int i = 0; i < parts.Length; i++) keys[i] = VirtualKey(parts[i].Trim());
        foreach (byte key in keys) { Key(key, false); Thread.Sleep(40); }
        Thread.Sleep(90);
        for (int i = keys.Length - 1; i >= 0; i--) { Key(keys[i], true); Thread.Sleep(30); }
    }

    public static IntPtr OverviewHost() { return FindWindow("WiriThumbnailOverviewHost", null); }

    public static bool OverviewOpen() { return OverviewHost() != IntPtr.Zero; }

    public static long OverviewProperty(string name)
    {
        IntPtr host = OverviewHost();
        if (host == IntPtr.Zero) return long.MinValue;
        return GetProp(host, "WiriOverview" + name).ToInt64();
    }

    // Every tile the overview draws, in the host's client coordinates:
    // "index|hwnd|x|y|w|h".  Session-local properties published by the overview.
    public static string[] OverviewTiles()
    {
        IntPtr host = OverviewHost();
        if (host == IntPtr.Zero) return new string[0];
        long count = GetProp(host, "WiriOverviewTileCount").ToInt64();
        var tiles = new List<string>();
        for (long i = 0; i < count; i++)
        {
            string prefix = "WiriOverviewTile" + i;
            tiles.Add(string.Format("{0}|{1}|{2}|{3}|{4}|{5}", i,
                GetProp(host, prefix + "Window").ToInt64(),
                GetProp(host, prefix + "X").ToInt64(),
                GetProp(host, prefix + "Y").ToInt64(),
                GetProp(host, prefix + "W").ToInt64(),
                GetProp(host, prefix + "H").ToInt64()));
        }
        return tiles.ToArray();
    }
}

// ---------------------------------------------------------------------------
// The demo applications.
// ---------------------------------------------------------------------------
public static class DemoApps
{
    sealed class Opening
    {
        public string Kind;
        public string Caption;
        public Form Form;
        public IntPtr Handle;
        public bool Ready;
        public Exception Error;
        public readonly object Sync = new object();
    }

    static readonly List<Opening> Openings = new List<Opening>();
    static bool visualStyles;

    public static IntPtr Open(string kind, string caption)
    {
        lock (Openings)
        {
            if (!visualStyles)
            {
                visualStyles = true;
                try { Application.EnableVisualStyles(); } catch { }
            }
        }
        var opening = new Opening { Kind = kind, Caption = caption };
        var thread = new Thread(delegate()
        {
            try
            {
                var form = new DemoForm(kind, caption);
                lock (opening.Sync) { opening.Form = form; }
                form.Shown += delegate
                {
                    lock (opening.Sync)
                    {
                        opening.Handle = form.Handle;
                        opening.Ready = true;
                        Monitor.PulseAll(opening.Sync);
                    }
                };
                Application.Run(form);
            }
            catch (Exception error)
            {
                lock (opening.Sync) { opening.Error = error; Monitor.PulseAll(opening.Sync); }
            }
            finally
            {
                lock (opening.Sync) { Monitor.PulseAll(opening.Sync); }
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
        lock (Openings) Openings.Add(opening);
        lock (opening.Sync)
        {
            var clock = System.Diagnostics.Stopwatch.StartNew();
            while (!opening.Ready && opening.Error == null && clock.ElapsedMilliseconds < 15000)
                Monitor.Wait(opening.Sync, 200);
            if (!opening.Ready)
            {
                if (opening.Error != null) throw new InvalidOperationException("demo window '" + caption + "' failed", opening.Error);
                throw new TimeoutException("demo window '" + caption + "' did not show up");
            }
            return opening.Handle;
        }
    }

    public static bool Alive(IntPtr hwnd)
    {
        lock (Openings)
        {
            foreach (var opening in Openings)
                if (opening.Ready && opening.Handle == hwnd) return DemoNative.IsWindow(hwnd) && !opening.Form.IsDisposed;
        }
        return false;
    }

    public static int CloseAll()
    {
        var pending = new List<Opening>();
        lock (Openings) { pending.AddRange(Openings); Openings.Clear(); }
        int count = 0;
        var clock = System.Diagnostics.Stopwatch.StartNew();
        foreach (var opening in pending)
        {
            Form form;
            lock (opening.Sync) form = opening.Form;
            if (form == null) continue;
            try
            {
                if (!form.IsDisposed && form.IsHandleCreated)
                {
                    form.BeginInvoke((MethodInvoker)delegate { form.Close(); });
                    count++;
                }
            }
            catch (InvalidOperationException) { }
        }
        // Give every UI thread time to run its Close and return from Application.Run.
        foreach (var opening in pending)
        {
            Form form;
            lock (opening.Sync) form = opening.Form;
            if (form == null) continue;
            while (clock.ElapsedMilliseconds < 10000)
            {
                bool gone;
                lock (opening.Sync) gone = opening.Form == null || opening.Form.IsDisposed;
                if (gone) break;
                Thread.Sleep(50);
            }
        }
        return count;
    }

    public static string[] OpenTitles()
    {
        var titles = new List<string>();
        lock (Openings) foreach (var opening in Openings) titles.Add(opening.Caption);
        return titles.ToArray();
    }

    // Paint every mock off screen, without a window on the desktop and without a
    // screenshot: a broken body has to fail the run before the camera rolls, not
    // after.  Returns one line per kind ("editor ink=7.4%") or the exception text.
    public static string[] SelfTest()
    {
        var report = new List<string>();
        var thread = new Thread(delegate()
        {
            foreach (string kind in new string[] { "editor", "terminal", "notes", "browser", "files", "chat", "music" })
            {
                try
                {
                    using (var form = new DemoForm(kind, "selftest-" + kind))
                    {
                        form.Size = new Size(760, 620);
                        IntPtr handle = form.Handle;
                        using (var bitmap = new Bitmap(760, 620))
                        {
                            form.DrawToBitmap(bitmap, new Rectangle(0, 0, 760, 620));
                            int ink = 0;
                            for (int y = 0; y < bitmap.Height; y += 2)
                                for (int x = 0; x < bitmap.Width; x += 2)
                                {
                                    Color pixel = bitmap.GetPixel(x, y);
                                    if (pixel.R > 0x3A || pixel.G > 0x3A || pixel.B > 0x44) ink++;
                                }
                            double percent = 100.0 * ink / ((bitmap.Width / 2.0) * (bitmap.Height / 2.0));
                            report.Add(string.Format("{0} ink={1:F1}%", kind, percent));
                        }
                    }
                }
                catch (Exception error)
                {
                    report.Add(kind + " FAILED " + error.GetType().Name + ": " + error.Message);
                }
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
        thread.Join(30000);
        if (report.Count == 0) report.Add("selftest FAILED (no report)");
        return report.ToArray();
    }
}

// Pixel checks over a frame that ffmpeg pulled out of the finished video: a caption band
// that is really dark with really bright glyphs, the accent word mark, and the app colours
// of the mock windows.  Done here rather than in PowerShell so a 1366x768 frame takes
// milliseconds instead of a minute.
public static class FrameStats
{
    static readonly int[][] Accents = new int[][]
    {
        // blue, green, amber, purple, cyan, pink, teal - the mock apps' accent colours
        new int[] { 0x7A, 0xA2, 0xF7 }, new int[] { 0x9E, 0xCE, 0x6A }, new int[] { 0xE0, 0xAF, 0x68 },
        new int[] { 0xBB, 0x9A, 0xF7 }, new int[] { 0x7D, 0xCF, 0xFF }, new int[] { 0xF7, 0x76, 0x8E },
        new int[] { 0x73, 0xDA, 0xCA },
    };
    static readonly string[] AccentNames = new string[] { "blue", "green", "amber", "purple", "cyan", "pink", "teal" };

    // One compact line of measurements for the log.
    public static string Analyze(string path, int bandHeight)
    {
        using (var bitmap = new Bitmap(path))
        {
            int width = bitmap.Width, height = bitmap.Height;
            int bandTop = Math.Max(0, height - bandHeight);
            double frameSum = 0, bandSum = 0;
            long frameCount = 0, bandCount = 0, glyphs = 0, mark = 0;
            var accents = new long[Accents.Length];
            for (int y = 0; y < height; y += 3)
            {
                for (int x = 0; x < width; x += 3)
                {
                    Color pixel = bitmap.GetPixel(x, y);
                    double luma = 0.299 * pixel.R + 0.587 * pixel.G + 0.114 * pixel.B;
                    frameSum += luma; frameCount++;
                    if (y >= bandTop)
                    {
                        bandSum += luma; bandCount++;
                        if (luma > 180) glyphs++;
                        // The accent word mark: blue-dominant pixels in the band's left corner.
                        if (x < 140 && pixel.B > 120 && (pixel.B - pixel.R) > 40 && pixel.G > 80) mark++;
                    }
                    else
                    {
                        for (int i = 0; i < Accents.Length; i++)
                        {
                            if (Math.Abs(pixel.R - Accents[i][0]) < 26 && Math.Abs(pixel.G - Accents[i][1]) < 26
                                && Math.Abs(pixel.B - Accents[i][2]) < 26) accents[i]++;
                        }
                    }
                }
            }
            var builder = new StringBuilder();
            builder.AppendFormat("luma={0:F1};band={1:F1};glyphs={2:F2};mark={3};accents=",
                frameSum / Math.Max(1, frameCount), bandSum / Math.Max(1, bandCount),
                100.0 * glyphs / Math.Max(1, bandCount), mark);
            bool first = true;
            for (int i = 0; i < Accents.Length; i++)
            {
                if (accents[i] > 30)
                {
                    if (!first) builder.Append(',');
                    builder.AppendFormat("{0}:{1}", AccentNames[i], accents[i]);
                    first = false;
                }
            }
            return builder.ToString();
        }
    }

    // A coarse luminance map: enough to see the strip, the gaps and the caption band.
    public static string[] Ascii(string path, int columns, int rows)
    {
        const string chars = " .:-=+*#%@";
        using (var bitmap = new Bitmap(path))
        {
            var lines = new List<string>();
            int cellWidth = Math.Max(1, bitmap.Width / columns);
            int cellHeight = Math.Max(1, bitmap.Height / rows);
            for (int row = 0; row < rows; row++)
            {
                var line = new StringBuilder(columns);
                for (int column = 0; column < columns; column++)
                {
                    double sum = 0; int count = 0;
                    for (int y = row * cellHeight; y < Math.Min(bitmap.Height, (row + 1) * cellHeight); y += 3)
                    {
                        for (int x = column * cellWidth; x < Math.Min(bitmap.Width, (column + 1) * cellWidth); x += 3)
                        {
                            Color pixel = bitmap.GetPixel(x, y);
                            sum += 0.299 * pixel.R + 0.587 * pixel.G + 0.114 * pixel.B;
                            count++;
                        }
                    }
                    double luma = sum / Math.Max(1, count);
                    int index = Math.Min(chars.Length - 1, (int)(luma / 256.0 * chars.Length));
                    line.Append(chars[index]);
                }
                lines.Add(line.ToString());
            }
            return lines.ToArray();
        }
    }
}

// The caption band that ffmpeg lays over the recording: drawn here, as a transparent
// PNG, instead of with drawtext - this ffmpeg build has no fontconfig and its drawtext
// filter crashed on the full graph.  One band per segment per output width.
public static class CaptionBand
{
    public static void Render(string path, int width, int bandHeight, string caption, int fontSize, int markSize)
    {
        using (var bitmap = new Bitmap(width, bandHeight, System.Drawing.Imaging.PixelFormat.Format32bppArgb))
        {
            using (var graphics = Graphics.FromImage(bitmap))
            {
                graphics.SmoothingMode = SmoothingMode.AntiAlias;
                graphics.TextRenderingHint = System.Drawing.Text.TextRenderingHint.AntiAliasGridFit;
                graphics.Clear(Color.Transparent);
                using (var band = new SolidBrush(Color.FromArgb(184, 0x0A, 0x0E, 0x15)))
                    graphics.FillRectangle(band, 0, 0, width, bandHeight);
                using (var line = new SolidBrush(Color.FromArgb(230, 0x4C, 0x8D, 0xFF)))
                    graphics.FillRectangle(line, 0, 0, width, Math.Max(1, bandHeight / 36));
                float scale = bandHeight / 72f;
                using (var mark = new Font("Segoe UI Semibold", Math.Max(8f, markSize), FontStyle.Bold, GraphicsUnit.Pixel))
                using (var text = new Font("Segoe UI Semibold", Math.Max(10f, fontSize), FontStyle.Regular, GraphicsUnit.Pixel))
                using (var markBrush = new SolidBrush(Color.FromArgb(0x9C, 0xBE, 0xFF)))
                using (var textBrush = new SolidBrush(Color.White))
                {
                    var format = new StringFormat(StringFormat.GenericTypographic);
                    format.Alignment = StringAlignment.Near;
                    format.LineAlignment = StringAlignment.Center;
                    var markFormat = new StringFormat(format);
                    markFormat.Alignment = StringAlignment.Near;
                    float markLeft = 18f * scale;
                    graphics.DrawString("wiri", mark, markBrush, new RectangleF(markLeft, 0, width, bandHeight), markFormat);
                    // The caption is centred on the video, not in the space left of the mark.
                    format.Alignment = StringAlignment.Center;
                    graphics.DrawString(caption, text, textBrush, new RectangleF(0, 0, width, bandHeight), format);
                }
            }
            bitmap.Save(path, System.Drawing.Imaging.ImageFormat.Png);
        }
    }
}

// The Windows 11 shell draws its menus in a separate z-band: EnumWindows never returns
// them (the recorder's first versions missed a desktop context menu that was in every
// frame for exactly this reason), so ask the compositor instead - what window is under a
// grid of points across the work area, and what owns the foreground.
public static class PopupProbe
{
    // What a point can land on when a shell flyout is open
    static readonly string[] PointPopupClasses = new string[]
    {
        "Microsoft.UI.Content.PopupWindowSiteBridge",
        "Xaml_WindowedPopupClass",
        "#32768",
    };
    // What the foreground can be while a shell menu is open (the menu host itself is a
    // zero-sized XamlExplorerHostIslandWindow that owns the focus with keyboard keytips)
    static readonly string[] ForegroundPopupPrefixes = new string[]
    {
        "XamlExplorerHostIslandWindow",
        "Microsoft.UI.Content.PopupWindowSiteBridge",
        "Xaml_WindowedPopupClass",
        "#32768",
    };
    static readonly string[] DesktopClasses = new string[] { "Progman", "WorkerW", "SysListView32" };

    public static string Describe(IntPtr hwnd, string source)
    {
        DemoNative.RECT rect = DemoNative.Bounds(hwnd);
        long style = DemoNative.ExStyleOf(hwnd);
        return string.Format("{0}: hwnd=0x{1:X} class={2} title={3} pid={4} process={5} rect={6},{7} {8}x{9} exstyle=0x{10:X8}",
            source, hwnd.ToInt64(), DemoNative.ClassOf(hwnd), DemoNative.Title(hwnd), DemoNative.PidOf(hwnd),
            DemoNative.ProcessOf(hwnd), rect.Left, rect.Top, rect.Width, rect.Height, style);
    }

    static bool IsPointPopupClass(string cls)
    {
        foreach (string candidate in PointPopupClasses) if (cls == candidate) return true;
        return false;
    }

    // The window at one point, if it is a shell popup (or if the point is empty desktop).
    public static string HitAt(int x, int y)
    {
        var point = new DemoNative.POINT(); point.X = x; point.Y = y;
        IntPtr hwnd = DemoNative.WindowFromPoint(point);
        if (hwnd == IntPtr.Zero) return null;
        IntPtr root = DemoNative.GetAncestor(hwnd, 2);
        if (root == IntPtr.Zero) root = hwnd;
        string cls = DemoNative.ClassOf(root);
        if (IsPointPopupClass(cls) || DemoNative.Title(root) == "PopupHost")
            return Describe(root, string.Format("point({0},{1})", x, y));
        return null;
    }

    public static bool IsDesktopPoint(int x, int y)
    {
        var point = new DemoNative.POINT(); point.X = x; point.Y = y;
        IntPtr hwnd = DemoNative.WindowFromPoint(point);
        if (hwnd == IntPtr.Zero) return false;
        IntPtr root = DemoNative.GetAncestor(hwnd, 2);
        if (root == IntPtr.Zero) root = hwnd;
        string cls = DemoNative.ClassOf(root);
        foreach (string candidate in DesktopClasses) if (cls == candidate) return true;
        return false;
    }

    // What is at one point, whatever it is: used for the report line in the take log.
    public static string PointDescription(int x, int y)
    {
        var point = new DemoNative.POINT(); point.X = x; point.Y = y;
        IntPtr hwnd = DemoNative.WindowFromPoint(point);
        if (hwnd == IntPtr.Zero) return string.Format("({0},{1}) no window", x, y);
        IntPtr root = DemoNative.GetAncestor(hwnd, 2);
        if (root == IntPtr.Zero) root = hwnd;
        return Describe(root, string.Format("point({0},{1})", x, y));
    }

    // The foreground window, if it means an open shell menu.
    public static string ForegroundHit()
    {
        IntPtr hwnd = DemoNative.GetForegroundWindow();
        if (hwnd == IntPtr.Zero) return null;
        string cls = DemoNative.ClassOf(hwnd);
        string title = DemoNative.Title(hwnd);
        string process = DemoNative.ProcessOf(hwnd);
        DemoNative.RECT rect = DemoNative.Bounds(hwnd);
        bool emptyRect = rect.Width <= 0 || rect.Height <= 0;
        bool popupClass = false;
        foreach (string prefix in ForegroundPopupPrefixes)
            if (cls.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)) popupClass = true;
        if (title == "PopupHost") popupClass = true;
        if (popupClass || (process == "explorer" && emptyRect))
            return Describe(hwnd, string.Format("foreground (emptyRect={0})", emptyRect));
        return null;
    }

    // A grid of WindowFromPoint probes across the work area: catches flyouts that no
    // top-level enumeration can see.
    public static string[] ScanHits(int step)
    {
        var hits = new List<string>();
        var seen = new HashSet<long>();
        Rectangle area = Screen.PrimaryScreen.WorkingArea;
        for (int y = area.Top; y < area.Bottom; y += step)
        {
            for (int x = area.Left; x < area.Right; x += step)
            {
                var point = new DemoNative.POINT(); point.X = x; point.Y = y;
                IntPtr hwnd = DemoNative.WindowFromPoint(point);
                if (hwnd == IntPtr.Zero) continue;
                IntPtr root = DemoNative.GetAncestor(hwnd, 2);
                if (root == IntPtr.Zero) root = hwnd;
                long id = root.ToInt64();
                if (seen.Contains(id)) continue;
                string cls = DemoNative.ClassOf(root);
                if (!(IsPointPopupClass(cls) || DemoNative.Title(root) == "PopupHost")) continue;
                seen.Add(id);
                hits.Add(Describe(root, string.Format("point({0},{1})", x, y)));
            }
        }
        return hits.ToArray();
    }
}

// A window that looks like a small, deliberate application.  One form per UI thread,// double buffered, repainted on resize: wiri animates these windows around all day.
sealed class DemoForm : Form
{
    static readonly Color Bg = Color.FromArgb(0x0F, 0x12, 0x17);
    static readonly Color Panel = Color.FromArgb(0x15, 0x1A, 0x22);
    static readonly Color Line = Color.FromArgb(0x25, 0x2C, 0x38);
    static readonly Color Ink = Color.FromArgb(0xE4, 0xE8, 0xEF);
    static readonly Color Sub = Color.FromArgb(0x8B, 0x95, 0xA7);
    static readonly Color Dim = Color.FromArgb(0x5E, 0x68, 0x7A);
    static readonly Color Amber = Color.FromArgb(0xE0, 0xAF, 0x68);
    static readonly Color Green = Color.FromArgb(0x9E, 0xCE, 0x6A);
    static readonly Color Blue = Color.FromArgb(0x7A, 0xA2, 0xF7);
    static readonly Color Cyan = Color.FromArgb(0x7D, 0xCF, 0xFF);
    static readonly Color Pink = Color.FromArgb(0xF7, 0x76, 0x8E);
    static readonly Color Teal = Color.FromArgb(0x73, 0xDA, 0xCA);
    static readonly Color Purple = Color.FromArgb(0xBB, 0x9A, 0xF7);

    readonly string kind;
    readonly Color accent;
    readonly string appName;
    readonly string subtitle;
    readonly Font titleFont = new Font("Segoe UI Semibold", 12.5f, FontStyle.Regular, GraphicsUnit.Point);
    readonly Font headFont = new Font("Segoe UI Semibold", 15f, FontStyle.Regular, GraphicsUnit.Point);
    readonly Font bodyFont = new Font("Segoe UI", 9.5f, FontStyle.Regular, GraphicsUnit.Point);
    readonly Font smallFont = new Font("Segoe UI", 8.25f, FontStyle.Regular, GraphicsUnit.Point);
    readonly Font monoFont = new Font("Consolas", 9.5f, FontStyle.Regular, GraphicsUnit.Point);
    readonly Font monoSmall = new Font("Consolas", 8.25f, FontStyle.Regular, GraphicsUnit.Point);

    public DemoForm(string kind, string caption)
    {
        this.kind = kind;
        Text = caption;
        Size = new Size(760, 620);
        MinimumSize = new Size(220, 140);
        StartPosition = FormStartPosition.Manual;
        Location = new Point(90, 70);
        BackColor = Bg;
        ForeColor = Ink;
        DoubleBuffered = true;
        SetStyle(ControlStyles.AllPaintingInWmPaint | ControlStyles.OptimizedDoubleBuffer
                 | ControlStyles.UserPaint | ControlStyles.ResizeRedraw, true);
        switch (kind)
        {
            case "editor":   accent = Blue;   appName = "Editor";   subtitle = "notes.md"; break;
            case "terminal": accent = Green;  appName = "Terminal"; subtitle = "pwsh"; break;
            case "notes":    accent = Amber;  appName = "Notes";    subtitle = "checklist.md"; break;
            case "browser":  accent = Purple; appName = "Browser";  subtitle = "wiri.local/docs"; break;
            case "files":    accent = Cyan;   appName = "Files";    subtitle = "C:\\wiri"; break;
            case "chat":     accent = Pink;   appName = "Chat";     subtitle = "#wiri"; break;
            case "music":    accent = Teal;   appName = "Music";    subtitle = "focus"; break;
            default:         accent = Blue;   appName = kind;       subtitle = ""; break;
        }
    }

    protected override void OnHandleCreated(EventArgs e)
    {
        base.OnHandleCreated(e);
        // A dark caption to match the content: DWMWA_USE_IMMERSIVE_DARK_MODE is 20 on
        // Windows 11 and 19 on Windows 10 1809+.
        int on = 1;
        if (DemoNative.DwmSetInt(Handle, 20, ref on, 4) != 0) { on = 1; DemoNative.DwmSetInt(Handle, 19, ref on, 4); }
    }

    protected override void OnPaint(PaintEventArgs e)
    {
        Graphics g = e.Graphics;
        g.SmoothingMode = SmoothingMode.AntiAlias;
        g.TextRenderingHint = System.Drawing.Text.TextRenderingHint.ClearTypeGridFit;
        Rectangle b = ClientRectangle;
        using (var brush = new SolidBrush(Bg)) g.FillRectangle(brush, b);
        using (var brush = new SolidBrush(accent)) g.FillRectangle(brush, 0, 0, b.Width, 3);
        DrawHeader(g, b);
        Rectangle body = new Rectangle(18, 62, Math.Max(10, b.Width - 36), Math.Max(10, b.Height - 62 - 14));
        switch (kind)
        {
            case "editor": DrawEditor(g, body); break;
            case "terminal": DrawTerminal(g, body); break;
            case "notes": DrawNotes(g, body); break;
            case "browser": DrawBrowser(g, body); break;
            case "files": DrawFiles(g, body); break;
            case "chat": DrawChat(g, body); break;
            case "music": DrawMusic(g, body); break;
        }
    }

    void DrawHeader(Graphics g, Rectangle b)
    {
        using (var brush = new SolidBrush(accent)) FillRound(g, brush, new Rectangle(18, 17, 9, 9), 4);
        using (var brush = new SolidBrush(Ink)) g.DrawString(appName, titleFont, brush, 34, 12);
        if (subtitle.Length > 0)
        {
            SizeF size = g.MeasureString(subtitle, smallFont);
            using (var brush = new SolidBrush(Dim))
                g.DrawString(subtitle, smallFont, brush, b.Width - 18 - size.Width, 18);
        }
        using (var pen = new Pen(Line)) g.DrawLine(pen, 18, 44, b.Width - 18, 44);
    }

    // --- the application bodies -------------------------------------------------

    void DrawEditor(Graphics g, Rectangle area)
    {
        string[][] lines = new string[][]
        {
            new string[] { "1",  "// wiri: windows are columns on an endless strip.", "c" },
            new string[] { "2",  "", "" },
            new string[] { "3",  "layout {", "k" },
            new string[] { "4",  "    gaps 16", "p" },
            new string[] { "5",  "    max-visible-columns 3", "p" },
            new string[] { "6",  "    column-width 620", "p" },
            new string[] { "7",  "}", "k" },
            new string[] { "8",  "", "" },
            new string[] { "9",  "binds {", "k" },
            new string[] { "10", "    Mod+Left  { focus-column-left; }", "p" },
            new string[] { "11", "    Mod+Right { focus-column-right; }", "p" },
            new string[] { "12", "    Mod+Comma { consume-window-into-column; }", "p" },
            new string[] { "13", "}", "k" },
            new string[] { "14", "", "" },
            new string[] { "15", "// opening a window never resizes the others.", "c" },
        };
        int y = area.Top;
        using (var gutter = new SolidBrush(Dim)) using (var text = new SolidBrush(Ink))
        using (var comment = new SolidBrush(Color.FromArgb(0x6A, 0x86, 0x7A)))
        using (var keyword = new SolidBrush(Blue))
        {
            foreach (string[] line in lines)
            {
                if (y + 18 > area.Bottom) break;
                g.DrawString(line[0], monoSmall, gutter, area.Left, y + 2);
                Brush brush = line[2] == "c" ? comment : (line[2] == "k" ? keyword : text);
                g.DrawString(line[1], monoFont, brush, area.Left + 26, y);
                y += 25;
            }
        }
        // The caret, so the window reads as a text editor rather than a label.
        int caretY = area.Top + 3 * 25;
        using (var pen = new Pen(accent, 1.6f)) g.DrawLine(pen, area.Left + 26, caretY + 2, area.Left + 26, caretY + 17);
    }

    void DrawTerminal(Graphics g, Rectangle area)
    {
        var lines = new List<object[]>
        {
            new object[] { "ps", "PS C:\\wiri> ", "cargo build --release" },
            new object[] { "out", "   Compiling wiri v0.1.0 (C:\\wiri)", "" },
            new object[] { "ok",  "    Finished `release` profile in 24.1s", "" },
            new object[] { "ps", "PS C:\\wiri> ", "wiri-ctl state" },
            new object[] { "out", "outputs                1", "" },
            new object[] { "out", "workspaces             2", "" },
            new object[] { "out", "columns                4", "" },
            new object[] { "out", "focused window         Terminal", "" },
            new object[] { "ps", "PS C:\\wiri> ", "_" },
        };
        int y = area.Top;
        using (var prompt = new SolidBrush(Green)) using (var cmd = new SolidBrush(Ink))
        using (var output = new SolidBrush(Sub)) using (var ok = new SolidBrush(Cyan))
        {
            foreach (object[] line in lines)
            {
                if (y + 18 > area.Bottom) break;
                string tag = (string)line[0];
                Brush brush = tag == "ps" ? cmd : (tag == "ok" ? ok : output);
                float x = area.Left;
                if (tag == "ps")
                {
                    g.DrawString((string)line[1], monoFont, prompt, x, y);
                    x += g.MeasureString((string)line[1], monoFont).Width;
                    g.DrawString((string)line[2], monoFont, cmd, x, y);
                }
                else
                {
                    g.DrawString((string)line[1], monoFont, brush, x, y);
                }
                y += 25;
            }
        }
    }

    void DrawNotes(Graphics g, Rectangle area)
    {
        int y = area.Top;
        using (var brush = new SolidBrush(Ink)) { g.DrawString("Demo checklist", headFont, brush, area.Left, y); y += 38; }
        string[][] items = new string[][]
        {
            new string[] { "1", "Columns scroll, nothing resizes" },
            new string[] { "1", "Tabs share a single column" },
            new string[] { "1", "Workspaces stack vertically" },
            new string[] { "0", "Record the README video" },
        };
        using (var check = new Pen(accent, 1.8f))
        using (var box = new Pen(Dim, 1.2f))
        using (var brush = new SolidBrush(Ink))
        using (var done = new SolidBrush(Color.FromArgb(0x93, 0x9C, 0xAD)))
        {
            foreach (string[] item in items)
            {
                if (y + 24 > area.Bottom) break;
                var square = new Rectangle(area.Left, y + 3, 13, 13);
                if (item[0] == "1")
                {
                    using (var fill = new SolidBrush(Color.FromArgb(0x2A, 0x3A, 0x2E))) FillRound(g, fill, square, 3);
                    using (var pen = new Pen(accent, 1.8f)) g.DrawRectangle(pen, square);
                    g.DrawLine(check, square.Left + 3, square.Top + 7, square.Left + 6, square.Top + 10);
                    g.DrawLine(check, square.Left + 6, square.Top + 10, square.Left + 11, square.Top + 3);
                    g.DrawString(item[1], bodyFont, done, area.Left + 24, y);
                }
                else
                {
                    g.DrawRectangle(box, square);
                    g.DrawString(item[1], bodyFont, brush, area.Left + 24, y);
                }
                y += 30;
            }
        }
        y += 12;
        using (var brush = new SolidBrush(Dim))
            g.DrawString("Everything above is one KDL config file.", smallFont, brush, area.Left, y);
    }

    void DrawBrowser(Graphics g, Rectangle area)
    {
        var bar = new Rectangle(area.Left, area.Top, area.Width, 30);
        using (var brush = new SolidBrush(Panel)) FillRound(g, brush, bar, 15);
        using (var brush = new SolidBrush(Dim)) FillRound(g, brush, new Rectangle(bar.Left + 12, bar.Top + 10, 10, 10), 5);
        using (var brush = new SolidBrush(Sub)) g.DrawString("wiri.local/docs", smallFont, brush, bar.Left + 30, bar.Top + 7);

        int y = area.Top + 46;
        using (var brush = new SolidBrush(Ink)) { g.DrawString("Scrollable tiling", headFont, brush, area.Left, y); y += 30; }
        using (var brush = new SolidBrush(Ink)) { g.DrawString("for Windows", headFont, brush, area.Left, y); y += 42; }
        string[] copy = new string[]
        {
            "Windows are columns on an endless strip.",
            "Open one and the others keep their size.",
            "Scroll to the ones off screen.",
        };
        using (var brush = new SolidBrush(Sub))
            foreach (string line in copy)
            {
                if (y + 18 > area.Bottom) break;
                g.DrawString(line, bodyFont, brush, area.Left, y);
                y += 22;
            }
        y += 14;
        // Three feature chips.
        string[] chips = new string[] { "Columns", "Tabs", "Overview" };
        float x = area.Left;
        using (var brush = new SolidBrush(Panel))
        using (var pen = new Pen(Line))
        using (var ink = new SolidBrush(Ink))
        {
            foreach (string chip in chips)
            {
                SizeF size = g.MeasureString(chip, smallFont);
                var chipRect = new Rectangle((int)x, y, (int)size.Width + 26, 28);
                FillRound(g, brush, chipRect, 14);
                g.DrawPath(pen, RoundRect(chipRect, 14));
                g.DrawString(chip, smallFont, ink, chipRect.Left + 13, chipRect.Top + 7);
                x += chipRect.Width + 10;
            }
        }
    }

    void DrawFiles(Graphics g, Rectangle area)
    {
        int y = area.Top;
        string[][] rows = new string[][]
        {
            new string[] { "d", "src", "142 files" },
            new string[] { "d", "docs", "38 files" },
            new string[] { "d", "examples", "6 files" },
            new string[] { "d", "resources", "3 files" },
            new string[] { "f", "README.md", "9.4 kB" },
            new string[] { "f", "Cargo.toml", "1.2 kB" },
            new string[] { "f", "LICENSE", "1.1 kB" },
        };
        using (var folder = new SolidBrush(accent))
        using (var file = new SolidBrush(Dim))
        using (var name = new SolidBrush(Ink))
        using (var size = new SolidBrush(Dim))
        using (var pen = new Pen(Line))
        {
            foreach (string[] row in rows)
            {
                if (y + 30 > area.Bottom) break;
                if (row[0] == "d") FillRound(g, folder, new Rectangle(area.Left + 2, y + 6, 18, 13), 3);
                else FillRound(g, file, new Rectangle(area.Left + 4, y + 4, 13, 17), 2);
                g.DrawString(row[1], bodyFont, name, area.Left + 34, y + 2);
                SizeF w = g.MeasureString(row[2], smallFont);
                g.DrawString(row[2], smallFont, size, area.Right - w.Width, y + 5);
                y += 31;
                g.DrawLine(pen, area.Left, y - 6, area.Right, y - 6);
            }
        }
    }

    void DrawChat(Graphics g, Rectangle area)
    {
        string[][] bubbles = new string[][]
        {
            new string[] { "l", "opening a window never resizes the others" },
            new string[] { "r", "so the strip just keeps going" },
            new string[] { "l", "gaps 16, rounded corners, focus ring" },
            new string[] { "r", "and the overview shows all of it" },
        };
        int y = area.Top;
        using (var left = new SolidBrush(Panel))
        using (var right = new SolidBrush(Color.FromArgb(0x22, 0x33, 0x52)))
        using (var ink = new SolidBrush(Ink))
        using (var pen = new Pen(Line))
        {
            foreach (string[] bubble in bubbles)
            {
                if (y + 40 > area.Bottom) break;
                SizeF size = g.MeasureString(bubble[1], bodyFont);
                int width = Math.Min(area.Width - 24, (int)size.Width + 28);
                var rect = new Rectangle(bubble[0] == "l" ? area.Left : area.Right - width, y, width, (int)size.Height + 18);
                FillRound(g, bubble[0] == "l" ? left : right, rect, 10);
                g.DrawPath(pen, RoundRect(rect, 10));
                g.DrawString(bubble[1], bodyFont, ink, rect.Left + 14, rect.Top + 8);
                y += rect.Height + 12;
            }
        }
    }

    void DrawMusic(Graphics g, Rectangle area)
    {
        var art = new Rectangle(area.Left, area.Top, Math.Min(96, area.Width - 8), Math.Min(96, area.Width - 8));
        using (var brush = new LinearGradientBrush(art, accent, Color.FromArgb(0x2B, 0x33, 0x46), 45f))
            FillRound(g, brush, art, 8);
        using (var brush = new SolidBrush(Bg)) FillRound(g, brush, new Rectangle(art.Left + art.Width / 2 - 11, art.Top + art.Height / 2 - 11, 22, 22), 11);
        using (var brush = new SolidBrush(accent))
        {
            Point[] triangle = new Point[]
            {
                new Point(art.Left + art.Width / 2 - 4, art.Top + art.Height / 2 - 6),
                new Point(art.Left + art.Width / 2 - 4, art.Top + art.Height / 2 + 6),
                new Point(art.Left + art.Width / 2 + 7, art.Top + art.Height / 2),
            };
            g.FillPolygon(brush, triangle);
        }
        int y = art.Bottom + 16;
        using (var brush = new SolidBrush(Ink)) g.DrawString("Focus", headFont, brush, area.Left, y);
        y += 30;
        using (var brush = new SolidBrush(Sub)) g.DrawString("wiri demo playlist · 7 tracks", smallFont, brush, area.Left, y);
        y += 24;
        string[][] tracks = new string[][]
        {
            new string[] { "01", "Columns on a strip", "3:12" },
            new string[] { "02", "Gaps and rings", "2:48" },
            new string[] { "03", "Vertical stack", "4:05" },
            new string[] { "04", "Diagonal grid", "3:31" },
        };
        using (var index = new SolidBrush(accent))
        using (var name = new SolidBrush(Ink))
        using (var time = new SolidBrush(Dim))
        using (var pen = new Pen(Line))
        {
            foreach (string[] track in tracks)
            {
                if (y + 26 > area.Bottom) break;
                g.DrawString(track[0], monoSmall, index, area.Left, y + 3);
                g.DrawString(track[1], bodyFont, name, area.Left + 30, y);
                SizeF w = g.MeasureString(track[2], smallFont);
                g.DrawString(track[2], smallFont, time, area.Right - w.Width, y + 3);
                y += 28;
                g.DrawLine(pen, area.Left, y - 6, area.Right, y - 6);
            }
        }
    }

    // --- drawing helpers --------------------------------------------------------

    static GraphicsPath RoundRect(Rectangle r, int radius)
    {
        int d = Math.Max(1, Math.Min(radius * 2, Math.Min(r.Width, r.Height)));
        var path = new GraphicsPath();
        path.AddArc(r.Left, r.Top, d, d, 180, 90);
        path.AddArc(r.Right - d, r.Top, d, d, 270, 90);
        path.AddArc(r.Right - d, r.Bottom - d, d, d, 0, 90);
        path.AddArc(r.Left, r.Bottom - d, d, d, 90, 90);
        path.CloseFigure();
        return path;
    }

    static void FillRound(Graphics g, Brush brush, Rectangle r, int radius)
    {
        if (r.Width <= 1 || r.Height <= 1) return;
        using (GraphicsPath path = RoundRect(r, radius)) g.FillPath(brush, path);
    }
}
