# Lane D: one-shot PNG/clipboard screenshots, output selection and current-mode no-op.
# Dot-sourced after the existing phases, while the global desktop mutex is still held.
# No mode, scale, position, rotation or primary-display setters are used by this test.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneD {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool EnumDisplaySettingsW(string name, int mode, IntPtr devmode);
    [DllImport("user32.dll")] static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")] static extern bool CloseClipboard();
    [DllImport("user32.dll")] static extern IntPtr GetClipboardData(uint format);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern uint RegisterClipboardFormatW(string name);
    [DllImport("user32.dll")] static extern bool IsClipboardFormatAvailable(uint format);
    [DllImport("user32.dll")] public static extern uint GetClipboardSequenceNumber();
    [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr memory);
    [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr memory);
    [DllImport("user32.dll")] static extern IntPtr MonitorFromWindow(IntPtr window, uint flags);
    [DllImport("shcore.dll")] static extern int GetDpiForMonitor(IntPtr monitor, int type, out uint x, out uint y);
    public static int[] Mode(string name) {
        IntPtr p = Marshal.AllocHGlobal(220);
        try {
            Marshal.Copy(new byte[220], 0, p, 220); Marshal.WriteInt16(p, 68, 220);
            if (!EnumDisplaySettingsW(name, -1, p)) throw new Exception("Cannot read current mode");
            return new int[] { Marshal.ReadInt32(p,172), Marshal.ReadInt32(p,176), Marshal.ReadInt32(p,184),
                Marshal.ReadInt32(p,76), Marshal.ReadInt32(p,80), Marshal.ReadInt32(p,84), Marshal.ReadInt32(p,168) };
        } finally { Marshal.FreeHGlobal(p); }
    }
    public static double Scale(IntPtr hwnd) {
        uint x,y; if (GetDpiForMonitor(MonitorFromWindow(hwnd, 1),0,out x,out y) != 0) throw new Exception("Cannot read DPI");
        return x/96.0;
    }
    public static int[] Clipboard() {
        if (!OpenClipboard(IntPtr.Zero)) return new int[] {0,0,0};
        try {
            IntPtr memory = GetClipboardData(8); if (memory == IntPtr.Zero) return new int[] {0,0,0};
            IntPtr p = GlobalLock(memory); if (p == IntPtr.Zero) return new int[] {0,0,0};
            try { return new int[] {Marshal.ReadInt32(p,4),Math.Abs(Marshal.ReadInt32(p,8)),
                IsClipboardFormatAvailable(RegisterClipboardFormatW("PNG")) ? 1 : 0}; }
            finally { GlobalUnlock(memory); }
        } finally { CloseClipboard(); }
    }
}
'@
function DGate([string]$name, [bool]$value) { $summary[$name] = $value }
function DPng([string]$path) {
    if (-not (Test-Path $path)) { return @(0,0,0) }
    $bytes = [IO.File]::ReadAllBytes($path)
    if ($bytes.Length -lt 24 -or [BitConverter]::ToString($bytes,0,8) -ne '89-50-4E-47-0D-0A-1A-0A') { return @(0,0,0) }
    $w = [int64]$bytes[16]*16777216 + [int64]$bytes[17]*65536 + [int64]$bytes[18]*256 + $bytes[19]
    $h = [int64]$bytes[20]*16777216 + [int64]$bytes[21]*65536 + [int64]$bytes[22]*256 + $bytes[23]
    # Also decode the full image (not just its IHDR) through the OS image decoder.
    try { $image = [Drawing.Image]::FromFile($path); $ok = ($image.Width -eq $w -and $image.Height -eq $h); $image.Dispose() }
    catch { $ok = $false }
    return @($w,$h,[int]$ok)
}
# One `screenshot-window` capture of the fixture.  The bind captures the *foreground* window,
# so the fixture has to be foreground first; a session whose foreground is held by another
# (invisible) window makes the OS refuse wiri's activation, and the same action is then
# requested for the fixture's own id (`screenshot-window --id`, niri's form) so the gate still
# describes the fixture instead of whatever happened to be foreground.  Returns which path ran.
function DCaptureWindow([string]$tag) {
    FocusForm $dForm
    if ([LT]::GetForegroundWindow() -eq $dForm) {
        Chord 'ctrl+alt+f9'; Start-Sleep -Milliseconds 2200
        return 'bind'
    }
    $null = Ctl $tag @('action', 'screenshot-window', '--id', [string][int64]$dForm)
    Start-Sleep -Milliseconds 1400
    return 'id'
}
function DMonitors([string]$tag) { return @((Ctl $tag @('--json','list-monitors') | ConvertFrom-Json).result) }
function DConfig([string]$pathSetting, [string]$scaleSetting) {
    return @"
screenshot-path $pathSetting
animations { off }
output "not-a-connected-output" { off }
output r"$dConnector" {
    mode "$dCurrentMode"
    $scaleSetting
    focus-at-startup
}
binds {
    Ctrl+Alt+F8 { screenshot-screen; }
    Ctrl+Alt+F9 { screenshot-window; }
}
"@
}
# Preset false gates survive exceptions, rather than silently disappearing from summary.json.
foreach ($gate in @('h26_screen_is_a_decodable_png_of_focused_output','h26_template_is_expanded','h26_screen_is_on_clipboard',
    'h27_window_is_a_decodable_png_of_only_the_window','h27_window_contains_the_test_content','h27_window_is_on_clipboard',
    'h28_null_writes_no_files','h28_both_null_actions_still_copy','h28_screenshot_captured_reports_paths_and_null',
    'h29_connector_match_is_case_insensitive_not_by_index','h29_focus_at_startup_selects_the_output',
    'h29_reload_without_scale_preserves_real_dpi','h29_explicit_current_mode_does_not_change_the_desktop')) { DGate $gate $false }
$dProc = $null; $dEventsProc = $null; $dForm = [IntPtr]::Zero
try {
    $dScreen = [Windows.Forms.Screen]::PrimaryScreen
    $dBefore = [LaneD]::Mode($dScreen.DeviceName)
    $dConnector = $dScreen.DeviceName.ToLowerInvariant()
    # EnumDisplaySettings rounds Hz; omit refresh so CCD preserves the exact rational.
    $dCurrentMode = '{0}x{1}' -f $dBefore[0],$dBefore[1]
    $dDir = Join-Path $D 'lane-d-captures'
    New-Item -ItemType Directory -Force $dDir | Out-Null
    $dTemplate = (Join-Path $dDir 'screen_%Y-%m-%d_%H-%M-%S.png').Replace('\','/')
    $dText = DConfig ('"'+$dTemplate+'"') 'scale 1.25'
    $dLog = Join-Path $D 'wiri.lane-d.log'
    $dProc = Start-WiriWithConfig $dText $dLog
    $dForm = [TestForms]::Open('lane-d-capture', 43, 177, 89)
    Start-Sleep -Milliseconds 1600
    # Ask wiri for the focus and wait for it: `screenshot-window` captures the *foreground*
    # window, and SwitchToThisWindow alone silently left another window foreground (measured
    # 2026-10-31: a zero-sized handle whose capture was the whole screen), which made the
    # window-capture gates describe a window that is not the fixture.
    FocusForm $dForm
    $dFocused = ([LT]::GetForegroundWindow() -eq $dForm)
    $dMons = @(DMonitors 'h29-monitors-start')
    $dMatch = @($dMons | Where-Object { $_.name -ieq $dScreen.DeviceName })
    DGate 'h29_connector_match_is_case_insensitive_not_by_index' ($dMatch.Count -eq 1 -and [Math]::Abs($dMatch[0].scale_factor-1.25) -lt 0.001)
    DGate 'h29_focus_at_startup_selects_the_output' ($dMatch.Count -eq 1 -and $dMatch[0].focused)
    $dEvents = Join-Path $D 'h28-screenshot-events.txt'
    $dEventsProc = Start-Process -FilePath $ctl -ArgumentList @('events','--stream','--filter','screenshot_captured') -RedirectStandardOutput $dEvents -RedirectStandardError "$dEvents.err" -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 800
    $seq0 = [LaneD]::GetClipboardSequenceNumber()
    Chord 'ctrl+alt+f8'; Start-Sleep -Milliseconds 1800
    $dScreenFiles = @(Get-ChildItem $dDir -Filter 'screen_*.png')
    $dScreenPng = @(0,0,0)
    if ($dScreenFiles.Count -eq 1) { $dScreenPng = @(DPng $dScreenFiles[0].FullName) }
    $clipScreen = [LaneD]::Clipboard()
    DGate 'h26_screen_is_a_decodable_png_of_focused_output' ($dScreenPng[2] -eq 1 -and $dScreenPng[0] -eq $dBefore[0] -and $dScreenPng[1] -eq $dBefore[1])
    DGate 'h26_template_is_expanded' ($dScreenFiles.Count -eq 1 -and $dScreenFiles[0].Name -match '^screen_\d{4}-\d\d-\d\d_\d\d-\d\d-\d\d\.png$')
    DGate 'h26_screen_is_on_clipboard' ($clipScreen[0] -eq $dBefore[0] -and $clipScreen[1] -eq $dBefore[1] -and $clipScreen[2] -eq 1 -and [LaneD]::GetClipboardSequenceNumber() -ne $seq0)
    # Reload the template; the next capture must use it without restarting wiri.
    $dText = $dText.Replace('screen_%Y','window_%Y')
    [IO.File]::WriteAllText($cfg,$dText,(New-Object Text.UTF8Encoding($false)))
    Ctl 'h27-template-reload' @('reload-config') | Out-Null
    Start-Sleep -Milliseconds 1400
    FocusForm $dForm
    $dRect = [LT]::Visible($dForm)
    # Which window the capture is of: `screenshot-window` uses the focused window, so the
    # foreground handle is part of the evidence (a different window would change the image size).
    $dCaptureHwnd = [LT]::GetForegroundWindow()
    $dCaptureRect = [LT]::Visible($dCaptureHwnd)
    $dCaptureVia = DCaptureWindow 'h27-window-capture'
    $dWindowFiles = @(Get-ChildItem $dDir -Filter 'window_*.png')
    $dWindowPng = @(0,0,0)
    if ($dWindowFiles.Count -eq 1) { $dWindowPng = @(DPng $dWindowFiles[0].FullName) }
    $clipWindow = [LaneD]::Clipboard()
    DGate 'h27_window_is_a_decodable_png_of_only_the_window' ($dWindowPng[2] -eq 1 -and [Math]::Abs($dWindowPng[0]-($dRect.R-$dRect.L)) -le 20 -and [Math]::Abs($dWindowPng[1]-($dRect.B-$dRect.T)) -le 20 -and $dWindowPng[0] -lt $dBefore[0])
    if ($dWindowFiles.Count -eq 1) { DGate 'h27_window_contains_the_test_content' ([LT]::CountColor($dWindowFiles[0].FullName,43,177,89) -gt 10000) }
    DGate 'h27_window_is_on_clipboard' ($dWindowPng[2] -eq 1 -and $clipWindow[0] -eq $dWindowPng[0] -and $clipWindow[1] -eq $dWindowPng[1] -and $clipWindow[2] -eq 1)
    Shot 'h27-window-capture'
    # null must still populate the clipboard and emit an event for each action, without a file.
    $filesBefore = @(Get-ChildItem $dDir | ForEach-Object { $_.Name }) -join ','
    # The output scale stays at the h27 value: a window screenshot is the window's own pixels
    # at the moment of the action, and the scale decides that pixel size, so the null window
    # capture is only comparable with the earlier one while the scale is unchanged.  The
    # `reload without a scale line is the real DPI` gate below gets its own reload instead.
    $dNullConfig = DConfig 'null' 'scale 1.25'
    [IO.File]::WriteAllText($cfg,$dNullConfig,(New-Object Text.UTF8Encoding($false)))
    Ctl 'h28-null-reload' @('reload-config') | Out-Null
    Start-Sleep -Milliseconds 1500
    $dNullMons = @(DMonitors 'h28-monitors-null')
    FocusForm $dForm
    $seq1 = [LaneD]::GetClipboardSequenceNumber()
    Chord 'ctrl+alt+f8'; Start-Sleep -Milliseconds 1800
    $seq2 = [LaneD]::GetClipboardSequenceNumber(); $nullScreen = [LaneD]::Clipboard()
    FocusForm $dForm
    $dNullHwnd = [LT]::GetForegroundWindow()
    $dNullRect = [LT]::Visible($dNullHwnd)
    $dNullFormRect = [LT]::Visible($dForm)
    $dNullVia = DCaptureWindow 'h28-window-null'
    $seq3 = [LaneD]::GetClipboardSequenceNumber(); $nullWindow = [LaneD]::Clipboard()
    $filesAfter = @(Get-ChildItem $dDir | ForEach-Object { $_.Name }) -join ','
    DGate 'h28_null_writes_no_files' ($filesBefore -eq $filesAfter)
    DGate 'h28_both_null_actions_still_copy' ($seq1 -ne $seq2 -and $seq2 -ne $seq3 -and $nullScreen[0] -eq $dBefore[0] -and $nullWindow[0] -eq $dWindowPng[0] -and $nullWindow[2] -eq 1)
    $captured = @(EvOf (EvList $dEvents) 'screenshot_captured')
    $saved = @($captured | Where-Object { $null -ne $_.data.path })
    $clipboardOnly = @($captured | Where-Object { $null -eq $_.data.path })
    DGate 'h28_screenshot_captured_reports_paths_and_null' ($captured.Count -eq 4 -and $saved.Count -eq 2 -and $clipboardOnly.Count -eq 2 -and (Test-Path $saved[0].data.path) -and (Test-Path $saved[1].data.path))
    # A reload with no `scale` line at all: the output must fall back to the monitor's real DPI
    # (this config deliberately writes no scale).
    [IO.File]::WriteAllText($cfg,(DConfig 'null' ''),(New-Object Text.UTF8Encoding($false)))
    Ctl 'h29-reload-no-scale' @('reload-config') | Out-Null
    Start-Sleep -Milliseconds 1500
    $dReloaded = @(DMonitors 'h29-monitors-reloaded' | Where-Object { $_.name -ieq $dScreen.DeviceName })
    DGate 'h29_reload_without_scale_preserves_real_dpi' ($dReloaded.Count -eq 1 -and [Math]::Abs($dReloaded[0].scale_factor - [LaneD]::Scale($dForm)) -lt 0.001)
    $dAfter = [LaneD]::Mode($dScreen.DeviceName)
    DGate 'h29_explicit_current_mode_does_not_change_the_desktop' (($dBefore -join ',') -eq ($dAfter -join ','))
    [ordered]@{
        connector=$dScreen.DeviceName; before=$dBefore; after=$dAfter
        screen_png=$dScreenPng; window_png=$dWindowPng
        clipboard_screen=$clipScreen; clipboard_window=$clipWindow
        # The h28 null-path sub-checks, as observed: the three clipboard sequence numbers, the
        # two null captures, the window rectangle of the capture before and after the reload,
        # and the handle each `screenshot-window` action actually captured.
        clipboard_seq=[ordered]@{ before=$seq0; null_screen=$seq1; after_null_screen=$seq2; after_null_window=$seq3 }
        null_screen=$nullScreen; null_window=$nullWindow
        window_png_size=@($dWindowPng[0],$dWindowPng[1])
        capture_hwnd=[int64]$dCaptureHwnd; capture_rect=$dCaptureRect; capture_via=$dCaptureVia
        null_capture_hwnd=[int64]$dNullHwnd; null_capture_rect=$dNullRect; null_capture_via=$dNullVia
        null_form_rect=$dNullFormRect; form_focused_at_start=[bool]$dFocused
        scale_after_null=@($dNullMons | ForEach-Object { [double]$_.scale_factor })
        form_hwnd=[int64]$dForm; form_rect=$dRect; form_png=$dWindowFiles.Count
        screen_png_files=@($dScreenFiles | ForEach-Object { $_.Name })
        window_png_files=@($dWindowFiles | ForEach-Object { $_.Name })
        events=$captured
    } | ConvertTo-Json -Depth 6 | Set-Content (Join-Path $D 'lane-d-values.json')
} catch {
    ($_ | Out-String) | Set-Content (Join-Path $D 'lane-d-error.txt')
} finally {
    if ($dEventsProc -and -not $dEventsProc.HasExited) { Stop-Process -Id $dEventsProc.Id -Force }
    if ($dForm -ne [IntPtr]::Zero -and [LT]::IsWindow($dForm)) { [TestForms]::CloseForm($dForm) }
    if ($dProc -and -not $dProc.HasExited) { Stop-WiriGracefully $dProc 'lane-d' }
}
