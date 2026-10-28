# H70: one-display named workspace layout precedence. No display/settings writes.
# Dot-source after lane-h under the harness lock; all UI and daemon handles are owned.
& {
    $nDir = Join-Path $D 'lane-c2-workspaces'
    $null = New-Item -ItemType Directory -Force $nDir
    $nUtf8 = New-Object Text.UTF8Encoding($false)
    $nEvidence = [ordered]@{ scope='one physical display; multi-output priority is unit-tested only' }
    $nNames = @('h70_workspace_layout_overrides_output', 'h70_workspace_override_removal_uses_output', 'h70_output_reload_rebuilds_workspace_layout')
    $nState = @{ daemon=$null; rpc=$null; events=$null; form=[IntPtr]::Zero }
    $nDevice = [System.Windows.Forms.Screen]::PrimaryScreen.DeviceName
    function C70Gate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $nEvidence[$name] = [ordered]@{ pass=$pass; observed=$observed }
    }
    function C70Request([string]$type, $data=@{}) {
        $request = @{type=$type}
        if ($data.Count -gt 0) { $request.data = $data }
        $nState.rpc.Send(($request | ConvertTo-Json -Depth 8 -Compress) + "`n")
        $line = $nState.rpc.ReadLine(4000)
        if ($null -eq $line) { throw "H70 $type reply timed out" }
        $reply = $line | ConvertFrom-Json
        [IO.File]::AppendAllText((Join-Path $nDir 'wire.ndjson'), ((@{ request=$request; reply=$reply } | ConvertTo-Json -Depth 10 -Compress) + "`n"), $nUtf8)
        return $reply
    }
    function C70Config([int]$outputWidth, [bool]$workspaceOverride) {
        $localLayout = ''; if ($workspaceOverride) { $localLayout = 'layout { gaps 28; default-column-width { fixed 310; }; }' }
        return 'animations { off; }' + "`n" +
            'input { focus-follows-mouse false; }' + "`n" +
            'layout { gaps 8; default-column-width { fixed 540; }; border { off; }; focus-ring { off; }; shadow { off; }; }' + "`n" +
            'output r#"' + $nDevice + '"# { layout { gaps 16; default-column-width { fixed ' + $outputWidth + '; }; }; }' + "`n" +
            'workspace "c2-local" { ' + $localLayout + ' }' + "`n" +
            'binds { Ctrl+Alt+Q { close-window; }; }' + "`n" +
            'config-notification { disable-failed; }' + "`n"
    }
    function C70Drain {
        $clock = [Diagnostics.Stopwatch]::StartNew(); $last=0
        do {
            $line = $nState.events.ReadLine(80)
            if ($null -ne $line) { $last=$clock.ElapsedMilliseconds }
            if (($clock.ElapsedMilliseconds-$last) -ge 320) { return }
        } while ($clock.ElapsedMilliseconds -lt 3000)
        throw 'H70 config event stream did not become quiet'
    }
    function C70Reload([string]$text) {
        C70Drain
        [IO.File]::WriteAllText($cfg, $text, $nUtf8)
        $reply = C70Request 'reload_config'
        if ($reply.success -ne $true) { throw 'H70 reload rejected' }
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 8000) {
            $line = $nState.events.ReadLine(100)
            if ($null -eq $line) { continue }
            $event = $line | ConvertFrom-Json
            if ($event.type -eq 'config_loaded' -and $event.data.config_path -ieq $cfg) {
                [IO.File]::AppendAllText((Join-Path $nDir 'events.ndjson'), ($line + "`n"), $nUtf8)
                if ($event.data.failed -ne $false) { throw 'H70 config failed to apply' }
                C70Drain
                return
            }
        }
        throw 'H70 config apply event timed out'
    }
    function C70Open([string]$label, [int]$expectedWidth) {
        $reply = C70Request 'run_action' @{name='focus-workspace'; args=@('c2-local'); quoted=@($true)}
        if ($reply.success -ne $true) { throw 'H70 named workspace focus failed' }
        $nState.form = [TestForms]::Open(('wiri-h70-' + $label), 70, 140, 210)
        Start-Sleep -Milliseconds 1300
        $reply = C70Request 'tile_request' @{window_hwnd=[int64]$nState.form; target_workspace='c2-local'}
        if ($reply.success -ne $true) { throw 'H70 owned form tiling failed' }
        FocusForm $nState.form
        if (-not (WaitForeground $nState.form 300 3000)) { throw 'H70 refused measurement: owned form is not foreground' }
        $rect = [LT]::Visible($nState.form)
        $width = $rect.R - $rect.L
        $observed = [ordered]@{ expected_width=$expectedWidth; actual_width=$width; rectangle=$rect; device=$nDevice }
        Shot ('h70-' + $label)
        $observed['pass'] = ([Math]::Abs($width-$expectedWidth) -le 14)
        return $observed
    }
    function C70Close {
        if ($nState.form -eq [IntPtr]::Zero) { return }
        if ([LT]::IsWindow($nState.form)) {
            FocusForm $nState.form
            if (-not (CloseFocused $nState.form 'h70-owned-form')) { throw 'H70 owned close skipped: foreground guard' }
            Start-Sleep -Milliseconds 700
            if ([LT]::IsWindow($nState.form)) { throw 'H70 close binding did not close the owned form' }
        }
        $nState.form = [IntPtr]::Zero
    }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'H70 refused an existing wiri process' }
        $text = C70Config 440 $true
        $nState.daemon = Start-WiriWithConfig $text (Join-Path $nDir 'daemon.stderr.log') -NoDefaultBinds
        $nState.rpc = New-Object LaneHConnection $env:WIRI_PIPE,$nState.daemon.Id
        $nState.events = New-Object LaneHConnection $env:WIRI_PIPE,$nState.daemon.Id
        $nState.events.Send("{`"type`":`"subscribe_events`",`"data`":{`"event_types`":[`"config_loaded`"]}}`n")
        $line = $nState.events.ReadLine(4000)
        if ($null -eq $line -or ($line | ConvertFrom-Json).success -ne $true) { throw 'H70 event subscription failed' }
        C70Drain
        $measure = C70Open 'workspace' 310
        C70Gate $nNames[0] $measure.pass $measure
        C70Close
        C70Reload (C70Config 440 $false)
        $measure = C70Open 'output' 440
        C70Gate $nNames[1] $measure.pass $measure
        C70Close
        C70Reload (C70Config 470 $false)
        $measure = C70Open 'output-reloaded' 470
        C70Gate $nNames[2] $measure.pass $measure
    } catch {
        $nEvidence['error'] = [string]$_
        foreach ($name in $nNames) { if (-not $nEvidence.Contains($name)) { C70Gate $name $false ([string]$_) } }
    } finally {
        $cleanup = New-Object 'Collections.Generic.List[string]'
        try { C70Close } catch { $cleanup.Add([string]$_) }
        $p = $nState.daemon
        if ($null -ne $p) {
            if (-not $p.HasExited) {
                try {
                    $quit = New-Object LaneHConnection $env:WIRI_PIPE,$p.Id
                    try { $quit.Send("{`"type`":`"quit`"}`n") } finally { $quit.Dispose() }
                } catch { $cleanup.Add([string]$_) }
                if (-not $p.WaitForExit(3000)) {
                    $cleanup.Add('H70 retained daemon required forced stop')
                    $p.Kill(); $null = $p.WaitForExit(5000)
                }
            }
            $p.Dispose()
        }
        foreach ($connection in @($nState.rpc,$nState.events)) { if ($null -ne $connection) { $connection.Dispose() } }
        if ($nState.form -ne [IntPtr]::Zero) { [TestForms]::CloseForm($nState.form) }
        C70Gate 'h70_owned_cleanup' ($cleanup.Count -eq 0) @($cleanup)
        [IO.File]::WriteAllText((Join-Path $nDir 'evidence.json'), ($nEvidence | ConvertTo-Json -Depth 12), $nUtf8)
    }
}
