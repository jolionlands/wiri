# Lane C1+C2: config diagnostics and reload scope. No machine settings.
# Dot-sourced with the desktop lock held. The live probes own their daemon/form.
# Physical output mode/scale writes are deliberately excluded: there is no virtual
# output backend here. Safe per-output layout reloads are also covered by H70.
& {
    $c2Dir = Join-Path $D 'lane-c2'
    $null = New-Item -ItemType Directory -Force $c2Dir
    $c2Details = [ordered]@{}
    $c2SavedWiriConfig = $env:WIRI_CONFIG
    $c2SavedNiriConfig = $env:NIRI_CONFIG
    function C2Gate([string]$name, [bool]$passed, $observed) {
        $summary[$name] = $passed
        $c2Details[$name] = [ordered]@{ pass = $passed; observed = $observed }
    }
    function C2Validate([string]$label, [string[]]$cliArgs) {
        $text = (& $wiri @cliArgs 2>&1 | Out-String)
        $rc = $LASTEXITCODE
        [IO.File]::WriteAllText((Join-Path $c2Dir "$label.txt"), $text)
        return [pscustomobject]@{ rc = $rc; text = $text }
    }
    try {
        $c2Good = Join-Path $c2Dir 'good.kdl'
        $c2Bad = Join-Path $c2Dir 'bad.kdl'
        $c2Child = Join-Path $c2Dir 'child.kdl'
        $c2Root = Join-Path $c2Dir 'root.kdl'
        [IO.File]::WriteAllText($c2Good, "binds {}`nconfig-notification { disable-failed; }`n")
        [IO.File]::WriteAllText($c2Bad, "input {}`ninput { mouse { accel-speed 2; } }`n")
        [IO.File]::WriteAllText($c2Child, "layout { gapz 3; }`n")
        [IO.File]::WriteAllText($c2Root, 'include "child.kdl"' + "`n")
        $c2Result = C2Validate 'valid' @('validate', '-c', $c2Good)
        C2Gate 'h66_niri_validate_offline' ($c2Result.rc -eq 0 -and $c2Result.text -match 'Configuration is valid') $c2Result
        $c2Result = C2Validate 'duplicate-ranges' @('validate', '-c', $c2Bad)
        C2Gate 'h66_duplicate_both_locations' ($c2Result.rc -ne 0 -and $c2Result.text -match 'duplicate node' -and $c2Result.text -match 'first definition is here' -and $c2Result.text -match 'bad.kdl:1:1' -and $c2Result.text -match 'bad.kdl:2:1') $c2Result
        C2Gate 'h66_ranges_reject_not_clamp' ($c2Result.rc -ne 0 -and $c2Result.text -match 'between -1 and 1') $c2Result
        $c2Result = C2Validate 'include-diagnostic' @('validate', '-c', $c2Root)
        C2Gate 'h66_source_labels_help_include_chain' ($c2Result.rc -ne 0 -and $c2Result.text -match 'child.kdl:1:' -and $c2Result.text -match 'included from .*root.kdl:1:1' -and $c2Result.text -match '\^ unknown option' -and $c2Result.text -match 'help:') $c2Result
        $env:WIRI_CONFIG = $c2Bad
        $env:NIRI_CONFIG = $c2Root
        $c2Result = C2Validate 'search-wiri' @('validate')
        C2Gate 'h66_wiri_config_precedes_niri_config' ($c2Result.rc -ne 0 -and $c2Result.text -match 'bad.kdl:2:1') $c2Result
        $c2Result = C2Validate 'search-cli' @('validate', '--config', $c2Good)
        C2Gate 'h66_cli_config_precedes_environment' ($c2Result.rc -eq 0) $c2Result
        $env:WIRI_CONFIG = ''
        $c2Result = C2Validate 'search-niri' @('validate')
        C2Gate 'h66_empty_wiri_uses_niri_config' ($c2Result.rc -ne 0 -and $c2Result.text -match 'child.kdl:1:') $c2Result
        $c2Missing = Join-Path $c2Dir 'missing-explicit.kdl'
        $c2Result = C2Validate 'missing-explicit' @('validate', '-c', $c2Missing)
        C2Gate 'h66_missing_explicit_is_error_not_created' ($c2Result.rc -ne 0 -and -not (Test-Path $c2Missing)) $c2Result
        $c2Text = (& $wiri completions powershell 2>&1 | Out-String)
        $c2Rc = $LASTEXITCODE
        [IO.File]::WriteAllText((Join-Path $c2Dir 'completions.ps1.txt'), $c2Text)
        C2Gate 'h66_powershell_completions_offline' ($c2Rc -eq 0 -and $c2Text -match 'Register-ArgumentCompleter' -and $c2Text -match 'validate') $c2Text
    } catch {
        $c2Details['error'] = [string]$_
        foreach ($c2GateName in @('h66_niri_validate_offline', 'h66_duplicate_both_locations', 'h66_ranges_reject_not_clamp', 'h66_source_labels_help_include_chain', 'h66_wiri_config_precedes_niri_config', 'h66_cli_config_precedes_environment', 'h66_empty_wiri_uses_niri_config', 'h66_missing_explicit_is_error_not_created', 'h66_powershell_completions_offline')) {
            if (-not $summary.Contains($c2GateName)) { C2Gate $c2GateName $false 'phase exception' }
        }
    } finally {
        $env:WIRI_CONFIG = $c2SavedWiriConfig
        $env:NIRI_CONFIG = $c2SavedNiriConfig
        [IO.File]::WriteAllText((Join-Path $c2Dir 'evidence.json'), ($c2Details | ConvertTo-Json -Depth 8))
    }
}

# C1 bounds and C2 global layout reloads on an already-admitted window.
& {
    $crDir = Join-Path $D 'lane-c2-reload'
    $null = New-Item -ItemType Directory -Force $crDir
    $crEvidence = [ordered]@{}
    $crCases = @(
        @{ id='scale'; key='scale'; value='200'; text='output "c1-nonexistent-output" { scale 200; }'; source='scale\s+200'; reason='must be between 0 and 10' },
        @{ id='accel_speed'; key='accel-speed'; value='5'; text='input { mouse { accel-speed 5; }; }'; source='accel-speed\s+5'; reason='must be between -1 and 1' },
        @{ id='negative_percentage'; key='max-scroll-amount'; value='-10%'; text='input { focus-follows-mouse max-scroll-amount="-10%"; }'; source='max-scroll-amount="-10%"'; reason='non-negative finite percentage' }
    )
    $crNames = @('h66_reload_layout_gaps_applied', 'h66_reload_layout_gaps_restored', 'h66_reload_owned_cleanup')
    foreach ($case in $crCases) {
        $crNames += 'h66_range_' + $case.id + '_json'
        $crNames += 'h66_range_' + $case.id + '_live_rejected_keeps_good'
    }
    foreach ($name in $crNames) { $summary[$name] = $false }
    $crProcess = $null
    $crForm = [IntPtr]::Zero
    $crUtf8 = New-Object Text.UTF8Encoding($false)
    $crSavedConfig = [IO.File]::ReadAllBytes($cfg)
    $crLog = Join-Path $crDir 'daemon.stderr.log'
    function CRGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $crEvidence[$name] = [ordered]@{ pass=$pass; observed=$observed }
    }
    function CRConfig([int]$gap) {
        return 'animations { off; }' + "`n" +
            'layout { gaps ' + $gap + '; border { off; }; focus-ring { off; }; shadow { off; }; }' + "`n" +
            'hotkey-overlay { skip-at-startup; }' + "`n" +
            'config-notification { disable-failed; }' + "`n" + 'binds {}' + "`n"
    }
    # Poll IPC and native geometry: >=3 matching samples over >=400ms, within
    # 2500ms. Rejection probes must retain good geometry for the ENTIRE window.
    function CRStable([int]$gap, [switch]$Retained) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $since = -1L; $matches = 0; $everBad = $false
        $samples = New-Object System.Collections.ArrayList
        do {
            $alive = -not $crProcess.HasExited -and [LT]::IsWindow($crForm)
            $r = $null; $managed = $false
            if ($alive) {
                $reply = CtlRaw @('--json', 'window-list')
                if ($reply.rc -eq 0) {
                    $list = $reply.text | ConvertFrom-Json
                    $mine = @($list.result | Where-Object { [int64]$_.hwnd -eq [int64]$crForm })
                    $managed = $list.success -eq $true -and $mine.Count -eq 1 -and
                        -not $mine[0].floating -and -not $mine[0].fullscreen -and -not $mine[0].minimized
                }
                $r = Rc $crForm
            }
            $ok = $alive -and $managed -and $null -ne $r -and $r.W -gt 0 -and
                [Math]::Abs($r.T - ($crArea.Top + $gap)) -le 3 -and
                [Math]::Abs($r.B - ($crArea.Bottom - $gap)) -le 3
            [void]$samples.Add(@{ ms=$clock.ElapsedMilliseconds; alive=$alive; managed=$managed; rect=$r; expected_gap=$gap; match=$ok })
            if ($ok) {
                if ($since -lt 0) { $since = $clock.ElapsedMilliseconds; $matches = 0 }
                $matches++
                if (-not $Retained -and $matches -ge 3 -and ($clock.ElapsedMilliseconds-$since) -ge 400) {
                    return @{ pass=$true; samples=$samples.ToArray() }
                }
            } else { $since = -1; $matches = 0; $everBad = $true }
            Start-Sleep -Milliseconds 100
        } while ($clock.ElapsedMilliseconds -lt 2500)
        return @{ pass=($Retained -and -not $everBad -and $matches -ge 3 -and ($clock.ElapsedMilliseconds-$since) -ge 400); samples=$samples.ToArray() }
    }
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'C1/C2 refuses an existing wiri instance' }
        $crGood = CRConfig 10
        $crProcess = Start-WiriWithConfig $crGood $crLog -NoDefaultBinds
        $null = $crProcess.Handle
        if ($crProcess.HasExited) { throw 'C1/C2 daemon exited at startup' }
        $crForm = [TestForms]::Open('lane-c2-reload-owned', 70, 140, 210)
        FocusForm $crForm
        if (-not (WaitForeground $crForm 200 2500)) { throw 'C1/C2 owned form did not take foreground' }
        $crArea = [System.Windows.Forms.Screen]::FromHandle($crForm).WorkingArea
        $baseline = CRStable 10
        $crEvidence['baseline'] = $baseline
        if (-not $baseline.pass) { throw 'C1/C2 baseline geometry did not settle' }

        foreach ($case in $crCases) {
            # A valid but different gap detects partial application of an invalid
            # candidate: applying ANY of these candidates would move the window.
            $candidate = (CRConfig 60) + $case.text + "`n"
            $path = Join-Path $crDir ($case.id + '.kdl')
            [IO.File]::WriteAllText($path, $candidate, $crUtf8)
            $validation = CtlRaw @('--json', 'validate-config', (Q $path))
            [IO.File]::WriteAllText((Join-Path $crDir ($case.id + '-validate.json')), $validation.text, $crUtf8)
            $json = $validation.text | ConvertFrom-Json
            $errors = @($json.issues | Where-Object {
                $_.severity -eq 'error' -and $_.message -match [regex]::Escape($case.key) -and
                $_.message -match [regex]::Escape($case.reason) -and
                $_.diagnostic -match $case.source -and $_.line -gt 0 -and $_.column -gt 0
            })
            CRGate ('h66_range_' + $case.id + '_json') ($validation.rc -ne 0 -and $json.errors -gt 0 -and $errors.Count -eq 1) $json

            $logBefore = [IO.File]::ReadAllText($crLog).Length
            [IO.File]::WriteAllText($cfg, $candidate, $crUtf8)
            $reload = CtlRaw @('reload-config')
            $kept = CRStable 10 -Retained
            # IPC's summary names the key; the fresh daemon diagnostic includes
            # the rejected value in its source line, not in the short summary.
            $freshLog = [IO.File]::ReadAllText($crLog).Substring($logBefore)
            $diagnostic = $freshLog -match $case.source -and $freshLog -match [regex]::Escape($case.key) -and
                $freshLog -match [regex]::Escape($case.reason)
            CRGate ('h66_range_' + $case.id + '_live_rejected_keeps_good') ($reload.rc -ne 0 -and $reload.text -match [regex]::Escape($case.key) -and $diagnostic -and $kept.pass) @{
                key=$case.key; rejected_value=$case.value; reload=$reload; fresh_log=$freshLog; retained=$kept
            }
        }

        # Same daemon and HWND, no re-admission: both vertical edges move inward
        # by 24px, then return to the previous good geometry (3px tolerance).
        [IO.File]::WriteAllText($cfg, (CRConfig 34), $crUtf8)
        $reload = CtlRaw @('reload-config')
        $changed = CRStable 34
        CRGate 'h66_reload_layout_gaps_applied' ($reload.rc -eq 0 -and $changed.pass) @{ reload=$reload; baseline=$baseline; changed=$changed; hwnd=[int64]$crForm }
        [IO.File]::WriteAllText($cfg, $crGood, $crUtf8)
        $reload = CtlRaw @('reload-config')
        $restored = CRStable 10
        CRGate 'h66_reload_layout_gaps_restored' ($reload.rc -eq 0 -and $restored.pass) @{ reload=$reload; restored=$restored; hwnd=[int64]$crForm }
    } catch {
        $crEvidence['error'] = [string]$_
        foreach ($name in $crNames) { if (-not $crEvidence.Contains($name)) { CRGate $name $false ([string]$_) } }
    } finally {
        $cleanup = New-Object 'Collections.Generic.List[string]'
        if ($crForm -ne [IntPtr]::Zero -and [LT]::IsWindow($crForm)) { [TestForms]::CloseForm($crForm) }
        if ($null -ne $crProcess) {
            if (-not $crProcess.HasExited) {
                try { $null = CtlRaw @('quit') } catch { $cleanup.Add([string]$_) }
                if (-not $crProcess.WaitForExit(3000)) {
                    $cleanup.Add('owned daemon required forced stop')
                    $crProcess.Kill(); $null = $crProcess.WaitForExit(5000)
                }
            } else { $cleanup.Add('owned daemon exited before cleanup') }
            $crProcess.Dispose()
            [IO.File]::WriteAllBytes($cfg, $crSavedConfig)
        }
        CRGate 'h66_reload_owned_cleanup' ($cleanup.Count -eq 0) @($cleanup)
        [IO.File]::WriteAllText((Join-Path $crDir 'evidence.json'), ($crEvidence | ConvertTo-Json -Depth 16), $crUtf8)
    }
}
