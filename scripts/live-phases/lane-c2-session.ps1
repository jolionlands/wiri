# H69: child-only environment, startup and reload integration. Dot-source under
# live-test.ps1's desktop lock, after LaneHConnection has been loaded by lane H.
# Only private files and retained, PID-verified daemons are used. No builds,
# physical switch simulation, system settings, or unrelated process shutdown.
& {
    $c2Root = Join-Path $D 'lane-c2-session'
    $c2Dir = Join-Path $c2Root ([Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -ErrorAction Stop $c2Dir
    $c2Utf8 = New-Object Text.UTF8Encoding($false)
    $c2Details = [ordered]@{ fixture_directory=$c2Dir; cleanup_errors=(New-Object 'Collections.Generic.List[string]') }
    $c2Gates = @('h69_environment_spawn_argv', 'h69_environment_spawn_sh',
        'h69_invalid_reload_keeps_environment', 'h69_reload_replaces_environment',
        'h69_parent_environment_unchanged', 'h69_startup_argv_and_shell_environment',
        'h69_no_autostart_suppresses_both', 'h69_reload_does_not_rerun_startup')
    foreach ($c2Gate in $c2Gates) {
        $summary[$c2Gate] = $false
        $c2Details[$c2Gate] = [ordered]@{ pass=$false; observed='not reached' }
    }
    # Never change $cfg or the harness process environment. ProcessStartInfo
    # supplies the daemon's inherited fixture; -c supplies its private config.
    $c2ParentEnvironment = [Environment]::GetEnvironmentVariables('Process')
    $c2Cfg = [IO.Path]::GetFullPath((Join-Path $c2Dir 'config.kdl'))
    $c2ParentProfile = Join-Path $c2Dir 'parent-profile'
    $c2ChildProfile = Join-Path $c2Dir 'child-profile'
    $c2DaemonDirectory = Join-Path $c2Dir 'daemon-working-directory'
    $c2Roaming = Join-Path $c2Dir 'roaming'
    # Both planned sessions share only this private journal. A later startup
    # can consume an earlier session's record; never use the harness journal.
    $c2Local = Join-Path $c2Dir 'local'
    $c2Journal = Join-Path $c2Local 'wiri\hidden.json'
    $c2Details['recovery_journal'] = $c2Journal
    $c2State = @{ owned=$null; rpc=$null; events=$null; pipe=$null; outTask=$null; errTask=$null; label=$null; terminationConfirmed=$true; stopRequested=$false }
    $c2Isolation = New-Object 'Collections.Generic.List[object]'
    function C2SessionGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $c2Details[$name] = [ordered]@{ pass=$pass; observed=$observed }
        [IO.File]::WriteAllText((Join-Path $c2Root 'evidence.json'), ($c2Details | ConvertTo-Json -Depth 18), $c2Utf8)
    }
    function C2KdlString([string]$text) {
        # Raw KDL avoids double-escaping literal Windows path separators.
        $hashes = '#'
        while ($text.Contains('"' + $hashes)) { $hashes += '#' }
        return 'r' + $hashes + '"' + $text + '"' + $hashes
    }
    function C2PsString([string]$text) { return "'" + $text.Replace("'","''") + "'" }
    function C2Wire($request, $reply) {
        $entry = [ordered]@{ daemon=$c2State.label; request=$request; reply=$reply }
        [IO.File]::AppendAllText((Join-Path $c2Dir 'wire.ndjson'), (($entry | ConvertTo-Json -Depth 12 -Compress) + "`n"), $c2Utf8)
    }
    function C2SessionRequest($request) {
        $c2State.rpc.Send(($request | ConvertTo-Json -Depth 10 -Compress) + "`n")
        $line = $c2State.rpc.ReadLine(4000)
        if ($null -eq $line) { throw 'H69 owned IPC reply timed out' }
        $reply = $line | ConvertFrom-Json
        C2Wire $request $reply
        return $reply
    }
    function C2SessionEvent([string]$line) {
        [IO.File]::AppendAllText((Join-Path $c2Dir 'events.ndjson'), ($line + "`n"), $c2Utf8)
        return ($line | ConvertFrom-Json)
    }
    function C2DrainConfigEvents {
        $clock = [Diagnostics.Stopwatch]::StartNew(); $last = 0
        do {
            $line = $c2State.events.ReadLine(80)
            if ($null -ne $line) { $null = C2SessionEvent $line; $last = $clock.ElapsedMilliseconds }
            if (($clock.ElapsedMilliseconds - $last) -ge 320) { return }
        } while ($clock.ElapsedMilliseconds -lt 3000)
        throw 'H69 config event stream did not become quiet before editing'
    }
    function C2AwaitConfig([bool]$failed) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 8000) {
            $line = $c2State.events.ReadLine(100)
            if ($null -eq $line) { continue }
            $event = C2SessionEvent $line
            if ($event.type -ceq 'config_loaded' -and $event.data.config_path -ieq $c2Cfg -and
                $null -ne $event.data.PSObject.Properties['failed'] -and [bool]$event.data.failed -eq $failed) { return $event }
        }
        throw "H69 timed out awaiting config_loaded failed=$failed for $c2Cfg"
    }
    function C2SessionReload([string]$text, [bool]$failed) {
        C2DrainConfigEvents
        [IO.File]::WriteAllText($c2Cfg, $text, $c2Utf8)
        $ack = C2SessionRequest @{type='reload_config'}
        # The ack only validates the file. The event is sent after apply/failure.
        $event = C2AwaitConfig $failed
        C2DrainConfigEvents
        return [pscustomobject]@{ ack=$ack; event=$event }
    }
    function C2NewProbe([string]$label) {
        $dir = Join-Path $c2Dir $label
        $null = New-Item -ItemType Directory -Force -ErrorAction Stop $dir
        $nameLines = @($c2Names.Keys | ForEach-Object { $_ + '=' + (C2PsString $c2Names[$_]) }) -join "`n"
        # A fresh file per invocation makes a startup rerun observable, including
        # when it writes the same values. Rename only after JSON is complete.
        $script = @'
$ErrorActionPreference = 'Stop'
$names = [ordered]@{
@@NAMES@@
}
$environment = [Environment]::GetEnvironmentVariables('Process')
$values = [ordered]@{}
foreach ($key in $names.Keys) {
    $found = @($environment.Keys | Where-Object { [String]::Equals([string]$_, [string]$names[$key], [StringComparison]::OrdinalIgnoreCase) })
    $value = $null
    if ($found.Count -eq 1) { $value = [string]$environment[$found[0]] }
    $values[$key] = [ordered]@{ present=($found.Count -eq 1); matches=$found.Count; value=$value }
}
$record = [ordered]@{ label=@@LABEL@@; pid=$PID; cwd=[IO.Directory]::GetCurrentDirectory(); env=$values }
$dir = @@DIR@@
$path = [IO.Path]::Combine($dir, ([Guid]::NewGuid().ToString('N') + '.json'))
# ReadJsonArray uses Get-Content on Windows PowerShell 5.1; a UTF-8 BOM
# keeps non-ASCII fixture values intact instead of decoding them as ANSI.
[IO.File]::WriteAllText(($path + '.tmp'), ($record | ConvertTo-Json -Depth 8), (New-Object Text.UTF8Encoding($true)))
[IO.File]::Move(($path + '.tmp'), $path)
'@
        $script = $script.Replace('@@NAMES@@',$nameLines).Replace('@@LABEL@@',(C2PsString $label)).Replace('@@DIR@@',(C2PsString $dir))
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
        $words = @('-NoLogo','-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-WindowStyle','Hidden','-EncodedCommand',$encoded)
        $shell = (Q $c2PowerShell) + ' ' + (@($words | ForEach-Object { Q $_ }) -join ' ')
        return [pscustomobject]@{ label=$label; dir=$dir; args=$words; shell=$shell }
    }
    function C2ProbeFiles($probe) { return @(Get-ChildItem -LiteralPath $probe.dir -Filter '*.json' -File -ErrorAction Stop) }
    function C2ReadProbe($probe) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $files = @(C2ProbeFiles $probe)
            if ($files.Count -gt 0) {
                $records = @(ReadJsonArray $files[0].FullName)
                if ($records.Count -ne 1) { throw "H69 malformed probe record: $($files[0].FullName)" }
                return [pscustomobject]@{ count=$files.Count; path=$files[0].FullName; snapshot=$records[0] }
            }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt 10000)
        throw "H69 probe timed out: $($probe.label)"
    }
    function C2EnvironmentMatches($observed, $expected) {
        if ($null -eq $observed -or $null -eq $observed.env) { return $false }
        foreach ($key in $expected.Keys) {
            $property = $observed.env.PSObject.Properties[$key]
            if ($null -eq $property) { return $false }
            $entry = $property.Value; $want = $expected[$key]
            $count = 0; if ($want.present) { $count = 1 }
            if ([int]$entry.matches -ne $count -or [bool]$entry.present -ne [bool]$want.present) { return $false }
            if ($want.present -and [string]$entry.value -cne [string]$want.value) { return $false }
        }
        return $true
    }
    function C2LaunchProbe($probe, [bool]$shell) {
        if ($shell) {
            $reply = C2SessionRequest @{type='run_action'; data=@{name='spawn-sh'; args=@($probe.shell); quoted=@($true)}}
        } else {
            $reply = C2SessionRequest @{type='spawn_command'; data=@{command=$c2PowerShell; args=$probe.args}}
        }
        if ($reply.success -ne $true) { throw "H69 spawn failed: $($reply | ConvertTo-Json -Compress)" }
        $observed = C2ReadProbe $probe
        if (-not $shell -and ([int]$reply.pid -le 0 -or [int]$reply.pid -ne [int]$observed.snapshot.pid)) { throw 'H69 argv probe PID differs from spawn reply' }
        $c2Isolation.Add($observed.snapshot)
        return [pscustomobject]@{ reply=$reply; observed=$observed }
    }
    function C2ProbeMatches($launch, $expected) {
        return ($launch.observed.count -eq 1 -and (C2EnvironmentMatches $launch.observed.snapshot $expected))
    }
    function C2SessionConfig([string]$environment, $startupArgv, $startupShell) {
        $argv = @($c2PowerShell) + @($startupArgv.args)
        return "animations { off; }`ninput { focus-follows-mouse false; }`nbinds {}`nconfig-notification { disable-failed; }`n" +
            "environment {`n$environment`n}`n" +
            'spawn-at-startup ' + (@($argv | ForEach-Object { C2KdlString $_ }) -join ' ') + "`n" +
            'spawn-sh-at-startup ' + (C2KdlString $startupShell.shell) + "`n"
    }
    function C2StartSession([string]$text, [bool]$noAutostart, [string]$label) {
        if ($null -ne $c2State.owned -or -not $c2State.terminationConfirmed) { throw 'H69 refused another session: prior daemon termination is unconfirmed' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'H69 refused an existing unowned wiri process' }
        [IO.File]::WriteAllText($c2Cfg, $text, $c2Utf8)
        $c2State.label = $label
        $c2State.pipe = '\\.\pipe\wiri_live_h69_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $psi = New-Object Diagnostics.ProcessStartInfo
        $psi.FileName = $wiri
        # --no-startup-overlay (lane P): a startup hotkey sheet must not take a scripted key press.
        $words = @('-v','--no-startup-overlay'); if ($noAutostart) { $words += '--no-autostart' }; $words += @('-c',(Q $c2Cfg))
        $psi.Arguments = $words -join ' '
        $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
        $psi.StandardOutputEncoding = [Text.Encoding]::UTF8; $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
        # Distinct from both profiles: a missing daemon USERPROFILE must not
        # make the spawner's inherited-cwd fallback satisfy the isolation gate.
        $psi.WorkingDirectory = $c2DaemonDirectory
        $psi.EnvironmentVariables['APPDATA'] = $c2Roaming; $psi.EnvironmentVariables['LOCALAPPDATA'] = $c2Local
        $psi.EnvironmentVariables['USERPROFILE'] = $c2ParentProfile
        $psi.EnvironmentVariables['WIRI_PIPE'] = $c2State.pipe
        $psi.EnvironmentVariables['WIRI_KEYBOARD_DRY_RUN'] = '1'
        $psi.EnvironmentVariables['WIRI_SYSTEM_ACTIONS_DRY_RUN'] = Join-Path $c2Dir ($label + '-system-actions.ndjson')
        foreach ($key in $c2Inherited.Keys) { $psi.EnvironmentVariables[$key] = $c2Inherited[$key] }
        $p = New-Object Diagnostics.Process; $p.StartInfo = $psi
        if (-not $p.Start()) { $p.Dispose(); throw 'H69 failed to start private daemon' }
        $c2State.owned = $p; $c2State.terminationConfirmed = $false; $c2State.stopRequested = $false
        $c2Details['pid_' + $label] = $p.Id
        $null = $p.Handle
        $c2State.outTask = $p.StandardOutput.ReadToEndAsync(); $c2State.errTask = $p.StandardError.ReadToEndAsync()
        $clock = [Diagnostics.Stopwatch]::StartNew(); $lastError = ''
        while ($clock.ElapsedMilliseconds -lt 20000 -and -not $p.HasExited) {
            try { $c2State.rpc = New-Object LaneHConnection $c2State.pipe,$p.Id; break }
            catch { $lastError = [string]$_; Start-Sleep -Milliseconds 100 }
        }
        if ($null -eq $c2State.rpc) { throw "H69 private daemon did not serve its verified pipe: $lastError" }
        $c2State.events = New-Object LaneHConnection $c2State.pipe,$p.Id
        $request = @{type='subscribe_events'; data=@{event_types=@('config_loaded')}}
        $c2State.events.Send(($request | ConvertTo-Json -Depth 5 -Compress) + "`n")
        $line = $c2State.events.ReadLine(4000)
        if ($null -eq $line) { throw 'H69 event subscription timed out' }
        $ack = $line | ConvertFrom-Json; C2Wire $request $ack
        if ($ack.success -ne $true) { throw 'H69 event subscription was rejected' }
        # Consume the initial replay before any edits, not as a reload result.
        $null = C2AwaitConfig $false
        C2DrainConfigEvents
    }
    function C2StopSession {
        foreach ($key in @('events','rpc')) {
            if ($null -ne $c2State[$key]) {
                try { $c2State[$key].Dispose() } catch { $c2Details.cleanup_errors.Add([string]$_) }
                $c2State[$key] = $null
            }
        }
        $p = $c2State.owned
        if ($null -eq $p) { return }
        try {
            if (-not $p.HasExited) {
                $c2State.stopRequested = $true
                try {
                    $quit = New-Object LaneHConnection $c2State.pipe,$p.Id
                    try { $quit.Send("{`"type`":`"quit`"}`n") } finally { $quit.Dispose() }
                } catch { $c2Details.cleanup_errors.Add([string]$_) }
                if (-not $p.WaitForExit(2500)) {
                    # Kill only through the retained process handle, never by name
                    # or by discovering a PID from an inherited/default pipe.
                    $c2Details['forced_owned_stop_' + $c2State.label] = $p.Id
                    $c2Details.cleanup_errors.Add('H69 required forced shutdown of owned daemon ' + $c2State.label)
                    $p.Kill()
                    if (-not $p.WaitForExit(5000)) { throw 'H69 retained daemon did not exit after Kill' }
                }
            } elseif (-not $c2State.stopRequested) {
                $c2Details.cleanup_errors.Add('H69 owned daemon exited unexpectedly before cleanup: ' + $c2State.label)
            }
            $c2State.terminationConfirmed = $p.HasExited
            $c2Details['exit_' + $c2State.label] = $p.ExitCode
            if ($p.ExitCode -ne 0) { $c2Details.cleanup_errors.Add('H69 owned daemon exited nonzero: ' + $c2State.label + ' exit=' + $p.ExitCode) }
            $c2Details['journal_after_' + $c2State.label] = Test-Path -LiteralPath $c2Journal -ErrorAction Stop
        } catch { $c2Details.cleanup_errors.Add([string]$_) }
        finally {
            $c2Details['termination_confirmed_' + $c2State.label] = $c2State.terminationConfirmed
            foreach ($stream in @(@('stdout','outTask'),@('stderr','errTask'))) {
                $task = $c2State[$stream[1]]
                if ($null -eq $task) { continue }
                try {
                    if ($task.Wait(2000)) {
                        $text = $task.GetAwaiter().GetResult()
                        [IO.File]::WriteAllText((Join-Path $c2Dir ($c2State.label + '.' + $stream[0])), $text, $c2Utf8)
                        $c2State[$stream[1]] = $null
                    } else { $c2Details.cleanup_errors.Add('H69 log drain timed out: ' + $stream[0]) }
                } catch { $c2Details.cleanup_errors.Add([string]$_) }
            }
            # Keep ownership and block C2StartSession if shutdown was not
            # confirmed. The outer finally may retry; never lose the handle.
            if ($c2State.terminationConfirmed) {
                try { $p.Dispose() } catch { $c2Details.cleanup_errors.Add([string]$_) }
                $c2State.owned = $null; $c2State.outTask = $null; $c2State.errTask = $null
            }
        }
    }
    try {
        foreach ($api in @('Q','ReadJsonArray')) {
            if ($null -eq (Get-Command $api -ErrorAction SilentlyContinue)) { throw "H69 missing harness API $api" }
        }
        if (-not ('LaneHConnection' -as [type])) { throw 'H69 requires the existing PID-verifying LaneHConnection API' }
        $c2PowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        if (-not (Test-Path -LiteralPath $c2PowerShell -PathType Leaf)) { throw 'H69 Windows PowerShell probe executable is unavailable' }
        $null = New-Item -ItemType Directory -Force -ErrorAction Stop $c2ParentProfile,$c2ChildProfile,$c2DaemonDirectory,$c2Roaming,$c2Local
        $prefix = 'WIRI_LIVE_H69_' + [Guid]::NewGuid().ToString('N').ToUpperInvariant() + '_'
        $c2Names = [ordered]@{ override=($prefix+'SET'); removed=($prefix+'REMOVE'); empty=($prefix+'EMPTY'); stale=($prefix+'STALE'); case=($prefix+'CASE'); inherited=($prefix+'INHERIT'); added=($prefix+'ADDED'); profile='USERPROFILE' }
        $c2Inherited = @{}
        foreach ($key in @('override','removed','empty','stale','case','inherited')) { $c2Inherited[$c2Names[$key]] = 'parent-' + $key }
        $firstValue = 'configured one ' + [char]0x03A9 + ' ' + [char]::ConvertFromUtf32(0x1F680) + ' = % & literal'
        $secondValue = 'configured two ' + [char]0x03BB
        $initialEnvironment = @(
            ($c2Names.override + ' ' + (C2KdlString $firstValue) + ';'),
            ($c2Names.removed + ' null;'), ($c2Names.empty + ' "";'),
            ($c2Names.stale + ' "only-in-first-map";'),
            ($c2Names.case.ToLowerInvariant() + ' "case-insensitive-override";'),
            ('USERPROFILE ' + (C2KdlString $c2ChildProfile) + ';')) -join "`n"
        $replacementEnvironment = @(
            ($c2Names.override + ' ' + (C2KdlString $secondValue) + ';'),
            ($c2Names.added + ' "new-in-second-map";'),
            ('USERPROFILE ' + (C2KdlString $c2ChildProfile) + ';')) -join "`n"
        $firstExpected = [ordered]@{}
        $secondExpected = [ordered]@{}
        foreach ($key in $c2Names.Keys) {
            $firstExpected[$key] = @{present=$true; value=$c2Inherited[$c2Names[$key]]}
            $secondExpected[$key] = @{present=$true; value=$c2Inherited[$c2Names[$key]]}
        }
        $firstExpected.override.value = $firstValue
        $firstExpected.removed = @{present=$false; value=$null}; $firstExpected.empty.value = ''
        $firstExpected.stale.value = 'only-in-first-map'; $firstExpected.case.value = 'case-insensitive-override'
        $firstExpected.added = @{present=$false; value=$null}; $firstExpected.profile.value = $c2ChildProfile
        $secondExpected.override.value = $secondValue; $secondExpected.added.value = 'new-in-second-map'; $secondExpected.profile.value = $c2ChildProfile
        $startupArgv = C2NewProbe 'startup-argv'; $startupShell = C2NewProbe 'startup-shell'
        $initialConfig = C2SessionConfig $initialEnvironment $startupArgv $startupShell
        C2StartSession $initialConfig $false 'normal'
        $startupA = C2ReadProbe $startupArgv; $startupS = C2ReadProbe $startupShell
        $c2Isolation.Add($startupA.snapshot); $c2Isolation.Add($startupS.snapshot)
        C2SessionGate 'h69_startup_argv_and_shell_environment' ($startupA.count -eq 1 -and $startupS.count -eq 1 -and
            (C2EnvironmentMatches $startupA.snapshot $firstExpected) -and (C2EnvironmentMatches $startupS.snapshot $firstExpected)) @($startupA,$startupS)
        $argvFirst = C2LaunchProbe (C2NewProbe 'argv-first') $false
        $shellFirst = C2LaunchProbe (C2NewProbe 'shell-first') $true
        C2SessionGate 'h69_environment_spawn_argv' (C2ProbeMatches $argvFirst $firstExpected) $argvFirst
        C2SessionGate 'h69_environment_spawn_sh' (C2ProbeMatches $shellFirst $firstExpected) $shellFirst
        # Case-insensitive duplicate names must reject the entire new map.
        $badEnvironment = $replacementEnvironment + "`n" + $c2Names.override.ToLowerInvariant() + ' "must-not-leak";'
        $badReload = C2SessionReload (C2SessionConfig $badEnvironment $startupArgv $startupShell) $true
        $argvBad = C2LaunchProbe (C2NewProbe 'argv-after-invalid') $false
        $shellBad = C2LaunchProbe (C2NewProbe 'shell-after-invalid') $true
        C2SessionGate 'h69_invalid_reload_keeps_environment' ($badReload.ack.success -eq $false -and
            -not [string]::IsNullOrWhiteSpace([string]$badReload.event.data.error) -and
            (C2ProbeMatches $argvBad $firstExpected) -and (C2ProbeMatches $shellBad $firstExpected)) ([ordered]@{reload=$badReload; argv=$argvBad; shell=$shellBad})
        $goodReload = C2SessionReload (C2SessionConfig $replacementEnvironment $startupArgv $startupShell) $false
        $argvSecond = C2LaunchProbe (C2NewProbe 'argv-after-replacement') $false
        $shellSecond = C2LaunchProbe (C2NewProbe 'shell-after-replacement') $true
        C2SessionGate 'h69_reload_replaces_environment' ($goodReload.ack.success -eq $true -and
            (C2ProbeMatches $argvSecond $secondExpected) -and (C2ProbeMatches $shellSecond $secondExpected)) ([ordered]@{reload=$goodReload; argv=$argvSecond; shell=$shellSecond})
        # Negative observations get a bounded quiet window, not an immediate
        # filesystem check while a rerun PowerShell process could still start.
        Start-Sleep -Milliseconds 4000
        $afterArgv = @(C2ProbeFiles $startupArgv).Count; $afterShell = @(C2ProbeFiles $startupShell).Count
        C2SessionGate 'h69_reload_does_not_rerun_startup' ($startupA.count -eq 1 -and $startupS.count -eq 1 -and $afterArgv -eq 1 -and $afterShell -eq 1) ([ordered]@{before=@($startupA.count,$startupS.count); after=@($afterArgv,$afterShell); quiet_ms=4000})
        C2StopSession
        $noAutoArgv = C2NewProbe 'no-autostart-argv'; $noAutoShell = C2NewProbe 'no-autostart-shell'
        C2StartSession (C2SessionConfig $initialEnvironment $noAutoArgv $noAutoShell) $true 'no-autostart'
        # Prove this daemon actually serves spawns with the configured map; an
        # exited/failed daemon is not evidence that --no-autostart worked.
        $noAutoLiveArgv = C2LaunchProbe (C2NewProbe 'no-autostart-live-argv') $false
        $noAutoLiveShell = C2LaunchProbe (C2NewProbe 'no-autostart-live-shell') $true
        Start-Sleep -Milliseconds 4000
        $noAutoCounts = @(@(C2ProbeFiles $noAutoArgv).Count,@(C2ProbeFiles $noAutoShell).Count)
        C2SessionGate 'h69_no_autostart_suppresses_both' ($noAutoCounts[0] -eq 0 -and $noAutoCounts[1] -eq 0 -and
            (C2ProbeMatches $noAutoLiveArgv $firstExpected) -and (C2ProbeMatches $noAutoLiveShell $firstExpected)) ([ordered]@{startup_counts=$noAutoCounts; argv=$noAutoLiveArgv; shell=$noAutoLiveShell; quiet_ms=4000})
    } catch {
        $c2Details['error'] = [string]$_
        $c2Details['error_location'] = [string]$_.InvocationInfo.PositionMessage
    } finally {
        C2StopSession
        if (-not $c2State.terminationConfirmed) { $c2Details.cleanup_errors.Add('H69 daemon termination remains unconfirmed; no further private session may start') }
        try {
            $journalRemaining = Test-Path -LiteralPath $c2Journal -ErrorAction Stop
            $c2Details['recovery_journal_remaining'] = $journalRemaining
            if ($journalRemaining) { $c2Details.cleanup_errors.Add('H69 private window recovery is unresolved; preserve ' + $c2Journal + ' (no harness journal replay API)') }
        } catch { $c2Details.cleanup_errors.Add([string]$_) }
        $afterEnvironment = [Environment]::GetEnvironmentVariables('Process')
        $parentUnchanged = $afterEnvironment.Count -eq $c2ParentEnvironment.Count
        foreach ($key in $c2ParentEnvironment.Keys) {
            if (-not $afterEnvironment.Contains($key) -or [string]$afterEnvironment[$key] -cne [string]$c2ParentEnvironment[$key]) { $parentUnchanged = $false }
        }
        # Child USERPROFILE is overridden, but the spawner's cwd still comes
        # from the daemon's original USERPROFILE. This also detects mutating
        # wiri's own process environment, not merely the harness environment.
        $daemonUnchanged = $c2Isolation.Count -ge 10
        foreach ($record in $c2Isolation) {
            if ($record.cwd -ine $c2ParentProfile -or $record.env.profile.value -cne $c2ChildProfile) { $daemonUnchanged = $false }
        }
        C2SessionGate 'h69_parent_environment_unchanged' ($parentUnchanged -and $daemonUnchanged -and $c2Details.cleanup_errors.Count -eq 0 -and -not $c2Details.Contains('error')) ([ordered]@{harness_unchanged=$parentUnchanged; daemon_cwd_unchanged=$daemonUnchanged; expected_cwd=$c2ParentProfile; child_profile=$c2ChildProfile; observations=$c2Isolation.Count; cleanup_ok=($c2Details.cleanup_errors.Count -eq 0)})
        $c2Details['safety'] = @('Parent config and process environment were never modified; all fixture files are under fixture_directory',
            'Every IPC connection checks the retained daemon PID; only that retained process may be force-stopped',
            'PowerShell probes only snapshot named fixture variables, write private JSON and exit; no builds or physical switches',
            'No harness process-tree ownership or private journal replay API: descendant termination is not attempted; unresolved private recovery fails and its journal is preserved')
        [IO.File]::WriteAllText((Join-Path $c2Root 'evidence.json'), ($c2Details | ConvertTo-Json -Depth 18), $c2Utf8)
    }
}
