# H85: real 60-second startup rule phase. Dot-source only under live-test.ps1's
# desktop mutex. Requires its LT, Rc, Q, and LaneHConnection helpers.
# Private config/profile/pipe; no product clock overrides or harness wiring.
& {
    $h85Root = Join-Path $D 'lane-startup-rules'
    $h85Dir = Join-Path $h85Root ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $h85Dir
    $h85Gates = @('h85_initial_scan_startup_rules', 'h85_new_admission_startup_rules',
        'h85_title_change_keeps_startup_rules', 'h85_reload_keeps_startup_rules',
        'h85_real_deadline_expires_without_reload_reset', 'h85_expiry_updates_existing_limits_and_opacity',
        'h85_expiry_does_not_replay_open_properties', 'h85_expiry_remains_consumed',
        'h85_post_expiry_admission_uses_runtime_rules', 'h85_cleanup')
    $h85Details = [ordered]@{ fixture_directory=$h85Dir; checks=[ordered]@{};
        cleanup_errors=@(); scope='Real single-output startup timeout, native alpha/visible width and IPC allocation; no fake clock' }
    foreach ($gate in $h85Gates) {
        $summary[$gate] = $false
        $h85Details.checks[$gate] = [ordered]@{ pass=$false; status='not reached' }
    }
    $h85State = @{ process=$null; rpc=$null; outTask=$null; errTask=$null; launch_ms=0.0; ready_ms=0.0 }
    $h85Forms = New-Object 'Collections.Generic.List[IntPtr]'
    $h85NativeReady = $false
    $h85Pipe = '\\.\pipe\wiri_live_h85_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    $h85Cfg = Join-Path $h85Dir 'config.kdl'
    $h85Utf8 = New-Object Text.UTF8Encoding($false)
    $h85Prefix = 'h85-' + [Guid]::NewGuid().ToString('N')
    function H85Now { return [double][Diagnostics.Stopwatch]::GetTimestamp() * 1000.0 / [Diagnostics.Stopwatch]::Frequency }
    function H85Evidence {
        [IO.File]::WriteAllText((Join-Path $h85Root 'evidence.json'), ($h85Details | ConvertTo-Json -Depth 20), $h85Utf8)
    }
    function H85Gate([string]$name, [bool]$pass, $observation) {
        $summary[$name] = $pass
        $h85Details.checks[$name] = [ordered]@{ pass=$pass; status='checked'; observed=$observation }
        H85Evidence
    }
    function H85Request([string]$tag, $request) {
        if ($null -eq $h85State.process -or $h85State.process.HasExited -or $null -eq $h85State.rpc) {
            throw 'H85 owned daemon/verified pipe is unavailable'
        }
        $h85State.rpc.Send(($request | ConvertTo-Json -Depth 8 -Compress) + "`n")
        $line = $h85State.rpc.ReadLine(4000)
        if ($null -eq $line) { throw "H85 request timed out: $tag" }
        $reply = $line | ConvertFrom-Json
        [ordered]@{ tag=$tag; elapsed_ms=((H85Now)-$h85State.launch_ms); request=$request; reply=$reply } |
            ConvertTo-Json -Depth 12 -Compress | Add-Content -Encoding UTF8 (Join-Path $h85Dir 'wire.ndjson')
        if ($reply.success -ne $true) { throw "H85 request rejected: $tag $line" }
        return $reply
    }
    function H85Observe([IntPtr]$h) {
        if ($h -eq [IntPtr]::Zero -or [LT]::Pid($h) -ne $PID) { throw 'H85 refuses an unowned fixture' }
        $reply = H85Request 'windows' @{type='windows'}
        $entries = @($reply.result.windows | Where-Object { [int64]$_.id -eq $h.ToInt64() })
        if ($entries.Count -ne 1) { throw "H85 expected one managed fixture: $h" }
        $dpi = [OwnedLiveForms]::Dpi($h)
        if ($dpi -eq 0) { throw 'H85 fixture DPI is unavailable' }
        return [pscustomobject]@{ hwnd=$h.ToInt64(); elapsed_ms=((H85Now)-$h85State.launch_ms);
            alpha=[LT]::Alpha($h); rect=(Rc $h); scale=([double]$dpi/96.0); engine=$entries[0];
            foreground=[LT]::GetForegroundWindow().ToInt64() }
    }
    function H85Matches($observation, [int]$alpha, [int]$physicalWidth, [bool]$floating=$false) {
        $window = $observation.engine
        return ([Math]::Abs($observation.alpha-$alpha) -le 1 -and
            [Math]::Abs($observation.rect.W-$physicalWidth) -le 3 -and
            [Math]::Abs($window.layout.window_size[0]*$observation.scale-$physicalWidth) -le 3 -and
            [Math]::Abs($window.layout.tile_size[0]*$observation.scale-$physicalWidth) -le 3 -and
            [bool]$window.is_floating -eq $floating)
    }
    function H85Await([IntPtr]$h, [int]$alpha, [int]$width, [bool]$floating=$false, [int]$budget=6000) {
        $until = (H85Now) + $budget
        do {
            $sample = H85Observe $h
            if (H85Matches $sample $alpha $width $floating) { return $sample }
            Start-Sleep -Milliseconds 200
        } while ((H85Now) -lt $until)
        $h85Details['last_mismatch'] = $sample
        throw "H85 native/IPC state did not reach alpha=$alpha width=$width floating=$floating"
    }
    function H85WaitUntil([double]$deadline) {
        while ((H85Now) -lt $deadline) {
            if ($h85State.process.HasExited) { throw 'H85 daemon exited during the real startup interval' }
            $remaining = $deadline - (H85Now)
            if ($remaining -gt 0) { Start-Sleep -Milliseconds ([int][Math]::Min(250, [Math]::Ceiling($remaining))) }
        }
    }
    function H85Config([int]$width, [string]$opacity) {
        return @"
input { focus-follows-mouse false; }
animations { off; }
binds {}
hotkey-overlay { skip-at-startup; }
layout { gaps 10; center-focused-column "never"; strip-frame false; focus-ring { off; }; border { off; }; shadow { off; }; }
window-rule { match title="^$h85Prefix-"; open-floating false; default-column-width { fixed 600; }; }
window-rule { match title="^$h85Prefix-" at-startup=true; opacity $opacity; min-width $width; max-width $width; }
window-rule { match title="^$h85Prefix-" at-startup=false; opacity 0.8; min-width 240; max-width 240; open-floating true; }
"@
    }
    function H85Open([string]$suffix) {
        $h = [OwnedLiveForms]::Open(($h85Prefix+'-'+$suffix), 70, 130, 180)
        if ($h -eq [IntPtr]::Zero -or [LT]::Pid($h) -ne $PID) { throw 'H85 form did not open as an owned HWND' }
        $h85Forms.Add($h)
        return $h
    }
    try {
        if (-not $ownsLiveLock) { throw 'H85 requires the root desktop mutex' }
        foreach ($type in @('LT','LaneHConnection')) {
            if (-not ($type -as [type])) { throw "H85 missing harness type $type" }
        }
        foreach ($helper in @('Rc','Q')) {
            if ($null -eq (Get-Command $helper -ErrorAction SilentlyContinue)) { throw "H85 missing helper $helper" }
        }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'H85 refuses an existing wiri process' }
        . (Join-Path (Split-Path $PSScriptRoot -Parent) 'live-fixtures\owned-forms.ps1')
        $h85NativeReady = $true
        $initial = H85Open 'initial'
        $h85Details['initial_before_daemon'] = @{ hwnd=$initial.ToInt64(); native=(Rc $initial); alpha=[LT]::Alpha($initial) }
        [IO.File]::WriteAllText($h85Cfg, (H85Config 360 '0.5'), $h85Utf8)
        $roaming = Join-Path $h85Dir 'roaming'; $local = Join-Path $h85Dir 'local'
        $null = New-Item -ItemType Directory -Force -Path $roaming,$local
        $psi = New-Object Diagnostics.ProcessStartInfo
        $psi.FileName=$wiri; $psi.Arguments='-v --no-autostart --no-startup-overlay -c '+(Q $h85Cfg)
        $psi.UseShellExecute=$false; $psi.CreateNoWindow=$true
        $psi.RedirectStandardOutput=$true; $psi.RedirectStandardError=$true
        $psi.WorkingDirectory=$h85Dir
        $psi.EnvironmentVariables['APPDATA']=$roaming; $psi.EnvironmentVariables['LOCALAPPDATA']=$local
        $psi.EnvironmentVariables['WIRI_PIPE']=$h85Pipe
        $psi.EnvironmentVariables['WIRI_KEYBOARD_DRY_RUN']='1'
        $psi.EnvironmentVariables['WIRI_SYSTEM_ACTIONS_DRY_RUN']=Join-Path $h85Dir 'system-actions.ndjson'
        $p = New-Object Diagnostics.Process; $p.StartInfo=$psi
        $h85State.launch_ms=H85Now
        if (-not $p.Start()) { $p.Dispose(); throw 'H85 private daemon did not start' }
        $h85State.process=$p; $null=$p.Handle
        $h85State.outTask=$p.StandardOutput.ReadToEndAsync(); $h85State.errTask=$p.StandardError.ReadToEndAsync()
        $h85Details['daemon'] = @{ pid=$p.Id; start_utc=$p.StartTime.ToUniversalTime().ToString('o'); pipe=$h85Pipe; config=$h85Cfg }
        $readyUntil=$h85State.launch_ms+15000
        do {
            if ($p.HasExited) { throw 'H85 private daemon exited during readiness' }
            try { $h85State.rpc=New-Object LaneHConnection $h85Pipe,$p.Id; break }
            catch { $h85Details['last_connect_error']=[string]$_; Start-Sleep -Milliseconds 100 }
        } while ((H85Now) -lt $readyUntil)
        if ($null -eq $h85State.rpc) { throw 'H85 verified private pipe did not become ready' }
        $null=H85Request 'ready-windows' @{type='windows'}
        $h85State.ready_ms=H85Now
        $h85Details['clock_bracket'] = @{ launch_ms=$h85State.launch_ms; ready_ms=$h85State.ready_ms;
            reason='Engine creation lies between process launch and a successful windows IPC query';
            earliest_expiry_ms=($h85State.launch_ms+60000); latest_observation_ms=($h85State.ready_ms+65000) }
        $first=H85Await $initial 128 360
        H85Gate 'h85_initial_scan_startup_rules' ($first.elapsed_ms -lt 25000) $first
        $createdMs=H85Now; $ordinary=H85Open 'new'
        $new=H85Await $ordinary 128 360
        H85Gate 'h85_new_admission_startup_rules' ($createdMs -gt $h85State.ready_ms -and $new.elapsed_ms -lt 25000) $new
        [OwnedLiveForms]::SetTitle($ordinary, ($h85Prefix+'-new-renamed'))
        $titleUntil=(H85Now)+4000
        do {
            $renamed=H85Observe $ordinary
            if ($renamed.engine.title -ceq ($h85Prefix+'-new-renamed')) { break }
            Start-Sleep -Milliseconds 100
        } while ((H85Now) -lt $titleUntil)
        if ($renamed.engine.title -cne ($h85Prefix+'-new-renamed')) { throw 'H85 title notification did not reach the engine' }
        $renamed=H85Await $ordinary 128 360
        H85Gate 'h85_title_change_keeps_startup_rules' ($renamed.engine.title -ceq ($h85Prefix+'-new-renamed')) $renamed

        H85WaitUntil ($h85State.launch_ms+25000)
        $reloadMs=H85Now
        [IO.File]::WriteAllText($h85Cfg, (H85Config 400 '0.6'), $h85Utf8)
        $null=H85Request 'reload-before-expiry' @{type='reload_config'}
        $reloadInitial=H85Await $initial 153 400; $reloadNew=H85Await $ordinary 153 400
        H85Gate 'h85_reload_keeps_startup_rules' ($reloadNew.elapsed_ms -lt 50000) @{ initial=$reloadInitial; ordinary=$reloadNew; reload_elapsed_ms=($reloadMs-$h85State.launch_ms) }
        H85WaitUntil ($h85State.launch_ms+50000)
        $preInitial=H85Await $initial 153 400; $preNew=H85Await $ordinary 153 400
        $null=H85Request 'focus-owned-before-expiry' @{type='focus_window'; data=@{window_hwnd=$ordinary.ToInt64()}}
        $focusUntil=(H85Now)+3000
        while ([LT]::GetForegroundWindow() -ne $ordinary -and (H85Now) -lt $focusUntil) { Start-Sleep -Milliseconds 100 }
        if ([LT]::GetForegroundWindow() -ne $ordinary) { throw 'H85 owned foreground did not settle' }
        $preNew=H85Observe $ordinary
        $h85Details['before_expiry']=@($preInitial,$preNew)
        $expiryUntil=$h85State.ready_ms+65000
        $expired=$false
        do {
            $afterInitial=H85Observe $initial; $afterNew=H85Observe $ordinary
            if ((H85Matches $afterInitial 204 240) -and (H85Matches $afterNew 204 240)) { $expired=$true; break }
            Start-Sleep -Milliseconds 250
        } while ((H85Now) -lt $expiryUntil)
        $expiryMs=H85Now
        H85Gate 'h85_real_deadline_expires_without_reload_reset' ($expired -and
            $expiryMs -ge ($h85State.launch_ms+60000) -and $expiryMs -le $expiryUntil -and
            $expiryMs -lt ($reloadMs+60000)) @{ elapsed_ms=($expiryMs-$h85State.launch_ms);
                ready_elapsed_ms=($h85State.ready_ms-$h85State.launch_ms); reload_elapsed_ms=($reloadMs-$h85State.launch_ms); initial=$afterInitial; ordinary=$afterNew }
        H85Gate 'h85_expiry_updates_existing_limits_and_opacity' $expired @($afterInitial,$afterNew)
        if (-not $expired) { throw 'H85 actual startup timeout failed to update both owned windows within the fixed budget' }
        H85Gate 'h85_expiry_does_not_replay_open_properties' (
            -not $afterInitial.engine.is_floating -and -not $afterNew.engine.is_floating -and
            $afterInitial.engine.workspace_id -eq $preInitial.engine.workspace_id -and
            $afterNew.engine.workspace_id -eq $preNew.engine.workspace_id -and
            $afterNew.foreground -eq $ordinary.ToInt64()) @{ before=@($preInitial,$preNew); after=@($afterInitial,$afterNew) }
        H85WaitUntil ($expiryMs+3000)
        $settledInitial=H85Await $initial 204 240; $settledNew=H85Await $ordinary 204 240
        H85Gate 'h85_expiry_remains_consumed' ((H85Matches $settledInitial 204 240) -and
            (H85Matches $settledNew 204 240)) @($settledInitial,$settledNew)
        $late=H85Open 'late'; $lateState=H85Await $late 204 240 $true
        H85Gate 'h85_post_expiry_admission_uses_runtime_rules' (H85Matches $lateState 204 240 $true) $lateState
    } catch {
        $h85Details['error']=($_ | Out-String).Trim()
        $h85Details['error_location']=[string]$_.InvocationInfo.PositionMessage
    } finally {
        if ($h85NativeReady) {
            try { [OwnedLiveForms]::ClosePrefix($h85Prefix+'-') }
            catch { $h85Details.cleanup_errors += 'owned opening cancellation: '+[string]$_ }
        }
        $closeUntil=(H85Now)+4000
        $allClosed = -not $h85NativeReady
        do {
            $open=@($h85Forms | Where-Object { [LT]::Pid($_) -eq $PID })
            if ($h85NativeReady) { $allClosed = [OwnedLiveForms]::AllClosedForPrefix($h85Prefix+'-') }
            if ($open.Count -eq 0 -and $allClosed) { break }
            Start-Sleep -Milliseconds 100
        } while ((H85Now) -lt $closeUntil)
        if ($open.Count -ne 0 -or -not $allClosed) { $h85Details.cleanup_errors += 'Owned fixture HWNDs or registered UI threads remain open' }
        $h85Details['opening_threads_finished']=$allClosed
        if ($h85NativeReady) { $h85Details['pending_openings']=[OwnedLiveForms]::PendingForPrefix($h85Prefix+'-') }
        $p=$h85State.process
        if ($null -ne $p) {
            try {
                if ($p.HasExited) { $h85Details.cleanup_errors += 'Owned daemon exited before cleanup' }
                else {
                    try { $null=H85Request 'quit-owned' @{type='quit'} } catch { $h85Details['quit_reply_error']=[string]$_ }
                    if (-not $p.WaitForExit(8000)) {
                        $h85Details.cleanup_errors += 'Graceful shutdown timed out; stopped retained owned process'
                        $p.Kill(); if (-not $p.WaitForExit(5000)) { throw 'Retained owned process did not exit' }
                    }
                }
                if ($p.HasExited -and $p.ExitCode -ne 0) { $h85Details.cleanup_errors += 'Owned daemon exited nonzero: '+$p.ExitCode }
            } catch { $h85Details.cleanup_errors += 'owned daemon cleanup: '+[string]$_ }
        }
        if ($null -ne $h85State.rpc) { try { $h85State.rpc.Dispose() } catch { $h85Details.cleanup_errors += [string]$_ } }
        foreach ($stream in @(@('stdout','outTask'),@('stderr','errTask'))) {
            $task=$h85State[$stream[1]]
            if ($null -ne $task) {
                try {
                    if (-not $task.Wait(2500)) { throw 'Owned log stream did not drain' }
                    [IO.File]::WriteAllText((Join-Path $h85Dir ($stream[0]+'.log')), $task.GetAwaiter().GetResult(), $h85Utf8)
                } catch { $h85Details.cleanup_errors += [string]$_ }
            }
        }
        $journal=Join-Path $h85Dir 'local\wiri\hidden.json'
        $h85Details['recovery_journal_remaining']=Test-Path -LiteralPath $journal
        if ($h85Details.recovery_journal_remaining) {
            $h85Details.cleanup_errors += 'Private recovery journal remains; preserve '+$journal
        }
        H85Gate 'h85_cleanup' ($null -ne $p -and $p.HasExited -and $h85Details.cleanup_errors.Count -eq 0 -and
            $open.Count -eq 0 -and $allClosed) @{
            errors=$h85Details.cleanup_errors; remaining_owned_forms=$open.Count; opening_threads_finished=$allClosed; daemon_started=($null -ne $p) }
        $summary['lane_startup_rules_details']=$h85Details
        H85Evidence
    }
}
