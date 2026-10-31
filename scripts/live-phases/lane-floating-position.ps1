# H82: real HWND evidence for IPC-only floating positioning. The parent owns
# the global live-test mutex and isolated APPDATA. Only lane-owned forms are
# focused, moved, resized, floated or closed; the retained daemon owns its pipe.
& {
    $fpRoot = Join-Path $D 'lane-floating-position'
    $fpDir = Join-Path $fpRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $fpDir
    $fpDetails = [ordered]@{ fixture_directory = $fpDir }
    $fpGates = @(
        'h82_typed_fixed_position_and_origin', 'h82_typed_full_area_proportions',
        'h82_explicit_inactive_id_preserves_foreground_and_unrelated_geometry',
        'h82_cli_signed_axis_preserves_omitted_axis',
        'h82_partial_visibility_bounds', 'h82_partial_position_survives_resize',
        'h82_partial_position_survives_refresh',
        'h82_tiled_position_is_stored_until_float', 'h82_stored_float_position_restores'
    )
    foreach ($fpGate in $fpGates) {
        $summary[$fpGate] = $false
        $fpDetails[$fpGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $fpProcess = $null
    $fpForms = New-Object System.Collections.ArrayList
    $fpSavedPipe = $env:WIRI_PIPE
    $fpPipe = '\\.\pipe\wiri_live_h82_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $fpSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    $fpOldDpi = [IntPtr]::Zero
    $fpRuntime = @{ focus_serial = 0 }
    function FPGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $fpDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $fpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fpRoot 'evidence.json')
    }
    function FPOwned([IntPtr]$h) {
        if (-not $fpForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "H82 refuses an unowned form: $h"
        }
    }
    function FPCtl([string]$tag, [string[]]$words) {
        if ($null -eq $fpProcess -or $fpProcess.HasExited -or $env:WIRI_PIPE -ne $fpPipe) {
            throw 'H82 retained daemon/pipe ownership is no longer valid'
        }
        $text = Ctl ('h82-'+$tag) $words
        if ($steps.Count -eq 0 -or [string]$steps[$steps.Count-1].rc -ne '0') {
            throw "H82 CLI failed: $tag $text"
        }
        return $text
    }
    function FPRequest([string]$tag, $request, [bool]$external = $true) {
        $json = $request | ConvertTo-Json -Depth 12 -Compress
        $reply = (FPCtl $tag @('raw-request', (Q $json))) | ConvertFrom-Json
        if ($external) {
            if ($null -eq $reply.PSObject.Properties['Ok'] -or $reply.Ok -ne 'Handled') {
                throw "H82 external action was not handled: $($reply | ConvertTo-Json -Compress)"
            }
        } elseif ($reply.success -ne $true) {
            throw "H82 adjacent request failed: $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 550
        return $reply
    }
    function FPAction([string]$tag, [string[]]$words) {
        $reply = (FPCtl $tag (@('--json', 'action') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) {
            throw "H82 action was rejected: $tag $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 550
    }
    function FPTyped([string]$tag, [IntPtr]$h, $x, $y) {
        FPOwned $h
        $fields = [ordered]@{ id = [int64]$h }
        if ($null -ne $x) { $fields['x'] = $x }
        if ($null -ne $y) { $fields['y'] = $y }
        $null = FPRequest $tag @{ Action = @{ MoveFloatingWindow = $fields } }
    }
    function FPNativeFocusSnapshot([string]$stage) {
        $foreground = [LT]::GetForegroundWindow()
        $owned = @()
        foreach ($form in $fpForms) {
            $exists = [LT]::IsWindow($form)
            $title = if ($exists) { [LT]::Title($form) } else { '' }
            $role = switch ($title) {
                'lane-h82-float' { 'A' }
                'lane-h82-focus' { 'B' }
                'lane-h82-tile' { 'T' }
                default { 'opening-or-closed' }
            }
            $owned += [ordered]@{ role=$role; hwnd=$form.ToInt64(); title=$title;
                is_window=$exists; pid=[LT]::Pid($form); class=[LT]::Class($form);
                visible=($exists -and [LT]::IsWindowVisible($form)); iconic=($exists -and [LT]::IsIconic($form)) }
        }
        return [ordered]@{ stage=$stage; observed_utc=[DateTime]::UtcNow.ToString('o');
            foreground=@{ hwnd=$foreground.ToInt64(); pid=[LT]::Pid($foreground); class=[LT]::Class($foreground) };
            owned_forms=$owned }
    }
    function FPFocusDiagnosticCommand([string]$tag, [string[]]$words) {
        $observation = [ordered]@{ tag=$tag; arguments=$words; before=(FPNativeFocusSnapshot ($tag+'-before'));
            rc=$null; reply=$null; error=$null; after=$null }
        $stepCount = $steps.Count
        try {
            $rawReply = FPCtl $tag $words
            # Ctl returns Get-Content -Raw. In Windows PowerShell 5.1 its string
            # carries provider/drive ETS properties; deep JSON follows that graph.
            # Copy characters into a new System.String, retaining the raw CLI text
            # without file-provider metadata. Ctl also retains the original .txt.
            if ($null -ne $rawReply) { $observation.reply = [string]::new(([string]$rawReply).ToCharArray()) }
        }
        catch { $observation.error = ($_ | Out-String).Trim() }
        if ($steps.Count -gt $stepCount) { $observation.rc = $steps[$steps.Count-1].rc }
        $observation.after = FPNativeFocusSnapshot ($tag+'-after')
        return $observation
    }
    function FPFocus([IntPtr]$h) {
        FPOwned $h
        $fpRuntime.focus_serial++
        $tag = 'focus-{0:D3}-{1}' -f $fpRuntime.focus_serial,$h.ToInt64()
        $focus = [ordered]@{ stage=$tag; target_hwnd=$h.ToInt64(); status='requested'; attempts=@() }
        if (-not $fpDetails.Contains('focus_attempts')) { $fpDetails['focus_attempts'] = @() }
        $fpDetails.focus_attempts += $focus
        # Preserve FocusForm's two legacy focus-window requests and wait budgets,
        # but retain unique checked replies instead of overwriting h6 evidence.
        foreach ($attempt in 1..2) {
            $command = FPFocusDiagnosticCommand ($tag+'-attempt-'+$attempt) @('--json','focus-window',[string]$h.ToInt64())
            $focus.attempts += $command
            $minimum = if ($attempt -eq 1) { 1300 } else { 800 }
            $nativeSettled = WaitForeground $h $minimum 4000
            $command['after_wait'] = FPNativeFocusSnapshot ($tag+'-attempt-'+$attempt+'-after-wait')
            if ($null -ne $command.error) { break }
            try { $accepted = (($command.reply | ConvertFrom-Json).success -eq $true) }
            catch { $accepted = $false; $command.error = 'Invalid focus reply: '+[string]$_ }
            if (-not $accepted) { $command.error = 'Legacy focus-window reply did not report success'; break }
            if ($nativeSettled) { break }
        }
        if ($null -eq $command.error -and (WaitForeground $h 200 3000)) {
            $focus.status = 'native-focus-settled'
            return
        }
        $originalError = "H82 owned form did not gain focus: $h"
        $focus.status = 'failed-original-precondition'
        $focus['error'] = $originalError
        $fpDetails['stage'] = $tag+'-failed'
        $focus['failure_native'] = FPNativeFocusSnapshot ($tag+'-failure')
        $focus['window_list'] = FPFocusDiagnosticCommand ($tag+'-failure-window-list') @('--json','window-list')
        $focus['niri_windows'] = FPFocusDiagnosticCommand ($tag+'-failure-niri-windows') @('--json','windows')
        $fpDetails | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 (Join-Path $fpDir ($tag+'-failure.json'))
        # A separate diagnostic action may restore the sole fixture tile. It
        # never satisfies this precondition or any gate: always throw below.
        if ([LT]::Title($h) -ceq 'lane-h82-tile' -and $null -eq $command.error) {
            $focus['separate_focus_tiling_probe'] = FPFocusDiagnosticCommand ($tag+'-diagnostic-only-focus-tiling') @('--json','action','focus-tiling')
            $focus.separate_focus_tiling_probe['restored_original_target'] = WaitForeground $h 200 3000
            $focus.separate_focus_tiling_probe['after_wait'] = FPNativeFocusSnapshot ($tag+'-diagnostic-only-after-wait')
        }
        $fpDetails | ConvertTo-Json -Depth 20 | Set-Content -Encoding UTF8 (Join-Path $fpDir ($tag+'-diagnostics.json'))
        throw $originalError
    }
    function FPOpen([string]$name) {
        $h = [TestForms]::Open($name, 80, 130, 200)
        [void]$fpForms.Add($h)
        Start-Sleep -Milliseconds 1200
        FPFocus $h
        return $h
    }
    function FPEntry([string]$tag, [IntPtr]$h) {
        FPOwned $h
        $reply = (FPCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "H82 window-list failed: $tag" }
        $entry = @($reply.result | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($entry.Count -ne 1) { throw "H82 expected one managed entry for $h, got $($entry.Count)" }
        return $entry[0]
    }
    function FPNear([double]$a, [double]$b, [double]$tol = 5) {
        return [Math]::Abs($a - $b) -le $tol
    }
    function FPSame($a, $b) {
        return (FPNear $a.L $b.L) -and (FPNear $a.T $b.T) -and
            (FPNear $a.W $b.W) -and (FPNear $a.H $b.H)
    }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H82 refuses an existing wiri instance'
        }
        if (-not ('LaneFloatingPositionNative' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneFloatingPositionNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L,T,R,B; }
    [StructLayout(LayoutKind.Sequential)] public struct INFO {
        public uint cbSize; public RECT monitor, work; public uint flags;
    }
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr hwnd, uint flags);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern bool GetMonitorInfo(IntPtr monitor, ref INFO info);
    [DllImport("shcore.dll")] public static extern int GetDpiForMonitor(IntPtr monitor, int kind, out uint x, out uint y);
    public static INFO Info(IntPtr monitor) {
        INFO info = new INFO(); info.cbSize = (uint)Marshal.SizeOf(typeof(INFO));
        if (!GetMonitorInfo(monitor, ref info)) throw new InvalidOperationException("GetMonitorInfo failed");
        return info;
    }
}
'@ -ErrorAction Stop
        }
        $fpOldDpi = [LaneFloatingPositionNative]::SetThreadDpiAwarenessContext([IntPtr](-4))
        if ($fpOldDpi -eq [IntPtr]::Zero) { throw 'H82 cannot establish physical-pixel observation context' }
        $env:WIRI_PIPE = $fpPipe
        $fpConfig = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
window-rule { match title="^lane-h82-(float|focus)$"; open-floating true; }
'@
        $fpProcess = Start-WiriWithConfig $fpConfig (Join-Path $fpDir 'wiri.stderr.log') -NoDefaultBinds
        $null = $fpProcess.Handle
        if ($fpProcess.HasExited) { throw 'H82 daemon exited during startup' }
        $fpDetails['daemon_pid'] = $fpProcess.Id
        $fpDetails['pipe'] = $fpPipe
        $fpA = FPOpen 'lane-h82-float'
        $fpB = FPOpen 'lane-h82-focus'
        $fpT = FPOpen 'lane-h82-tile'
        if (-not (FPEntry 'float-entry' $fpA).floating -or -not (FPEntry 'focus-entry' $fpB).floating -or
            (FPEntry 'tile-entry' $fpT).floating) { throw 'H82 fixture layers do not match the configuration' }
        FPFocus $fpB
        $fpMonitor = [LaneFloatingPositionNative]::MonitorFromWindow($fpA, 2)
        $fpInfo = [LaneFloatingPositionNative]::Info($fpMonitor)
        $fpWork = $fpInfo.work
        [uint32]$fpDpiX = 0; [uint32]$fpDpiY = 0
        $fpDpiRc = [LaneFloatingPositionNative]::GetDpiForMonitor($fpMonitor, 0, [ref]$fpDpiX, [ref]$fpDpiY)
        if ($fpDpiRc -ne 0 -or $fpDpiX -eq 0 -or $fpDpiX -ne $fpDpiY) {
            throw 'H82 cannot obtain the monitor DPI needed for logical-position expectations'
        }
        $fpScale = [double]$fpDpiX / 96.0
        $fpDetails['geometry_context'] = @{ work = $fpWork; dpi_x = $fpDpiX; dpi_y = $fpDpiY;
            scale = $fpScale; nonzero_origin_exercised = ($fpWork.L -ne 0 -or $fpWork.T -ne 0) }
        $fpOtherBefore = Rc $fpB
        $fpForegroundBefore = [LT]::GetForegroundWindow()
        FPTyped 'typed-fixed' $fpA @{ SetFixed = 80.0 } @{ SetFixed = 70.0 }
        $fpFixed = Rc $fpA
        FPGate 'h82_typed_fixed_position_and_origin' (
            (FPNear $fpFixed.L ($fpWork.L + 80 * $fpScale)) -and
            (FPNear $fpFixed.T ($fpWork.T + 70 * $fpScale))) @{
                actual = $fpFixed; work = $fpWork; scale = $fpScale; logical_x = 80; logical_y = 70 }
        FPTyped 'typed-proportions' $fpA @{ SetProportion = 50.0 } @{ SetProportion = 25.0 }
        $fpProportional = Rc $fpA
        FPGate 'h82_typed_full_area_proportions' (
            (FPNear $fpProportional.L ($fpWork.L + ($fpWork.R - $fpWork.L) * 0.5)) -and
            (FPNear $fpProportional.T ($fpWork.T + ($fpWork.B - $fpWork.T) * 0.25))) @{
                actual = $fpProportional; work = $fpWork; proportions = @(50, 25) }
        $fpOtherAfter = Rc $fpB
        $fpForegroundAfter = [LT]::GetForegroundWindow()
        FPGate 'h82_explicit_inactive_id_preserves_foreground_and_unrelated_geometry' (
            $fpForegroundBefore -eq $fpB -and $fpForegroundAfter -eq $fpB -and
            (FPSame $fpOtherBefore $fpOtherAfter)) @{
                inactive_target = [int64]$fpA; foreground_before = [int64]$fpForegroundBefore;
                foreground_after = [int64]$fpForegroundAfter; unrelated_before = $fpOtherBefore; unrelated_after = $fpOtherAfter }
        $null = FPCtl 'cli-direct-signed-x' @('msg', '--json', 'move-floating-window', '--id', [string][int64]$fpA, '--x', '-10')
        Start-Sleep -Milliseconds 550
        $fpSigned = Rc $fpA
        FPGate 'h82_cli_signed_axis_preserves_omitted_axis' (
            (FPNear $fpSigned.L ($fpProportional.L - 10 * $fpScale)) -and
            (FPNear $fpSigned.T $fpProportional.T) -and (FPNear $fpSigned.W $fpProportional.W) -and
            (FPNear $fpSigned.H $fpProportional.H)) @{ before = $fpProportional; after = $fpSigned; logical_dx = -10 }

        FPTyped 'partial-left' $fpA @{ SetFixed = -1000000.0 } @{ SetFixed = 70.0 }
        $fpPartial = Rc $fpA
        $fpVisibleX = [Math]::Min(75.0, [Math]::Max(10.0, $fpPartial.W / $fpScale / 4.0)) * $fpScale
        $fpExpectedLeft = $fpWork.L - [Math]::Max(0.0, $fpPartial.W - $fpVisibleX)
        FPGate 'h82_partial_visibility_bounds' (
            (FPNear $fpPartial.L $fpExpectedLeft) -and $fpPartial.L -lt $fpWork.L -and
            (FPNear ($fpPartial.R - $fpWork.L) $fpVisibleX)) @{
                actual = $fpPartial; expected_left = $fpExpectedLeft; visible_physical = $fpVisibleX; scale = $fpScale }
        FPAction 'resize-partial-height-zero' @('set-window-height', '--id', [string][int64]$fpA, '+0')
        $fpResized = Rc $fpA
        FPGate 'h82_partial_position_survives_resize' (FPSame $fpPartial $fpResized) @{
            before = $fpPartial; after = $fpResized; resize = '+0' }
        # Reset placement so a failed resize gate cannot make the refresh gate vacuous.
        FPTyped 'partial-left-before-refresh' $fpA @{ SetFixed = -1000000.0 } @{ SetFixed = 70.0 }
        $fpBeforeRefresh = Rc $fpA
        FPAction 'refresh-partial' @('refresh')
        $fpRefreshed = Rc $fpA
        FPGate 'h82_partial_position_survives_refresh' (
            $fpBeforeRefresh.L -lt $fpWork.L -and (FPSame $fpBeforeRefresh $fpRefreshed)) @{
                before = $fpBeforeRefresh; after = $fpRefreshed }

        FPFocus $fpT
        $fpTileBefore = Rc $fpT
        $fpTileEntryBefore = FPEntry 'tile-before-store' $fpT
        FPTyped 'tile-store-both-axes' $fpT @{ SetFixed = 110.0 } @{ SetFixed = 90.0 }
        $fpTileAfter = Rc $fpT
        $fpTileEntryAfter = FPEntry 'tile-after-store' $fpT
        FPGate 'h82_tiled_position_is_stored_until_float' (
            -not $fpTileEntryBefore.floating -and -not $fpTileEntryAfter.floating -and
            $fpTileEntryBefore.column -eq $fpTileEntryAfter.column -and
            (FPSame $fpTileBefore $fpTileAfter) -and [LT]::GetForegroundWindow() -eq $fpT) @{
                before = $fpTileBefore; after = $fpTileAfter; column_before = $fpTileEntryBefore.column;
                column_after = $fpTileEntryAfter.column; floating = $fpTileEntryAfter.floating }
        FPOwned $fpT
        $null = FPCtl 'float-stored-tile' @('float-window', [string][int64]$fpT)
        Start-Sleep -Milliseconds 650
        $fpRestored = Rc $fpT
        $fpRestoredEntry = FPEntry 'stored-float-entry' $fpT
        FPGate 'h82_stored_float_position_restores' (
            $fpRestoredEntry.floating -and
            (FPNear $fpRestored.L ($fpWork.L + 110 * $fpScale)) -and
            (FPNear $fpRestored.T ($fpWork.T + 90 * $fpScale))) @{
                actual = $fpRestored; work = $fpWork; scale = $fpScale; stored_logical = @(110, 90) }
        $fpDetails['stage'] = 'gates-complete'
    } catch {
        $fpDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        foreach ($fpForm in $fpForms) {
            try {
                if ([LT]::IsWindow($fpForm) -and [LT]::Pid($fpForm) -eq $PID) { [TestForms]::CloseForm($fpForm) }
            } catch { $fpDetails['form_cleanup_error'] = [string]$_ }
        }
        try {
            if ($null -ne $fpProcess -and -not $fpProcess.HasExited) {
                if ($env:WIRI_PIPE -eq $fpPipe) { Stop-WiriGracefully $fpProcess 'h82-owned' }
                elseif (-not $fpProcess.HasExited) { $fpProcess.Kill(); $null = $fpProcess.WaitForExit(5000) }
            }
        } catch {
            $fpDetails['daemon_cleanup_error'] = [string]$_
            if ($null -ne $fpProcess -and -not $fpProcess.HasExited) {
                try { $fpProcess.Kill(); $null = $fpProcess.WaitForExit(5000) } catch { $fpDetails['daemon_kill_error'] = [string]$_ }
            }
        } finally {
            $env:WIRI_PIPE = $fpSavedPipe
            if ($null -ne $fpSavedConfig) { [IO.File]::WriteAllText($cfg, $fpSavedConfig, (New-Object Text.UTF8Encoding($false))) }
            if ($fpOldDpi -ne [IntPtr]::Zero) { [void][LaneFloatingPositionNative]::SetThreadDpiAwarenessContext($fpOldDpi) }
            $fpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fpRoot 'evidence.json')
            $summary['lane_floating_position_details'] = $fpDetails
        }
    }
}
