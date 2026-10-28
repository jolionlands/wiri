# Lane A / H17: OFFLINE animation config validation only.
# Append to lane-a.ps1 or dot-source after $summary, $D, $ctl, Q and CtlRaw exist,
# before the parent writes summary.json. The parent retains the desktop lock.
# No daemon, HWND, input, display setting, live config or IPC request is touched.
# CtlRaw owns its short-lived validate-config process and its timeout handling.
# Fixtures/diagnostics are retained in a unique H17 artifact directory under $D.
# These gates do NOT prove decoded defaults, solver behaviour, reload, visible motion,
# frame pacing, idle CPU, resize cross-fades or a two-workspace slide.

function H17Case([string]$name, [string]$config, [int]$expectedRc = 0,
    [string[]]$patterns = @(), [bool]$warnings = $false) {
    return [pscustomobject]@{
        name = $name; gate = "h17_config_$name"; config = $config
        expected_rc = $expectedRc; patterns = $patterns; warnings = $warnings
    }
}

$h17Cases = @(
    H17Case 'absent_section_accepted' '// H17: animation section intentionally absent.'
    H17Case 'empty_section_accepted' 'animations {}'
    H17Case 'global_controls_accepted' 'animations { off false; on; enabled true; enable true; slowdown 2.5; }'
    H17Case 'global_off_accepted' 'animations { off; }'
    H17Case 'legacy_flags_and_resize_block_accepted' @'
animations {
    enabled true; duration 250; easing "ease-in-out"
    workspace-transition true; focus-transition false; window-move true
    window-resize false; popup false; shadow false
    window-resize { duration-ms 200; curve "linear"; }
}
'@
    H17Case 'per_animation_springs_accepted' @'
animations {
    workspace-switch { spring damping-ratio=0.7 stiffness=1000 epsilon=0.0001; }
    horizontal-view-movement { spring damping-ratio=1 stiffness=800 epsilon=0.0001; }
    window-movement { spring damping-ratio=2 stiffness=800 epsilon=0.0001; }
    window-resize { spring damping-ratio=1 stiffness=800 epsilon=0.0001; }
}
'@
    H17Case 'global_spring_with_easing_override_accepted' @'
animations {
    spring damping-ratio=0.8 stiffness=900 epsilon=0.0001
    workspace-switch { duration-ms 240; curve "ease-out-cubic"; }
}
'@
    H17Case 'named_easing_curves_accepted' @'
animations {
    workspace-switch { duration-ms 300; curve "linear"; }
    horizontal-view-movement { duration-ms 240; curve "ease-out-quad"; }
    window-movement { duration-ms 200; curve "ease-out-cubic"; }
    window-resize { duration-ms 180; curve "ease-out-expo"; }
}
'@
    H17Case 'curve_only_before_global_duration_accepted' @'
animations {
    workspace-switch { curve "linear"; }
    horizontal-view-movement { easing "ease-out-quad"; }
    window-movement { curve "ease-out-cubic"; }
    window-resize { curve "ease-out-expo"; }
    duration-ms 450
}
'@
    H17Case 'per_animation_off_accepted' @'
animations {
    workspace-switch { off; }
    horizontal-view-movement { off; }
    window-movement { off; }
    window-resize { off; }
}
'@
    # Acceptance of this historical name is NOT evidence of a Bezier solver.
    H17Case 'legacy_curve_aliases_accepted' @'
animations {
    easing "none"
    workspace-switch { easing "ease-in"; }
    horizontal-view-movement { easing "ease-out"; }
    window-movement { easing "ease-in-out"; }
    window-resize { easing "cubic-bezier"; }
}
'@
    H17Case 'numeric_limits_accepted' @'
animations {
    slowdown 1000; duration-ms 60000; curve "linear"
    workspace-switch { spring damping-ratio=0 stiffness=0.000001 epsilon=0.000000000001; }
    horizontal-view-movement { spring damping-ratio=100 stiffness=1000000000 epsilon=1; }
    window-movement { duration-ms 0; curve "linear"; }
}
'@
    H17Case 'zero_slowdown_accepted' 'animations { slowdown 0; }'

    H17Case 'negative_slowdown_rejected' 'animations { slowdown -1; }' 1 @('slowdown.*must be between')
    H17Case 'excessive_slowdown_rejected' 'animations { slowdown 1001; }' 1 @('slowdown.*must be between')
    H17Case 'nonfinite_slowdown_rejected' 'animations { slowdown "NaN"; }' 1 @('slowdown.*must be between')
    H17Case 'negative_duration_rejected' 'animations { window-movement { duration-ms -1; } }' 1 @('duration-ms.*must be between')
    H17Case 'excessive_duration_rejected' 'animations { duration-ms 60001; }' 1 @('duration-ms.*must be between')
    H17Case 'negative_damping_rejected' 'animations { spring damping-ratio=-0.1; }' 1 @('spring.*must be between')
    H17Case 'excessive_damping_rejected' 'animations { spring damping-ratio=100.1; }' 1 @('spring.*must be between')
    H17Case 'zero_stiffness_rejected' 'animations { window-resize { spring stiffness=0; } }' 1 @('spring.*must be between')
    H17Case 'excessive_stiffness_rejected' 'animations { spring stiffness=1000000001; }' 1 @('spring.*must be between')
    H17Case 'zero_epsilon_rejected' 'animations { spring epsilon=0; }' 1 @('spring.*must be between')
    H17Case 'excessive_epsilon_rejected' 'animations { spring epsilon=1.01; }' 1 @('spring.*must be between')
    H17Case 'nonfinite_spring_rejected' 'animations { spring stiffness="NaN"; }' 1 @('spring.*must be between')
    H17Case 'unknown_curve_rejected' 'animations { window-resize { curve "h17-not-a-curve"; } }' 1 @('unknown animation curve', 'h17-not-a-curve')
    H17Case 'unknown_animation_rejected' 'animations { window-movment {} }' 1 @('window-movment')
    H17Case 'unknown_override_option_rejected' 'animations { window-resize { durtion-ms 120; } }' 1 @('durtion-ms')
    H17Case 'unknown_spring_property_rejected' 'animations { spring mystery=1; }' 1 @('unknown spring property', 'mystery')
    H17Case 'duplicate_spring_property_rejected' 'animations { spring damping-ratio=0.7 damping_ratio=0.8; }' 1 @('duplicate spring property')
    H17Case 'duplicate_duration_alias_rejected' 'animations { duration 100; duration-ms 200; }' 1 @('duplicate animation option')
    H17Case 'duplicate_animation_block_rejected' 'animations { window-movement { off; }; window-movement { off; }; }' 1 @('duplicate animation option')
    H17Case 'duplicate_curve_alias_rejected' 'animations { window-resize { curve "linear"; easing "ease-out-expo"; } }' 1 @('duplicate animation option')
    H17Case 'global_spring_easing_conflict_rejected' 'animations { spring; duration-ms 200; }' 1 @('cannot combine spring')
    H17Case 'override_spring_easing_conflict_rejected' 'animations { workspace-switch { spring; curve "linear"; } }' 1 @('cannot combine spring')

    H17Case 'window_open_close_options_accepted' 'animations { window-open {}; window-close {}; }'
    # Lane V implements this camera block; keep a clean fixture as well as
    # the unchanged warnings for compositor-only effects below.
    H17Case 'overview_camera_accepted' 'animations { overview-open-close { spring damping-ratio=1 stiffness=800 epsilon=0.0001; }; }'
    # Lane A implements the recent-windows overlay fade (animations_decode.rs
    # `recent-windows-close` -> AnimationsConfig::recent_windows_close), so it must now
    # validate clean, with an explicit duration/curve proving the block is decoded.
    H17Case 'recent_windows_close_accepted' 'animations { recent-windows-close { duration-ms 250; curve "ease-out-expo"; }; }'
    # Only the three compositor-only effects wiri still has no compositor for warn.
    # The two implemented blocks (overview-open-close, recent-windows-close) have their
    # own clean fixtures above, which fail on any warning.
    H17Case 'compositor_options_warn_not_implemented' @'
animations {
    config-notification-open-close {}
    exit-confirmation-open-close {}
    screenshot-ui-open {}
}
'@ 0 @('config-notification-open-close',
        'exit-confirmation-open-close', 'screenshot-ui-open',
        'composited open/close animations') $true
)

foreach ($h17Case in $h17Cases) { $summary[$h17Case.gate] = $false }
$summary.h17_config_fragment_completed = $false
$h17Results = @()
$h17ArtifactDir = $null
$h17Utf8 = New-Object System.Text.UTF8Encoding($false)
try {
    if (-not (Get-Command CtlRaw -ErrorAction SilentlyContinue) -or
        -not (Get-Command Q -ErrorAction SilentlyContinue)) {
        throw 'H17 requires the parent Q and CtlRaw helpers.'
    }
    $h17ArtifactDir = [System.IO.Path]::GetFullPath((Join-Path $D ('h17-animations-' + [guid]::NewGuid().ToString('N'))))
    [void][System.IO.Directory]::CreateDirectory($h17ArtifactDir)
    [System.IO.File]::WriteAllText((Join-Path $h17ArtifactDir 'scope.txt'), @'
H17 validates isolated animation config fixtures with wiri-ctl validate-config, before IPC.
Each gate means ONLY that the expected exit code and diagnostics were observed.
The manifest records the fixture, expected/observed exit codes, patterns and any exception.
Clean fixtures must print "ok (no issues)". Rejections must name this fixture:line:column.
The compositor-options fixture must warn without errors; warnings do NOT implement effects.
No daemon or desktop resources are created, reconfigured or closed by this fragment.

UNVERIFIED: normalized defaults and curve-only inheritance, spring/easing math, velocity
handoff/retargeting, actual slowdown/off behaviour, startup/reload propagation, backend
SetWindowPos frames, idle scheduling, cadence/vsync and visible workspace/view/move/resize.
COMPOSITOR GAPS: incoming-only workspace slide, no outgoing-workspace slide, no resize
content cross-fade, no open/close/overview/recent/UI compositing or custom shaders, and
legacy cubic-bezier is a cubic-out alias rather than a control-point Bezier curve.
This fragment is exercised by the parent harness. Separate h17 motion gates in
lane-a-animation-motion.ps1 observe HWND resize, move, workspace slide and reload-off;
config acceptance here does not establish those visual results. See the Phase 2B report.
'@, $h17Utf8)

    foreach ($h17Case in $h17Cases) {
        $h17Path = Join-Path $h17ArtifactDir ($h17Case.name + '.kdl')
        $h17Diagnostics = Join-Path $h17ArtifactDir ($h17Case.name + '.diagnostics.txt')
        $h17Record = [ordered]@{
            gate = $h17Case.gate; fixture = $h17Path; diagnostics = $h17Diagnostics
            expected_rc = $h17Case.expected_rc; observed_rc = $null
            warnings_expected = $h17Case.warnings; required_patterns = $h17Case.patterns
            passed = $false; exception = $null
        }
        try {
            [System.IO.File]::WriteAllText($h17Path, ($h17Case.config + "`r`n"), $h17Utf8)
            $h17Raw = CtlRaw @('validate-config', (Q $h17Path))
            $h17Record.observed_rc = $h17Raw.rc
            $h17Text = [string]$h17Raw.text
            [System.IO.File]::WriteAllText($h17Diagnostics, $h17Text, $h17Utf8)
            # A timeout, missing binary, generic usage error or unrelated diagnostic cannot pass.
            $h17Ok = ($null -ne $h17Raw.rc) -and ($h17Raw.rc -eq $h17Case.expected_rc)
            $h17Located = [regex]::Escape($h17Path) + ':[1-9][0-9]*:[1-9][0-9]*:\s*'
            if ($h17Case.expected_rc -eq 1) {
                $h17Ok = $h17Ok -and ($h17Text -match ($h17Located + 'error:'))
            } elseif ($h17Case.warnings) {
                $h17Ok = $h17Ok -and ($h17Text -match ($h17Located + 'warning:')) -and
                    ($h17Text -match ': 0 error\(s\), [1-9][0-9]* warning\(s\)') -and
                    ($h17Text -notmatch ':\s*error:')
            } else {
                $h17Ok = $h17Ok -and $h17Text.Contains($h17Path + ': ok (no issues)') -and
                    ($h17Text -notmatch ':\s*(error|warning):')
            }
            foreach ($h17Pattern in $h17Case.patterns) {
                $h17Ok = $h17Ok -and ($h17Text -match $h17Pattern)
            }
            $h17Record.passed = [bool]$h17Ok
            $summary[$h17Case.gate] = [bool]$h17Ok
        } catch {
            $h17Record.exception = ($_ | Out-String)
            [System.IO.File]::WriteAllText($h17Diagnostics, $h17Record.exception, $h17Utf8)
        }
        $h17Results += [pscustomobject]$h17Record
    }
    [System.IO.File]::WriteAllText((Join-Path $h17ArtifactDir 'manifest.json'),
        (ConvertTo-Json -InputObject $h17Results -Depth 6), $h17Utf8)
    $summary.h17_config_fragment_completed = ($h17Results.Count -eq $h17Cases.Count)
} catch {
    $_ | Out-String | Set-Content -Encoding UTF8 (Join-Path $D 'h17-error.txt')
}
# No cleanup: only retained artifacts and CtlRaw-owned short-lived CLI processes exist.
