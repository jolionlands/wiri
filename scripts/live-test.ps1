# wiri live smoke test for tridaris. Runs in the interactive session (scheduled task /it).
#
# Phase A  three plain WinForms windows (created inside this process, no extra consoles),
#          driven over the wiri IPC CLI; positions, focus and screenshots recorded.
# Phase B  real terminal windows (a visible console per cmd.exe = a Windows Terminal window with
#          its XAML island child) - the situation that made wiri thrash.
# Phase C  crash safety: switch to workspace 2 (hides the windows), kill wiri by PID, check the
#          windows are still hidden, start wiri again, check they come back.
# Phase G  real key presses: Escape reaches applications, Escape leaves overview without freezing wiri,
#          Ctrl+Alt+2 / Ctrl+Alt+1 hide and show the windows.
# Phase H  Phase 1 scenarios, each under a config of its own (wiri is restarted for every one):
#          H1 modifiers, H2 spawn, H3 actions follow the real foreground window (floats, dialogs),
#          H4 minimized windows leave no gap, H5 floating windows belong to their workspace,
#          H6 the niri column keymap (consume/expel, move-window, first/last, maximize-column), pressed
#          with the shipped default binds, H7 niri's action names and argument grammar (set-column-width
#          "50%" / "+10%" / "600", presets, focus-workspace 2 against focus-workspace "2", set-window-height),
#          H8 the workspace model (named workspaces, index addressing, previous, back and forth, no wrap),
#          H9 the Phase 0 leftovers (default hotkeys that register, an exact fullscreen, a terminal's minimum width,
#          windows put back where they were after a crash), H11 hide-method "cloak" (the windows of another
#          workspace stay visible to the system but are not drawn; Alt+Tab to one brings its workspace; a crash,
#          a restart and a quit leave nothing cloaked).  (H10, monitor unplug, needs a second display: engine unit
#          tests only.)
#          H12 the KDL config (Phase 2A item 1): a niri binds block with properties loads as written and its binds
#          run (real chords: cooldown-ms=, repeat=false), validate-config names line and column, a bad reload keeps the
#          last good config and reports it (log, config_loaded event, tray balloon), a half-written save is applied once,
#          a BOM or UTF-16 file loads, an include in another directory is read and watched, a bad config at startup
#          falls back to the defaults and says why.
#          H13 window rules that act (Phase 2A item 2): default-column-width / default-window-height, open-maximized,
#          open-floating, open-fullscreen, open-on-workspace (by name) / open-in-workspace, open-on-output (a real name and one
#          that is not there), open-focused false, min/max size limits, opacity, `app-id` regexes (process name), several
#          `match` lines and an `exclude` line, rules that follow the title and the focus of a window while it lives, and a
#          config reload that reaches the windows that are open.
# Phase D  close one window, `wiri-ctl quit`, check every window is visible again.
#
# Outputs (all in C:\wiri-dev\live): summary.json, per-step *.txt / *.json, *.png screenshots,
# wiri.stderr.log (+ wiri.stderr2.log for the restarted instance).
param([string]$Bin = 'C:\wiri-dev\wiri\target\release')

$ErrorActionPreference = 'Continue'
$D = 'C:\wiri-dev\live'
if (Test-Path $D) { Remove-Item -Recurse -Force $D }
New-Item -ItemType Directory -Force $D | Out-Null
$wiri = Join-Path $Bin 'wiri.exe'
$ctl = Join-Path $Bin 'wiri-ctl.exe'
$steps = New-Object System.Collections.Generic.List[object]
$log1 = Join-Path $D 'wiri.stderr.log'
$log2 = Join-Path $D 'wiri.stderr2.log'

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;

public static class LT {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern void SwitchToThisWindow(IntPtr h, bool altTab);
  [DllImport("dwmapi.dll", EntryPoint = "DwmGetWindowAttribute")] static extern int DwmGetUInt(IntPtr h, int attr, out uint value, int size);
  // DWMWA_CLOAKED: nonzero while something (the application, the shell, the owner) keeps DWM from drawing the window;
  // -1 when DWM does not answer.
  public static int Cloaked(IntPtr h) { uint v; int hr = DwmGetUInt(h, 14, out v, 4); return hr == 0 ? (int)v : -1; }
  // How many pixels of a screenshot have exactly this colour (a window of a plain colour that is on the screen).
  public static int CountColor(string path, int r, int g, int b) {
    using (var bmp = new System.Drawing.Bitmap(path)) {
      var rect = new System.Drawing.Rectangle(0, 0, bmp.Width, bmp.Height);
      var data = bmp.LockBits(rect, System.Drawing.Imaging.ImageLockMode.ReadOnly, System.Drawing.Imaging.PixelFormat.Format32bppArgb);
      int count = 0;
      try {
        int stride = data.Stride;
        byte[] row = new byte[stride];
        for (int y = 0; y < bmp.Height; y++) {
          Marshal.Copy(IntPtr.Add(data.Scan0, y * stride), row, 0, stride);
          for (int x = 0; x < bmp.Width; x++) {
            int i = x * 4;
            if (row[i] == b && row[i + 1] == g && row[i + 2] == r) { count++; }
          }
        }
      } finally { bmp.UnlockBits(data); }
      return count;
    }
  }
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
  // The top-level window that is on top at a screen point (what the user sees there).
  public static IntPtr TopWindowAt(int x, int y) {
    POINT p; p.X = x; p.Y = y;
    IntPtr h = WindowFromPoint(p);
    return h == IntPtr.Zero ? h : GetAncestor(h, 2);
  }

  public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
  public static string Class(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
  public static uint Pid(IntPtr h) { uint p; GetWindowThreadProcessId(h, out p); return p; }
  public static List<IntPtr> TopLevel() {
    var r = new List<IntPtr>();
    EnumWindows(delegate(IntPtr h, IntPtr l) { r.Add(h); return true; }, IntPtr.Zero);
    return r;
  }
  // Visible bounds (what the user sees, without the invisible resize border).
  public static RECT Visible(IntPtr h) { RECT r; if (DwmGetWindowAttribute(h, 9, out r, 16) != 0) { GetWindowRect(h, out r); } return r; }
  public static void Close(IntPtr h) { PostMessage(h, 0x0010, IntPtr.Zero, IntPtr.Zero); }
  [DllImport("user32.dll", EntryPoint = "GetWindowLongW")] static extern int GetLongValue(IntPtr h, int index);
  [DllImport("user32.dll")] static extern bool GetLayeredWindowAttributes(IntPtr h, out uint key, out byte alpha, out uint flags);
  // The alpha (0 to 255) the window is drawn with: 255 for a window that is not layered, -1 when it cannot be read.
  public static int Alpha(IntPtr h) {
    if ((GetLongValue(h, -20) & 0x80000) == 0) { return 255; }
    uint key; byte alpha; uint flags;
    if (!GetLayeredWindowAttributes(h, out key, out alpha, out flags)) { return -1; }
    return (flags & 2) != 0 ? (int)alpha : 255;
  }
  public static void Key(byte vk, bool up) {
    uint f = 0;
    if ((vk >= 0x21 && vk <= 0x28) || vk == 0x2D || vk == 0x2E) f |= 1;
    if (up) f |= 2;
    keybd_event(vk, 0, f, UIntPtr.Zero);
  }
}

// The shell's view of a window (IApplicationView): what Alt+Tab and the taskbar are made from.  The interface definitions
// are the ones wiri uses for hide-method "cloak" (komorebi's, taken from AltTabAccessor, MIT); a method is only called when
// this script needs it, the others are there for the vtable layout.
public static class ShellViews {
  [ComImport, Guid("6D5140C1-7436-11CE-8034-00AA006009FA"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IServiceProviderX {
    [PreserveSig] int QueryService(ref Guid service, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object obj);
  }
  [ComImport, Guid("1841C6D7-4F9D-42C0-AF41-8747538F10E5"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IApplicationViewCollection {
    [PreserveSig] int GetViews(out IntPtr views);
    [PreserveSig] int GetViewsByZOrder(out IntPtr views);
    [PreserveSig] int GetViewsByAppUserModelId([MarshalAs(UnmanagedType.LPWStr)] string id, out IntPtr views);
    [PreserveSig] int GetViewForHwnd(IntPtr hwnd, [MarshalAs(UnmanagedType.Interface)] out IApplicationView view);
  }
  [ComImport, Guid("372E1D3B-38D3-42E4-A15B-8AB2B178F513"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
  interface IApplicationView {
    [PreserveSig] int GetIids(out int count, out IntPtr iids);
    [PreserveSig] int GetRuntimeClassName(out IntPtr name);
    [PreserveSig] int GetTrustLevel(out int level);
    [PreserveSig] int SetFocus();
    [PreserveSig] int SwitchTo();
    [PreserveSig] int TryInvokeBack(IntPtr callback);
    [PreserveSig] int GetThumbnailWindow(out IntPtr hwnd);
    [PreserveSig] int GetMonitor(out IntPtr monitor);
    [PreserveSig] int GetVisibility(out int visibility);
    [PreserveSig] int SetCloak(int cloakType, int flags);
    [PreserveSig] int GetPosition(ref Guid iid, out IntPtr position);
    [PreserveSig] int SetPosition(IntPtr position);
    [PreserveSig] int InsertAfterWindow(IntPtr hwnd);
    [PreserveSig] int GetExtendedFramePosition(out LT.RECT rect);
    [PreserveSig] int GetAppUserModelId(out IntPtr id);
    [PreserveSig] int SetAppUserModelId([MarshalAs(UnmanagedType.LPWStr)] string id);
    [PreserveSig] int IsEqualByAppUserModelId([MarshalAs(UnmanagedType.LPWStr)] string id, out int result);
    [PreserveSig] int GetViewState(out uint state);
    [PreserveSig] int SetViewState(uint state);
    [PreserveSig] int GetNeediness(out int neediness);
    [PreserveSig] int GetLastActivationTimestamp(out ulong timestamp);
    [PreserveSig] int SetLastActivationTimestamp(ulong timestamp);
    [PreserveSig] int GetVirtualDesktopId(out Guid id);
    [PreserveSig] int SetVirtualDesktopId(ref Guid id);
    [PreserveSig] int GetShowInSwitchers(out int show);
  }
  static IApplicationView View(IntPtr hwnd) {
    var shell = (IServiceProviderX)Activator.CreateInstance(Type.GetTypeFromCLSID(new Guid("C2F03A33-21F5-47FA-B4BB-156362A2F239")));
    Guid service = new Guid("1841C6D7-4F9D-42C0-AF41-8747538F10E5");
    Guid riid = service;
    object collection;
    if (shell.QueryService(ref service, ref riid, out collection) != 0) { return null; }
    IApplicationView view;
    if (((IApplicationViewCollection)collection).GetViewForHwnd(hwnd, out view) != 0) { return null; }
    return view;
  }
  // The shell's call for "the user picked this window in Alt+Tab" (or on the taskbar).  0 = done, -1 = the shell has no
  // view of the window, -3 = something threw.
  public static int SwitchTo(IntPtr hwnd) {
    try { var v = View(hwnd); return v == null ? -1 : v.SwitchTo(); } catch (Exception) { return -3; }
  }
  // 1 when the shell lists the window in Alt+Tab and on the taskbar, 0 when it does not; -1 = no view of the window,
  // -2 = the call failed, -3 = something threw.
  public static int InSwitchers(IntPtr hwnd) {
    try {
      var v = View(hwnd);
      if (v == null) { return -1; }
      int show;
      return v.GetShowInSwitchers(out show) == 0 ? show : -2;
    } catch (Exception) { return -3; }
  }
}

// Plain test windows living in this process, one STA thread each.
public static class TestForms {
  static readonly List<Form> forms = new List<Form>();
  public static readonly Dictionary<string, LT.RECT> Initial = new Dictionary<string, LT.RECT>();
  static readonly StringBuilder keys = new StringBuilder();
  public static string KeyLog() { lock (keys) { return keys.ToString(); } }
  public static void ClearKeys() { lock (keys) { keys.Length = 0; } }
  // A window that records every key it is given (does a plain Escape reach an application?).
  public static IntPtr OpenKeys(string title) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    var t = new Thread(delegate() {
      var f = new Form();
      f.Text = title; f.Width = 520; f.Height = 380;
      f.BackColor = Color.FromArgb(90, 90, 90);
      f.KeyPreview = true;
      f.KeyDown += delegate(object s2, KeyEventArgs e2) { lock (keys) { keys.Append(e2.KeyCode.ToString()).Append(','); } };
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
  public static IntPtr Open(string title, int r, int g, int b) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    var t = new Thread(delegate() {
      var f = new Form();
      f.Text = title; f.Width = 520; f.Height = 380;
      f.BackColor = Color.FromArgb(r, g, b);
      f.StartPosition = FormStartPosition.WindowsDefaultLocation;
      f.Shown += delegate(object s, EventArgs e) {
        handle = f.Handle;
        LT.RECT rc; LT.GetWindowRect(f.Handle, out rc); lock (Initial) { Initial[title] = rc; }
        ready.Set();
      };
      lock (forms) { forms.Add(f); }
      Application.Run(f);
    });
    t.SetApartmentState(ApartmentState.STA);
    t.IsBackground = true;
    t.Start();
    ready.WaitOne(15000);
    return handle;
  }
  // A window whose UI thread stops answering for hangMs right after it is shown: what a hung
  // application looks like to the window manager.
  public static IntPtr OpenHung(string title, int hangMs) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    var t = new Thread(delegate() {
      var f = new Form();
      f.Text = title; f.Width = 520; f.Height = 380;
      f.BackColor = Color.FromArgb(200, 40, 40);
      f.StartPosition = FormStartPosition.WindowsDefaultLocation;
      f.Shown += delegate(object s, EventArgs e) {
        handle = f.Handle;
        ready.Set();
        Thread.Sleep(hangMs);
      };
      lock (forms) { forms.Add(f); }
      Application.Run(f);
    });
    t.SetApartmentState(ApartmentState.STA);
    t.IsBackground = true;
    t.Start();
    ready.WaitOne(15000);
    return handle;
  }
  // A tool window (WS_EX_TOOLWINDOW): a window wiri does not manage.
  public static IntPtr OpenTool(string title) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    var t = new Thread(delegate() {
      var f = new Form();
      f.Text = title; f.Width = 520; f.Height = 380;
      f.FormBorderStyle = FormBorderStyle.FixedToolWindow;
      f.ShowInTaskbar = false;
      f.BackColor = Color.FromArgb(170, 130, 20);
      f.StartPosition = FormStartPosition.Manual;
      f.Location = new Point(200, 120);
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
  // A dialog owned by the form `owner` (an owned top-level window, like an "Open file" box).
  public static IntPtr OpenOwned(IntPtr owner, string title) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    Form ownerForm = null;
    lock (forms) { foreach (var f in forms) { if (f.IsHandleCreated && f.Handle == owner) { ownerForm = f; } } }
    if (ownerForm == null) { return IntPtr.Zero; }
    ownerForm.BeginInvoke((MethodInvoker)delegate() {
      var d = new Form();
      d.Text = title; d.Width = 300; d.Height = 200;
      d.ShowInTaskbar = false;
      d.StartPosition = FormStartPosition.CenterParent;
      d.BackColor = Color.FromArgb(200, 200, 200);
      d.Shown += delegate(object s, EventArgs e) { handle = d.Handle; ready.Set(); };
      lock (forms) { forms.Add(d); }
      d.Show(ownerForm);
    });
    ready.WaitOne(15000);
    return handle;
  }
  public static void CloseForm(IntPtr h) {
    lock (forms) {
      foreach (var f in forms) {
        if (f.IsHandleCreated && f.Handle == h) { var ff = f; ff.BeginInvoke((MethodInvoker)delegate() { ff.Close(); }); }
      }
    }
  }
  // The window's own thread changes its title, like a browser tab or a terminal does.
  public static void SetTitle(IntPtr h, string title) {
    lock (forms) {
      foreach (var f in forms) {
        if (f.IsHandleCreated && f.Handle == h) { var ff = f; ff.BeginInvoke((MethodInvoker)delegate() { ff.Text = title; }); }
      }
    }
  }
}
'@

function Shot([string]$name) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $D "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

# Every wiri-ctl call gets 5 s; a hung IPC call is recorded as rc=timeout instead of blocking the run.
function Ctl([string]$name, [string[]]$ctlArgs) {
    $outFile = Join-Path $D "$name.txt"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process -FilePath $ctl -ArgumentList $ctlArgs -RedirectStandardOutput $outFile -RedirectStandardError "$outFile.err" -WindowStyle Hidden -PassThru
    $null = $p.Handle    # keep the process handle open, otherwise ExitCode comes back empty
    if ($p.WaitForExit(5000)) { $rc = $p.ExitCode } else { Stop-Process -Id $p.Id -Force; $rc = 'timeout' }
    $steps.Add([pscustomobject]@{ step = $name; args = ($ctlArgs -join ' '); rc = $rc; ms = $sw.ElapsedMilliseconds })
    return (Get-Content $outFile -Raw -ErrorAction SilentlyContinue)
}

# ---- log analysis ---------------------------------------------------------------------------
function LogLines([string]$path, [string]$needle) {
    if (-not (Test-Path $path)) { return @() }
    return @(Select-String -Path $path -Pattern $needle -SimpleMatch -ErrorAction SilentlyContinue)
}
function PositioningLines([string]$path) { (LogLines $path 'Positioning window').Count }
# Positioning lines per one-second bucket (timestamps are UTC, second resolution is enough).
function Buckets([string]$path, [string]$needle) {
    $b = @{}
    foreach ($l in (LogLines $path $needle)) {
        $m = [regex]::Match($l.Line, '(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)\.\d+Z')
        if ($m.Success) { $b[$m.Groups[1].Value] = 1 + [int]$b[$m.Groups[1].Value] }
    }
    return $b
}
function MaxPerSecond($buckets, [int]$skipFirst) {
    # Skip the first seconds of the log: the startup tiling pass is a legitimate burst.
    $keys = @($buckets.Keys | Sort-Object)
    if ($keys.Count -eq 0) { return 0 }
    $max = 0
    foreach ($k in ($keys | Select-Object -Skip $skipFirst)) { if ($buckets[$k] -gt $max) { $max = $buckets[$k] } }
    return $max
}

# ---- window helpers --------------------------------------------------------------------------
function FormInfo($hwnds) {
    foreach ($h in $hwnds) {
        $r = New-Object LT+RECT
        [void][LT]::GetWindowRect($h, [ref]$r)
        $v = [LT]::Visible($h)
        [pscustomobject]@{
            title = [LT]::Title($h); hwnd = [int64]$h
            visible = [LT]::IsWindowVisible($h); minimized = [LT]::IsIconic($h)
            rect = "$($r.L),$($r.T),$($r.R),$($r.B)"
            visible_rect = "$($v.L),$($v.T),$($v.R),$($v.B)"
        }
    }
}
# Shown (not hidden, not minimised) - a window parked below the screen still counts.
function VisibleCount($hwnds) { @($hwnds | Where-Object { [LT]::IsWindow($_) -and [LT]::IsWindowVisible($_) -and -not [LT]::IsIconic($_) }).Count }
# Shown AND with at least 100x100 px inside the screen: what "visible to the user" means.
function OnScreen($h) {
    $v = [LT]::Visible($h); $s = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $w = [Math]::Min($v.R, $s.Right) - [Math]::Max($v.L, $s.Left)
    $t = [Math]::Min($v.B, $s.Bottom) - [Math]::Max($v.T, $s.Top)
    return ($w -ge 100 -and $t -ge 100)
}
function OnScreenCount($hwnds) { @($hwnds | Where-Object { [LT]::IsWindow($_) -and [LT]::IsWindowVisible($_) -and -not [LT]::IsIconic($_) -and (OnScreen $_) }).Count }
function NewTopLevel($before) { @([LT]::TopLevel() | Where-Object { $before -notcontains $_ }) }

function Start-Wiri([string]$stderrLog, [string]$stdoutLog) {
    return Start-Process -FilePath $wiri -ArgumentList '-v' -RedirectStandardError $stderrLog -RedirectStandardOutput $stdoutLog -PassThru
}

# ---- clean slate -----------------------------------------------------------------------------
Get-Process wiri -ErrorAction SilentlyContinue | ForEach-Object { Stop-Process -Id $_.Id -Force }
$cfg = Join-Path $env:APPDATA 'wiri\config.kdl'
if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $D 'config.kdl.before') -Force; Remove-Item $cfg -Force }   # test the generated default config
$wiriData = Join-Path $env:LOCALAPPDATA 'wiri'
foreach ($f in 'wiri.log', 'wiri.log.1', 'hidden.json') { Remove-Item (Join-Path $wiriData $f) -Force -ErrorAction SilentlyContinue }
Shot '00-before'

# Is wiri.exe a GUI-subsystem program (no console window of its own)?  PE optional header Subsystem: 2 = GUI, 3 = console.
$bytes = [IO.File]::ReadAllBytes($wiri)
$peOffset = [BitConverter]::ToInt32($bytes, 0x3C)
$subsystem = [BitConverter]::ToUInt16($bytes, $peOffset + 24 + 68)
$ctlBytes = [IO.File]::ReadAllBytes($ctl)
$ctlSubsystem = [BitConverter]::ToUInt16($ctlBytes, [BitConverter]::ToInt32($ctlBytes, 0x3C) + 24 + 68)

$topBefore = [LT]::TopLevel()
$w = Start-Wiri $log1 (Join-Path $D 'wiri.stdout.log')
Start-Sleep -Seconds 4
$steps.Add([pscustomobject]@{ step = 'start'; args = "pid $($w.Id)"; rc = $(if ($w.HasExited) { $w.ExitCode } else { 'running' }); bytes = 0 })
$wiriNewWindows = @(NewTopLevel $topBefore | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Pid($_) -eq $w.Id) }).Count
# A console-subsystem wiri.exe opens a console window (a Windows Terminal window here) the moment it starts.
$wiriNewConsole = @(NewTopLevel $topBefore | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Class($_) -match 'CASCADIA|ConsoleWindowClass') }).Count
Ctl '01-state' @('state') | Out-Null
Ctl '01-bindings' @('test-bindings') | Out-Null

# ---- Phase A: three plain windows -------------------------------------------------------------
$forms = @()
foreach ($i in 1..3) {
    $forms += [TestForms]::Open("wiri-test-$i", 40, (60 + 50 * $i), 120)
    Start-Sleep -Milliseconds 900
}
Start-Sleep -Seconds 3
# Settled layout must stop moving windows: count repositions over 5 idle seconds.
$p0 = PositioningLines $log1
Start-Sleep -Seconds 5
$settleRate = [Math]::Round(((PositioningLines $log1) - $p0) / 5.0, 2)
Ctl '02-windows' @('windows') | Out-Null
Ctl '02-state' @('state') | Out-Null
Ctl '02-focus' @('get-focus') | Out-Null
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '02-forms.json')
$fgTitle = [LT]::Title([LT]::GetForegroundWindow())
Set-Content (Join-Path $D '02-foreground.txt') $fgTitle
Shot '02-three-windows'

# Are the three windows non-overlapping columns inside the work area (above the taskbar)?
$wa = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
$vr = @($forms | ForEach-Object { [LT]::Visible($_) })
$inside = @($vr | Where-Object { $_.L -ge $wa.Left - 1 -and $_.R -le $wa.Right + 1 -and $_.T -ge $wa.Top - 1 -and $_.B -le $wa.Bottom + 1 }).Count
$overlaps = 0
for ($a = 0; $a -lt $vr.Count; $a++) { for ($b = $a + 1; $b -lt $vr.Count; $b++) {
    if ($vr[$a].L -lt $vr[$b].R -and $vr[$a].R -gt $vr[$b].L -and $vr[$a].T -lt $vr[$b].B -and $vr[$a].B -gt $vr[$b].T) { $overlaps++ }
} }
$minWidth = ($vr | ForEach-Object { $_.R - $_.L } | Measure-Object -Minimum).Minimum

# ---- Phase B: real terminal windows (the loop trigger) ----------------------------------------
$topB = [LT]::TopLevel()
$cons = @()
foreach ($i in 1..2) {
    $cons += Start-Process -FilePath cmd.exe -ArgumentList '/k', "title wiri-con-$i" -PassThru
    Start-Sleep -Milliseconds 1200
}
Start-Sleep -Seconds 4
$pB = PositioningLines $log1
Start-Sleep -Seconds 5
$termSettleRate = [Math]::Round(((PositioningLines $log1) - $pB) / 5.0, 2)
Ctl '02b-windows' @('windows') | Out-Null
$termWindows = @(NewTopLevel $topB | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Class($_) -match 'CASCADIA|ConsoleWindow') })
$termInfo = FormInfo $termWindows
$termInfo | ConvertTo-Json | Set-Content (Join-Path $D '02b-terminals.json')
Shot '02b-terminals'
foreach ($c in $cons) { if (-not $c.HasExited) { Stop-Process -Id $c.Id -Force } }
Start-Sleep -Seconds 2
# The terminals are gone: the three windows must be back on screen (the strip re-fits itself).
$formsOnScreenAfterTerminals = OnScreenCount $forms
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '02c-forms-after-terminals.json')
Shot '02c-after-terminals'

# ---- original steps: maximize, workspace 2 and back -------------------------------------------
Ctl '03-maximize' @('maximize-column') | Out-Null
Start-Sleep -Seconds 1
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '03-forms.json')
Shot '03-maximized'
Ctl '03b-maximize-back' @('maximize-column') | Out-Null

Ctl '04-ws2' @('switch-workspace', '2') | Out-Null
Start-Sleep -Seconds 1
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '04-forms-ws2.json')
$hiddenOnWs2 = $forms.Count - (VisibleCount $forms)
Shot '04-workspace2'
Ctl '05-ws1' @('switch-workspace', '1') | Out-Null
Start-Sleep -Seconds 1
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '05-forms-ws1.json')
$visibleOnWs1 = VisibleCount $forms
Shot '05-workspace1'

# ---- Phase C: crash safety ---------------------------------------------------------------------
Ctl '08-ws2-before-kill' @('switch-workspace', '2') | Out-Null
Start-Sleep -Seconds 1
$hiddenJson = Join-Path $wiriData 'hidden.json'
$hiddenJsonBeforeKill = (Test-Path $hiddenJson)
if ($hiddenJsonBeforeKill) { Copy-Item $hiddenJson (Join-Path $D '08-hidden.json') -Force }
Stop-Process -Id $w.Id -Force            # a crash: no chance to restore anything
Start-Sleep -Seconds 2
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '08-forms-wiri-dead.json')
$hiddenWhileDead = $forms.Count - (VisibleCount $forms)
Shot '08-crash-hidden'
$w2 = Start-Wiri $log2 (Join-Path $D 'wiri.stdout2.log')
Start-Sleep -Seconds 5
FormInfo $forms | ConvertTo-Json | Set-Content (Join-Path $D '09-forms-restarted.json')
$visibleAfterRestart = VisibleCount $forms
# The record also keeps where every window was before wiri moved it, so the file stays: what must be gone is
# the list of hidden windows.
$hiddenJsonAfterRestart = $false
if (Test-Path $hiddenJson) {
    try { $hiddenJsonAfterRestart = (@(((Get-Content $hiddenJson -Raw) | ConvertFrom-Json).windows).Count -gt 0) } catch { $hiddenJsonAfterRestart = $true }
}
Shot '09-crash-restored'
Ctl '09-windows-restarted' @('windows') | Out-Null
$w = $w2    # the instance that is left running from here on

# ---- Phase E: one hung application ---------------------------------------------------------------
# A window whose UI thread stops answering must not stall wiri or its IPC (every wiri-ctl call
# below has to come back well inside two seconds while the window is hung).
$hung = [TestForms]::OpenHung('wiri-hung', 12000)
Start-Sleep -Milliseconds 1500
$hungFirst = $steps.Count
foreach ($i in 1..3) { Ctl "10-hung-windows-$i" @('windows') | Out-Null; Start-Sleep -Milliseconds 600 }
Ctl '10-hung-state' @('state') | Out-Null
Ctl '10-hung-ws2' @('switch-workspace', '2') | Out-Null
Start-Sleep -Milliseconds 500
Ctl '10-hung-ws1' @('switch-workspace', '1') | Out-Null
$hungSteps = @($steps | Select-Object -Skip $hungFirst)
$hungMaxMs = ($hungSteps | Measure-Object -Property ms -Maximum).Maximum
$hungFailures = @($hungSteps | Where-Object { $_.rc -ne 0 }).Count
Shot '10-hung'
Start-Sleep -Seconds 12      # let the hang end before the next phase
Ctl '10-after-hang-windows' @('windows') | Out-Null
$hungWindowTiled = @(Get-Content (Join-Path $D '10-after-hang-windows.txt') -ErrorAction SilentlyContinue | Where-Object { $_ -match 'wiri-hung' }).Count

# ---- Phase F: a real console - wiri attaches to it, wiri-ctl draws its tables there ---------------
$checkCmd = Join-Path $D 'console-check.cmd'
@(
    '@echo off',
    'title wiri-console-check',
    'echo === wiri --version',
    "start /wait `"`" `"$wiri`" --version",
    'echo === wiri --not-a-flag',
    "start /wait `"`" `"$wiri`" --not-a-flag",
    'echo === wiri-ctl state',
    "`"$ctl`" state",
    'echo === done'
) | Set-Content -Encoding ASCII $checkCmd
$conF = Start-Process -FilePath cmd.exe -ArgumentList '/k', "`"$checkCmd`"" -PassThru
Start-Sleep -Seconds 5
Shot '11-console-check'
if (-not $conF.HasExited) { Stop-Process -Id $conF.Id -Force }
Start-Sleep -Seconds 1

# ---- Phase G: real key presses (RegisterHotKey + the keyboard hook) --------------------------------
$vk = @{
    ctrl = 0x11; alt = 0x12; shift = 0x10; esc = 0x1B; space = 0x20; 'return' = 0x0D
    '1' = 0x31; '2' = 0x32; '3' = 0x33; '4' = 0x34; a = 0x41; f = 0x46; j = 0x4A; k = 0x4B; q = 0x51; t = 0x54
    left = 0x25; right = 0x27; home = 0x24; end = 0x23
    comma = 0xBC; period = 0xBE; lbracket = 0xDB; rbracket = 0xDD; grave = 0xC0
    f1 = 0x70; f2 = 0x71; f3 = 0x72; f5 = 0x74; f6 = 0x75; f7 = 0x76; f8 = 0x77; f9 = 0x78
}
function Chord([string]$spec) {
    $keys = @($spec.ToLower().Split('+') | ForEach-Object { [byte]$vk[$_] })
    foreach ($k in $keys) { [LT]::Key($k, $false); Start-Sleep -Milliseconds 40 }
    Start-Sleep -Milliseconds 90
    for ($i = $keys.Count - 1; $i -ge 0; $i--) { [LT]::Key($keys[$i], $true); Start-Sleep -Milliseconds 30 }
}
# Does a plain Escape reach an application while wiri runs?  (It used to be registered as a hotkey, which
# takes the key away from every other program.)
[TestForms]::ClearKeys()
$keysForm = [TestForms]::OpenKeys('wiri-keys')
Start-Sleep -Milliseconds 1800
Chord 'esc'; Start-Sleep -Milliseconds 400
Chord 'a';   Start-Sleep -Milliseconds 400
Chord 'esc'; Start-Sleep -Milliseconds 400
$keysSeen = [TestForms]::KeyLog()
Set-Content (Join-Path $D '12-keys-seen.txt') $keysSeen
[TestForms]::CloseForm($keysForm)
Start-Sleep -Milliseconds 1200

# Overview on with the hotkey, off with Escape: wiri must keep answering (it used to deadlock).
Chord 'ctrl+alt+space'; Start-Sleep -Milliseconds 1500
Shot '12-overview-on'
Chord 'esc'; Start-Sleep -Milliseconds 1500
Shot '12-overview-escaped'
$null = Ctl '12-state-after-overview-esc' @('state')
$overviewResponsive = (@($steps | Where-Object { $_.step -eq '12-state-after-overview-esc' } | Select-Object -Last 1).rc -eq 0)

# Workspace 2 and back with the real hotkeys.
Chord 'ctrl+alt+2'; Start-Sleep -Milliseconds 1500
$hotkeyHidden = $forms.Count - (VisibleCount $forms)
Shot '12-hotkey-workspace-2'
Chord 'ctrl+alt+1'; Start-Sleep -Milliseconds 1500
$hotkeyBack = VisibleCount $forms
Shot '12-hotkey-workspace-1'

# ---- Phase H: Phase 1 scenarios - wiri restarted under a config of its own per scenario ------------------
# The generated default config is kept for the record before it is overwritten.
if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $D 'config.kdl.generated') -Force }
$log3 = Join-Path $D 'wiri.stderr3.log'
$pdir = Join-Path $D 'p1 dir'      # a directory with a space in its name
New-Item -ItemType Directory -Force $pdir | Out-Null

# One command-line argument, quoted the way every Windows program (and Start-Process, which does not quote)
# reads it back.
function Q([string]$s) {
    if ($s -ne '' -and $s -notmatch '[\s"]') { return $s }
    $s = [regex]::Replace($s, '(\\*)"', '$1$1\"')
    $s = [regex]::Replace($s, '(\\+)$', '$1$1')
    return '"' + $s + '"'
}
function Stop-WiriGracefully($proc, [string]$tag) {
    Ctl "$tag-quit" @('quit') | Out-Null
    $null = $proc.WaitForExit(10000)
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Start-Sleep -Milliseconds 800
}
function Start-WiriWithConfig([string]$text, [string]$stderr) {
    [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
    $p = Start-Wiri $stderr ($stderr + '.out')
    Start-Sleep -Seconds 4
    return $p
}
# Poll for a file that a program started by wiri writes.
function WaitFile([string]$path, [int]$sec) {
    for ($i = 0; $i -lt ($sec * 4) -and -not (Test-Path $path); $i++) { Start-Sleep -Milliseconds 250 }
    return (Test-Path $path)
}
function ReadJsonArray([string]$path) {
    if (-not (Test-Path $path)) { return @() }
    $raw = Get-Content $path -Raw -ErrorAction SilentlyContinue
    if (-not $raw) { return @() }
    return @($raw.TrimStart([char]0xFEFF) | ConvertFrom-Json | ForEach-Object { $_ })   # PS 5.1 hands the array back as one object
}
function SameList($a, $b) {
    $a = @($a); $b = @($b)
    if ($a.Count -ne $b.Count) { return $false }
    for ($i = 0; $i -lt $a.Count; $i++) { if ([string]$a[$i] -ne [string]$b[$i]) { return $false } }
    return $true
}

# ---- H1: modifiers - an unknown one is rejected (it used to register a bare global key), `Mod` follows mod-key
$h1Config = @"
input {
    mod-key "ctrl-alt"
}
binds {
    Hyper+F9 close-window
    Mod5+F8 close-window
    Mod+F7 refresh
}
"@
Stop-WiriGracefully $w 'h1'
$w3 = Start-WiriWithConfig $h1Config $log3
$h1Raw = Ctl 'h1-bindings' @('--json', 'bindings')
$h1Entries = @()
try { $h1Entries = @(($h1Raw | ConvertFrom-Json).result) } catch { }
# Windows virtual keys: F7 = 0x76 (118), F8 = 0x77 (119), F9 = 0x78 (120).
$h1StolenKeys = @($h1Entries | Where-Object { $_.vk_code -eq 119 -or $_.vk_code -eq 120 }).Count
$h1ModBind = @($h1Entries | Where-Object { $_.vk_code -eq 118 -and $_.modifiers -eq 3 }).Count
[TestForms]::ClearKeys()
$h1Keys = [TestForms]::OpenKeys('wiri-keys-h1')
Start-Sleep -Milliseconds 1800
Chord 'f9'; Start-Sleep -Milliseconds 400
Chord 'f8'; Start-Sleep -Milliseconds 400
$h1KeysSeen = [TestForms]::KeyLog()
Set-Content (Join-Path $D 'h1-keys-seen.txt') $h1KeysSeen
[TestForms]::CloseForm($h1Keys)
Start-Sleep -Milliseconds 1000
$w = $w3

# ---- H2: spawn - every argument kept, quoted paths with spaces, programs found on PATH ----------------------
$argdump = Join-Path $D 'argdump.ps1'
Set-Content -Path $argdump -Encoding ASCII -Value 'param([string]$OutFile)', 'ConvertTo-Json -InputObject @($args) -Compress | Set-Content -Path $OutFile -Encoding UTF8'
Copy-Item (Join-Path $env:SystemRoot 'System32\cmd.exe') (Join-Path $pdir 'my cmd.exe') -Force
foreach ($f in 'bind-spawn.json', 'bind-shell.txt', 'startup-spawn.json', 'ipc-spawn.json', 'ipc-line.txt') { Remove-Item (Join-Path $pdir $f) -Force -ErrorAction SilentlyContinue }

# The built-in Mod+Enter (no spawn bind in this config) starts Windows Terminal: press it for real.
$topBefore = [LT]::TopLevel()
Chord 'ctrl+alt+return'
$defaultTerm = $null
for ($i = 0; $i -lt 40 -and -not $defaultTerm; $i++) {
    Start-Sleep -Milliseconds 250
    $defaultTerm = @(NewTopLevel $topBefore | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Class($_) -match 'CASCADIA|ConsoleWindow') }) | Select-Object -First 1
}
$spawnDefaultTerminal = [bool]$defaultTerm
Shot '13-default-terminal'
if ($defaultTerm) { [LT]::Close($defaultTerm) }
Start-Sleep -Seconds 2

$h2Config = @"
binds {
    Mod+F7 spawn "powershell" "-NoProfile" "-File" "$argdump" "$pdir\bind-spawn.json" "arg one" "C:\Program Files\x y" "double  space" "quote\"d"
    Mod+F6 spawn-sh r#"echo shell ok> "$pdir\bind-shell.txt""#
}
spawn-at-startup "powershell" "-NoProfile" "-File" "$argdump" "$pdir\startup-spawn.json" "start arg" "C:\Program Files\startup dir\\"
"@
$log4 = Join-Path $D 'wiri.stderr4.log'
Stop-WiriGracefully $w 'h2'
$w4 = Start-WiriWithConfig $h2Config $log4
$null = WaitFile (Join-Path $pdir 'startup-spawn.json') 12
$spawnStartupArgsOk = SameList (ReadJsonArray (Join-Path $pdir 'startup-spawn.json')) @('start arg', 'C:\Program Files\startup dir\')
Chord 'ctrl+alt+f7'
$null = WaitFile (Join-Path $pdir 'bind-spawn.json') 12
$spawnBindArgsOk = SameList (ReadJsonArray (Join-Path $pdir 'bind-spawn.json')) @('arg one', 'C:\Program Files\x y', 'double  space', 'quote"d')
Chord 'ctrl+alt+f6'
$null = WaitFile (Join-Path $pdir 'bind-shell.txt') 8
$shellText = if (Test-Path (Join-Path $pdir 'bind-shell.txt')) { (Get-Content (Join-Path $pdir 'bind-shell.txt') -Raw).Trim() } else { '' }
$spawnShellOk = ($shellText -eq 'shell ok')
# IPC spawn: arguments given separately, and a whole quoted command line whose program path has a space.
$null = Ctl 'h2-ipc-spawn-args' @('spawn', 'powershell', '-NoProfile', '-File', (Q $argdump), (Q (Join-Path $pdir 'ipc-spawn.json')), (Q 'arg one'), (Q 'C:\Program Files\x y'))
$null = WaitFile (Join-Path $pdir 'ipc-spawn.json') 12
$spawnIpcArgsOk = SameList (ReadJsonArray (Join-Path $pdir 'ipc-spawn.json')) @('arg one', 'C:\Program Files\x y')
$line = "`"$(Join-Path $pdir 'my cmd.exe')`" /c echo ipc line> `"$(Join-Path $pdir 'ipc-line.txt')`""
$null = Ctl 'h2-ipc-spawn-line' @('spawn', (Q $line))
$null = WaitFile (Join-Path $pdir 'ipc-line.txt') 8
$ipcLineText = if (Test-Path (Join-Path $pdir 'ipc-line.txt')) { (Get-Content (Join-Path $pdir 'ipc-line.txt') -Raw).Trim() } else { '' }
$spawnIpcLineOk = ($ipcLineText -eq 'ipc line')
# A bare program name that lives only on PATH (an App Execution Alias): the example of the default config.
$topBefore = [LT]::TopLevel()
$null = Ctl 'h2-ipc-spawn-wt' @('spawn', 'wt.exe')
$wtWin = $null
for ($i = 0; $i -lt 40 -and -not $wtWin; $i++) {
    Start-Sleep -Milliseconds 250
    $wtWin = @(NewTopLevel $topBefore | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Class($_) -match 'CASCADIA|ConsoleWindow') }) | Select-Object -First 1
}
$spawnWtWindow = [bool]$wtWin
Shot '13-spawn-wt'
if ($wtWin) { [LT]::Close($wtWin) }
Start-Sleep -Seconds 2
$w = $w4

# ---- H3: actions act on the window the user is looking at - a floating one included, a dialog's owner, never a guess ----
# The visible rectangle of a window as a small object, and "within tol pixels".
function Rc($h) { $v = [LT]::Visible($h); return [pscustomobject]@{ L = $v.L; T = $v.T; R = $v.R; B = $v.B; W = ($v.R - $v.L); H = ($v.B - $v.T) } }
function Near([double]$a, [double]$b, [double]$tol) { return ([Math]::Abs($a - $b) -le $tol) }
function AliveCount($hwnds) { @($hwnds | Where-Object { [LT]::IsWindow($_) }).Count }
$log5 = Join-Path $D 'wiri.stderr5.log'
$h3Config = @"
input {
    mod-key "ctrl-alt"
}
"@
Stop-WiriGracefully $w 'h3'
$w5 = Start-WiriWithConfig $h3Config $log5
$w = $w5
$scr = [System.Windows.Forms.Screen]::PrimaryScreen
$sb = $scr.Bounds
$swa = $scr.WorkingArea

# The windows wiri manages, as `wiri-ctl --json windows` reports them (hwnd, workspace, floating).
function WinList([string]$tag) {
    $raw = Ctl $tag @('--json', 'windows')
    try { return @(($raw | ConvertFrom-Json).result) } catch { return @() }
}
function WinOf($list, $hwnd) { return (@($list | Where-Object { [int64]$_.hwnd -eq [int64]$hwnd }) | Select-Object -First 1) }
# The column of a window in its strip (-1 when it is in none: floating, minimized, unknown).
function ColOf($list, $hwnd) { $e = WinOf $list $hwnd; if ($null -eq $e -or $null -eq $e.column) { return -1 } else { return [int]$e.column } }

# (a) a floating window that has the keyboard focus is what toggle-floating, move, fullscreen and close act on -
# the engine's own focus is a tile behind it.
$fa = [TestForms]::Open('wiri-h3-float', 90, 40, 140)
Start-Sleep -Seconds 2
$list0 = WinList 'h3-windows-before'
$floatsBefore = @($list0 | Where-Object { $_.floating -eq $true }).Count
Chord 'ctrl+alt+t'; Start-Sleep -Milliseconds 1800
$h3FloatKeepsForeground = ([LT]::GetForegroundWindow() -eq $fa)
$r1 = Rc $fa
$init = [TestForms]::Initial['wiri-h3-float']
$h3FloatOwnSize = (Near $r1.W ($init.R - $init.L) 24) -and (Near $r1.H ($init.B - $init.T) 24)
$h3FloatCentered = (Near (($r1.L + $r1.R) / 2) (($swa.Left + $swa.Right) / 2) 10) -and (Near (($r1.T + $r1.B) / 2) (($swa.Top + $swa.Bottom) / 2) 10)
$list1 = WinList 'h3-windows-floated'
$h3ReportedFloating = ((WinOf $list1 $fa).floating -eq $true)
Shot 'h3-01-floated'

# A second press puts the SAME window back into the strip (it used to float the next tile as well).
Chord 'ctrl+alt+t'; Start-Sleep -Milliseconds 1800
$list2 = WinList 'h3-windows-tiled-again'
$h3SecondPressTiles = ((WinOf $list2 $fa).floating -eq $false)
$h3SecondPressFloatsNothingElse = (@($list2 | Where-Object { $_.floating -eq $true }).Count -eq $floatsBefore)
$rt = Rc $fa
$h3TiledAgainHasColumnHeight = ($rt.H -gt $r1.H + 40)
Shot 'h3-02-tiled-again'
# ... and the third press floats it again exactly where it floated before (remembered geometry).
Chord 'ctrl+alt+t'; Start-Sleep -Milliseconds 1800
$r1b = Rc $fa
$h3FloatGeometryRemembered = (Near $r1b.L $r1.L 4) -and (Near $r1b.T $r1.T 4) -and (Near $r1b.W $r1.W 4) -and (Near $r1b.H $r1.H 4)
$h3ForegroundAfterThirdPress = ([LT]::GetForegroundWindow() -eq $fa)

Chord 'ctrl+alt+shift+left'; Start-Sleep -Milliseconds 1200
$r2 = Rc $fa
$h3FloatNudged = (Near ($r2.L - $r1b.L) (-50) 4) -and (Near $r2.T $r1b.T 4) -and (Near $r2.W $r1b.W 4) -and (Near $r2.H $r1b.H 4)
Chord 'ctrl+alt+f'; Start-Sleep -Milliseconds 1800
$r3 = Rc $fa
Shot 'h3-03-float-fullscreen'
$h3FullscreenCovers = ($r3.L -le $sb.Left + 1) -and ($r3.T -le $sb.Top + 1) -and ($r3.R -ge $sb.Right - 1) -and ($r3.B -ge $sb.Bottom - 1)
$h3FullscreenOthers = @($forms | Where-Object { [LT]::IsWindow($_) } | Where-Object { $v = Rc $_; ($v.W -ge $sb.Width - 40) -and ($v.H -ge $sb.Height - 40) }).Count
Chord 'ctrl+alt+f'; Start-Sleep -Milliseconds 1800
$r4 = Rc $fa
Shot 'h3-04-float-fullscreen-left'
$h3FullscreenRestores = (Near $r4.L $r2.L 4) -and (Near $r4.T $r2.T 4) -and (Near $r4.W $r2.W 4) -and (Near $r4.H $r2.H 4)

# IPC focus-window raises a floating window (it used to answer "ok" and do nothing), tiles still get it too.
$null = Ctl 'h3-ipc-focus-tile' @('focus-window', [string][int64]$forms[0])
Start-Sleep -Milliseconds 1500
$h3IpcFocusTile = ([LT]::GetForegroundWindow() -eq $forms[0])
$null = Ctl 'h3-ipc-focus-float' @('focus-window', [string][int64]$fa)
Start-Sleep -Milliseconds 1500
$h3IpcFocusFloat = ([LT]::GetForegroundWindow() -eq $fa)

# The same through IPC: `wiri-ctl action center-window` acts on the foreground floating window.
$null = Ctl 'h3-ipc-center' @('action', 'center-window')
Start-Sleep -Milliseconds 1200
$r5 = Rc $fa
$h3IpcCenters = (Near (($r5.L + $r5.R) / 2) (($swa.Left + $swa.Right) / 2) 6) -and (Near (($r5.T + $r5.B) / 2) (($swa.Top + $swa.Bottom) / 2) 6) -and (Near $r5.W $r4.W 4)
$tilesBefore = AliveCount $forms
Chord 'ctrl+alt+q'; Start-Sleep -Milliseconds 2000
$h3FloatClosed = -not [LT]::IsWindow($fa)
$h3TilesSurviveFloatClose = ((AliveCount $forms) -eq $tilesBefore)
Shot 'h3-05-float-closed'

# (b) a dialog has the focus: close-window closes the dialog, not the application window that owns it.
$fb = [TestForms]::Open('wiri-h3-owner', 140, 90, 40)
Start-Sleep -Seconds 2
$dlg = [TestForms]::OpenOwned($fb, 'wiri-h3-dialog')
Start-Sleep -Milliseconds 1500
$h3DialogForeground = ($dlg -ne [IntPtr]::Zero) -and ([LT]::GetForegroundWindow() -eq $dlg)
Shot 'h3-06-dialog'
Chord 'ctrl+alt+q'; Start-Sleep -Milliseconds 2000
$h3DialogClosed = ($dlg -ne [IntPtr]::Zero) -and -not [LT]::IsWindow($dlg)
$h3OwnerSurvives = [LT]::IsWindow($fb)

# (c) an unmanaged window (a tool window) has the focus: close-window must not guess and close a tile behind it.
$tool = [TestForms]::OpenTool('wiri-h3-tool')
Start-Sleep -Milliseconds 1500
$h3ToolForeground = ([LT]::GetForegroundWindow() -eq $tool)
$aliveBefore = AliveCount (@($forms) + @($fb, $tool))
Chord 'ctrl+alt+q'; Start-Sleep -Milliseconds 2000
$h3CloseRefusedOnUnmanaged = ((AliveCount (@($forms) + @($fb, $tool))) -eq $aliveBefore)
Shot 'h3-07-unmanaged-foreground'
[TestForms]::CloseForm($tool)
[TestForms]::CloseForm($fb)
Start-Sleep -Milliseconds 1200

# ---- H4: a minimized window leaves no gap and comes back into its old place -------------------------------
# The title bar button's message (WM_SYSCOMMAND): SC_MINIMIZE = 0xF020, SC_RESTORE = 0xF120.
function Minimize($h) { [void][LT]::PostMessage($h, 0x0112, [IntPtr]0xF020, [IntPtr]::Zero) }
function Restore($h) { [void][LT]::PostMessage($h, 0x0112, [IntPtr]0xF120, [IntPtr]::Zero) }
$m = @()
foreach ($i in 1..3) { $m += [TestForms]::Open("wiri-h4-$i", (60 * $i), 120, 90); Start-Sleep -Milliseconds 1500 }
Start-Sleep -Seconds 1
$a1 = Rc $m[0]; $a2 = Rc $m[1]; $a3 = Rc $m[2]
$h4OrderBefore = ($a1.L -lt $a2.L) -and ($a2.L -lt $a3.L)
$h4GapBefore = $a3.L - $a1.R
Shot 'h4-01-three-windows'

# (a) the middle one is minimized: its neighbours close up
Minimize $m[1]; Start-Sleep -Milliseconds 1800
$h4Iconic = [LT]::IsIconic($m[1])
$b1 = Rc $m[0]; $b3 = Rc $m[2]
$h4GapAfter = $b3.L - $b1.R
$h4NoGap = ($h4GapAfter -ge 0) -and ($h4GapAfter -lt 60) -and ($h4GapBefore -gt 300)
$h4IpcMinimized = ((WinOf (WinList 'h4-windows-minimized') $m[1]).minimized -eq $true)
Shot 'h4-02-middle-minimized'

# ... and it is restored between them again, at its old size
Restore $m[1]; Start-Sleep -Milliseconds 1800
$c2 = Rc $m[1]
$lc = WinList 'h4-windows-restored'
# Consecutive columns: nothing else squeezed in between, and the window is back in the middle.
$h4RestoredBetween = (-not [LT]::IsIconic($m[1])) -and ((ColOf $lc $m[0]) -ge 0) -and ((ColOf $lc $m[1]) -eq ((ColOf $lc $m[0]) + 1)) -and ((ColOf $lc $m[2]) -eq ((ColOf $lc $m[1]) + 1))
$h4RestoredSize = (Near $c2.W $a2.W 8) -and (Near $c2.H $a2.H 8)
$h4IpcNotMinimized = ((WinOf $lc $m[1]).minimized -eq $false)
Shot 'h4-03-restored'

# (b) all three minimized (Win+D), restored in another order: the old order comes back
foreach ($h in $m) { Minimize $h; Start-Sleep -Milliseconds 500 }
Start-Sleep -Milliseconds 1200
$h4AllIconic = (@($m | Where-Object { [LT]::IsIconic($_) }).Count -eq 3)
Shot 'h4-04-all-minimized'
foreach ($h in @($m[2], $m[0], $m[1])) { Restore $h; Start-Sleep -Milliseconds 900 }
Start-Sleep -Milliseconds 1200
$ld = WinList 'h4-windows-restored-in-order'
$h4AllRestored = (@($m | Where-Object { [LT]::IsIconic($_) }).Count -eq 0)
$h4OrderKept = ((ColOf $ld $m[0]) -ge 0) -and ((ColOf $ld $m[1]) -eq ((ColOf $ld $m[0]) + 1)) -and ((ColOf $ld $m[2]) -eq ((ColOf $ld $m[1]) + 1))
Shot 'h4-05-restored-in-order'

# (c) wiri-ctl focus-window brings a minimized window back
Minimize $m[1]; Start-Sleep -Milliseconds 1500
$null = Ctl 'h4-ipc-focus-minimized' @('focus-window', [string][int64]$m[1])
Start-Sleep -Milliseconds 1800
$e2 = Rc $m[1]
$h4FocusRestores = (-not [LT]::IsIconic($m[1])) -and ([LT]::GetForegroundWindow() -eq $m[1]) -and (Near $e2.W $a2.W 8)

# (d) a window closed while it is minimized is forgotten
Minimize $m[2]; Start-Sleep -Milliseconds 1500
[TestForms]::CloseForm($m[2]); Start-Sleep -Milliseconds 1800
$h4ClosedGone = -not [LT]::IsWindow($m[2])
$h4ClosedForgotten = ($null -eq (WinOf (WinList 'h4-windows-after-close-gone') $m[2]))
$lf = WinList 'h4-windows-after-close'
$h4NeighboursStay = (-not [LT]::IsIconic($m[0])) -and (-not [LT]::IsIconic($m[1])) -and ((ColOf $lf $m[0]) -ge 0) -and ((ColOf $lf $m[1]) -eq ((ColOf $lf $m[0]) + 1))
Shot 'h4-06-closed-while-minimized'
[TestForms]::CloseForm($m[1]); [TestForms]::CloseForm($m[0])
Start-Sleep -Milliseconds 1200

function OverlapsRect($h, $rc) { $t = [LT]::Visible($h); return ($t.L -lt $rc.R -and $t.R -gt $rc.L -and $t.T -lt $rc.B -and $t.B -gt $rc.T) }
# ---- H5: a floating window belongs to its workspace, stays above the tiles and keeps its geometry ----------------
$ff = [TestForms]::Open('wiri-h5-float', 150, 60, 60)
Start-Sleep -Seconds 2
Chord 'ctrl+alt+t'; Start-Sleep -Milliseconds 1800          # the window in front floats
$g0 = Rc $ff
$gcx = [int](($g0.L + $g0.R) / 2); $gcy = [int](($g0.T + $g0.B) / 2)
$h5FloatOnTop = ([LT]::TopWindowAt($gcx, $gcy) -eq $ff)
$lg = WinList 'h5-windows-floated'
$h5IpcWorkspace1 = (((WinOf $lg $ff).floating -eq $true) -and ((WinOf $lg $ff).workspace -eq 1))
Shot 'h5-01-floated'

# (a) it is hidden with its workspace and shown with it, where it was
Chord 'ctrl+alt+2'; Start-Sleep -Milliseconds 1800
$h5HiddenOnWs2 = -not [LT]::IsWindowVisible($ff)
Shot 'h5-02-workspace-2'
Chord 'ctrl+alt+1'; Start-Sleep -Milliseconds 1800
$g1 = Rc $ff
$h5ShownOnWs1 = [LT]::IsWindowVisible($ff) -and (Near $g1.L $g0.L 4) -and (Near $g1.T $g0.T 4) -and (Near $g1.W $g0.W 4) -and (Near $g1.H $g0.H 4)
$h5OnTopAfterReturn = ([LT]::TopWindowAt($gcx, $gcy) -eq $ff)
Shot 'h5-03-back-on-workspace-1'

# (b) a tile that overlaps it takes the focus: the float stays above it
$under = @(WinList 'h5-windows-for-overlap' | Where-Object {
    $_.floating -eq $false -and $_.minimized -eq $false -and $null -ne $_.column -and
    [LT]::IsWindowVisible([IntPtr][int64]$_.hwnd) -and (OverlapsRect ([IntPtr][int64]$_.hwnd) $g0)
}) | Select-Object -First 1
if ($under) { $null = Ctl 'h5-focus-overlapping-tile' @('focus-window', [string][int64]$under.hwnd) }
Start-Sleep -Milliseconds 1800
$h5TileGotFocus = ($null -ne $under) -and ([LT]::GetForegroundWindow() -eq [IntPtr][int64]$under.hwnd)
$h5FloatAboveFocusedTile = ([LT]::TopWindowAt($gcx, $gcy) -eq $ff)
Shot 'h5-04-tile-focused'
# (and the same after a real click-like activation: the workspace round trip activates the focused tile)
Chord 'ctrl+alt+2'; Start-Sleep -Milliseconds 1500
Chord 'ctrl+alt+1'; Start-Sleep -Milliseconds 1800
$h5FloatAboveAfterRoundTrip = ([LT]::TopWindowAt($gcx, $gcy) -eq $ff)

# (c) moved to workspace 2 with the key, it leaves workspace 1 and shows up on 2 at the same place
$null = Ctl 'h5-focus-float' @('focus-window', [string][int64]$ff)
Start-Sleep -Milliseconds 1500
Chord 'ctrl+alt+shift+2'; Start-Sleep -Milliseconds 1800
$h5MovedAwayHidden = -not [LT]::IsWindowVisible($ff)
$lm = WinList 'h5-windows-moved'
$h5IpcWorkspace2 = (((WinOf $lm $ff).floating -eq $true) -and ((WinOf $lm $ff).workspace -eq 2))
Chord 'ctrl+alt+2'; Start-Sleep -Milliseconds 1800
$g2 = Rc $ff
$h5ShownOnWs2 = [LT]::IsWindowVisible($ff) -and (Near $g2.L $g0.L 4) -and (Near $g2.T $g0.T 4) -and (Near $g2.W $g0.W 4) -and (Near $g2.H $g0.H 4)
Shot 'h5-05-float-on-workspace-2'

# (d) focus-window on it from workspace 1 goes to its workspace
Chord 'ctrl+alt+1'; Start-Sleep -Milliseconds 1500
$h5HiddenAgainOnWs1 = -not [LT]::IsWindowVisible($ff)
$null = Ctl 'h5-focus-float-from-ws1' @('focus-window', [string][int64]$ff)
Start-Sleep -Milliseconds 1800
$h5FocusSwitchesWorkspace = [LT]::IsWindowVisible($ff) -and ([LT]::GetForegroundWindow() -eq $ff)
Shot 'h5-06-focus-window-switched-workspace'

# put it back on workspace 1 and close it
Chord 'ctrl+alt+shift+1'; Start-Sleep -Milliseconds 1500
Chord 'ctrl+alt+1'; Start-Sleep -Milliseconds 1500
$null = Ctl 'h5-focus-float-again' @('focus-window', [string][int64]$ff)
Start-Sleep -Milliseconds 1200
Chord 'ctrl+alt+q'; Start-Sleep -Milliseconds 1800
$h5Closed = -not [LT]::IsWindow($ff)

# ---- H6: the niri column keymap behaves like niri, driven by the shipped default binds --------------------------
$log6 = Join-Path $D 'wiri.stderr6.log'
$h6Config = @"
input {
    mod-key "ctrl-alt"
}
"@
Stop-WiriGracefully $w 'h6'
$w6 = Start-WiriWithConfig $h6Config $log6
$w = $w6
# The binds of the niri keymap are registered (a chord that another program owns would be missing here).
$h6BindsRaw = Ctl 'h6-bindings' @('--json', 'bindings')
$h6Binds = @()
try { $h6Binds = @(($h6BindsRaw | ConvertFrom-Json).result) } catch { }
function HasBind([int]$vkCode, [int]$mods, [string]$action) {
    return (@($h6Binds | Where-Object { $_.vk_code -eq $vkCode -and $_.modifiers -eq $mods -and $_.action -eq $action }).Count -eq 1)
}
$h6BindsRegistered = (HasBind 0xBC 3 'ConsumeWindowIntoColumn') -and (HasBind 0xBE 3 'ExpelWindowFromColumn') -and
    (HasBind 0xDB 7 'ConsumeOrExpelWindowLeft') -and (HasBind 0xDD 7 'ConsumeOrExpelWindowRight') -and
    (HasBind 0x4B 3 'MoveWindowUp') -and (HasBind 0x4A 3 'MoveWindowDown') -and
    (HasBind 0x24 3 'FocusColumnFirst') -and (HasBind 0x23 3 'FocusColumnLast') -and
    (HasBind 0x24 7 'MoveColumnToFirst') -and (HasBind 0x23 7 'MoveColumnToLast') -and (HasBind 0x46 7 'MaximizeColumn')
function TileOf($list, $hwnd) { $e = WinOf $list $hwnd; if ($null -eq $e -or $null -eq $e.tile) { return -1 } else { return [int]$e.tile } }
function Press([string]$chord) { Chord $chord; Start-Sleep -Milliseconds 1400 }
function FocusForm($h) { $null = Ctl 'h6-focus-window' @('focus-window', [string][int64]$h); Start-Sleep -Milliseconds 1300 }
# Three windows, one column each, the last three columns of the strip: s1 | s2 | s3
$sf = @()
foreach ($i in 1..3) { $sf += [TestForms]::Open("wiri-h6-$i", (50 * $i), 130, 200); Start-Sleep -Milliseconds 1500 }
Start-Sleep -Seconds 1
$l0 = WinList 'h6-windows-start'
$h6ThreeColumns = ((ColOf $l0 $sf[0]) -ge 0) -and ((ColOf $l0 $sf[1]) -eq ((ColOf $l0 $sf[0]) + 1)) -and ((ColOf $l0 $sf[2]) -eq ((ColOf $l0 $sf[1]) + 1))
Shot 'h6-01-three-columns'

# consume-or-expel-window-left (Ctrl+Alt+Shift+[): a lone window joins the column on its left (at the bottom)
FocusForm $sf[1]
Press 'ctrl+alt+shift+lbracket'
$l = WinList 'h6-after-consume-left'
$h6ConsumeLeft = ((ColOf $l $sf[1]) -eq (ColOf $l $sf[0])) -and ((TileOf $l $sf[1]) -eq ((TileOf $l $sf[0]) + 1))
$h6EmptiedColumnVanished = ((ColOf $l $sf[2]) -eq ((ColOf $l $sf[0]) + 1))
$h6FocusStaysOnMovedWindow = ([LT]::GetForegroundWindow() -eq $sf[1])
Shot 'h6-02-consumed-left'
# ... a window that shares its column leaves it, to the left
Press 'ctrl+alt+shift+lbracket'
$l = WinList 'h6-after-expel-left'
$h6ExpelLeft = (((ColOf $l $sf[1]) + 1) -eq (ColOf $l $sf[0])) -and (((ColOf $l $sf[0]) + 1) -eq (ColOf $l $sf[2]))
Shot 'h6-03-expelled-left'

# consume-or-expel-window-right (Ctrl+Alt+Shift+]): s2 | s1 | s3 -> s2 goes to the bottom of s1's column, then leaves to the right
FocusForm $sf[1]
Press 'ctrl+alt+shift+rbracket'
$l = WinList 'h6-after-consume-right'
$h6ConsumeRight = ((ColOf $l $sf[1]) -eq (ColOf $l $sf[0])) -and ((TileOf $l $sf[1]) -eq ((TileOf $l $sf[0]) + 1))
Press 'ctrl+alt+shift+rbracket'
$l = WinList 'h6-after-expel-right'
$h6ExpelRight = ((ColOf $l $sf[1]) -eq ((ColOf $l $sf[0]) + 1)) -and ((ColOf $l $sf[2]) -eq ((ColOf $l $sf[1]) + 1))
Shot 'h6-04-expelled-right'

# consume-window-into-column (Ctrl+Alt+Comma): the first window of the column on the RIGHT goes under the focused one
FocusForm $sf[0]
Press 'ctrl+alt+comma'
$l = WinList 'h6-after-consume-into'
$h6ConsumeInto = ((ColOf $l $sf[1]) -eq (ColOf $l $sf[0])) -and ((TileOf $l $sf[1]) -eq ((TileOf $l $sf[0]) + 1))
$h6ConsumeIntoFocusStays = ([LT]::GetForegroundWindow() -eq $sf[0])
# expel-window-from-column (Ctrl+Alt+Period): the BOTTOM window leaves to the right; the focus stays on the one above
Press 'ctrl+alt+period'
$l = WinList 'h6-after-expel-from'
$h6ExpelFrom = ((ColOf $l $sf[1]) -eq ((ColOf $l $sf[0]) + 1)) -and ((TileOf $l $sf[0]) -eq 0)
$h6ExpelFromFocusStays = ([LT]::GetForegroundWindow() -eq $sf[0])
Shot 'h6-05-consume-and-expel'

# move-window-up / move-window-down (Ctrl+Alt+K / J) inside a column, and the niri focus-window-up / down names
FocusForm $sf[0]
Press 'ctrl+alt+comma'       # s1 over s2 again
FocusForm $sf[1]
Press 'ctrl+alt+k'
$l = WinList 'h6-after-move-up'
$h6MoveUp = ((TileOf $l $sf[1]) -lt (TileOf $l $sf[0])) -and ((ColOf $l $sf[1]) -eq (ColOf $l $sf[0]))
$h6MoveKeepsFocus = ([LT]::GetForegroundWindow() -eq $sf[1])
Press 'ctrl+alt+j'
$l = WinList 'h6-after-move-down'
$h6MoveDown = ((TileOf $l $sf[1]) -gt (TileOf $l $sf[0]))
$null = Ctl 'h6-action-focus-window-up' @('action', 'focus-window-up'); Start-Sleep -Milliseconds 1300
$h6FocusWindowUp = ([LT]::GetForegroundWindow() -eq $sf[0])
$null = Ctl 'h6-action-focus-window-down' @('action', 'focus-window-down'); Start-Sleep -Milliseconds 1300
$h6FocusWindowDown = ([LT]::GetForegroundWindow() -eq $sf[1])
FocusForm $sf[0]
Press 'ctrl+alt+period'      # back to s1 | s2 | s3

# move-column-right / move-column-left (Ctrl+Alt+Shift+Right / Left) swap two columns (they used to merge the windows)
FocusForm $sf[0]
Press 'ctrl+alt+shift+right'
$l = WinList 'h6-after-move-column-right'
$h6MoveColumnRight = ((ColOf $l $sf[0]) -eq ((ColOf $l $sf[1]) + 1)) -and ((TileOf $l $sf[0]) -eq 0) -and ((TileOf $l $sf[1]) -eq 0) -and ([LT]::GetForegroundWindow() -eq $sf[0])
Press 'ctrl+alt+shift+left'
$l = WinList 'h6-after-move-column-left'
$h6MoveColumnLeft = ((ColOf $l $sf[1]) -eq ((ColOf $l $sf[0]) + 1)) -and ((TileOf $l $sf[0]) -eq 0) -and ((TileOf $l $sf[1]) -eq 0) -and ([LT]::GetForegroundWindow() -eq $sf[0])

# focus-column-first / focus-column-last (Ctrl+Alt+Home / End)
Press 'ctrl+alt+home'
$l = WinList 'h6-after-focus-first'
$fg = [int64][LT]::GetForegroundWindow()
$first = @($l | Where-Object { $_.column -eq 0 -and $_.tile -eq 0 }) | Select-Object -First 1
$h6FocusFirst = ($null -ne $first) -and ([int64]$first.hwnd -eq $fg)
Press 'ctrl+alt+end'
$maxCol = ($l | Where-Object { $null -ne $_.column } | Measure-Object -Property column -Maximum).Maximum
$last = @($l | Where-Object { $_.column -eq $maxCol -and $_.tile -eq 0 }) | Select-Object -First 1
$h6FocusLast = ($null -ne $last) -and ([int64]$last.hwnd -eq [int64][LT]::GetForegroundWindow())

# move-column-to-last / move-column-to-first (Ctrl+Alt+Shift+End / Home): the column goes to the end / start of the strip and keeps the focus
FocusForm $sf[1]
Press 'ctrl+alt+shift+end'
$l = WinList 'h6-after-column-to-last'
$maxCol = ($l | Where-Object { $null -ne $_.column } | Measure-Object -Property column -Maximum).Maximum
$h6ColumnToLast = ((ColOf $l $sf[1]) -eq $maxCol) -and ([LT]::GetForegroundWindow() -eq $sf[1])
Press 'ctrl+alt+shift+home'
$l = WinList 'h6-after-column-to-first'
$h6ColumnToFirst = ((ColOf $l $sf[1]) -eq 0) -and ([LT]::GetForegroundWindow() -eq $sf[1])
Shot 'h6-06-column-moved-to-first'

# maximize-column (Ctrl+Alt+Shift+F): a column with ONE window becomes as wide as the screen, and back
FocusForm $sf[2]
$m0 = Rc $sf[2]
Press 'ctrl+alt+shift+f'
$m1 = Rc $sf[2]
$h6MaximizeFull = ($m1.W -ge ($sb.Width - 60)) -and ($m1.L -le 60)
Shot 'h6-07-maximized'
Press 'ctrl+alt+shift+f'
$m2 = Rc $sf[2]
$h6MaximizeRestores = (Near $m2.W $m0.W 8)
Shot 'h6-08-maximize-restored'
foreach ($h in $sf) { [TestForms]::CloseForm($h) }
Start-Sleep -Milliseconds 1500

# ---- H7: niri's action names and argument grammar port as they are ------------------------------------------------
# set-column-width "50%" / "+10%" / "-10%" / "600" / "+100", switch-preset-column-width, focus-workspace 2 (an
# index) against focus-workspace "2" (a name), and niri's names through wiri-ctl.
$log7 = Join-Path $D 'wiri.stderr7.log'
$h7Config = @"
input {
    mod-key "ctrl-alt"
}
binds {
    Mod+F1 set-column-width "50%"
    Mod+F2 set-column-width "+10%"
    Mod+F3 set-column-width "-10%"
    Mod+F5 set-column-width "600"
    Mod+F6 set-column-width "+100"
    Mod+F7 switch-preset-column-width
    Mod+F9 switch-preset-column-width-back
    Mod+2 focus-workspace 2
    Mod+1 focus-workspace 1
    Mod+3 focus-workspace "2"
}
"@
Stop-WiriGracefully $w 'h7'
$w7 = Start-WiriWithConfig $h7Config $log7
$w = $w7
$h7BindWarnings = (LogLines $log7 'Config bind:').Count
$h7BindsRaw = Ctl 'h7-bindings' @('--json', 'bindings')
$h7Binds = @()
try { $h7Binds = @(($h7BindsRaw | ConvertFrom-Json).result) } catch { }
function H7Bind([int]$vkCode, [string]$action) {
    return (@($h7Binds | Where-Object { $_.vk_code -eq $vkCode -and $_.modifiers -eq 3 -and $_.action -eq $action }).Count -eq 1)
}
$h7BindsRegistered = (H7Bind 0x70 'SetColumnWidth(SetProportion(50000))') -and (H7Bind 0x71 'SetColumnWidth(AdjustProportion(10000))') -and
    (H7Bind 0x72 'SetColumnWidth(AdjustProportion(-10000))') -and (H7Bind 0x74 'SetColumnWidth(SetFixed(600))') -and
    (H7Bind 0x75 'SetColumnWidth(AdjustFixed(100))') -and (H7Bind 0x76 'ColumnWidthPresetCycle') -and
    (H7Bind 0x78 'ColumnWidthPresetCycleBack') -and (H7Bind 0x32 'FocusWorkspace(2)') -and
    (H7Bind 0x31 'FocusWorkspace(1)') -and (H7Bind 0x33 'FocusWorkspaceNamed("2")')
function WinW($h) { return (Rc $h).W }
function WinH($h) { return (Rc $h).H }
$sf = @()
foreach ($i in 1..3) { $sf += [TestForms]::Open("wiri-h7-$i", (50 * $i), 90, 200); Start-Sleep -Milliseconds 1500 }
Start-Sleep -Seconds 1
Shot 'h7-01-three-columns'

# --- the size grammar through real key presses (the middle window; its column is alone in it)
FocusForm $sf[1]
Press 'ctrl+alt+f1'
$wHalf = WinW $sf[1]
# a 1366 px screen: view 1350, 16 px gaps, 4 px borders: half = (1350 + 16) / 2 - 16 - 8 = 659
$h7SetPercent = (Near $wHalf 659 14)
Shot 'h7-02-set-50-percent'
Press 'ctrl+alt+f2'
$wPlus = WinW $sf[1]
$h7AdjustPercentUp = (Near ($wPlus - $wHalf) 137 14)
Press 'ctrl+alt+f3'
$h7AdjustPercentDown = (Near (WinW $sf[1]) $wHalf 8)
Press 'ctrl+alt+f5'
$h7SetPixels = (Near (WinW $sf[1]) 600 10)
Press 'ctrl+alt+f6'
$h7AdjustPixels = (Near (WinW $sf[1]) 700 10)
Shot 'h7-03-plus-100-pixels'

# two columns at 50 % fill the view exactly: the window edges meet the outer gap (8 px) and border (4 px)
FocusForm $sf[0]
Press 'ctrl+alt+f1'
FocusForm $sf[1]
Press 'ctrl+alt+f1'
$a = Rc $sf[0]; $b = Rc $sf[1]
$h7HalvesFillTheView = (Near $a.L 12 8) -and (Near $b.R ($sb.Width - 12) 8) -and (Near ($b.L - $a.R) 24 8)
Shot 'h7-04-two-halves'

# switch-preset-column-width: from a 600 px window the first preset that is wider (a half), then two thirds,
# a third; going back from a third: two thirds
Press 'ctrl+alt+f5'
Press 'ctrl+alt+f7'
$pA = WinW $sf[1]
Press 'ctrl+alt+f7'
$pB = WinW $sf[1]
Press 'ctrl+alt+f7'
$pC = WinW $sf[1]
Press 'ctrl+alt+f9'
$pD = WinW $sf[1]
# presets of this screen: a third 431, a half 659, two thirds 887 (window widths)
$h7PresetCycle = (Near $pA 659 14) -and (Near $pB 887 14) -and (Near $pC 431 14) -and (Near $pD 887 14)
Shot 'h7-05-presets'

# --- focus-workspace 2 is the second workspace, focus-workspace "2" the workspace named "2" (there is none)
Press 'ctrl+alt+2'
$h7WorkspaceIndex = ((VisibleCount $sf) -eq 0)
Shot 'h7-06-workspace-2'
Press 'ctrl+alt+1'
$h7WorkspaceIndexBack = ((VisibleCount $sf) -eq 3)
Press 'ctrl+alt+3'
$h7QuotedIsAName = ((VisibleCount $sf) -eq 3)

# --- niri's names through wiri-ctl action
FocusForm $sf[2]
$null = Ctl 'h7-action-fullscreen' @('action', 'fullscreen-window'); Start-Sleep -Milliseconds 1600
$r = Rc $sf[2]
$h7FullscreenWindow = ($r.L -le $sb.Left + 1) -and ($r.T -le $sb.Top + 1) -and ($r.R -ge $sb.Right - 1) -and ($r.B -ge $sb.Bottom - 1)
Shot 'h7-07-fullscreen-window'
$null = Ctl 'h7-action-fullscreen-off' @('action', 'fullscreen-window'); Start-Sleep -Milliseconds 1600
$h7FullscreenWindowOff = ((Rc $sf[2]).W -lt ($sb.Width - 100))
$null = Ctl 'h7-action-float' @('action', 'toggle-window-floating'); Start-Sleep -Milliseconds 1300
$h7ToggleWindowFloating = ((WinOf (WinList 'h7-windows-floating') $sf[2]).floating -eq $true)
$null = Ctl 'h7-action-float-again' @('action', 'move-window-to-floating'); Start-Sleep -Milliseconds 1300
$h7ToFloatingIsNoToggle = ((WinOf (WinList 'h7-windows-still-floating') $sf[2]).floating -eq $true)
$null = Ctl 'h7-action-tile' @('action', 'move-window-to-tiling'); Start-Sleep -Milliseconds 1300
$h7ToTiling = ((WinOf (WinList 'h7-windows-tiled') $sf[2]).floating -eq $false)
$null = Ctl 'h7-action-tile-again' @('action', 'move-window-to-tiling'); Start-Sleep -Milliseconds 1300
$h7ToTilingIsNoToggle = ((WinOf (WinList 'h7-windows-still-tiled') $sf[2]).floating -eq $false)
$null = Ctl 'h7-action-focus-monitor' @('action', 'focus-monitor-left'); Start-Sleep -Milliseconds 800
$h7FocusMonitorAtTheEdge = ([LT]::GetForegroundWindow() -eq $sf[2])

# set-window-height: two windows in one column, the upper one gets 70 % of the view, then 30 %, then the default
FocusForm $sf[0]
$null = Ctl 'h7-action-consume' @('action', 'consume-window-into-column'); Start-Sleep -Milliseconds 1500
$hEqual0 = WinH $sf[0]; $hEqual1 = WinH $sf[1]
$null = Ctl 'h7-action-height-70' @('action', 'set-window-height', '70%'); Start-Sleep -Milliseconds 1500
# work area 728 - 16 outer gaps = 712 high; 12 px between the windows: 70 % = 0.7 * 724 - 12 = 495 - 8 = 487; the other 700 - 495 - 8 = 197
$ha = WinH $sf[0]; $hb = WinH $sf[1]
$h7WindowHeightPercent = (Near $ha 487 14) -and (Near $hb 197 14)
Shot 'h7-08-height-70'
$null = Ctl 'h7-action-height-minus-30' @('action', 'set-window-height', '-30%'); Start-Sleep -Milliseconds 1500
$ha = WinH $sf[0]; $hb = WinH $sf[1]
$h7WindowHeightAdjust = (Near $ha 270 14) -and (Near $hb 414 14)
$null = Ctl 'h7-action-height-reset' @('action', 'reset-window-height'); Start-Sleep -Milliseconds 1500
$h7WindowHeightReset = (Near (WinH $sf[0]) (WinH $sf[1]) 14) -and (Near (WinH $sf[0]) $hEqual0 14)
Shot 'h7-09-height-reset'
$null = Ctl 'h7-ctl-set-width' @('set-column-width', '50%'); Start-Sleep -Milliseconds 1300
$h7CtlSetColumnWidth = (Near (WinW $sf[0]) 659 14)
$null = Ctl 'h7-ctl-set-width-minus' @('set-column-width', '-10%'); Start-Sleep -Milliseconds 1300
$h7CtlSetColumnWidthMinus = (Near (WinW $sf[0]) 522 14)
foreach ($h in $sf) { [TestForms]::CloseForm($h) }
Start-Sleep -Milliseconds 1500

# ---- H8: the workspace model: named workspaces that always exist, index addressing, previous, back and forth ----------
# Config: workspace "mail" (no block) and workspace "web" (block), workspace-auto-back-and-forth in input.
$log8 = Join-Path $D 'wiri.stderr8.log'
$h8Config = @"
input {
    mod-key "ctrl-alt"
    workspace-auto-back-and-forth
}
workspace "mail"
workspace "web" {
}
binds {
    Mod+F1 focus-workspace "web"
    Mod+F2 focus-workspace-previous
    Mod+F3 focus-workspace 3
    Mod+F5 focus-workspace 9
}
"@
Stop-WiriGracefully $w 'h8'
$w8 = Start-WiriWithConfig $h8Config $log8
$w = $w8
$h8BindWarnings = (LogLines $log8 'Config bind:').Count
function WsList([string]$tag) {
    $raw = Ctl $tag @('--json', 'get-workspace-list')
    try { return @(($raw | ConvertFrom-Json).result.workspaces) } catch { return @() }
}
$wl = WsList 'h8-workspaces-at-start'
$h8Names = @($wl | ForEach-Object { $_.name })
# mail and web exist from the start ("web" is empty), the empty workspace below them is the third
$h8NamedExist = ($wl.Count -eq 3) -and ($h8Names[0] -eq 'mail') -and ($h8Names[1] -eq 'web') -and ($wl[1].window_count -eq 0)
$sf = @()
foreach ($i in 1..3) { $sf += [TestForms]::Open("wiri-h8-$i", (50 * $i), 100, 220); Start-Sleep -Milliseconds 1500 }
Start-Sleep -Seconds 1
Shot 'h8-01-three-forms-on-mail'

# focus-workspace "web": the named workspace is there although nothing lives on it
Press 'ctrl+alt+f1'
$h8WebIsFocusable = ((VisibleCount $sf) -eq 0)
Shot 'h8-02-web'
# focus-workspace-previous: back to the workspace the forms are on
Press 'ctrl+alt+f2'
$h8Previous = ((VisibleCount $sf) -eq 3)
# focus-workspace 3 is the empty workspace below the two named ones; asking for it again goes back
Press 'ctrl+alt+f3'
$h8ThirdIsTheEmptyOne = ((VisibleCount $sf) -eq 0)
Press 'ctrl+alt+f3'
$h8BackAndForth = ((VisibleCount $sf) -eq 3)
# focus-workspace 9 is not a workspace of its own: it is the last one, and nothing is made up
Press 'ctrl+alt+f5'
$wl = WsList 'h8-workspaces-after-9'
$h8NumberPastTheEnd = ((VisibleCount $sf) -eq 0) -and ($wl.Count -eq 3)
# niri does not wrap: down at the bottom stays, up at the top stays
$null = Ctl 'h8-action-down' @('action', 'focus-workspace-down'); Start-Sleep -Milliseconds 1200
$h8NoWrapAtTheBottom = ((VisibleCount $sf) -eq 0)
Press 'ctrl+alt+1'
$h8BackOnTheFirst = ((VisibleCount $sf) -eq 3)
$null = Ctl 'h8-action-up' @('action', 'focus-workspace-up'); Start-Sleep -Milliseconds 1200
$h8NoWrapAtTheTop = ((VisibleCount $sf) -eq 3)

# index addressing counts positions: make a workspace in the middle go away, then ask for number 3
FocusForm $sf[0]
Press 'ctrl+alt+shift+3'      # form 1 to the third workspace (the empty one: a new empty one appears below)
FocusForm $sf[1]
Press 'ctrl+alt+shift+4'      # form 2 to the fourth
$null = Ctl 'h8-focus-form-1' @('focus-window', [string][int64]$sf[0]); Start-Sleep -Milliseconds 1500
$h8SwitchedToItsWorkspace = ([LT]::GetForegroundWindow() -eq $sf[0]) -and ([LT]::IsWindowVisible($sf[0]))
Press 'ctrl+alt+shift+1'      # form 1 back to the first: its workspace (the third) is empty now
Press 'ctrl+alt+1'            # leave it: the empty workspace in the middle vanishes
Press 'ctrl+alt+f3'           # focus-workspace 3: the workspace of form 2 (it was the fourth before the gap closed)
$l = WinList 'h8-windows-after-the-gap'
$h8ThirdIsFormTwo = ([LT]::IsWindowVisible($sf[1])) -and (-not [LT]::IsWindowVisible($sf[0])) -and ((WinOf $l $sf[1]).workspace -eq 3) -and ((WinOf $l $sf[0]).workspace -eq 1)
Shot 'h8-03-third-is-form-two'
Press 'ctrl+alt+1'
foreach ($h in $sf) { [TestForms]::CloseForm($h) }
Start-Sleep -Milliseconds 1500

# ---- H9: Phase 0 leftovers - default hotkeys that register, an exact fullscreen, a terminal's minimum width, ------
# ----     windows put back where they were after a crash ----------------------------------------------------------
# No config file: the only thing under test is what the shipped defaults do.
$log9 = Join-Path $D 'wiri.stderr9.log'
Stop-WiriGracefully $w 'h9'
Remove-Item $cfg -Force -ErrorAction SilentlyContinue
$w9 = Start-Wiri $log9 ($log9 + '.out')
Start-Sleep -Seconds 4
$w = $w9
$sb = [System.Windows.Forms.SystemInformation]::VirtualScreen
# Every default chord registers: Ctrl+Alt+Tab is Windows' own task switcher and was refused at every start.
$h9FailedHotkeys = (LogLines $log9 'FAIL hotkey').Count
$h9RegisteredHotkeys = (LogLines $log9 'OK hotkey').Count

# --- the key above Tab takes the place of Ctrl+Alt+Tab: back to the window that had the focus before
$h9A = [TestForms]::Open('wiri-h9-a', 40, 120, 200); Start-Sleep -Milliseconds 1500
$h9B = [TestForms]::Open('wiri-h9-b', 200, 120, 40); Start-Sleep -Milliseconds 1800
FocusForm $h9A
$h9FocusedA = ([LT]::GetForegroundWindow() -eq $h9A)
Chord 'ctrl+alt+grave'; Start-Sleep -Milliseconds 1400
$h9GraveGoesBack = ([LT]::GetForegroundWindow() -eq $h9B)

# --- fullscreen is exactly the monitor (it used to be 7 px larger), and the tile that comes back has the usual inset
FocusForm $h9B
$null = Ctl 'h9-action-fullscreen' @('action', 'fullscreen-window'); Start-Sleep -Milliseconds 1800
$fr = Rc $h9B
$h9FullscreenExact = ($fr.L -eq $sb.Left) -and ($fr.T -eq $sb.Top) -and ($fr.W -eq $sb.Width) -and ($fr.H -eq $sb.Height)
Set-Content (Join-Path $D 'h9-fullscreen-rect.json') ($fr | ConvertTo-Json)
Shot 'h9-01-fullscreen'
$null = Ctl 'h9-action-fullscreen-off' @('action', 'fullscreen-window'); Start-Sleep -Milliseconds 1800
$tr = Rc $h9B
$ar = Rc $h9A
$h9FullscreenOffTiles = ($tr.W -lt ($sb.Width - 60)) -and ($tr.H -lt $sb.Height)
$h9TileAfterFullscreenAligned = (Near $tr.T $ar.T 2) -and (Near $tr.H $ar.H 2)
Set-Content (Join-Path $D 'h9-tiles-after-fullscreen.json') (@($ar, $tr) | ConvertTo-Json)
Shot 'h9-02-tile-after-fullscreen'
[TestForms]::CloseForm($h9A)
[TestForms]::CloseForm($h9B)
Start-Sleep -Milliseconds 1500

# --- three terminals side by side: a Windows Terminal window will not get narrower than about 462 px, so its column has to be
# --- that wide (it used to be asked for 440 px, refuse, and lie on top of its neighbour)
$topBefore9 = [LT]::TopLevel()
$cons9 = @()
foreach ($i in 1..3) {
    $cons9 += Start-Process -FilePath cmd.exe -ArgumentList '/k', "title wiri-h9-con-$i" -PassThru
    Start-Sleep -Milliseconds 1600
}
Start-Sleep -Seconds 4
$wts9 = @(NewTopLevel $topBefore9 | Where-Object { [LT]::IsWindowVisible($_) -and ([LT]::Title($_) -like 'wiri-h9-con-*') })
$h9TerminalsOpened = ($wts9.Count -eq 3)
$r9 = @($wts9 | ForEach-Object { Rc $_ })
Set-Content (Join-Path $D 'h9-terminal-rects.json') ($r9 | ConvertTo-Json)
Shot 'h9-03-three-terminals'
# A window that is scrolled out of view is parked below the screen (at x=0, like every parked window): only windows on the screen are compared.
$on9 = @($r9 | Where-Object { $_.R -gt 0 -and $_.L -lt $sb.Width -and $_.B -gt 0 -and $_.T -lt $sb.Height })
$h9Overlaps = 0
for ($a = 0; $a -lt $on9.Count; $a++) { for ($b = $a + 1; $b -lt $on9.Count; $b++) {
    $x = $on9[$a]; $y = $on9[$b]
    if ($x.L -lt $y.R - 1 -and $x.R -gt $y.L + 1 -and $x.T -lt $y.B - 1 -and $x.B -gt $y.T + 1) { $h9Overlaps++ }
} }
foreach ($c in $cons9) { if (-not $c.HasExited) { Stop-Process -Id $c.Id -Force } }
Start-Sleep -Seconds 2

# --- a crash does not cost the windows their old places: wiri dies with some of them parked below the screen, is started
# --- again, and `quit` puts every window back where it was before the first wiri touched it (it used to pile them up in
# --- the middle of the screen)
$cf = @()
foreach ($i in 1..4) {
    $cf += [TestForms]::Open("wiri-h9-crash-$i", (40 * $i), 140, (230 - 40 * $i))
    Start-Sleep -Milliseconds 1400
    $null = Ctl "h9-width-$i" @('set-column-width', '700')     # wide columns: the first ones scroll out of view and are parked
    Start-Sleep -Milliseconds 500
}
Start-Sleep -Seconds 2
$h9Initial = @{}
foreach ($f in $cf) { $t = [LT]::Title($f); $h9Initial[$t] = [TestForms]::Initial[$t] }
$h9Parked = @($cf | Where-Object { (Rc $_).T -ge $sb.Height }).Count
$hj = Join-Path $wiriData 'hidden.json'
function H9Originals() {
    if (-not (Test-Path $hj)) { return 0 }
    try {
        $j = (Get-Content $hj -Raw) | ConvertFrom-Json
        $mine = @($cf | ForEach-Object { [int64]$_ })
        return @($j.originals | Where-Object { $mine -contains [int64]$_.hwnd }).Count
    } catch { return -1 }
}
$h9OriginalsBefore = H9Originals
if (Test-Path $hj) { Copy-Item $hj (Join-Path $D 'h9-hidden-before-the-crash.json') -Force }
Stop-Process -Id $w.Id -Force            # a crash: nothing is put back
Start-Sleep -Seconds 2
$h9MovedByWiri = 0
foreach ($f in $cf) {
    $init = $h9Initial[[LT]::Title($f)]
    $r = New-Object LT+RECT; [void][LT]::GetWindowRect($f, [ref]$r)
    if ($r.L -ne $init.L -or $r.T -ne $init.T) { $h9MovedByWiri++ }
}
Shot 'h9-04-after-the-crash'
$log9b = Join-Path $D 'wiri.stderr9b.log'
$w9b = Start-Wiri $log9b ($log9b + '.out')
Start-Sleep -Seconds 5
$h9OriginalsAfterRestart = H9Originals
if (Test-Path $hj) { Copy-Item $hj (Join-Path $D 'h9-hidden-after-the-restart.json') -Force }
Stop-WiriGracefully $w9b 'h9b'
$h9Restored = 0
$h9RestoredRects = @()
foreach ($f in $cf) {
    $init = $h9Initial[[LT]::Title($f)]
    $r = New-Object LT+RECT; [void][LT]::GetWindowRect($f, [ref]$r)
    $h9RestoredRects += [pscustomobject]@{ title = [LT]::Title($f); L = $r.L; T = $r.T; R = $r.R; B = $r.B; initial = "$($init.L),$($init.T),$($init.R),$($init.B)" }
    if ((Near $r.L $init.L 2) -and (Near $r.T $init.T 2) -and (Near ($r.R - $r.L) ($init.R - $init.L) 2) -and (Near ($r.B - $r.T) ($init.B - $init.T) 2)) { $h9Restored++ }
}
Set-Content (Join-Path $D 'h9-restored-rects.json') ($h9RestoredRects | ConvertTo-Json)
Shot 'h9-05-restored-after-crash-and-quit'
$h9RecordGoneAfterQuit = -not (Test-Path $hj)
foreach ($f in $cf) { [TestForms]::CloseForm($f) }
Start-Sleep -Milliseconds 1500
# the instance that is left running for the rest of the script
$log9c = Join-Path $D 'wiri.stderr9c.log'
$w = Start-Wiri $log9c ($log9c + '.out')
Start-Sleep -Seconds 4

# ---- H11: hide-method "cloak" - the windows of another workspace stay visible to the system but are not drawn ---------
# The default (hide) is what every phase above ran with; this one asks for the shell's cloak.  The windows are plain-coloured
# forms, so "not drawn" is read off the screenshot: not one pixel of their colour is left.  "In Alt+Tab and on the taskbar" is
# what the shell itself says (IApplicationView::GetShowInSwitchers), and "Alt+Tab to a window" is the shell's own call
# (IApplicationView::SwitchTo) - the same interfaces wiri cloaks with, reached from this script through COM interop.
$log11 = Join-Path $D 'wiri.stderr11.log'
$h11Config = @"
layout {
    hide-method "cloak"
}
"@
Stop-WiriGracefully $w 'h11'
$w11 = Start-WiriWithConfig $h11Config $log11
$w = $w11
$h11Warnings = (LogLines $log11 'hide-method').Count + (LogLines $log11 'does not offer the shell').Count
$h11Colors = @(@(255, 0, 255), @(0, 255, 255), @(255, 255, 0))
$cf11 = @()
foreach ($i in 0..2) {
    $cf11 += [TestForms]::Open("wiri-h11-$($i + 1)", $h11Colors[$i][0], $h11Colors[$i][1], $h11Colors[$i][2])
    Start-Sleep -Milliseconds 1500
}
Start-Sleep -Seconds 2
function H11Drawn([string]$shot) {
    Shot $shot
    $p = Join-Path $D "$shot.png"
    return @(0, 1, 2 | ForEach-Object { [LT]::CountColor($p, $h11Colors[$_][0], $h11Colors[$_][1], $h11Colors[$_][2]) })
}
function H11Cloaked() { return @($cf11 | ForEach-Object { [int][LT]::Cloaked($_) }) }
function H11Visible() { return @($cf11 | ForEach-Object { [LT]::IsWindowVisible($_) }) }
function H11InSwitchers() { return @($cf11 | ForEach-Object { [int][ShellViews]::InSwitchers($_) }) }
$h11DrawnBefore = H11Drawn 'h11-01-three-forms'
$h11InSwitchersBefore = H11InSwitchers
# the first form was in front, then the third: the window Alt+Tab goes back to from the third one is the first
FocusForm $cf11[0]
FocusForm $cf11[2]

# to the second workspace: the three forms leave the screen but not the system
$null = Ctl 'h11-ws2' @('switch-workspace', '2'); Start-Sleep -Milliseconds 2500
$h11Cloaked = H11Cloaked
$h11Visible = H11Visible
$h11InSwitchersAway = H11InSwitchers
$h11DrawnAway = H11Drawn 'h11-02-forms-cloaked'
$hj11 = Join-Path $wiriData 'hidden.json'
$h11RecordedCloaked = 0
if (Test-Path $hj11) {
    try {
        $mine = @($cf11 | ForEach-Object { [int64]$_ })
        $h11RecordedCloaked = @(((Get-Content $hj11 -Raw) | ConvertFrom-Json).windows | Where-Object { $mine -contains [int64]$_.hwnd -and $_.cloaked }).Count
        Copy-Item $hj11 (Join-Path $D 'h11-hidden-while-cloaked.json') -Force
    } catch { $h11RecordedCloaked = -1 }
}

# Alt+Tab (the keys): the shell goes back to the first form, which is cloaked, and its workspace comes with it
$h11ForegroundBefore = [int64][LT]::GetForegroundWindow()
[LT]::Key(0x12, $false); Start-Sleep -Milliseconds 80
[LT]::Key(0x09, $false); Start-Sleep -Milliseconds 80
[LT]::Key(0x09, $true); Start-Sleep -Milliseconds 400
[LT]::Key(0x12, $true)
Start-Sleep -Milliseconds 2500
$h11ForegroundNow = [int64][LT]::GetForegroundWindow()
$h11CloakedAfterAltTab = H11Cloaked
$h11VisibleAfterAltTab = H11Visible
$h11DrawnAfterAltTab = H11Drawn 'h11-03-after-alt-tab'
$null = Ctl 'h11-windows-after-alt-tab' @('--json', 'windows')

# away again, then wiri dies: the windows stay cloaked (nobody is left to show them)
$null = Ctl 'h11-ws2-again' @('switch-workspace', '2'); Start-Sleep -Milliseconds 2500
$h11CloakedBeforeCrash = H11Cloaked
Stop-Process -Id $w.Id -Force
Start-Sleep -Seconds 2
$h11CloakedWhileDead = H11Cloaked
$h11DrawnWhileDead = H11Drawn 'h11-04-wiri-dead'
# started again, wiri shows (uncloaks) what the dead one had hidden; it tiles in its own order, so only the windows in view are drawn
$log11b = Join-Path $D 'wiri.stderr11b.log'
$w11b = Start-Wiri $log11b ($log11b + '.out')
Start-Sleep -Seconds 5
$h11CloakedAfterRestart = H11Cloaked
$h11VisibleAfterRestart = H11Visible
$h11DrawnAfterRestart = H11Drawn 'h11-05-after-restart'

# and a quit with windows cloaked puts everything back
$null = Ctl 'h11-ws2-third' @('switch-workspace', '2'); Start-Sleep -Milliseconds 2500
$h11CloakedBeforeQuit = H11Cloaked
$h11VisibleBeforeQuit = H11Visible
Stop-WiriGracefully $w11b 'h11b'
Start-Sleep -Milliseconds 1500
$h11CloakedAfterQuit = H11Cloaked
$h11VisibleAfterQuit = H11Visible
$h11DrawnAfterQuit = H11Drawn 'h11-06-after-quit'
$h11Values = [ordered]@{
    forms = @($cf11 | ForEach-Object { [int64]$_ })
    drawn_before = $h11DrawnBefore; in_switchers_before = $h11InSwitchersBefore
    cloaked_away = $h11Cloaked; visible_away = $h11Visible; in_switchers_away = $h11InSwitchersAway; drawn_away = $h11DrawnAway
    foreground_before_alt_tab = $h11ForegroundBefore; foreground_now = $h11ForegroundNow
    cloaked_after_alt_tab = $h11CloakedAfterAltTab; visible_after_alt_tab = $h11VisibleAfterAltTab; drawn_after_alt_tab = $h11DrawnAfterAltTab
    cloaked_before_crash = $h11CloakedBeforeCrash; cloaked_while_dead = $h11CloakedWhileDead; drawn_while_dead = $h11DrawnWhileDead
    cloaked_after_restart = $h11CloakedAfterRestart; visible_after_restart = $h11VisibleAfterRestart; drawn_after_restart = $h11DrawnAfterRestart
    cloaked_before_quit = $h11CloakedBeforeQuit; visible_before_quit = $h11VisibleBeforeQuit
    cloaked_after_quit = $h11CloakedAfterQuit; visible_after_quit = $h11VisibleAfterQuit; drawn_after_quit = $h11DrawnAfterQuit
}
$h11Values | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'h11-values.json')
foreach ($f in $cf11) { [TestForms]::CloseForm($f) }
Start-Sleep -Milliseconds 1500
# the instance that is left running for the rest of the script: the default method again
Remove-Item $cfg -Force -ErrorAction SilentlyContinue
$log11c = Join-Path $D 'wiri.stderr11c.log'
$w = Start-Wiri $log11c ($log11c + '.out')
Start-Sleep -Seconds 4

# ---- H12: the KDL config - niri's binds block loads as written, a mistake is reported with its place and the last good -----
# ----      config stays in use, a half-written save is never applied, includes are watched, any encoding loads ---------------
# The Phase A windows are still open; the binds write marker lines (cmd.exe appending to a file) so that "the bind ran" and
# "how many times" can be read back.  Everything is written the way a niri user would: a binds block with braces, raw strings,
# properties (repeat=false, cooldown-ms=, hotkey-overlay-title=), a /- commented bind and a block comment.
$log12 = Join-Path $D 'wiri.stderr12.log'
$f12 = @{ bind = (Join-Path $pdir 'h12-bind.txt'); cool = (Join-Path $pdir 'h12-cool.txt'); once = (Join-Path $pdir 'h12-once.txt');
          rep = (Join-Path $pdir 'h12-rep.txt'); new = (Join-Path $pdir 'h12-new.txt'); inc = (Join-Path $pdir 'h12-inc.txt');
          bom = (Join-Path $pdir 'h12-bom.txt'); u16 = (Join-Path $pdir 'h12-u16.txt'); extra = (Join-Path $pdir 'h12-extra.kdl') }
foreach ($p in $f12.Values) { Remove-Item $p -Force -ErrorAction SilentlyContinue }
$utf8NoBom = New-Object Text.UTF8Encoding($false)
function MarkerLines([string]$path) { if (Test-Path $path) { return @(Get-Content $path | Where-Object { $_ -ne '' }).Count } else { return 0 } }
function WaitText([string]$path, [string]$needle, [int]$sec) {
    for ($i = 0; $i -lt ($sec * 4); $i++) {
        if ((Test-Path $path) -and ((Get-Content $path -Raw -ErrorAction SilentlyContinue) -like "*$needle*")) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return $false
}
function CountText([string]$path, [string]$needle) {
    if (-not (Test-Path $path)) { return 0 }
    return ([regex]::Matches((Get-Content $path -Raw -ErrorAction SilentlyContinue), [regex]::Escape($needle))).Count
}
function H12Binds([string]$tag) {
    $raw = Ctl $tag @('--json', 'bindings')
    try { return @(($raw | ConvertFrom-Json).result) } catch { return @() }
}
function H12Has($entries, [int]$vkCode) { return (@($entries | Where-Object { $_.vk_code -eq $vkCode -and $_.modifiers -eq 3 }).Count -ge 1) }
# wiri-ctl called directly: the exit code and the text are the point, so the step list is left alone
function CtlRaw([string[]]$ctlArgs) {
    $outFile = Join-Path $D ('h12-raw-' + [guid]::NewGuid().ToString('N') + '.txt')
    $p = Start-Process -FilePath $ctl -ArgumentList $ctlArgs -RedirectStandardOutput $outFile -RedirectStandardError "$outFile.err" -WindowStyle Hidden -PassThru
    $null = $p.Handle
    if ($p.WaitForExit(5000)) { $rc = $p.ExitCode } else { Stop-Process -Id $p.Id -Force; $rc = -1 }
    $text = [string](Get-Content $outFile -Raw -ErrorAction SilentlyContinue) + [string](Get-Content "$outFile.err" -Raw -ErrorAction SilentlyContinue)
    return [pscustomobject]@{ rc = $rc; text = $text }
}
# Hold a chord down with the last key repeating, like a held key does.
function HoldChord([string]$spec, [int]$repeats) {
    $keys = @($spec.ToLower().Split('+') | ForEach-Object { [byte]$vk[$_] })
    foreach ($k in $keys) { [LT]::Key($k, $false); Start-Sleep -Milliseconds 40 }
    for ($i = 0; $i -lt $repeats; $i++) { Start-Sleep -Milliseconds 170; [LT]::Key($keys[$keys.Count - 1], $false) }
    Start-Sleep -Milliseconds 90
    for ($i = $keys.Count - 1; $i -ge 0; $i--) { [LT]::Key($keys[$i], $true); Start-Sleep -Milliseconds 30 }
}

$h12Good = @"
// H12: niri's syntax, as it is pasted from a niri config
input {
    mod-key "ctrl-alt"
}
layout {
    gaps 12
    border {
        width 3
        active-color "#83a598"
    }
}
binds {
    Mod+F1 hotkey-overlay-title="Write a marker" { spawn-sh r#"echo marker>> "$($f12.bind)""#; }
    Mod+F2 cooldown-ms=1500 { spawn-sh r#"echo cool>> "$($f12.cool)""#; }
    Mod+F3 repeat=false { spawn-sh r#"echo once>> "$($f12.once)""#; }
    Mod+F5 { spawn-sh r#"echo rep>> "$($f12.rep)""#; }
    /-Mod+F4 { spawn-sh "echo this bind is commented out"; }
    /* a block comment */ Mod+F6 { focus-workspace "2"; }
}
"@
Stop-WiriGracefully $w 'h12'
$w12 = Start-WiriWithConfig $h12Good $log12
$w = $w12
Shot 'h12-01-niri-config-loaded'
$h12b = H12Binds 'h12-bindings-good'
$h12BindsRegistered = (H12Has $h12b 0x70) -and (H12Has $h12b 0x71) -and (H12Has $h12b 0x72) -and (H12Has $h12b 0x74) -and (H12Has $h12b 0x75)
$h12SlashdashGone = -not (H12Has $h12b 0x73)
$h12Issues = (LogLines $log12 ': error: ').Count + (LogLines $log12 ': warning: ').Count

# the bind runs its action (a real chord)
Chord 'ctrl+alt+f1'
$h12BindRan = WaitFile $f12.bind 8

# cooldown-ms=1500: a second press 400 ms later is ignored, one after the cooldown fires
Chord 'ctrl+alt+f2'; Start-Sleep -Milliseconds 400
Chord 'ctrl+alt+f2'; Start-Sleep -Milliseconds 1200
$h12CoolAfterTwo = MarkerLines $f12.cool
Start-Sleep -Milliseconds 800
Chord 'ctrl+alt+f2'; Start-Sleep -Milliseconds 1200
$h12CoolAfterThird = MarkerLines $f12.cool

# repeat=false: holding the key down fires once; a bind without it fires for the repeats too
HoldChord 'ctrl+alt+f3' 6; Start-Sleep -Milliseconds 1200
$h12Once = MarkerLines $f12.once
HoldChord 'ctrl+alt+f5' 6; Start-Sleep -Milliseconds 1500
$h12Rep = MarkerLines $f12.rep

# wiri-ctl validate-config: the niri file is fine (the options wiri lacks are warnings), a mistake names its line and column
$h12GoodFile = Join-Path $pdir 'h12-good.kdl'
[IO.File]::WriteAllText($h12GoodFile, ($h12Good + "`nprefer-no-csd`nhotkey-overlay { skip-at-startup }`n"), $utf8NoBom)
$h12BadFile = Join-Path $pdir 'h12-bad.kdl'
$h12BadText = @"
layout {
    gaps 8
    gapz 9
}
binds {
    Mod+T { spawn "x"; }
    Mod+Y { clsoe-window; }
}
"@
[IO.File]::WriteAllText($h12BadFile, $h12BadText, $utf8NoBom)
$vGood = CtlRaw @('validate-config', (Q $h12GoodFile))
$vBad = CtlRaw @('validate-config', (Q $h12BadFile))
Set-Content (Join-Path $D 'h12-validate-good.txt') $vGood.text
Set-Content (Join-Path $D 'h12-validate-bad.txt') $vBad.text
$h12ValidateGood = ($vGood.rc -eq 0) -and ($vGood.text -match 'prefer-no-csd') -and ($vGood.text -notmatch ': error: ')
$h12ValidateBad = ($vBad.rc -eq 1) -and ($vBad.text -match 'h12-bad\.kdl:3:5: error: unknown option ''gapz''') -and
    ($vBad.text -match 'h12-bad\.kdl:7:\d+: error: bind Mod\+Y: unknown action ''clsoe-window''') -and ($vBad.text -match 'did you mean ''gaps''')

# a bad reload: the last good config stays, the log and the event stream say where, a balloon is asked for
$evOut = Join-Path $D 'h12-events.txt'
$evProc = Start-Process -FilePath $ctl -ArgumentList @('events', '--stream', '--filter', 'config_loaded') -RedirectStandardOutput $evOut -RedirectStandardError "$evOut.err" -WindowStyle Hidden -PassThru
Start-Sleep -Milliseconds 800
[IO.File]::WriteAllText($cfg, $h12BadText, $utf8NoBom)
$h12FailedEvent = WaitText $evOut '"failed":true' 8
Start-Sleep -Milliseconds 1200
Shot 'h12-02-after-a-bad-reload'
$h12b = H12Binds 'h12-bindings-after-bad-reload'
$h12KeptLastGood = (H12Has $h12b 0x70) -and (H12Has $h12b 0x71) -and (H12Has $h12b 0x72) -and (H12Has $h12b 0x74)
$h12FailedEventHasPlace = ((Get-Content $evOut -Raw -ErrorAction SilentlyContinue) -match 'config\.kdl:3:5')
$h12LogHasPlace = (@(LogLines $log12 'Config reload failed').Count -ge 1) -and (@(LogLines $log12 'config.kdl:3:5: error').Count -ge 1)
$h12BalloonFailed = (LogLines $log12 'balloon could not be shown').Count
$h12FailedEvents1 = CountText $evOut '"failed":true'
$markerBefore = MarkerLines $f12.bind
Chord 'ctrl+alt+f1'; Start-Sleep -Milliseconds 1500
$h12OldBindStillFires = ((MarkerLines $f12.bind) -eq ($markerBefore + 1))

# a save in two steps: half a file, 80 ms later the rest.  Only the finished file may be applied, and nothing is reported.
$h12New = @"
input { mod-key "ctrl-alt"; }
binds {
    Mod+F1 { spawn-sh r#"echo marker>> "$($f12.bind)""#; }
    Mod+F7 { spawn-sh r#"echo new>> "$($f12.new)""#; }
}
"@
$h12Half = $h12New.Substring(0, [int]($h12New.Length * 0.62))
$appliedBefore = CountText $evOut '"failed":false'
[IO.File]::WriteAllText($cfg, $h12Half, $utf8NoBom)
Start-Sleep -Milliseconds 80
[IO.File]::WriteAllText($cfg, $h12New, $utf8NoBom)
$h12Applied = WaitText $evOut '"failed":false' 8
Start-Sleep -Milliseconds 1200
$h12b = H12Binds 'h12-bindings-after-save'
$h12SaveApplied = (H12Has $h12b 0x76) -and -not (H12Has $h12b 0x71)
$h12NoFailureForTheSave = ((CountText $evOut '"failed":true') -eq $h12FailedEvents1) -and (((CountText $evOut '"failed":false') - $appliedBefore) -eq 1)
Chord 'ctrl+alt+f7'
$h12NewBindRan = WaitFile $f12.new 8

# any encoding: a UTF-8 file with a byte-order mark and CRLF, and a UTF-16 file (what PowerShell's Out-File writes)
$h12Bom = "binds {`r`n    Mod+F9 { spawn-sh r#`"echo bom>> `"$($f12.bom)`"`"#; }`r`n}`r`n"
[IO.File]::WriteAllText($cfg, $h12Bom, (New-Object Text.UTF8Encoding($true)))
Start-Sleep -Milliseconds 2200
$h12b = H12Binds 'h12-bindings-bom'
$h12BomLoaded = (H12Has $h12b 0x78) -and -not (H12Has $h12b 0x76)
$h12Utf16 = "binds {`r`n    Mod+F7 { spawn-sh r#`"echo u16>> `"$($f12.u16)`"`"#; }`r`n}`r`n"
[IO.File]::WriteAllText($cfg, $h12Utf16, [Text.Encoding]::Unicode)
Start-Sleep -Milliseconds 2200
$h12b = H12Binds 'h12-bindings-utf16'
$h12Utf16Loaded = (H12Has $h12b 0x76) -and -not (H12Has $h12b 0x78)

# include: a file in another directory, read where the include stands, and watched on its own
$h12Extra1 = "binds {`n    Mod+F6 { spawn-sh r#`"echo inc>> `"$($f12.inc)`"`"#; }`n}`n"
[IO.File]::WriteAllText($f12.extra, $h12Extra1, $utf8NoBom)
$h12Inc = "include `"$($f12.extra)`"`nbinds {`n    Mod+F1 { spawn-sh `"echo main`"; }`n}`n"
[IO.File]::WriteAllText($cfg, $h12Inc, $utf8NoBom)
Start-Sleep -Milliseconds 2200
$h12b = H12Binds 'h12-bindings-include'
$h12IncludeLoaded = (H12Has $h12b 0x75) -and (H12Has $h12b 0x70)
$h12Extra2 = "binds {`n    Mod+F9 { spawn-sh r#`"echo inc2>> `"$($f12.inc)`"`"#; }`n}`n"
[IO.File]::WriteAllText($f12.extra, $h12Extra2, $utf8NoBom)
Start-Sleep -Milliseconds 2200
$h12b = H12Binds 'h12-bindings-include-edited'
$h12IncludeWatched = (H12Has $h12b 0x78) -and -not (H12Has $h12b 0x75)

# wiri-ctl reload-config tells what is wrong with the file (the same parser)
[IO.File]::WriteAllText($cfg, $h12BadText, $utf8NoBom)
Start-Sleep -Milliseconds 2200
$rBad = CtlRaw @('reload-config')
Set-Content (Join-Path $D 'h12-reload-bad.txt') $rBad.text
$h12ReloadCommandSaysWhy = ($rBad.rc -ne 0) -and ($rBad.text -match 'config\.kdl:3:5')

if ($evProc -and -not $evProc.HasExited) { Stop-Process -Id $evProc.Id -Force }

# a config with a mistake at startup: wiri starts with its defaults and says why
$h12BadStart = "input {`n    repeat-delay 250`n    repeat-rat 25`n}`n"
Stop-WiriGracefully $w 'h12b'
$log12b = Join-Path $D 'wiri.stderr12b.log'
[IO.File]::WriteAllText($cfg, $h12BadStart, $utf8NoBom)
$w12b = Start-Wiri $log12b ($log12b + '.out')
Start-Sleep -Seconds 3
Shot 'h12-03-bad-config-at-startup'
Start-Sleep -Seconds 2
$w = $w12b
$h12b = H12Binds 'h12-bindings-bad-startup'
$h12DefaultsAtStartup = (H12Has $h12b 0x51) -and (@($h12b).Count -ge 50)
$h12StartupReported = (@(LogLines $log12b 'Using defaults (config error').Count -ge 1) -and (@(LogLines $log12b 'config.kdl:3:5: error').Count -ge 1)
$h12StartupBalloon = (@(LogLines $log12b 'Showed the config error balloon').Count -ge 1)
$h12Values = [ordered]@{
    binds_registered = $h12BindsRegistered; slashdash_gone = $h12SlashdashGone; issues_in_log = $h12Issues
    bind_ran = $h12BindRan; cool_after_two = $h12CoolAfterTwo; cool_after_third = $h12CoolAfterThird; once = $h12Once; rep = $h12Rep
    validate_good = $vGood.text; validate_bad = $vBad.text
    failed_event = $h12FailedEvent; kept_last_good = $h12KeptLastGood; event_has_place = $h12FailedEventHasPlace
    log_has_place = $h12LogHasPlace; balloon_failed = $h12BalloonFailed; old_bind_still_fires = $h12OldBindStillFires
    save_applied = $h12SaveApplied; no_failure_for_the_save = $h12NoFailureForTheSave; new_bind_ran = $h12NewBindRan
    bom = $h12BomLoaded; utf16 = $h12Utf16Loaded; include = $h12IncludeLoaded; include_watched = $h12IncludeWatched
    reload_command = $rBad.text; defaults_at_startup = $h12DefaultsAtStartup; startup_reported = $h12StartupReported; startup_balloon = $h12StartupBalloon
}
$h12Values | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'h12-values.json')
# the instance that is left running for the rest of the script: the default config again
Stop-WiriGracefully $w12b 'h12c'
Remove-Item $cfg -Force -ErrorAction SilentlyContinue
$log12c = Join-Path $D 'wiri.stderr12c.log'
$w = Start-Wiri $log12c ($log12c + '.out')
Start-Sleep -Seconds 4

# ---- H13: window rules that act -------------------------------------------------------------------------------------
# One config in niri's syntax, then windows whose titles the rules match.  What a rule did is read from the screen: the visible
# rectangle of the window (DWM's extended frame bounds), the alpha it is drawn with (GetLayeredWindowAttributes), the workspace
# `wiri-ctl windows` reports for it, whether it is visible and which window has the foreground.  Every test window is a form of
# this PowerShell process, so `app-id` has the process name `powershell.exe` to match.
$log13 = Join-Path $D 'wiri.stderr13.log'
$h13Monitors = Ctl 'h13-monitors' @('--json', 'list-monitors')
$h13Output = ''
try { $h13Output = [string]@(($h13Monitors | ConvertFrom-Json).result)[0].name } catch { }
$h13Template = @'
input {
    mod-key "ctrl-alt"
}
layout {
    gaps 10
    border {
        width 4
    }
}
workspace "web"
workspace "chat"

window-rule {
    match title="^h13-fixed"
    default-column-width { fixed 500; }
}
window-rule {
    match title="^h13-half"
    default-column-width { proportion 0.5; }
}
window-rule {
    match title="^h13-max"
    open-maximized true
}
window-rule {
    match title="^h13-limit"
    max-width 420
    max-height 400
}
window-rule {
    match app-id=r#"^powershell(\.exe)?$"# title="^h13-appid"
    min-width 333
}
window-rule {
    match app-id=r#"^notepad$"# title="^h13-notappid"
    min-width 333
}
window-rule {
    match title="^h13-or-a"
    match title="^h13-or-b"
    exclude title="private"
    opacity 0.5
}
window-rule {
    match title="^h13-focus"
    exclude is-focused=true
    opacity 0.6
}
window-rule {
    match title="^h13-chat"
    open-on-workspace "chat"
}
window-rule {
    match title="^h13-num"
    open-in-workspace 3
}
window-rule {
    match title="^h13-float"
    open-floating true
    default-column-width { fixed 420; }
    default-window-height { fixed 300; }
}
window-rule {
    match title="^h13-full"
    open-fullscreen true
}
window-rule {
    match title="^h13-quiet"
    open-focused false
}
window-rule {
    match title="^h13-out-ok"
    open-on-output r#"@OUTPUT@"#
    default-column-width { fixed 480; }
}
window-rule {
    match title="^h13-out-no"
    open-on-output "NoSuchDisplay"
    default-column-width { fixed 460; }
}
'@
$h13Config = $h13Template.Replace('@OUTPUT@', $h13Output)
Stop-WiriGracefully $w 'h13'
$w13 = Start-WiriWithConfig $h13Config $log13
$w = $w13
$h13Issues = (LogLines $log13 ': error: ').Count + (LogLines $log13 ': warning: ').Count
$h13View = $swa.Width - 20            # the work area less the 10 px outer gap on each side
function H13Open([string]$title, [int]$green) { $h = [TestForms]::Open($title, 90, $green, 160); Start-Sleep -Milliseconds 2200; return $h }
$h13All = @()

# (a) default-column-width: a fixed size is the window's (the border comes on top), a proportion is niri's
$h13Fixed = H13Open 'h13-fixed' 60;  $h13All += $h13Fixed
$h13Half = H13Open 'h13-half' 100;   $h13All += $h13Half
$rFixed = Rc $h13Fixed
$rHalf = Rc $h13Half
$h13FixedWidth = (Near $rFixed.W 500 4)
$h13HalfWidth = (Near $rHalf.W (0.5 * ($h13View + 10) - 10 - 8) 6)
Shot 'h13-01-default-widths'

# (b) open-maximized: the column is as wide as the view
$h13Max = H13Open 'h13-max' 140;     $h13All += $h13Max
$rMax = Rc $h13Max
$h13MaxFillsView = (Near $rMax.W ($h13View - 8) 6)

# (c) min / max size: a column asked to be wider than max-width stays at it; the limits follow the title of the window
$h13Lim = H13Open 'h13-limit' 180;   $h13All += $h13Lim
$null = Ctl 'h13-wide' @('set-column-width', '900'); Start-Sleep -Milliseconds 1500
$rLim = Rc $h13Lim
$h13LimitWidth = (Near $rLim.W 420 4)
$h13LimitHeight = (Near $rLim.H 400 4)
Shot 'h13-02-size-limits'
[TestForms]::SetTitle($h13Lim, 'h13-free now'); Start-Sleep -Milliseconds 1800
$rFree = Rc $h13Lim
$h13LimitsLifted = ($rFree.W -gt 800) -and ($rFree.H -gt 500)
[TestForms]::SetTitle($h13Lim, 'h13-limit again'); Start-Sleep -Milliseconds 1800
$rAgain = Rc $h13Lim
$h13LimitsBack = (Near $rAgain.W 420 4) -and (Near $rAgain.H 400 4)

# (d) app-id is a regex searched in the process name: min-width applies to the powershell.exe window, not to the one whose
# rule wants notepad
$h13P = H13Open 'h13-appid' 200;     $h13All += $h13P
$null = Ctl 'h13-narrow-1' @('set-column-width', '100'); Start-Sleep -Milliseconds 1500
$rP = Rc $h13P
$h13AppIdMatches = ($rP.W -ge 325)
$h13N = H13Open 'h13-notappid' 220;  $h13All += $h13N
$null = Ctl 'h13-narrow-2' @('set-column-width', '100'); Start-Sleep -Milliseconds 1500
$rN = Rc $h13N
$h13AppIdNoMatch = ($rN.W -lt 250)

# (e) several match lines are OR-ed, an exclude line vetoes; the rules follow the title of the window
$oA = H13Open 'h13-or-a' 30;         $h13All += $oA
$oB = H13Open 'h13-or-b' 60;         $h13All += $oB
$oC = H13Open 'h13-or-c' 90;         $h13All += $oC
$oP = H13Open 'h13-or-a private' 120; $h13All += $oP
$h13OrFirstLine = (Near ([LT]::Alpha($oA)) 128 2)
$h13OrSecondLine = (Near ([LT]::Alpha($oB)) 128 2)
$h13OrNoLine = ([LT]::Alpha($oC) -eq 255)
$h13ExcludeVetoes = ([LT]::Alpha($oP) -eq 255)
Shot 'h13-03-opacity'
[TestForms]::SetTitle($oC, 'h13-or-a renamed'); Start-Sleep -Milliseconds 1500
$h13OpacityFollowsTitle = (Near ([LT]::Alpha($oC)) 128 2)
[TestForms]::SetTitle($oA, 'h13-or-a private now'); Start-Sleep -Milliseconds 1500
$h13OpacityLeavesWhenExcluded = ([LT]::Alpha($oA) -eq 255)

# (f) a rule that depends on the focus (exclude is-focused=true): the window without the focus is dimmed, and it follows
$fA = H13Open 'h13-focus-a' 150;     $h13All += $fA
$fB = H13Open 'h13-focus-b' 170;     $h13All += $fB
$h13FocusDimsTheOther = (Near ([LT]::Alpha($fA)) 153 2) -and ([LT]::Alpha($fB) -eq 255)
FocusForm $fA
$h13FocusFollows = ([LT]::Alpha($fA) -eq 255) -and (Near ([LT]::Alpha($fB)) 153 2)
Shot 'h13-04-focus-dimming'

# (g) open-on-workspace "chat": the window opens on that workspace, the user stays where they are
$wChat = H13Open 'h13-chat' 190;     $h13All += $wChat
$lc = WinList 'h13-windows-chat'
$h13ChatWorkspace = ((WinOf $lc $wChat).workspace -eq 2)
$h13ChatHidden = (-not [LT]::IsWindowVisible($wChat))
$h13ChatKeepsFocus = ([LT]::GetForegroundWindow() -ne $wChat) -and ((WinOf $lc $fA).workspace -eq 1) -and [LT]::IsWindowVisible($fA)
$null = Ctl 'h13-go-chat' @('action', 'focus-workspace', 'chat'); Start-Sleep -Milliseconds 1500
$h13ChatShowsWhenVisited = [LT]::IsWindowVisible($wChat)
Shot 'h13-05-chat-workspace'
$null = Ctl 'h13-go-back' @('action', 'focus-workspace', '1'); Start-Sleep -Milliseconds 1500
# open-in-workspace 3: the workspace at that position (the empty one below the two named ones)
$wNum = [TestForms]::Open('h13-num', 90, 200, 160); Start-Sleep -Milliseconds 2200; $h13All += $wNum
$ln = WinList 'h13-windows-num'
$h13NumWorkspace = ((WinOf $ln $wNum).workspace -eq 3) -and (-not [LT]::IsWindowVisible($wNum))

# (h) open-floating with a default size: floats, at that size, centred
$h13F = H13Open 'h13-float' 210;     $h13All += $h13F
$rF = Rc $h13F
$lf = WinList 'h13-windows-float'
$h13FloatOpens = ((WinOf $lf $h13F).floating -eq $true)
$h13FloatSize = (Near $rF.W 420 6) -and (Near $rF.H 300 6)
$h13FloatCentered = (Near (($rF.L + $rF.R) / 2) (($swa.Left + $swa.Right) / 2) 8) -and (Near (($rF.T + $rF.B) / 2) (($swa.Top + $swa.Bottom) / 2) 8)
Shot 'h13-06-open-floating'

# (i) open-fullscreen: the window covers the monitor from the start, and leaves fullscreen like any other
$h13Full = H13Open 'h13-full' 230;   $h13All += $h13Full
$rFull = Rc $h13Full
$h13FullscreenOpens = ($rFull.L -le $sb.Left + 1) -and ($rFull.T -le $sb.Top + 1) -and ($rFull.R -ge $sb.Right - 1) -and ($rFull.B -ge $sb.Bottom - 1)
Shot 'h13-07-open-fullscreen'
$null = Ctl 'h13-fullscreen-off' @('action', 'fullscreen-window'); Start-Sleep -Milliseconds 1800
$rBack = Rc $h13Full
$h13FullscreenLeaves = ($rBack.W -lt ($sb.Width - 60))
[TestForms]::CloseForm($h13Full); Start-Sleep -Milliseconds 1500

# (j) open-focused false: the window that had the focus keeps it
FocusForm $fA
$h13Q = H13Open 'h13-quiet' 250;     $h13All += $h13Q
Start-Sleep -Milliseconds 800
$lq = WinList 'h13-windows-quiet'
$h13QuietKeepsFocus = ([LT]::GetForegroundWindow() -eq $fA)
$h13QuietIsTiled = ($null -ne (WinOf $lq $h13Q)) -and ((WinOf $lq $h13Q).column -ge 0) -and ((WinOf $lq $h13Q).workspace -eq 1)

# (k) open-on-output: the name wiri-ctl list-monitors gives places the window, one that no monitor has is ignored
$h13OutOk = H13Open 'h13-out-ok' 20;  $h13All += $h13OutOk
$h13OutNo = H13Open 'h13-out-no' 40;  $h13All += $h13OutNo
$h13OutputNameReported = ($h13Output -match '^\\\\\.\\DISPLAY\d+$')
$h13OutputPlaces = (Near (Rc $h13OutOk).W 480 4) -and (OnScreen $h13OutOk)
$h13OutputUnknownIgnored = (Near (Rc $h13OutNo).W 460 4) -and (OnScreen $h13OutNo)

# (l) a reload reaches the windows that are open: the opacity of the rule changes under them
$h13Reloaded = $h13Config.Replace('opacity 0.5', 'opacity 0.3')
[IO.File]::WriteAllText($cfg, $h13Reloaded, $utf8NoBom)
Start-Sleep -Milliseconds 2800
$h13ReloadUpdatesOpenWindows = (Near ([LT]::Alpha($oB)) 77 2) -and (Near ([LT]::Alpha($oC)) 77 2) -and ([LT]::Alpha($oA) -eq 255)
Shot 'h13-08-after-reload'
$h13Values = [ordered]@{
    output = $h13Output; view = $h13View; issues = $h13Issues
    fixed_w = $rFixed.W; half_w = $rHalf.W; max_w = $rMax.W; limit = "$($rLim.W)x$($rLim.H)"; free = "$($rFree.W)x$($rFree.H)"; again = "$($rAgain.W)x$($rAgain.H)"
    appid_w = $rP.W; notappid_w = $rN.W
    alpha_or = @([LT]::Alpha($oA), [LT]::Alpha($oB), [LT]::Alpha($oC), [LT]::Alpha($oP))
    float = "$($rF.W)x$($rF.H) at $($rF.L),$($rF.T)"; fullscreen = "$($rFull.L),$($rFull.T),$($rFull.R),$($rFull.B)"; full_back_w = $rBack.W
    chat_workspace = (WinOf $lc $wChat).workspace; num_workspace = (WinOf $ln $wNum).workspace
}
$h13Values | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'h13-values.json')
foreach ($h in $h13All) { [TestForms]::CloseForm($h) }
Start-Sleep -Milliseconds 2000
# the instance that is left running for the rest of the script: the default config again
Stop-WiriGracefully $w13 'h13b'
Remove-Item $cfg -Force -ErrorAction SilentlyContinue
$log13b = Join-Path $D 'wiri.stderr13b.log'
$w = Start-Wiri $log13b ($log13b + '.out')
Start-Sleep -Seconds 4

# ---- Phase D: close one window, quit -----------------------------------------------------------
[TestForms]::CloseForm($forms[1])
Start-Sleep -Seconds 2
Ctl '06-windows-after-close' @('windows') | Out-Null
Shot '06-after-close'

Ctl '07-quit' @('quit') | Out-Null
Start-Sleep -Seconds 3
$alive = -not $w.HasExited
$remaining = @($forms | Where-Object { [LT]::IsWindow($_) })
$visibleAfterQuit = OnScreenCount $remaining
FormInfo $remaining | ConvertTo-Json | Set-Content (Join-Path $D '07-forms-after-quit.json')
Shot '07-after-quit'
if ($alive) { Stop-Process -Id $w.Id -Force }
foreach ($f in $forms) { if ([LT]::IsWindow($f)) { [TestForms]::CloseForm($f) } }
if ([LT]::IsWindow($hung)) { [TestForms]::CloseForm($hung) }

# ---- summary ---------------------------------------------------------------------------------
$initialRects = [TestForms]::Initial
$formsInfo = @(Get-Content (Join-Path $D '02-forms.json') -Raw | ConvertFrom-Json | ForEach-Object { $_ })   # PS 5.1 hands back the JSON array as one object
$formsTiled = @($formsInfo | Where-Object {
    $init = $initialRects[$_.title]
    $cur = $_.rect -split ','
    $null -ne $init -and ( ([int]$cur[2] - [int]$cur[0]) -ne ($init.R - $init.L) -or ([int]$cur[3] - [int]$cur[1]) -ne ($init.B - $init.T) )
}).Count

$posBuckets = Buckets $log1 'Positioning window'
$actBuckets = Buckets $log1 'activate_window'
$ctlOut = @(Get-ChildItem $D -Filter '*.txt' | Where-Object { $_.Name -match '^(01|02|02b|03|04|05|06|07|08|09)-' })
$ctlNonAscii = 0
foreach ($f in $ctlOut) {
    $t = Get-Content $f.FullName -Raw -Encoding UTF8 -ErrorAction SilentlyContinue
    if ($t) { $t = $t.TrimStart([char]0xFEFF) }
    if ($t -and ($t -match '[^\u0000-\u007F]')) { $ctlNonAscii++ }
}
$wiriLog = Join-Path $wiriData 'wiri.log'
$allLogs = @($log1, $log2)
$summary = [ordered]@{
    settle_positioning_per_s = $settleRate
    test_windows_moved_by_wiri = $formsTiled
    ctl_timeouts = @($steps | Where-Object { $_.rc -eq 'timeout' }).Count
    unknown_key_warnings = (($allLogs | ForEach-Object { (LogLines $_ 'unknown key').Count } | Measure-Object -Sum).Sum)
    config_bind_warnings = (($allLogs | ForEach-Object { (LogLines $_ 'Config bind:').Count } | Measure-Object -Sum).Sum)
    wiri_exited_on_quit = -not $alive
    windows_visible_after_quit = $visibleAfterQuit
    windows_expected_after_quit = @($remaining).Count
    # --- added: layout of the three windows
    forms_inside_work_area = $inside
    forms_overlapping_pairs = $overlaps
    forms_min_visible_width = $minWidth
    work_area = "$($wa.Left),$($wa.Top),$($wa.Right),$($wa.Bottom)"
    foreground_after_open = $fgTitle
    # --- added: loop checks (terminal windows are what used to start it)
    terminals_settle_positioning_per_s = $termSettleRate
    terminal_windows_seen = @($termWindows).Count
    forms_on_screen_after_terminals_closed = $formsOnScreenAfterTerminals
    max_positioning_per_s = (MaxPerSecond $posBuckets 2)
    positioning_lines_total = (PositioningLines $log1)
    max_activate_lines_per_s = (MaxPerSecond $actBuckets 2)
    activation_failure_lines = ((LogLines $log1 'Could not bring').Count + (LogLines $log1 'failed to bring').Count)
    activation_ok_lines = (LogLines $log1 'activate_window:').Count
    # --- added: console / output / logging
    wiri_subsystem = $subsystem
    wiri_ctl_subsystem = $ctlSubsystem
    wiri_visible_windows_of_its_own = $wiriNewWindows
    wiri_new_console_windows_at_start = $wiriNewConsole
    wiri_exe_titled_windows_listed = @(Get-Content (Join-Path $D '02-windows.txt') -ErrorAction SilentlyContinue | Where-Object { $_ -match 'wiri\.exe' }).Count
    ctl_output_files_with_non_ascii = $ctlNonAscii
    wiri_log_exists = (Test-Path $wiriLog)
    wiri_log_bytes = $(if (Test-Path $wiriLog) { (Get-Item $wiriLog).Length } else { 0 })
    ctl_max_ms = (($steps | Where-Object { $_.ms } | Measure-Object -Property ms -Maximum).Maximum)
    ctl_max_ms_while_an_app_hung = $hungMaxMs
    ctl_failures_while_an_app_hung = $hungFailures
    hung_window_is_managed_afterwards = $hungWindowTiled
    # --- added: workspaces and crash safety
    windows_hidden_on_workspace_2 = $hiddenOnWs2
    windows_visible_back_on_workspace_1 = $visibleOnWs1
    hidden_json_written_before_kill = $hiddenJsonBeforeKill
    windows_hidden_while_wiri_dead = $hiddenWhileDead
    windows_visible_after_restart = $visibleAfterRestart
    hidden_json_present_after_restart = $hiddenJsonAfterRestart
    # --- added: real key presses
    escape_reaches_applications = ($keysSeen -match 'Escape,A,Escape,')
    overview_escape_responsive = $overviewResponsive
    hotkey_workspace_hidden = $hotkeyHidden
    hotkey_workspace_back_visible = $hotkeyBack
    # --- Phase 1 / H1: modifiers
    h1_unknown_modifier_warnings = (LogLines $log3 'unknown modifier').Count
    h1_unsupported_modifier_warnings = (LogLines $log3 'cannot be bound on Windows').Count
    h1_rejected_binds_registered = $h1StolenKeys
    h1_mod_bind_is_ctrl_alt = ($h1ModBind -eq 1)
    h1_f9_f8_reach_application = (($h1KeysSeen -match 'F9,') -and ($h1KeysSeen -match 'F8,'))
    # --- Phase 1 / H2: spawn
    h2_default_terminal_opens = $spawnDefaultTerminal
    h2_startup_args_ok = $spawnStartupArgsOk
    h2_bind_args_ok = $spawnBindArgsOk
    h2_shell_ok = $spawnShellOk
    h2_ipc_args_ok = $spawnIpcArgsOk
    h2_ipc_command_line_with_spaced_program_ok = $spawnIpcLineOk
    h2_wt_exe_found_on_path = $spawnWtWindow
    # --- Phase 1 / H3: actions target the real foreground window
    h3_float_keeps_foreground = $h3FloatKeepsForeground
    h3_float_reported_floating = $h3ReportedFloating
    h3_second_press_tiles_the_same_window = $h3SecondPressTiles
    h3_second_press_floats_nothing_else = $h3SecondPressFloatsNothingElse
    h3_tiled_again_has_column_height = $h3TiledAgainHasColumnHeight
    h3_float_geometry_remembered = $h3FloatGeometryRemembered
    h3_float_keeps_foreground_after_retoggle = $h3ForegroundAfterThirdPress
    h3_float_keeps_own_size = $h3FloatOwnSize
    h3_float_is_centered = $h3FloatCentered
    h3_move_nudges_float_50px = $h3FloatNudged
    h3_fullscreen_covers_monitor = $h3FullscreenCovers
    h3_fullscreen_only_the_float = ($h3FullscreenOthers -eq 0)
    h3_fullscreen_exit_restores_float = $h3FullscreenRestores
    h3_ipc_focus_window_tile = $h3IpcFocusTile
    h3_ipc_focus_window_float = $h3IpcFocusFloat
    h3_ipc_action_centers_float = $h3IpcCenters
    h3_close_closes_the_float = $h3FloatClosed
    h3_tiles_survive_float_close = $h3TilesSurviveFloatClose
    h3_dialog_was_foreground = $h3DialogForeground
    h3_close_hits_dialog_not_owner = ($h3DialogClosed -and $h3OwnerSurvives)
    h3_tool_window_was_foreground = $h3ToolForeground
    h3_close_refuses_unmanaged_foreground = $h3CloseRefusedOnUnmanaged
    # --- Phase 1 / H4: minimized windows
    h4_three_windows_in_order = $h4OrderBefore
    h4_minimized_window_is_iconic = $h4Iconic
    h4_neighbours_close_the_gap = $h4NoGap
    h4_ipc_reports_minimized = $h4IpcMinimized
    h4_restored_between_its_neighbours = $h4RestoredBetween
    h4_restored_keeps_its_size = $h4RestoredSize
    h4_ipc_reports_restored = $h4IpcNotMinimized
    h4_all_minimized = $h4AllIconic
    h4_all_restored = $h4AllRestored
    h4_old_order_kept_after_restoring_in_another_order = $h4OrderKept
    h4_focus_window_restores_a_minimized_window = $h4FocusRestores
    h4_closed_while_minimized_is_gone = $h4ClosedGone
    h4_closed_while_minimized_is_forgotten = $h4ClosedForgotten
    h4_neighbours_stay_after_closing_a_minimized_window = $h4NeighboursStay
    # --- Phase 1 / H5: floating windows and workspaces
    h5_float_on_top_when_it_floats = $h5FloatOnTop
    h5_ipc_reports_float_on_workspace_1 = $h5IpcWorkspace1
    h5_float_hidden_on_another_workspace = $h5HiddenOnWs2
    h5_float_shown_again_at_its_place = $h5ShownOnWs1
    h5_float_on_top_after_returning = $h5OnTopAfterReturn
    h5_overlapping_tile_got_the_focus = $h5TileGotFocus
    h5_float_stays_above_the_focused_tile = $h5FloatAboveFocusedTile
    h5_float_stays_above_after_workspace_round_trip = $h5FloatAboveAfterRoundTrip
    h5_move_to_workspace_hides_the_float_here = $h5MovedAwayHidden
    h5_ipc_reports_float_on_workspace_2 = $h5IpcWorkspace2
    h5_float_shown_on_its_new_workspace_at_the_same_place = $h5ShownOnWs2
    h5_float_hidden_again_on_workspace_1 = $h5HiddenAgainOnWs1
    h5_focus_window_switches_to_the_float_workspace = $h5FocusSwitchesWorkspace
    h5_float_closed = $h5Closed
    # --- Phase 1 / H6: niri column keymap
    h6_niri_keymap_binds_registered = $h6BindsRegistered
    h6_three_single_window_columns = $h6ThreeColumns
    h6_consume_or_expel_left_joins_the_left_column = $h6ConsumeLeft
    h6_emptied_column_vanishes = $h6EmptiedColumnVanished
    h6_focus_stays_on_the_moved_window = $h6FocusStaysOnMovedWindow
    h6_consume_or_expel_left_expels_to_the_left = $h6ExpelLeft
    h6_consume_or_expel_right_joins_the_right_column = $h6ConsumeRight
    h6_consume_or_expel_right_expels_to_the_right = $h6ExpelRight
    h6_consume_window_into_column_takes_the_right_neighbour = $h6ConsumeInto
    h6_consume_keeps_the_focus = $h6ConsumeIntoFocusStays
    h6_expel_takes_the_bottom_window = $h6ExpelFrom
    h6_expel_keeps_the_focus = $h6ExpelFromFocusStays
    h6_move_window_up = $h6MoveUp
    h6_move_window_keeps_the_focus = $h6MoveKeepsFocus
    h6_move_window_down = $h6MoveDown
    h6_focus_window_up_name = $h6FocusWindowUp
    h6_focus_window_down_name = $h6FocusWindowDown
    h6_focus_column_first = $h6FocusFirst
    h6_focus_column_last = $h6FocusLast
    h6_move_column_right_swaps_the_columns = $h6MoveColumnRight
    h6_move_column_left_swaps_them_back = $h6MoveColumnLeft
    h6_move_column_to_last = $h6ColumnToLast
    h6_move_column_to_first = $h6ColumnToFirst
    h6_maximize_column_full_width_for_a_one_window_column = $h6MaximizeFull
    h6_maximize_column_restores_the_width = $h6MaximizeRestores
    # --- Phase 1 / H7: niri action names and argument grammar
    h7_config_bind_warnings = $h7BindWarnings
    h7_niri_binds_registered = $h7BindsRegistered
    h7_set_column_width_percent = $h7SetPercent
    h7_set_column_width_plus_percent = $h7AdjustPercentUp
    h7_set_column_width_minus_percent = $h7AdjustPercentDown
    h7_set_column_width_pixels = $h7SetPixels
    h7_set_column_width_plus_pixels = $h7AdjustPixels
    h7_two_50_percent_columns_fill_the_view = $h7HalvesFillTheView
    h7_switch_preset_column_width_cycles_and_goes_back = $h7PresetCycle
    h7_focus_workspace_number_is_an_index = $h7WorkspaceIndex
    h7_focus_workspace_index_back = $h7WorkspaceIndexBack
    h7_focus_workspace_quoted_number_is_a_name = $h7QuotedIsAName
    h7_fullscreen_window = $h7FullscreenWindow
    h7_fullscreen_window_off = $h7FullscreenWindowOff
    h7_toggle_window_floating = $h7ToggleWindowFloating
    h7_move_window_to_floating_is_no_toggle = $h7ToFloatingIsNoToggle
    h7_move_window_to_tiling = $h7ToTiling
    h7_move_window_to_tiling_is_no_toggle = $h7ToTilingIsNoToggle
    h7_focus_monitor_left_at_the_edge_keeps_the_focus = $h7FocusMonitorAtTheEdge
    h7_set_window_height_percent = $h7WindowHeightPercent
    h7_set_window_height_minus_percent = $h7WindowHeightAdjust
    h7_reset_window_height = $h7WindowHeightReset
    h7_wiri_ctl_set_column_width = $h7CtlSetColumnWidth
    h7_wiri_ctl_set_column_width_minus_percent = $h7CtlSetColumnWidthMinus
    # --- Phase 1 / H8: the workspace model
    h8_config_bind_warnings = $h8BindWarnings
    h8_named_workspaces_exist_from_the_start = $h8NamedExist
    h8_focus_workspace_name_of_an_empty_workspace = $h8WebIsFocusable
    h8_focus_workspace_previous_goes_back = $h8Previous
    h8_focus_workspace_3_is_the_empty_one_below = $h8ThirdIsTheEmptyOne
    h8_auto_back_and_forth = $h8BackAndForth
    h8_a_number_past_the_last_is_the_last = $h8NumberPastTheEnd
    h8_focus_workspace_down_does_not_wrap = $h8NoWrapAtTheBottom
    h8_back_on_the_first_workspace = $h8BackOnTheFirst
    h8_focus_workspace_up_does_not_wrap = $h8NoWrapAtTheTop
    h8_focus_window_switches_to_its_workspace = $h8SwitchedToItsWorkspace
    h8_number_counts_positions_after_a_workspace_went = $h8ThirdIsFormTwo
    # --- Phase 1 / H9: Phase 0 leftovers
    h9_failed_default_hotkeys = $h9FailedHotkeys
    h9_default_hotkeys_registered = ($h9RegisteredHotkeys -ge 60)
    h9_focus_window_focused_the_first_form = $h9FocusedA
    h9_ctrl_alt_grave_goes_back_to_the_previous_window = $h9GraveGoesBack
    h9_fullscreen_covers_exactly_the_monitor = $h9FullscreenExact
    h9_fullscreen_off_gives_the_tile_back = $h9FullscreenOffTiles
    h9_tile_after_fullscreen_is_aligned_with_its_neighbour = $h9TileAfterFullscreenAligned
    h9_three_terminals_opened = $h9TerminalsOpened
    h9_overlapping_terminal_pairs = $h9Overlaps
    h9_a_window_was_parked_when_wiri_died = ($h9Parked -ge 1)
    h9_originals_recorded_before_the_crash = ($h9OriginalsBefore -eq $cf.Count)
    h9_wiri_had_moved_the_windows_before_the_crash = ($h9MovedByWiri -ge 3)
    h9_originals_survive_the_restart = ($h9OriginalsAfterRestart -eq $cf.Count)
    h9_every_window_back_where_it_was_after_crash_and_quit = ($h9Restored -eq $cf.Count)
    h9_record_gone_after_a_graceful_quit = $h9RecordGoneAfterQuit
    # --- Phase 1 / H11: hide-method "cloak"
    h11_config_accepted_without_warnings = ($h11Warnings -eq 0)
    h11_forms_drawn_before_hiding = (@($h11DrawnBefore | Where-Object { $_ -gt 1000 }).Count -eq 3)
    h11_shell_lists_the_forms_in_alt_tab_before = (@($h11InSwitchersBefore | Where-Object { $_ -eq 1 }).Count -eq 3)
    h11_other_workspace_windows_are_cloaked_not_hidden = ((@($h11Cloaked | Where-Object { $_ -gt 0 }).Count -eq 3) -and (@($h11Visible | Where-Object { $_ }).Count -eq 3))
    h11_cloaked_windows_are_not_drawn = (@($h11DrawnAway | Where-Object { $_ -eq 0 }).Count -eq 3)
    h11_cloaked_windows_stay_in_alt_tab_and_the_taskbar = (@($h11InSwitchersAway | Where-Object { $_ -eq 1 }).Count -eq 3)
    h11_hidden_json_marks_them_cloaked = ($h11RecordedCloaked -eq 3)
    h11_alt_tab_changed_the_foreground_window = ($h11ForegroundNow -ne $h11ForegroundBefore)
    h11_alt_tab_to_a_cloaked_window_brings_its_workspace = ((@($h11CloakedAfterAltTab | Where-Object { $_ -eq 0 }).Count -eq 3) -and (@($h11DrawnAfterAltTab | Where-Object { $_ -gt 1000 }).Count -eq 3))
    h11_windows_stay_cloaked_when_wiri_dies = ((@($h11CloakedBeforeCrash | Where-Object { $_ -gt 0 }).Count -eq 3) -and (@($h11CloakedWhileDead | Where-Object { $_ -gt 0 }).Count -eq 3) -and (@($h11DrawnWhileDead | Where-Object { $_ -eq 0 }).Count -eq 3))
    h11_restart_uncloaks_what_the_dead_wiri_hid = ((@($h11CloakedAfterRestart | Where-Object { $_ -eq 0 }).Count -eq 3) -and (@($h11VisibleAfterRestart | Where-Object { $_ }).Count -eq 3) -and (@($h11DrawnAfterRestart | Where-Object { $_ -gt 1000 }).Count -ge 1))
    h11_windows_are_cloaked_again_before_the_quit = ((@($h11CloakedBeforeQuit | Where-Object { $_ -gt 0 }).Count -eq 3) -and (@($h11VisibleBeforeQuit | Where-Object { $_ }).Count -eq 3))
    h11_quit_uncloaks_everything = ((@($h11CloakedAfterQuit | Where-Object { $_ -eq 0 }).Count -eq 3) -and (@($h11VisibleAfterQuit | Where-Object { $_ }).Count -eq 3))
    # --- Phase 2A / H12: the KDL config
    h12_generated_default_config_has_no_issues = (((LogLines $log1 ': error: ').Count + (LogLines $log1 ': warning: ').Count) -eq 0)
    h12_niri_binds_block_registered = $h12BindsRegistered
    h12_slashdashed_bind_is_not_registered = $h12SlashdashGone
    h12_niri_config_loads_without_issues = ($h12Issues -eq 0)
    h12_bind_runs_its_action = $h12BindRan
    h12_cooldown_ignores_a_press_inside_it = ($h12CoolAfterTwo -eq 1)
    h12_cooldown_ends = ($h12CoolAfterThird -eq 2)
    h12_repeat_false_fires_once = ($h12Once -eq 1)
    h12_binds_repeat_by_default = ($h12Rep -ge 3)
    h12_validate_config_accepts_a_niri_file = $h12ValidateGood
    h12_validate_config_names_line_and_column = $h12ValidateBad
    h12_bad_reload_gives_a_failed_config_loaded_event = $h12FailedEvent
    h12_bad_reload_keeps_the_last_good_binds = $h12KeptLastGood
    h12_failed_event_names_file_line_column = $h12FailedEventHasPlace
    h12_bad_reload_is_logged_with_its_place = $h12LogHasPlace
    h12_error_balloon_could_be_shown = ($h12BalloonFailed -eq 0)
    h12_old_bind_still_fires_after_a_bad_reload = $h12OldBindStillFires
    h12_half_written_save_is_applied_once_and_not_reported = ($h12SaveApplied -and $h12NoFailureForTheSave)
    h12_bind_of_the_new_config_runs = $h12NewBindRan
    h12_utf8_bom_config_loads = $h12BomLoaded
    h12_utf16_config_loads = $h12Utf16Loaded
    h12_include_binds_load = $h12IncludeLoaded
    h12_editing_an_include_reloads = $h12IncludeWatched
    h12_reload_config_command_says_why = $h12ReloadCommandSaysWhy
    h12_bad_startup_config_falls_back_to_defaults = $h12DefaultsAtStartup
    h12_bad_startup_config_is_reported = $h12StartupReported
    h12_bad_startup_balloon_was_shown = $h12StartupBalloon
    # --- Phase 2A / H13: window rules that act
    h13_config_loads_without_issues = ($h13Issues -eq 0)
    h13_monitor_name_is_reported = $h13OutputNameReported
    h13_default_column_width_fixed = $h13FixedWidth
    h13_default_column_width_proportion = $h13HalfWidth
    h13_open_maximized_fills_the_view = $h13MaxFillsView
    h13_max_width_and_height_hold_the_window = ($h13LimitWidth -and $h13LimitHeight)
    h13_limits_lift_when_the_title_stops_matching = $h13LimitsLifted
    h13_limits_return_when_the_title_matches_again = $h13LimitsBack
    h13_app_id_regex_matches_the_process_name = $h13AppIdMatches
    h13_app_id_regex_that_does_not_match_leaves_the_window_alone = $h13AppIdNoMatch
    h13_first_match_line_applies = $h13OrFirstLine
    h13_second_match_line_applies = $h13OrSecondLine
    h13_window_matching_no_line_is_untouched = $h13OrNoLine
    h13_exclude_line_vetoes = $h13ExcludeVetoes
    h13_opacity_follows_a_new_title = $h13OpacityFollowsTitle
    h13_opacity_leaves_when_the_title_is_excluded = $h13OpacityLeavesWhenExcluded
    h13_focus_rule_dims_the_window_without_focus = $h13FocusDimsTheOther
    h13_focus_rule_follows_the_focus = $h13FocusFollows
    h13_open_on_workspace_by_name = $h13ChatWorkspace
    h13_open_on_workspace_window_is_hidden = $h13ChatHidden
    h13_open_on_workspace_leaves_the_user_where_they_are = $h13ChatKeepsFocus
    h13_open_on_workspace_window_shows_when_visited = $h13ChatShowsWhenVisited
    h13_open_in_workspace_by_position = $h13NumWorkspace
    h13_open_floating_floats_the_window = $h13FloatOpens
    h13_open_floating_default_size = $h13FloatSize
    h13_open_floating_is_centred = $h13FloatCentered
    h13_open_fullscreen_covers_the_monitor = $h13FullscreenOpens
    h13_open_fullscreen_window_leaves_fullscreen = $h13FullscreenLeaves
    h13_open_focused_false_keeps_the_foreground = $h13QuietKeepsFocus
    h13_open_focused_false_window_is_tiled = $h13QuietIsTiled
    h13_open_on_output_with_a_real_name = $h13OutputPlaces
    h13_open_on_output_with_an_unknown_name_is_ignored = $h13OutputUnknownIgnored
    h13_reload_changes_the_opacity_of_open_windows = $h13ReloadUpdatesOpenWindows
    steps = $steps
}
([pscustomobject]$summary) | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'summary.json')
if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $D 'config.kdl.last') -Force }
if (Test-Path $wiriLog) { Copy-Item $wiriLog (Join-Path $D 'wiri.log.copy') -Force }
'done' | Set-Content (Join-Path $D 'DONE')
