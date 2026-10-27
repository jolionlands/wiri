# H68: keyboard startup/reload evidence only, never native keyboard verification.
# Dot-source under live-test.ps1's desktop lock, after LaneHConnection is loaded.
# Every owned daemon, including restoration/Drop, uses WIRI_KEYBOARD_DRY_RUN=1.
& {
    $kRoot = Join-Path $D 'lane-c2-keyboard'
    $kDir = [IO.Path]::GetFullPath((Join-Path $kRoot ([Guid]::NewGuid().ToString('N'))))
    $kUtf8 = New-Object Text.UTF8Encoding($false)
    $kDetails = [ordered]@{ fixture_directory=$kDir; scope='dry-run logs only; no native keyboard state assertions'; cleanup_errors=(New-Object 'Collections.Generic.List[string]') }
    $kGates = @('h68_unconfigured_startup_no_keyboard_apply', 'h68_explicit_keyboard_reload_dry_run',
        'h68_keyboard_removal_dry_run', 'h68_explicit_keyboard_startup_dry_run', 'h68_owned_cleanup')
    foreach ($gate in $kGates) {
        $summary[$gate] = $false
        $kDetails[$gate] = [ordered]@{ pass=$false; observed='not reached' }
    }
    # Private config and child-only environment: the harness config/environment
    # never need changing or restoring, even if startup or IPC fails.
    $kState = @{ owned=$null; rpc=$null; events=$null; pipe=$null; label=$null; local=$null;
        outTask=$null; errTask=$null; terminated=$true; stopRequested=$false }
    $kCfg = Join-Path $kDir 'config.kdl'
    $kBase = "animations { off; }`ninput { focus-follows-mouse false; }`nbinds {}`nconfig-notification { disable-failed; }`n"
    $kExplicit = "animations { off; }`ninput { focus-follows-mouse false; keyboard { repeat-delay 500; repeat-rate 10; numlock; xkb { layout `"us`"; } } }`nbinds {}`nconfig-notification { disable-failed; }`n"
    $kNative = 'Keyboard option changed \(non-persistent, best-effort restoration\)'
    $kActivity = 'Keyboard repeat-delay approximation|Keyboard repeat-rate approximation|WIRI_KEYBOARD_DRY_RUN|Keyboard option skipped|Configured keyboard layout|Keyboard restore deferred|Keyboard option changed'
    function H68Gate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $kDetails[$name] = [ordered]@{ pass=$pass; observed=$observed }
    }
    function H68Request($request) {
        $kState.rpc.Send(($request | ConvertTo-Json -Depth 6 -Compress) + "`n")
        $line = $kState.rpc.ReadLine(4000)
        if ($null -eq $line) { throw 'H68 owned IPC reply timed out' }
        $reply = $line | ConvertFrom-Json
        [IO.File]::AppendAllText((Join-Path $kDir 'wire.ndjson'), (([ordered]@{ daemon=$kState.label; request=$request; reply=$reply } | ConvertTo-Json -Depth 8 -Compress) + "`n"), $kUtf8)
        return $reply
    }
    function H68ReadEvent([string]$line) {
        [IO.File]::AppendAllText((Join-Path $kDir 'events.ndjson'), ($line + "`n"), $kUtf8)
        return ($line | ConvertFrom-Json)
    }
    function H68Drain {
        $clock = [Diagnostics.Stopwatch]::StartNew(); $last = 0
        do {
            $line = $kState.events.ReadLine(80)
            if ($null -ne $line) { $null = H68ReadEvent $line; $last = $clock.ElapsedMilliseconds }
            if (($clock.ElapsedMilliseconds - $last) -ge 320) { return }
        } while ($clock.ElapsedMilliseconds -lt 3000)
        throw 'H68 config event stream did not become quiet before editing'
    }
    function H68AwaitConfig {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 8000) {
            $line = $kState.events.ReadLine(100)
            if ($null -eq $line) { continue }
            $event = H68ReadEvent $line
            if ($event.type -ceq 'config_loaded' -and $event.data.config_path -ieq $kCfg -and
                $null -ne $event.data.PSObject.Properties['failed'] -and $event.data.failed -eq $false) { return $event }
        }
        throw 'H68 successful config_loaded event for private config timed out'
    }
    function H68Logs {
        # Read the private rotating log while the writer is alive. No shared
        # desktop log and no native keyboard getters/setters are used here.
        $text = ''
        foreach ($name in @('wiri.log.1','wiri.log')) {
            $path = Join-Path $kState.local ('wiri\' + $name)
            if (-not [IO.File]::Exists($path)) { continue }
            $stream = $null; $reader = $null
            try {
                $stream = [IO.File]::Open($path, [IO.FileMode]::Open, [IO.FileAccess]::Read, ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
                $reader = New-Object IO.StreamReader($stream, [Text.Encoding]::UTF8, $true)
                $text += $reader.ReadToEnd()
            } finally {
                if ($null -ne $reader) { $reader.Dispose() }
                elseif ($null -ne $stream) { $stream.Dispose() }
            }
        }
        return $text
    }
    function H68Snapshot([string]$label) {
        $text = H68Logs
        if ([string]::IsNullOrWhiteSpace($text)) { throw 'H68 private daemon log is empty or unavailable' }
        [IO.File]::WriteAllText((Join-Path $kDir ($label + '.log')), $text, $kUtf8)
        return $text
    }
    function H68ExplicitEvidence([string]$text) {
        # Approximation logs are unconditional for these explicit repeat values.
        # Matching native values may produce NO shadow apply/restore log; US may
        # already match or be unavailable. Warnings are evidence, not OS claims.
        $delay = @($text -split "`n" | Where-Object { $_ -match 'Keyboard repeat-delay approximation' })
        $rate = @($text -split "`n" | Where-Object { $_ -match 'Keyboard repeat-rate approximation \(hardware dependent\)' })
        $dryDelay = @($delay | Where-Object { $_ -match 'dry_run[= :]+true' })
        $dryRate = @($rate | Where-Object { $_ -match 'dry_run[= :]+true' })
        return [ordered]@{ pass=($dryDelay.Count -gt 0 -and $dryRate.Count -gt 0 -and $text -notmatch $kNative);
            repeat_delay=$delay; repeat_rate=$rate;
            planned_operations=@($text -split "`n" | Where-Object { $_ -match 'WIRI_KEYBOARD_DRY_RUN' });
            warnings=@($text -split "`n" | Where-Object { $_ -match 'Keyboard option skipped|Configured keyboard layout skipped|Keyboard restore deferred|Keyboard layout restore deferred' });
            note='NumLock/layout and restore logs can be silent when no change is needed; native keyboard state was not tested.' }
    }
    function H68Start([string]$text, [string]$label) {
        if ($null -ne $kState.owned -or -not $kState.terminated) { throw 'H68 previous owned daemon termination is unconfirmed' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'H68 refused an existing unowned wiri process' }
        $session = Join-Path $kDir $label
        $local = Join-Path $session 'local'; $roaming = Join-Path $session 'roaming'; $profile = Join-Path $session 'profile'
        $null = New-Item -ItemType Directory -Force -ErrorAction Stop $local,$roaming,$profile
        [IO.File]::WriteAllText($kCfg, $text, $kUtf8)
        $kState.label = $label; $kState.local = $local
        $kState.pipe = '\\.\pipe\wiri_live_h68_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $psi = New-Object Diagnostics.ProcessStartInfo
        $psi.FileName = $wiri; $psi.Arguments = '-v --no-tray --no-autostart -c ' + (Q $kCfg)
        $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
        $psi.StandardOutputEncoding = [Text.Encoding]::UTF8; $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
        $psi.WorkingDirectory = $session
        $psi.EnvironmentVariables['APPDATA'] = $roaming; $psi.EnvironmentVariables['LOCALAPPDATA'] = $local
        $psi.EnvironmentVariables['USERPROFILE'] = $profile
        $psi.EnvironmentVariables['WIRI_CONFIG'] = $kCfg; $psi.EnvironmentVariables['NIRI_CONFIG'] = $kCfg
        $psi.EnvironmentVariables['WIRI_PIPE'] = $kState.pipe
        $psi.EnvironmentVariables['WIRI_KEYBOARD_DRY_RUN'] = '1'
        $psi.EnvironmentVariables['WIRI_SYSTEM_ACTIONS_DRY_RUN'] = Join-Path $session 'system-actions.ndjson'
        $p = New-Object Diagnostics.Process; $p.StartInfo = $psi
        if (-not $p.Start()) { $p.Dispose(); throw 'H68 private daemon failed to start' }
        $kState.owned = $p; $kState.terminated = $false; $kState.stopRequested = $false
        $null = $p.Handle
        $kState.outTask = $p.StandardOutput.ReadToEndAsync(); $kState.errTask = $p.StandardError.ReadToEndAsync()
        $kDetails['daemon_' + $label] = [ordered]@{ pid=$p.Id; pipe=$kState.pipe; config=$kCfg; local_appdata=$local; keyboard_dry_run='1' }
        $clock = [Diagnostics.Stopwatch]::StartNew(); $lastError = ''
        while ($clock.ElapsedMilliseconds -lt 20000 -and -not $p.HasExited) {
            try { $kState.rpc = New-Object LaneHConnection $kState.pipe,$p.Id; break }
            catch { $lastError = [string]$_; Start-Sleep -Milliseconds 100 }
        }
        if ($null -eq $kState.rpc) { throw "H68 private PID-verified pipe unavailable: $lastError" }
        $kState.events = New-Object LaneHConnection $kState.pipe,$p.Id
        $request = @{type='subscribe_events'; data=@{event_types=@('config_loaded')}}
        $kState.events.Send(($request | ConvertTo-Json -Depth 5 -Compress) + "`n")
        $line = $kState.events.ReadLine(4000)
        if ($null -eq $line) { throw 'H68 event subscription timed out' }
        $ack = $line | ConvertFrom-Json
        if ($ack.success -ne $true) { throw 'H68 event subscription rejected' }
        $null = H68AwaitConfig
        H68Drain # Consume startup replay before editing: never count it as reload.
    }
    function H68Reload([string]$text) {
        H68Drain
        [IO.File]::WriteAllText($kCfg, $text, $kUtf8)
        $ack = H68Request @{type='reload_config'}
        if ($ack.success -ne $true) { throw 'H68 reload request rejected' }
        # The IPC acknowledgement only enqueues; config_loaded follows apply.
        $event = H68AwaitConfig
        H68Drain
        return [ordered]@{ ack=$ack; event=$event }
    }
    function H68Stop {
        foreach ($key in @('events','rpc')) {
            if ($null -ne $kState[$key]) {
                try { $kState[$key].Dispose() } catch { $kDetails.cleanup_errors.Add([string]$_) }
                $kState[$key] = $null
            }
        }
        $p = $kState.owned
        if ($null -eq $p) { return }
        try {
            if (-not $p.HasExited) {
                $kState.stopRequested = $true
                try {
                    # Reverify the server PID before sending quit, even in finally.
                    $quit = New-Object LaneHConnection $kState.pipe,$p.Id
                    try { $quit.Send("{`"type`":`"quit`"}`n") } finally { $quit.Dispose() }
                } catch { $kDetails.cleanup_errors.Add([string]$_) }
                if (-not $p.WaitForExit(2500)) {
                    $kDetails.cleanup_errors.Add('H68 forced stop of retained daemon: ' + $kState.label)
                    $p.Kill()
                    if (-not $p.WaitForExit(5000)) { throw 'H68 owned daemon did not exit after retained-handle Kill' }
                }
            } elseif (-not $kState.stopRequested) {
                $kDetails.cleanup_errors.Add('H68 daemon exited unexpectedly: ' + $kState.label)
            }
            $kState.terminated = $p.HasExited
            $kDetails['exit_' + $kState.label] = $p.ExitCode
            if ($p.ExitCode -ne 0) { $kDetails.cleanup_errors.Add('H68 nonzero daemon exit: ' + $p.ExitCode) }
        } catch { $kDetails.cleanup_errors.Add([string]$_) }
        finally {
            foreach ($stream in @(@('stdout','outTask'),@('stderr','errTask'))) {
                $task = $kState[$stream[1]]
                if ($null -eq $task) { continue }
                try {
                    if (-not $task.Wait(2000)) { throw ('H68 stream drain timed out: ' + $stream[0]) }
                    $text = $task.GetAwaiter().GetResult()
                    [IO.File]::WriteAllText((Join-Path $kDir ($kState.label + '.' + $stream[0])), $text, $kUtf8)
                    if ($text -match $kNative) { $kDetails.cleanup_errors.Add('H68 native keyboard write marker in ' + $stream[0]) }
                    $kState[$stream[1]] = $null
                } catch { $kDetails.cleanup_errors.Add([string]$_) }
            }
            try {
                $text = H68Snapshot ($kState.label + '-after-stop')
                if ($text -match $kNative) { $kDetails.cleanup_errors.Add('H68 native keyboard write marker in private log') }
            } catch { $kDetails.cleanup_errors.Add([string]$_) }
            $kDetails['termination_confirmed_' + $kState.label] = $kState.terminated
            # Preserve the handle on failed shutdown; no subsequent start allowed.
            if ($kState.terminated) { $p.Dispose(); $kState.owned = $null }
        }
    }
    try {
        $null = New-Item -ItemType Directory -Force -ErrorAction Stop $kDir
        if ($null -eq (Get-Command Q -ErrorAction SilentlyContinue)) { throw 'H68 requires harness argument quoting helper Q' }
        if (-not ('LaneHConnection' -as [type])) { throw 'H68 requires existing PID-verifying LaneHConnection' }
        H68Start $kBase 'unconfigured'
        $startup = H68Snapshot 'unconfigured-startup'
        H68Gate 'h68_unconfigured_startup_no_keyboard_apply' ($startup -notmatch $kActivity) ([ordered]@{ log='unconfigured-startup.log'; note='No keyboard apply logs; absence is not a native API trace.' })
        $reload = H68Reload $kExplicit
        $configured = H68Snapshot 'explicit-reload'
        if (-not $configured.StartsWith($startup, [StringComparison]::Ordinal)) { throw 'H68 log rotated during reload; cannot attribute evidence' }
        $evidence = H68ExplicitEvidence $configured.Substring($startup.Length)
        $evidence['reload'] = $reload
        H68Gate 'h68_explicit_keyboard_reload_dry_run' $evidence.pass $evidence
        $removal = H68Reload $kBase
        $removed = H68Snapshot 'keyboard-removed'
        H68Gate 'h68_keyboard_removal_dry_run' ($removed -notmatch $kNative) ([ordered]@{ reload=$removal; log='keyboard-removed.log'; note='Restoration is shadow-only; unchanged values need no restoration log.' })
        H68Stop
        if ($kDetails.cleanup_errors.Count -gt 0) { throw 'H68 first daemon cleanup failed; refusing another session' }
        H68Start $kExplicit 'explicit-startup'
        $configured = H68Snapshot 'explicit-startup'
        $evidence = H68ExplicitEvidence $configured
        H68Gate 'h68_explicit_keyboard_startup_dry_run' $evidence.pass $evidence
    } catch {
        $kDetails['error'] = [string]$_
    } finally {
        H68Stop
        $clean = ($kState.terminated -and $null -eq $kState.owned -and $kDetails.cleanup_errors.Count -eq 0)
        H68Gate 'h68_owned_cleanup' $clean ([ordered]@{ termination_confirmed=$kState.terminated; errors=@($kDetails.cleanup_errors) })
        if (-not $clean) {
            foreach ($gate in $kGates) {
                if ($gate -eq 'h68_owned_cleanup') { continue }
                $summary[$gate] = $false; $kDetails[$gate].pass = $false
            }
        }
        [IO.File]::WriteAllText((Join-Path $kDir 'evidence.json'), ($kDetails | ConvertTo-Json -Depth 14), $kUtf8)
        [IO.File]::WriteAllText((Join-Path $kRoot 'evidence.json'), ($kDetails | ConvertTo-Json -Depth 14), $kUtf8)
    }
}
