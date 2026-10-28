# Lane C2: config diagnostics and offline/dry-run checks. No machine settings.
# Dot-sourced with the desktop lock held. This phase starts no foreign UI/process.
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
