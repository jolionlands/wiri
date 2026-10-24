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
$vk = @{ ctrl = 0x11; alt = 0x12; esc = 0x1B; space = 0x20; '1' = 0x31; '2' = 0x32; a = 0x41 }
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
    steps = $steps
}
([pscustomobject]$summary) | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $D 'summary.json')
if (Test-Path $cfg) { Copy-Item $cfg (Join-Path $D 'config.kdl.generated') -Force }
if (Test-Path $wiriLog) { Copy-Item $wiriLog (Join-Path $D 'wiri.log.copy') -Force }
'done' | Set-Content (Join-Path $D 'DONE')
