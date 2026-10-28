# H81: actual horizontal camera movement, with owned application HWNDs.
# This measures geometry, not frame pacing or the numerical curve shape. The
# bounded solver's numerical contract is tested separately in Rust.
# The parent owns the desktop mutex; this phase owns only its forms and daemon.
& {
    $h81Dir = Join-Path $D 'h81-camera'
    $null = New-Item -ItemType Directory -Force $h81Dir
    $h81Evidence = [ordered]@{ cases = [ordered]@{} }
    foreach ($name in @('linear', 'bezier')) {
        foreach ($suffix in @('common_intermediate_motion', 'target_visible', 'settled')) {
            $summary["h81_${name}_$suffix"] = $false
        }
    }
    $summary.h81_off_has_no_intermediate_motion = $false
    $summary.h81_off_target_visible = $false
    $summary.h81_cleanup = $false
    $h81Process = $null
    $h81Forms = New-Object System.Collections.ArrayList
    function H81Rects {
        return @($h81Forms | ForEach-Object { Rc $_ })
    }
    function H81Same($a, $b, [int]$tolerance = 3) {
        if ($a.Count -ne $b.Count) { return $false }
        for ($i = 0; $i -lt $a.Count; $i++) {
            foreach ($key in @('L', 'T', 'W', 'H')) {
                if ([Math]::Abs($a[$i].$key - $b[$i].$key) -gt $tolerance) { return $false }
            }
        }
        return $true
    }
    function H81Owned([IntPtr]$h) {
        return ($h81Forms.Contains($h) -and [LT]::IsWindow($h) -and [LT]::Pid($h) -eq $PID)
    }
    function H81View($rects) {
        $valid = ($rects.Count -eq 3)
        $windows = New-Object System.Collections.ArrayList
        foreach ($r in $rects) {
            $overlap = [Math]::Min($r.R, $swa.Right) - [Math]::Max($r.L, $swa.Left)
            $inView = ($r.W -gt 0) -and ($r.H -gt 0) -and ($overlap -ge 20) -and
                ($r.T -ge ($swa.Top - 3)) -and ($r.B -le ($swa.Bottom + 3))
            if (-not $inView) { $valid = $false }
            [void]$windows.Add([pscustomobject]@{ overlap_x_px = $overlap; in_view = $inView })
        }
        return [pscustomobject]@{ valid = $valid; windows = @($windows) }
    }
    function H81Close {
        foreach ($h in $h81Forms) {
            if (H81Owned $h) { [TestForms]::CloseForm($h) }
        }
        if ($null -ne $h81Process) { Stop-WiriGracefully $h81Process 'h81' }
        Start-Sleep -Milliseconds 300
        if (@($h81Forms | Where-Object { [LT]::IsWindow($_) }).Count -ne 0) {
            throw 'camera fixture did not close'
        }
    }
    try {
        foreach ($case in @('linear', 'bezier', 'off')) {
            $h81Forms.Clear()
            # Three 40%-width columns overflow enough to scroll, while every
            # column still intersects the view at both camera endpoints. At
            # 60%, the first/last HWND is parked below the display by offscreen
            # virtualization, so its native rectangle cannot measure camera delta.
            $width = [Math]::Ceiling($swa.Width * 0.40)
            $motion = switch ($case) {
                'linear' { 'duration-ms 1600; curve "linear";' }
                'bezier' { 'duration-ms 1600; curve "cubic-bezier" 0 0 0 1;' }
                'off' { 'off;' }
            }
            $config = @"
input { mod-key "ctrl-alt"; }
animations {
    horizontal-view-movement { $motion }
    window-movement { off; }
    window-resize { off; }
    workspace-switch { off; }
}
layout { gaps 10; center-focused-column "never"; focus-ring { off; }; border { off; }; }
window-rule { match title="^h81-"; default-column-width { fixed $width; }; }
"@
            $h81Process = Start-WiriWithConfig $config (Join-Path $h81Dir "$case.stderr.log")
            foreach ($i in 1..3) {
                $h = [TestForms]::Open("h81-$case-$i", 90, (100 + $i * 25), 180)
                $h81Forms.Add($h) | Out-Null
                Start-Sleep -Milliseconds 1300
            }
            $first = $h81Forms[0]; $target = $h81Forms[2]
            if (-not (H81Owned $first) -or -not (H81Owned $target)) { throw 'camera fixture ownership lost' }
            FocusForm $first
            Start-Sleep -Milliseconds 1800
            if ([LT]::GetForegroundWindow() -ne $first) { throw 'camera start fixture did not keep focus' }
            $before = @(H81Rects)
            $beforeView = H81View $before
            $h81Evidence.cases[$case] = [ordered]@{
                config = $config; requested_column_width = $width
                work_area = @{ L = $swa.Left; T = $swa.Top; R = $swa.Right; B = $swa.Bottom }
                before = $before; fixture_before = $beforeView
            }
            foreach ($h in $h81Forms) {
                if (-not (H81Owned $h)) { throw 'camera fixture ownership lost before sampling' }
            }
            if (-not $beforeView.valid) { throw 'camera fixture is parked or does not intersect the work area before sampling' }
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $raw = Ctl "h81-$case-focus" @('focus-window', [string][int64]$target)
            if ($steps[-1].rc -ne 0) { throw "camera focus request failed: $raw" }
            $replyMs = $clock.ElapsedMilliseconds
            $samples = New-Object System.Collections.ArrayList
            do {
                $samples.Add([pscustomobject]@{ ms = $clock.ElapsedMilliseconds; rects = @(H81Rects) }) | Out-Null
                Start-Sleep -Milliseconds 15
            } while ($clock.ElapsedMilliseconds -lt 2400)
            $after = @(H81Rects)
            $rest = [Diagnostics.Stopwatch]::StartNew()
            $restSamples = New-Object System.Collections.ArrayList
            do {
                $restSamples.Add([pscustomobject]@{ ms = $rest.ElapsedMilliseconds; rects = @(H81Rects) }) | Out-Null
                Start-Sleep -Milliseconds 20
            } while ($rest.ElapsedMilliseconds -lt 450)
            $still = @(H81Rects)
            $settled = H81Same $after $still
            foreach ($sample in $restSamples) {
                if (-not (H81Same $sample.rects $after)) { $settled = $false }
            }
            $total = $after[0].L - $before[0].L
            $intermediate = New-Object System.Collections.ArrayList
            foreach ($sample in $samples) {
                if ([Math]::Abs($total) -le 100) { continue }
                $delta = $sample.rects[0].L - $before[0].L
                if ([Math]::Abs($delta) -le 6 -or [Math]::Abs($delta - $total) -le 6) { continue }
                $common = $true
                for ($i = 0; $i -lt 3; $i++) {
                    $r = $sample.rects[$i]; $b = $before[$i]
                    if ([Math]::Abs(($r.L - $b.L) - $delta) -gt 3 -or
                        [Math]::Abs($r.T - $b.T) -gt 3 -or [Math]::Abs($r.W - $b.W) -gt 3 -or
                        [Math]::Abs($r.H - $b.H) -gt 3) { $common = $false }
                }
                if ($common -and ($delta / $total) -gt 0 -and ($delta / $total) -lt 1) {
                    $intermediate.Add($delta) | Out-Null
                }
            }
            $distinct = @($intermediate | Select-Object -Unique).Count
            $afterView = H81View $after
            $allSamplesInView = $true
            foreach ($sample in @($samples) + @($restSamples)) {
                if (-not (H81View $sample.rects).valid) { $allSamplesInView = $false }
            }
            $stillView = H81View $still
            $visible = ([LT]::GetForegroundWindow() -eq $target) -and
                ($after[2].L -ge ($swa.Left - 3)) -and ($after[2].R -le ($swa.Right + 3)) -and
                ($after[2].T -ge ($swa.Top - 3)) -and ($after[2].B -le ($swa.Bottom + 3))
            $h81Evidence.cases[$case] = [ordered]@{
                requested_column_width = $width
                work_area = @{ L = $swa.Left; T = $swa.Top; R = $swa.Right; B = $swa.Bottom }
                config = $config; before = $before; after = $after; still = $still
                fixture_before = $beforeView; fixture_after = $afterView; fixture_still = $stillView
                all_samples_in_view = $allSamplesInView
                reply_ms = $replyMs; total_delta = $total; intermediate_positions = $distinct
                target_visible = $visible; samples = @($samples); rest_samples = @($restSamples)
            }
            foreach ($h in $h81Forms) {
                if (-not (H81Owned $h)) { throw 'camera fixture ownership lost after sampling' }
            }
            if (-not $afterView.valid -or -not $stillView.valid -or -not $allSamplesInView) {
                throw 'camera fixture became parked or stopped intersecting the work area during sampling'
            }
            if ($case -eq 'off') {
                $allInstant = $true
                foreach ($sample in $samples) {
                    if (-not (H81Same $sample.rects $after)) { $allInstant = $false }
                }
                $summary.h81_off_has_no_intermediate_motion = ([Math]::Abs($total) -gt 100) -and
                    ($replyMs -lt 700) -and $allInstant -and $settled
                $summary.h81_off_target_visible = $visible
            } else {
                $summary["h81_${case}_common_intermediate_motion"] = ([Math]::Abs($total) -gt 100) -and ($distinct -ge 3)
                $summary["h81_${case}_target_visible"] = $visible
                $summary["h81_${case}_settled"] = $settled
            }
            Shot "h81-$case-settled"
            H81Close
            $h81Process = $null
        }
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h81Dir 'error.txt')
    } finally {
        H81Close
        $summary.h81_cleanup = (@($h81Forms | Where-Object { [LT]::IsWindow($_) }).Count -eq 0) -and
            ($null -eq $h81Process -or $h81Process.HasExited)
        $h81Evidence | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $h81Dir 'evidence.json')
    }
}
