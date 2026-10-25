# Parsing the harness schema never executes desktop automation.
function Get-WiriLiveSummaryKeys {
    param([string]$Harness = (Join-Path $PSScriptRoot '..\live-test.ps1'))
    $ErrorActionPreference = 'Stop'
    $tokens = $null; $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($Harness, [ref]$tokens, [ref]$parseErrors)
    if ($parseErrors.Count) { throw 'Cannot parse live harness result schema' }
    $assignments = @($ast.FindAll({ param($node)
        $node -is [Management.Automation.Language.AssignmentStatementAst] -and
        $node.Left -is [Management.Automation.Language.VariableExpressionAst] -and
        $node.Left.VariablePath.UserPath -eq 'summary'
    }, $true))
    if ($assignments.Count -ne 1) { throw 'Expected one static live harness summary' }
    $table = $assignments[0].Right.Find({ param($node)
        $node -is [Management.Automation.Language.HashtableAst]
    }, $true)
    if ($null -eq $table) { throw 'Missing live harness summary schema' }
    $table.KeyValuePairs | ForEach-Object {
        if ($_.Item1 -isnot [Management.Automation.Language.StringConstantExpressionAst]) {
            throw 'Live summary keys must remain literal strings'
        }
        $_.Item1.Value
    }
}

# Pure gate checks, shared by the hosted adapter and fixture tests.
function Get-WiriLiveSummaryFailures {
    param(
        [Parameter(Mandatory = $true)][psobject]$Summary,
        [Parameter(Mandatory = $true)][string[]]$Keys
    )
    $ErrorActionPreference = 'Stop'
    # Missing properties are failures below, not caller-dependent StrictMode exceptions.
    Set-StrictMode -Off
    $failures = New-Object 'System.Collections.Generic.List[string]'
    $names = @($Summary.PSObject.Properties.Name)
    foreach ($key in $keys) {
        if ($names -notcontains $key) { $failures.Add("missing gate/metric: $key") }
    }
    # Descriptive timing maxima and counters remain informational. These are the smoke-test counts.
    $counts = @{
        ctl_timeouts = 0; unknown_key_warnings = 0; config_bind_warnings = 0
        forms_overlapping_pairs = 0; settle_positioning_per_s = 0; terminals_settle_positioning_per_s = 0
        ctl_failures_while_an_app_hung = 0; ctl_output_files_with_non_ascii = 0
        wiri_visible_windows_of_its_own = 0; wiri_new_console_windows_at_start = 0; wiri_exe_titled_windows_listed = 0
        wiri_subsystem = 2; wiri_ctl_subsystem = 3; test_windows_moved_by_wiri = 3; forms_inside_work_area = 3
        forms_on_screen_after_terminals_closed = 3; windows_hidden_on_workspace_2 = 3
        windows_visible_back_on_workspace_1 = 3; windows_hidden_while_wiri_dead = 3; windows_visible_after_restart = 3
        hotkey_workspace_hidden = 3; hotkey_workspace_back_visible = 3
        h1_unknown_modifier_warnings = 1; h1_unsupported_modifier_warnings = 1; h1_rejected_binds_registered = 0
        h7_config_bind_warnings = 0; h8_config_bind_warnings = 0
        h9_failed_default_hotkeys = 0; h9_overlapping_terminal_pairs = 0; hung_window_is_managed_afterwards = 1
    }
    function Test-Number($Value) {
        return ($Value -is [int] -or $Value -is [long] -or $Value -is [double] -or $Value -is [decimal])
    }
    foreach ($gate in $counts.GetEnumerator()) {
        $value = $Summary.($gate.Key)
        if (-not (Test-Number $value) -or $value -ne $gate.Value) { $failures.Add($gate.Key) }
    }
    $wtGate = 'h2_wt_exe_found_on_path'
    $skips = @($Summary.ci_skips)
    $validWtSkip = $skips.Count -eq 1 -and $skips[0].gate -is [string] -and $skips[0].gate -ceq $wtGate -and
        $skips[0].reason -is [string] -and -not [string]::IsNullOrWhiteSpace($skips[0].reason) -and
        $null -eq $Summary.$wtGate
    if ($names -notcontains 'ci_skips' -or ($skips.Count -gt 0 -and -not $validWtSkip)) {
        $failures.Add('invalid/missing ci_skips (only the explicit missing-WT gate may be skipped)')
    }
    $booleanGates = @('wiri_exited_on_quit', 'wiri_log_exists', 'hidden_json_written_before_kill',
        'hidden_json_present_after_restart', 'escape_reaches_applications', 'overview_escape_responsive')
    $booleanGates += @($keys | Where-Object { $_ -match '^h[0-9]+_' -and -not $counts.ContainsKey($_) })
    foreach ($name in $booleanGates) {
        if ($name -ceq $wtGate -and $validWtSkip) { continue }
        $value = $Summary.$name
        $expected = $name -ne 'hidden_json_present_after_restart'
        if ($value -isnot [bool] -or $value -ne $expected) { $failures.Add($name) }
    }
    if (-not (Test-Number $Summary.windows_expected_after_quit) -or $Summary.windows_expected_after_quit -le 0 -or
        -not (Test-Number $Summary.windows_visible_after_quit) -or
        $Summary.windows_visible_after_quit -ne $Summary.windows_expected_after_quit) {
        $failures.Add('windows_visible_after_quit')
    }
    if ($null -eq $Summary.steps -or @($Summary.steps).Count -eq 0) { $failures.Add('missing steps') }
    foreach ($step in $Summary.steps) {
        $runningStart = $step.step -is [string] -and $step.rc -is [string] -and
            $step.step -ceq 'start' -and $step.rc -ceq 'running'
        if ($step.step -isnot [string] -or [string]::IsNullOrWhiteSpace($step.step) -or
            (-not $runningStart -and (-not (Test-Number $step.rc) -or $step.rc -ne 0))) {
            $failures.Add("step: $($step.step) (rc=$($step.rc))")
        }
    }
    return $failures.ToArray()
}
