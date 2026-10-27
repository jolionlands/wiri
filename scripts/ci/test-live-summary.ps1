# Fixture-only checks. Never starts wiri, opens test windows or injects keys.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'live-summary.ps1')
$harness = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\live-test.ps1'))
$keys = @(Get-WiriLiveSummaryKeys -Harness $harness)
$fixture = [ordered]@{}
foreach ($name in $keys) {
    $fixture[$name] = if ($name -match '^h[0-9]+_') { $true } else { 0 }
}
foreach ($name in 'wiri_exited_on_quit', 'wiri_log_exists', 'hidden_json_written_before_kill',
    'escape_reaches_applications', 'overview_escape_responsive') { $fixture[$name] = $true }
$fixture.hidden_json_present_after_restart = $false
$expectedCounts = @{
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
foreach ($pair in $expectedCounts.GetEnumerator()) { $fixture[$pair.Key] = $pair.Value }
$fixture.windows_expected_after_quit = 2
$fixture.windows_visible_after_quit = 2
$fixture.steps = @([pscustomobject]@{ step = 'start'; rc = 'running' }, [pscustomobject]@{ step = '01-state'; rc = 0 })
$fixture.ci_skips = @()
$json = ([pscustomobject]$fixture) | ConvertTo-Json -Depth 8
function New-Summary { return ($json | ConvertFrom-Json) }
function Assert-Valid($Summary) {
    $failures = @(Get-WiriLiveSummaryFailures -Summary $Summary -Keys $keys)
    if ($failures.Count) { throw "Expected valid summary: $($failures -join ', ')" }
}
function Assert-Invalid($Summary, [string]$Contains) {
    $failures = @(Get-WiriLiveSummaryFailures -Summary $Summary -Keys $keys)
    if (-not @($failures | Where-Object { $_.Contains($Contains) }).Count) {
        throw "Expected failure containing '$Contains', got: $($failures -join ', ')"
    }
}
Assert-Valid (New-Summary)
# Every phase boolean is required, typed and true; one restart-record gate must instead be false.
$s = New-Summary; $s.h3_float_keeps_foreground = $false; Assert-Invalid $s 'h3_float_keeps_foreground'
$s = New-Summary; $s.PSObject.Properties.Remove('h4_all_restored'); Assert-Invalid $s 'h4_all_restored'
$s = New-Summary; $s.h5_float_closed = $null; Assert-Invalid $s 'h5_float_closed'
$s = New-Summary; $s.h8_auto_back_and_forth = 'true'; Assert-Invalid $s 'h8_auto_back_and_forth'
$s = New-Summary; $s.hidden_json_present_after_restart = $true; Assert-Invalid $s 'hidden_json_present_after_restart'
$s = New-Summary; $s.wiri_exited_on_quit = $null; Assert-Invalid $s 'wiri_exited_on_quit'
foreach ($name in $expectedCounts.Keys) {
    $s = New-Summary; $s.$name = 99; Assert-Invalid $s $name
    $s = New-Summary; $s.$name = $null; Assert-Invalid $s $name
}
$s = New-Summary; $s.ctl_timeouts = $false; Assert-Invalid $s 'ctl_timeouts'
$s = New-Summary; $s.ctl_timeouts = '0'; Assert-Invalid $s 'ctl_timeouts'
$s = New-Summary; $s.windows_visible_after_quit = 1; Assert-Invalid $s 'windows_visible_after_quit'
$s = New-Summary; $s.windows_expected_after_quit = 0; Assert-Invalid $s 'windows_visible_after_quit'
$s = New-Summary; $s.steps = @(); Assert-Invalid $s 'missing steps'
$s = New-Summary; $s.steps[1].rc = $null; Assert-Invalid $s 'step: 01-state'
$s = New-Summary; $s.steps[1].rc = 1; Assert-Invalid $s 'step: 01-state'
$s = New-Summary; $s.steps[1].rc = 'running'; Assert-Invalid $s 'step: 01-state'
$s = New-Summary; $s.steps[0].rc = @('running', 1); Assert-Invalid $s 'step: start'
$s = New-Summary; $s.steps[0].step = @('start', '01-state'); Assert-Invalid $s 'step:'
# Only a null WT gate with one explicit explanation may be skipped. No false gate can hide behind it.
$s = New-Summary; $s.h2_wt_exe_found_on_path = $null
$s.ci_skips = @([pscustomobject]@{ gate = 'h2_wt_exe_found_on_path'; reason = 'wt.exe missing' })
Assert-Valid $s
$s.ci_skips += $s.ci_skips[0]; Assert-Invalid $s 'invalid/missing ci_skips'
$s.ci_skips = @($s.ci_skips[0]); $s.ci_skips[0].reason = ' '; Assert-Invalid $s 'invalid/missing ci_skips'
$s.ci_skips[0].reason = 'wt.exe missing'; $s.ci_skips[0].gate = @('h2_wt_exe_found_on_path', 'other')
Assert-Invalid $s 'invalid/missing ci_skips'
$s = New-Summary; $s.h2_wt_exe_found_on_path = $null; Assert-Invalid $s 'h2_wt_exe_found_on_path'
$s = New-Summary; $s.ci_skips = @([pscustomobject]@{ gate = 'h3_float_keeps_foreground'; reason = 'not a dependency skip' })
Assert-Invalid $s 'invalid/missing ci_skips'
$s = New-Summary; $s.PSObject.Properties.Remove('ci_skips'); Assert-Invalid $s 'invalid/missing ci_skips'
# Live phases are dot-sourced into the parent's dynamic scope, and PowerShell variable names are
# case-insensitive: a phase variable named $d (a form handle in lane T's first version) silently
# redirects every Ctl/Shot/CloseFocused output path, which all read the harness directory $D.
# Lane W overrides $D on purpose (exactly "D") so the shared helpers write into its own folder.
function Get-WiriAssignedVariable {
    param($Node)
    if ($Node -is [Management.Automation.Language.AssignmentStatementAst]) {
        $target = $Node.Left
        while ($target -is [Management.Automation.Language.ConvertExpressionAst]) { $target = $target.Child }
        if ($target -is [Management.Automation.Language.VariableExpressionAst]) { return $target.VariablePath.UserPath }
    } elseif ($Node -is [Management.Automation.Language.ForEachStatementAst]) {
        return $Node.Variable.VariablePath.UserPath
    } elseif ($Node -is [Management.Automation.Language.ParameterAst]) {
        return $Node.Name.VariablePath.UserPath
    }
    return $null
}
function Get-WiriHarnessDirShadows {
    param([Parameter(Mandatory = $true)]$Ast, [switch]$AllowUpperD)
    $found = @($Ast.FindAll({ param($node)
        $name = Get-WiriAssignedVariable $node
        $null -ne $name -and $name -ieq 'D'
    }, $true))
    @($found | Where-Object {
        $name = Get-WiriAssignedVariable $_
        -not ($AllowUpperD -and $_ -is [Management.Automation.Language.AssignmentStatementAst] -and $name -ceq 'D')
    })
}
$synthetic = [Management.Automation.Language.Parser]::ParseInput('& { $tA = 1; $d = 2; foreach ($D in 1) { } }', [ref]$null, [ref]$null)
if (@(Get-WiriHarnessDirShadows -Ast $synthetic).Count -ne 2) { throw 'Shadow lint missed a synthetic $d assignment or foreach variable' }
$synthetic = [Management.Automation.Language.Parser]::ParseInput('function F($d) { }; & { $tD = 1; $D = Join-Path $D "x" }', [ref]$null, [ref]$null)
if (@(Get-WiriHarnessDirShadows -Ast $synthetic).Count -ne 2) { throw 'Shadow lint missed a $d parameter or an uppercase override' }
if (@(Get-WiriHarnessDirShadows -Ast $synthetic -AllowUpperD).Count -ne 1) { throw 'Shadow lint must still reject a $d parameter when uppercase overrides are allowed' }
foreach ($phase in Get-ChildItem -Path (Join-Path $PSScriptRoot '..\live-phases') -Filter '*.ps1') {
    $parseErrors = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile($phase.FullName, [ref]$null, [ref]$parseErrors)
    if ($parseErrors.Count) { throw "Cannot parse $($phase.Name): $($parseErrors[0].Message)" }
    $shadows = @(Get-WiriHarnessDirShadows -Ast $ast -AllowUpperD:($phase.Name -eq 'lane-w.ps1'))
    if ($shadows.Count) {
        throw "$($phase.Name) line $($shadows[0].Extent.StartLineNumber) shadows the harness directory `$D (variable names are case-insensitive); rename it with a lane prefix"
    }
}
Write-Host 'Live phases do not shadow the harness directory $D.'
Write-Host 'Live-summary fixture tests passed (no desktop automation).'
