# Lane F / H30-H34. The caller already holds the desktop lock.
# Two isolated test workspaces, two columns and four owned HWNDs. The first
# workspace is only a parking place for the pre-existing desktop, never a target
# of a window/column mutation. Unsupported actions fail gates; nothing is skipped.
# No display mode, scale, position, rotation or primary-output changes.
& {
    $fDir = Join-Path $Out 'lane-f'
    $null = New-Item -ItemType Directory -Force -Path $fDir
    $fOwned = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $fClients = New-Object 'System.Collections.Generic.List[System.Diagnostics.Process]'
    $fHandles = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $fChecks = New-Object 'System.Collections.Generic.List[object]'
    $fNotes = [ordered]@{}
    # Functions mutate this object rather than assigning caller-scoped scalars.
    $fRuntime = @{ serial=0; current=$null; subscriber=$null; events=$null }
    $fSavedEnvironment = @{}
    foreach ($fKey in @('APPDATA','LOCALAPPDATA','WIRI_PIPE','WIRI_SNAPSHOT_DIR')) {
        $fSavedEnvironment[$fKey] = [Environment]::GetEnvironmentVariable($fKey, 'Process')
    }
    # Local scope shadows these harness paths; the caller's config is never written.
    $testRoaming = Join-Path $fDir 'roaming'
    $testLocal = Join-Path $fDir 'local'
    $cfg = Join-Path $testRoaming 'wiri\config.kdl'
    $fSnapshots = Join-Path $fDir 'snapshots'
    foreach ($fPath in @((Split-Path $cfg),$testLocal,$fSnapshots)) {
        $null = New-Item -ItemType Directory -Force -Path $fPath
    }
    $fSource = 'lane-f-source'
    $fTarget = 'lane-f-target'
    $fGateNames = @(
        'h30_two_columns_two_test_workspaces',
        'h30_indexed_columns_clamp_and_remember_tiles',
        'h30_indexed_windows_clamp',
        'h30_column_wraps', 'h30_window_edges_and_wraps',
        'h30_composites_use_tiles_before_fallback', 'h30_composites_fallback_at_edges',
        'h30_horizontal_focus_remembers_each_column',
        'h30_floating_directional_focus_stays_in_workspace',
        'h30_floating_tile_indices_are_noops',
        'h30_window_or_workspace_focus', 'h30_cli_and_typed_run_action',
        'h30_kdl_binds_execute_layout_actions',
        'h31_short_focus_is_excluded_at_750ms',
        'h31_previous_toggles_exact_endpoints', 'h31_closed_windows_are_excluded',
        'h31_history_crosses_workspaces', 'h31_cli_and_typed_run_action',
        'h32_window_transfer_names_indices_default_focus',
        'h32_window_transfer_focus_false_preserves_source',
        'h32_column_transfer_default_focus', 'h32_column_transfer_focus_false',
        'h32_relative_window_transfers', 'h32_relative_column_transfers',
        'h32_transfers_preserve_membership_and_placement',
        'h32_cli_and_typed_run_action',
        'h33_named_startup_order', 'h33_set_unset_workspace_names',
        'h33_optional_workspace_reference', 'h33_workspace_reordering',
        'h33_exactly_one_trailing_empty_workspace',
        'h33_floating_workspace_hide_show', 'h33_cli_and_typed_run_action',
        'h34_focus_monitor_targets', 'h34_window_monitor_targets',
        'h34_column_monitor_targets', 'h34_workspace_monitor_targets',
        'h34_composite_monitor_fallbacks', 'h34_cli_and_typed_run_action',
        'h34_display_topology_unchanged', 'h34_single_display_actions_are_harmless'
    )
    foreach ($fGate in $fGateNames) { $summary[$fGate] = $false }
    $summary['h30_f_owned_process_cleanup'] = $false

    function FCheck([string]$gate, [bool]$pass, $observed) {
        if ($fGateNames -notcontains $gate) { throw "Unknown Lane F gate: $gate" }
        [void]$fChecks.Add([pscustomobject]@{ gate=$gate; pass=$pass; observed=$observed })
    }
    function FFinish([string]$prefix, [bool]$complete) {
        foreach ($gate in @($fGateNames | Where-Object { $_.StartsWith($prefix) })) {
            $checks = @($fChecks.ToArray() | Where-Object { $_.gate -eq $gate })
            $summary[$gate] = ($complete -and $checks.Count -gt 0 -and @($checks | Where-Object { -not $_.pass }).Count -eq 0)
        }
    }
    function FCheckedCtl([string]$tag, [string[]]$arguments) {
        $mark = $steps.Count
        $text = Ctl $tag $arguments
        if ($steps.Count -ne $mark + 1 -or $steps[$mark].step -ne $tag -or [string]$steps[$mark].rc -ne '0') {
            throw "Lane F IPC failed: $tag (see the step rc and its stdout/stderr)"
        }
        return $text
    }
    function FJson([string]$tag, [string]$command) {
        $reply = FCheckedCtl $tag @('--json',$command) | ConvertFrom-Json
        if (-not $reply.success) { throw "Lane F query failed: $tag" }
        return $reply.result
    }
    function FToken($value) {
        if ($value -is [bool]) { return ([string]$value).ToLowerInvariant() }
        $text = [string]$value
        if ($text -match '[\s"]' -or $text.Length -eq 0) { return (Q $text) }
        return $text
    }
    function FCommand([string]$tag, [string]$transport, [string]$name, [object[]]$arguments = @(), [bool[]]$quoted = @()) {
        if ($quoted.Count -eq 0) { $quoted = @($arguments | ForEach-Object { $false }) }
        if ($quoted.Count -ne $arguments.Count) { throw 'Each action argument needs a quoted bit' }
        if ($transport -eq 'cli') {
            $words = @('action',$name)
            for ($i = 0; $i -lt $arguments.Count; $i++) {
                $words += $(if ($quoted[$i]) { Q ([string]$arguments[$i]) } else { FToken $arguments[$i] })
            }
            $null = FCheckedCtl $tag $words
        } elseif ($transport -eq 'raw') {
            $request = [ordered]@{ type='run_action'; data=[ordered]@{ name=$name; args=@($arguments); quoted=@($quoted) } }
            $json = $request | ConvertTo-Json -Depth 8 -Compress
            $json | Set-Content (Join-Path $fDir "$tag.request.json")
            $clock = [Diagnostics.Stopwatch]::StartNew()
            $replyText = [LaneFNative]::RequestOwned($fPipeName, $fRuntime.current, $json)
            $replyText | Set-Content (Join-Path $fDir "$tag.reply.json")
            $reply = $replyText | ConvertFrom-Json
            $rc = if ($reply.success) { 0 } else { 1 }
            [void]$steps.Add([pscustomobject]@{ step=$tag; args="raw run_action $name"; rc=$rc; ms=$clock.ElapsedMilliseconds })
            if ($rc -ne 0) { throw "Typed run_action failed: $tag" }
        } else { throw "Unknown transport: $transport" }
    }
    function FState([string]$tag) {
        $focus = FJson "$tag-focus" 'focused-window'
        $list = @(FJson "$tag-window-list" 'window-list')
        $workspaces = FJson "$tag-workspaces" 'workspaces'
        $windows = FJson "$tag-windows" 'windows'
        $monitors = @(FJson "$tag-monitors" 'list-monitors')
        return [pscustomobject]@{
            foreground=[int64][LT]::GetForegroundWindow()
            focus=$focus.focused_window; list=$list
            workspaces=@($workspaces.workspaces); windows=@($windows.windows)
            monitors=$monitors; forms=@(FormInfo $fHandles.ToArray())
        }
    }
    function FFocusIs($state, [int64]$expected) {
        if ($expected -eq 0) {
            return ($null -eq $state.focus -and $fHandles.ToArray() -notcontains [IntPtr]$state.foreground)
        }
        return ($null -ne $state.focus -and [int64]$state.focus.id -eq $expected -and $state.foreground -eq $expected)
    }
    function FWaitFocus([int64]$expected, [int]$timeout = 700) {
        # Poll the owned pipe rather than spending the full settle interval: an
        # action that takes effect at once does not make the phase slower.
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt $timeout) {
            $reply = [LaneFNative]::RequestOwned($fPipeName,$fRuntime.current,'{"type":"focused_window"}') | ConvertFrom-Json
            $focus = $reply.result.focused_window
            $foreground = [int64][LT]::GetForegroundWindow()
            if ($expected -eq 0) {
                if ($null -eq $focus -and $fHandles.ToArray() -notcontains [IntPtr]$foreground) { return $true }
            } elseif ($null -ne $focus -and [int64]$focus.id -eq $expected -and $foreground -eq $expected) {
                return $true
            }
            Start-Sleep -Milliseconds 20
        }
        return $false
    }
    function FRun([string]$label, [string]$transport, [string]$name, [object[]]$arguments, [int64]$expected, [bool[]]$quoted = @(), [int]$settle = 700, [Diagnostics.Stopwatch]$shortVisitClock = $null) {
        $fRuntime.serial++
        $tag = 'lane-f-{0:D3}-{1}' -f $fRuntime.serial,$label
        if ($null -eq $shortVisitClock) {
            $before = FState "$tag-before"
        } else {
            # Do not turn a short H31 endpoint into a settled visit by running
            # FState's five CLI subprocesses before focus-window-previous.
            $probe = [LaneFNative]::RequestOwned($fPipeName,$fRuntime.current,'{"type":"focused_window"}') | ConvertFrom-Json
            if (-not $probe.success) { throw 'Short-visit focus query failed' }
            $before = [pscustomobject]@{ foreground=[int64][LT]::GetForegroundWindow(); focus=$probe.result.focused_window }
        }
        $mark = @(EvList $fRuntime.events).Count
        FCommand $tag $transport $name $arguments $quoted
        # Include query/event/command overhead through the action reply. This is
        # a conservative bound on the visit at history dispatch, not a dwell claim.
        $dispatchUpperBound = if ($null -ne $shortVisitClock) { $shortVisitClock.ElapsedMilliseconds } else { $null }
        $null = FWaitFocus $expected $settle
        Start-Sleep -Milliseconds 80
        $after = FState "$tag-after"
        $events = @(@(EvList $fRuntime.events) | Select-Object -Skip $mark)
        $record = [pscustomobject]@{
            label=$label; transport=$transport; action=$name; args=@($arguments); quoted=@($quoted)
            expected_hwnd=$expected; pass=(FFocusIs $after $expected)
            before_capture=$(if ($null -eq $shortVisitClock) { 'full-state' } else { 'focus-only' })
            short_visit_until_dispatch_upper_bound_ms=$dispatchUpperBound
            before=$before; after=$after; events=$events
        }
        $record | ConvertTo-Json -Depth 16 | Set-Content (Join-Path $fDir "$tag.evidence.json")
        return $record
    }
    function FQuick([string]$name, [object[]]$arguments = @(), [string]$transport = 'cli', [bool[]]$quoted = @()) {
        $fRuntime.serial++
        FCommand ('lane-f-{0:D3}-setup' -f $fRuntime.serial) $transport $name $arguments $quoted
        Start-Sleep -Milliseconds 120
    }
    function FFocus([IntPtr]$handle, [int]$timeout = 400) {
        if (-not $fHandles.Contains($handle) -or -not [LT]::IsWindow($handle)) { throw "Not a live owned Lane F HWND: $handle" }
        $fRuntime.serial++
        $null = FCheckedCtl ('lane-f-{0:D3}-focus-owned' -f $fRuntime.serial) @('focus-window',[string][int64]$handle)
        $settled = FWaitFocus ([int64]$handle) $timeout
        $retried = $false
        if (-not $settled -and $timeout -ge 400) {
            # The e7ced50 and 8c6c9b0 live cycles each lost a whole group (H31, then H30) to ONE fixture
            # focus that did not settle, with every recorded product check passing. focus-window is
            # idempotent, so ask once more and wait again; the retry is recorded, and a window that
            # never settles still fails the fixture. Short-visit callers (< 400 ms) must not retry.
            $retried = $true
            $fRuntime.serial++
            $null = FCheckedCtl ('lane-f-{0:D3}-focus-owned-retry' -f $fRuntime.serial) @('focus-window',[string][int64]$handle)
            $settled = FWaitFocus ([int64]$handle) $timeout
            $fNotes['fixture-focus-retries'] = 1 + [int]$fNotes['fixture-focus-retries']
        }
        $null = WaitForeground $handle 0 $timeout
        Start-Sleep -Milliseconds 20
        $actual = [int64][LT]::GetForegroundWindow()
        [void]$fChecks.Add([pscustomobject]@{ gate='fixture-focus'; pass=($settled -and $actual -eq [int64]$handle); expected_hwnd=[int64]$handle; actual_hwnd=$actual; managed_focus_settled=$settled; retried=$retried })
        if (-not $settled -or $actual -ne [int64]$handle) { throw "Fixture focus expected managed/native $handle, got native $actual (managed settled: $settled)" }
    }
    function FHoldFocus([IntPtr]$handle, [int]$minimum = 1150) {
        FFocus $handle
        FocusForm $handle # Ordinary settled focus uses the shared harness helper.
        if (-not (FWaitFocus ([int64]$handle) 700)) { throw "Unsettled Lane F focus before hold: $handle" }
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ([LT]::GetForegroundWindow() -ne $handle) { throw "Lane F focus changed during hold: $handle" }
            Start-Sleep -Milliseconds 20
        } while ($clock.ElapsedMilliseconds -lt $minimum)
        if (-not (FWaitFocus ([int64]$handle) 700)) { throw "Unsettled Lane F focus after hold: $handle" }
        return $clock.ElapsedMilliseconds
    }
    function FWindow($state, [IntPtr]$handle) {
        return (@($state.windows | Where-Object { [int64]$_.id -eq [int64]$handle }) | Select-Object -First 1)
    }
    function FWorkspace($state, [string]$name) {
        return (@($state.workspaces | Where-Object { $_.name -ceq $name -and $_.output -eq $fHome }) | Select-Object -First 1)
    }
    function FMembers($state, $workspaceId) {
        return @($state.windows | Where-Object { [uint64]$_.workspace_id -eq [uint64]$workspaceId } | ForEach-Object { [int64]$_.id } | Sort-Object)
    }
    function FSameMembers($left, $right) { return ((@($left) -join ',') -ceq (@($right | Sort-Object) -join ',')) }
    function FTrailing($state) {
        foreach ($output in @($state.workspaces | ForEach-Object { $_.output } | Select-Object -Unique)) {
            $ordered = @($state.workspaces | Where-Object { $_.output -eq $output } | Sort-Object idx)
            if ($ordered.Count -eq 0) { return $false }
            $tail = $ordered[$ordered.Count - 1]
            if ($null -ne $tail.name -or @(FMembers $state $tail.id).Count -ne 0) { return $false }
            if ($ordered.Count -gt 1) {
                $penultimate = $ordered[$ordered.Count - 2]
                if ($null -eq $penultimate.name -and @(FMembers $state $penultimate.id).Count -eq 0) { return $false }
            }
            for ($index = 0; $index -lt $ordered.Count; $index++) {
                if ([int]$ordered[$index].idx -ne $index + 1) { return $false }
            }
        }
        return $true
    }
    function FStopSubscriber {
        if ($null -ne $fRuntime.subscriber) {
            if (-not $fRuntime.subscriber.HasExited) { $fRuntime.subscriber.Kill(); $null = $fRuntime.subscriber.WaitForExit(5000) }
            if ($fRuntime.subscriber.HasExited -and $fRuntime.subscriber.ExitCode -ne 0) {
                $fNotes["subscriber-exit-$($fRuntime.subscriber.Id)"] = $fRuntime.subscriber.ExitCode
            }
            $fRuntime.subscriber = $null
        }
    }
    function FStop($process, [string]$tag) {
        if ($null -eq $process -or -not $fOwned.Contains($process) -or $process.HasExited) { return }
        $detail = [ordered]@{ pid=$process.Id; pipe=$fPipeName; quit=''; forced=$false }
        try { $detail.quit = [LaneFNative]::RequestOwned($fPipeName,$process,'{"type":"quit"}') }
        catch { $detail.quit = [string]$_ }
        if (-not $process.WaitForExit(2500)) {
            $detail.forced = $true
            $process.Kill() # The retained Process handle, never a fresh PID/name lookup.
            if (-not $process.WaitForExit(5000)) { throw "Owned Lane F daemon failed to stop: $($process.Id)" }
        }
        $fNotes["$tag-stop-$($process.Id)"] = $detail
    }
    function FBegin([string]$phase) {
        FStopSubscriber
        FStop $fRuntime.current $phase
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'Lane F refuses to start while ANY wiri process exists'
        }
        [IO.File]::WriteAllText($cfg,$fConfig,(New-Object Text.UTF8Encoding($false)))
        $process = Start-Wiri (Join-Path $fDir "$phase.stderr.log") (Join-Path $fDir "$phase.stdout.log")
        [void]$fOwned.Add($process)
        $null = $process.Handle # Retain before any wait or check can fail.
        $fRuntime.current = $process
        Start-Sleep -Seconds 4
        if ($process.HasExited) { throw "$phase daemon exited at startup" }
        # A PID-checked request also proves that Ctl/events will reach our daemon.
        $probe = [LaneFNative]::RequestOwned($fPipeName,$process,'{"type":"focused_window"}') | ConvertFrom-Json
        if (-not $probe.success) { throw 'Owned pipe readiness query failed' }
        $fRuntime.events = Join-Path $fDir "$phase.events.jsonl"
        $fRuntime.subscriber = Start-Process -FilePath $ctl -ArgumentList @('events','--stream') -RedirectStandardOutput $fRuntime.events -RedirectStandardError "$($fRuntime.events).err" -WindowStyle Hidden -PassThru
        [void]$fClients.Add($fRuntime.subscriber)
        $null = $fRuntime.subscriber.Handle
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if ($fRuntime.subscriber.HasExited) { throw "Subscriber exited: $phase" }
            $replay = @(EvList $fRuntime.events)
            if (@(EvOf $replay 'config_loaded').Count -gt 0) { break }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt 5000)
        if ($replay.Count -lt 5 -or (@($replay | Select-Object -First 5 | ForEach-Object { $_.type }) -join ',') -ne 'workspaces_changed,windows_changed,overview_opened_or_closed,keyboard_layouts_changed,config_loaded') {
            throw 'Missing or out-of-order subscriber state replay'
        }
        if (@(EvOf $replay 'config_loaded' | Where-Object { $_.data.failed }).Count -gt 0) { throw 'Lane F config was rejected' }
    }
    function FFixture {
        # Rebuild from an empty source strip without touching non-owned windows.
        FQuick 'focus-monitor' @($fHome)
        # Normalize floating windows BEFORE rebuilding: unfloat appends a column.
        foreach ($handle in $fHandles.ToArray()) {
            if (-not [LT]::IsWindow($handle)) { throw 'The four-window fixture is no longer intact' }
            FFocus $handle
            $fRuntime.serial++
            $entry = WinOf @(FJson ('lane-f-{0:D3}-float-check' -f $fRuntime.serial) 'window-list') $handle
            if ($entry.floating) { FQuick 'toggle-floating' }
        }
        foreach ($handle in $fHandles.ToArray()) {
            FQuick 'move-window-to-workspace' @($fTarget,'--id',[int64]$handle,'--focus',$false) 'raw'
        }
        foreach ($handle in $fHandles.ToArray()) {
            FQuick 'move-window-to-workspace' @($fSource,'--id',[int64]$handle,'--focus',$false) 'raw'
        }
        FQuick 'focus-workspace' @($fSource)
        # Transfers insert beside the destination's active column, not at the
        # end. Explicitly establish A,B,C,D before building the stacked fixture.
        foreach ($handle in $fHandles.ToArray()) {
            FFocus $handle
            FQuick 'move-column-to-last'
        }
        # Consume B into A and D into C; check, rather than assume, the result.
        FFocus $fA
        FQuick 'consume-window-into-column'
        FFocus $fC
        FQuick 'consume-window-into-column'
        FFocus $fA
        $fRuntime.serial++
        $fixtureTag = 'lane-f-{0:D3}-fixture' -f $fRuntime.serial
        $state = FState $fixtureTag
        $state | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $fDir "$fixtureTag.json")
        $source = FWorkspace $state $fSource
        $target = FWorkspace $state $fTarget
        $expected = @(@($fA,0,0),@($fB,0,1),@($fC,1,0),@($fD,1,1))
        $valid = ($null -ne $source -and $null -ne $target -and (FSameMembers (FMembers $state $source.id) @([int64]$fA,[int64]$fB,[int64]$fC,[int64]$fD)) -and @(FMembers $state $target.id).Count -eq 0)
        foreach ($position in $expected) {
            $entry = WinOf $state.list $position[0]
            $valid = $valid -and $null -ne $entry -and -not $entry.floating -and [int]$entry.column -eq $position[1] -and [int]$entry.tile -eq $position[2]
        }
        $state | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $fDir 'fixture.last.json')
        if (-not $valid) { throw 'Expected A/B and C/D stacked columns on source, and an empty target' }
        return $state
    }
    function FFocusCheck([string]$gate, [string]$label, [IntPtr]$start, [string]$transport, [string]$action, [object[]]$arguments, [IntPtr]$expected, [bool[]]$quoted = @()) {
        FFocus $start
        $record = FRun $label $transport $action $arguments ([int64]$expected) $quoted
        FCheck $gate $record.pass ([ordered]@{ action=$action; transport=$transport; start=[int64]$start; expected=[int64]$expected; actual=$record.after.foreground })
        return $record
    }

    function FPlace([IntPtr]$handle, [int]$x, [int]$y) {
        if (-not $fHandles.Contains($handle)) { throw 'Refusing to position a non-owned HWND' }
        $fRuntime.serial++
        $tag = 'lane-f-{0:D3}-place-owned' -f $fRuntime.serial
        $null = FCheckedCtl "$tag-size" @('resize-window',[string][int64]$handle,'120','90')
        $null = FCheckedCtl "$tag-position" @('move-window',[string][int64]$handle,[string]$x,[string]$y)
        Start-Sleep -Milliseconds 350
    }
    function FTransferCheck($record, [string]$destination, [IntPtr[]]$moved, [object[]]$positions) {
        $state = $record.after
        $source = FWorkspace $state $fSource
        $target = FWorkspace $state $fTarget
        $all = @($fHandles.ToArray() | ForEach-Object { [int64]$_ })
        $moving = @($moved | ForEach-Object { [int64]$_ })
        $sourceExpected = $all
        $targetExpected = @()
        if ($destination -eq $fTarget) {
            $sourceExpected = @($all | Where-Object { $moving -notcontains $_ })
            $targetExpected = $moving
        }
        $valid = ($record.pass -and $null -ne $source -and $null -ne $target -and
            (FSameMembers (FMembers $state $source.id) $sourceExpected) -and
            (FSameMembers (FMembers $state $target.id) $targetExpected))
        foreach ($position in $positions) {
            $entry = WinOf $state.list $position.hwnd
            $destinationWorkspace = if ($destination -eq $fTarget) { $target } else { $source }
            $window = FWindow $state $position.hwnd
            $valid = ($valid -and $null -ne $entry -and $null -ne $window -and -not $entry.floating -and
                [uint64]$window.workspace_id -eq [uint64]$destinationWorkspace.id -and
                [int]$entry.column -eq [int]$position.column -and [int]$entry.tile -eq [int]$position.tile)
        }
        FCheck 'h32_transfers_preserve_membership_and_placement' $valid ([ordered]@{
            label=$record.label; destination=$destination; moved=$moving; positions=$positions
            source_members=@(FMembers $state $source.id); target_members=@(FMembers $state $target.id)
        })
        return $valid
    }

    try {
        Add-Type -TypeDefinition @'
using System;
using System.Diagnostics;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class LaneFNative {
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool GetUserNameW(StringBuilder name, ref uint size);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool ProcessIdToSessionId(uint pid, out uint session);
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
    public static string PipeName(string configured) {
        string name = (configured ?? "").Trim();
        if (name.Length == 0) {
            uint length = 32768, session;
            var user = new StringBuilder((int)length);
            if (!GetUserNameW(user, ref length)) throw new System.ComponentModel.Win32Exception();
            if (!ProcessIdToSessionId((uint)Process.GetCurrentProcess().Id, out session)) throw new System.ComponentModel.Win32Exception();
            var safe = new StringBuilder();
            foreach (char ch in user.ToString()) safe.Append((ch >= 'A' && ch <= 'Z') || (ch >= 'a' && ch <= 'z') ||
                (ch >= '0' && ch <= '9') || ch == '.' || ch == '_' || ch == '-' ? ch : '_');
            if (safe.Length == 0) safe.Append("user");
            name = "wiri_control_" + safe + "_" + session;
        }
        const string prefix = @"\\.\pipe\";
        return name.StartsWith(prefix, StringComparison.OrdinalIgnoreCase) ? name.Substring(prefix.Length) : name;
    }
    static int Remaining(Stopwatch clock) {
        int remaining = 5000 - (int)clock.ElapsedMilliseconds;
        if (remaining <= 0) throw new TimeoutException("Lane F pipe request exceeded 5000 ms");
        return remaining;
    }
    public static string RequestOwned(string name, Process owned, string request) {
        if (owned == null || owned.HasExited) throw new InvalidOperationException("Owned daemon is not running");
        // Touch the retained handle before connecting; do not open another Process by PID.
        IntPtr retained = owned.Handle;
        var clock = Stopwatch.StartNew();
        using (var pipe = new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            pipe.Connect(Remaining(clock));
            uint server;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle,out server)) throw new System.ComponentModel.Win32Exception();
            if (server != (uint)owned.Id || owned.HasExited) throw new InvalidOperationException("Connected pipe is not the retained Lane F daemon");
            byte[] bytes = Encoding.UTF8.GetBytes(request);
            if (!pipe.WriteAsync(bytes,0,bytes.Length).Wait(Remaining(clock))) throw new TimeoutException("Lane F pipe write");
            using (var response = new MemoryStream()) {
                byte[] buffer = new byte[4096];
                while (true) {
                    var read = pipe.ReadAsync(buffer,0,buffer.Length);
                    if (!read.Wait(Remaining(clock))) throw new TimeoutException("Lane F pipe read");
                    if (read.Result == 0) break;
                    if (response.Length + read.Result > 65536) throw new InvalidDataException("Lane F response exceeded 65536 bytes");
                    response.Write(buffer,0,read.Result);
                }
                return Encoding.UTF8.GetString(response.ToArray());
            }
        }
    }
}
'@
        $fPipeName = [LaneFNative]::PipeName($fSavedEnvironment['WIRI_PIPE'])
        # Canonicalize only in this lane, then restore the original value verbatim.
        $env:WIRI_PIPE = '\\.\pipe\' + $fPipeName
        # Do not read, overwrite or delete the real snapshot directory, whether
        # selected by override, APPDATA\wiri\snapshots or USERPROFILE\.wiri\snapshots.
        $env:WIRI_SNAPSHOT_DIR = $fSnapshots
        $fHome = [Windows.Forms.Screen]::PrimaryScreen.DeviceName
        $fConfig = @'
animations { off }
input { focus-follows-mouse false; }
workspace "lane-f-desktop"
workspace "lane-f-source"
workspace "lane-f-target"
binds {
    Ctrl+Alt+Shift+F5 { focus-column 2; }
    Ctrl+Alt+Shift+F6 { focus-window-in-column 2; }
}
window-rule {
    match title="^lane-f-owned-"
    open-on-workspace "lane-f-source"
}
'@
        FBegin 'h30'
        foreach ($index in 1..4) {
            $handle = [TestForms]::Open("lane-f-owned-$index",(35 + 40*$index),(180 - 25*$index),(60 + 25*$index))
            [void]$fHandles.Add($handle) # Retain before any settling delay.
            Start-Sleep -Milliseconds 800
        }
        $fA=$fHandles[0]; $fB=$fHandles[1]; $fC=$fHandles[2]; $fD=$fHandles[3]
        $fTopologyBefore = @([Windows.Forms.Screen]::AllScreens | ForEach-Object { '{0}:{1}:{2}:{3}' -f $_.DeviceName,$_.Bounds,$_.WorkingArea,$_.Primary } | Sort-Object)

        $fComplete = $false
        try {
            $fInitial = FFixture
            FCheck 'h30_two_columns_two_test_workspaces' $true $fInitial
            foreach ($fTransport in @('cli','raw')) {
                $null = FFixture
                # Seed B and D as the remembered tiles. FFocusCheck deliberately
                # changes only the starting column's memory for each case.
                $fCases = @(
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column';v=@(1);s=$fC;e=$fB},
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column';v=@(0);s=$fC;e=$fB},
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column';v=@(2);s=$fA;e=$fD},
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column';v=@(999);s=$fA;e=$fD},
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column-first';v=@();s=$fC;e=$fB},
                    @{g='h30_indexed_columns_clamp_and_remember_tiles';a='focus-column-last';v=@();s=$fA;e=$fD},
                    @{g='h30_indexed_windows_clamp';a='focus-window-in-column';v=@(1);s=$fB;e=$fA},
                    @{g='h30_indexed_windows_clamp';a='focus-window-in-column';v=@(0);s=$fD;e=$fC},
                    @{g='h30_indexed_windows_clamp';a='focus-window-in-column';v=@(2);s=$fA;e=$fB},
                    @{g='h30_indexed_windows_clamp';a='focus-window-in-column';v=@(999);s=$fC;e=$fD},
                    @{g='h30_column_wraps';a='focus-column-right-or-first';v=@();s=$fA;e=$fD},
                    @{g='h30_column_wraps';a='focus-column-right-or-first';v=@();s=$fD;e=$fB},
                    @{g='h30_column_wraps';a='focus-column-left-or-last';v=@();s=$fC;e=$fB},
                    @{g='h30_column_wraps';a='focus-column-left-or-last';v=@();s=$fB;e=$fD},
                    @{g='h30_window_edges_and_wraps';a='focus-window-up';v=@();s=$fA;e=$fA},
                    @{g='h30_window_edges_and_wraps';a='focus-window-down';v=@();s=$fB;e=$fB},
                    @{g='h30_window_edges_and_wraps';a='focus-window-top';v=@();s=$fB;e=$fA},
                    @{g='h30_window_edges_and_wraps';a='focus-window-bottom';v=@();s=$fC;e=$fD},
                    @{g='h30_window_edges_and_wraps';a='focus-window-down-or-top';v=@();s=$fA;e=$fB},
                    @{g='h30_window_edges_and_wraps';a='focus-window-down-or-top';v=@();s=$fB;e=$fA},
                    @{g='h30_window_edges_and_wraps';a='focus-window-up-or-bottom';v=@();s=$fD;e=$fC},
                    @{g='h30_window_edges_and_wraps';a='focus-window-up-or-bottom';v=@();s=$fC;e=$fD},
                    @{g='h30_composites_use_tiles_before_fallback';a='focus-window-down-or-column-left';v=@();s=$fC;e=$fD},
                    @{g='h30_composites_use_tiles_before_fallback';a='focus-window-down-or-column-right';v=@();s=$fA;e=$fB},
                    @{g='h30_composites_use_tiles_before_fallback';a='focus-window-up-or-column-left';v=@();s=$fD;e=$fC},
                    @{g='h30_composites_use_tiles_before_fallback';a='focus-window-up-or-column-right';v=@();s=$fB;e=$fA},
                    @{g='h30_composites_fallback_at_edges';a='focus-window-down-or-column-left';v=@();s=$fD;e=$fB},
                    @{g='h30_composites_fallback_at_edges';a='focus-window-down-or-column-right';v=@();s=$fB;e=$fD},
                    @{g='h30_composites_fallback_at_edges';a='focus-window-up-or-column-left';v=@();s=$fC;e=$fB},
                    @{g='h30_composites_fallback_at_edges';a='focus-window-up-or-column-right';v=@();s=$fA;e=$fD}
                )
                foreach ($fCase in $fCases) {
                    FFocus $fB; FFocus $fD
                    $null = FFocusCheck $fCase.g "h30-$($fCase.a)-$fTransport" $fCase.s $fTransport $fCase.a $fCase.v $fCase.e
                }
                FFocus $fB; FFocus $fD
                foreach ($fStep in @(
                    @{a='focus-column-left';e=$fB},@{a='focus-column-right';e=$fD},
                    @{a='focus-column-left';e=$fB},@{a='focus-column-right';e=$fD}
                )) {
                    $fRecord = FRun "h30-column-memory-$fTransport" $fTransport $fStep.a @() ([int64]$fStep.e)
                    FCheck 'h30_horizontal_focus_remembers_each_column' $fRecord.pass $fRecord
                }

                $null = FFixture
                FQuick 'move-window-to-workspace' @($fTarget,'--id',[int64]$fD,'--focus',$false) 'raw'
                foreach ($fCase in @(
                    @{a='focus-window-or-workspace-down';s=$fA;e=$fB},
                    @{a='focus-window-or-workspace-up';s=$fB;e=$fA}
                )) {
                    $null = FFocusCheck 'h30_window_or_workspace_focus' "h30-workspace-primary-$fTransport" $fCase.s $fTransport $fCase.a @() $fCase.e
                }
                FFocus $fD; FFocus $fB
                $fRecord = FRun "h30-workspace-down-edge-$fTransport" $fTransport 'focus-window-or-workspace-down' @() ([int64]$fD)
                $fWs = FWorkspace $fRecord.after $fTarget
                FCheck 'h30_window_or_workspace_focus' ($fRecord.pass -and $fWs.is_active) $fRecord
                $fRecord = FRun "h30-workspace-up-edge-$fTransport" $fTransport 'focus-window-or-workspace-up' @() ([int64]$fB)
                $fWs = FWorkspace $fRecord.after $fSource
                FCheck 'h30_window_or_workspace_focus' ($fRecord.pass -and $fWs.is_active) $fRecord

                $null = FFixture
                foreach ($fHandle in $fHandles.ToArray()) { FFocus $fHandle; FQuick 'toggle-floating' }
                $fArea = ([Windows.Forms.Screen]::AllScreens | Where-Object { $_.DeviceName -eq $fHome }).WorkingArea
                $fX = $fArea.X + 80; $fY = $fArea.Y + 150
                FPlace $fA $fX $fY
                FPlace $fB ($fX+160) $fY
                FPlace $fC $fX ($fY+120)
                FPlace $fD ($fX+60) $fY
                FFocus $fA
                FQuick 'move-window-to-workspace' @($fTarget,'--id',[int64]$fD,'--focus',$false) 'raw'
                foreach ($fCase in @(
                    @{a='focus-right';s=$fA;e=$fB},@{a='focus-left';s=$fB;e=$fA},
                    @{a='focus-down';s=$fA;e=$fC},@{a='focus-up';s=$fC;e=$fA}
                )) {
                    $fRecord = FFocusCheck 'h30_floating_directional_focus_stays_in_workspace' "h30-float-$($fCase.a)-$fTransport" $fCase.s $fTransport $fCase.a @() $fCase.e
                    $fWs = FWorkspace $fRecord.after $fSource
                    FCheck 'h30_floating_directional_focus_stays_in_workspace' ($fWs.is_active -and [uint64](FWindow $fRecord.after $fCase.e).workspace_id -eq [uint64]$fWs.id) $fRecord.after
                }
                # Equal squared distances: the lower HWND wins, not enumeration order.
                FPlace $fB ($fX+160) ($fY-60)
                FPlace $fC ($fX+160) ($fY+60)
                $fTie = if ([int64]$fB -lt [int64]$fC) { $fB } else { $fC }
                $null = FFocusCheck 'h30_floating_directional_focus_stays_in_workspace' "h30-float-distance-tie-$fTransport" $fA $fTransport 'focus-right' @() $fTie
                foreach ($fCase in @(
                    @{a='focus-column';v=@(1)},@{a='focus-column';v=@(999)},
                    @{a='focus-column-first';v=@()},@{a='focus-column-last';v=@()},
                    @{a='focus-window-in-column';v=@(2)},@{a='focus-window-top';v=@()},
                    @{a='focus-window-bottom';v=@()},@{a='focus-window-down-or-top';v=@()},
                    @{a='focus-window-up-or-bottom';v=@()}
                )) {
                    $null = FFocusCheck 'h30_floating_tile_indices_are_noops' "h30-float-tile-noop-$fTransport" $fA $fTransport $fCase.a $fCase.v $fA
                }
                FCheck 'h30_cli_and_typed_run_action' $true @{transport=$fTransport; typed_numeric_args=($fTransport -eq 'raw')}
            }
            $null = FFixture
            FFocus $fD
            FFocus $fA
            Chord 'ctrl+alt+shift+f5'
            Start-Sleep -Milliseconds 700
            $fKeyColumn = FState 'lane-f-h30-kdl-column'
            FFocus $fC
            Chord 'ctrl+alt+shift+f6'
            Start-Sleep -Milliseconds 700
            $fKeyWindow = FState 'lane-f-h30-kdl-window'
            FCheck 'h30_kdl_binds_execute_layout_actions' ((FFocusIs $fKeyColumn ([int64]$fD)) -and (FFocusIs $fKeyWindow ([int64]$fD))) @{column=$fKeyColumn;window=$fKeyWindow}
            $fComplete = $true
        } catch { $fNotes['h30-error'] = ($_ | Out-String) }
        finally { FFinish 'h30_' $fComplete }
        $fComplete = $false
        try {
            foreach ($fTransport in @('cli','raw')) {
                # niri's `focus true` (the default of move-window-to-workspace --window-id) is
                # ActivateWindow::Smart (input/mod.rs:1420-1470), and Monitor::move_to_workspace
                # resolves Smart as "the moved window is the monitor's active window"
                # (layout/monitor.rs:843-845, pinned e19df12): a window that is NOT focused is
                # moved without taking the focus, whatever the flag says, and only the focused
                # one is followed.  So with --id naming an unfocused B, A stays focused and the
                # source workspace stays active for the default AND for --focus false; the
                # default follows only when --id names the window that already has the focus.
                foreach ($fByIndex in @($false,$true)) {
                    foreach ($fFollow in @($true,$false)) {
                        $fState = FFixture
                        $fReference = if ($fByIndex) { [int](FWorkspace $fState $fTarget).idx } else { $fTarget }
                        # The explicit HWND is not focused before dispatch: A stays focused.
                        FFocus $fA
                        $fArgs = @($fReference,'--id',[int64]$fB)
                        if (-not $fFollow) { $fArgs += @('--focus',$false) }
                        $fRecord = FRun "h32-window-$fByIndex-$fFollow-$fTransport" $fTransport 'move-window-to-workspace' $fArgs ([int64]$fA)
                        $fValid = FTransferCheck $fRecord $fTarget @($fB) @(@{hwnd=$fB;column=0;tile=0})
                        $fGate = if ($fFollow) { 'h32_window_transfer_names_indices_default_focus' } else { 'h32_window_transfer_focus_false_preserves_source' }
                        $fSourceWs = FWorkspace $fRecord.after $fSource
                        $fTargetWs = FWorkspace $fRecord.after $fTarget
                        FCheck $fGate ($fValid -and $fSourceWs.is_active -and -not $fTargetWs.is_active) $fRecord
                    }
                    # --id names the window that already has the focus: the default follows it
                    # to the target workspace (Smart is true), by name and by index.
                    $fState = FFixture
                    $fReference = if ($fByIndex) { [int](FWorkspace $fState $fTarget).idx } else { $fTarget }
                    FFocus $fA; FFocus $fB
                    $fRecord = FRun "h32-window-focused-$fByIndex-$fTransport" $fTransport 'move-window-to-workspace' @($fReference,'--id',[int64]$fB) ([int64]$fB)
                    $fValid = FTransferCheck $fRecord $fTarget @($fB) @(@{hwnd=$fB;column=0;tile=0})
                    $fSourceWs = FWorkspace $fRecord.after $fSource
                    $fTargetWs = FWorkspace $fRecord.after $fTarget
                    FCheck 'h32_window_transfer_names_indices_default_focus' ($fValid -and $fTargetWs.is_active -and -not $fSourceWs.is_active) $fRecord
                }
                # No --id: the focused window is the target. Exercise inline flags,
                # explicit true, and the legacy non-following numeric action too.
                foreach ($fCase in @(
                    @{a='move-window-to-workspace';v=@($fTarget);follow=$true},
                    @{a='move-window-to-workspace';v=@($fTarget,'--focus=true');follow=$true},
                    @{a='move-window-to-workspace';v=@($fTarget,'--focus=false');follow=$false}
                )) {
                    $null = FFixture
                    FFocus $fA; FFocus $fB
                    $fExpected = if ($fCase.follow) { $fB } else { $fA }
                    $fRecord = FRun "h32-implicit-window-$fTransport" $fTransport $fCase.a $fCase.v ([int64]$fExpected)
                    $fValid = FTransferCheck $fRecord $fTarget @($fB) @(@{hwnd=$fB;column=0;tile=0})
                    $fGate = if ($fCase.follow) { 'h32_window_transfer_names_indices_default_focus' } else { 'h32_window_transfer_focus_false_preserves_source' }
                    FCheck $fGate $fValid $fRecord
                }
                $fState = FFixture
                FFocus $fA
                $fRecord = FRun "h32-legacy-window-id-$fTransport" $fTransport 'move-to-workspace' @([int](FWorkspace $fState $fTarget).idx,"--id=$([int64]$fB)") ([int64]$fA)
                $fValid = FTransferCheck $fRecord $fTarget @($fB) @(@{hwnd=$fB;column=0;tile=0})
                FCheck 'h32_window_transfer_focus_false_preserves_source' $fValid $fRecord

                foreach ($fByIndex in @($false,$true)) {
                    foreach ($fFollow in @($true,$false)) {
                        $fState = FFixture
                        $fReference = if ($fByIndex) { [int](FWorkspace $fState $fTarget).idx } else { $fTarget }
                        FFocus $fC; FFocus $fB
                        $fArgs = @($fReference)
                        if (-not $fFollow) { $fArgs += @('--focus',$false) }
                        $fExpected = if ($fFollow) { $fB } else { $fC }
                        $fRecord = FRun "h32-column-$fByIndex-$fFollow-$fTransport" $fTransport 'move-column-to-workspace' $fArgs ([int64]$fExpected)
                        $fValid = FTransferCheck $fRecord $fTarget @($fA,$fB) @(@{hwnd=$fA;column=0;tile=0},@{hwnd=$fB;column=0;tile=1})
                        $fGate = if ($fFollow) { 'h32_column_transfer_default_focus' } else { 'h32_column_transfer_focus_false' }
                        FCheck $fGate $fValid $fRecord
                    }
                }
                foreach ($fFollow in @($true,$false)) {
                    $null = FFixture
                    FFocus $fA; FFocus $fB
                    $fArgs = @()
                    if (-not $fFollow) { $fArgs = @('--focus',$false) }
                    $fExpected = if ($fFollow) { $fB } else { $fA }
                    $fRecord = FRun "h32-relative-window-down-$fFollow-$fTransport" $fTransport 'move-window-to-workspace-down' $fArgs ([int64]$fExpected)
                    $fValid = FTransferCheck $fRecord $fTarget @($fB) @(@{hwnd=$fB;column=0;tile=0})
                    FCheck 'h32_relative_window_transfers' $fValid $fRecord
                    if ($fFollow) { FFocus $fB; $fArgs=@() } else { FFocus $fC; $fArgs=@('--id',[int64]$fB,'--focus',$false) }
                    $fExpected = if ($fFollow) { $fB } else { $fC }
                    $fRecord = FRun "h32-relative-window-up-$fFollow-$fTransport" $fTransport 'move-window-to-workspace-up' $fArgs ([int64]$fExpected)
                    # Insert after the source's remembered active column: A (0)
                    # after a following move, or explicitly focused C (1) otherwise.
                    $fReturnColumn = if ($fFollow) { 1 } else { 2 }
                    $fValid = FTransferCheck $fRecord $fSource @($fB) @(@{hwnd=$fB;column=$fReturnColumn;tile=0})
                    FCheck 'h32_relative_window_transfers' $fValid $fRecord

                    $null = FFixture
                    FFocus $fC; FFocus $fB
                    $fArgs = @()
                    if (-not $fFollow) { $fArgs=@('--focus',$false) }
                    $fExpected = if ($fFollow) { $fB } else { $fC }
                    $fRecord = FRun "h32-relative-column-down-$fFollow-$fTransport" $fTransport 'move-column-to-workspace-down' $fArgs ([int64]$fExpected)
                    $fValid = FTransferCheck $fRecord $fTarget @($fA,$fB) @(@{hwnd=$fA;column=0;tile=0},@{hwnd=$fB;column=0;tile=1})
                    FCheck 'h32_relative_column_transfers' $fValid $fRecord
                    FFocus $fB
                    # On a non-following move the now-empty target must lose managed
                    # focus, rather than leaving a hidden owned HWND as foreground.
                    $fExpected = if ($fFollow) { $fB } else { [IntPtr]::Zero }
                    $fRecord = FRun "h32-relative-column-up-$fFollow-$fTransport" $fTransport 'move-column-to-workspace-up' $fArgs ([int64]$fExpected)
                    $fValid = FTransferCheck $fRecord $fSource @($fA,$fB) @(@{hwnd=$fA;column=1;tile=0},@{hwnd=$fB;column=1;tile=1})
                    FCheck 'h32_relative_column_transfers' $fValid $fRecord
                }
                FCheck 'h32_cli_and_typed_run_action' $true @{transport=$fTransport; numeric_workspace_index=$true; numeric_hwnd=$true; boolean_focus=$true}
            }
            $fComplete = $true
        } catch { $fNotes['h32-error'] = ($_ | Out-String) }
        finally { FFinish 'h32_' $fComplete }
        $fComplete = $false
        try {
            FBegin 'h33'
            $fState = FState 'lane-f-h33-startup'
            foreach ($fOutput in @($fState.monitors | ForEach-Object { $_.name })) {
                $fNames = @($fState.workspaces | Where-Object { $_.output -eq $fOutput -and $null -ne $_.name } | Sort-Object idx | ForEach-Object { $_.name })
                FCheck 'h33_named_startup_order' (($fNames -join ',') -ceq 'lane-f-desktop,lane-f-source,lane-f-target') @{output=$fOutput;names=$fNames}
            }
            FCheck 'h33_exactly_one_trailing_empty_workspace' (FTrailing $fState) $fState.workspaces
            foreach ($fTransport in @('cli','raw')) {
                $fState = FFixture
                $fSourceId = (FWorkspace $fState $fSource).id
                $fTargetId = (FWorkspace $fState $fTarget).id
                FFocus $fA
                FQuick 'move-window-to-workspace' @($fTarget,'--id',[int64]$fD,'--focus',$false) 'raw'
                FFocus $fD
                # The omitted reference acts on the active, occupied workspace.
                # Keeping D here also prevents unset-name from pruning the subject.
                $fName = "lane-f renamed $fTransport"
                $fRecord = FRun "h33-set-current-$fTransport" $fTransport 'set-workspace-name' @($fName) ([int64]$fD)
                $fRenamed = FWorkspace $fRecord.after $fName
                FCheck 'h33_set_unset_workspace_names' ($fRecord.pass -and $null -ne $fRenamed -and [uint64]$fRenamed.id -eq [uint64]$fTargetId -and $null -eq (FWorkspace $fRecord.after $fTarget)) $fRecord
                $fRecord = FRun "h33-unset-current-$fTransport" $fTransport 'unset-workspace-name' @() ([int64]$fD)
                $fUnnamed = @($fRecord.after.workspaces | Where-Object { [uint64]$_.id -eq [uint64]$fTargetId })
                FCheck 'h33_set_unset_workspace_names' ($fRecord.pass -and $fUnnamed.Count -eq 1 -and $null -eq $fUnnamed[0].name -and $fUnnamed[0].is_active -and (FSameMembers (FMembers $fRecord.after $fTargetId) @([int64]$fD))) $fRecord
                FQuick 'set-workspace-name' @($fTarget) $fTransport
                FFocus $fA
                # Explicit references must not activate the workspace being renamed.
                # Use a name, an index, split and inline --workspace spellings.
                $fRecord = FRun "h33-set-explicit-name-$fTransport" $fTransport 'set-workspace-name' @($fName,'--workspace',$fTarget) ([int64]$fA)
                $fRenamed = FWorkspace $fRecord.after $fName
                FCheck 'h33_optional_workspace_reference' ($fRecord.pass -and [uint64]$fRenamed.id -eq [uint64]$fTargetId -and (FWorkspace $fRecord.after $fSource).is_active) $fRecord
                $fIndex = [int]$fRenamed.idx
                $fRecord = FRun "h33-unset-explicit-index-$fTransport" $fTransport 'unset-workspace-name' @('--workspace',$fIndex) ([int64]$fA)
                $fUnnamed = @($fRecord.after.workspaces | Where-Object { [uint64]$_.id -eq [uint64]$fTargetId })
                FCheck 'h33_optional_workspace_reference' ($fRecord.pass -and $fUnnamed.Count -eq 1 -and $null -eq $fUnnamed[0].name -and (FWorkspace $fRecord.after $fSource).is_active) $fRecord
                FQuick 'set-workspace-name' @($fTarget,"--workspace=$fIndex") $fTransport

                # Reorder only the two owned workspaces, never the desktop parking
                # workspace. Stable IPC identities, membership and focus must survive.
                $fState = FState "lane-f-h33-before-reorder-$fTransport"
                $fSourceIndex = [int](FWorkspace $fState $fSource).idx
                $fTargetIndex = [int](FWorkspace $fState $fTarget).idx
                $fDesktopId = (FWorkspace $fState 'lane-f-desktop').id
                $fRecord = FRun "h33-reorder-explicit-$fTransport" $fTransport 'move-workspace-to-index' @($fSourceIndex,'--workspace',$fTarget) ([int64]$fA)
                $fMoved = FWorkspace $fRecord.after $fTarget
                $fStayed = FWorkspace $fRecord.after $fSource
                $fValid = ($fRecord.pass -and [uint64]$fMoved.id -eq [uint64]$fTargetId -and [uint64]$fStayed.id -eq [uint64]$fSourceId -and [int]$fMoved.idx -eq $fSourceIndex -and [int]$fStayed.idx -eq $fTargetIndex -and $fStayed.is_active -and [uint64](FWorkspace $fRecord.after 'lane-f-desktop').id -eq [uint64]$fDesktopId -and (FSameMembers (FMembers $fRecord.after $fTargetId) @([int64]$fD)))
                FCheck 'h33_workspace_reordering' $fValid $fRecord
                FCheck 'h33_exactly_one_trailing_empty_workspace' (FTrailing $fRecord.after) $fRecord.after.workspaces
                # The omitted reference reorders the active source back to index 2.
                $fRecord = FRun "h33-reorder-current-$fTransport" $fTransport 'move-workspace-to-index' @($fSourceIndex) ([int64]$fA)
                $fValid = ($fRecord.pass -and [int](FWorkspace $fRecord.after $fSource).idx -eq $fSourceIndex -and [int](FWorkspace $fRecord.after $fTarget).idx -eq $fTargetIndex -and [uint64](FWorkspace $fRecord.after $fSource).id -eq [uint64]$fSourceId -and [uint64](FWorkspace $fRecord.after $fTarget).id -eq [uint64]$fTargetId)
                FCheck 'h33_workspace_reordering' $fValid $fRecord
                FCheck 'h33_exactly_one_trailing_empty_workspace' (FTrailing $fRecord.after) $fRecord.after.workspaces

                # Occupy the unnamed tail with one owned HWND, then empty it again.
                # Each state must have exactly one unnamed, unoccupied trailing slot.
                $fTail = @($fRecord.after.workspaces | Where-Object { $_.output -eq $fHome } | Sort-Object idx)[-1]
                $fRecord = FRun "h33-fill-trailing-$fTransport" $fTransport 'move-window-to-workspace' @([int]$fTail.idx,'--id',[int64]$fD,'--focus',$false) ([int64]$fA)
                $fFilled = @($fRecord.after.workspaces | Where-Object { [uint64]$_.id -eq [uint64]$fTail.id })
                FCheck 'h33_exactly_one_trailing_empty_workspace' ($fRecord.pass -and $fFilled.Count -eq 1 -and (FSameMembers (FMembers $fRecord.after $fTail.id) @([int64]$fD)) -and (FTrailing $fRecord.after)) $fRecord
                $fRecord = FRun "h33-empty-trailing-$fTransport" $fTransport 'move-window-to-workspace' @($fTarget,'--id',[int64]$fD,'--focus',$false) ([int64]$fA)
                FCheck 'h33_exactly_one_trailing_empty_workspace' ($fRecord.pass -and (FTrailing $fRecord.after)) $fRecord

                FFocus $fD
                FQuick 'toggle-floating'
                $fRecord = FRun "h33-hide-float-$fTransport" $fTransport 'focus-workspace' @($fSource) ([int64]$fA)
                $fEntry = WinOf $fRecord.after.list $fD
                $fWindow = FWindow $fRecord.after $fD
                $fHidden = (-not [LT]::IsWindowVisible($fD) -or [LT]::Cloaked($fD) -gt 0)
                FCheck 'h33_floating_workspace_hide_show' ($fRecord.pass -and $fHidden -and $fEntry.floating -and [uint64]$fWindow.workspace_id -eq [uint64]$fTargetId -and (FWorkspace $fRecord.after $fSource).is_active) @{record=$fRecord;hidden=$fHidden;cloaked=[LT]::Cloaked($fD)}
                $fRecord = FRun "h33-show-float-$fTransport" $fTransport 'focus-workspace' @($fTarget) ([int64]$fD)
                FCheck 'h33_floating_workspace_hide_show' ($fRecord.pass -and [LT]::IsWindowVisible($fD) -and [LT]::Cloaked($fD) -eq 0 -and (FWorkspace $fRecord.after $fTarget).is_active -and (WinOf $fRecord.after.list $fD).floating) $fRecord
                FCheck 'h33_exactly_one_trailing_empty_workspace' (FTrailing $fRecord.after) $fRecord.after.workspaces
                FCheck 'h33_cli_and_typed_run_action' $true @{transport=$fTransport;numeric_workspace_index=$true;optional_reference=$true}
            }
            $fComplete = $true
        } catch { $fNotes['h33-error'] = ($_ | Out-String) }
        finally { FFinish 'h33_' $fComplete }
        $fComplete = $false
        try {
            FBegin 'h34'
            # Do not change the physical topology to manufacture coverage. Resolve
            # targets from the topology actually reported by this owned daemon.
            function F34Workspace($state, [string]$output, [string]$name) {
                return (@($state.workspaces | Where-Object { $_.output -ceq $output -and $_.name -ceq $name }) | Select-Object -First 1)
            }
            function F34Output($state) {
                $focused = @($state.monitors | Where-Object { $_.focused })
                if ($focused.Count -ne 1) { throw 'Expected exactly one focused output' }
                return [string]$focused[0].name
            }
            function F34Topology($state) {
                return (@($state.monitors | Sort-Object name | ForEach-Object {
                    '{0}|{1}|{2},{3},{4},{5}|{6},{7},{8},{9}|{10}' -f $_.output_id,$_.name,$_.bounds_x,$_.bounds_y,$_.bounds_width,$_.bounds_height,$_.work_area_x,$_.work_area_y,$_.work_area_width,$_.work_area_height,$_.scale_factor
                }) -join ';')
            }
            function F34Places($state, [bool]$foreignOnly = $false) {
                return (@($state.windows | Sort-Object id | ForEach-Object {
                    $window = $_
                    if (-not $foreignOnly -or $fHandles.ToArray() -notcontains [IntPtr][int64]$window.id) {
                        $entry = WinOf $state.list ([IntPtr][int64]$window.id)
                        '{0}|{1}|{2}|{3}|{4}|{5}|{6}' -f $window.id,$window.workspace_id,$window.output,$entry.column,$entry.tile,$entry.floating,$entry.fullscreen
                    }
                }) -join ';')
            }
            function F34Destination($state, [string]$route) {
                # Cycle in cached output-id order; directions prefer overlapping
                # cross-axis spans, then nearest edges/centres, just as the resolver.
                $ordered = @($state.monitors | Sort-Object { [uint64]$_.output_id })
                $fHomeMonitor = @($ordered | Where-Object { $_.name -ceq $fHome })[0]
                if ($route -eq 'name') {
                    $other = @($ordered | Where-Object { $_.name -cne $fHome })
                    if ($other.Count -gt 0) { return [string]$other[0].name }
                    return $fHome
                }
                $index = 0
                for ($i=0; $i -lt $ordered.Count; $i++) { if ($ordered[$i].name -ceq $fHome) { $index=$i; break } }
                switch ($route) {
                    'previous' { return [string]$ordered[($index+$ordered.Count-1)%$ordered.Count].name }
                    'next' { return [string]$ordered[($index+1)%$ordered.Count].name }
                }
                $ax=[long]$fHomeMonitor.bounds_x; $ay=[long]$fHomeMonitor.bounds_y
                $ar=$ax+[long]$fHomeMonitor.bounds_width; $ab=$ay+[long]$fHomeMonitor.bounds_height
                $candidates = @($ordered | Where-Object { $_.name -cne $fHome } | ForEach-Object {
                    $bx=[long]$_.bounds_x; $by=[long]$_.bounds_y
                    $br=$bx+[long]$_.bounds_width; $bb=$by+[long]$_.bounds_height
                    $horizontal=($route -eq 'left' -or $route -eq 'right')
                    $ahead=switch ($route) {
                        'left' { $bx+$br -lt $ax+$ar }; 'right' { $bx+$br -gt $ax+$ar }
                        'up' { $by+$bb -lt $ay+$ab }; 'down' { $by+$bb -gt $ay+$ab }
                    }
                    if ($ahead) {
                        $gap=switch ($route) {
                            'left' { [Math]::Max(0,$ax-$br) }; 'right' { [Math]::Max(0,$bx-$ar) }
                            'up' { [Math]::Max(0,$ay-$bb) }; 'down' { [Math]::Max(0,$by-$ab) }
                        }
                        $crossGap=if ($horizontal) { [Math]::Max(0,[Math]::Max($ay-$bb,$by-$ab)) } else { [Math]::Max(0,[Math]::Max($ax-$br,$bx-$ar)) }
                        $crossCenter=if ($horizontal) { [Math]::Abs($by+$bb-$ay-$ab) } else { [Math]::Abs($bx+$br-$ax-$ar) }
                        [pscustomobject]@{name=$_.name;off_axis=($crossGap -gt 0);distance=([double]$gap*$gap+[double]$crossGap*$crossGap);cross_center=$crossCenter;id=[uint64]$_.output_id}
                    }
                } | Sort-Object off_axis,distance,cross_center,id)
                if ($candidates.Count -gt 0) { return [string]$candidates[0].name }
                return $fHome
            }
            function F34Prepare {
                # All remote destinations are owned and empty. Never append a tile
                # to the active desktop workspace of a pre-existing application.
                $state = FState 'lane-f-h34-prepare'
                foreach ($output in @($state.monitors | ForEach-Object { $_.name })) {
                    if ($output -ceq $fHome) { continue }
                    $target = F34Workspace $state $output $fTarget
                    if ($null -eq $target -or @(FMembers $state $target.id).Count -ne 0) { throw "Remote owned target is not empty: $output" }
                    FQuick 'focus-monitor' @($output)
                    FQuick 'focus-workspace' @($fTarget)
                }
                FQuick 'focus-monitor' @($fHome)
                return (FFixture)
            }
            function F34Return([string]$movingWorkspace = '') {
                if ($movingWorkspace.Length -gt 0) {
                    $state = FState 'lane-f-h34-return-workspace'
                    $workspace = @($state.workspaces | Where-Object { $_.name -ceq $movingWorkspace })
                    if ($workspace.Count -ne 1) { throw 'Lost the uniquely named owned workspace' }
                    $members = @(FMembers $state $workspace[0].id)
                    if (-not (FSameMembers $members @([int64]$fA,[int64]$fB,[int64]$fC,[int64]$fD))) { throw 'Refusing to restore a workspace with unexpected membership' }
                    FQuick 'focus-monitor' @($workspace[0].output)
                    FQuick 'move-workspace-to-monitor' @($fHome,'--workspace',$movingWorkspace) 'raw'
                    FQuick 'focus-monitor' @($fHome)
                    FQuick 'set-workspace-name' @($fSource,'--workspace',$movingWorkspace) 'raw'
                } else {
                    foreach ($handle in $fHandles.ToArray()) {
                        $state = FState 'lane-f-h34-return-window'
                        $window = FWindow $state $handle
                        $workspace = @($state.workspaces | Where-Object { [uint64]$_.id -eq [uint64]$window.workspace_id })
                        if ($workspace.Count -ne 1) { throw 'Owned monitor transfer lost its workspace' }
                        if ($workspace[0].output -cne $fHome) {
                            FFocus $handle
                            FQuick 'move-window-to-monitor' @($fHome,'--id',[int64]$handle) 'raw'
                        }
                    }
                }
                FQuick 'focus-monitor' @($fHome)
            }
            $fState = F34Prepare
            $fMonitorTopology = F34Topology $fState
            foreach ($fTransport in @('cli','raw')) {
                foreach ($fFamily in @('focus-monitor','move-window-to-monitor','move-column-to-monitor','move-workspace-to-monitor')) {
                    foreach ($fRoute in @('left','right','up','down','previous','next','name')) {
                        $fState = F34Prepare
                        $fDestination = F34Destination $fState $fRoute
                        $fAway = ($fDestination -cne $fHome)
                        $fAction = if ($fRoute -eq 'name') { $fFamily } else { "$fFamily-$fRoute" }
                        $fArgs = @()
                        if ($fRoute -eq 'name') { $fArgs = @($fDestination) }
                        $fMovingName = ''
                        $fMoving = @()
                        FFocus $fA; FFocus $fB
                        $fStart = $fB
                        $fExpected = $fB
                        $fExpectedOutput = $fHome
                        $fSourceWorkspace = FWorkspace $fState $fSource
                        $fSourceId = [uint64]$fSourceWorkspace.id
                        $fDestinationWorkspace = F34Workspace $fState $fDestination $fTarget
                        $fDestinationId = [uint64]$fDestinationWorkspace.id
                        switch ($fFamily) {
                            'focus-monitor' {
                                if ($fAway) { $fExpected=[IntPtr]::Zero; $fExpectedOutput=$fDestination }
                            }
                            'move-window-to-monitor' {
                                # Alternate foreground and inactive numeric targets.
                                # Only the selected/foreground window follows its move.
                                if ($fRoute -in @('left','up','previous','name')) {
                                    $fMoving=@($fD); $fArgs += @('--id',[int64]$fD)
                                } else {
                                    $fMoving=@($fB)
                                    if ($fAway) { $fExpectedOutput=$fDestination }
                                }
                            }
                            'move-column-to-monitor' {
                                $fMoving=@($fA,$fB)
                                if ($fAway) { $fExpectedOutput=$fDestination }
                            }
                            'move-workspace-to-monitor' {
                                $fMoving=@($fA,$fB,$fC,$fD)
                                # Configured names exist on every monitor. A unique
                                # temporary name avoids any destination-name collision.
                                $fMovingName="lane-f-moving-$fTransport-$fRoute"
                                FQuick 'set-workspace-name' @($fMovingName) 'raw'
                                if ($fRoute -in @('up','down','name')) { $fArgs += @('--workspace',$fMovingName) }
                                elseif ($fRoute -in @('previous','next')) { $fArgs += @('--workspace',[int]$fSourceWorkspace.idx) }
                                if ($fAway) { $fExpectedOutput=$fDestination }
                            }
                        }
                        $fRecord = FRun "h34-$fFamily-$fRoute-$fTransport" $fTransport $fAction $fArgs ([int64]$fExpected)
                        # Compare each action with its own precondition, not a phase
                        # baseline that can outlive unrelated application windows.
                        $fValid = ($fRecord.pass -and (F34Output $fRecord.after) -ceq $fExpectedOutput -and
                            (F34Topology $fRecord.after) -ceq $fMonitorTopology -and
                            (F34Places $fRecord.before $true) -ceq (F34Places $fRecord.after $true))
                        if ($fFamily -eq 'focus-monitor' -or -not $fAway) {
                            $fValid = ($fValid -and (F34Places $fRecord.before) -ceq (F34Places $fRecord.after))
                        } elseif ($fFamily -eq 'move-workspace-to-monitor') {
                            $fMovedWorkspace = @($fRecord.after.workspaces | Where-Object { [uint64]$_.id -eq $fSourceId })
                            $fValid = ($fValid -and $fMovedWorkspace.Count -eq 1 -and $fMovedWorkspace[0].output -ceq $fDestination -and
                                $fMovedWorkspace[0].name -ceq $fMovingName -and $fMovedWorkspace[0].is_active -and
                                (FSameMembers (FMembers $fRecord.after $fSourceId) @([int64]$fA,[int64]$fB,[int64]$fC,[int64]$fD)))
                            foreach ($fHandle in $fMoving) {
                                $fBeforeEntry=WinOf $fRecord.before.list $fHandle; $fAfterEntry=WinOf $fRecord.after.list $fHandle
                                $fValid=($fValid -and [int]$fBeforeEntry.column -eq [int]$fAfterEntry.column -and [int]$fBeforeEntry.tile -eq [int]$fAfterEntry.tile)
                            }
                        } else {
                            $fMovingIds=@($fMoving | ForEach-Object { [int64]$_ })
                            $fRemaining=@($fHandles.ToArray() | ForEach-Object { [int64]$_ } | Where-Object { $fMovingIds -notcontains $_ })
                            $fValid=($fValid -and (FSameMembers (FMembers $fRecord.after $fSourceId) $fRemaining) -and
                                (FSameMembers (FMembers $fRecord.after $fDestinationId) $fMovingIds))
                            for ($fTile=0; $fTile -lt $fMoving.Count; $fTile++) {
                                $fEntry=WinOf $fRecord.after.list $fMoving[$fTile]
                                $fWindow=FWindow $fRecord.after $fMoving[$fTile]
                                $fValid=($fValid -and $null -ne $fEntry -and -not $fEntry.floating -and
                                    [uint64]$fWindow.workspace_id -eq $fDestinationId -and [int]$fEntry.column -eq 0 -and [int]$fEntry.tile -eq $fTile)
                            }
                        }
                        $fGate=switch ($fFamily) {
                            'focus-monitor' {'h34_focus_monitor_targets'}
                            'move-window-to-monitor' {'h34_window_monitor_targets'}
                            'move-column-to-monitor' {'h34_column_monitor_targets'}
                            'move-workspace-to-monitor' {'h34_workspace_monitor_targets'}
                        }
                        FCheck $fGate $fValid @{record=$fRecord;route=$fRoute;destination=$fDestination;expected_output=$fExpectedOutput;moving_workspace_id=$fSourceId}
                        FCheck 'h34_display_topology_unchanged' ((F34Topology $fRecord.after) -ceq $fMonitorTopology) @{before=$fMonitorTopology;after=(F34Topology $fRecord.after)}
                        if ($fState.monitors.Count -eq 1) {
                            FCheck 'h34_single_display_actions_are_harmless' ($fValid -and (F34Places $fRecord.before) -ceq (F34Places $fRecord.after)) $fRecord
                        }
                        F34Return $fMovingName
                    }
                }
                # Also exercise a self-named target on multi-display desktops. This
                # is a real no-op assertion, not a passing single-display skip.
                foreach ($fFamily in @('focus-monitor','move-window-to-monitor','move-column-to-monitor','move-workspace-to-monitor')) {
                    $null=F34Prepare
                    FFocus $fB
                    $fRecord=FRun "h34-self-$fFamily-$fTransport" $fTransport $fFamily @($fHome) ([int64]$fB)
                    FCheck 'h34_single_display_actions_are_harmless' ($fRecord.pass -and (F34Output $fRecord.after) -ceq $fHome -and (F34Places $fRecord.before) -ceq (F34Places $fRecord.after)) $fRecord
                }
                $null=F34Prepare
                $fCompositeCases=@(
                    @{action='focus-column-or-monitor-left';route='left';start=$fC;tile=$fA;edge=$fA},
                    @{action='focus-column-or-monitor-right';route='right';start=$fA;tile=$fC;edge=$fC},
                    @{action='focus-window-or-monitor-up';route='up';start=$fB;tile=$fA;edge=$fA},
                    @{action='focus-window-or-monitor-down';route='down';start=$fA;tile=$fB;edge=$fB}
                )
                foreach ($fCase in $fCompositeCases) {
                    FFocus $fA; FFocus $fC
                    FFocus $fCase.start
                    $fRecord=FRun "h34-tile-first-$($fCase.route)-$fTransport" $fTransport $fCase.action @() ([int64]$fCase.tile)
                    FCheck 'h34_composite_monitor_fallbacks' ($fRecord.pass -and (F34Output $fRecord.after) -ceq $fHome -and (F34Places $fRecord.before) -ceq (F34Places $fRecord.after)) $fRecord
                    FFocus $fCase.edge
                    $fState=FState 'lane-f-h34-composite-edge'
                    $fDestination=F34Destination $fState $fCase.route
                    $fExpected=if ($fDestination -ceq $fHome) { $fCase.edge } else { [IntPtr]::Zero }
                    $fRecord=FRun "h34-edge-fallback-$($fCase.route)-$fTransport" $fTransport $fCase.action @() ([int64]$fExpected)
                    FCheck 'h34_composite_monitor_fallbacks' ($fRecord.pass -and (F34Output $fRecord.after) -ceq $fDestination -and (F34Places $fRecord.before) -ceq (F34Places $fRecord.after)) $fRecord
                }
                FCheck 'h34_cli_and_typed_run_action' $true @{transport=$fTransport;families=4;targets=7;numeric_owned_hwnd=$true;numeric_workspace_index=$true}
            }
            $fTopologyAfter=@([Windows.Forms.Screen]::AllScreens | ForEach-Object { '{0}:{1}:{2}:{3}' -f $_.DeviceName,$_.Bounds,$_.WorkingArea,$_.Primary } | Sort-Object)
            FCheck 'h34_display_topology_unchanged' (($fTopologyBefore -join ';') -ceq ($fTopologyAfter -join ';')) @{before=$fTopologyBefore;after=$fTopologyAfter}
            $fComplete=$true
        } catch { $fNotes['h34-error']=($_ | Out-String) }
        finally { FFinish 'h34_' $fComplete }

        # H31 runs last: it deliberately closes one of the four owned windows.
        # Managed focus follows the native foreground event, and the e7ced50 daemon logs put that
        # at 6-70 ms (p50 42, p90 63, p99 150) after the IPC focus reply, so a 40-50 ms wait is a
        # coin flip: FFocus $fC 50 threw and took the whole group (17 passing checks) with it.
        # FFocus only waits for the fixture to settle; the 750 ms visit bounds below are still
        # asserted from their own stopwatch, so a slow settle fails the gate instead of passing it.
        $fVisitWait=200
        $fComplete=$false
        try {
            foreach ($fTransport in @('cli','raw')) {
                FBegin "h31-$fTransport"
                $null=FFixture
                # Fixture setup necessarily visits B; discard that history before
                # the debounce oracle so an older settled B cannot qualify it.
                # FFixture ends with A foreground; only A settles at this restart.
                FBegin "h31-$fTransport-clean-history"
                # A focus timeout is not a dwell: hold A for a measured 1150ms
                # after both managed and native focus agree, before visiting B/C.
                $fSettledA=FHoldFocus $fA 1150
                $fVisitClock=[Diagnostics.Stopwatch]::StartNew()
                FFocus $fB $fVisitWait
                $fBConfirmed=$fVisitClock.ElapsedMilliseconds
                FFocus $fC $fVisitWait
                $fBEnd=$fVisitClock.ElapsedMilliseconds
                $fRecord=FRun "h31-exclude-short-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fA) -shortVisitClock $fVisitClock
                FCheck 'h31_short_focus_is_excluded_at_750ms' ($fBEnd -lt 750 -and $fRecord.short_visit_until_dispatch_upper_bound_ms -lt 750 -and (FFocusIs $fRecord.before ([int64]$fC)) -and $fRecord.pass) @{record=$fRecord;short_visit_upper_bound_ms=$fBEnd;b_confirmed_ms=$fBConfirmed;c_confirmed_ms=$fBEnd;settled_a_ms=$fSettledA;debounce_ms=750}
                $fTogglePass=$fRecord.pass
                foreach ($fEndpoint in @($fC,$fA,$fC,$fA)) {
                    $fRecord=FRun "h31-toggle-$([int64]$fEndpoint)-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fEndpoint)
                    $fTogglePass=($fTogglePass -and $fRecord.pass)
                    FCheck 'h31_previous_toggles_exact_endpoints' $fRecord.pass $fRecord
                }
                # Seed a settled visit on source, then a short intermediate B and
                # an endpoint on target. Previous must activate the right home.
                FQuick 'move-window-to-workspace' @($fTarget,'--id',[int64]$fD,'--focus',$false) 'raw'
                $fSettledA=FHoldFocus $fA 1150
                $fVisitClock.Restart()
                FFocus $fB $fVisitWait
                $fCrossBConfirmed=$fVisitClock.ElapsedMilliseconds
                FFocus $fD $fVisitWait
                $fCrossDConfirmed=$fVisitClock.ElapsedMilliseconds
                # Dispatch immediately; the product must handle queued native focus
                # observations without a fixture sleep masking that ordering race.
                $fCrossShort=$fVisitClock.ElapsedMilliseconds
                $fRecord=FRun "h31-cross-workspace-back-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fA) -shortVisitClock $fVisitClock
                FCheck 'h31_history_crosses_workspaces' ($fCrossShort -lt 750 -and $fRecord.short_visit_until_dispatch_upper_bound_ms -lt 750 -and (FFocusIs $fRecord.before ([int64]$fD)) -and $fRecord.pass -and (FWorkspace $fRecord.after $fSource).is_active) @{record=$fRecord;short_visit_upper_bound_ms=$fCrossShort;b_confirmed_ms=$fCrossBConfirmed;d_confirmed_ms=$fCrossDConfirmed;focus_event_drain_ms=0;settled_a_ms=$fSettledA;debounce_ms=750}
                $fCrossPass=$fRecord.pass
                $fRecord=FRun "h31-cross-workspace-forward-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fD)
                $fCrossPass=($fCrossPass -and $fRecord.pass -and (FWorkspace $fRecord.after $fTarget).is_active)
                FCheck 'h31_history_crosses_workspaces' $fCrossPass $fRecord
                FCheck 'h31_cli_and_typed_run_action' ($fTogglePass -and $fCrossPass) @{transport=$fTransport;debounce_ms=750;typed_debounce_number=$true}
            }
            # D is now a permanently qualified MRU endpoint. Close it by numeric
            # owned HWND while A is foreground, then query history via BOTH paths.
            FFocus $fA
            $fRecord=FRun 'h31-close-owned-endpoint' 'raw' 'close-window' @('--id',[int64]$fD) ([int64]$fA)
            $fClosed=($fRecord.pass -and -not [LT]::IsWindow($fD) -and $null -eq (FWindow $fRecord.after $fD))
            FCheck 'h31_closed_windows_are_excluded' $fClosed $fRecord
            foreach ($fTransport in @('cli','raw')) {
                $null=FHoldFocus $fA 1150
                FFocus $fC $fVisitWait
                $fRecord=FRun "h31-closed-excluded-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fA)
                FCheck 'h31_closed_windows_are_excluded' ($fClosed -and $fRecord.pass -and $null -eq (FWindow $fRecord.after $fD)) $fRecord
                $fRecord=FRun "h31-closed-toggle-$fTransport" $fTransport 'focus-window-previous' @('--debounce-ms',750) ([int64]$fC)
                FCheck 'h31_closed_windows_are_excluded' ($fClosed -and $fRecord.pass -and -not [LT]::IsWindow($fD)) $fRecord
            }
            $fComplete=$true
        } catch { $fNotes['h31-error']=($_ | Out-String) }
        finally { FFinish 'h31_' $fComplete }
    } catch {
        $fNotes['fatal'] = ($_ | Out-String)
    } finally {
        $fCleanup = $true
        try { FStopSubscriber } catch { $fCleanup=$false; $fNotes['subscriber-cleanup-error']=[string]$_ }
        foreach ($client in $fClients.ToArray()) {
            try { if (-not $client.HasExited) { $client.Kill(); if (-not $client.WaitForExit(5000)) { throw 'Subscriber did not exit' } } }
            catch { $fCleanup=$false; $fNotes["client-cleanup-$($client.Id)"]=[string]$_ }
        }
        foreach ($process in $fOwned.ToArray()) {
            try { FStop $process 'finally' } catch { $fCleanup=$false; $fNotes["daemon-cleanup-$($process.Id)"]=[string]$_ }
        }
        foreach ($handle in $fHandles.ToArray()) {
            try { if ([LT]::IsWindow($handle)) { [TestForms]::CloseForm($handle) } }
            catch { $fCleanup=$false; $fNotes["window-cleanup-$handle"]=[string]$_ }
        }
        foreach ($fKey in $fSavedEnvironment.Keys) {
            [Environment]::SetEnvironmentVariable($fKey,$fSavedEnvironment[$fKey],'Process')
        }
        $summary['h30_f_owned_process_cleanup'] = $fCleanup
        [ordered]@{ pipe=$fPipeName; owned_hwnds=@($fHandles.ToArray() | ForEach-Object { [int64]$_ }); checks=$fChecks.ToArray(); notes=$fNotes } |
            ConvertTo-Json -Depth 16 | Set-Content (Join-Path $fDir 'lane-f-values.json')
    }
}
