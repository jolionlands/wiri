# Lane P: pointer-device settings, the hotkey overlay's startup options and the
# dry-run audit of the pointer writes.  Dot-sourced by live-test.ps1 after the
# last lane include; runs only under its desktop lock.
#
# SAFETY: nothing in this lane may change the machine's real pointer or touchpad
# settings.  Every daemon it starts has WIRI_POINTER_SETTINGS_DRY_RUN set, which
# swaps wiri's real pointer backend for an Audited one that *records* the writes
# it would make instead of making them (reads still go to the OS).  The real
# values — SPI_GETMOUSE's three thresholds, SM_SWAPBUTTON and the two precision
# touchpad registry values — are snapshotted before the first daemon and after
# the last one, and compared: the lane fails if anything moved.
#
# Gates (all pre-set to $false; evidence in lane-p/evidence.json):
#   h75_hotkey_overlay_shown_at_startup        the sheet appears at startup
#   h75_hotkey_overlay_skip_at_startup         `hotkey-overlay { skip-at-startup }` suppresses it
#   h75_hotkey_overlay_hide_not_bound          `hide-not-bound` drops the unbound placeholders
#   h80_curated_hotkeys_render                curated core titles appear on the painted sheet
#   h80_hotkey_overlay_title_per_bind         explicit markup title appears; null stays bound but hidden
#   h75_toggle_touchpad_event_dry_run          `toggle-touchpad` emits TouchpadToggled {on} both ways
#   h75_pointer_settings_dry_run_audit         the audit file names exactly the writes it would make
#   h75_pointer_settings_restore_on_quit       quitting records the restores of those settings
#   h75_pointer_settings_real_values_unchanged the real settings are byte-identical afterwards
& {
    $pRoot = Join-Path $D 'lane-p'
    $pDir = Join-Path $pRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force $pDir
    $pDetails = [ordered]@{ fixture_directory = $pDir }
    $pGates = @('h75_hotkey_overlay_shown_at_startup', 'h75_hotkey_overlay_skip_at_startup',
        'h75_hotkey_overlay_hide_not_bound', 'h75_toggle_touchpad_event_dry_run',
        'h75_pointer_settings_dry_run_audit', 'h75_pointer_settings_restore_on_quit',
        'h75_pointer_settings_real_values_unchanged', 'h80_curated_hotkeys_render',
        'h80_hotkey_overlay_title_per_bind')
    foreach ($pGate in $pGates) { $summary[$pGate] = $false }
    $pDetails['gates'] = @($pGates)
    function PGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $pDetails[$name] = [ordered]@{ pass = $pass; observed = $observed }
        $pDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $pRoot 'evidence.json')
    }
    function PCheckpoint([string]$stage) {
        $pDetails['stage'] = $stage
        $pDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $pRoot 'evidence.json')
    }
    # A log file a running daemon still has open: share it instead of failing.
    function PReadText([string]$path) {
        if (-not (Test-Path $path)) { return '' }
        try {
            $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
            try { $reader = New-Object IO.StreamReader($stream); return $reader.ReadToEnd() }
            finally { if ($reader) { $reader.Close() } else { $stream.Close() } }
        } catch { return '' }
    }
    function PWaitLog($daemon, [string]$pattern, [int]$ms = 8000) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $text = PReadText $daemon.Stderr
            if ($text -match $pattern) { return $text }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt $ms)
        return (PReadText $daemon.Stderr)
    }
    function PWaitPipe($daemon, [int]$ms = 20000) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ($daemon.Process.HasExited) { throw "Owned daemon $($daemon.Name) exited during startup" }
            try { if ([IO.Directory]::GetFiles('\\.\pipe\') -contains $daemon.Pipe) { return } } catch { return }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt $ms)
        throw "Owned daemon $($daemon.Name) never served its pipe"
    }
    function PRequest($rpc, $request) {
        $text = $request | ConvertTo-Json -Depth 12 -Compress
        $rpc.Send($text + "`n")
        $reply = $rpc.ReadLine(4000)
        if ($null -eq $reply) { throw "Lane P IPC timeout: $text" }
        [ordered]@{ request = $request; reply = ($reply | ConvertFrom-Json) } |
            ConvertTo-Json -Depth 12 -Compress | Add-Content -Encoding UTF8 (Join-Path $pDir 'wire.ndjson')
        return ($reply | ConvertFrom-Json)
    }
    function PAction($rpc, [string]$name) {
        $reply = PRequest $rpc @{ type = 'run_action'; data = @{ name = $name; args = @(); quoted = @() } }
        if (-not $reply.success) { throw "Action $name failed: $($reply | ConvertTo-Json -Compress)" }
        return $reply
    }
    function PDrainEvents($events, [int]$waitMs = 400) {
        $records = @()
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $line = $events.ReadLine(60)
            if ($null -ne $line) {
                Add-Content -Encoding UTF8 (Join-Path $pDir 'events.ndjson') $line
                $records += ($line | ConvertFrom-Json)
            }
        } while ($clock.ElapsedMilliseconds -lt $waitMs)
        return $records
    }
    # A daemon of our own: its own pipe, its own config, its own (child-local)
    # profile, and the pointer backend in dry-run mode.  Never --no-autostart
    # unless asked, because that also keeps the startup sheet closed.
    function PStartDaemon([string]$name, [string]$configText, [switch]$NoAutostart) {
        $configPath = Join-Path $pDir ($name + '.kdl')
        [IO.File]::WriteAllText($configPath, $configText, (New-Object Text.UTF8Encoding($false)))
        $audit = Join-Path $pDir ($name + '-pointer-audit.ndjson')
        $daemon = [pscustomobject]@{
            Name    = $name
            Config  = $configPath
            Stderr  = Join-Path $pDir ($name + '.stderr')
            Stdout  = Join-Path $pDir ($name + '.stdout')
            Audit   = $audit
            Pipe    = '\\.\pipe\wiri_live_p_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
            Process = $null
        }
        $savedDry = $env:WIRI_POINTER_SETTINGS_DRY_RUN
        $savedRoaming = $env:APPDATA
        $savedLocal = $env:LOCALAPPDATA
        $savedProfile = $env:USERPROFILE
        $env:WIRI_PIPE = $daemon.Pipe
        $env:WIRI_POINTER_SETTINGS_DRY_RUN = $audit
        $arguments = @('-v')
        if ($NoAutostart) { $arguments += '--no-autostart' }
        $arguments += @('-c', ('"' + $configPath + '"'))
        try {
            $env:APPDATA = Join-Path $pDir 'roaming'
            $env:LOCALAPPDATA = Join-Path $pDir 'local'
            $env:USERPROFILE = $pDir
            $daemon.Process = Start-Process -FilePath $wiri -ArgumentList $arguments -PassThru `
                -RedirectStandardError $daemon.Stderr -RedirectStandardOutput $daemon.Stdout
            $null = $daemon.Process.Handle
        } finally {
            $env:APPDATA = $savedRoaming
            $env:LOCALAPPDATA = $savedLocal
            $env:USERPROFILE = $savedProfile
            # The pipe name stays set for this daemon's connections; the dry-run
            # switch is only for the child, never for later code of this run.
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = $savedDry
        }
        # Owned from the moment it exists: whatever happens below (the record, the pipe wait),
        # the lane's `finally` stops it.  A daemon that was only registered after PStartDaemon
        # had returned leaked when this function threw, and the next lane then refused to run
        # next to an unowned wiri process.
        $pDaemons.Add($daemon)
        $pDaemonRecords.Add([ordered]@{
                name = $name; pid = $daemon.Process.Id; pipe = $daemon.Pipe; config = $configPath; audit = $audit
            })
        # NOT `@($pDaemonRecords)`: on Windows PowerShell 5.1 the array operator on a
        # List[object] throws "Argument types do not match" (see EvList in live-test.ps1).
        $pDetails['daemons'] = $pDaemonRecords.ToArray()
        PWaitPipe $daemon
        return $daemon
    }
    # Graceful, prompt-free quit: `quit skip-confirmation=true` over IPC.
    function PStopDaemon($daemon) {
        if ($null -eq $daemon -or $null -eq $daemon.Process) { return }
        if (-not $daemon.Process.HasExited) {
            try {
                $rpc = New-Object LaneHConnection $daemon.Pipe, $daemon.Process.Id
                try {
                    $null = PRequest $rpc @{ type = 'run_action'; data = @{ name = 'quit'; args = @('--skip-confirmation'); quoted = @() } }
                } finally { $rpc.Dispose() }
            } catch { $pDetails[($daemon.Name + '-quit-error')] = [string]$_ }
            # The reply can be lost with the pipe when the daemon shuts down at once, so the
            # exit is what is waited for; only a daemon that is still up after that is killed.
            $null = [LanePNative]::Exited($daemon.Process.Handle, 8000)
            if (-not $daemon.Process.HasExited) {
                $pDetails[($daemon.Name + '-forced-stop')] = $true
                Stop-Process -Id $daemon.Process.Id -Force
            }
        }
        $null = $daemon.Process.WaitForExit(4000)
    }
    # These probes need the actual GDI-painted text, not a second implementation of
    # curated_rows over get-bindings. Windows OCR reads a retained screen capture;
    # unavailable OCR or unreadable text fails closed (never a skipped/pass gate).
    function PAwaitWinRT($operation, [type]$resultType, [DateTime]$deadline) {
        $method = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
            $_.Name -eq 'AsTask' -and $_.IsGenericMethod -and
            $_.GetGenericArguments().Count -eq 1 -and $_.GetParameters().Count -eq 1 -and
            $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
        } | Select-Object -First 1
        $task = $method.MakeGenericMethod($resultType).Invoke($null, @($operation))
        $remaining = [int][Math]::Floor(($deadline - [DateTime]::UtcNow).TotalMilliseconds)
        if ($remaining -le 0 -or -not $task.Wait($remaining)) { throw 'Lane P OCR operation timed out' }
        return $task.Result
    }
    function POverlayText([IntPtr]$window, [string]$path, [DateTime]$deadline) {
        $bounds = [LanePNative]::Bounds($window)
        $bitmap = New-Object Drawing.Bitmap ($bounds[2] - $bounds[0]), ($bounds[3] - $bounds[1])
        try {
            $graphics = [Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.CopyFromScreen($bounds[0], $bounds[1], 0, 0, $bitmap.Size)
                $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
            } finally { $graphics.Dispose() }
        } finally { $bitmap.Dispose() }
        $file = PAwaitWinRT ([Windows.Storage.StorageFile]::GetFileFromPathAsync($path)) ([Windows.Storage.StorageFile]) $deadline
        $stream = PAwaitWinRT ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream]) $deadline
        try {
            $decoder = PAwaitWinRT ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder]) $deadline
            $software = PAwaitWinRT ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap]) $deadline
            try {
                $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage((New-Object Windows.Globalization.Language 'en-US'))
                if ($null -eq $engine) { throw 'English Windows OCR is unavailable' }
                $result = PAwaitWinRT ($engine.RecognizeAsync($software)) ([Windows.Media.Ocr.OcrResult]) $deadline
                return $result.Text
            } finally { $software.Dispose() }
        } finally { $stream.Dispose() }
    }
    function PContentProbe([string]$name, [string]$configText, $expectedTitles, [int]$expectedRows) {
        $stderr = Join-Path $pDir ($name + '.stderr')
        [IO.File]::WriteAllText((Join-Path $pDir ($name + '.kdl')), $configText)
        $env:WIRI_PIPE = '\\.\pipe\wiri_live_p_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $savedDry = $env:WIRI_POINTER_SETTINGS_DRY_RUN
        $audit = Join-Path $pDir ($name + '-pointer-audit.ndjson')
        try {
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = $audit
            $process = Start-WiriWithConfig $configText $stderr -NoDefaultBinds
        } finally { $env:WIRI_POINTER_SETTINGS_DRY_RUN = $savedDry }
        $daemon = [pscustomobject]@{ Name = $name; Process = $process; Pipe = $env:WIRI_PIPE; Stderr = $stderr }
        $pDaemons.Add($daemon)
        $pDaemonRecords.Add([ordered]@{ name = $name; pid = $process.Id; pipe = $daemon.Pipe; audit = $audit })
        $pDetails['daemons'] = $pDaemonRecords.ToArray()
        $connection = $null
        try {
            $connection = New-Object LaneHConnection $daemon.Pipe, $process.Id
            $bindingsReply = PRequest $connection @{ type = 'get_bindings' }
            if (-not $bindingsReply.success) { throw 'get-bindings failed' }
            $bindings = @($bindingsReply.result)
            # PS 5.1 can wrap the top-level JSON array in one scalar object.
            if ($bindings.Count -eq 1 -and $bindings[0] -is [System.Array]) { $bindings = @($bindings[0]) }
            $null = PAction $connection 'show-hotkey-overlay'
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $window = [IntPtr]::Zero
            $rows = -1
            $text = ''
            $missing = @($expectedTitles)
            $ocrError = $null
            $deadline = [DateTime]::UtcNow.AddSeconds(15)
            $capture = Join-Path $pDir ($name + '.png')
            do {
                $log = PReadText $stderr
                $hits = [regex]::Matches($log, 'toggling cheatsheet with (\d+) rows \(hide-not-bound true\)')
                if ($hits.Count -gt 0) { $rows = [int]$hits[$hits.Count - 1].Groups[1].Value }
                # Find belongs INSIDE the poll: the log can precede HWND creation.
                $window = [LanePNative]::Find($process.Id, 'WiriBindingsCheatsheet')
                if ($window -ne [IntPtr]::Zero -and $rows -eq $expectedRows) {
                    try {
                        $text = POverlayText $window $capture $deadline
                        $missing = @($expectedTitles | Where-Object { $text.IndexOf($_, [StringComparison]::Ordinal) -lt 0 })
                        $ocrError = $null
                        if ($missing.Count -eq 0) { break }
                    } catch { $ocrError = [string]$_ }
                }
                Start-Sleep -Milliseconds 200
            } while ($clock.ElapsedMilliseconds -lt 15000)
            return [ordered]@{
                window = $window.ToInt64(); rows = $rows; expected_rows = $expectedRows
                expected_titles = @($expectedTitles); missing_titles = $missing; rendered_text = $text
                bindings = $bindings; capture = $capture; ocr_error = $ocrError
                log = @($log -split "`r?`n" | Where-Object { $_ -match 'toggling cheatsheet' })
                pass = ($window -ne [IntPtr]::Zero -and $rows -eq $expectedRows -and
                    $missing.Count -eq 0 -and $null -eq $ocrError)
            }
        } finally {
            if ($connection) { $connection.Dispose() }
            PStopDaemon $daemon
        }
    }
    # The precision touchpad values the backend can write, read the same way it reads them.
    function PRealSettings {
        $mouse = [LanePNative]::MouseThresholds()
        $swap = [LanePNative]::SwapButton()
        $aap = '<absent>'
        $leave = '<absent>'
        try {
            $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\PrecisionTouchPad')
            if ($null -ne $key) {
                try {
                    $aap = [string]$key.GetValue('AAPThreshold', '<absent>')
                    $leave = [string]$key.GetValue('LeaveOnWithMouse', '<absent>')
                } finally { $key.Close() }
            }
        } catch { $aap = '<unreadable>'; $leave = '<unreadable>' }
        return [pscustomobject]@{
            Mouse = $mouse; Swap = [string]$swap; Aap = $aap; Leave = $leave
            Text  = "mouse=$mouse;swap=$swap;aap=$aap;leave=$leave"
        }
    }
    function PAuditLines([string]$path) {
        $text = PReadText $path
        if ([string]::IsNullOrWhiteSpace($text)) { return @() }
        return @($text -split "`r?`n" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
    }
    function PSettingValue($lines, [string]$setting, [string]$action) {
        $hit = @($lines | Where-Object { $_.setting -eq $setting -and $_.action -eq $action })
        if ($hit.Count -eq 0) { return $null }
        # The last one: the restore of a setting follows the write that changed it.
        return (@($hit[-1].value) -join ',')
    }

    $pDaemons = New-Object System.Collections.Generic.List[object]
    $pDaemonRecords = New-Object System.Collections.Generic.List[object]
    $pSavedPipe = $env:WIRI_PIPE
    $pBefore = $null
    $pAfter = $null
    $pRpc = $null
    try {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class LanePNative {
    delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    struct RECT { public int Left, Top, Right, Bottom; }
    public static int[] Bounds(IntPtr h) {
        RECT r;
        if (!GetWindowRect(h, out r)) throw new InvalidOperationException("Overlay bounds unavailable");
        return new int[] { r.Left, r.Top, r.Right, r.Bottom };
    }
    [DllImport("user32.dll")] static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll", SetLastError = true)] static extern bool SystemParametersInfoW(uint action, uint param, [In, Out] int[] data, uint flags);
    [DllImport("user32.dll")] static extern int GetSystemMetrics(int index);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    public const uint WM_CLOSE = 0x0010;
    /// A visible top-level window of this process with this class name, or IntPtr.Zero.
    public static IntPtr Find(int pid, string className) {
        IntPtr found = IntPtr.Zero;
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner == (uint)pid && IsWindowVisible(h)) {
                var cls = new StringBuilder(128); GetClassNameW(h, cls, cls.Capacity);
                if (cls.ToString() == className) { found = h; return false; }
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static void Close(IntPtr h) { if (h != IntPtr.Zero && IsWindow(h)) PostMessageW(h, WM_CLOSE, IntPtr.Zero, IntPtr.Zero); }
    public static bool Exited(IntPtr h, uint ms) { return WaitForSingleObject(h, ms) == 0; }
    /// SPI_GETMOUSE: the two speed thresholds and the acceleration flag, comma separated.
    public static string MouseThresholds() {
        var data = new int[3];
        if (!SystemParametersInfoW(0x0003, 0, data, 0)) { return "unreadable"; }
        return data[0] + "," + data[1] + "," + data[2];
    }
    /// SM_SWAPBUTTON: nonzero when the primary and secondary mouse buttons are exchanged.
    public static int SwapButton() { return GetSystemMetrics(23); }
}
'@
        PCheckpoint 'snapshotting-real-pointer-settings'
        $pBefore = PRealSettings
        $pDetails['real-settings-before'] = $pBefore.Text

        # ---- 1. the sheet appears at startup ---------------------------------------------
        PCheckpoint 'starting-startup-overlay-daemon'
        $pShownConfig = @"
binds {
    extend-defaults false
    Ctrl+Alt+F9 { close-window; }
}
"@
        $pShown = PStartDaemon 'overlay-shown' $pShownConfig
        $pOverlay = [IntPtr]::Zero
        $pOverlayClock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $pOverlay = [LanePNative]::Find($pShown.Process.Id, 'WiriBindingsCheatsheet')
            if ($pOverlay -ne [IntPtr]::Zero) { break }
            Start-Sleep -Milliseconds 100
        } while ($pOverlayClock.ElapsedMilliseconds -lt 10000)
        $pShownLog = PWaitLog $pShown 'Hotkey overlay: shown at startup' 4000
        $pShownRows = -1
        if ($pShownLog -match 'Hotkey overlay: shown at startup with (\d+) rows') { $pShownRows = [int]$Matches[1] }
        $pShownPass = $pOverlay -ne [IntPtr]::Zero -and $pShownRows -gt 0 -and $pShownLog -match 'hide-not-bound false'
        PGate 'h75_hotkey_overlay_shown_at_startup' $pShownPass ([ordered]@{
                window = $pOverlay.ToInt64(); rows = $pShownRows
                skipped_line = ($pShownLog -match 'Hotkey overlay: skipped at startup')
                log = ($pShownLog -split "`r?`n" | Where-Object { $_ -match 'Hotkey overlay|ShowKeyBindings' })
            })
        # A sheet on screen would take the first key press of a later script.
        [LanePNative]::Close($pOverlay)
        PStopDaemon $pShown

        # ---- 2. `skip-at-startup` keeps it closed, `hide-not-bound` trims the list --------
        PCheckpoint 'starting-skip-and-pointer-daemon'
        $pSkipConfig = @"
hotkey-overlay {
    skip-at-startup
    hide-not-bound
}
binds {
    extend-defaults false
    Ctrl+Alt+F9 { close-window; }
}
input {
    mouse {
        accel-profile "flat"
        left-handed
    }
    touchpad {
        dwt
        disabled-on-external-mouse
    }
}
"@
        $pSkip = PStartDaemon 'overlay-skip' $pSkipConfig
        $pSkipLog = PWaitLog $pSkip 'Hotkey overlay: skipped at startup' 8000
        # Nothing should appear at startup: give a stray sheet the same grace the shown run had.
        Start-Sleep -Milliseconds 1500
        $pStartupWindow = [LanePNative]::Find($pSkip.Process.Id, 'WiriBindingsCheatsheet')
        PGate 'h75_hotkey_overlay_skip_at_startup' ($pStartupWindow -eq [IntPtr]::Zero -and
            $pSkipLog -match 'Hotkey overlay: skipped at startup \(hotkey-overlay \{ skip-at-startup \}\)') ([ordered]@{
                window = $pStartupWindow.ToInt64()
                log = ($pSkipLog -split "`r?`n" | Where-Object { $_ -match 'Hotkey overlay|ShowKeyBindings' })
            })

        $pRpc = New-Object LaneHConnection $pSkip.Pipe, $pSkip.Process.Id
        $null = PAction $pRpc 'show-hotkey-overlay'
        $pHiddenRows = -1
        $pHiddenLog = ''
        $pOpenClock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $pHiddenLog = PReadText $pSkip.Stderr
            if ($pHiddenLog -match 'toggling cheatsheet with (\d+) rows \(hide-not-bound true\)') {
                # Keep the LAST toggling line: the stderr stream can still
                # hold an earlier daemon's 17-row line, and -match pins
                # $Matches to the first occurrence.
                $pHiddenRows = [int]([regex]::Matches($pHiddenLog, 'toggling cheatsheet with (\d+) rows \(hide-not-bound true\)') | Select-Object -Last 1 | ForEach-Object { $_.Groups[1].Value })
                $pOpenedWindow = [LanePNative]::Find($pSkip.Process.Id, 'WiriBindingsCheatsheet')
                if ($pOpenedWindow -ne [IntPtr]::Zero) { break }
            }
            Start-Sleep -Milliseconds 100
        } while ($pOpenClock.ElapsedMilliseconds -lt 15000)
        PGate 'h75_hotkey_overlay_hide_not_bound' ($pOpenedWindow -ne [IntPtr]::Zero -and $pHiddenRows -ge 1 -and
            $pShownRows -gt $pHiddenRows) ([ordered]@{
                shown_rows = $pShownRows; hidden_rows = $pHiddenRows
                opened_by_action = $pOpenedWindow.ToInt64()
                log = ($pHiddenLog -split "`r?`n" | Where-Object { $_ -match 'ShowKeyBindings' })
            })
        [LanePNative]::Close($pOpenedWindow)

        # ---- 3. `toggle-touchpad` tells the event stream (dry run) ------------------------
        PCheckpoint 'subscribing-to-events'
        $pEvents = New-Object LaneHConnection $pSkip.Pipe, $pSkip.Process.Id
        try {
            $pEvents.Send('"EventStream"' + "`n")
            $null = PDrainEvents $pEvents 500
            $null = PAction $pRpc 'toggle-touchpad'
            $pOffEvents = PDrainEvents $pEvents 700
            $null = PAction $pRpc 'toggle-touchpad'
            $pOnEvents = PDrainEvents $pEvents 700
            $pSawOff = @($pOffEvents | Where-Object { $null -ne $_.TouchpadToggled }).Count -ge 1 -and
                @($pOffEvents | Where-Object { $null -ne $_.TouchpadToggled -and $_.TouchpadToggled.on -eq $false }).Count -ge 1
            $pSawOn = @($pOnEvents | Where-Object { $null -ne $_.TouchpadToggled -and $_.TouchpadToggled.on -eq $true }).Count -ge 1
            PGate 'h75_toggle_touchpad_event_dry_run' ($pSawOff -and $pSawOn) ([ordered]@{
                    after_first = @($pOffEvents | Where-Object { $null -ne $_.TouchpadToggled })
                    after_second = @($pOnEvents | Where-Object { $null -ne $_.TouchpadToggled })
                })
        } finally { $pEvents.Dispose(); $pEvents = $null }

        # ---- 4. the audit recorded exactly the writes a real run would make --------------
        PCheckpoint 'checking-the-pointer-audit'
        $pWrites = @(PAuditLines $pSkip.Audit | Where-Object { $_.action -eq 'write' })
        $pExpect = [ordered]@{}
        if ($pBefore.Mouse -ne '0,0,0') { $pExpect['enhanced-pointer-precision'] = '0,0,0' }
        if ($pBefore.Swap -ne '1') { $pExpect['mouse-button-swap'] = '1' }
        if ($pBefore.Aap -ne '3') { $pExpect['touchpad-typing-sensitivity'] = '3' }
        if ($pBefore.Leave -ne '0') { $pExpect['touchpad-leave-on-with-mouse'] = '0' }
        $pWrote = [ordered]@{}
        foreach ($setting in $pExpect.Keys) { $pWrote[$setting] = PSettingValue $pWrites $setting 'write' }
        $pUnexpected = @($pWrites | Where-Object { -not $pExpect.Contains($_.setting) })
        $pAuditPass = $pWrites.Count -ge 1 -and $pUnexpected.Count -eq 0 -and
            @($pExpect.Keys | Where-Object { $pWrote[$_] -ne $pExpect[$_] }).Count -eq 0
        PGate 'h75_pointer_settings_dry_run_audit' $pAuditPass ([ordered]@{
                expected = $pExpect; written = $pWrote
                unexpected = $pUnexpected; lines = @(PAuditLines $pSkip.Audit)
            })

        # ---- 5. a graceful quit restores them (still in the audit) ----------------------
        PCheckpoint 'quitting-the-pointer-daemon'
        PStopDaemon $pSkip
        $pRestores = @(PAuditLines $pSkip.Audit | Where-Object { $_.action -eq 'write' -or $_.action -eq 'remove' })
        # Every setting wiri wrote is put back: the two system-wide ones by writing the value
        # they had, the registry ones by writing it or by removing the value again.
        $pWas = [ordered]@{
            'enhanced-pointer-precision'   = $pBefore.Mouse
            'mouse-button-swap'            = $pBefore.Swap
            'touchpad-typing-sensitivity'  = $pBefore.Aap
            'touchpad-leave-on-with-mouse' = $pBefore.Leave
        }
        $pRestored = [ordered]@{}
        $pRestoreWrong = @()
        foreach ($setting in $pExpect.Keys) {
            $write = PSettingValue $pRestores $setting 'write'
            $remove = PSettingValue $pRestores $setting 'remove'
            $pRestored[$setting] = [ordered]@{ was = $pWas[$setting]; write = $write; remove = $remove }
            if ($pWas[$setting] -eq '<absent>') {
                # It did not exist before: the restore deletes it again.
                if ($null -eq $remove) { $pRestoreWrong += $setting }
            } elseif ($write -ne $pWas[$setting]) {
                $pRestoreWrong += $setting
            }
        }
        PGate 'h75_pointer_settings_restore_on_quit' ($pExpect.Count -ge 1 -and $pRestoreWrong.Count -eq 0) ([ordered]@{
                restored = $pRestored; wrong = $pRestoreWrong; lines = $pRestores
            })

        # ---- 6. C27/C28: read the painted sheet, not merely the registered keymap ------
        PCheckpoint 'checking-curated-overlay-content'
        Add-Type -AssemblyName System.Drawing
        Add-Type -AssemblyName System.Runtime.WindowsRuntime
        $null = [Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime]
        $null = [Windows.Storage.Streams.IRandomAccessStream, Windows.Storage.Streams, ContentType = WindowsRuntime]
        $null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime]
        $null = [Windows.Graphics.Imaging.SoftwareBitmap, Windows.Graphics.Imaging, ContentType = WindowsRuntime]
        $null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
        $null = [Windows.Media.Ocr.OcrResult, Windows.Foundation, ContentType = WindowsRuntime]
        $null = [Windows.Globalization.Language, Windows.Globalization, ContentType = WindowsRuntime]
        # niri's curated Important Hotkeys list (ui/hotkey_overlay.rs collect_actions, pinned
        # e19df12, :197-304) and the titles action_name gives them (:458-494), in niri's order:
        # exactly these 18 actions are always candidates; with `hide-not-bound` the ones with no
        # bind drop out.  No defaults, aliases, placeholders or title overrides.  The earlier
        # gate also bound Spawn, focus-window-up/down and fullscreen-window and expected 18 rows
        # with the pre-niri titles ("Launch application", "Focus window above", "Fullscreen
        # window"...); none of those is in niri's list (a spawn is listed only for a Mod/Super
        # chord, :281-296, and these chords are Ctrl+Alt+Shift), so 14 rows is correct for that
        # config.  The wiri-specific wording is only the program name: "Exit wiri" for "Exit niri".
        $pCore = @(
            @('show-hotkey-overlay', 'Show Important Hotkeys'),
            @('quit', 'Exit wiri'),
            @('close-window', 'Close Focused Window'),
            @('focus-column-left', 'Focus Column to the Left'),
            @('focus-column-right', 'Focus Column to the Right'),
            @('move-column-left', 'Move Column Left'),
            @('move-column-right', 'Move Column Right'),
            @('focus-workspace-down', 'Switch Workspace Down'),
            @('focus-workspace-up', 'Switch Workspace Up'),
            @('move-column-to-workspace-down', 'Move Column to Workspace Down'),
            @('move-column-to-workspace-up', 'Move Column to Workspace Up'),
            @('switch-preset-column-width', 'Switch Preset Column Widths'),
            @('maximize-column', 'Maximize Column'),
            @('consume-or-expel-window-left', 'Consume or Expel Window Left'),
            @('consume-or-expel-window-right', 'Consume or Expel Window Right'),
            @('toggle-window-floating', 'Move Window Between Floating and Tiling'),
            @('switch-focus-between-floating-and-tiling', 'Switch Focus Between Floating and Tiling'),
            @('toggle-overview', 'Open the Overview')
        )
        # Registered, but not in niri's list: they must not get a row (so rows stay 18).
        $pNotCurated = @('spawn "notepad.exe"', 'focus-window-up', 'focus-window-down', 'fullscreen-window')
        $pBound = @($pCore | ForEach-Object { $_[0] }) + $pNotCurated
        $pCoreConfig = "hotkey-overlay { skip-at-startup; hide-not-bound; }`nbinds {`n    extend-defaults false`n"
        for ($i = 0; $i -lt $pBound.Count; $i++) {
            $pCoreConfig += '    Ctrl+Alt+Shift+' + [char](65 + $i) + ' { ' + $pBound[$i] + "; }`n"
        }
        $pCoreConfig += "}`n"
        $pCoreTitles = @($pCore | ForEach-Object { $_[1] })
        $pCurated = PContentProbe 'curated-hotkeys' $pCoreConfig $pCoreTitles $pCore.Count
        # Important Hotkeys also occurs in the header: demand the row copy too.
        # Check registration as well, so placeholders cannot stand in for binds.
        $pCurated['hotkeys_title_occurrences'] = [regex]::Matches($pCurated.rendered_text, 'Important Hotkeys').Count
        $pCurated['registered_count'] = @($pCurated.bindings).Count
        # The painted sheet carries none of the non-curated actions' pre-niri wording either.
        $pCurated['non_curated_text'] = @([regex]::Matches($pCurated.rendered_text, 'Spawn|notepad|FIXME|Fullscreen|Launch application|Focus window (above|below)') | ForEach-Object { $_.Value })
        PGate 'h80_curated_hotkeys_render' ($pCurated.pass -and
            $pCurated.hotkeys_title_occurrences -eq 2 -and
            $pCurated.registered_count -eq $pBound.Count -and
            $pCurated.non_curated_text.Count -eq 0) $pCurated

        PCheckpoint 'checking-per-bind-overlay-titles'
        # niri paints a bind's title with Pango set_markup (wiki
        # Configuration:-Key-Bindings, "Custom Hotkey Overlay Titles"), so `<b>` is
        # formatting, not text: the row shows the title's words, without the tag.  Look for
        # those words (they are still the title, not the default action description) and
        # require that the tag was not painted literally.
        $pMarkupTitle = 'C28 Launch'
        $pTitleConfig = @'
hotkey-overlay { skip-at-startup; hide-not-bound; }
binds {
    extend-defaults false
    Ctrl+Alt+F9 hotkey-overlay-title="<b>C28 Launch</b>" { spawn "notepad.exe"; }
    Ctrl+Alt+F10 hotkey-overlay-title=null { close-window; }
}
'@
        $pTitles = PContentProbe 'per-bind-titles' $pTitleConfig @($pMarkupTitle) 1
        # The tag is markup, not text: a literal rendering would have painted it.
        $pTitles['markup_not_literal'] = ($pTitles.rendered_text -notmatch '<b>' -and
            $pTitles.rendered_text -notmatch 'b>C28')
        # null does NOT unregister the bind: its binding-table row remains, but
        # curated_rows omits it from the sheet altogether (not an empty title row).
        $pTitles['null_binding'] = @($pTitles.bindings | Where-Object {
            $_.vk_code -eq 0x79 -and $_.modifiers -eq 3 -and $_.action -eq 'CloseWindow'
        })
        $pTitles['titled_binding'] = @($pTitles.bindings | Where-Object {
            $_.vk_code -eq 0x78 -and $_.modifiers -eq 3 -and $_.action -like 'Spawn*'
        })
        PGate 'h80_hotkey_overlay_title_per_bind' ($pTitles.pass -and
            $pTitles.markup_not_literal -and
            @($pTitles.bindings).Count -eq 2 -and $pTitles.null_binding.Count -eq 1 -and
            $pTitles.titled_binding.Count -eq 1 -and $pTitles.rendered_text -notmatch 'Close window|Launch application') $pTitles
    } catch {
        $pDetails['error'] = ($_ | Out-String)
    } finally {
        PCheckpoint 'closing-owned-daemons'
        if ($pRpc) { $pRpc.Dispose(); $pRpc = $null }
        foreach ($daemon in $pDaemons) { PStopDaemon $daemon }
        # Nothing this lane started may outlive it: the next lane refuses to run next to a
        # wiri process it does not own.  Only the PIDs recorded above are ever touched.
        foreach ($record in $pDaemonRecords.ToArray()) {
            $left = Get-Process -Id $record.pid -ErrorAction SilentlyContinue
            if ($null -ne $left -and $left.ProcessName -eq 'wiri') {
                $pDetails[($record.name + '-left-running')] = $true
                Stop-Process -Id $record.pid -Force -ErrorAction SilentlyContinue
            }
        }
        $env:WIRI_PIPE = $pSavedPipe
        PCheckpoint 'snapshotting-real-pointer-settings-again'
        $pAfter = PRealSettings
        $pDetails['real-settings-after'] = $pAfter.Text
        # The last word of this lane: the test machine is exactly as it was.
        PGate 'h75_pointer_settings_real_values_unchanged' ($null -ne $pBefore -and
            $pBefore.Text -eq $pAfter.Text) ([ordered]@{
                before = $(if ($pBefore) { $pBefore.Text } else { '<not taken>' })
                after  = $pAfter.Text
            })
        $pDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $pDir 'evidence.json')
        $pDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $pRoot 'evidence.json')
    }
}
