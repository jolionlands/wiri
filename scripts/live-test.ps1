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
#          H4 minimized windows leave no gap.
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
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }

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
  public static void Key(byte vk, bool up) {
    uint f = 0;
    if ((vk >= 0x21 && vk <= 0x28) || vk == 0x2D || vk == 0x2E) f |= 1;
    if (up) f |= 2;
    keybd_event(vk, 0, f, UIntPtr.Zero);
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
$hiddenJsonAfterRestart = (Test-Path $hiddenJson)
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
    '1' = 0x31; '2' = 0x32; '3' = 0x33; '4' = 0x34; a = 0x41; f = 0x46; q = 0x51; t = 0x54
    left = 0x25; right = 0x27
    f6 = 0x75; f7 = 0x76; f8 = 0x77; f9 = 0x78
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
    Mod+F6 spawn-sh "echo shell ok> \"$pdir\bind-shell.txt\""
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
    steps = $steps
}
([pscustomobject]$summary) | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'summary.json')
if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $D 'config.kdl.last') -Force }
if (Test-Path $wiriLog) { Copy-Item $wiriLog (Join-Path $D 'wiri.log.copy') -Force }
'done' | Set-Content (Join-Path $D 'DONE')
