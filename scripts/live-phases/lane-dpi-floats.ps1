# H96: the floating layer under a fullscreen tile (niri workspace.rs:1722-1728 `is_floating_visible`,
# scrolling.rs:2946-2962 `fullscreen_window_above_top_layer`). niri does not draw the floating windows of a
# workspace while its active window is a fullscreen scrolling window and the floating layer is not the active
# one; focusing a float (or leaving fullscreen) brings them back, where they were. wiri hides them with its
# ordinary hide path (`layout { hide-method }`) and restores them as soon as niri would draw them.
# The parent owns the global live-test mutex and isolated APPDATA. Only lane-owned forms are focused, made
# fullscreen or closed; the retained daemon owns its pipe. Every gate group has its own try/catch. Nothing
# here changes a display or input setting.
& {
    $dpfRoot = Join-Path $D 'lane-dpi-floats'
    $dpfDir = Join-Path $dpfRoot ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $dpfDir
    $dpfDetails = [ordered]@{ fixture_directory = $dpfDir }
    $dpfGates = @(
        'h96_float_over_tile_is_visible_in_front_of_the_tile',
        'h96_fullscreen_tile_hides_the_floating_layer',
        'h96_focus_floating_shows_the_float_again_at_its_place',
        'h96_focusing_the_fullscreen_tile_hides_the_float_again',
        'h96_focus_window_by_id_reveals_the_hidden_float',
        'h96_unfullscreen_shows_the_float_again_at_its_place'
    )
    foreach ($dpfGate in $dpfGates) {
        $summary[$dpfGate] = $false
        $dpfDetails[$dpfGate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $dpfProcess = $null
    $dpfForms = New-Object System.Collections.ArrayList
    $dpfSavedPipe = $env:WIRI_PIPE
    $dpfPipe = '\\.\pipe\wiri_live_h96_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $dpfSavedConfig = if (Test-Path -LiteralPath $cfg) { [IO.File]::ReadAllText($cfg) } else { $null }
    function DPFGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $dpfDetails[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        $dpfDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $dpfRoot 'evidence.json')
    }
    function DPFOwned([IntPtr]$h) {
        if (-not $dpfForms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "H96 refuses an unowned form: $h"
        }
    }
    function DPFCtl([string]$tag, [string[]]$words) {
        if ($null -eq $dpfProcess -or $dpfProcess.HasExited -or $env:WIRI_PIPE -ne $dpfPipe) {
            throw 'H96 retained daemon/pipe ownership is no longer valid'
        }
        $text = Ctl ('h96-' + $tag) $words
        if ($steps.Count -eq 0 -or [string]$steps[$steps.Count - 1].rc -ne '0') {
            throw "H96 CLI failed: $tag $text"
        }
        return $text
    }
    function DPFAction([string]$tag, [string[]]$words) {
        $reply = (DPFCtl $tag (@('--json', 'action') + $words)) | ConvertFrom-Json
        if ($reply.success -ne $true) {
            throw "H96 action was rejected: $tag $($reply | ConvertTo-Json -Compress)"
        }
        Start-Sleep -Milliseconds 1500
    }
    function DPFFocus([IntPtr]$h) {
        DPFOwned $h
        foreach ($attempt in 1..2) {
            $null = DPFCtl ('focus-' + $h.ToInt64() + '-' + $attempt) @('--json', 'focus-window', [string][int64]$h)
            if (WaitForeground $h 900 4000) { Start-Sleep -Milliseconds 300; return }
        }
        throw "H96 owned form did not gain focus: $h"
    }
    function DPFOpen([string]$name) {
        $h = [TestForms]::Open($name, 80, 130, 200)
        [void]$dpfForms.Add($h)
        Start-Sleep -Milliseconds 1200
        return $h
    }
    function DPFEntry([string]$tag, [IntPtr]$h) {
        DPFOwned $h
        $reply = (DPFCtl $tag @('--json', 'window-list')) | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "H96 window-list failed: $tag" }
        $entry = @($reply.result | Where-Object { [int64]$_.hwnd -eq [int64]$h })
        if ($entry.Count -ne 1) { throw "H96 expected one managed entry for $h, got $($entry.Count)" }
        return $entry[0]
    }
    # Drawn: visible and not cloaked (the hide method decides which of the two hides a window).
    function DPFShown([IntPtr]$h) { return ([LT]::IsWindowVisible($h) -and [LT]::Cloaked($h) -le 0) }
    function DPFWaitShown([IntPtr]$h, [bool]$want, [int]$maxMs) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ((DPFShown $h) -ne $want -and $sw.ElapsedMilliseconds -lt $maxMs) { Start-Sleep -Milliseconds 50 }
        return ((DPFShown $h) -eq $want)
    }
    function DPFNear([double]$a, [double]$b, [double]$tol = 6) { return [Math]::Abs($a - $b) -le $tol }
    function DPFSame($a, $b) {
        return (DPFNear $a.L $b.L) -and (DPFNear $a.T $b.T) -and (DPFNear $a.W $b.W) -and (DPFNear $a.H $b.H)
    }
    function DPFOverlap($a, $b) {
        return ($a.L -lt $b.R) -and ($b.L -lt $a.R) -and ($a.T -lt $b.B) -and ($b.T -lt $a.B)
    }
    function DPFCentre($rect) {
        return [pscustomobject]@{ X = [int]($rect.L + $rect.W / 2); Y = [int]($rect.T + $rect.H / 2) }
    }
    function DPFIsFloat($entry) { return ($entry.floating -eq $true) }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H96 refuses an existing wiri instance'
        }
        $env:WIRI_PIPE = $dpfPipe
        $dpfConfig = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 0; center-focused-column "never"; }
window-rule { match title="^lane-h96-float$"; open-floating true; }
'@
        $dpfProcess = Start-WiriWithConfig $dpfConfig (Join-Path $dpfDir 'wiri.stderr.log') -NoDefaultBinds
        $null = $dpfProcess.Handle
        if ($dpfProcess.HasExited) { throw 'H96 daemon exited during startup' }
        $dpfDetails['daemon_pid'] = $dpfProcess.Id
        $dpfDetails['pipe'] = $dpfPipe
        $dpfF = DPFOpen 'lane-h96-float'
        $dpfT = DPFOpen 'lane-h96-tile'
        if (-not (DPFIsFloat (DPFEntry 'f-entry' $dpfF)) -or (DPFIsFloat (DPFEntry 't-entry' $dpfT))) {
            throw 'H96 fixture layers do not match the configuration'
        }
        DPFFocus $dpfT
        $dpfFloatHome = Rc $dpfF
        $dpfTileHome = Rc $dpfT
        $dpfDetails['float_geometry_before'] = $dpfFloatHome
        $dpfDetails['tile_geometry_before'] = $dpfTileHome

        # Group 1: a float over a tile is drawn in front of it.
        try {
            $dpfMid = DPFCentre $dpfFloatHome
            $dpfTop = [LT]::TopWindowAt($dpfMid.X, $dpfMid.Y)
            $dpfBaseEntry = DPFEntry 'g1-float' $dpfF
            DPFGate 'h96_float_over_tile_is_visible_in_front_of_the_tile' (
                (DPFIsFloat $dpfBaseEntry) -and (DPFShown $dpfF) -and (DPFOverlap $dpfFloatHome $dpfTileHome) -and
                $dpfTop -eq $dpfF) @{
                    entry = $dpfBaseEntry; float = $dpfFloatHome; tile = $dpfTileHome
                    top_at_float_centre = [int64]$dpfTop }
        } catch { $dpfDetails['group_baseline_error'] = ($_ | Out-String).Trim() }

        # Group 2: the focused tile goes fullscreen: the floating layer is not drawn, and the float stays managed.
        try {
            DPFFocus $dpfT
            DPFAction 'g2-fullscreen-on' @('fullscreen-window')
            $dpfTileEntry = DPFEntry 'g2-tile' $dpfT
            $dpfHidden = DPFWaitShown $dpfF $false 4000
            $dpfFloatEntry = DPFEntry 'g2-float' $dpfF
            DPFGate 'h96_fullscreen_tile_hides_the_floating_layer' (
                $dpfTileEntry.fullscreen -eq $true -and $dpfHidden -and (DPFIsFloat $dpfFloatEntry)) @{
                    tile = $dpfTileEntry; float = $dpfFloatEntry; float_hidden = $dpfHidden
                    visible = [LT]::IsWindowVisible($dpfF); cloaked = [LT]::Cloaked($dpfF) }
        } catch { $dpfDetails['group_hide_error'] = ($_ | Out-String).Trim() }

        # Group 3: the focus-floating action makes the floating layer the active one: the float is back at its
        # place with the foreground, while the tile is still fullscreen.
        try {
            DPFAction 'g3-focus-floating' @('focus-floating')
            $dpfShown = DPFWaitShown $dpfF $true 4000
            $dpfGotForeground = WaitForeground $dpfF 600 4000
            $dpfBackRect = Rc $dpfF
            $dpfTileStill = DPFEntry 'g3-tile' $dpfT
            DPFGate 'h96_focus_floating_shows_the_float_again_at_its_place' (
                $dpfShown -and $dpfGotForeground -and (DPFSame $dpfBackRect $dpfFloatHome) -and
                $dpfTileStill.fullscreen -eq $true) @{
                    shown = $dpfShown; foreground = $dpfGotForeground; before = $dpfFloatHome; after = $dpfBackRect
                    tile = $dpfTileStill }
        } catch { $dpfDetails['group_focus_floating_error'] = ($_ | Out-String).Trim() }

        # Group 4: focus returns to the fullscreen tile: the floating layer goes away again.
        try {
            DPFFocus $dpfT
            $dpfHiddenAgain = DPFWaitShown $dpfF $false 4000
            $dpfTileAgain = DPFEntry 'g4-tile' $dpfT
            DPFGate 'h96_focusing_the_fullscreen_tile_hides_the_float_again' (
                $dpfHiddenAgain -and $dpfTileAgain.fullscreen -eq $true) @{
                    float_hidden = $dpfHiddenAgain; tile = $dpfTileAgain }
        } catch { $dpfDetails['group_refocus_tile_error'] = ($_ | Out-String).Trim() }

        # Group 5: an explicit focus-window request for the hidden float reveals it (the request may need a
        # second try while the show is still being delivered to the form's thread, so no CLI failure is fatal).
        try {
            $dpfRevealed = $false
            foreach ($dpfAttempt in 1..3) {
                $dpfRaw = CtlRaw @('--json', 'focus-window', [string][int64]$dpfF)
                $dpfDetails["g5_attempt_$dpfAttempt"] = $dpfRaw
                if ((DPFWaitShown $dpfF $true 3000) -and (WaitForeground $dpfF 600 3000)) { $dpfRevealed = $true; break }
            }
            $dpfIdRect = Rc $dpfF
            $dpfTileById = DPFEntry 'g5-tile' $dpfT
            DPFGate 'h96_focus_window_by_id_reveals_the_hidden_float' (
                $dpfRevealed -and (DPFSame $dpfIdRect $dpfFloatHome) -and $dpfTileById.fullscreen -eq $true) @{
                    revealed = $dpfRevealed; before = $dpfFloatHome; after = $dpfIdRect; tile = $dpfTileById }
        } catch { $dpfDetails['group_focus_by_id_error'] = ($_ | Out-String).Trim() }

        # Group 6: leaving fullscreen brings the floating layer back at its place, in front of the tile.
        try {
            DPFFocus $dpfT
            $null = DPFWaitShown $dpfF $false 4000
            DPFAction 'g6-fullscreen-off' @('fullscreen-window')
            $dpfShownBack = DPFWaitShown $dpfF $true 4000
            $dpfTileOff = DPFEntry 'g6-tile' $dpfT
            $dpfOffRect = Rc $dpfF
            $dpfOffMid = DPFCentre $dpfOffRect
            $dpfOffTop = [IntPtr]::Zero
            $dpfSw = [Diagnostics.Stopwatch]::StartNew()
            while ($dpfSw.ElapsedMilliseconds -lt 4000) {
                $dpfOffTop = [LT]::TopWindowAt($dpfOffMid.X, $dpfOffMid.Y)
                if ($dpfOffTop -eq $dpfF) { break }
                Start-Sleep -Milliseconds 100
            }
            DPFGate 'h96_unfullscreen_shows_the_float_again_at_its_place' (
                $dpfTileOff.fullscreen -eq $false -and $dpfShownBack -and (DPFSame $dpfOffRect $dpfFloatHome) -and
                $dpfOffTop -eq $dpfF) @{
                    tile = $dpfTileOff; shown = $dpfShownBack; before = $dpfFloatHome; after = $dpfOffRect
                    top_at_float_centre = [int64]$dpfOffTop }
        } catch { $dpfDetails['group_unfullscreen_error'] = ($_ | Out-String).Trim() }
        $dpfDetails['stage'] = 'gates-complete'
    } catch {
        $dpfDetails['error'] = ($_ | Out-String).Trim()
    } finally {
        foreach ($dpfForm in $dpfForms) {
            try {
                if ([LT]::IsWindow($dpfForm) -and [LT]::Pid($dpfForm) -eq $PID) { [TestForms]::CloseForm($dpfForm) }
            } catch { $dpfDetails['form_cleanup_error'] = [string]$_ }
        }
        try {
            if ($null -ne $dpfProcess -and -not $dpfProcess.HasExited) {
                if ($env:WIRI_PIPE -eq $dpfPipe) { Stop-WiriGracefully $dpfProcess 'h96-owned' }
                elseif (-not $dpfProcess.HasExited) { $dpfProcess.Kill(); $null = $dpfProcess.WaitForExit(5000) }
            }
        } catch {
            $dpfDetails['daemon_cleanup_error'] = [string]$_
            if ($null -ne $dpfProcess -and -not $dpfProcess.HasExited) {
                try { $dpfProcess.Kill(); $null = $dpfProcess.WaitForExit(5000) } catch { $dpfDetails['daemon_kill_error'] = [string]$_ }
            }
        } finally {
            $env:WIRI_PIPE = $dpfSavedPipe
            if ($null -ne $dpfSavedConfig) { [IO.File]::WriteAllText($cfg, $dpfSavedConfig, (New-Object Text.UTF8Encoding($false))) }
            $dpfDetails | ConvertTo-Json -Depth 16 | Set-Content -Encoding UTF8 (Join-Path $dpfRoot 'evidence.json')
            $summary['lane_dpi_floats_details'] = $dpfDetails
        }
    }
}
