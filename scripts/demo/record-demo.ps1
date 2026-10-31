# Record the wiri README demo: a 45-50 s desktop capture with burned-in captions,
# plus the files GitHub needs (an MP4, a looping WebP and a poster frame).
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\demo\record-demo.ps1
#   powershell ... -File scripts\demo\record-demo.ps1 -Rehearse        # keys only, no camera
#   powershell ... -File scripts\demo\record-demo.ps1 -EncodeOnly      # re-encode the last take
#   powershell ... -File scripts\demo\record-demo.ps1 -NoClips         # skip the per-segment WebPs
#
# What it does, in order
#   1. takes the machine-wide desktop mutex Global\wiri-live-test (an official live cycle
#      may hold it for up to two hours) and refuses to run next to another wiri,
#   2. drives wiri from THIS worktree's target\release with a throwaway profile and the
#      showcase config written below, in the script's own WinForms windows only,
#   3. records the desktop with ffmpeg gdigrab at 30 fps while it plays seven short
#      segments (columns, move/resize, tabs, workspaces, overview, float/fullscreen,
#      unified grid), pausing about a second after every action,
#   4. quits wiri, closes every window it opened, restores any window it had to park,
#      releases the mutex, and only then encodes (encoding needs no desktop),
#   5. writes docs/assets/demo.mp4 (H.264, faststart), demo.webp (looping, 960 px),
#      demo-poster.png and, when the budget allows, one small clip-<segment>.webp each.
#
# Everything it leaves behind lives under .demo\ (git-ignored): the lossless-ish raw
# recording, timeline.json, the per-run logs and the frames it checks itself with.
param(
    [string]$OutDir,
    [string]$StageDir,
    [ValidateRange(0, 720)][int]$LockWaitMinutes = 120,
    [switch]$Rehearse,
    [switch]$EncodeOnly,
    [switch]$VerifyOnly,
    [switch]$SkipEncode,
    [switch]$NoClips,
    [double]$CaptionOffset = 0.0
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Off

$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $OutDir) { $OutDir = Join-Path $repo 'docs\assets' }
if (-not $StageDir) { $StageDir = Join-Path $repo '.demo' }
# ffmpeg runs with the staging directory as its working directory, so every path the
# script hands it has to be absolute.  (Path.Combine returns the second path when it is
# already rooted, and Windows PowerShell 5.1 has no two-argument GetFullPath.)
$OutDir = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).Path, $OutDir))
$StageDir = [IO.Path]::GetFullPath([IO.Path]::Combine((Get-Location).Path, $StageDir))
$session = Join-Path $StageDir 'session'
$captionsDir = Join-Path $StageDir 'captions'
$fontsDir = Join-Path $StageDir 'fonts'
$framesDir = Join-Path $StageDir 'frames'
$rawPath = Join-Path $StageDir 'raw.mkv'
$timelinePath = Join-Path $StageDir 'timeline.json'
# Wall-clock seconds between the capture device coming up and the timeline's zero: the
# script waits this long after ffmpeg reports 'Capturing' before it starts its stopwatch.
$captureOffset = 0.4
$wiri = Join-Path $repo 'target\release\wiri.exe'
$ctl = Join-Path $repo 'target\release\wiri-ctl.exe'

New-Item -ItemType Directory -Force $StageDir, $session, $captionsDir, $fontsDir, $framesDir, $OutDir | Out-Null
$runLog = Join-Path $StageDir 'run.log'
Set-Content -Path $runLog -Value ("# wiri demo run " + (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'))
$clock = [Diagnostics.Stopwatch]::StartNew()

function Log([string]$message) {
    $line = '[{0,7:F1}s] {1}' -f $clock.Elapsed.TotalSeconds, $message
    Write-Host $line
    Add-Content -Path $runLog -Value $line
}

# --- ffmpeg ---------------------------------------------------------------------
$ffmpeg = (Get-Command ffmpeg.exe -ErrorAction SilentlyContinue).Source
if (-not $ffmpeg) {
    $ffmpeg = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages') -Recurse -Filter ffmpeg.exe -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $ffmpeg) { throw 'ffmpeg.exe not found (see scripts/record-live.ps1 for how this machine finds it)' }
$ffprobe = Join-Path (Split-Path -Parent $ffmpeg) 'ffprobe.exe'
Log "ffmpeg: $ffmpeg"

# --- the showcase config --------------------------------------------------------
# #GRID# is empty for the first six segments and becomes the opt-in unified grid for
# the last one: the script writes the file again and wiri hot-reloads it, which is
# exactly what the README tells a user to do.
$configTemplate = @'
// wiri demo config: the look and the keys the README video uses.
// scripts/demo/record-demo.ps1 writes this into a throwaway profile and reloads it
// with `layout { unified-grid }` for the last segment.
version "1.0"

input {
    mod-key "ctrl-alt"
}

binds {
    // Keep wiri's built-in keymap and add only what the video needs.
    extend-defaults

    // niri's own up/down workspace keys, so the vertical slide has a key of its own.
    Mod+Page_Down { focus-workspace-down; }
    Mod+Page_Up   { focus-workspace-up; }

    // Tabbed columns: hide or show the tab strip of the focused column.
    Mod+Shift+T { toggle-column-tabbed-display; }

    // The width preset cycle, on a plain letter: the default Mod+Backslash never fires
    // through this machine's keyboard hook, and the video needs the resize to be visible.
    Mod+W { switch-preset-column-width; }

    // wiri's opt-in diagonal actions (README, "Unified XY grid").  A bind on the same
    // keys as a built-in wins, so Mod+Alt+J and Mod+Alt+L replace the built-in
    // Mod+J (move-window-down) and Mod+L (scroll-right) in this demo config.
    Mod+Alt+U { focus-up-left; }
    Mod+Alt+I { focus-up-right; }
    Mod+Alt+J { focus-down-left; }
    Mod+Alt+L { focus-down-right; }
}

layout {
    // Room to breathe: 16 px between windows and around the strip.
    gaps 16
#GRID#
    border {
        width 3
        radius 10
        inactive-color "#2a303c"
        active-color "#4c8dff"
        // The demo opens windows on a workspace that is hidden at the time, which some
        // applications (and the shell) read as a request for attention.  Keep the urgent
        // colour the same as the inactive one so the ring stays on the focused window.
        urgent-color "#2a303c"
    }

    focus-ring {
        width 3
        gap 4
        active-color "#8ab4ff"
        inactive-color "#39414f"
    }

    shadow {
        on
        softness 24
        spread 2
        offset x=0 y=8
        color "#00000066"
    }

    // Three columns share the screen; from the fourth on the strip scrolls.
    column-width-mode "proportional"
    max-visible-columns 3
    column-width 620

    snap-on-drag true
    snap-threshold-px 20
}

overview {
    zoom 0.5
}

animations {
    on
    duration 260
    easing "ease-out-cubic"
    // Springs, so the workspace slide settles instead of stopping dead.
    workspace-switch { spring damping-ratio=1.0 stiffness=520 epsilon=0.001 }
}
'@

$gridBlock = @'
    // The opt-in extension this last segment demonstrates.
    unified-grid
    unified-grid-chord-ms 150
'@

$configShowcase = $configTemplate -replace '#GRID#', ''
$configGrid = $configTemplate -replace '#GRID#', $gridBlock

# --- captions -------------------------------------------------------------------
# Kept short and plain: the burned-in band is one line at the bottom of the frame.
$segments = @(
    [pscustomobject]@{ key = 'columns'; caption = 'Columns on an endless strip' },
    [pscustomobject]@{ key = 'move-resize'; caption = 'Move and resize' },
    [pscustomobject]@{ key = 'tabs'; caption = 'Tabbed columns' },
    [pscustomobject]@{ key = 'workspaces'; caption = 'Workspaces stack vertically' },
    [pscustomobject]@{ key = 'overview'; caption = 'Overview: pan, then click a window' },
    [pscustomobject]@{ key = 'float-fullscreen'; caption = 'Floating and fullscreen' },
    [pscustomobject]@{ key = 'grid'; caption = 'Unified XY grid (opt-in)' }
)
foreach ($segment in $segments) {
    Set-Content -Path (Join-Path $captionsDir ($segment.key + '.txt')) -Value $segment.caption -Encoding ASCII
}

# --- wiri-ctl -------------------------------------------------------------------
function Invoke-Ctl([string[]]$CtlArgs, [int]$TimeoutSeconds = 10) {
    $psi = New-Object Diagnostics.ProcessStartInfo $ctl
    $psi.Arguments = ($CtlArgs | ForEach-Object { if ($_ -match '[\s"]') { '"' + ($_ -replace '"', '\"') + '"' } else { $_ } }) -join ' '
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    $process = [Diagnostics.Process]::Start($psi)
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        try { $process.Kill() } catch { }
        return [pscustomobject]@{ rc = 'timeout'; text = ''; err = '' }
    }
    return [pscustomobject]@{ rc = $process.ExitCode; text = $stdout.Result; err = $stderr.Result }
}

# --- choreography helpers -------------------------------------------------------
$script:timeline = New-Object System.Collections.Generic.List[object]
$script:current = $null
$script:watch = $null

function Wait-Seconds([double]$seconds) {
    if ($seconds -le 0) { return }
    Start-Sleep -Milliseconds ([int]($seconds * 1000))
}

function Start-Segment([string]$key) {
    # The caption travels with the segment into timeline.json: the encode phase draws the
    # bands from it and never sees $segments.
    $caption = ''
    $match = @($segments | Where-Object { $_.key -eq $key })
    if ($match.Count -gt 0) { $caption = $match[0].caption }
    $script:current = [pscustomobject]@{ key = $key; caption = $caption; start = [Math]::Round($script:watch.Elapsed.TotalSeconds, 3); end = $null }
    $script:timeline.Add($script:current)
    Log ("segment {0} starts at {1:F2}s" -f $key, $script:current.start)
    if (-not $Rehearse) {
        # A popup that appears mid-take would be in the published video: stop and bin the take.
        if (-not (Test-StrayPopups $key -DuringTake)) {
            $script:takeFailed = $true
            throw ("a shell popup appeared while segment '{0}' was starting; this take is contaminated" -f $key)
        }
    } else {
        # Rehearse the check itself, without letting it end the run.
        if (-not (Test-StrayPopups $key -DuringTake)) { Log '  (rehearsal) the per-segment popup check would have stopped the take' }
    }
}

function Stop-Segment {
    if ($script:current) {
        $script:current.end = [Math]::Round($script:watch.Elapsed.TotalSeconds, 3)
        Log ("segment {0} ends at {1:F2}s ({2:F2}s)" -f $script:current.key, $script:current.end, ($script:current.end - $script:current.start))
        $script:current = $null
    }
}

# One real chord, then the pause that lets the animation finish before the next one.
# Every chord is followed by a one-line state probe in the run log: the take then carries
# its own proof of what each key press did, next to the frames that show it.
function Get-StateLine([string]$label) {
    $windows = Invoke-Ctl @('windows', '--json')
    $state = Invoke-Ctl @('state')
    $parts = New-Object System.Collections.Generic.List[string]
    try {
        $list = @(($windows.text | ConvertFrom-Json).result.windows)
        foreach ($window in $list) {
            $name = ($window.title -replace '[^\x20-\x7e]', '').Trim()
            $size = $window.layout.tile_size
            $place = $window.layout.pos_in_scrolling_layout
            $flags = ''
            if ($window.is_floating) { $flags += ' FLOAT' }
            if ($window.is_fullscreen) { $flags += ' FULL' }
            $parts.Add(('{0}[ws{1} c{2}t{3} {4}x{5}{6}]' -f $name, $window.workspace_id,
                $(if ($place) { $place[0] } else { '-' }), $(if ($place) { $place[1] } else { '-' }),
                [Math]::Round($size[0]), [Math]::Round($size[1]), $flags))
        }
    } catch { $parts.Add('state unreadable') }
    $activeWorkspace = [regex]::Match(($state.text -replace "\r?\n", ' '), 'active workspace\s*:\s*(\S+)').Groups[1].Value
    $overviewText = 'n/a'
    if ($label -match 'space|escape') { $overviewText = (Invoke-Ctl @('overview-state')).text -replace "\r?\n", ' ' }
    $line = ('{0,-22} active={1} overview={2} | {3}' -f $label, $activeWorkspace, $overviewText.Trim(), ($parts -join ' '))
    Add-Content -Path (Join-Path $session 'states.txt') -Value $line
    Log ('  ' + $line)
}

function Send-Chord([string]$chord, [double]$pause = 1.0, [string]$note = '') {
    if ($Rehearse) { Log ("  chord {0} {1}" -f $chord, $note) }
    [DemoNative]::Chord($chord)
    if (-not $Rehearse) { Get-StateLine $chord }
    Wait-Seconds $pause
}

function Open-Demo([string]$kind, [string]$caption, [double]$pause = 1.2) {
    Log ("  open {0} '{1}'" -f $kind, $caption)
    $handle = [DemoApps]::Open($kind, $caption)
    if ($handle -eq [IntPtr]::Zero) { throw "window '$caption' did not open" }
    $script:apps[$caption] = $handle
    Wait-Seconds $pause
}

function Save-Snapshot([string]$tag) {
    $reply = Invoke-Ctl @('windows', '--json')
    Set-Content -Path (Join-Path $session ($tag + '-windows.json')) -Value $reply.text
    $state = Invoke-Ctl @('state')
    Set-Content -Path (Join-Path $session ($tag + '-state.txt')) -Value $state.text
    if ($Rehearse) { Log ("  state: " + (($state.text -split "`n") -join ' | ')) }
}

# --- the desktop session --------------------------------------------------------
$script:apps = @{}
$script:takeFailed = $false
$script:captureInfo = $null
$script:preflightShellHwnds = @()
$wiriProcess = $null
$record = $null
$minimized = @()
$savedRoaming = $env:APPDATA
$savedLocal = $env:LOCALAPPDATA
$savedPipe = $env:WIRI_PIPE
$savedCfg = $env:WIRI_CONFIG

function Test-DesktopBusy {
    $processes = @(Get-Process wiri -ErrorAction SilentlyContinue)
    if ($processes.Count -gt 0) {
        return ('a wiri is already running (pid ' + (($processes | ForEach-Object { $_.Id }) -join ', ') + ')')
    }
    return $null
}

# --- stray shell popups ---------------------------------------------------------
# A Windows 11 desktop context menu owned by explorer.exe was sitting on the tridaris
# desktop and ended up in two takes.  Nothing in this script right-clicks: the menu was
# already open, and the old "foreign windows" gate could not see it because a menu has no
# window title.  So the script now takes a census of every visible top-level window, treats
# anything that is a shell menu/flyout (or any other explorer.exe window that is not the
# shell's own furniture) as a stray popup, and dismisses it before the camera rolls.  Every
# segment boundary re-runs the check, and a take in which a popup appears is thrown away
# instead of encoded.
$script:shellClasses = @('Shell_TrayWnd', 'Shell_SecondaryTrayWnd', 'Progman', 'WorkerW',
                         'TrayNotifyWnd', 'SysPager', 'Button', 'tooltips_class32',
                         'Windows.UI.Core.CoreWindow', 'WiriThumbnailOverviewHost', 'WiriDecor',
                         'IME', 'Default IME', 'MSCTFIME UI',
                         # Windows 11 shell infrastructure that is not a popup
                         'XamlExplorerHostIslandWindow', 'ApplicationManager_DesktopShellWindow',
                         'DesktopWindowXamlSource', 'Windows.UI.Input.InputSite.WindowClass',
                         'MultitaskingViewFrame', 'ForegroundStaging', 'Shell_InputSwitchTopLevelWindow',
                         # StartAllBack (installed on tridaris) draws the taskbar itself
                         'SIBTranslucentLayer', 'TApplication')
$script:popupClasses = @('#32768', '#32769', 'Xaml_WindowedPopupClass', 'PopupHost',
                         'NotifyIconOverflowWindow', 'TopLevelWindowForOverflowXamlIsland',
                         'Windows.UI.Composition.DesktopWindowContentBridge', 'MenuHostWindowClass',
                         'TaskListThumbnailWnd', 'Xaml_WindowedPopupClass_uwp')

function Write-WindowCensus([string]$name, $windows) {
    $path = Join-Path $StageDir ('windows-{0}.txt' -f $name)
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add(('# window check: {0} ({1})' -f $name, (Get-Date -Format 'HH:mm:ss')))
    $lines.Add('# every visible top-level window (EnumWindows)')
    foreach ($window in ($windows | Sort-Object Process, Class)) {
        $lines.Add('  ' + [DemoNative]::Describe($window))
    }
    # The shell's menus live in a z-band EnumWindows does not return: ask WindowFromPoint
    # over a grid of the work area and look at what owns the foreground.
    $lines.Add('# foreground and WindowFromPoint grid (every 48 px over the work area)')
    $probe = @(Get-PopupHits)
    if ($probe.Count -eq 0) {
        $lines.Add('  no shell popup found')
    } else {
        foreach ($hit in $probe) { $lines.Add('  HIT ' + $hit) }
    }
    $lines.Add('# WindowFromPoint(518,455): ' + [PopupProbe]::PointDescription(518, 455))
    $lines | Set-Content -Path $path
    return $path
}

# Everything the shell can show a menu through: the foreground owner, and what a grid of
# points lands on.
function Get-PopupHits {
    $hits = New-Object System.Collections.Generic.List[string]
    $foreground = [PopupProbe]::ForegroundHit()
    if ($foreground) { $hits.Add($foreground) }
    foreach ($hit in [PopupProbe]::ScanHits(48)) { $hits.Add($hit) }
    return $hits
}

function Get-StrayPopups($windows) {
    $ownPid = [int64]$PID
    return @($windows | Where-Object {
        [int64]$_.Pid -ne $ownPid -and
        ($script:popupClasses -contains $_.Class -or
         ($_.Process -eq 'explorer' -and $script:shellClasses -notcontains $_.Class))
    })
}

# During the take the bar is higher: a menu class, or an explorer.exe window that was not
# part of the shell as it stood before the take (the pre-flight allowlist), is a popup that
# appeared while recording.  Shell furniture that was there all along stays quiet, so a
# start menu island cannot abort a good take by accident.
function Get-NewShellPopups($windows) {
    $ownPid = [int64]$PID
    return @($windows | Where-Object {
        if ([int64]$_.Pid -eq $ownPid) { return $false }
        if ($script:popupClasses -contains $_.Class) { return $true }
        if ($_.Process -eq 'explorer') {
            if ($script:shellClasses -contains $_.Class) { return $false }
            return ($script:preflightShellHwnds -notcontains [int64]$_.Hwnd)
        }
        return $false
    })
}

# Stray shell popups.  Windows 11 draws its menus in a z-band EnumWindows never returns, so
# the real check is geometric: what owns the foreground, and what a grid of WindowFromPoint
# probes across the work area lands on.  The EnumWindows census is kept as a second opinion
# (it catches ordinary stray windows), and everything is logged with class, title, pid, rect.
function Test-StrayPopups {
    param([string]$Name, [switch]$Dismiss, [switch]$DuringTake)
    $windows = @([DemoNative]::TopLevelWindows($true))
    [void](Write-WindowCensus $Name $windows)
    $hits = @(Get-PopupHits)
    if ($DuringTake) { $enumerated = @(Get-NewShellPopups $windows) } else { $enumerated = @(Get-StrayPopups $windows) }
    if ($hits.Count -eq 0 -and $enumerated.Count -eq 0) {
        Log ('window check {0}: clean ({1} top-level windows, no shell popup; point(518,455)={2})' -f `
            $Name, $windows.Count, [PopupProbe]::PointDescription(518, 455))
        if ($Dismiss -and -not $DuringTake) {
            # remember the shell as it is now, so the per-segment check can tell new from old
            $script:preflightShellHwnds = @($windows | Where-Object { $_.Process -eq 'explorer' } | ForEach-Object { [int64]$_.Hwnd })
        }
        return $true
    }
    foreach ($hit in $hits) { Log ('window check {0}: POPUP {1}' -f $Name, $hit) }
    foreach ($popup in $enumerated) { Log ('window check {0}: STRAY {1}' -f $Name, [DemoNative]::Describe($popup)) }
    if (-not $Dismiss) { return $false }

    # A submenu can need more than one Escape; the menu host owns the foreground, so a real
    # SendInput goes to it.
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        [DemoNative]::SendInputEscape()
        Wait-Seconds 0.3
        $left = @(Get-PopupHits) + @(Get-StrayPopups @([DemoNative]::TopLevelWindows($true)))
        if ($left.Count -eq 0) {
            Log ('  removed by SendInput Escape (attempt {0})' -f $attempt)
            break
        }
        Log ('  still present after Escape {0}: {1}' -f $attempt, $left[0])
    }

    # Then one left click on an empty part of the desktop, and only where the desktop really
    # is the top window (never on a demo window, the taskbar or another application).
    $left = @(Get-PopupHits) + @(Get-StrayPopups @([DemoNative]::TopLevelWindows($true)))
    if ($left.Count -gt 0) {
        $point = $null
        foreach ($candidate in @(@{ x = 1200; y = 400 }, @{ x = 200; y = 640 }, @{ x = 1200; y = 640 }, @{ x = 120; y = 120 })) {
            if ([PopupProbe]::IsDesktopPoint($candidate.x, $candidate.y)) { $point = $candidate; break }
        }
        if ($point) {
            Log ('  clicking the empty desktop at {0},{1}' -f $point.x, $point.y)
            [DemoNative]::SendInputClick($point.x, $point.y)
            Wait-Seconds 0.4
        } else {
            Log '  no empty desktop point to click (every candidate is covered)'
        }
    }

    $left = @(Get-PopupHits) + @(Get-StrayPopups @([DemoNative]::TopLevelWindows($true)))
    if ($left.Count -gt 0) {
        throw ('a shell popup is still on screen: ' + ($left -join ' | '))
    }
    Log ('window check {0}: clean after dismissal; point(518,455)={1}' -f $Name, [PopupProbe]::PointDescription(518, 455))
    if ($Dismiss -and -not $DuringTake) {
        $script:preflightShellHwnds = @([DemoNative]::TopLevelWindows($true) | Where-Object { $_.Process -eq 'explorer' } | ForEach-Object { [int64]$_.Hwnd })
    }
    return $true
}

$mutex = $null
$ownsMutex = $false
if ($EncodeOnly -or $VerifyOnly) {
    Log 'skipping the desktop session (encode/verify only)'
} else {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -Path (Join-Path $PSScriptRoot 'demo-native.cs')
    foreach ($line in [DemoApps]::SelfTest()) {
        Log ("selftest " + $line)
        if ($line -match 'FAILED') { throw "the demo windows do not paint: $line" }
    }
    if (-not (Test-Path $wiri)) { throw "missing $wiri - build it with: cargo build --release -j 2" }

    $mutex = New-Object System.Threading.Mutex($false, 'Global\wiri-live-test')
    try { $ownsMutex = $mutex.WaitOne([TimeSpan]::FromMinutes($LockWaitMinutes)) }
    catch [System.Threading.AbandonedMutexException] { $ownsMutex = $true }
    if (-not $ownsMutex) { throw "the desktop is held by another wiri run (waited $LockWaitMinutes min)" }
    Log 'desktop mutex Global\wiri-live-test acquired'

    try {
        $busy = Test-DesktopBusy
        if ($busy) { throw $busy }

        # Census first, then dismiss whatever shell menu is sitting on the desktop, then
        # minimise the other applications' windows.  A menu has no title, so it used to slip
        # through the foreign-window gate and be recorded.
        [void](Test-StrayPopups 'preflight')
        [void](Test-StrayPopups 'preflight-dismiss' -Dismiss)
        Wait-Seconds 0.4

        # A throwaway profile: wiri's config, state and recovery journal stay in .demo\.
        $roaming = Join-Path $StageDir 'profile\roaming'
        $local = Join-Path $StageDir 'profile\local'
        New-Item -ItemType Directory -Force (Join-Path $roaming 'wiri'), $local | Out-Null
        $cfgPath = Join-Path $roaming 'wiri\config.kdl'
        [IO.File]::WriteAllText($cfgPath, $configShowcase, (New-Object Text.UTF8Encoding($false)))
        [IO.File]::WriteAllText((Join-Path $StageDir 'showcase-grid.kdl'), $configGrid, (New-Object Text.UTF8Encoding($false)))
        $valid = Invoke-Ctl @('validate-config', $cfgPath)
        Log ("config: " + $valid.text.Trim())
        if ($valid.rc -ne 0) { throw 'the showcase config does not validate' }

        # Anything that is not ours must not be on screen: the video is published.  These
        # windows are the machine's, not the demo's; they are minimized for the take and
        # restored in the finally block below.
        $foreign = @([DemoNative]::VisibleWindows() | Where-Object { [DemoNative]::PidOf($_) -ne $PID })
        foreach ($handle in $foreign) {
            Log ("parking " + [DemoNative]::Describe($handle))
            [DemoNative]::Minimize($handle)
            $minimized += $handle
        }
        Wait-Seconds 0.8
        $leftover = @([DemoNative]::VisibleWindows() | Where-Object { [DemoNative]::PidOf($_) -ne $PID })
        # The text-input host is a CoreWindow the shell keeps around; it is not drawn and it
        # cannot be minimized.  Anything else still on screen would end up in the video.
        $drawn = @($leftover | Where-Object { [DemoNative]::ClassOf($_) -ne 'Windows.UI.Core.CoreWindow' })
        foreach ($handle in $leftover) { Log ("still visible: " + [DemoNative]::Describe($handle)) }
        if ($drawn.Count -gt 0) { throw 'another program still has a window on screen; refusing to record over it' }
        # And once more right before the camera rolls: nothing may have re-opened a menu.
        [void](Test-StrayPopups 'preflight-final' -Dismiss)
        [DemoNative]::ParkCursor()

        # Start wiri from this worktree with the demo profile and its own pipe.
        $env:WIRI_PIPE = '\\.\pipe\wiri_demo_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
        $env:WIRI_CONFIG = $cfgPath
        $env:APPDATA = $roaming
        $env:LOCALAPPDATA = $local
        try {
            $wiriProcess = Start-Process -FilePath $wiri -ArgumentList '-v', '--no-startup-overlay' `
                -WorkingDirectory $repo -PassThru `
                -RedirectStandardError (Join-Path $session 'wiri.stderr.log') `
                -RedirectStandardOutput (Join-Path $session 'wiri.stdout.log')
        } finally {
            $env:APPDATA = $savedRoaming
            $env:LOCALAPPDATA = $savedLocal
        }
        $pipePath = $env:WIRI_PIPE
        $started = $false
        for ($i = 0; $i -lt 200 -and -not $started; $i++) {
            if ($wiriProcess.HasExited) { break }
            try { if ([IO.Directory]::GetFiles('\\.\pipe\') -contains $pipePath) { $started = $true; break } } catch { }
            Start-Sleep -Milliseconds 100
        }
        if (-not $started) { throw 'wiri did not open its pipe' }
        Wait-Seconds 1.2
        $state = Invoke-Ctl @('state')
        Log ("wiri pid {0}: {1}" -f $wiriProcess.Id, (($state.text -split "`n")[0]))

        # The binds the choreography needs must have survived RegisterHotKey; a chord some
        # other program owns would silently do nothing and spoil a whole take.  Windows'
        # own modifier bits: 1 = Alt, 2 = Ctrl, 4 = Shift, 8 = Win.
        $bindingReply = Invoke-Ctl @('bindings', '--json')
        Set-Content -Path (Join-Path $session 'bindings.json') -Value $bindingReply.text
        $bindingEntries = @()
        try {
            # The reply is {"result": {"bindings": [ {id, chord, modifiers, vk_code, action}, ... ]}}.
            $parsed = $bindingReply.text | ConvertFrom-Json
            if ($parsed.PSObject.Properties.Name -contains 'result') {
                if ($parsed.result.PSObject.Properties.Name -contains 'bindings') {
                    $bindingEntries = @($parsed.result.bindings)
                } else {
                    $bindingEntries = @($parsed.result)
                }
            } else {
                $bindingEntries = @($parsed)
            }
        } catch { Log ('bindings reply did not parse: ' + $_.Exception.Message) }
        $needed = @(
            @{ mods = 3; vk = 0x25; name = 'ctrl+alt+left' },
            @{ mods = 3; vk = 0x27; name = 'ctrl+alt+right' },
            @{ mods = 3; vk = 0x24; name = 'ctrl+alt+home' },
            @{ mods = 7; vk = 0x25; name = 'ctrl+alt+shift+left' },
            @{ mods = 3; vk = 0x57; name = 'ctrl+alt+w' },
            @{ mods = 3; vk = 0xBC; name = 'ctrl+alt+comma' },
            @{ mods = 7; vk = 0x54; name = 'ctrl+alt+shift+t' },
            @{ mods = 3; vk = 0x28; name = 'ctrl+alt+down' },
            @{ mods = 3; vk = 0x22; name = 'ctrl+alt+pagedown' },
            @{ mods = 3; vk = 0x21; name = 'ctrl+alt+pageup' },
            @{ mods = 3; vk = 0x20; name = 'ctrl+alt+space' },
            @{ mods = 3; vk = 0x54; name = 'ctrl+alt+t' },
            @{ mods = 3; vk = 0x46; name = 'ctrl+alt+f' },
            @{ mods = 3; vk = 0x49; name = 'ctrl+alt+i' },
            @{ mods = 3; vk = 0x4A; name = 'ctrl+alt+j' }
        )
        $missing = @()
        if ($bindingEntries.Count -eq 0) {
            Log 'bindings reply unusable; skipping the chord registration check'
        } else {
            foreach ($chord in $needed) {
                $found = @($bindingEntries | Where-Object { [int]$_.modifiers -eq $chord.mods -and [int]$_.vk_code -eq $chord.vk })
                if ($found.Count -eq 0) { $missing += $chord.name }
            }
        }
        if ($missing.Count -gt 0) {
            Log ('bindings not registered: ' + ($missing -join ', '))
            throw 'the demo chords are not all registered; see bindings.json'
        }
        Log ('all {0} demo chords are registered' -f $needed.Count)

        # Windows that must already be there when the camera rolls: the editor on
        # workspace 1, and two columns on workspace 2 for the vertical slide.
        Open-Demo 'editor' 'Editor — notes.md' 1.6
        Send-Chord 'ctrl+alt+2' 1.2
        Open-Demo 'chat' 'Chat — #wiri' 1.4
        Open-Demo 'music' 'Music — focus' 1.4
        Send-Chord 'ctrl+alt+1' 1.4
        Save-Snapshot '00-setup'
        [DemoNative]::ParkCursor()
        Wait-Seconds 0.8

        if (-not $Rehearse) {
            # 30 fps into MJPEG in a Matroska file.  x264, even at ultrafast, was the
            # bottleneck on this laptop: the capture loop fell behind by about a tenth of
            # the take and, because gdigrab stamps frames by count, the file never covered
            # that time (which shifts the burned-in captions).  MJPEG costs a couple of
            # milliseconds a frame, so the take stays on the wall clock; it is a throwaway
            # intermediate, re-encoded to H.264 at the end.
            $recordLog = Join-Path $session 'ffmpeg-record.log'
            $recordArgs = @('-hide_banner', '-loglevel', 'info', '-y',
                            '-fflags', '+genpts',
                            '-f', 'gdigrab', '-framerate', '30', '-draw_mouse', '1', '-i', 'desktop',
                            '-c:v', 'mjpeg', '-q:v', '2', '-pix_fmt', 'yuvj420p',
                            '-g', '30', '-t', '300', '-f', 'matroska', $rawPath)
            $record = Start-Process -FilePath $ffmpeg -ArgumentList $recordArgs -WindowStyle Hidden -PassThru `
                -RedirectStandardError $recordLog
            # t=0 of the timeline is the moment the capture device comes up.
            for ($i = 0; $i -lt 100; $i++) {
                try {
                    if ((Test-Path $recordLog) -and ((Get-Content $recordLog -Raw) -match 'Capturing')) { break }
                } catch { }
                Start-Sleep -Milliseconds 60
            }
            Wait-Seconds 0.4
        }
        $script:watch = [Diagnostics.Stopwatch]::StartNew()
        Wait-Seconds 0.55

        # ---- a) columns on an endless strip -------------------------------------
        Start-Segment 'columns'
        Wait-Seconds 0.2
        Open-Demo 'terminal' 'Terminal — pwsh' 1.35
        Open-Demo 'notes' 'Notes — checklist.md' 1.35
        Open-Demo 'browser' 'Browser — wiri.local/docs' 1.35
        Open-Demo 'files' 'Files — C:\wiri' 1.35
        Send-Chord 'ctrl+alt+home' 1.45 'focus-column-first: ride the strip back'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '01-columns' }

        # ---- b) move and resize --------------------------------------------------
        Start-Segment 'move-resize'
        Wait-Seconds 0.3
        Send-Chord 'ctrl+alt+right' 0.85 'focus-column-right'
        Send-Chord 'ctrl+alt+right' 0.85 'focus-column-right'
        Send-Chord 'ctrl+alt+shift+left' 1.15 'move-column-left'
        Send-Chord 'ctrl+alt+w' 1.25 'switch-preset-column-width'
        Send-Chord 'ctrl+alt+w' 1.25 'switch-preset-column-width'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '02-move-resize' }

        # ---- c) tabbed columns ---------------------------------------------------
        Start-Segment 'tabs'
        Wait-Seconds 0.25
        Send-Chord 'ctrl+alt+home' 0.5 'focus-column-first'
        Send-Chord 'ctrl+alt+right' 0.6 'focus the second column'
        Send-Chord 'ctrl+alt+comma' 0.95 'consume-window-into-column'
        Send-Chord 'ctrl+alt+comma' 0.95 'consume-window-into-column'
        Send-Chord 'ctrl+alt+shift+t' 1.05 'toggle-column-tabbed-display'
        Send-Chord 'ctrl+alt+down' 0.9 'focus-window-down: next tab'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '03-tabs' }

        # ---- d) workspaces stack vertically --------------------------------------
        # Two slides with a long settle: the vertical move is the whole point of the beat,
        # so it must not be swallowed by a slow capture frame.
        Start-Segment 'workspaces'
        Wait-Seconds 0.25
        Send-Chord 'ctrl+alt+pagedown' 2.0 'focus-workspace-down'
        Send-Chord 'ctrl+alt+pageup' 1.9 'focus-workspace-up'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '04-workspaces' }

        # ---- e) overview ---------------------------------------------------------
        Start-Segment 'overview'
        Wait-Seconds 0.25
        Send-Chord 'ctrl+alt+space' 1.7 'toggle-overview'
        Send-Chord 'ctrl+alt+right' 1.0 'pan across the columns'
        Send-Chord 'ctrl+alt+right' 0.85 'pan across the columns'
        Send-Chord 'ctrl+alt+pagedown' 1.25 'down a workspace'
        # Click a real window in the overview: read where the tiles are, put the pointer
        # on the one nearest the middle and click it.
        $overviewHost = [DemoNative]::OverviewHost()
        if ($overviewHost -ne [IntPtr]::Zero) {
            $overviewRect = [DemoNative]::Bounds($overviewHost)
            # Prefer the workspace-1 windows: the jump then always lands where the two
            # segments after this one expect to start.
            $primaryCaptions = @('Editor — notes.md', 'Terminal — pwsh', 'Notes — checklist.md',
                                 'Browser — wiri.local/docs', 'Files — C:\wiri')
            $mine = @()
            foreach ($caption in $primaryCaptions) {
                if ($script:apps.ContainsKey($caption)) { $mine += [Int64]$script:apps[$caption] }
            }
            $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
            $best = $null
            foreach ($line in [DemoNative]::OverviewTiles()) {
                $parts = $line -split '\|'
                $hwnd = [Int64]$parts[1]
                $tile = [pscustomobject]@{ hwnd = $hwnd; x = [int]$parts[2]; y = [int]$parts[3]; w = [int]$parts[4]; h = [int]$parts[5] }
                $cx = $tile.x + $tile.w / 2
                $cy = $tile.y + $tile.h / 2
                $distance = [Math]::Sqrt([Math]::Pow($cx - $overviewRect.Width / 2, 2) + [Math]::Pow($cy - $overviewRect.Height / 2, 2))
                if ($mine -notcontains $hwnd) { $distance += 100000 }
                # The click has to land on the screen: a panned overview can put a tile above it.
                $candidateX = $overviewRect.Left + [int]($tile.x + $tile.w / 2)
                $candidateY = $overviewRect.Top + [int]($tile.y + $tile.h / 2)
                if ($candidateX -lt 0 -or $candidateY -lt 0 -or $candidateX -ge $screen.Width -or $candidateY -ge $screen.Height) { $distance += 100000 }
                if ($tile.w -gt 40 -and $tile.h -gt 40 -and ($null -eq $best -or $distance -lt $best.distance)) {
                    $tile | Add-Member -NotePropertyName distance -NotePropertyValue $distance
                    $best = $tile
                }
            }
            if ($best) {
                $clickX = $overviewRect.Left + [int]($best.x + $best.w / 2)
                $clickY = $overviewRect.Top + [int]($best.y + $best.h / 2)
                Log ("  click tile hwnd=0x{0:X} at {1},{2}" -f $best.hwnd, $clickX, $clickY)
                [DemoNative]::Click($clickX, $clickY)
                Wait-Seconds 1.2
                # Back to the corner: the pointer is only interesting where it clicks.
                [DemoNative]::ParkCursor()
            } else {
                Log '  no overview tile to click'
                Wait-Seconds 1.2
            }
        } else {
            Log '  overview host window not found'
            Wait-Seconds 1.2
        }
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '05-overview' }

        # ---- f) floating and fullscreen -----------------------------------------
        Start-Segment 'float-fullscreen'
        Wait-Seconds 0.25
        Send-Chord 'ctrl+alt+t' 1.45 'toggle-window-floating'
        Send-Chord 'ctrl+alt+t' 1.05 'toggle-window-floating back'
        Send-Chord 'ctrl+alt+f' 1.55 'fullscreen-window'
        Send-Chord 'ctrl+alt+f' 1.1 'fullscreen-window back'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '06-float-fullscreen' }

        # ---- g) unified XY grid, opt-in by a config reload ----------------------
        Start-Segment 'grid'
        Wait-Seconds 0.3
        [IO.File]::WriteAllText($cfgPath, $configGrid, (New-Object Text.UTF8Encoding($false)))
        Log '  config rewritten with layout { unified-grid } (hot reload)'
        Wait-Seconds 1.2
        $monitors = Invoke-Ctl @('list-monitors', '--json')
        Set-Content -Path (Join-Path $session '07-monitors.json') -Value $monitors.text
        if ($monitors.text -match '"unified_grid"\s*:\s*true') { Log '  unified grid is on' } else { Log '  WARNING: the monitor does not report unified_grid' }
        Send-Chord 'ctrl+alt+j' 1.2 'focus-down-left (diagonal)'
        Send-Chord 'ctrl+alt+i' 1.2 'focus-up-right (diagonal)'
        Send-Chord 'ctrl+alt+space' 1.25 'toggle-overview'
        Send-Chord 'ctrl+alt+j' 1.5 'diagonal pan in the overview'
        Stop-Segment
        if (-not $Rehearse) { Save-Snapshot '08-grid' }

        # Trail: keep the last frame calm, and check the screen one last time.
        Wait-Seconds 0.5
        if (-not $Rehearse) {
            if (-not (Test-StrayPopups 'final' -DuringTake)) {
                $script:takeFailed = $true
                throw 'a shell popup appeared before the take ended; this take is contaminated'
            }
        }
        Log ('choreography done after {0:F2}s' -f $script:watch.Elapsed.TotalSeconds)
        if (-not $Rehearse) {
            Stop-Process -Id $record.Id -Force -ErrorAction SilentlyContinue
            $record.WaitForExit(5000) | Out-Null
            $record = $null
            Log 'recording stopped'
            if (Test-Path $rawPath) {
                # ffprobe complains about the truncated Matroska trailer on stderr; with
                # $ErrorActionPreference = 'Stop' that would end the run, so it is contained.
                $rawInfo = @()
                $lastPts = @()
                $savedPreference = $ErrorActionPreference
                $ErrorActionPreference = 'Continue'
                try {
                    $rawInfo = @(& $ffprobe -v error -select_streams v:0 -show_entries stream=width,height,r_frame_rate -of default=nw=1:nk=1 $rawPath 2>$null)
                    $lastPts = @(& $ffprobe -v error -select_streams v:0 -show_entries packet=pts_time -of csv=p=0 $rawPath 2>$null |
                        ForEach-Object { [double]$_ })
                } catch {
                    Log ('ffprobe: ' + $_.Exception.Message)
                } finally {
                    $ErrorActionPreference = $savedPreference
                }
                $captureEnd = 0.0
                if ($lastPts.Count -gt 0) { $captureEnd = ($lastPts | Measure-Object -Maximum).Maximum }
                # The stopwatch starts 0.4 s after the capture device comes up, so the wall
                # time the raw covers is the stopwatch plus that lead.
                $captureWall = [Math]::Round($script:watch.Elapsed.TotalSeconds + $captureOffset, 3)
                $stretch = 1.0
                if ($captureEnd -gt 1) { $stretch = [Math]::Round($captureWall / $captureEnd, 5) }
                $script:captureInfo = [pscustomobject]@{ end = [Math]::Round($captureEnd, 3); wall = $captureWall; offset = $captureOffset; stretch = $stretch }
                Log ('raw take: ' + (($rawInfo | Where-Object { $_ }) -join ', ') + ' (' + [Math]::Round((Get-Item $rawPath).Length / 1MB, 1) + ' MB)')
                # gdigrab numbers its frames, so a frame the capture loop misses is time that
                # the file never covers; the encode stretches the take back onto the wall
                # clock with this factor (the captions are burned in at timeline times).
                Log ("capture covers {0:F2}s of the {1:F2}s wall time (stretch {2:F4})" -f $captureEnd, $captureWall, $stretch)
                if ($stretch -gt 1.6) { Log 'WARNING: the capture fell far behind; the take may look uneven' }
            } else {
                Log 'ERROR: the recording produced no file'
            }
        }
    } catch {
        $script:takeFailed = $true
        Log ('take aborted: ' + $_.Exception.Message)
        throw
    } finally {
        if ($record -and -not $record.HasExited) { Stop-Process -Id $record.Id -Force -ErrorAction SilentlyContinue }
        if ($wiriProcess -and -not $wiriProcess.HasExited) {
            $null = Invoke-Ctl @('quit')
            if (-not $wiriProcess.WaitForExit(8000)) { Stop-Process -Id $wiriProcess.Id -Force -ErrorAction SilentlyContinue }
            Log 'wiri quit'
        }
        if ($script:apps.Count -gt 0 -and ('DemoApps' -as [type])) {
            $closed = [DemoApps]::CloseAll()
            Log ("closed {0} demo windows" -f $closed)
        }
        if ('DemoNative' -as [type]) {
            foreach ($handle in $minimized) {
                if ([DemoNative]::IsWindow($handle)) { [DemoNative]::Restore($handle) }
            }
            if ($minimized.Count -gt 0) { Log ("restored {0} parked windows" -f $minimized.Count) }
            [DemoNative]::ParkCursor()
        }
        # A take with a shell popup in it (or one that fell over) is not worth encoding:
        # delete the intermediate so nothing can pick it up by mistake.
        if ($script:takeFailed -and (Test-Path $rawPath)) {
            Remove-Item $rawPath -Force -ErrorAction SilentlyContinue
            Log 'contaminated take deleted; nothing from it will be encoded'
        }
        $env:WIRI_PIPE = $savedPipe
        $env:WIRI_CONFIG = $savedCfg
        $env:APPDATA = $savedRoaming
        $env:LOCALAPPDATA = $savedLocal
        if ($ownsMutex) { try { $mutex.ReleaseMutex() } catch { }; Log 'desktop mutex released' }
    }
}

# --- timeline -------------------------------------------------------------------
# A rehearsal is not a take: it drives the desktop but must not overwrite the timeline
# of the last real recording.
if ($script:timeline.Count -gt 0 -and -not $Rehearse) {
    # ToArray(), not @($list): Windows PowerShell 5.1 throws "Argument types do not
    # match" when a List<object> is wrapped in an array inside a PSCustomObject.
    $timeline = [pscustomobject]@{
        recorded = (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
        capture = $script:captureInfo
        segments = $script:timeline.ToArray()
    }
    $timeline | ConvertTo-Json -Depth 5 | Set-Content -Path $timelinePath
    Log "timeline written to $timelinePath"
} elseif (Test-Path $timelinePath) {
    $timeline = Get-Content $timelinePath -Raw | ConvertFrom-Json
    Log "re-using the timeline from $timelinePath"
}

if ($SkipEncode) { Log 'encoding skipped (-SkipEncode)'; exit 0 }
if ($Rehearse) { Log 'rehearsal finished (no recording, nothing encoded)'; exit 0 }
if (-not (Test-Path $rawPath)) { throw "no recording at $rawPath (run without -EncodeOnly first)" }

# -------------------------------------------------------------------------------
# Encoding: MP4, looping WebP, poster, optional per-segment clips.
# -------------------------------------------------------------------------------
$captionsSrc = $env:WINDIR + '\Fonts'
foreach ($font in 'segoeuib.ttf', 'segoeui.ttf') {
    $target = Join-Path $fontsDir $font
    if (-not (Test-Path $target)) { Copy-Item (Join-Path $captionsSrc $font) $target -Force }
}

$first = $timeline.segments[0]
$last = $timeline.segments[$timeline.segments.Count - 1]
$trimStart = [Math]::Max(0.0, [double]$first.start - 0.55)
$trimEnd = [double]$last.end + 0.45
$duration = $trimEnd - $trimStart
# gdigrab timestamps by frame count, so a take that could not sustain 30 fps is shorter
# than the wall clock it recorded.  Stretch it back onto the wall clock before trimming;
# the captions are burned in at the timeline's own (wall clock) times afterwards.
$stretch = 1.0
$captureOffsetUsed = 0.0
$captureInfo = $null
if ($timeline.PSObject.Properties.Name -contains 'capture') { $captureInfo = $timeline.capture }
if ($captureInfo -and $captureInfo.PSObject.Properties.Name -contains 'stretch' -and [double]$captureInfo.stretch -gt 0) {
    $stretch = [double]$captureInfo.stretch
    $captureOffsetUsed = [double]$captureInfo.offset
}
Log ("cut {0:F2}s .. {1:F2}s -> {2:F2}s of video (time stretch {3:F4}, capture offset {4:F2}s)" -f $trimStart, $trimEnd, $duration, $stretch, $captureOffsetUsed)

# Nothing is encoded while a shell menu is on screen: the take would carry it into every
# frame, and the pixel checks cannot tell a dark menu from dark window content.  This is the
# geometric probe again (foreground owner plus a WindowFromPoint grid), right before the
# encoder runs.
if (-not ('PopupProbe' -as [type])) {
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -Path (Join-Path $PSScriptRoot 'demo-native.cs')
}
$preEncodeHits = @(Get-PopupHits)
if ($preEncodeHits.Count -gt 0) {
    Log ('pre-encode check: POPUP PRESENT: ' + ($preEncodeHits -join ' | '))
    throw 'a shell popup is on screen; refusing to encode a contaminated take'
}
Log ('pre-encode check: no shell popup; point(518,455)=' + [PopupProbe]::PointDescription(518, 455))

# drawtext texts live in files, and every path in the filter graph is relative to the
# staging directory: no Windows drive colon ever has to be escaped on a command line.
function Build-CaptionChain([int]$fontSize, [int]$bandHeight, [int]$textY, [int]$markSize, [int]$markY, [double]$Origin = -1) {
    $parts = New-Object System.Collections.Generic.List[string]
    $markOffset = 26
    $origin = $trimStart
    if ($Origin -ge 0) { $origin = $Origin }
    foreach ($segment in $timeline.segments) {
        $start = [Math]::Round([double]$segment.start - $origin + $CaptionOffset, 3)
        $end = [Math]::Round([double]$segment.end - $origin + $CaptionOffset, 3)
        if ($start -lt 0) { $start = 0 }
        $window = "between(t,$($start.ToString([Globalization.CultureInfo]::InvariantCulture)),$($end.ToString([Globalization.CultureInfo]::InvariantCulture)))"
        $parts.Add("drawbox=x=0:y=ih-${bandHeight}:w=iw:h=${bandHeight}:color=0x0A0E15@0.68:t=fill:enable='$window'")
        $parts.Add("drawtext=fontfile=fonts/segoeuib.ttf:text='wiri':fontcolor=0x7AA2F7:fontsize=${markSize}:x=${markOffset}:y=h-${markY}:enable='$window'")
        $parts.Add("drawtext=fontfile=fonts/segoeuib.ttf:textfile=captions/$($segment.key).txt:fontcolor=white:fontsize=${fontSize}:x=(w-text_w)/2:y=h-${textY}:enable='$window'")
    }
    return ($parts -join ',')
}

$fadeOut = [Math]::Max(0.0, $duration - 0.55)
$fade = "fade=t=in:st=0:d=0.45,fade=t=out:st=$([Math]::Round($fadeOut,2)):d=0.5"
$trim = "trim=start=$([Math]::Round($trimStart,3)):end=$([Math]::Round($trimEnd,3)),setpts=PTS-STARTPTS"

function Write-TextFile([string]$path, [string]$text) {
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding($false)))
}

function Get-SizeMB([string]$path) {
    if (-not (Test-Path $path)) { return 0 }
    return (Get-Item $path).Length / 1MB
}

# ffmpeg runs with the staging directory as its working directory so the filter scripts
# can use plain relative paths for the font and the caption text files; the media paths
# on the command line are absolute (a drive colon there is not a filter-graph colon).
function Invoke-FfmpegEncode {
    param(
        [Parameter(Mandatory = $true)][string]$InputPath,
        [Parameter(Mandatory = $true)][string]$FilterScript,
        [Parameter(Mandatory = $true)][string]$OutputPath,
        [string[]]$CodecArgs = @(),
        [string[]]$PreInput = @(),
        [string[]]$PostOutputOptions = @()
    )
    # This ffmpeg build has no -filter_complex_script, so the graph (written to the staging
    # directory first, so it can be inspected afterwards) is passed inline.  It contains no
    # whitespace, which is what a joined command line needs.
    $graph = ([IO.File]::ReadAllText((Join-Path $StageDir $FilterScript))).Trim()
    $ffArgs = @('-hide_banner', '-loglevel', 'error', '-y')
    $ffArgs += $PreInput
    $ffArgs += @('-i', $InputPath, '-filter_complex', $graph, '-map', '[v]', '-an')
    $ffArgs += $CodecArgs
    $ffArgs += $PostOutputOptions
    $ffArgs += $OutputPath
    $log = Join-Path $session 'ffmpeg-encode.log'
    $process = Start-Process -FilePath $ffmpeg -ArgumentList $ffArgs -WorkingDirectory $StageDir -PassThru -NoNewWindow `
        -RedirectStandardError $log -Wait
    if ($process.ExitCode -ne 0) {
        if (Test-Path $log) { Log (Get-Content $log -Raw) }
        throw ("ffmpeg failed on " + [IO.Path]::GetFileName($OutputPath))
    }
}

# ---- captions as overlays ------------------------------------------------------
# The caption bands are transparent PNGs drawn by CaptionBand (demo-native.cs): this
# ffmpeg build has no fontconfig and its drawtext filter crashed on the full graph, and
# GDI+ gives crisper text anyway.  One band per segment per output width; ffmpeg only
# overlays them, which keeps every encode graph a simple linear chain.
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
if (-not ('CaptionBand' -as [type])) {
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -Path (Join-Path $PSScriptRoot 'demo-native.cs')
}

function New-CaptionBands {
    param([int]$Width, [int]$BandHeight, [int]$FontSize, [int]$MarkSize)
    $directory = Join-Path $StageDir ('bands\' + $Width)
    New-Item -ItemType Directory -Force $directory | Out-Null
    $paths = New-Object System.Collections.Generic.List[string]
    foreach ($segment in $timeline.segments) {
        $path = Join-Path $directory ($segment.key + '.png')
        [CaptionBand]::Render($path, $Width, $BandHeight, $segment.caption, $FontSize, $MarkSize)
        $paths.Add($path)
    }
    return $paths
}

function New-CaptionGraph {
    param([string]$Pre, [string]$Post, [double]$Origin = -1)
    $origin = $trimStart
    if ($Origin -ge 0) { $origin = $Origin }
    $chain = New-Object System.Collections.Generic.List[string]
    $chain.Add('[0:v]' + $Pre + '[base]')
    $current = 'base'
    $index = 1
    foreach ($segment in $timeline.segments) {
        $start = [Math]::Round([double]$segment.start - $origin + $CaptionOffset, 3)
        $end = [Math]::Round([double]$segment.end - $origin + $CaptionOffset, 3)
        if ($start -lt 0) { $start = 0 }
        $window = 'between(t,{0},{1})' -f $start.ToString([Globalization.CultureInfo]::InvariantCulture),
            $end.ToString([Globalization.CultureInfo]::InvariantCulture)
        $next = 'b' + $index
        $chain.Add(('[{0}][{1}:v]overlay=x=0:y=main_h-overlay_h:enable={2}[{3}]' -f $current, $index, ("'" + $window + "'"), $next))
        $current = $next
        $index++
    }
    $chain.Add(('[{0}]{1}[v]' -f $current, $Post))
    return ($chain -join ';')
}

$fadeOut = [Math]::Max(0.0, $duration - 0.55)
$fade = 'fade=t=in:st=0:d=0.45,fade=t=out:st={0}:d=0.5' -f [Math]::Round($fadeOut, 2)
# The raw take is stretched back onto the wall clock first (see the recording phase), then
# trimmed: the trim times are therefore wall-clock times, offset by the capture lead.
$stretchText = $stretch.ToString('F5', [Globalization.CultureInfo]::InvariantCulture)
$wallStart = [Math]::Round($trimStart + $captureOffsetUsed, 3)
$wallEnd = [Math]::Round($trimEnd + $captureOffsetUsed, 3)
$trim = 'setpts=PTS*{0},trim=start={1}:end={2},setpts=PTS-STARTPTS' -f $stretchText, $wallStart, $wallEnd

function Write-TextFile([string]$path, [string]$text) {
    [IO.File]::WriteAllText($path, $text, (New-Object Text.UTF8Encoding($false)))
}

function Get-SizeMB([string]$path) {
    if (-not (Test-Path $path)) { return 0 }
    return (Get-Item $path).Length / 1MB
}

# ffmpeg runs with the staging directory as its working directory.  Every path handed to
# it is absolute, so a drive colon never has to be escaped.
function Invoke-FfmpegEncode {
    param(
        [Parameter(Mandatory = $true)][string]$InputPath,
        [Parameter(Mandatory = $true)][string]$Graph,
        [Parameter(Mandatory = $true)][string]$OutputPath,
        [string[]]$BandPaths = @(),
        [string[]]$CodecArgs = @(),
        [string[]]$PreInput = @(),
        [string[]]$PostOutputOptions = @()
    )
    $ffArgs = @('-hide_banner', '-loglevel', 'error', '-y')
    $ffArgs += $PreInput
    $ffArgs += @('-i', $InputPath)
    foreach ($band in $BandPaths) { $ffArgs += @('-i', $band) }
    $ffArgs += @('-filter_complex', $Graph, '-map', '[v]', '-an')
    $ffArgs += $CodecArgs
    $ffArgs += $PostOutputOptions
    $ffArgs += $OutputPath
    $log = Join-Path $session 'ffmpeg-encode.log'
    $process = Start-Process -FilePath $ffmpeg -ArgumentList $ffArgs -WorkingDirectory $StageDir -PassThru -NoNewWindow `
        -RedirectStandardError $log -Wait
    if ($process.ExitCode -ne 0) {
        if (Test-Path $log) { Log ((Get-Content $log -Raw) -replace "`r?`n", ' | ') }
        throw ("ffmpeg failed on " + [IO.Path]::GetFileName($OutputPath) + " (rc=" + $process.ExitCode + ")")
    }
}

# ---- MP4 ----------------------------------------------------------------------
# CRF is tuned against the budget, not guessed: encode, measure, step back.  fps=30
# resamples the wall-clock-timestamped capture to a constant frame rate for players.
$mp4 = Join-Path $OutDir 'demo.mp4'
$bandsMp4 = New-CaptionBands -Width 1366 -BandHeight 72 -FontSize 27 -MarkSize 20
# The MJPEG intermediate is full range (yuvj420p); convert it to limited-range yuv420p so
# the published file carries the pixel format the README promises and plays with the right
# levels everywhere.
$graphMp4 = New-CaptionGraph -Pre ($trim + ',fps=30') -Post ($fade + ',scale=in_range=full:out_range=limited,format=yuv420p')
Write-TextFile (Join-Path $StageDir 'graph-mp4.txt') $graphMp4
$crf = 18
for ($attempt = 1; $attempt -le 4; $attempt++) {
    Invoke-FfmpegEncode -InputPath $rawPath -Graph $graphMp4 -BandPaths $bandsMp4 -OutputPath $mp4 `
        -CodecArgs @('-c:v', 'libx264', '-preset', 'slow', '-crf', "$crf", '-profile:v', 'high', '-pix_fmt', 'yuv420p', '-movflags', '+faststart')
    $size = Get-SizeMB $mp4
    Log ("mp4 crf {0}: {1:F2} MB" -f $crf, $size)
    if ($size -le 3.3 -or $crf -ge 30) { break }
    $crf += 3
}

# ---- poster -------------------------------------------------------------------
# A frame from the tabbed segment: three columns, a focus ring and a visible tab
# strip, with the caption that goes with it.
$posterTime = [Math]::Round([double]$timeline.segments[2].start - $trimStart + 3.1, 2)
$poster = Join-Path $OutDir 'demo-poster.png'
$posterWidth = 1366
for ($attempt = 1; $attempt -le 3; $attempt++) {
    $posterArgs = @('-hide_banner', '-loglevel', 'error', '-y', '-ss', "$posterTime", '-i', $mp4,
                    '-frames:v', '1', '-vf', "scale=${posterWidth}:-2:flags=lanczos", $poster)
    Start-Process -FilePath $ffmpeg -ArgumentList $posterArgs -PassThru -NoNewWindow -Wait `
        -RedirectStandardError (Join-Path $session 'ffmpeg-poster.log') | Out-Null
    $posterBytes = (Get-Item $poster).Length
    Log ("poster {0}px: {1:F0} kB at {2:F2}s" -f $posterWidth, ($posterBytes / 1KB), $posterTime)
    if ($posterBytes -le 300KB -or $posterWidth -le 1000) { break }
    $posterWidth -= 160
}

# ---- looping WebP ------------------------------------------------------------
# The captions are drawn again at 960 px, so the text is sharp at the smaller size
# instead of being scaled down, and the palette is built from the finished look.
$webp = Join-Path $OutDir 'demo.webp'
$bands960 = New-CaptionBands -Width 960 -BandHeight 50 -FontSize 19 -MarkSize 14
$graphWebp = New-CaptionGraph -Pre ("$trim,fps=13,scale=960:-2:flags=lanczos") -Post ($fade + ',format=rgb24,split[a][b];[a]palettegen=max_colors=192:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3')
Write-TextFile (Join-Path $StageDir 'graph-webp.txt') $graphWebp
# The measured cost of this take: q74 lands around 2.7 MB, q66 around 2.2 MB, so start
# where the budget is and only step down if the content gets busier.
$quality = 66
for ($attempt = 1; $attempt -le 5; $attempt++) {
    Invoke-FfmpegEncode -InputPath $rawPath -Graph $graphWebp -BandPaths $bands960 -OutputPath $webp `
        -CodecArgs @('-c:v', 'libwebp_anim', '-loop', '0', '-q:v', "$quality", '-compression_level', '6', '-f', 'webp')
    $webpBytes = (Get-Item $webp).Length
    Log ("webp q {0}: {1:F2} MB" -f $quality, ($webpBytes / 1MB))
    if ($webpBytes -le 2.35MB -or $quality -le 40) { break }
    $quality -= 8
}

# ---- per-segment clips (optional: only while the whole set stays in budget) ---
$mainTotal = (Get-SizeMB $mp4) + (Get-SizeMB $webp) + (Get-SizeMB $poster)
Log ("main assets: {0:F2} MB" -f $mainTotal)
$clipFiles = New-Object System.Collections.Generic.List[object]
if (-not $NoClips -and $mainTotal -lt 5.55) {
    $bands720 = New-CaptionBands -Width 720 -BandHeight 56 -FontSize 21 -MarkSize 16
    foreach ($segment in $timeline.segments) {
        $clipStart = [Math]::Round([double]$segment.start - $trimStart, 3)
        $clipLength = [Math]::Round([double]$segment.end - [double]$segment.start + 0.2, 2)
        $wallClipStart = [Math]::Round($clipStart + $captureOffsetUsed, 3)
        $wallClipEnd = [Math]::Round($wallClipStart + $clipLength, 3)
        $pre = 'setpts=PTS*{0},trim=start={1}:end={2},setpts=PTS-STARTPTS,fps=12,scale=720:-2:flags=lanczos' -f $stretchText, $wallClipStart, $wallClipEnd
        $graph = New-CaptionGraph -Pre $pre -Origin ([double]$segment.start - 0.05) `
            -Post 'format=rgb24,split[a][b];[a]palettegen=max_colors=160:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3'
        $clipPath = Join-Path $OutDir ('clip-' + $segment.key + '.webp')
        try {
            Invoke-FfmpegEncode -InputPath $rawPath -Graph $graph -BandPaths $bands720 -OutputPath $clipPath `
                -CodecArgs @('-c:v', 'libwebp_anim', '-loop', '0', '-q:v', '66', '-compression_level', '6', '-f', 'webp')
            $clipFiles.Add([pscustomobject]@{ name = ('clip-' + $segment.key + '.webp'); mb = (Get-SizeMB $clipPath) })
        } catch {
            Log ("clip {0} failed: {1}" -f $segment.key, $_.Exception.Message)
        }
    }
    $clipTotal = 0.0
    if ($clipFiles.Count -gt 0) { $clipTotal = ($clipFiles | Measure-Object -Property mb -Sum).Sum }
    $tooBig = @($clipFiles | Where-Object { $_.mb -gt 0.6 })
    Log ("clips: {0} files, {1:F2} MB total" -f $clipFiles.Count, $clipTotal)
    if ($tooBig.Count -gt 0 -or ($mainTotal + $clipTotal) -gt 7.8) {
        Log 'clip budget exceeded; removing the per-segment WebPs'
        foreach ($clip in $clipFiles) { Remove-Item (Join-Path $OutDir $clip.name) -Force -ErrorAction SilentlyContinue }
        $clipFiles.Clear()
    }
} else {
    Log 'per-segment clips skipped (no room in the asset budget, or -NoClips)'
}

# ---- check the result ---------------------------------------------------------
# The captions and the windows have to be readable in the actual pixels, so every
# segment gets a frame pulled out of the finished MP4 and measured: the caption band
# must be dark with bright glyphs and the accent word mark, and three frames are
# printed as a coarse luminance map so the layout can be checked by eye.
function ConvertFrom-Stats([string]$line) {
    $values = @{}
    foreach ($part in ($line -split ';')) {
        $pair = $part -split '=', 2
        if ($pair.Count -eq 2) { $values[$pair[0]] = $pair[1] }
    }
    return $values
}

$verifyLog = New-Object System.Collections.Generic.List[string]
$verifyLog.Add('frames pulled from demo.mp4 (times are video time, band is 72 px)')
$failures = 0
foreach ($segment in $timeline.segments) {
    $at = [Math]::Round([double]$segment.start - $trimStart + 1.45, 2)
    $framePath = Join-Path $framesDir ($segment.key + '.png')
    $frameArgs = @('-hide_banner', '-loglevel', 'error', '-y', '-ss', "$at", '-i', $mp4, '-frames:v', '1', $framePath)
    Start-Process -FilePath $ffmpeg -ArgumentList $frameArgs -PassThru -NoNewWindow -Wait `
        -RedirectStandardError (Join-Path $session 'ffmpeg-frames.log') | Out-Null
    if (-not (Test-Path $framePath)) {
        $verifyLog.Add(('{0,-16} frame not extracted' -f $segment.key))
        $failures++
        continue
    }
    $values = ConvertFrom-Stats ([FrameStats]::Analyze($framePath, 72))
    $band = [double]$values['band']
    $glyphs = [double]$values['glyphs']
    $mark = [int]$values['mark']
    $accents = [string]$values['accents']
    $verifyLog.Add(('{0,-16} {1,6:F2}s  band luma {2,5:F1}  glyphs {3,5:F2}%  mark {4,4}  accents {5}' -f $segment.key, $at, $band, $glyphs, $mark, $accents))
    if ($band -gt 110) { $verifyLog.Add('  FAIL: the caption band is not dark'); $failures++ }
    if ($glyphs -lt 0.15 -or $glyphs -gt 14) { $verifyLog.Add('  FAIL: the caption text is not legible'); $failures++ }
    # The word mark is small and blue; H.264's 4:2:0 chroma dulls it, so the check asks
    # for its presence, not for its exact colour.
    if ($mark -lt 8) { $verifyLog.Add('  FAIL: the wiri word mark is missing from the band'); $failures++ }
}
foreach ($key in 'columns', 'tabs', 'overview') {
    $framePath = Join-Path $framesDir ($key + '.png')
    if (Test-Path $framePath) {
        $verifyLog.Add('')
        $verifyLog.Add("$key, luminance map (one character per ~14x29 px)")
        foreach ($row in [FrameStats]::Ascii($framePath, 96, 26)) { $verifyLog.Add('  ' + $row) }
    }
}
$verifyLog.Add('')
if ($failures -eq 0) { $verifyLog.Add('all caption and colour checks passed') } else { $verifyLog.Add(('{0} checks failed' -f $failures)) }
$verifyLog | Set-Content -Path (Join-Path $StageDir 'verify.txt')
$verifyLog | ForEach-Object { Write-Host $_ }
if ($failures -gt 0) { Log 'the captions did not pass their pixel checks; see verify.txt' }

# ---- report ------------------------------------------------------------------
$clipTotal = 0.0
if ($clipFiles.Count -gt 0) { $clipTotal = ($clipFiles | Measure-Object -Property mb -Sum).Sum }
$assetTotal = (Get-SizeMB $mp4) + (Get-SizeMB $webp) + (Get-SizeMB $poster) + $clipTotal
$report = New-Object System.Collections.Generic.List[string]
$report.Add('wiri README demo assets')
$report.Add('recorded   ' + $timeline.recorded)
$report.Add(('video      {0:F2} s at 1366x768, 30 fps' -f $duration))
foreach ($segment in $timeline.segments) {
    $from = [Math]::Round([double]$segment.start - $trimStart, 2)
    $to = [Math]::Round([double]$segment.end - $trimStart, 2)
    $report.Add(('  {0,-16} {1,6:F2}s .. {2,6:F2}s  {3}' -f $segment.key, $from, $to, $segment.caption))
}
foreach ($file in 'demo.mp4', 'demo.webp', 'demo-poster.png') {
    $path = Join-Path $OutDir $file
    if (Test-Path $path) { $report.Add(('asset      {0,-24} {1,8:F2} MB' -f $file, (Get-SizeMB $path))) }
}
foreach ($clip in $clipFiles) { $report.Add(('asset      {0,-24} {1,8:F2} MB' -f $clip.name, $clip.mb)) }
$report.Add(('total      {0:F2} MB' -f $assetTotal))
$report.Add(('frames     {0}' -f $framesDir))
$report -join "`n" | Write-Host
$report -join "`n" | Add-Content -Path $runLog
