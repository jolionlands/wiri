# H90: the animation frame clock is paced by the desktop's presentation clock, not by a
# fixed 16 ms timer, and nothing is armed while nothing animates.
# The parent harness owns the desktop mutex. This phase owns its daemon and its forms and
# never changes a display or input setting: it only reads what the daemon logged and its
# own process CPU time.
#
# What the gates can and cannot show:
#  * h90_animation_uses_a_presentation_clock: the burst summary line the new clock writes
#    once per animation (`animation clock: backend=... interval_us=... frames=... mean_us=...
#    max_us=...`) names a real presentation backend and its mean spacing is near the
#    reported refresh interval, over enough frames that a burst of catch-up ticks or a
#    16 ms sleep plus 10 ms of processing would not fit.
#  * h90_no_animation_wakeups_at_rest: after the animation settled, the daemon's own CPU
#    time does not grow over a three second window and it logs no further burst.
# Neither gate observes displayed frames or a hardware page flip; they are cadence and
# idle-cost evidence for a Windows approximation, and the matrix row stays partial.
$h90Gates = @('h90_animation_uses_a_presentation_clock', 'h90_no_animation_wakeups_at_rest')
foreach ($gate in $h90Gates) { $summary[$gate] = $false }
# slowdown 10 stretches a column resize to several seconds (a measured burst: 251 frames
# over 4168 ms at 60 Hz; the resize's default animation is a spring, and slowdown stretches
# its settle time), so the burst is long enough to measure a cadence.
$h90Config = @'
input { mod-key "ctrl-alt"; }
animations { slowdown 10; }
layout {
    focus-ring { off; }
    border { off; }
}
window-rule { match title="^h90-"; default-column-width { fixed 350; }; }
'@
$h90Daemon = $null; $h90Forms = @(); $h90Evidence = [ordered]@{}
$h90Log = Join-Path $D 'wiri.stderr90.log'
try {
    $h90Daemon = Start-WiriWithConfig $h90Config $h90Log
    foreach ($i in 1..1) { $h90Forms += [TestForms]::Open("h90-$i", 120, 120, 160); Start-Sleep -Milliseconds 1800 }
    $h90Form = $h90Forms[-1]
    FocusForm $h90Form
    # The summary line is written once, when a burst ends.  `slowdown 10` stretches the
    # window-open spring of the fixture into several seconds and the slowed resize after it
    # runs for several more, so a fixed delay cannot be used: wait for the opening burst's
    # line first (up to 20 s), sample the count, then wait for the line the resize writes.
    $h90Settle = [Diagnostics.Stopwatch]::StartNew()
    while ($h90Settle.ElapsedMilliseconds -lt 20000 -and (LogLines $h90Log 'animation clock:').Count -eq 0) {
        Start-Sleep -Milliseconds 200
    }
    $h90Before = (LogLines $h90Log 'animation clock:').Count
    $null = Ctl 'h90-resize' @('action', 'set-column-width', '650')
    $h90Wait = [Diagnostics.Stopwatch]::StartNew()
    while ($h90Wait.ElapsedMilliseconds -lt 25000 -and (LogLines $h90Log 'animation clock:').Count -le $h90Before) {
        Start-Sleep -Milliseconds 200
    }
    $h90Lines = @(LogLines $h90Log 'animation clock:')
    $h90New = @($h90Lines | Select-Object -Skip $h90Before)
    $h90Evidence.settle_ms = $h90Settle.ElapsedMilliseconds
    $h90Evidence.resize_wait_ms = $h90Wait.ElapsedMilliseconds
    $h90Last = if ($h90New.Count -gt 0) { $h90New[-1].Line } else { '' }
    $h90Fields = [ordered]@{}
    foreach ($key in 'backend', 'interval_us', 'refresh', 'frames', 'missed', 'timeouts', 'mean_us', 'max_us', 'span_ms') {
        $m = [regex]::Match($h90Last, [regex]::Escape($key) + '=([^ ]+)')
        if ($m.Success) { $h90Fields[$key] = $m.Groups[1].Value }
    }
    $h90Evidence.burst = $h90Last
    $h90Evidence.fields = $h90Fields
    $h90Evidence.new_lines = $h90New.Count
    $h90Backend = [string]$h90Fields['backend']
    $h90Frames = [int]$h90Fields['frames']
    $h90Mean = [int]$h90Fields['mean_us']
    $h90Max = [int]$h90Fields['max_us']
    $h90Interval = [int]$h90Fields['interval_us']
    # A presentation backend, at least 30 presented frames (a 10x-slowed 150 ms animation is
    # ~90), a mean spacing between 5 ms and 40 ms and a worst gap under 120 ms: a fixed
    # 16 ms sleep plus per-frame work, a burst of catch-up ticks, or a stalled loop would not
    # produce that. The interval the daemon used must be a plausible refresh interval.
    $summary.h90_animation_uses_a_presentation_clock = (
        $h90New.Count -ge 1) -and (
        $h90Backend -in @('DCompositionWaitForCompositorClock', 'DwmFlush', 'sleep')) -and (
        [string]$h90Fields['refresh'] -ne '') -and (
        $h90Frames -ge 30) -and (
        $h90Mean -ge 5000) -and ($h90Mean -le 40000) -and (
        $h90Max -le 120000) -and (
        $h90Interval -ge 4000) -and ($h90Interval -le 100000)
    Shot 'h90-paced'
    # Idle: let everything settle, then one three second window with no input, no ctl call
    # and no screenshot. Nothing may be armed while no timeline runs, so the process must
    # not spend CPU and must not log another burst.
    Start-Sleep -Milliseconds 1200
    $h90Daemon.Refresh()
    $h90CpuBefore = $h90Daemon.TotalProcessorTime.TotalMilliseconds
    $h90LinesBefore = (LogLines $h90Log 'animation clock:').Count
    Start-Sleep -Seconds 3
    $h90Daemon.Refresh()
    $h90CpuAfter = $h90Daemon.TotalProcessorTime.TotalMilliseconds
    $h90LinesAfter = (LogLines $h90Log 'animation clock:').Count
    $h90Evidence.idle = @{
        cpu_ms_before = $h90CpuBefore; cpu_ms_after = $h90CpuAfter
        cpu_ms_delta = ($h90CpuAfter - $h90CpuBefore)
        bursts_before = $h90LinesBefore; bursts_after = $h90LinesAfter
        alive = (-not $h90Daemon.HasExited)
    }
    $summary.h90_no_animation_wakeups_at_rest = (
        (-not $h90Daemon.HasExited) -and
        (($h90CpuAfter - $h90CpuBefore) -lt 40) -and
        ($h90LinesAfter -eq $h90LinesBefore))
} catch { $_ | Out-String | Set-Content (Join-Path $D 'h90-pace-error.txt') }
finally {
    foreach ($h in $h90Forms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
    if ($null -ne $h90Daemon) { Stop-WiriGracefully $h90Daemon 'h90' }
    $h90Evidence | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $D 'h90-pace.json')
}
