# H67. Dot-source after lane-h, under live-test's mutex. Only owned probes receive keys.
& {
    $bDir = Join-Path $D 'lane-c2-binds'
    $null = New-Item -ItemType Directory -Force $bDir
    $bUtf8 = New-Object Text.UTF8Encoding($false)
    $bEvidence = [ordered]@{}
    $bNames = @('h67_omitted_binds_no_defaults','h67_empty_binds_no_defaults',
        'h67_nonempty_binds_replace_defaults','h67_include_overrides_canonical_aliases_final_mod',
        'h67_duplicate_aliases_both_locations','h67_unknown_modifier_rejects_retains_last_good',
        'h67_altgr_aliases_validation_only',
        'h79_action_names_all_parse','h79_action_names_all_run_over_ipc')
    $bState = @{ p=$null; rpc=$null; events=$null; form=[IntPtr]::Zero; out=$null; err=$null }
    $bPipe = '\\.\pipe\wiri-h67-' + [Guid]::NewGuid().ToString('N')
    $cfg = Join-Path $bDir 'config.kdl'
    $bBase = 'animations { off; }' + "`n" + 'input { mod-key "ctrl-alt"; focus-follows-mouse false; }' + "`n" + 'config-notification { disable-failed; }' + "`n"
    $bSavedPipe = $env:WIRI_PIPE
    function C67Gate($name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $bEvidence[$name] = [ordered]@{pass=$pass; observed=$observed}
        [IO.File]::WriteAllText((Join-Path $bDir 'evidence.json'), ($bEvidence | ConvertTo-Json -Depth 14), $bUtf8)
    }
    function C67Wire($value) {
        [IO.File]::AppendAllText((Join-Path $bDir 'wire.ndjson'), (($value | ConvertTo-Json -Depth 12 -Compress) + "`n"), $bUtf8)
    }
    function C67Rpc([string]$type, $data = @{}) {
        $request = @{type=$type}
        if ($data.Count -gt 0) { $request.data = $data }
        $bState.rpc.Send(($request | ConvertTo-Json -Depth 8 -Compress) + "`n")
        $line = $bState.rpc.ReadLine(4000)
        if ($null -eq $line) { throw "H67 $type timed out" }
        $reply = $line | ConvertFrom-Json
        C67Wire @{request=$request; reply=$reply}
        return $reply
    }
    function C67Binds {
        $r = C67Rpc 'get_bindings'
        if ($r.success -ne $true) { throw 'H67 get_bindings failed' }
        return @($r.result)
    }
    function C67Has($rows, [int]$mods, [int]$key, [string]$action) {
        return (@($rows | Where-Object { $_.modifiers -eq $mods -and $_.vk_code -eq $key -and $_.action -eq $action }).Count -eq 1)
    }
    function C67Drain {
        $clock = [Diagnostics.Stopwatch]::StartNew(); $last = 0
        do {
            $line = $bState.events.ReadLine(80)
            if ($null -ne $line) { C67Wire @{event=($line | ConvertFrom-Json)}; $last = $clock.ElapsedMilliseconds }
            if (($clock.ElapsedMilliseconds - $last) -ge 320) { return }
        } while ($clock.ElapsedMilliseconds -lt 3000)
        throw 'H67 event stream did not become quiet'
    }
    function C67Reload([string]$text, [bool]$failed = $false) {
        C67Drain
        [IO.File]::WriteAllText($cfg, $text, $bUtf8)
        $null = C67Rpc 'reload_config'
        # An acknowledgement is NOT evidence that the new config was applied.
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 8000) {
            $line = $bState.events.ReadLine(100)
            if ($null -eq $line) { continue }
            $event = $line | ConvertFrom-Json
            C67Wire @{event=$event}
            if ($event.type -eq 'config_loaded' -and $event.data.config_path -ieq $cfg -and
                $null -ne $event.data.PSObject.Properties['failed'] -and [bool]$event.data.failed -eq $failed) {
                C67Drain
                return $event
            }
        }
        throw "H67 missing config_loaded failed=$failed"
    }
    function C67Echo([string]$chord, [string]$key, [bool]$expected) {
        $null = C67Rpc 'focus_window' @{window_hwnd=[int64]$bState.form}
        FocusForm $bState.form
        if (-not (WaitForeground $bState.form 200 2500)) { throw 'H67 refused keys: probe is not foreground' }
        [TestForms]::ClearKeys()
        Chord $chord
        Start-Sleep -Milliseconds 350
        if ([LT]::GetForegroundWindow() -ne $bState.form) { throw 'H67 probe lost foreground' }
        Chord 'a'
        Start-Sleep -Milliseconds 200
        $seen = [TestForms]::KeyLog()
        C67Wire @{chord=$chord; key_log=$seen; expected_echo=$expected}
        return (($seen.Split(',') -contains 'A') -and (($seen.Split(',') -contains $key) -eq $expected) -and
            [LT]::GetForegroundWindow() -eq $bState.form)
    }
    function C67Validate([string]$name, [string]$text) {
        $path = Join-Path $bDir ($name + '.kdl')
        [IO.File]::WriteAllText($path, $text, $bUtf8)
        $r = [LaneHNative]::RunCli($wiri, @('validate','-c',$path), $null, 5000)
        [IO.File]::WriteAllText((Join-Path $bDir ($name + '.stdout.txt')), $r.Out, $bUtf8)
        [IO.File]::WriteAllText((Join-Path $bDir ($name + '.stderr.txt')), $r.Err, $bUtf8)
        return @{rc=[string]$r.Rc; text=($r.Out + "`n" + $r.Err)}
    }
    # Local override: retain Start-WiriWithConfig's NoDefaultBinds behavior, but
    # launch with child-only isolation, an explicit private config and retained handle.
    function Start-Wiri([string]$stderrLog, [string]$stdoutLog) {
        $psi = New-Object Diagnostics.ProcessStartInfo $wiri
        # No startup hotkey sheet (lane P): it must not take the first scripted key press.
        $psi.Arguments = '-v --no-startup-overlay -c ' + (Q $cfg)
        $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
        foreach ($pair in @(@('APPDATA','roaming'),@('LOCALAPPDATA','local'))) {
            $path = Join-Path $bDir $pair[1]
            $null = New-Item -ItemType Directory -Force $path
            $psi.EnvironmentVariables[$pair[0]] = $path
        }
        $psi.EnvironmentVariables['WIRI_PIPE'] = $bPipe
        $psi.EnvironmentVariables['WIRI_KEYBOARD_DRY_RUN'] = '1'
        $psi.EnvironmentVariables['WIRI_SYSTEM_ACTIONS_DRY_RUN'] = Join-Path $bDir 'system-actions.jsonl'
        # h79 records every action it sends instead of performing it: a quit
        # confirmation, the screenshot UI or a suspend would end this lane, and
        # the claim under test is that the names are understood, not their effect.
        $psi.EnvironmentVariables['WIRI_ACTIONS_DRY_RUN'] = Join-Path $bDir 'actions.jsonl'
        $bState.p = [Diagnostics.Process]::Start($psi)
        $null = $bState.p.Handle
        $bState.out = $bState.p.StandardOutput.ReadToEndAsync()
        $bState.err = $bState.p.StandardError.ReadToEndAsync()
        return $bState.p
    }
    try {
        if ($null -ne $w -and -not $w.HasExited) { Stop-WiriGracefully $w 'h67-predecessor' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -gt 0) { throw 'H67 refuses an existing wiri process' }
        $env:WIRI_PIPE = $bPipe
        $null = Start-WiriWithConfig $bBase (Join-Path $bDir 'daemon.stderr.txt') -NoDefaultBinds
        $bState.rpc = New-Object LaneHConnection $bPipe,$bState.p.Id
        $bState.events = New-Object LaneHConnection $bPipe,$bState.p.Id
        $bState.events.Send("{`"type`":`"subscribe_events`",`"data`":{`"event_types`":[`"config_loaded`"]}}`n")
        $line = $bState.events.ReadLine(4000)
        if ($null -eq $line -or ($line | ConvertFrom-Json).success -ne $true) { throw 'H67 subscription failed' }
        C67Drain
        $bState.form = [TestForms]::OpenKeys('wiri-h67-owned-key-echo')
        Start-Sleep -Milliseconds 1200
        $rows = @(C67Binds)
        $echoJ = C67Echo 'ctrl+alt+j' 'J' $true
        $echoF = C67Echo 'ctrl+alt+f' 'F' $true
        C67Gate $bNames[0] ($rows.Count -eq 0 -and $echoJ -and $echoF) @{bindings=$rows; echoJ=$echoJ; echoF=$echoF}
        $null = C67Reload ($bBase + 'binds {}')
        $rows = @(C67Binds)
        $echoJ = C67Echo 'ctrl+alt+j' 'J' $true
        $echoF = C67Echo 'ctrl+alt+f' 'F' $true
        C67Gate $bNames[1] ($rows.Count -eq 0 -and $echoJ -and $echoF) @{bindings=$rows; echoJ=$echoJ; echoF=$echoF}
        $null = C67Reload ($bBase + 'binds { Ctrl+Alt+F7 toggle-floating; }')
        $rows = @(C67Binds)
        $echoJ = C67Echo 'ctrl+alt+j' 'J' $true
        $echoF = C67Echo 'ctrl+alt+f' 'F' $true
        $consumed = C67Echo 'ctrl+alt+f7' 'F7' $false
        C67Gate $bNames[2] ($rows.Count -eq 1 -and (C67Has $rows 3 118 'ToggleFloating') -and $echoJ -and $echoF -and $consumed) @{bindings=$rows; echoJ=$echoJ; echoF=$echoF; sentinel_consumed=$consumed}
        $include = Join-Path $bDir 'included.kdl'
        [IO.File]::WriteAllText($include, 'binds { Mod+Return toggle-floating; Control+Prior toggle-floating; }', $bUtf8)
        $good = $bBase.Replace('input { mod-key "ctrl-alt"; focus-follows-mouse false; }', '') +
            'include r#"' + $include + '"#' + "`n" +
            'binds { Alt+Enter toggle-fullscreen; Ctrl+PageUp toggle-fullscreen; }' + "`n" + 'input { mod-key "alt"; focus-follows-mouse false; }'
        $null = C67Reload $good
        $rows = @(C67Binds)
        C67Gate $bNames[3] ($rows.Count -eq 2 -and (C67Has $rows 1 13 'ToggleFullscreen') -and (C67Has $rows 2 33 'ToggleFullscreen')) $rows
        $duplicate = C67Validate 'duplicate' "binds {`n    Control+Prior toggle-floating;`n    Ctrl+PageUp toggle-fullscreen;`n}`n"
        C67Gate $bNames[4] ($duplicate.rc -match '^[1-9][0-9]*$' -and $duplicate.text -match 'duplicate chord' -and
            $duplicate.text -match 'first definition is here' -and $duplicate.text -match ':2:\d+' -and $duplicate.text -match ':3:\d+') $duplicate
        $invalid = $bBase + 'binds { Hyper+F7 toggle-floating; }'
        $bad = C67Validate 'unknown-modifier' $invalid
        $failedEvent = C67Reload $invalid $true
        $retained = @(C67Binds)
        C67Gate $bNames[5] ($bad.rc -match '^[1-9][0-9]*$' -and $bad.text -match "unknown modifier 'Hyper'" -and
            $retained.Count -eq 2 -and (C67Has $retained 1 13 'ToggleFullscreen') -and (C67Has $retained 2 33 'ToggleFullscreen')) @{validation=$bad; completion=$failedEvent; retained=$retained}
        # Validation ONLY: never synthesize AltGr/Mod5/ISO_Level3_Shift.
        $altgr = C67Validate 'altgr-aliases' ($bBase + 'binds { AltGr+F7 toggle-floating; Mod5+F8 toggle-floating; ISO_Level3_Shift+F9 toggle-floating; }')
        C67Gate $bNames[6] ($altgr.rc -eq '0') @{mode='validation-only; no AltGr injection'; validation=$altgr}
        # H79: ONE source of truth.  `wiri-ctl action-names --json` prints the
        # vocabulary the decoder ships (config::decode::bindable_niri_actions())
        # together with the arguments that action's own parser accepts, so a new
        # niri action is bound and run by these two gates without anyone editing
        # this script.  One gate per half, not one per action.
        $catalog = [LaneHNative]::RunCli($ctl, @('action-names','--json'), $null, 10000)
        [IO.File]::WriteAllText((Join-Path $bDir 'action-names.json'), $catalog.Out, $bUtf8)
        $actions = @()
        try { $actions = @($catalog.Out | ConvertFrom-Json) } catch { }
        if ([string]$catalog.Rc -ne '0' -or $actions.Count -lt 50) {
            throw 'H79 action-names did not list the action vocabulary'
        }
        $c79Keys = @('A','B','C','D','E','F','G','H','I','J','K','L','M','N','O','P','Q','R','S','T','U','V','W','X','Y','Z','F1','F2','F3','F4','F5','F6')
        $c79Mods = @('Ctrl+Alt+','Ctrl+Alt+Shift+','Ctrl+Shift+','Alt+Shift+')
        if ($actions.Count -gt ($c79Keys.Count * $c79Mods.Count)) {
            throw 'H79 the fixture needs more distinct chords than the lane defines'
        }
        # One bind per name, every chord carrying at least two modifiers so the
        # fixture cannot answer a stray keystroke while it is loaded.
        $c79Binds = New-Object 'Collections.Generic.List[string]'
        for ($i = 0; $i -lt $actions.Count; $i++) {
            $chord = $c79Mods[[int]($i % $c79Mods.Count)] + $c79Keys[[int][Math]::Floor($i / $c79Mods.Count)]
            $words = @([string]$actions[$i].name) + @($actions[$i].args | ForEach-Object { [string]$_ })
            $c79Binds.Add('    ' + $chord + ' ' + ($words -join ' ') + ';')
        }
        $c79Text = $bBase + "binds {`n" + ($c79Binds -join "`n") + "`n}`n"
        $c79Path = Join-Path $bDir 'all-actions.kdl'
        [IO.File]::WriteAllText($c79Path, $c79Text, $bUtf8)
        # `validate-config` is the daemon's own decoder: what it reports here is
        # what the run log would report for these binds, and it needs no daemon.
        $c79Check = [LaneHNative]::RunCli($ctl, @('validate-config',$c79Path,'--json'), $null, 20000)
        [IO.File]::WriteAllText((Join-Path $bDir 'all-actions.validate.stdout.txt'), $c79Check.Out, $bUtf8)
        [IO.File]::WriteAllText((Join-Path $bDir 'all-actions.validate.stderr.txt'), $c79Check.Err, $bUtf8)
        $c79Report = $null
        try { $c79Report = $c79Check.Out | ConvertFrom-Json } catch { }
        $null = C67Reload $c79Text
        $c79Loaded = @(C67Binds)
        $c79Text2 = $c79Check.Out + "`n" + $c79Check.Err
        C67Gate $bNames[7] ([string]$c79Check.Rc -eq '0' -and $null -ne $c79Report -and
            [int]$c79Report.errors -eq 0 -and [int]$c79Report.warnings -eq 0 -and
            $c79Text2 -notmatch 'unknown action' -and $c79Text2 -notmatch 'not implemented by wiri yet' -and
            $c79Loaded.Count -eq $actions.Count) @{count=$actions.Count; rc=[string]$c79Check.Rc;
            errors=$c79Report.errors; warnings=$c79Report.warnings; loaded=$c79Loaded.Count; config=$c79Path}
        # Every name over the pipe.  The daemon records the accepted action in
        # actions.jsonl instead of performing it, so the audit lines are the
        # evidence that each name reached the same decoder the binds use.
        $c79AuditPath = Join-Path $bDir 'actions.jsonl'
        $c79Failed = New-Object 'Collections.Generic.List[string]'
        foreach ($action in $actions) {
            $mark = $steps.Count
            $null = Ctl ('h79-run-' + [string]$action.name) (@('action',[string]$action.name) + @($action.args | ForEach-Object { [string]$_ }))
            if ($steps.Count -ne $mark + 1 -or [string]$steps[$mark].rc -ne '0') {
                $c79Failed.Add([string]$action.name)
            }
        }
        $c79Sent = @($actions | ForEach-Object { [string]$_.name })
        $c79Audited = @()
        if (Test-Path $c79AuditPath) {
            $c79Audited = @(Get-Content $c79AuditPath | ForEach-Object { [string]($_ | ConvertFrom-Json).name })
        }
        $c79Missing = @($c79Sent | Where-Object { $c79Audited -notcontains $_ })
        C67Gate $bNames[8] ($c79Failed.Count -eq 0 -and $c79Missing.Count -eq 0 -and
            $c79Audited.Count -eq $c79Sent.Count) @{ran=$c79Sent.Count; failed=@($c79Failed.ToArray());
            audited=$c79Audited.Count; missing=@($c79Missing)}
    } catch {
        $failure = [string]$_
        foreach ($name in $bNames) { if (-not $bEvidence.Contains($name)) { C67Gate $name $false @{error=$failure} } }
    } finally {
        $cleanup = New-Object 'Collections.Generic.List[string]'
        if ($null -ne $bState.p) {
            if (-not $bState.p.HasExited) {
                try {
                    if ($null -ne $bState.events) {
                        $null = C67Reload ($bBase + 'binds { Ctrl+Alt+Q close-window; }')
                        if ($bState.form -ne [IntPtr]::Zero -and [LT]::IsWindow($bState.form)) {
                            FocusForm $bState.form
                            $null = CloseFocused $bState.form 'h67-owned-form'
                        }
                    }
                } catch { $cleanup.Add([string]$_) }
                try {
                    $quit = New-Object LaneHConnection $bPipe,$bState.p.Id
                    try { $quit.Send("{`"type`":`"quit`"}`n") } finally { $quit.Dispose() }
                } catch { $cleanup.Add([string]$_) }
                if (-not $bState.p.WaitForExit(2500)) {
                    try { $bState.p.Kill(); if (-not $bState.p.WaitForExit(5000)) { $cleanup.Add('owned process did not exit') } } catch { $cleanup.Add([string]$_) }
                }
            }
            foreach ($stream in @('out','err')) {
                if ($null -ne $bState[$stream] -and $bState[$stream].Wait(2000)) {
                    [IO.File]::WriteAllText((Join-Path $bDir ('daemon.' + $stream + '.txt')), $bState[$stream].Result, $bUtf8)
                }
            }
        }
        foreach ($connection in @($bState.rpc,$bState.events)) { if ($null -ne $connection) { $connection.Dispose() } }
        if ($bState.form -ne [IntPtr]::Zero) { [TestForms]::CloseForm($bState.form) }
        if ($null -ne $bState.p) { $bState.p.Dispose() }
        $env:WIRI_PIPE = $bSavedPipe
        C67Gate 'h67_owned_cleanup' ($cleanup.Count -eq 0) @($cleanup)
    }
}
