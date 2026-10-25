# Lane A item 6 / H21: Important Hotkeys. Integrate after the harness helpers and $summary exist,
# with no daemon running (the Phase 2B lane owns its daemon). Dot-source this fragment or append
# it to lane-a.ps1. Only H21's daemon and key-recorder form are stopped/closed here.
#
# Geometry is measured in physical pixels at the REAL session DPI. Run the parent harness on
# a 1366x768 desktop and again at the required scaled DPI(s); this fragment does not change
# display settings, simulate a smaller work area, or claim an unobserved DPI was tested.
#
# The overlay body is owner-drawn, not accessible through GetWindowText. The title-property
# gates below compare actual screenshots, with same-config controls. They prove that changing
# the string / setting null affects rendering, not OCR correctness. Review the saved PNGs for
# the exact custom text and the missing close-window row (see h21-visual-review.txt).

function H21Windows($proc) {
    if ($null -eq $proc) { return }
    foreach ($h in [LT]::TopLevel()) {
        if ([LT]::Pid($h) -eq $proc.Id -and [LT]::Class($h) -eq 'WiriBindingsCheatsheet') { $h }
    }
}
function H21WaitShown($proc) {
    for ($i = 0; $i -lt 40; $i++) {
        $windows = @(H21Windows $proc)
        if ($windows.Count -eq 1 -and [LT]::IsWindowVisible($windows[0]) -and -not [LT]::IsIconic($windows[0])) { return $windows[0] }
        Start-Sleep -Milliseconds 100
    }
    return [IntPtr]::Zero
}
function H21WaitDestroyed($proc, [IntPtr]$old) {
    if ($old -eq [IntPtr]::Zero) { return $false }
    for ($i = 0; $i -lt 40; $i++) {
        # A hidden window is NOT a pass: the lazily created HWND must be destroyed.
        if (-not [LT]::IsWindow($old) -and @(H21Windows $proc).Count -eq 0) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
function H21Show($proc, [string]$tag, [bool]$realKeys = $false) {
    if ($realKeys) { Chord 'ctrl+alt+f1' }
    else { $null = Ctl $tag @('action', 'show-hotkey-overlay') }
    $h = H21WaitShown $proc
    Start-Sleep -Milliseconds 350
    return $h
}
function H21Rect([int]$l, [int]$t, [int]$r, [int]$b) {
    return [pscustomobject]@{ left = $l; top = $t; right = $r; bottom = $b; width = ($r - $l); height = ($b - $t) }
}
function H21Capture([string]$tag, [IntPtr]$h) {
    # Shot and the bounds queries run in the same per-monitor-aware thread context.
    Shot $tag
    if ($h -eq [IntPtr]::Zero -or -not [LT]::IsWindow($h)) { return $null }
    $r = New-Object LT+RECT
    if (-not [LT]::GetWindowRect($h, [ref]$r)) { return $null }
    $v = [LT]::Visible($h)
    $screen = [System.Windows.Forms.Screen]::FromHandle($h)
    $wa = $screen.WorkingArea; $sb = $screen.Bounds
    $fits = ($r.R -gt $r.L -and $r.B -gt $r.T -and $v.R -gt $v.L -and $v.B -gt $v.T -and
        $r.L -ge $wa.Left -and $r.T -ge $wa.Top -and $r.R -le $wa.Right -and $r.B -le $wa.Bottom -and
        $v.L -ge $wa.Left -and $v.T -ge $wa.Top -and $v.R -le $wa.Right -and $v.B -le $wa.Bottom)
    $capture = [pscustomobject]@{
        hwnd = [int64]$h; process_id = [LT]::Pid($h)
        class = [LT]::Class($h); window_title = [LT]::Title($h) # Caption only; never the drawn rows.
        window_dpi = [H21Native]::GetDpiForWindow($h)
        raw_bounds = (H21Rect $r.L $r.T $r.R $r.B)
        visible_bounds = (H21Rect $v.L $v.T $v.R $v.B)
        monitor = $screen.DeviceName
        monitor_bounds = (H21Rect $sb.Left $sb.Top $sb.Right $sb.Bottom)
        work_area = (H21Rect $wa.Left $wa.Top $wa.Right $wa.Bottom)
        observed_1366x768 = ($sb.Width -eq 1366 -and $sb.Height -eq 768)
        fits_work_area = $fits; visible = [LT]::IsWindowVisible($h)
        cloaked = [LT]::Cloaked($h)
        crop_file = "$tag-overlay.png"; crop_sha256 = $null
    }
    $desktop = $null; $crop = $null
    try {
        $desktop = New-Object System.Drawing.Bitmap (Join-Path $D "$tag.png")
        $vs = [System.Windows.Forms.SystemInformation]::VirtualScreen
        $box = New-Object System.Drawing.Rectangle ($r.L - $vs.Left), ($r.T - $vs.Top), ($r.R - $r.L), ($r.B - $r.T)
        # Do not clamp an overflowing overlay into a misleading, apparently complete screenshot.
        if ($box.Width -gt 0 -and $box.Height -gt 0 -and $box.Left -ge 0 -and $box.Top -ge 0 -and
            $box.Right -le $desktop.Width -and $box.Bottom -le $desktop.Height) {
            $crop = $desktop.Clone($box, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
            $cropPath = Join-Path $D $capture.crop_file
            $crop.Save($cropPath, [System.Drawing.Imaging.ImageFormat]::Png)
            $capture.crop_sha256 = (Get-FileHash -LiteralPath $cropPath -Algorithm SHA256).Hash
        }
    } finally {
        if ($null -ne $crop) { $crop.Dispose() }
        if ($null -ne $desktop) { $desktop.Dispose() }
    }
    $capture | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 (Join-Path $D "$tag-bounds.json")
    return $capture
}
function H21SameImage($a, $b) {
    return ($null -ne $a -and $null -ne $b -and $a.crop_sha256 -and $b.crop_sha256 -and
        $a.crop_sha256 -eq $b.crop_sha256 -and $a.raw_bounds.width -eq $b.raw_bounds.width -and
        $a.raw_bounds.height -eq $b.raw_bounds.height -and $a.window_dpi -eq $b.window_dpi)
}

$h21GateNames = @(
    'h21_no_overlay_window_until_show', 'h21_show_hotkey_overlay_action_shows_one_window',
    'h21_overlay_window_title_is_important_hotkeys', 'h21_bounds_fit_work_area_at_observed_dpi',
    'h21_letter_key_destroys_overlay', 'h21_dismissal_letter_is_consumed',
    'h21_real_chord_recreates_overlay', 'h21_modifier_keydown_destroys_overlay',
    'h21_alt_keydown_destroys_overlay', 'h21_escape_destroys_overlay', 'h21_wiri_answers_after_dismissal',
    'h21_all_title_configs_show_and_fit', 'h21_screenshot_controls_are_stable',
    'h21_custom_title_string_changes_drawn_pixels', 'h21_null_title_changes_drawn_pixels',
    'h21_no_overlay_windows_after_quit', 'h21_fragment_completed'
)
foreach ($h21Gate in $h21GateNames) { $summary[$h21Gate] = $false }
$h21Proc = $null; $h21Input = [IntPtr]::Zero; $h21OldDpiContext = [IntPtr]::Zero
$h21Pids = @(); $h21Shots = @{}; $h21Repeats = @{}
# Override the actual default close-window chord, not a second chord that a curated core row
# might ignore. Only this property's value changes between screenshot configurations.
$h21Template = @'
input { mod-key "ctrl-alt"; }
animations { off; }
binds {
    Mod+F1 { show-hotkey-overlay; }
    Mod+Q __H21_TITLE__ { close-window; }
}
'@
$h21Variants = @(
    [pscustomobject]@{ name = 'title-a'; property = 'hotkey-overlay-title="H21 VISIBLE TITLE ALPHA"' },
    [pscustomobject]@{ name = 'title-b'; property = 'hotkey-overlay-title="H21 VISIBLE TITLE BRAVO"' },
    [pscustomobject]@{ name = 'null'; property = 'hotkey-overlay-title=null' },
    [pscustomobject]@{ name = 'unset-return'; property = '' }
)
try {
    # Inline observer-only interop, not a module or a new input implementation. Restored in finally.
    if (-not ('H21Native' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class H21Native {
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
}
'@ -ErrorAction Stop
    }
    $h21OldDpiContext = [H21Native]::SetThreadDpiAwarenessContext([IntPtr](-4)) # DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2
    if ($h21OldDpiContext -eq [IntPtr]::Zero) { throw 'Cannot establish per-monitor-v2 observation context; physical-pixel geometry is not trustworthy.' }

    $h21Config = $h21Template.Replace('__H21_TITLE__', '')
    [IO.File]::WriteAllText((Join-Path $D 'h21-unset-config.kdl'), $h21Config)
    $h21Proc = Start-WiriWithConfig $h21Config (Join-Path $D 'wiri.stderr21-unset.log')
    $h21Pids += $h21Proc.Id
    $summary.h21_no_overlay_window_until_show = (-not $h21Proc.HasExited -and @(H21Windows $h21Proc).Count -eq 0)
    $h21Input = [TestForms]::OpenKeys('h21-key-recorder')
    Start-Sleep -Milliseconds 1400
    $null = Ctl 'h21-focus-input' @('focus-window', [string][int64]$h21Input)
    Start-Sleep -Milliseconds 500

    $h21Hwnd = H21Show $h21Proc 'h21-show-action'
    $h21Shots['unset'] = H21Capture 'h21-01-unset' $h21Hwnd
    $h21Repeats['unset'] = H21Capture 'h21-01-unset-repeat' $h21Hwnd
    $h21ShowStep = @($steps | Where-Object { $_.step -eq 'h21-show-action' } | Select-Object -Last 1)
    $summary.h21_show_hotkey_overlay_action_shows_one_window = ($h21Hwnd -ne [IntPtr]::Zero -and
        @(H21Windows $h21Proc).Count -eq 1 -and $h21ShowStep.Count -eq 1 -and $h21ShowStep[0].rc -eq 0)
    $summary.h21_overlay_window_title_is_important_hotkeys = ($h21Hwnd -ne [IntPtr]::Zero -and [LT]::Title($h21Hwnd) -eq 'Important Hotkeys')
    $summary.h21_bounds_fit_work_area_at_observed_dpi = ($null -ne $h21Shots['unset'] -and
        $h21Shots['unset'].window_dpi -ge 96 -and $h21Shots['unset'].fits_work_area -and $h21Shots['unset'].cloaked -eq 0)

    [TestForms]::ClearKeys()
    Press 'a'
    $summary.h21_letter_key_destroys_overlay = H21WaitDestroyed $h21Proc $h21Hwnd
    $h21KeyLog = [TestForms]::KeyLog()
    Set-Content -Encoding UTF8 (Join-Path $D 'h21-letter-key-log.txt') $h21KeyLog
    # The dismissal key is consumed, not forwarded to the application.
    $summary.h21_dismissal_letter_is_consumed = ($summary.h21_letter_key_destroys_overlay -and
        [LT]::GetForegroundWindow() -eq $h21Input -and $h21KeyLog -notmatch '(^|,)A(,|$)')
    Shot 'h21-02-letter-dismissed'

    # Do not demand a different numeric HWND: Windows may reuse a destroyed handle.
    $h21Hwnd = H21Show $h21Proc 'h21-real-show-chord' $true
    $summary.h21_real_chord_recreates_overlay = ($summary.h21_letter_key_destroys_overlay -and
        $h21Hwnd -ne [IntPtr]::Zero -and [LT]::Title($h21Hwnd) -eq 'Important Hotkeys')
    $null = H21Capture 'h21-03-real-chord' $h21Hwnd
    try {
        [LT]::Key([byte]0x10, $false) # Bare Shift, still held when destruction is checked.
        $summary.h21_modifier_keydown_destroys_overlay = H21WaitDestroyed $h21Proc $h21Hwnd
        Shot 'h21-04-shift-down-dismissed'
    } finally { [LT]::Key([byte]0x10, $true) }

    $h21Hwnd = H21Show $h21Proc 'h21-show-for-alt'
    try {
        [LT]::Key([byte]0x12, $false) # Also exercise WM_SYSKEYDOWN, not just WM_KEYDOWN.
        $summary.h21_alt_keydown_destroys_overlay = H21WaitDestroyed $h21Proc $h21Hwnd
        Shot 'h21-05-alt-down-dismissed'
    } finally { [LT]::Key([byte]0x12, $true) }
    # Alt down/repeats/up are consumed: no application menu or double-Alt action should
    # activate, and no menu-clearing Escape is needed here.

    $h21Hwnd = H21Show $h21Proc 'h21-show-for-escape'
    Press 'esc'
    $summary.h21_escape_destroys_overlay = H21WaitDestroyed $h21Proc $h21Hwnd
    Shot 'h21-06-escape-dismissed'
    $null = Ctl 'h21-state-after-dismissal' @('state')
    $h21StateStep = @($steps | Where-Object { $_.step -eq 'h21-state-after-dismissal' } | Select-Object -Last 1)
    $summary.h21_wiri_answers_after_dismissal = ($h21StateStep.Count -eq 1 -and $h21StateStep[0].rc -eq 0)
    Stop-WiriGracefully $h21Proc 'h21-unset'
    $h21Proc = $null

    foreach ($h21Variant in $h21Variants) {
        $h21Config = $h21Template.Replace('__H21_TITLE__', $h21Variant.property)
        [IO.File]::WriteAllText((Join-Path $D ("h21-" + $h21Variant.name + '-config.kdl')), $h21Config)
        $h21Proc = Start-WiriWithConfig $h21Config (Join-Path $D ("wiri.stderr21-" + $h21Variant.name + '.log'))
        $h21Pids += $h21Proc.Id
        $summary.h21_no_overlay_window_until_show = ($summary.h21_no_overlay_window_until_show -and
            -not $h21Proc.HasExited -and @(H21Windows $h21Proc).Count -eq 0)
        $null = Ctl ("h21-focus-" + $h21Variant.name) @('focus-window', [string][int64]$h21Input)
        Start-Sleep -Milliseconds 500
        $h21Hwnd = H21Show $h21Proc ("h21-show-" + $h21Variant.name)
        $h21Shots[$h21Variant.name] = H21Capture ("h21-" + $h21Variant.name) $h21Hwnd
        Start-Sleep -Milliseconds 300
        $h21Repeats[$h21Variant.name] = H21Capture ("h21-" + $h21Variant.name + '-repeat') $h21Hwnd
        Stop-WiriGracefully $h21Proc ("h21-" + $h21Variant.name)
        $h21Proc = $null
    }

    $h21AllShown = ($h21Shots.Count -eq 5)
    $h21Reference = $h21Shots['unset']
    $h21Stable = H21SameImage $h21Reference $h21Shots['unset-return']
    foreach ($h21Name in @('unset', 'title-a', 'title-b', 'null', 'unset-return')) {
        $h21Snapshot = $h21Shots[$h21Name]
        $h21AllShown = ($h21AllShown -and $null -ne $h21Snapshot -and $h21Snapshot.visible -and $h21Snapshot.cloaked -eq 0 -and
            $h21Snapshot.window_title -eq 'Important Hotkeys' -and $h21Snapshot.window_dpi -ge 96 -and
            $h21Snapshot.fits_work_area -and [bool]$h21Snapshot.crop_sha256)
        # A monitor/DPI/taskbar change during the experiment must not masquerade as a title change.
        $h21Stable = ($h21Stable -and (H21SameImage $h21Snapshot $h21Repeats[$h21Name]) -and
            $h21Snapshot.window_dpi -eq $h21Reference.window_dpi -and $h21Snapshot.monitor -eq $h21Reference.monitor -and
            ($h21Snapshot.monitor_bounds | ConvertTo-Json -Compress) -eq ($h21Reference.monitor_bounds | ConvertTo-Json -Compress) -and
            ($h21Snapshot.work_area | ConvertTo-Json -Compress) -eq ($h21Reference.work_area | ConvertTo-Json -Compress))
    }
    $summary.h21_all_title_configs_show_and_fit = $h21AllShown
    $summary.h21_screenshot_controls_are_stable = $h21Stable
    $summary.h21_custom_title_string_changes_drawn_pixels = ($h21AllShown -and $h21Stable -and
        -not (H21SameImage $h21Shots['title-a'] $h21Shots['title-b']) -and
        -not (H21SameImage $h21Shots['unset'] $h21Shots['title-a']))
    # Compare null against UNSET too: null versus a custom label alone could pass if null were
    # incorrectly treated as unset and merely restored the default action description.
    $summary.h21_null_title_changes_drawn_pixels = ($h21AllShown -and $h21Stable -and
        -not (H21SameImage $h21Shots['null'] $h21Shots['unset']) -and
        -not (H21SameImage $h21Shots['null'] $h21Shots['title-a']) -and
        -not (H21SameImage $h21Shots['null'] $h21Shots['title-b']))
    $h21Shots | ConvertTo-Json -Depth 6 | Set-Content -Encoding UTF8 (Join-Path $D 'h21-evidence.json')
    @'
Visual review (real desktop Shot PNGs; *-overlay.png are unaltered crops, not synthesized text):
- h21-01-unset: Important Hotkeys, default close-window row with Ctrl+Alt+Q.
- h21-title-a: H21 VISIBLE TITLE ALPHA replaces that row's description.
- h21-title-b: H21 VISIBLE TITLE BRAVO replaces the same description.
- h21-null: the close-window row/chord is absent, NOT a literal "null" or the default description.
- h21-unset-return matches h21-01-unset; per-config repeats must be identical.
The pixel gates detect ignored properties, not the semantic correctness of glyphs or row removal.
Review these images before claiming exact text/null-hides-row parity. No CLI or caption text is
used as a substitute for the owner-drawn body. If controls differ, treat pixel gates as failed,
not as evidence that the properties work. Bounds JSON records actual monitor/work-area pixels
and window DPI; a 1366x768 / scaled-DPI matrix requires real parent runs in those environments.
'@ | Set-Content -Encoding UTF8 (Join-Path $D 'h21-visual-review.txt')
    $summary.h21_fragment_completed = $true
} catch {
    ($_ | Out-String) | Set-Content -Encoding UTF8 (Join-Path $D 'h21-error.txt')
} finally {
    try {
        # Failed held-key assertions must never leave modifiers held for the next lane item.
        [LT]::Key([byte]0x10, $true)
        [LT]::Key([byte]0x12, $true)
        [LT]::Key([byte]0x11, $true)
        if ($null -ne $h21Proc) { Stop-WiriGracefully $h21Proc 'h21-finally' }
        if ($h21Input -ne [IntPtr]::Zero -and [LT]::IsWindow($h21Input)) { [TestForms]::CloseForm($h21Input) }
    } finally {
        if ($h21OldDpiContext -ne [IntPtr]::Zero) { [void][H21Native]::SetThreadDpiAwarenessContext($h21OldDpiContext) }
    }
}
$summary.h21_no_overlay_windows_after_quit = ($h21Pids.Count -gt 0 -and @([LT]::TopLevel() | Where-Object {
    [LT]::Class($_) -eq 'WiriBindingsCheatsheet' -and $h21Pids -contains [LT]::Pid($_)
}).Count -eq 0)
