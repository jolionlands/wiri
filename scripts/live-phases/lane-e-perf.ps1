# Lane E / Phase 4 groundwork: information only. Dot-source after the existing summary
# has been assembled, before it is written. Does not alter any existing gate or $steps.
# All p_* values are numbers; -1 means unavailable, never a pass/fail or a zero sample.
function Invoke-LaneEPerf {
    $ErrorActionPreference = 'Stop'
    $metrics = [ordered]@{}
    foreach ($name in @(
        'exe_bytes', 'ctl_exe_bytes', 'cold_first_ipc_ms', 'cold_first_layout_ms', 'cold_samples',
        'cold_start_call_ms', 'cold_observer_start_max_ms', 'existing_sentinel_windows', 'managed_windows',
        'idle_seconds', 'idle_cpu_ms', 'idle_one_core_percent', 'idle_context_switches_per_s',
        'idle_input_changed', 'idle_new_threads', 'idle_exited_threads', 'idle_positioning_lines',
        'rest_threads_total', 'rest_threads_named', 'rest_threads_unnamed',
        'hotkey_samples', 'hotkey_timeouts', 'hotkey_invalid_samples', 'hotkey_move_median_ms',
        'hotkey_move_max_ms', 'hotkey_observer_gap_max_ms',
        'maximize_setwindowpos_lines_median', 'maximize_setwindowpos_lines_max',
        'maximize_positioning_lines_median', 'width_setwindowpos_lines_median',
        'fullscreen_setwindowpos_lines_median', 'move_setwindowpos_lines_median'
    )) { $metrics["p_$name"] = -1 }
    foreach ($when in @('startup', 'after')) {
        foreach ($name in @('private_bytes', 'working_set_bytes', 'threads', 'handles', 'gdi_objects', 'user_objects')) {
            $metrics["p_${when}_$name"] = -1
        }
    }
    $errors = New-Object 'System.Collections.Generic.List[string]'
    $actions = New-Object 'System.Collections.Generic.List[object]'
    $keys = New-Object 'System.Collections.Generic.List[object]'
    $owned = $null
    $sentinels = @()
    $perfDir = Join-Path $D 'perf'
    $log = Join-Path $perfDir 'local\wiri\wiri.log'
    $evidence = [ordered]@{
        schema = 1; kind = 'info only; no performance thresholds'; unavailable = -1
        machine = $env:COMPUTERNAME; os = @(); observer_reported_os = [Environment]::OSVersion.VersionString
        collector_bitness = [IntPtr]::Size * 8; logical_processors = [Environment]::ProcessorCount
        utc = [DateTime]::UtcNow.ToString('o'); source_commit = ''; binary_version = ''
        configuration = 'shipped default_config.kdl; -v --no-autostart; tray on, animations/status bar/focus-follows-mouse off'
        cold_definition = 'OS process creation to complete successful get_state reply / both pre-existing sentinels resized to tiled heights with disjoint rects and at least one on-screen (scrolled-out tiles may be parked below monitors)'
        switches_method = 'NtQuerySystemInformation(SystemProcessInformation) per-thread cumulative ContextSwitches delta / elapsed QPC seconds; process/TID creation identity matched. Endpoint-based lower bound: exited threads and threads born+exited between snapshots cannot be fully counted.'
        action_definition = 'log deltas from just before one action through 600 ms after it; all managed windows, not just the target'
        setwindowpos_definition = 'SetWindowPos [sync] SUCCESS log lines: successful positioning calls, NOT a complete native API trace (unlogged style/z-order/failures excluded)'
        latency_definition = 'Ctrl+Alt+Shift+F via one SendInput batch to first GetWindowRect change; 1 ms sleeps between observations; no DWM present/final-frame claim'
        errors = $errors; hotkeys = $keys; actions = $actions
    }
    function PValue([string]$name, $value) { $metrics["p_$name"] = $value }
    function PResources([string]$when) {
        $r = [LaneE.Native]::ReadResources($owned.Id)
        PValue "${when}_private_bytes" $r.PrivateBytes
        PValue "${when}_working_set_bytes" $r.WorkingSetBytes
        PValue "${when}_threads" $r.Threads
        PValue "${when}_handles" $r.Handles
        PValue "${when}_gdi_objects" $r.GdiObjects
        PValue "${when}_user_objects" $r.UserObjects
        return $r
    }
    function PRequest([string]$json) {
        # Never send Quit/actions to somebody else's pipe after our child has exited.
        if ($null -eq $owned -or $owned.HasExited) { throw 'Lane E child is not running' }
        $text = [LaneE.Native]::Request($json, 5000, $owned.Id)
        $reply = $text | ConvertFrom-Json
        if ($reply.success -ne $true) { throw "Lane E IPC rejected request: $text" }
        return $reply
    }
    function PLog {
        if (-not (Test-Path $log)) { throw 'Lane E child did not write its isolated debug log' }
        $text = [LaneE.Native]::ReadSharedText($log)
        return [pscustomobject]@{
            positioning = [regex]::Matches($text, 'Positioning window ').Count
            swp = [regex]::Matches($text, 'SetWindowPos(?: sync)? SUCCESS:').Count
            fallback = [regex]::Matches($text, 'MoveWindow SUCCESS:|All position methods FAILED:|SetWindowPos failed for').Count
        }
    }
    function PAction([string]$group, [string]$name, [string[]]$arguments) {
        $before = @(FormInfo @($target))[0].rect
        $beforeState = (PRequest '{"type":"get_state"}').result.windows | Where-Object { $_.hwnd -eq [int64]$target }
        $a = PLog
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $json = @{ type = 'run_action'; data = @{ name = $name; args = @($arguments) + @('--id', [string][int64]$target) } } | ConvertTo-Json -Compress -Depth 4
        $null = PRequest $json
        Start-Sleep -Milliseconds 600
        $b = PLog
        $after = @(FormInfo @($target))[0].rect
        $intervalMs = $clock.Elapsed.TotalMilliseconds
        # The viewport can follow a swap, leaving the target's rectangle unchanged. Confirm the
        # column mutation in IPC state outside the log interval rather than rejecting a real move.
        $afterState = (PRequest '{"type":"get_state"}').result.windows | Where-Object { $_.hwnd -eq [int64]$target }
        $confirmed = $before -ne $after
        if ($group -eq 'move') { $confirmed = $null -ne $beforeState -and $null -ne $afterState -and $beforeState.column -ne $afterState.column }
        if ($group -eq 'fullscreen') { $confirmed = $confirmed -and $beforeState.fullscreen -ne $afterState.fullscreen }
        $actions.Add([pscustomobject]@{ group = $group; action = $name; args = @($arguments)
            confirmed = $confirmed; before = $before; after = $after; interval_ms = $intervalMs
            column_before = $beforeState.column; column_after = $afterState.column
            fullscreen_before = $beforeState.fullscreen; fullscreen_after = $afterState.fullscreen
            setwindowpos_lines = $b.swp - $a.swp; positioning_lines = $b.positioning - $a.positioning
            fallback_lines = $b.fallback - $a.fallback })
    }
    try {
        New-Item -ItemType Directory -Force $perfDir | Out-Null
        PValue 'exe_bytes' (Get-Item $wiri).Length
        PValue 'ctl_exe_bytes' (Get-Item $ctl).Length
        $evidence.source_commit = (& git rev-parse HEAD | Out-String).Trim()
        $evidence.binary_version = (& $ctl --version | Out-String).Trim()
        $evidence.exe_sha256 = (Get-FileHash $wiri -Algorithm SHA256).Hash
        $evidence.ctl_sha256 = (Get-FileHash $ctl -Algorithm SHA256).Hash
        $evidence.cpu = @(Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors)
        $evidence.os = @(Get-CimInstance Win32_OperatingSystem | Select-Object Caption, Version, BuildNumber)
        # The harness has finished and quit all its children. Refuse rather than stop any unrelated process.
        if (@(Get-Process wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'A wiri process is still running; perf phase will not stop or measure it' }
        if (-not ('LaneE.Native' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot '..\perf\Native.cs') }
        # JIT and validate the native counter layout before measuring the child.
        $null = [LaneE.Native]::ReadThreads($PID)
        $null = [LaneE.Native]::ReadResources($PID)
        $null = [LaneE.Native]::Median([double[]]@(2, 1))
        $config = Join-Path $perfDir 'config.kdl'
        Copy-Item (Join-Path $PSScriptRoot '..\..\resources\default_config.kdl') $config
        foreach ($name in @('local', 'roaming')) { New-Item -ItemType Directory -Force (Join-Path $perfDir $name) | Out-Null }
        # Existing, ordinary windows, not a stream of new windows or a synthetic load test.
        $sentinels += [TestForms]::Open('lane-e-perf-1', 40, 110, 160)
        $sentinels += [TestForms]::Open('lane-e-perf-2', 40, 150, 160)
        Start-Sleep -Milliseconds 600
        if ($sentinels -contains [IntPtr]::Zero) { throw 'Could not create the two sentinel windows' }
        $evidence.sentinel_hwnds = @($sentinels | ForEach-Object { [int64]$_ })
        $evidence.before_start = @(FormInfo $sentinels)
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = [IO.Path]::GetFullPath($wiri)
        $info.Arguments = '-v --no-autostart --config "' + $config + '"'
        $info.UseShellExecute = $false; $info.CreateNoWindow = $true
        $info.RedirectStandardError = $true; $info.RedirectStandardOutput = $true
        $info.WorkingDirectory = $perfDir
        # Only the child's environment changes; all new persistence/log/config files stay in the worktree.
        $info.EnvironmentVariables['LOCALAPPDATA'] = Join-Path $perfDir 'local'
        $info.EnvironmentVariables['APPDATA'] = Join-Path $perfDir 'roaming'
        $info.EnvironmentVariables['WIRI_CONFIG'] = $config
        # The collector and child must share an explicit endpoint, not the old global prefix.
        [LaneE.Native]::PipeName = 'wiri_perf_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $info.EnvironmentVariables['WIRI_PIPE'] = '\\.\pipe\' + [LaneE.Native]::PipeName
        $evidence.pipe = $info.EnvironmentVariables['WIRI_PIPE']
        $height = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea.Height - 64
        $cold = [LaneE.Native]::Start($info, [IntPtr[]]$sentinels, $height, 15000)
        $owned = $cold.Process
        $evidence.pid = $owned.Id; $evidence.started_utc = $cold.StartedUtc.ToString('o')
        $evidence.cold_ipc_error = $cold.IpcError; $evidence.cold_layout_error = $cold.LayoutError
        [IO.File]::WriteAllText((Join-Path $perfDir 'first-ipc.json'), $cold.IpcReply)
        PValue 'cold_first_ipc_ms' $cold.FirstIpcMs
        PValue 'cold_first_layout_ms' $cold.FirstLayoutMs
        PValue 'cold_start_call_ms' $cold.StartCallMs
        PValue 'cold_observer_start_max_ms' ([Math]::Max($cold.IpcObserverMs, $cold.LayoutObserverMs))
        $evidence.cold_ipc_observer_ms = $cold.IpcObserverMs; $evidence.cold_layout_observer_ms = $cold.LayoutObserverMs
        $evidence.cold_layout_rects = $cold.LayoutRects; $evidence.cold_layout_on_screen_windows = $cold.LayoutOnScreenWindows
        PValue 'cold_samples' 1
        PValue 'existing_sentinel_windows' $sentinels.Count
        if ($cold.IpcError) { $errors.Add($cold.IpcError) }
        if ($cold.LayoutError) { $errors.Add($cold.LayoutError) }
        Start-Sleep -Seconds 5 # allow startup indicator / initial location events to settle
        $evidence.startup_resources = PResources 'startup'
        # Information only: which threads the daemon owns once startup has settled, grouped by
        # the description SetThreadDescription gives them. One numeric key per description plus
        # the totals; the rows themselves stay text in measurement.json.
        $restGroups = @([LaneE.Native]::ThreadDescriptions($owned.Id))
        $evidence.rest_threads = @($restGroups | ForEach-Object {
            [pscustomobject]@{ description = $_.Name; count = $_.Count; tids = @($_.Ids) }
        })
        PValue 'rest_threads_total' ([int](($restGroups | Measure-Object Count -Sum).Sum))
        PValue 'rest_threads_named' ([int](($restGroups | Where-Object { $_.Name -ne '' } | Measure-Object Count -Sum).Sum))
        PValue 'rest_threads_unnamed' ([int](($restGroups | Where-Object { $_.Name -eq '' } | Measure-Object Count -Sum).Sum))
        foreach ($group in $restGroups) {
            $key = 'rest_threads_by_' + (($group.Name.ToLowerInvariant() -replace '[^a-z0-9]+', '_').Trim('_'))
            if ($key -eq 'rest_threads_by_') { $key = 'rest_threads_by_unnamed' }
            if ($key.Length -gt 60) { $key = $key.Substring(0, 60) }
            PValue $key ([int]$group.Count)
        }
        $state = PRequest '{"type":"get_state"}'
        $state | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $perfDir 'startup-state.json')
        $windowReply = PRequest '{"type":"window_list"}'
        $evidence.managed_windows = @($windowReply.result).Count
        PValue 'managed_windows' $evidence.managed_windows
        $evidence.startup_rects = @(FormInfo $sentinels)
        $evidence.window_threads = @([LT]::TopLevel() | Where-Object { [LT]::Pid($_) -eq $owned.Id } | ForEach-Object {
            [uint32]$windowPid = 0
            $threadId = [LT]::GetWindowThreadProcessId($_, [ref]$windowPid)
            [pscustomobject]@{ hwnd = [int64]$_; tid = $threadId; class = [LT]::Class($_); visible = [LT]::IsWindowVisible($_) }
        })
        Shot 'p-startup'
        # Exactly one 30 s no-input observation, no event subscribers and no harness IPC during it.
        $a = PLog
        try {
            $idle = [LaneE.Native]::Idle($owned.Id, 30000)
            $idle | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $perfDir 'idle.json')
            PValue 'idle_seconds' $idle.Seconds
            PValue 'idle_cpu_ms' $idle.CpuMs
            PValue 'idle_one_core_percent' $idle.OneCorePercent
            PValue 'idle_context_switches_per_s' $idle.SwitchesPerSecond
            PValue 'idle_input_changed' ([int]($idle.InputTickBefore -ne $idle.InputTickAfter))
            PValue 'idle_new_threads' $idle.NewThreads
            PValue 'idle_exited_threads' $idle.ExitedThreads
        } catch { $errors.Add("Idle counter unavailable: $_") }
        $b = PLog
        PValue 'idle_positioning_lines' ($b.positioning - $a.positioning)

        $target = $sentinels[0]
        $null = PRequest ('{"type":"focus_window","data":{"window_hwnd":' + [int64]$target + '}}')
        Start-Sleep -Milliseconds 1500
        $invalid = 0; $timeouts = 0
        for ($i = 0; $i -lt 10; $i++) {
            # Focus is set once, outside the timed region; refuse misattribution if another app steals it.
            $a = PLog
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $key = [LaneE.Native]::MaximizeAndObserve($target, 1500)
            if ($key.InputsSent -ne 8 -or $key.ForegroundBefore -ne [int64]$target -or $key.ForegroundAfter -ne [int64]$target) { $invalid++ }
            if ($key.InputsSent -eq 8 -and $key.MoveMs -lt 0) { $timeouts++ }
            Start-Sleep -Milliseconds 600 # at least 600 ms between injections, even for a fast action
            $b = PLog
            $finalRect = @(FormInfo @($target))[0].rect
            $oldCoords = $key.Before -split ','; $newCoords = $finalRect -split ','
            $widthDelta = ([int]$newCoords[2] - [int]$newCoords[0]) - ([int]$oldCoords[2] - [int]$oldCoords[0])
            $confirmed = $key.InputsSent -eq 8 -and $key.MoveMs -ge 0 -and $key.ForegroundBefore -eq [int64]$target -and
                $key.ForegroundAfter -eq [int64]$target -and $(if ($i % 2 -eq 0) { $widthDelta -gt 10 } else { $widthDelta -lt -10 })
            $keys.Add([pscustomobject]@{ sample = $i + 1; confirmed = $confirmed; observed = $key; final_rect = $finalRect })
            $actions.Add([pscustomobject]@{ group = 'maximize'; action = 'maximize-column'; sample = $i + 1
                confirmed = $confirmed; interval_ms = $clock.Elapsed.TotalMilliseconds
                setwindowpos_lines = $b.swp - $a.swp; positioning_lines = $b.positioning - $a.positioning
                fallback_lines = $b.fallback - $a.fallback })
            if (-not $confirmed) { $errors.Add('Stopped hotkey observation after an unconfirmed transition (no further input injected)'); break }
        }
        $valid = @($keys | Where-Object { $_.confirmed } | ForEach-Object { $_.observed })
        PValue 'hotkey_samples' $valid.Count
        PValue 'hotkey_timeouts' $timeouts
        PValue 'hotkey_invalid_samples' $invalid
        PValue 'hotkey_move_median_ms' ([LaneE.Native]::Median([double[]]@($valid | ForEach-Object { $_.MoveMs })))
        if ($valid.Count -gt 0) { PValue 'hotkey_move_max_ms' (($valid | Measure-Object MoveMs -Maximum).Maximum) }
        PValue 'hotkey_observer_gap_max_ms' (($keys | ForEach-Object { $_.observed } | Measure-Object MaxObserverGapMs -Maximum).Maximum)
        PAction 'width' 'set-column-width' @('600')
        PAction 'width' 'set-column-width' @('400')
        PAction 'fullscreen' 'fullscreen-window' @()
        PAction 'fullscreen' 'fullscreen-window' @()
        # Establish a non-boundary move precondition outside all capture intervals.
        $null = PRequest ('{"type":"run_action","data":{"name":"move-column-to-last","args":["--id","' + [int64]$target + '"]}}')
        Start-Sleep -Milliseconds 600
        PAction 'move' 'move-column-left' @()
        PAction 'move' 'move-column-right' @()
        foreach ($group in @('maximize', 'width', 'fullscreen', 'move')) {
            $rows = @($actions | Where-Object { $_.group -eq $group -and $_.confirmed })
            PValue "${group}_action_samples" $rows.Count
            PValue "${group}_setwindowpos_lines_median" ([LaneE.Native]::Median([double[]]@($rows | ForEach-Object { $_.setwindowpos_lines })))
        }
        $rows = @($actions | Where-Object { $_.group -eq 'maximize' -and $_.confirmed })
        if ($rows.Count -gt 0) { PValue 'maximize_setwindowpos_lines_max' (($rows | Measure-Object setwindowpos_lines -Maximum).Maximum) }
        PValue 'maximize_positioning_lines_median' ([LaneE.Native]::Median([double[]]@($rows | ForEach-Object { $_.positioning_lines })))
        Start-Sleep -Seconds 5
        $evidence.after_resources = PResources 'after'
        $evidence.after_rects = @(FormInfo $sentinels)
        Shot 'p-after'
    } catch {
        $errors.Add($_.ToString())
        Write-Warning "Lane E performance information incomplete: $_"
    } finally {
        try {
            if ($null -ne $owned -and -not $owned.HasExited) {
                try { $null = PRequest '{"type":"quit"}' } catch { $errors.Add("Perf quit: $_") }
                if (-not $owned.WaitForExit(10000)) { $owned.Kill(); $null = $owned.WaitForExit(5000); $errors.Add('Own perf child needed forced shutdown') }
            }
        } catch { $errors.Add("Perf child cleanup: $_") }
        if ($null -ne $owned) { try { $owned.Dispose() } catch { $errors.Add("Perf process handle cleanup: $_") } }
        foreach ($h in $sentinels) {
            try { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } } catch { $errors.Add("Perf form cleanup: $_") }
        }
        PValue 'collector_errors' $errors.Count
        $evidence.metrics = $metrics
        try { $evidence | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $perfDir 'measurement.json') }
        catch { PValue 'collector_errors' ($errors.Count + 1); Write-Warning "Could not write Lane E detail: $_" }
    }
    return $metrics
}
# No collector/evidence failure may prevent the existing harness summary from being written.
try {
    $laneEPerf = Invoke-LaneEPerf
    foreach ($key in $laneEPerf.Keys) { $summary[$key] = $laneEPerf[$key] }
} catch { $summary['p_collector_errors'] = -1; Write-Warning "Lane E information unavailable: $_" }
