# Lane LAST: niri's two cursor options and the tablet stylus binds.
# Dot-sourced by live-test.ps1 before lane-e-perf.ps1 (which stays last); runs only
# under the harness's desktop lock.
#
# What is checked here:
#   hide-when-typing      ONLY in dry-run mode.  The daemon carries
#                         WIRI_POINTER_SETTINGS_DRY_RUN, so lane P's Audited backend records
#                         the SPI_SETMOUSEVANISH write it *would* make and the restore instead
#                         of making them.  The machine's real SPI_GETMOUSEVANISH value is read
#                         before the first daemon and after the last one and must be unchanged.
#   hide-after-inactive-ms one private daemon with a 400 ms timeout.  The system cursor object
#                         (GetCursorInfo) must change exactly when wiri logs that it hid the
#                         pointer, change again when an injected move shows it, and be restored
#                         after a graceful quit.  This is the one place in the whole run where
#                         the lane lets a daemon swap the real cursors, and it is the daemon's
#                         own explicit hide/restore: the `finally` below reloads the user's
#                         cursor scheme unconditionally, so a failed assertion cannot leave the
#                         machine without a pointer.
#   TabletStylusButton1-3 decode and registration only.  Windows has no global pen-button
#                         stream without UIAccess, so nothing here claims a hardware pen event
#                         was delivered; the evidence says so.
#
# Gates (all pre-set to $false; evidence in lane-last/<run>/evidence.json and lane-last/evidence.json):
#   h78_hide_when_typing_dry_run                      the audit shows the SPI write and the restore
#   h78_hide_when_typing_real_value_unchanged         the real SPI_GETMOUSEVANISH value is unchanged
#   h78_cursor_hides_after_inactive_ms                the cursor object changes when the timer hides it
#   h78_cursor_shows_on_pointer_movement              an injected move shows it again
#   h78_cursor_restored_on_quit                       a graceful quit reloads the user's cursors
#   h78_tablet_stylus_bind_decode_and_registration    the binds decode, validate and register
& {
    $lastRoot = Join-Path $D 'lane-last'
    $lastDir = Join-Path $lastRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force $lastDir
    $lastDetails = [ordered]@{
        fixture_directory = $lastDir
        kind = 'cursor hiding is checked in dry-run (hide-when-typing) and with one private daemon (hide-after-inactive-ms); stylus binds are decode+registration only'
    }
    $lastGates = @('h78_hide_when_typing_dry_run', 'h78_hide_when_typing_real_value_unchanged',
        'h78_cursor_hides_after_inactive_ms', 'h78_cursor_shows_on_pointer_movement',
        'h78_cursor_restored_on_quit', 'h78_tablet_stylus_bind_decode_and_registration')
    foreach ($lastGateName in $lastGates) { $summary[$lastGateName] = $false }
    $lastDetails['gates'] = @($lastGates)
    function LastWriteEvidence {
        $lastDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $lastDir 'evidence.json')
        $lastDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $lastRoot 'evidence.json')
    }
    function LastGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $lastDetails[$name] = [ordered]@{ pass = $pass; observed = $observed }
        LastWriteEvidence
    }
    function LastCheckpoint([string]$stage) {
        $lastDetails['stage'] = $stage
        LastWriteEvidence
    }
    # A log file a running daemon still has open: share it instead of failing.
    function LastReadText([string]$path) {
        if (-not (Test-Path $path)) { return '' }
        try {
            $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
            try { $reader = New-Object IO.StreamReader($stream); return $reader.ReadToEnd() }
            finally { if ($reader) { $reader.Close() } else { $stream.Close() } }
        } catch { return '' }
    }
    function LastWaitLog($daemon, [string]$pattern, [int]$ms = 8000) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $text = LastReadText $daemon.Stderr
            if ($text -match $pattern) { return $text }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt $ms)
        return (LastReadText $daemon.Stderr)
    }
    function LastWaitPipe($daemon, [int]$ms = 25000) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ($daemon.Process.HasExited) { throw "Owned daemon $($daemon.Name) exited during startup" }
            try { if ([IO.Directory]::GetFiles('\\.\pipe\') -contains $daemon.Pipe) { return } } catch { return }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt $ms)
        throw "Owned daemon $($daemon.Name) never served its pipe"
    }
    function LastRequest($rpc, $request) {
        $text = $request | ConvertTo-Json -Depth 12 -Compress
        $rpc.Send($text + "`n")
        $reply = $rpc.ReadLine(4000)
        if ($null -eq $reply) { throw "Lane LAST IPC timeout: $text" }
        [ordered]@{ request = $request; reply = ($reply | ConvertFrom-Json) } |
            ConvertTo-Json -Depth 12 -Compress | Add-Content -Encoding UTF8 (Join-Path $lastDir 'wire.ndjson')
        return ($reply | ConvertFrom-Json)
    }
    # A daemon of our own: its own pipe, its own config, its own (child-local) profile, and
    # lane P's pointer backend in dry-run mode so no pointer/cursor *setting* is written.
    function LastStartDaemon([string]$name, [string]$configText) {
        $configPath = Join-Path $lastDir ($name + '.kdl')
        [IO.File]::WriteAllText($configPath, $configText, (New-Object Text.UTF8Encoding($false)))
        $audit = Join-Path $lastDir ($name + '-pointer-audit.ndjson')
        $daemon = [pscustomobject]@{
            Name    = $name
            Config  = $configPath
            Stderr  = Join-Path $lastDir ($name + '.stderr')
            Stdout  = Join-Path $lastDir ($name + '.stdout')
            Audit   = $audit
            Pipe    = '\\.\pipe\wiri_live_last_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
            Process = $null
        }
        $lastSavedDry = $env:WIRI_POINTER_SETTINGS_DRY_RUN
        $lastSavedRoaming = $env:APPDATA
        $lastSavedLocal = $env:LOCALAPPDATA
        $lastSavedProfile = $env:USERPROFILE
        $env:WIRI_PIPE = $daemon.Pipe
        $env:WIRI_POINTER_SETTINGS_DRY_RUN = $audit
        $arguments = @('-v', '--no-autostart', '-c', ('"' + $configPath + '"'))
        try {
            $env:APPDATA = Join-Path $lastDir 'roaming'
            $env:LOCALAPPDATA = Join-Path $lastDir 'local'
            $env:USERPROFILE = $lastDir
            $daemon.Process = Start-Process -FilePath $wiri -ArgumentList $arguments -PassThru `
                -RedirectStandardError $daemon.Stderr -RedirectStandardOutput $daemon.Stdout
            $null = $daemon.Process.Handle
        } finally {
            $env:APPDATA = $lastSavedRoaming
            $env:LOCALAPPDATA = $lastSavedLocal
            $env:USERPROFILE = $lastSavedProfile
            # Only this lane's child ever sees the dry-run switch.
            $env:WIRI_POINTER_SETTINGS_DRY_RUN = $lastSavedDry
        }
        # Owned from the moment it exists: the record is written before anything can throw.
        $lastDaemons.Add($daemon)
        $lastDaemonRecords.Add([ordered]@{
                name = $name; pid = $daemon.Process.Id; pipe = $daemon.Pipe; config = $configPath; audit = $audit
            })
        # NOT `@($lastDaemonRecords)`: Windows PowerShell 5.1 cannot array-ify a List[object].
        $lastDetails['daemons'] = $lastDaemonRecords.ToArray()
        LastWaitPipe $daemon
        return $daemon
    }
    # Graceful, prompt-free quit: `quit skip-confirmation=true` over IPC.  Only a daemon that is
    # still up after that is killed, and only the PID recorded above is ever touched.
    function LastStopDaemon($daemon) {
        if ($null -eq $daemon -or $null -eq $daemon.Process) { return }
        if (-not $daemon.Process.HasExited) {
            try {
                $rpc = New-Object LaneHConnection $daemon.Pipe, $daemon.Process.Id
                try {
                    $null = LastRequest $rpc @{ type = 'run_action'; data = @{ name = 'quit'; args = @('--skip-confirmation'); quoted = @() } }
                } finally { $rpc.Dispose() }
            } catch { $lastDetails[($daemon.Name + '-quit-error')] = [string]$_ }
            if ('LaneLastNative' -as [type]) {
                $null = [LaneLastNative]::Exited($daemon.Process.Handle, 10000)
            } else {
                $null = $daemon.Process.WaitForExit(10000)
            }
            if (-not $daemon.Process.HasExited) {
                $lastDetails[($daemon.Name + '-forced-stop')] = $true
                Stop-Process -Id $daemon.Process.Id -Force
            }
        }
        $null = $daemon.Process.WaitForExit(4000)
    }
    function LastAuditLines([string]$path) {
        $text = LastReadText $path
        if ([string]::IsNullOrWhiteSpace($text)) { return @() }
        return @($text -split "`r?`n" | Where-Object { $_.Trim() } | ForEach-Object { $_ | ConvertFrom-Json })
    }
    # "handle;flags": CURSOR_SHOWING is 1, so a non-zero handle with the flag set is a cursor
    # the system is drawing.  The identity of the object is what changes when wiri swaps the
    # system cursors for transparent ones and back.
    function LastCursorHandle([string]$state) {
        if ([string]::IsNullOrEmpty($state)) { return -1 }
        return [int64]($state.Split(';')[0])
    }

    $lastDaemons = New-Object System.Collections.Generic.List[object]
    $lastDaemonRecords = New-Object System.Collections.Generic.List[object]
    $lastSavedPipe = $env:WIRI_PIPE
    $lastVanishBefore = $null
    $lastVanishAfter = $null
    $lastRpc = $null
    $lastDaemon = $null
    try {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class LaneLastNative {
    [StructLayout(LayoutKind.Sequential)]
    public struct CURSORINFO {
        public int cbSize;
        public int flags;
        public IntPtr hCursor;
        public int x;
        public int y;
    }
    [DllImport("user32.dll", SetLastError = true)] static extern bool GetCursorInfo(ref CURSORINFO info);
    [DllImport("user32.dll", SetLastError = true)] static extern bool SystemParametersInfoW(uint action, uint param, IntPtr data, uint flags);
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr h, uint ms);
    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT {
        public uint type;
        public MOUSEINPUT mi;
    }
    const uint CURSOR_SHOWING = 0x0001;
    const uint INPUT_MOUSE = 0;
    const uint MOUSEEVENTF_MOVE = 0x0001;
    /// SPI_GETMOUSEVANISH: the per-user "hide pointer while typing" flag (niri hide-when-typing).
    const uint SPI_GETMOUSEVANISH = 0x1028;
    /// SPI_SETCURSORS: reload the user's cursor scheme (the product's own restore path).
    const uint SPI_SETCURSORS = 0x0057;
    /// "handle;flags", or "unreadable" when GetCursorInfo fails.
    public static string CursorState() {
        var info = new CURSORINFO();
        info.cbSize = Marshal.SizeOf(typeof(CURSORINFO));
        if (!GetCursorInfo(ref info)) { return "unreadable"; }
        return info.hCursor.ToInt64() + ";" + (info.flags & CURSOR_SHOWING);
    }
    /// 1 when the machine hides the pointer while typing, 0 when it does not, -1 on failure.
    public static int MouseVanish() {
        IntPtr buffer = Marshal.AllocHGlobal(4);
        try {
            Marshal.WriteInt32(buffer, -1);
            if (!SystemParametersInfoW(SPI_GETMOUSEVANISH, 0, buffer, 0)) { return -1; }
            return Marshal.ReadInt32(buffer) != 0 ? 1 : 0;
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    /// A real relative mouse move through SendInput: WH_MOUSE_LL sees it (SetCursorPos alone
    /// does not).  Returns how many events SendInput accepted (1 on success).
    public static uint Move(int dx, int dy) {
        var input = new INPUT();
        input.type = INPUT_MOUSE;
        input.mi = new MOUSEINPUT();
        input.mi.dx = dx;
        input.mi.dy = dy;
        input.mi.dwFlags = MOUSEEVENTF_MOVE;
        return SendInput(1, new INPUT[] { input }, Marshal.SizeOf(typeof(INPUT)));
    }
    /// The user's own cursors come back (what the daemon does on hide/show/quit).
    public static bool RestoreCursors() { return SystemParametersInfoW(SPI_SETCURSORS, 0, IntPtr.Zero, 0); }
    public static bool Exited(IntPtr h, uint ms) { return WaitForSingleObject(h, ms) == 0; }
}
'@
        LastCheckpoint 'snapshotting-the-real-hide-when-typing-value'
        $lastVanishBefore = [LaneLastNative]::MouseVanish()
        $lastDetails['real-hide-when-typing-before'] = $lastVanishBefore
        $lastDetails['cursor-before-daemon'] = [LaneLastNative]::CursorState()

        # ---- one private daemon: both cursor options and three stylus binds ---------------
        LastCheckpoint 'starting-the-private-daemon'
        $lastConfigText = @'
// Lane LAST: the private daemon of this lane.
hotkey-overlay { skip-at-startup }
binds {
    extend-defaults false
    Mod+TabletStylusButton1 { close-window; }
    Mod+TabletStylusButton2 { close-window; }
    Mod+TabletStylusButton3 { close-window; }
}
cursor {
    hide-when-typing
    hide-after-inactive-ms 400
}
'@
        $lastDaemon = LastStartDaemon 'cursor-stylus' $lastConfigText
        $lastRpc = New-Object LaneHConnection $lastDaemon.Pipe, $lastDaemon.Process.Id
        $lastDetails['config'] = $lastConfigText
        # Registration happens on the hotkey thread at startup; the decode is the same parser
        # `wiri-ctl validate-config` uses.
        $lastStylusLog = LastWaitLog $lastDaemon 'Tablet stylus binds: 3 registered' 25000
        $lastStylusRegistered = $lastStylusLog -match 'Tablet stylus binds: 3 registered'
        $lastVanishLog = LastWaitLog $lastDaemon 'hide-pointer-while-typing' 3000

        # ---- h78_cursor_hides_after_inactive_ms ------------------------------------------
        # A visible pointer first: any injected move shows it and re-arms the one timer.
        $lastMove1 = [LaneLastNative]::Move(3, 0)
        Start-Sleep -Milliseconds 150
        $lastVisibleState = [LaneLastNative]::CursorState()
        LastCheckpoint 'waiting-for-the-inactivity-timer'
        $lastHideLog = LastWaitLog $lastDaemon 'cursor: pointer hidden' 8000
        $lastHiddenState = [LaneLastNative]::CursorState()
        $lastHideSawLog = $lastHideLog -match 'cursor: pointer hidden'
        LastGate 'h78_cursor_hides_after_inactive_ms' ($lastHideSawLog -and $lastMove1 -ge 1 -and
            $lastHiddenState -ne 'unreadable' -and $lastVisibleState -ne 'unreadable' -and
            (LastCursorHandle $lastHiddenState) -ne 0 -and
            (LastCursorHandle $lastHiddenState) -ne (LastCursorHandle $lastVisibleState)) ([ordered]@{
                visible = $lastVisibleState
                hidden = $lastHiddenState
                timeout_ms = 400
                send_input_accepted = $lastMove1
                log = ($lastHideLog -split "`r?`n" | Where-Object { $_ -match 'cursor:' })
            })

        # ---- h78_cursor_shows_on_pointer_movement ----------------------------------------
        LastCheckpoint 'moving-the-pointer-again'
        $lastMove2 = [LaneLastNative]::Move(-4, 0)
        $lastShowLog = LastWaitLog $lastDaemon 'cursor: pointer shown again' 8000
        $lastShownState = [LaneLastNative]::CursorState()
        LastGate 'h78_cursor_shows_on_pointer_movement' ($lastShowLog -match 'pointer shown again' -and
            $lastMove2 -ge 1 -and $lastShownState -ne 'unreadable' -and
            (LastCursorHandle $lastShownState) -ne (LastCursorHandle $lastHiddenState)) ([ordered]@{
                hidden = $lastHiddenState
                shown = $lastShownState
                send_input_accepted = $lastMove2
                log = ($lastShowLog -split "`r?`n" | Where-Object { $_ -match 'cursor:' })
            })

        # ---- h78_tablet_stylus_bind_decode_and_registration ------------------------------
        LastCheckpoint 'validating-the-stylus-binds-offline'
        $lastValidate = CtlRaw @('validate-config', (Q $lastDaemon.Config))
        $lastBadConfig = Join-Path $lastDir 'stylus-button-4.kdl'
        [IO.File]::WriteAllText($lastBadConfig,
            "binds {`n    Mod+TabletStylusButton4 { close-window; }`n}`n",
            (New-Object Text.UTF8Encoding($false)))
        $lastValidateBad = CtlRaw @('validate-config', (Q $lastBadConfig))
        $lastDetails['kind-stylus'] = 'decode and registration only: no promoted pen event was injected and no hardware pen was used'
        LastGate 'h78_tablet_stylus_bind_decode_and_registration' ($lastStylusRegistered -and
            $lastValidate.rc -eq 0 -and $lastValidate.text -match 'ok \(no issues\)' -and
            $lastValidateBad.rc -ne 0 -and $lastValidateBad.text -match 'not a stylus button') ([ordered]@{
                registered = $lastStylusRegistered
                registration_log = ($lastStylusLog -split "`r?`n" | Where-Object { $_ -match 'Tablet stylus binds' })
                validate_rc = $lastValidate.rc
                validate_text = $lastValidate.text
                unknown_button_rc = $lastValidateBad.rc
                unknown_button_text = $lastValidateBad.text
                note = 'promoted pen mouse events only; no raw WM_POINTER without UIAccess'
            })

        # ---- quit: the user's cursors come back and the audit records the restore --------
        LastCheckpoint 'quitting-the-private-daemon'
        LastStopDaemon $lastDaemon
        $lastQuitLog = LastReadText $lastDaemon.Stderr
        $lastRestoredState = [LaneLastNative]::CursorState()
        $lastRestoreLogged = $lastQuitLog -match 'cursor: Windows cursors reloaded'
        LastGate 'h78_cursor_restored_on_quit' ($lastDaemon.Process.HasExited -and $lastRestoreLogged -and
            $lastRestoredState -ne 'unreadable' -and
            (LastCursorHandle $lastRestoredState) -ne (LastCursorHandle $lastHiddenState)) ([ordered]@{
                exited = $lastDaemon.Process.HasExited
                hidden = $lastHiddenState
                after_quit = $lastRestoredState
                log = ($lastQuitLog -split "`r?`n" | Where-Object { $_ -match 'cursor:|pointer settings:' })
            })

        # ---- h78_hide_when_typing_dry_run ------------------------------------------------
        LastCheckpoint 'reading-the-pointer-audit'
        $lastAudit = @(LastAuditLines $lastDaemon.Audit)
        $lastVanishLines = @($lastAudit | Where-Object { $_.setting -eq 'hide-pointer-while-typing' })
        if ($lastVanishBefore -eq 1) {
            # The machine already hides the pointer while typing: nothing is written, nothing
            # is owed back.
            $lastVanishPass = ($lastVanishLines.Count -eq 0)
            $lastVanishWhy = 'the machine already had the setting on, so the correct behaviour is no write at all'
        } else {
            # The dry-run write must say "on" and the quit must write the value it found.
            $lastWrites = @($lastVanishLines | Where-Object { $_.action -eq 'write' })
            $lastVanishPass = ($lastWrites.Count -eq 2 -and
                ((@($lastWrites[0].value) -join ',') -eq '1') -and
                ((@($lastWrites[-1].value) -join ',') -eq '0'))
            $lastVanishWhy = 'one SPI_SETMOUSEVANISH write of 1 at startup and one of 0 on quit'
        }
        LastGate 'h78_hide_when_typing_dry_run' ($lastVanishBefore -ne -1 -and $lastVanishPass) ([ordered]@{
                real_value_before = $lastVanishBefore
                writes = @($lastVanishLines)
                why = $lastVanishWhy
                audit_all = $lastAudit
                log = ($lastVanishLog -split "`r?`n" | Where-Object { $_ -match 'pointer settings:' })
            })
    } catch {
        $lastDetails['error'] = ($_ | Out-String)
    } finally {
        LastCheckpoint 'closing-owned-daemons'
        if ($lastRpc) { $lastRpc.Dispose(); $lastRpc = $null }
        foreach ($lastEach in $lastDaemons) { LastStopDaemon $lastEach }
        foreach ($lastRecord in $lastDaemonRecords.ToArray()) {
            $lastLeft = Get-Process -Id $lastRecord.pid -ErrorAction SilentlyContinue
            if ($null -ne $lastLeft -and $lastLeft.ProcessName -eq 'wiri') {
                $lastDetails[($lastRecord.name + '-left-running')] = $true
                Stop-Process -Id $lastRecord.pid -Force -ErrorAction SilentlyContinue
            }
        }
        $env:WIRI_PIPE = $lastSavedPipe
        # SAFETY, unconditionally: even if an assertion above threw while the pointer was
        # hidden, the machine gets its own cursors back.  This is the same call the product
        # makes on quit and is the only cursor write this lane performs itself.
        $lastRestoreInFinally = $false
        if ('LaneLastNative' -as [type]) {
            try { $lastRestoreInFinally = [LaneLastNative]::RestoreCursors() } catch { $lastRestoreInFinally = $false }
            $lastDetails['cursor-restored-in-finally'] = $lastRestoreInFinally
            $lastDetails['cursor-after-lane'] = [LaneLastNative]::CursorState()
            LastCheckpoint 'snapshotting-the-real-hide-when-typing-value-again'
            $lastVanishAfter = [LaneLastNative]::MouseVanish()
            $lastDetails['real-hide-when-typing-after'] = $lastVanishAfter
        }
        # The last word of this lane: the machine's own "hide pointer while typing" setting is
        # exactly what it was (the daemon ran in dry-run mode, so it could not have moved).
        LastGate 'h78_hide_when_typing_real_value_unchanged' ($null -ne $lastVanishBefore -and
            $lastVanishBefore -ne -1 -and $lastVanishBefore -eq $lastVanishAfter) ([ordered]@{
                before = $(if ($null -ne $lastVanishBefore) { $lastVanishBefore } else { 'not taken' })
                after = $(if ($null -ne $lastVanishAfter) { $lastVanishAfter } else { 'not taken' })
            })
        LastCheckpoint 'done'
    }
}
