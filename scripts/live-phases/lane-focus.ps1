# H92: the focus core.
#   (a) an explicit `focus-window <id>` makes the managed (IPC) focus follow the native foreground in one request,
#       with no retry: official run aa6c884 recorded Windows accepting SetForegroundWindow for a form, raising no
#       EVENT_SYSTEM_FOREGROUND, and managed focus staying on the previous window for good;
#   (b) niri's *ById actions (niri:src/input/mod.rs 1110-1136, 1684-1736, 2111-2117, 2213-2250) change the named
#       window in place and never move focus: native foreground and managed focus stay on the focused form.
# The parent owns the global live-test mutex and isolated APPDATA. Only lane-owned forms are focused, floated,
# moved or closed; the retained daemon owns its pipe. Every gate group has its own try/catch.
& {
    $fcRoot = Join-Path $D 'lane-focus'
    $fcDir = Join-Path $fcRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $fcDir
    $fcDetails = [ordered]@{ fixture_directory = $fcDir }
    $fcGates = @(
        'h92_focus_window_converges_managed_focus_without_retry',
        'h92_by_id_consume_or_expel_keeps_focus',
        'h92_by_id_center_window_centres_only_the_active_column',
        'h92_by_id_reset_window_height_keeps_focus',
        'h92_by_id_switch_preset_window_width_keeps_focus',
        'h92_by_id_toggle_window_floating_keeps_focus',
        'h92_by_id_move_window_to_floating_and_tiling_keep_focus'
    )
    foreach ($fcGate in $fcGates) {
        $summary[$fcGate] = $false
        $fcDetails[$fcGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $fcProcess = $null
    $fcForms = New-Object System.Collections.ArrayList
    $fcSavedPipe = $env:WIRI_PIPE
    $fcPipe = '\\.\pipe\wiri_live_h92_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $fcSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    function FCGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $fcDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $fcDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fcRoot 'evidence.json')
    }
    function FCOwned([IntPtr]$h) {
        if (-not $fcForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "H92 refuses an unowned form: $h"
        }
    }
    function FCCtl([string]$tag, [string[]]$words) {
        if ($null -eq $fcProcess -or $fcProcess.HasExited -or $env:WIRI_PIPE -ne $fcPipe) {
            throw 'H92 retained daemon/pipe ownership is no longer valid'
        }
        $text = Ctl ('h92-' + $tag) $words
        if ($steps.Count -eq 0 -or [string]$steps[$steps.Count - 1].rc -ne '0') {
            throw "H92 CLI failed: $tag $text"
        }
        return $text
    }
    function FCAction([string]$tag, [string[]]$words) {
        $reply = (FCCtl $tag (@('--json', 'action') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) {
            throw "H92 action was rejected: $tag $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 900
    }
    # One focus request, no retry: the gate under test is that this alone settles managed focus.
    function FCFocusOnce([IntPtr]$h, [string]$tag) {
        FCOwned $h
        $null = FCCtl $tag @('--json', 'focus-window', [string][int64]$h)
    }
    function FCFocusSettled([IntPtr]$h) {
        FCOwned $h
        foreach ($attempt in 1..3) {
            $null = FCCtl ('settle-' + $h.ToInt64() + '-' + $attempt) @('--json', 'focus-window', [string][int64]$h)
            if ((WaitForeground $h 900 4000) -and (FCManagedWaits $h 2500)) { Start-Sleep -Milliseconds 250; return }
        }
        throw "H92 owned form did not gain focus: $h"
    }
    function FCManaged() {
        $query = (PipeRequest '{"type":"focused_window"}') | ConvertFrom-Json
        if ($query.success -and $null -ne $query.result.focused_window) { return [int64]$query.result.focused_window.id }
        return $null
    }
    # Wait up to $maxMs for the managed focus to be $h.
    function FCManagedWaits([IntPtr]$h, [int]$maxMs) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ((FCManaged) -eq [int64]$h) { return $true }
            Start-Sleep -Milliseconds 40
        } while ($clock.ElapsedMilliseconds -lt $maxMs)
        return ((FCManaged) -eq [int64]$h)
    }
    function FCOpen([string]$name) {
        $h = [TestForms]::Open($name, 90, 120, 200)
        [void]$fcForms.Add($h)
        Start-Sleep -Milliseconds 1200
        return $h
    }
    function FCEntry([string]$tag, [IntPtr]$h) {
        FCOwned $h
        $reply = (FCCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "H92 window-list failed: $tag" }
        $entry = @($reply.result | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($entry.Count -ne 1) { throw "H92 expected one managed entry for $h, got $($entry.Count)" }
        return $entry[0]
    }
    function FCNear([double]$a, [double]$b, [double]$tol = 8) { return [Math]::Abs($a - $b) -le $tol }
    # Native and managed focus are still on $h, now and after a pause (a late notice would show then).
    function FCFocusKept([IntPtr]$h) {
        $native = ([LT]::GetForegroundWindow() -eq $h)
        $managed = ((FCManaged) -eq [int64]$h)
        Start-Sleep -Milliseconds 700
        $nativeLater = ([LT]::GetForegroundWindow() -eq $h)
        $managedLater = ((FCManaged) -eq [int64]$h)
        return [ordered]@{ pass = ($native -and $managed -and $nativeLater -and $managedLater)
            native = $native; managed = $managed; native_later = $nativeLater; managed_later = $managedLater }
    }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H92 refuses an existing wiri instance'
        }
        $env:WIRI_PIPE = $fcPipe
        $fcConfig = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
'@
        $fcProcess = Start-WiriWithConfig $fcConfig (Join-Path $fcDir 'wiri.stderr.log') -NoDefaultBinds
        $null = $fcProcess.Handle
        if ($fcProcess.HasExited) { throw 'H92 daemon exited during startup' }
        $fcDetails['daemon_pid'] = $fcProcess.Id
        $fcDetails['pipe'] = $fcPipe
        $fcA = FCOpen 'lane-h92-a'
        $fcB = FCOpen 'lane-h92-b'
        $fcC = FCOpen 'lane-h92-c'
        $fcEntryA = FCEntry 'a-entry' $fcA
        $fcEntryB = FCEntry 'b-entry' $fcB
        $fcEntryC = FCEntry 'c-entry' $fcC
        if ($fcEntryA.floating -eq $true -or $fcEntryB.floating -eq $true -or $fcEntryC.floating -eq $true -or
            [int]$fcEntryB.column -ne ([int]$fcEntryA.column + 1) -or [int]$fcEntryC.column -ne ([int]$fcEntryB.column + 1)) {
            throw 'H92 fixture is not three tiled columns A | B | C'
        }
        $fcWork = [pscustomobject]@{ L = $swa.Left; T = $swa.Top; R = $swa.Right; B = $swa.Bottom }

        # Group 1: a single focus-window request settles managed focus on the native foreground. Six
        # requests, each to a different window than the one before; nothing retries.
        try {
            FCFocusSettled $fcA
            $fcSteps = @()
            $fcAll = $true
            foreach ($fcTarget in @($fcB, $fcC, $fcA, $fcC, $fcB, $fcA)) {
                FCFocusOnce $fcTarget ('g1-focus-' + $fcTarget.ToInt64())
                $fcNative = (WaitForeground $fcTarget 600 4000)
                $fcManagedSettled = (FCManagedWaits $fcTarget 2500)
                $fcSteps += [ordered]@{ target = $fcTarget.ToInt64(); native = $fcNative; managed = $fcManagedSettled
                    managed_id = (FCManaged); foreground = [LT]::GetForegroundWindow().ToInt64() }
                if (-not ($fcNative -and $fcManagedSettled)) { $fcAll = $false }
            }
            FCGate 'h92_focus_window_converges_managed_focus_without_retry' $fcAll @{ steps = $fcSteps }
        } catch { $fcDetails['group_converge_error'] = ($_ | Out-String).Trim() }

        # Group 2: consume-or-expel-window-left/right --id C: C joins B's column and leaves it again.
        try {
            FCFocusSettled $fcA
            FCAction 'g2-consume-left' @('consume-or-expel-window-left', '--id', [string][int64]$fcC)
            $fcJoined = FCEntry 'g2-joined' $fcC
            $fcBJoined = FCEntry 'g2-b-joined' $fcB
            $fcKeptJoin = FCFocusKept $fcA
            FCAction 'g2-expel-right' @('consume-or-expel-window-right', '--id', [string][int64]$fcC)
            $fcExpelled = FCEntry 'g2-expelled' $fcC
            $fcBExpelled = FCEntry 'g2-b-expelled' $fcB
            $fcKeptExpel = FCFocusKept $fcA
            FCGate 'h92_by_id_consume_or_expel_keeps_focus' (
                [int]$fcJoined.column -eq [int]$fcBJoined.column -and
                [int]$fcExpelled.column -eq ([int]$fcBExpelled.column + 1) -and $fcKeptJoin.pass -and $fcKeptExpel.pass) @{
                    joined = $fcJoined; b_joined = $fcBJoined; expelled = $fcExpelled; b_expelled = $fcBExpelled
                    kept_join = $fcKeptJoin; kept_expel = $fcKeptExpel }
        } catch { $fcDetails['group_consume_error'] = ($_ | Out-String).Trim() }

        # Group 3: center-window --id: a column that is not the active one stays; the active one is centred.
        try {
            FCFocusSettled $fcA
            $fcBeforeB = Rc $fcB
            $fcBeforeA = Rc $fcA
            FCAction 'g3-center-b' @('center-window', '--id', [string][int64]$fcB)
            $fcAfterB = Rc $fcB
            $fcAfterBKept = FCFocusKept $fcA
            FCAction 'g3-center-a' @('center-window', '--id', [string][int64]$fcA)
            $fcAfterA = Rc $fcA
            $fcAfterAKept = FCFocusKept $fcA
            $fcWorkCenter = ($fcWork.L + $fcWork.R) / 2
            FCGate 'h92_by_id_center_window_centres_only_the_active_column' (
                (FCNear $fcBeforeB.L $fcAfterB.L) -and (FCNear $fcBeforeB.W $fcAfterB.W) -and
                (FCNear ($fcAfterA.L + $fcAfterA.W / 2) $fcWorkCenter 12) -and $fcAfterBKept.pass -and $fcAfterAKept.pass) @{
                    before_b = $fcBeforeB; after_b = $fcAfterB; before_a = $fcBeforeA; after_a = $fcAfterA
                    work = $fcWork; kept_b = $fcAfterBKept; kept_a = $fcAfterAKept }
        } catch { $fcDetails['group_center_error'] = ($_ | Out-String).Trim() }

        # Group 4: reset-window-height --id: C shares B's column, is sized by id, then reset by id.
        try {
            FCFocusSettled $fcA
            FCAction 'g4-consume' @('consume-or-expel-window-left', '--id', [string][int64]$fcC)
            $fcHeightStart = Rc $fcC
            FCAction 'g4-set-height' @('set-window-height', '--id', [string][int64]$fcC, '300')
            $fcHeightSet = Rc $fcC
            FCAction 'g4-reset' @('reset-window-height', '--id', [string][int64]$fcC)
            $fcHeightReset = Rc $fcC
            $fcHeightKept = FCFocusKept $fcA
            FCAction 'g4-expel' @('consume-or-expel-window-right', '--id', [string][int64]$fcC)
            FCGate 'h92_by_id_reset_window_height_keeps_focus' (
                -not (FCNear $fcHeightStart.H $fcHeightSet.H 12) -and (FCNear $fcHeightStart.H $fcHeightReset.H 12) -and
                $fcHeightKept.pass) @{ start = $fcHeightStart; set = $fcHeightSet; reset = $fcHeightReset; kept = $fcHeightKept }
        } catch { $fcDetails['group_reset_error'] = ($_ | Out-String).Trim() }

        # Group 5: switch-preset-window-width and -back --id C change C's column width only.
        try {
            FCFocusSettled $fcA
            $fcWidthA = Rc $fcA
            $fcWidth0 = Rc $fcC
            FCAction 'g5-width-forward' @('switch-preset-window-width', '--id', [string][int64]$fcC)
            $fcWidth1 = Rc $fcC
            $fcWidth1Kept = FCFocusKept $fcA
            FCAction 'g5-width-back' @('switch-preset-window-width-back', '--id', [string][int64]$fcC)
            $fcWidth2 = Rc $fcC
            $fcWidth2Kept = FCFocusKept $fcA
            $fcWidthAAfter = Rc $fcA
            FCGate 'h92_by_id_switch_preset_window_width_keeps_focus' (
                -not (FCNear $fcWidth0.W $fcWidth1.W 6) -and -not (FCNear $fcWidth1.W $fcWidth2.W 6) -and
                (FCNear $fcWidthA.W $fcWidthAAfter.W 6) -and $fcWidth1Kept.pass -and $fcWidth2Kept.pass) @{
                    before = $fcWidth0; forward = $fcWidth1; back = $fcWidth2; a_before = $fcWidthA; a_after = $fcWidthAAfter
                    kept_forward = $fcWidth1Kept; kept_back = $fcWidth2Kept }
        } catch { $fcDetails['group_width_error'] = ($_ | Out-String).Trim() }

        # Group 6: toggle-window-floating --id B floats it and tiles it again, focus never moves.
        try {
            FCFocusSettled $fcA
            FCAction 'g6-float' @('toggle-window-floating', '--id', [string][int64]$fcB)
            $fcFloated = FCEntry 'g6-floated' $fcB
            $fcFloatKept = FCFocusKept $fcA
            FCAction 'g6-tile' @('toggle-window-floating', '--id', [string][int64]$fcB)
            $fcTiled = FCEntry 'g6-tiled' $fcB
            $fcTileKept = FCFocusKept $fcA
            FCGate 'h92_by_id_toggle_window_floating_keeps_focus' (
                $fcFloated.floating -eq $true -and $fcTiled.floating -eq $false -and $null -ne $fcTiled.column -and
                [int]$fcTiled.column -ge 0 -and $fcFloatKept.pass -and $fcTileKept.pass) @{
                    floated = $fcFloated; tiled = $fcTiled; kept_float = $fcFloatKept; kept_tile = $fcTileKept }
        } catch { $fcDetails['group_toggle_error'] = ($_ | Out-String).Trim() }

        # Group 7: move-window-to-floating / -to-tiling --id C, idempotent, focus never moves.
        try {
            FCFocusSettled $fcA
            FCAction 'g7-to-floating' @('move-window-to-floating', '--id', [string][int64]$fcC)
            $fcMovedFloat = FCEntry 'g7-floating' $fcC
            FCAction 'g7-to-floating-again' @('move-window-to-floating', '--id', [string][int64]$fcC)
            $fcMovedFloatAgain = FCEntry 'g7-floating-again' $fcC
            $fcMoveFloatKept = FCFocusKept $fcA
            FCAction 'g7-to-tiling' @('move-window-to-tiling', '--id', [string][int64]$fcC)
            $fcMovedTile = FCEntry 'g7-tiling' $fcC
            $fcMoveTileKept = FCFocusKept $fcA
            FCGate 'h92_by_id_move_window_to_floating_and_tiling_keep_focus' (
                $fcMovedFloat.floating -eq $true -and $fcMovedFloatAgain.floating -eq $true -and
                $fcMovedTile.floating -eq $false -and $fcMoveFloatKept.pass -and $fcMoveTileKept.pass) @{
                    floating = $fcMovedFloat; floating_again = $fcMovedFloatAgain; tiling = $fcMovedTile
                    kept_float = $fcMoveFloatKept; kept_tile = $fcMoveTileKept }
        } catch { $fcDetails['group_move_error'] = ($_ | Out-String).Trim() }
        $fcDetails['stage'] = 'gates-complete'
    } catch {
        $fcDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        foreach ($fcForm in $fcForms) {
            try {
                if ([LT]::IsWindow($fcForm) -and [LT]::Pid($fcForm) -eq $PID) { [TestForms]::CloseForm($fcForm) }
            } catch { $fcDetails['form_cleanup_error'] = [string]$_ }
        }
        try {
            if ($null -ne $fcProcess -and -not $fcProcess.HasExited) {
                if ($env:WIRI_PIPE -eq $fcPipe) { Stop-WiriGracefully $fcProcess 'h92-owned' }
                elseif (-not $fcProcess.HasExited) { $fcProcess.Kill(); $null = $fcProcess.WaitForExit(5000) }
            }
        } catch {
            $fcDetails['daemon_cleanup_error'] = [string]$_
            if ($null -ne $fcProcess -and -not $fcProcess.HasExited) {
                try { $fcProcess.Kill(); $null = $fcProcess.WaitForExit(5000) } catch { $fcDetails['daemon_kill_error'] = [string]$_ }
            }
        } finally {
            $env:WIRI_PIPE = $fcSavedPipe
            if ($null -ne $fcSavedConfig) { [IO.File]::WriteAllText($cfg, $fcSavedConfig, (New-Object Text.UTF8Encoding($false))) }
            $fcDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fcRoot 'evidence.json')
            $summary['lane_focus_details'] = $fcDetails
        }
    }
}

# H92 (new-window focus policy): niri's default heuristic (a new window is focused unless the active window is
# fullscreen) and `debug { strict-new-window-focus-policy }` (only a window Windows itself gave the foreground is
# focused), mapped to Windows (niri:src/handlers/compositor.rs:149-176). A window "without a token" is one that
# appeared without the foreground: these gates open one with ShowWithoutActivation. The other half, a window Windows
# did give the foreground (niri's valid token), has no deterministic desktop trigger and is unit-proven only.
# One private daemon per policy; only lane-owned forms are focused, fullscreened or closed.
& {
    $fpRoot = Join-Path $D 'lane-focus-policy'
    $fpDir = Join-Path $fpRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $fpDir
    $fpDetails = [ordered]@{ fixture_directory = $fpDir }
    $fpGates = @(
        'h92_new_window_default_heuristic_focuses_a_window_without_the_foreground',
        'h92_new_window_default_heuristic_keeps_a_fullscreen_window_focused',
        'h92_new_window_strict_policy_does_not_focus_a_window_without_the_foreground',
        'h92_new_window_strict_policy_still_honours_an_explicit_focus_request'
    )
    foreach ($fpGate in $fpGates) {
        $summary[$fpGate] = $false
        $fpDetails[$fpGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $fpForms = New-Object System.Collections.ArrayList
    $fpSavedPipe = $env:WIRI_PIPE
    $fpSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    $fpState = @{ process = $null; pipe = '' }
    function FPGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $fpDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $fpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fpRoot 'evidence.json')
    }
    function FPCtl([string]$tag, [string[]]$words) {
        $process = $fpState.process
        if ($null -eq $process -or $process.HasExited -or $env:WIRI_PIPE -ne $fpState.pipe) {
            throw 'H92 policy daemon/pipe ownership is no longer valid'
        }
        $text = Ctl ('h92p-' + $tag) $words
        if ($steps.Count -eq 0 -or [string]$steps[$steps.Count - 1].rc -ne '0') {
            throw "H92 policy CLI failed: $tag $text"
        }
        return $text
    }
    function FPAction([string]$tag, [string[]]$words) {
        $reply = (FPCtl $tag (@('--json', 'action') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) {
            throw "H92 policy action was rejected: $tag $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 900
    }
    function FPManaged() {
        $query = (PipeRequest '{"type":"focused_window"}') | ConvertFrom-Json
        if ($query.success -and $null -ne $query.result.focused_window) { return [int64]$query.result.focused_window.id }
        return $null
    }
    function FPManagedWaits([IntPtr]$h, [int]$maxMs) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ((FPManaged) -eq [int64]$h) { return $true }
            Start-Sleep -Milliseconds 40
        } while ($clock.ElapsedMilliseconds -lt $maxMs)
        return ((FPManaged) -eq [int64]$h)
    }
    function FPFocusSettled([IntPtr]$h) {
        foreach ($attempt in 1..3) {
            $null = FPCtl ('settle-' + $h.ToInt64() + '-' + $attempt) @('--json', 'focus-window', [string][int64]$h)
            if ((WaitForeground $h 900 4000) -and (FPManagedWaits $h 2500)) { Start-Sleep -Milliseconds 250; return }
        }
        throw "H92 policy form did not gain focus: $h"
    }
    function FPOpen([string]$name) {
        $h = [TestForms]::Open($name, 90, 120, 200)
        [void]$fpForms.Add($h)
        Start-Sleep -Milliseconds 1200
        return $h
    }
    function FPOpenQuiet([string]$name) {
        $h = [LaneFocusForms]::OpenNoActivate($name)
        [void]$fpForms.Add($h)
        # Admission, a possible native activation and the stream settle.
        Start-Sleep -Milliseconds 2200
        return $h
    }
    function FPManagedEntry([string]$tag, [IntPtr]$h) {
        $reply = (FPCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        $entry = @($reply.result | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        return $(if ($entry.Count -eq 1) { $entry[0] } else { $null })
    }
    function FPStart([string]$tag, [string]$configText) {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H92 policy refuses an existing wiri instance'
        }
        $fpState.pipe = '\\.\pipe\wiri_live_h92p_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $env:WIRI_PIPE = $fpState.pipe
        $fpState.process = Start-WiriWithConfig $configText (Join-Path $fpDir ('wiri.' + $tag + '.stderr.log')) -NoDefaultBinds
        $null = $fpState.process.Handle
        if ($fpState.process.HasExited) { throw "H92 policy daemon exited during startup: $tag" }
        $fpDetails[$tag + '_daemon_pid'] = $fpState.process.Id
    }
    function FPStop([string]$tag) {
        foreach ($fpForm in @($fpForms)) {
            try {
                if ([LT]::IsWindow($fpForm) -and [LT]::Pid($fpForm) -eq $PID) { [LaneFocusForms]::Close($fpForm); [TestForms]::CloseForm($fpForm) }
            } catch { $fpDetails['form_cleanup_error'] = [string]$_ }
        }
        $fpForms.Clear()
        Start-Sleep -Milliseconds 600
        $process = $fpState.process
        try {
            if ($null -ne $process -and -not $process.HasExited) {
                if ($env:WIRI_PIPE -eq $fpState.pipe) { Stop-WiriGracefully $process ('h92p-' + $tag) }
                elseif (-not $process.HasExited) { $process.Kill(); $null = $process.WaitForExit(5000) }
            }
        } catch {
            $fpDetails[$tag + '_daemon_cleanup_error'] = [string]$_
            if ($null -ne $process -and -not $process.HasExited) {
                try { $process.Kill(); $null = $process.WaitForExit(5000) } catch { $fpDetails[$tag + '_daemon_kill_error'] = [string]$_ }
            }
        }
        $fpState.process = $null
    }
    try {
        if (-not ('LaneFocusForms' -as [type])) {
            Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Threading;
using System.Windows.Forms;
public class LaneFocusNoActivateForm : Form {
  protected override bool ShowWithoutActivation { get { return true; } }
}
public static class LaneFocusForms {
  static readonly List<Form> forms = new List<Form>();
  // A window that appears without taking the foreground: a background launch.
  public static IntPtr OpenNoActivate(string title) {
    IntPtr handle = IntPtr.Zero;
    var ready = new ManualResetEvent(false);
    var t = new Thread(delegate() {
      var f = new LaneFocusNoActivateForm();
      f.Text = title; f.Width = 520; f.Height = 380;
      f.BackColor = Color.FromArgb(40, 120, 90);
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
  public static void Close(IntPtr h) {
    lock (forms) {
      foreach (var f in forms) {
        if (f.IsHandleCreated && f.Handle == h) { var ff = f; ff.BeginInvoke((MethodInvoker)delegate() { ff.Close(); }); }
      }
    }
  }
}
'@ -ErrorAction Stop
        }

        # Phase 1: the default heuristic.
        try {
            FPStart 'default' @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
'@
            $fpA = FPOpen 'lane-h92p-a'
            FPFocusSettled $fpA
            # Group 1: a background launch is focused by default.
            try {
                $fpB = FPOpenQuiet 'lane-h92p-b'
                $fpBEntry = FPManagedEntry 'g1-b-entry' $fpB
                $fpBManaged = (FPManagedWaits $fpB 2500)
                $fpBNative = ([LT]::GetForegroundWindow() -eq $fpB)
                FPGate 'h92_new_window_default_heuristic_focuses_a_window_without_the_foreground' (
                    $null -ne $fpBEntry -and $fpBManaged -and $fpBNative) @{
                        entry = $fpBEntry; managed = $fpBManaged; native = $fpBNative; managed_id = (FPManaged)
                        foreground = [LT]::GetForegroundWindow().ToInt64() }
            } catch { $fpDetails['group_default_error'] = ($_ | Out-String).Trim() }
            # Group 2: a fullscreen window keeps the focus against a background launch.
            try {
                FPFocusSettled $fpA
                FPAction 'g2-fullscreen-on' @('fullscreen-window')
                $fpAFullscreen = FPManagedEntry 'g2-a-fullscreen' $fpA
                $fpC = FPOpenQuiet 'lane-h92p-c'
                $fpCEntry = FPManagedEntry 'g2-c-entry' $fpC
                $fpKeptManaged = ((FPManaged) -eq [int64]$fpA)
                $fpKeptNative = ([LT]::GetForegroundWindow() -eq $fpA)
                Start-Sleep -Milliseconds 900
                $fpKeptLater = (((FPManaged) -eq [int64]$fpA) -and ([LT]::GetForegroundWindow() -eq $fpA))
                $fpAAfter = FPManagedEntry 'g2-a-after' $fpA
                FPGate 'h92_new_window_default_heuristic_keeps_a_fullscreen_window_focused' (
                    $null -ne $fpAFullscreen -and $fpAFullscreen.fullscreen -eq $true -and $null -ne $fpCEntry -and
                    $fpKeptManaged -and $fpKeptNative -and $fpKeptLater -and $null -ne $fpAAfter -and
                    $fpAAfter.fullscreen -eq $true) @{
                        a_fullscreen = $fpAFullscreen; c_entry = $fpCEntry; kept_managed = $fpKeptManaged
                        kept_native = $fpKeptNative; kept_later = $fpKeptLater; a_after = $fpAAfter }
                FPAction 'g2-fullscreen-off' @('fullscreen-window')
            } catch { $fpDetails['group_fullscreen_error'] = ($_ | Out-String).Trim() }
        } catch { $fpDetails['phase_default_error'] = ($_ | Out-String).Trim() }
        finally { FPStop 'default' }

        # Phase 2: debug { strict-new-window-focus-policy }.
        try {
            FPStart 'strict' @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
debug { strict-new-window-focus-policy; }
'@
            $fpSA = FPOpen 'lane-h92p-sa'
            FPFocusSettled $fpSA
            $fpSB = $null
            # Group 3: a window that appeared without the foreground is not focused.
            try {
                $fpSB = FPOpenQuiet 'lane-h92p-sb'
                $fpSBEntry = FPManagedEntry 'g3-b-entry' $fpSB
                $fpSKeptManaged = ((FPManaged) -eq [int64]$fpSA)
                $fpSKeptNative = ([LT]::GetForegroundWindow() -eq $fpSA)
                Start-Sleep -Milliseconds 900
                $fpSKeptLater = (((FPManaged) -eq [int64]$fpSA) -and ([LT]::GetForegroundWindow() -eq $fpSA))
                FPGate 'h92_new_window_strict_policy_does_not_focus_a_window_without_the_foreground' (
                    $null -ne $fpSBEntry -and $fpSKeptManaged -and $fpSKeptNative -and $fpSKeptLater) @{
                        entry = $fpSBEntry; kept_managed = $fpSKeptManaged; kept_native = $fpSKeptNative
                        kept_later = $fpSKeptLater; managed_id = (FPManaged)
                        foreground = [LT]::GetForegroundWindow().ToInt64() }
            } catch { $fpDetails['group_strict_error'] = ($_ | Out-String).Trim() }
            # Group 4: an explicit focus request still reaches the unfocused window.
            try {
                if ($null -eq $fpSB) { throw 'H92 strict group 3 did not open its window' }
                $null = FPCtl 'g4-focus' @('--json', 'focus-window', [string][int64]$fpSB)
                $fpSNative = (WaitForeground $fpSB 900 4000)
                $fpSManaged = (FPManagedWaits $fpSB 2500)
                FPGate 'h92_new_window_strict_policy_still_honours_an_explicit_focus_request' (
                    $fpSNative -and $fpSManaged) @{ native = $fpSNative; managed = $fpSManaged; managed_id = (FPManaged) }
            } catch { $fpDetails['group_strict_focus_error'] = ($_ | Out-String).Trim() }
        } catch { $fpDetails['phase_strict_error'] = ($_ | Out-String).Trim() }
        finally { FPStop 'strict' }
        $fpDetails['stage'] = 'gates-complete'
    } catch {
        $fpDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        try { if ($null -ne $fpState.process -and -not $fpState.process.HasExited) { FPStop 'final' } } catch { $fpDetails['final_cleanup_error'] = [string]$_ }
        $env:WIRI_PIPE = $fpSavedPipe
        if ($null -ne $fpSavedConfig) { [IO.File]::WriteAllText($cfg, $fpSavedConfig, (New-Object Text.UTF8Encoding($false))) }
        $fpDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $fpRoot 'evidence.json')
        $summary['lane_focus_policy_details'] = $fpDetails
    }
}
