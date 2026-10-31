# Lane EVIDENCE (H95): the precise desktop checks that were missing for parity rows whose
# behaviour exists and is unit-tested but had no live observation of exactly that behaviour:
#   * the overview's workspace shadow is painted where `overview { workspace-shadow { } }`
#     puts it (niri niri-config/src/misc.rs workspace-shadow; wiri
#     overlay/overview_appearance.rs::background, offset/spread/softness scaled by the camera
#     zoom), and `off` stops it;
#   * the deprecated per-output `background-color` (niri moved it to `layout`) reaches the
#     overview backdrop when `overview { backdrop-color }` is not written;
#   * `center-column` centres the focused column at the view centre for first and last
#     columns of different fixed widths (niri src/input/mod.rs CenterColumn ->
#     src/layout/scrolling.rs centre_focused_column; wiri engine::center_focused_column uses
#     reveal_with_policy(Always) and allows the negative leading space);
#   * the tab indicator paints niri's urgent colour on the urgent tab, overriding the tab
#     selection, and goes back to the inactive colour when urgency is cleared (niri
#     src/layout/scrolling.rs tab_indicator: urgent / active / inactive; wiri
#     layout/engine/decor.rs::tab_specs, unit-tested only until now);
#   * Mod+left-drag moves a floating window, Mod+right-drag resizes it, and Escape cancels
#     the grab back to the pre-drag geometry (niri src/layout/floating.rs interactive move and
#     resize for the floating layer; wiri input/low_level_hook.rs grab + engine/interactive.rs
#     + layout/engine/native_move.rs::finish_native_move's FloatHome branch);
#   * `toggle-keyboard-shortcuts-inhibit` swallows exactly the enables that are inhibitable,
#     while the toggle itself and `allow-inhibiting=false` keeps working (niri
#     src/input/mod.rs KeyboardShortcutsInhibitState; wiri input/keyboard.rs KeyboardEngine).
#
# What these gates can and cannot show:
#   * Every observation is a real screen pixel (CopyFromScreen) or a real DWM-visible window
#     rectangle; the mouse input is real SendInput through the low-level hook, not posted
#     messages, except for the overview camera which is driven through the normal ctl actions.
#   * This machine has one 1366x768 output at 100%. Nothing here qualifies mixed DPI, a second
#     output, a touchpad, a pen or a touchscreen; the rows that still need those stay partial.
#   * The tab gate and the floating gate need the engine's own focus on an owned form, so a
#     foreign program taking the foreground during the run fails them honestly instead of
#     passing vacuously.
# Every gate group has its own try/catch: one failing group leaves the others' results alone.
$h95Keys = @(
    'h95_overview_workspace_shadow_paints_and_off_removes_it',
    'h95_deprecated_output_background_colour_is_the_overview_backdrop',
    'h95_center_column_centres_variable_width_edge_columns',
    'h95_tab_indicator_urgent_colour_overrides_the_selection',
    'h95_mod_drag_moves_a_floating_window',
    'h95_mod_drag_resizes_a_floating_window',
    'h95_mod_drag_escape_cancels_to_the_pre_drag_rect',
    'h95_shortcuts_inhibit_blocks_inhibitable_binds_only'
)
foreach ($h95Key in $h95Keys) { $summary[$h95Key] = $false }

if (-not ('LaneEvidenceNative' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing, System.Core, System.Windows.Forms -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneEvidenceNative {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint data, flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort vk, scan; public uint flags, time; public UIntPtr extra; }
    [StructLayout(LayoutKind.Explicit)] struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mouse; [FieldOffset(0)] public KEYBDINPUT key; }
    [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public INPUTUNION data; }
    const uint MOUSE = 0, KEYBOARD = 1;
    const uint ABSOLUTE = 0x8000, MOVE = 0x0001;
    const uint LEFTDOWN = 0x0002, LEFTUP = 0x0004, RIGHTDOWN = 0x0008, RIGHTUP = 0x0010, KEYUP = 0x0002;
    // Not REINJECTED (0x57495257) and not ACTIVATION_INPUT_TAG (0x57495249).
    static readonly UIntPtr Tag = new UIntPtr(0x4c414e45);
    [DllImport("user32.dll", SetLastError = true)] static extern uint SendInput(uint count, INPUT[] inputs, int size);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT p);
    [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr h, uint flags);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr FindWindowW(string cls, string title);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern IntPtr GetPropW(IntPtr h, string name);
    [DllImport("user32.dll", SetLastError = true)] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    static void Send(INPUT i) {
        if (SendInput(1, new INPUT[] { i }, Marshal.SizeOf(typeof(INPUT))) != 1)
            throw new InvalidOperationException("H95 SendInput refused: " + Marshal.GetLastWin32Error());
    }
    public static int[] Rect(IntPtr h) {
        RECT r;
        if (!GetWindowRect(h, out r)) throw new InvalidOperationException("H95 GetWindowRect failed");
        return new int[] { r.L, r.T, r.R, r.B };
    }
    public static int[] Cursor() { POINT p; GetCursorPos(out p); return new int[] { p.X, p.Y }; }
    public static IntPtr Root(int x, int y) { POINT p; p.X = x; p.Y = y; return GetAncestor(WindowFromPoint(p), 2); }
    public static IntPtr FindClass(string cls) { return FindWindowW(cls, null); }
    public static long Prop(IntPtr h, string name) { return GetPropW(h, name).ToInt64(); }
    public static void Move(int x, int y) {
        RECT screen = new RECT();
        screen.L = System.Windows.Forms.SystemInformation.VirtualScreen.Left;
        screen.T = System.Windows.Forms.SystemInformation.VirtualScreen.Top;
        screen.R = System.Windows.Forms.SystemInformation.VirtualScreen.Width;
        screen.B = System.Windows.Forms.SystemInformation.VirtualScreen.Height;
        INPUT i = new INPUT(); i.type = MOUSE;
        i.data.mouse.dx = (int)(((long)(x - screen.L) * 65536 + 32768) / screen.R);
        i.data.mouse.dy = (int)(((long)(y - screen.T) * 65536 + 32768) / screen.B);
        i.data.mouse.flags = ABSOLUTE | MOVE; i.data.mouse.extra = Tag;
        Send(i);
    }
    public static void Button(int vk, bool up) {
        INPUT i = new INPUT(); i.type = MOUSE;
        if (vk == 1) i.data.mouse.flags = up ? LEFTUP : LEFTDOWN;
        else if (vk == 2) i.data.mouse.flags = up ? RIGHTUP : RIGHTDOWN;
        else throw new InvalidOperationException("H95 unknown button " + vk);
        i.data.mouse.extra = Tag;
        Send(i);
    }
    public static void Key(int vk, bool up) {
        INPUT i = new INPUT(); i.type = KEYBOARD;
        i.data.key.vk = (ushort)vk; i.data.key.flags = up ? KEYUP : 0; i.data.key.extra = Tag;
        Send(i);
    }
    public static IntPtr Pack(int x, int y) { return new IntPtr(unchecked((int)(((uint)(ushort)y << 16) | (ushort)x))); }
}
'@
}

$h95Dir = Join-Path $D 'lane-evidence'
$null = New-Item -ItemType Directory -Path $h95Dir -Force
$h95Details = [ordered]@{
    reference = 'niri e19df12cf8'
    checks = [ordered]@{}
    observations = (New-Object 'System.Collections.Generic.List[object]')
    limitations = @(
        'One 1366x768 output at 100%; no mixed DPI, second output, touchpad, pen or touchscreen claim.',
        'Overview pointer/camera state is read from the host properties the engine publishes; the shadow and backdrop are real screen pixels.',
        'Tab and floating gates require the engine focus on an owned form; foreign foreground interference fails them.',
        'Floating drags are real SendInput Mod+button grabs; no OLE drag or touch/pen path is exercised.'
    )
}
function EvGate([string]$name, [bool]$pass, $observed) {
    $summary[$name] = $pass
    $h95Details.checks[$name] = [ordered]@{ pass = $pass; observed = $observed }
    $h95Details | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 (Join-Path $h95Dir 'evidence.json')
}
function EvCtl([string]$tag, [string[]]$words) {
    if ($null -eq $h95Process -or $h95Process.HasExited) { throw "H95 daemon is not alive: $tag" }
    $text = Ctl ('h95-' + $tag) $words
    if ($steps.Count -eq 0 -or [string]$steps[$steps.Count - 1].rc -ne '0') { throw "H95 CLI failed: $tag" }
    return $text
}
function EvJson([string]$tag, [string[]]$words) {
    $reply = (EvCtl $tag (@('--json') + $words)) | ConvertFrom-Json
    if ($reply.success -ne $true) { throw "H95 request was rejected: $tag" }
    return $reply
}
function EvAction([string]$tag, [string[]]$words) {
    $null = EvJson $tag (@('action') + $words)
    Start-Sleep -Milliseconds 500
}
function EvTyped([string]$tag, $request) {
    if ($null -eq $h95Process -or $h95Process.HasExited) { throw "H95 daemon is not alive: $tag" }
    $json = $request | ConvertTo-Json -Depth 8 -Compress
    $reply = (EvCtl $tag @('raw-request', (Q $json))) | ConvertFrom-Json
    if ($null -eq $reply.PSObject.Properties['Ok'] -or $reply.Ok -ne 'Handled') {
        throw "H95 typed request was not handled: $tag $($reply | ConvertTo-Json -Compress) (request $json)"
    }
    Start-Sleep -Milliseconds 550
    return $reply
}
function EvPixel([int]$x, [int]$y) {
    $bmp = New-Object Drawing.Bitmap 1, 1
    $g = [Drawing.Graphics]::FromImage($bmp)
    try {
        $g.CopyFromScreen($x, $y, 0, 0, $bmp.Size)
        $c = $bmp.GetPixel(0, 0)
        return @([int]$c.R, [int]$c.G, [int]$c.B)
    } finally { $g.Dispose(); $bmp.Dispose() }
}
function EvNearColor($rgb, $expected, [int]$tolerance = 14) {
    if ($null -eq $rgb -or $rgb.Count -ne 3) { return $false }
    for ($i = 0; $i -lt 3; $i++) {
        if ([Math]::Abs([int]$rgb[$i] - [int]$expected[$i]) -gt $tolerance) { return $false }
    }
    return $true
}
function EvRgb([string]$hex) {
    $h = $hex.TrimStart('#')
    return @([Convert]::ToInt32($h.Substring(0, 2), 16),
             [Convert]::ToInt32($h.Substring(2, 2), 16),
             [Convert]::ToInt32($h.Substring(4, 2), 16))
}
function EvHost { return [LaneEvidenceNative]::FindClass('WiriThumbnailOverviewHost') }
function EvProp([IntPtr]$h, [string]$name) {
    if ($h -eq [IntPtr]::Zero) { return 0 }
    return [LaneEvidenceNative]::Prop($h, ('WiriOverview' + $name))
}
function EvWaitOverview([int]$zoomMillionths, [int]$seconds = 5) {
    for ($i = 0; $i -lt ($seconds * 10); $i++) {
        $h = EvHost
        if ($h -ne [IntPtr]::Zero -and (EvProp $h 'ZoomMillionths') -eq $zoomMillionths -and
                (EvProp $h 'WorkspaceCount') -ge 1) { return $h }
        Start-Sleep -Milliseconds 100
    }
    throw 'H95 overview host did not reach the configured camera'
}
function EvWaitOverviewGone([int]$seconds = 5) {
    for ($i = 0; $i -lt ($seconds * 10); $i++) {
        if ((EvHost) -eq [IntPtr]::Zero) { return $true }
        Start-Sleep -Milliseconds 100
    }
    return $false
}
function EvReload([string]$text, [string]$tag) {
    [IO.File]::WriteAllText($cfg, $text, (New-Object Text.UTF8Encoding($false)))
    $null = EvCtl ('reload-' + $tag) @('reload-config')
    Start-Sleep -Milliseconds 800
}
function EvWinList([string]$tag) {
    return @((EvCtl $tag @('--json', 'window-list') | ConvertFrom-Json).result)
}
function EvOwned([IntPtr]$h) {
    if (-not $h95Forms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
        throw "H95 refused a form it does not own: $h"
    }
}
# Close every fixture this lane opened so one group's columns cannot leak into the next
# group's first/last-column or foreign-tile preconditions.
function EvCloseForms {
    foreach ($form in $h95Forms) {
        if ([LT]::IsWindow($form)) { [TestForms]::CloseForm($form) }
    }
    $h95Forms.Clear()
    Start-Sleep -Milliseconds 900
}
# Wait until the engine reports exactly this window as visible and foreground, so a colour or
# geometry observation cannot silently describe a foreign program's window.
function EvOwnForeground([IntPtr]$h) {
    EvOwned $h
    FocusForm $h
    for ($i = 0; $i -lt 60; $i++) {
        if ([LT]::GetForegroundWindow() -eq $h) { Start-Sleep -Milliseconds 250; return $true }
        Start-Sleep -Milliseconds 50
    }
    throw 'H95 owned form did not take the foreground'
}
# Held input must never outlive a group, whatever went wrong inside it.
$h95Input = [pscustomobject]@{ alt = $false; left = $false; right = $false }
function EvReleaseAll {
    try { if ($h95Input.left) { [LaneEvidenceNative]::Button(1, $true); $h95Input.left = $false } } catch { }
    try { if ($h95Input.right) { [LaneEvidenceNative]::Button(2, $true); $h95Input.right = $false } } catch { }
    try { if ($h95Input.alt) { [LaneEvidenceNative]::Key(0xA4, $true); $h95Input.alt = $false } } catch { }
}
function EvAltDown { if (-not $h95Input.alt) { [LaneEvidenceNative]::Key(0xA4, $false); $h95Input.alt = $true; Start-Sleep -Milliseconds 90 } }
function EvAltUp { if ($h95Input.alt) { [LaneEvidenceNative]::Key(0xA4, $true); $h95Input.alt = $false; Start-Sleep -Milliseconds 90 } }
function EvLeftDown { if (-not $h95Input.left) { [LaneEvidenceNative]::Button(1, $false); $h95Input.left = $true } }
function EvLeftUp { if ($h95Input.left) { [LaneEvidenceNative]::Button(1, $true); $h95Input.left = $false } }
function EvRightDown { if (-not $h95Input.right) { [LaneEvidenceNative]::Button(2, $false); $h95Input.right = $true } }
function EvRightUp { if ($h95Input.right) { [LaneEvidenceNative]::Button(2, $true); $h95Input.right = $false } }
# A real pointer move through SendInput: WH_MOUSE_LL sees it, SetCursorPos alone does not.
function EvMoveTo([int]$x, [int]$y, [int]$settleMs = 60) {
    [LaneEvidenceNative]::Move($x, $y)
    Start-Sleep -Milliseconds $settleMs
}
# Ctrl+Alt+<F-key> through the low-level keyboard hook, the same way lane CFG presses its binds.
function EvChord([byte]$key) {
    [LT]::Key(0x11, $false); [LT]::Key(0x12, $false)
    [LT]::Key($key, $false); Start-Sleep -Milliseconds 90; [LT]::Key($key, $true)
    [LT]::Key(0x12, $true); [LT]::Key(0x11, $true)
}
function EvRect([IntPtr]$h) {
    # DWM's extended frame bounds: what the user sees, without the invisible resize border.
    $v = [LT]::Visible($h)
    return [pscustomobject]@{ L = [int]$v.L; T = [int]$v.T; W = ([int]$v.R - [int]$v.L); H = ([int]$v.B - [int]$v.T) }
}
function EvFrameRect([IntPtr]$h) {
    $r = [LaneEvidenceNative]::Rect($h)
    return [pscustomobject]@{ L = $r[0]; T = $r[1]; W = ($r[2] - $r[0]); H = ($r[3] - $r[1]) }
}
function EvNear($a, $b, [int]$tolerance = 6) {
    return ([Math]::Abs([int]$a - [int]$b) -le $tolerance)
}
# The two tab sample points of a `position "left"` indicator, from its own host rectangle:
# 2 px margin, `gaps-between-tabs 14`, an even split of the remaining length.
function EvTabPoints($r) {
    $long = $r.H - 4
    $first = [int][Math]::Ceiling(($long - 14) / 2.0)
    $second = $long - 14 - $first
    if ($first -lt 8 -or $second -lt 8) { throw 'H95 tab indicator too short for unambiguous probes' }
    $cross = $r.L + 8
    $start = $r.T + 2
    return @(
        [pscustomobject]@{ X = $cross; Y = ($start + [int]($first / 2)) },
        [pscustomobject]@{ X = $cross; Y = ($start + $first + 14 + [int]($second / 2)) }
    )
}

$h95Base = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
overview { zoom 0.5; backdrop-color "#102030"; workspace-shadow { off; }; }
'@
$h95Process = $null
$h95Forms = New-Object System.Collections.ArrayList
try {
    $h95Process = Start-WiriWithConfig $h95Base (Join-Path $h95Dir 'base.stderr.log')
    if ($null -eq $h95Process -or $h95Process.HasExited) { throw 'H95 daemon did not start' }
    $null = EvWinList 'start'

    # ---- Group 1a: `overview { workspace-shadow { } }` is painted where niri puts it ------
    try {
        $h95ShadowOn = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 0
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
overview {
    zoom 0.5
    backdrop-color "#102030"
    workspace-shadow { offset y=10; softness 0; spread 0; color "#ff0000"; }
}
'@
        EvReload $h95ShadowOn 'shadow-on'
        $h95Form = [TestForms]::Open('wiri-h95-shadow', 80, 120, 160); $h95Forms.Add($h95Form) | Out-Null
        Start-Sleep -Milliseconds 1500
        EvAction 'open-overview' @('toggle-overview')
        $h95Host = EvWaitOverview 500000
        # The active workspace is centred at half height; the 50 px workspace gap is scaled to
        # 25 px, so 2 px below the bottom edge is inside the offset-10 (scaled to 5 px) band and
        # above the next workspace.
        $h95X = [int]((EvProp $h95Host 'Workspace0X') + (EvProp $h95Host 'Workspace0W') / 2)
        $h95Bottom = [int]((EvProp $h95Host 'Workspace0Y') + (EvProp $h95Host 'Workspace0H'))
        Start-Sleep -Milliseconds 700
        $h95Red = EvPixel $h95X ($h95Bottom + 2)
        $h95OnPass = (EvNearColor $h95Red (EvRgb '#ff0000') 40)
        $h95Details.observations.Add([ordered]@{ stage = 'shadow-on'; x = $h95X; y = ($h95Bottom + 2); rgb = $h95Red })
        # niri's `off`: the same point is the configured backdrop again.
        $h95ShadowOff = $h95ShadowOn.Replace('workspace-shadow { offset y=10; softness 0; spread 0; color "#ff0000"; }', 'workspace-shadow { off; }')
        EvReload $h95ShadowOff 'shadow-off'
        $h95Host = EvWaitOverview 500000
        Start-Sleep -Milliseconds 700
        $h95BackdropPixels = @()
        for ($i = 0; $i -lt 30; $i++) {
            $h95BackdropPixels = EvPixel $h95X ($h95Bottom + 2)
            if (EvNearColor $h95BackdropPixels (EvRgb '#102030') 16) { break }
            Start-Sleep -Milliseconds 150
        }
        $h95OffPass = EvNearColor $h95BackdropPixels (EvRgb '#102030') 16
        $h95Details.observations.Add([ordered]@{ stage = 'shadow-off'; x = $h95X; y = ($h95Bottom + 2); rgb = $h95BackdropPixels })
        EvGate 'h95_overview_workspace_shadow_paints_and_off_removes_it' ($h95OnPass -and $h95OffPass) @{
            on = $h95Red; off = $h95BackdropPixels; x = $h95X; y = ($h95Bottom + 2)
        }
        Shot 'h95-overview-shadow'
        EvAction 'close-overview' @('close-overview')
        if (-not (EvWaitOverviewGone)) { throw 'H95 overview host did not retire' }
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-shadow-error.txt')
        $h95Details['error-shadow'] = [string]$_
        try { $null = EvCtl 'shadow-recover-close' @('action', 'close-overview') } catch { }
    }

    # ---- Group 1b: the deprecated per-output `background-color` is the overview backdrop ---
    try {
        $h95Outputs = (EvJson 'outputs' @('outputs')).result.outputs
        $h95Names = @($h95Outputs.PSObject.Properties | ForEach-Object { $_.Name })
        if ($h95Names.Count -lt 1) { throw 'H95 could not read the focused output name' }
        $h95Name = [string]$h95Names[0]
        $h95Deprecated = @"
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 0
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
output r#"$h95Name"# { background-color "#204060"; }
overview { zoom 0.5; workspace-shadow { off; }; }
"@
        EvReload $h95Deprecated 'deprecated-background'
        $h95Form2 = [TestForms]::Open('wiri-h95-backdrop', 90, 130, 170); $h95Forms.Add($h95Form2) | Out-Null
        Start-Sleep -Milliseconds 1500
        EvAction 'open-overview-2' @('open-overview')
        $h95Host = EvWaitOverview 500000
        Start-Sleep -Milliseconds 900
        $h95Published = [int](EvProp $h95Host 'BackdropRgb')
        $h95GapX = [int]((EvProp $h95Host 'Workspace0X') + (EvProp $h95Host 'Workspace0W') / 2)
        $h95GapY = [int]((EvProp $h95Host 'Workspace0Y') - 6)
        $h95GapPixels = EvPixel $h95GapX $h95GapY
        $h95Details.observations.Add([ordered]@{ stage = 'deprecated-background'; output = $h95Name;
            published = ('0x{0:x6}' -f $h95Published); x = $h95GapX; y = $h95GapY; rgb = $h95GapPixels })
        EvGate 'h95_deprecated_output_background_colour_is_the_overview_backdrop' (
            $h95Published -eq 0x204060 -and (EvNearColor $h95GapPixels (EvRgb '#204060') 16)) @{
            output = $h95Name; published = ('0x{0:x6}' -f $h95Published); rgb = $h95GapPixels
        }
        EvAction 'close-overview-2' @('close-overview')
        if (-not (EvWaitOverviewGone)) { throw 'H95 overview host did not retire after the backdrop check' }
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-backdrop-error.txt')
        $h95Details['error-backdrop'] = [string]$_
        try { $null = EvCtl 'backdrop-recover-close' @('action', 'close-overview') } catch { }
    }

    # ---- Group 2: `center-column` centres variable-width first and last columns -------------
    try {
        $h95Centered = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
'@
        EvReload $h95Centered 'centre-column'
        EvCloseForms
        $h95Narrow = [TestForms]::Open('wiri-h95-narrow', 70, 140, 90); $h95Forms.Add($h95Narrow) | Out-Null
        Start-Sleep -Milliseconds 1400
        EvAction 'narrow-width' @('set-column-width', '320')
        $h95Wide = [TestForms]::Open('wiri-h95-wide', 70, 110, 200); $h95Forms.Add($h95Wide) | Out-Null
        Start-Sleep -Milliseconds 1400
        EvAction 'wide-width' @('set-column-width', '640')
        # The wide window opened as a new column right of the narrow one and has the focus.
        EvOwnForeground $h95Wide
        EvAction 'focus-narrow' @('focus-column-left')
        $null = EvOwnForeground $h95Narrow
        $h95NarrowBefore = EvRect $h95Narrow
        EvAction 'centre-narrow' @('center-column')
        $h95NarrowAfter = EvRect $h95Narrow
        $h95Work = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea
        $h95Centre = [int]($h95Work.Left + $h95Work.Width / 2)
        $h95NarrowMid = [int]($h95NarrowAfter.L + $h95NarrowAfter.W / 2)
        $h95Details.observations.Add([ordered]@{ stage = 'centre-narrow'; before = $h95NarrowBefore;
            after = $h95NarrowAfter; centre = $h95Centre })
        $h95NarrowPass = ($h95NarrowAfter.W -ge 317 -and $h95NarrowAfter.W -le 323 -and
            (EvNear $h95NarrowMid $h95Centre 6) -and $h95NarrowAfter.L -gt ($h95NarrowBefore.L + 40))
        EvAction 'focus-wide' @('focus-column-right')
        $null = EvOwnForeground $h95Wide
        $h95WideBefore = EvRect $h95Wide
        EvAction 'centre-wide' @('center-column')
        $h95WideAfter = EvRect $h95Wide
        $h95WideMid = [int]($h95WideAfter.L + $h95WideAfter.W / 2)
        $h95Details.observations.Add([ordered]@{ stage = 'centre-wide'; before = $h95WideBefore;
            after = $h95WideAfter; centre = $h95Centre })
        # The wide column is wider than the screen would show at the centre only if it fits; 640
        # fits in the 1366 px work area with the 16 px gaps.  Its minimal-reveal position can
        # already be close to the centre, so the delta is not asserted for it.
        $h95WidePass = ($h95WideAfter.W -ge 637 -and $h95WideAfter.W -le 643 -and
            (EvNear $h95WideMid $h95Centre 6))
        EvGate 'h95_center_column_centres_variable_width_edge_columns' ($h95NarrowPass -and $h95WidePass) @{
            narrow = $h95NarrowAfter; wide = $h95WideAfter; centre = $h95Centre
        }
        Shot 'h95-center-column'
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-center-error.txt')
        $h95Details['error-center'] = [string]$_
    }

    # ---- Group 3: the tab indicator's urgent colour --------------------------------------
    try {
        $h95Tabs = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
    tab-indicator {
        place-within-column true
        position "left"
        gap 8
        width 12
        length total-proportion=0.64
        gaps-between-tabs 14
        corner-radius 4
        active-color "#e03040"; inactive-color "#20b050"; urgent-color "#3050f0";
    }
}
'@
        EvReload $h95Tabs 'tab-urgent'
        EvCloseForms
        # The desktop's other applications are adopted into the first workspace, so the tab
        # fixtures get a fresh workspace of their own: the "no foreign tiles in this strip row"
        # precondition below then holds by construction instead of depending on what the rest of
        # the desktop holds, and the two fixtures are that row's first and second column.
        EvAction 'tab-workspace' @('focus-workspace', '2')
        $h95TabA = [TestForms]::Open('wiri-h95-tab-a', 60, 160, 60); $h95Forms.Add($h95TabA) | Out-Null
        Start-Sleep -Milliseconds 1400
        $h95TabB = [TestForms]::Open('wiri-h95-tab-b', 160, 60, 160); $h95Forms.Add($h95TabB) | Out-Null
        Start-Sleep -Milliseconds 1600
        $h95List = EvWinList 'tabs-before'
        $h95EA = WinOf $h95List $h95TabA
        $h95EB = WinOf $h95List $h95TabB
        if ($null -eq $h95EA -or $null -eq $h95EB -or
                (ColOf $h95List $h95TabA) -lt 0 -or
                (ColOf $h95List $h95TabB) -ne ((ColOf $h95List $h95TabA) + 1) -or
                $h95EA.workspace -ne $h95EB.workspace) {
            throw 'H95 owned tab probes are not adjacent columns of one workspace'
        }
        $h95Foreign = @($h95List | Where-Object {
            $_.workspace -eq $h95EA.workspace -and $null -ne $_.column -and [int]$_.column -ge 0 -and
            [int64]$_.hwnd -notin @([int64]$h95TabA, [int64]$h95TabB)
        })
        if ($h95Foreign.Count -ne 0) { throw 'H95 refuses a workspace that holds foreign tiles' }
        EvOwnForeground $h95TabA
        EvAction 'consume-a' @('consume-window-into-column')
        EvAction 'tabbed' @('set-column-display', 'tabbed')
        $h95Visible = @($h95Forms | Where-Object { [LT]::IsWindow($_) -and [LT]::IsWindowVisible($_) -and
            ([int64]$_ -eq [int64]$h95TabA -or [int64]$_ -eq [int64]$h95TabB) })
        if ($h95Visible.Count -ne 1) { throw 'H95 tabbed column does not show exactly one owned tab' }
        $h95Shown = [IntPtr]$h95Visible[0]
        $h95Hidden = if ([int64]$h95Shown -eq [int64]$h95TabA) { $h95TabB } else { $h95TabA }
        $h95Hosts = @([LT]::TopLevel() | Where-Object { [LT]::Title($_) -like 'WiriDecor Tabs *' })
        if ($h95Hosts.Count -ne 1) { throw "H95 expected one owned tab indicator host, saw $($h95Hosts.Count)" }
        $h95Indicator = EvFrameRect $h95Hosts[0]
        if ($h95Indicator.W -lt 8 -or $h95Indicator.H -lt 60) { throw 'H95 tab indicator host is too small to sample' }
        $h95Points = EvTabPoints $h95Indicator
        # The indicator fades a whole column 50% when the column does not hold the engine's
        # keyboard focus, so sample only with one of this column's tabs foreground and focused.
        $h95Fg = [LT]::GetForegroundWindow()
        if ([int64]$h95Fg -ne [int64]$h95TabA -and [int64]$h95Fg -ne [int64]$h95TabB) {
            $null = EvOwnForeground $h95Shown
        }
        if ([int64][LT]::GetForegroundWindow() -ne [int64]$h95TabA -and
                [int64][LT]::GetForegroundWindow() -ne [int64]$h95TabB) {
            throw 'H95 tab column lost the foreground before sampling'
        }
        Start-Sleep -Milliseconds 400
        $h95First = EvPixel $h95Points[0].X $h95Points[0].Y
        $h95Second = EvPixel $h95Points[1].X $h95Points[1].Y
        $h95Active = EvRgb '#e03040'; $h95Inactive = EvRgb '#20b050'; $h95Urgent = EvRgb '#3050f0'
        $h95FirstActive = EvNearColor $h95First $h95Active
        $h95SecondActive = EvNearColor $h95Second $h95Active
        $h95FirstInactive = EvNearColor $h95First $h95Inactive
        $h95SecondInactive = EvNearColor $h95Second $h95Inactive
        $h95SelectedPoint = if ($h95FirstActive) { $h95Points[0] } elseif ($h95SecondActive) { $h95Points[1] } else { $null }
        $h95OtherPoint = if ($h95FirstActive) { $h95Points[1] } elseif ($h95SecondActive) { $h95Points[0] } else { $null }
        $h95Painted = ($null -ne $h95SelectedPoint) -and
            (($h95FirstActive -and $h95SecondInactive) -or ($h95SecondActive -and $h95FirstInactive))
        $h95Details.observations.Add([ordered]@{ stage = 'tab-paints'; points = $h95Points;
            first = $h95First; second = $h95Second; shown = [int64]$h95Shown; hidden = [int64]$h95Hidden })
        if (-not $h95Painted) { throw 'H95 tab indicator did not paint the configured active/inactive colours' }
        # niri: urgency overrides the tab selection, without switching the tab.
        $null = EvTyped 'urgent-hidden' @{ Action = @{ SetWindowUrgent = @{ id = [int64]$h95Hidden } } }
        Start-Sleep -Milliseconds 500
        $h95UrgentPixels = EvPixel $h95OtherPoint.X $h95OtherPoint.Y
        $h95SelectedAfter = EvPixel $h95SelectedPoint.X $h95SelectedPoint.Y
        $h95StillShown = @($h95Forms | Where-Object { [LT]::IsWindow($_) -and [LT]::IsWindowVisible($_) -and
            ([int64]$_ -eq [int64]$h95TabA -or [int64]$_ -eq [int64]$h95TabB) })
        $h95UrgentPass = (EvNearColor $h95UrgentPixels $h95Urgent) -and
            (EvNearColor $h95SelectedAfter $h95Active) -and $h95StillShown.Count -eq 1 -and
            [int64]$h95StillShown[0] -eq [int64]$h95Shown
        $h95Details.observations.Add([ordered]@{ stage = 'tab-urgent'; urgent = $h95UrgentPixels;
            selected = $h95SelectedAfter; still_shown = @($h95StillShown | ForEach-Object { [int64]$_ }) })
        $null = EvTyped 'unurgent-hidden' @{ Action = @{ UnsetWindowUrgent = @{ id = [int64]$h95Hidden } } }
        Start-Sleep -Milliseconds 500
        $h95Restored = EvPixel $h95OtherPoint.X $h95OtherPoint.Y
        $h95RestorePass = EvNearColor $h95Restored $h95Inactive
        $h95Details.observations.Add([ordered]@{ stage = 'tab-unurgent'; restored = $h95Restored })
        EvGate 'h95_tab_indicator_urgent_colour_overrides_the_selection' (
            $h95Painted -and $h95UrgentPass -and $h95RestorePass) @{
            first = $h95First; second = $h95Second; urgent = $h95UrgentPixels;
            selected = $h95SelectedAfter; restored = $h95Restored
        }
        Shot 'h95-tab-urgent'
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-tab-error.txt')
        $h95Details['error-tab'] = [string]$_
    }

    # ---- Group 4: Mod+button grabs on a floating window ----------------------------------
    try {
        $h95Floats = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
window-rule { match title="^wiri-h95-float$"; open-floating true; }
'@
        EvReload $h95Floats 'floating'
        EvCloseForms
        $h95Float = [TestForms]::Open('wiri-h95-float', 90, 90, 200); $h95Forms.Add($h95Float) | Out-Null
        Start-Sleep -Milliseconds 1800
        $h95FloatEntry = WinOf (EvWinList 'float-before') $h95Float
        if ($null -eq $h95FloatEntry -or -not $h95FloatEntry.floating) {
            throw 'H95 opened fixture is not a floating window'
        }
        # x/y are niri's PositionChange values (`SetFixed`, `SetProportion`, `AdjustFixed`, ...),
        # not bare pixels: a number here is rejected while the request is decoded.
        $null = EvTyped 'float-place' @{ Action = @{ MoveFloatingWindow = @{
            id = [int64]$h95Float; x = @{ SetFixed = 300.0 }; y = @{ SetFixed = 200.0 } } } }
        Start-Sleep -Milliseconds 700
        $h95Before = EvRect $h95Float
        if ($h95Before.W -lt 150 -or $h95Before.H -lt 100) { throw 'H95 floating fixture has no usable rectangle' }

        # --- h95_mod_drag_moves_a_floating_window ---
        $h95StartX = [int]($h95Before.L + $h95Before.W / 2)
        $h95StartY = [int]($h95Before.T + $h95Before.H / 3)
        EvMoveTo $h95StartX $h95StartY 120
        if ([LaneEvidenceNative]::Root($h95StartX, $h95StartY).ToInt64() -ne [int64]$h95Float) {
            throw 'H95 pointer is not over the floating fixture before the move grab'
        }
        $h95Dx = 120; $h95Dy = 90
        EvAltDown
        EvLeftDown
        try {
            for ($i = 1; $i -le 6; $i++) {
                EvMoveTo ([int]($h95StartX + $h95Dx * $i / 6)) ([int]($h95StartY + $h95Dy * $i / 6)) 60
            }
            Start-Sleep -Milliseconds 200
            $h95Held = EvRect $h95Float
        } finally {
            EvLeftUp
            EvAltUp
        }
        Start-Sleep -Milliseconds 700
        $h95After = EvRect $h95Float
        $h95AfterEntry = WinOf (EvWinList 'float-after-move') $h95Float
        $h95Details.observations.Add([ordered]@{ stage = 'float-move'; before = $h95Before;
            held = $h95Held; after = $h95After; entry = $h95AfterEntry })
        $h95MovePass = (EvNear $h95Held.L ($h95Before.L + $h95Dx) 8) -and
            (EvNear $h95Held.T ($h95Before.T + $h95Dy) 8) -and
            (EvNear $h95After.L ($h95Before.L + $h95Dx) 8) -and
            (EvNear $h95After.T ($h95Before.T + $h95Dy) 8) -and
            (EvNear $h95After.W $h95Before.W 4) -and (EvNear $h95After.H $h95Before.H 4) -and
            $null -ne $h95AfterEntry -and $h95AfterEntry.floating -and
            $h95AfterEntry.workspace -eq $h95FloatEntry.workspace
        EvGate 'h95_mod_drag_moves_a_floating_window' $h95MovePass @{
            before = $h95Before; held = $h95Held; after = $h95After
        }

        # --- h95_mod_drag_resizes_a_floating_window ---
        $h95ResizeBefore = EvRect $h95Float
        $h95PressX = [int]($h95ResizeBefore.L + $h95ResizeBefore.W * 2 / 3)
        $h95PressY = [int]($h95ResizeBefore.T + $h95ResizeBefore.H * 2 / 3)
        EvMoveTo $h95PressX $h95PressY 120
        if ([LaneEvidenceNative]::Root($h95PressX, $h95PressY).ToInt64() -ne [int64]$h95Float) {
            throw 'H95 pointer is not over the floating fixture before the resize grab'
        }
        $h95Rw = 80; $h95Rh = 60
        EvAltDown
        EvRightDown
        try {
            for ($i = 1; $i -le 6; $i++) {
                EvMoveTo ([int]($h95PressX + $h95Rw * $i / 6)) ([int]($h95PressY + $h95Rh * $i / 6)) 60
            }
            Start-Sleep -Milliseconds 200
            $h95ResizeHeld = EvRect $h95Float
        } finally {
            EvRightUp
            EvAltUp
        }
        Start-Sleep -Milliseconds 700
        $h95ResizeAfter = EvRect $h95Float
        $h95Details.observations.Add([ordered]@{ stage = 'float-resize'; before = $h95ResizeBefore;
            held = $h95ResizeHeld; after = $h95ResizeAfter })
        $h95ResizePass = (EvNear $h95ResizeAfter.W ($h95ResizeBefore.W + $h95Rw) 10) -and
            (EvNear $h95ResizeAfter.H ($h95ResizeBefore.H + $h95Rh) 10) -and
            (EvNear $h95ResizeAfter.L $h95ResizeBefore.L 4) -and
            (EvNear $h95ResizeAfter.T $h95ResizeBefore.T 4)
        EvGate 'h95_mod_drag_resizes_a_floating_window' $h95ResizePass @{
            before = $h95ResizeBefore; held = $h95ResizeHeld; after = $h95ResizeAfter
        }

        # --- h95_mod_drag_escape_cancels_to_the_pre_drag_rect ---
        $h95CancelBefore = EvRect $h95Float
        $h95CancelX = [int]($h95CancelBefore.L + $h95CancelBefore.W / 2)
        $h95CancelY = [int]($h95CancelBefore.T + $h95CancelBefore.H / 3)
        EvMoveTo $h95CancelX $h95CancelY 120
        EvAltDown
        EvLeftDown
        try {
            for ($i = 1; $i -le 4; $i++) {
                EvMoveTo ([int]($h95CancelX + 25 * $i)) ([int]($h95CancelY + 20 * $i)) 60
            }
            Start-Sleep -Milliseconds 200
            $h95CancelHeld = EvRect $h95Float
            [LaneEvidenceNative]::Key(0x1B, $false)
            Start-Sleep -Milliseconds 80
            [LaneEvidenceNative]::Key(0x1B, $true)
            Start-Sleep -Milliseconds 400
        } finally {
            EvLeftUp
            EvAltUp
        }
        Start-Sleep -Milliseconds 600
        $h95CancelAfter = EvRect $h95Float
        $h95CancelEntry = WinOf (EvWinList 'float-after-cancel') $h95Float
        $h95CancelMoved = -not (EvNear $h95CancelHeld.L $h95CancelBefore.L 4)
        $h95CancelPass = $h95CancelMoved -and
            (EvNear $h95CancelAfter.L $h95CancelBefore.L 5) -and
            (EvNear $h95CancelAfter.T $h95CancelBefore.T 5) -and
            (EvNear $h95CancelAfter.W $h95CancelBefore.W 4) -and
            (EvNear $h95CancelAfter.H $h95CancelBefore.H 4) -and
            (-not $h95Process.HasExited) -and $null -ne $h95CancelEntry -and $h95CancelEntry.floating
        $h95Details.observations.Add([ordered]@{ stage = 'float-cancel'; before = $h95CancelBefore;
            held = $h95CancelHeld; after = $h95CancelAfter })
        EvGate 'h95_mod_drag_escape_cancels_to_the_pre_drag_rect' $h95CancelPass @{
            before = $h95CancelBefore; held = $h95CancelHeld; after = $h95CancelAfter
        }
        Shot 'h95-floating-grab'
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-float-error.txt')
        $h95Details['error-float'] = [string]$_
    }

    # ---- Group 5: the shortcut inhibitor blocks exactly the inhibitable binds --------------
    # niri's keyboard-shortcuts inhibitor swallows the enables that are inhibitable, while the
    # toggle itself and every `allow-inhibiting=false` enable keep working (niri
    # src/input/mod.rs KeyboardShortcutsInhibitState; wiri input/keyboard.rs KeyboardEngine).
    # Every observation is a `wiri-ctl window-list` field, so a "the bind fired" result is the
    # engine's own state and never a timing guess.
    try {
        $h95Inhibit = @'
input { mod-key "Alt"; }
animations { off; }
layout {
    gaps 16
    border { off; }
    focus-ring { off; }
    shadow { off; }
}
binds {
    Ctrl+Alt+F8 { toggle-window-floating; }
    Ctrl+Alt+F7 { toggle-keyboard-shortcuts-inhibit; }
    Ctrl+Alt+F6 allow-inhibiting=false { toggle-window-floating; }
}
'@
        EvReload $h95Inhibit 'inhibit'
        EvCloseForms
        $h95InhibitForm = [TestForms]::Open('wiri-h95-inhibit', 60, 100, 140); $h95Forms.Add($h95InhibitForm) | Out-Null
        Start-Sleep -Milliseconds 1600
        $null = EvOwnForeground $h95InhibitForm
        $h95InhibitState = {
            $entry = WinOf (EvWinList 'inhibit-state') $h95InhibitForm
            if ($null -eq $entry) { throw 'H95 inhibit fixture left the window list' }
            return [bool]$entry.floating
        }
        $h95Steps = New-Object 'System.Collections.Generic.List[object]'
        $h95Steps.Add([ordered]@{ stage = 'opened'; floating = (& $h95InhibitState) })
        EvChord 0x77; Start-Sleep -Milliseconds 700   # Ctrl+Alt+F8: inhibitable, so it fires now.
        $h95Steps.Add([ordered]@{ stage = 'bind-before-inhibit'; floating = (& $h95InhibitState) })
        EvChord 0x77; Start-Sleep -Milliseconds 700   # ... and again, unchanged: the control.
        $h95Steps.Add([ordered]@{ stage = 'bind-before-inhibit-2'; floating = (& $h95InhibitState) })
        EvChord 0x76; Start-Sleep -Milliseconds 700   # Ctrl+Alt+F7: enable the inhibitor.
        EvChord 0x77; Start-Sleep -Milliseconds 700   # A blocked enable must change nothing.
        $h95Steps.Add([ordered]@{ stage = 'inhibited-bind'; floating = (& $h95InhibitState) })
        EvChord 0x75; Start-Sleep -Milliseconds 700   # Ctrl+Alt+F6: allow-inhibiting=false fires.
        $h95Steps.Add([ordered]@{ stage = 'allow-inhibiting-false'; floating = (& $h95InhibitState) })
        EvChord 0x76; Start-Sleep -Milliseconds 700   # Release the inhibitor for the rest of the run.
        EvChord 0x77; Start-Sleep -Milliseconds 700   # The inhibitable bind works again.
        $h95Steps.Add([ordered]@{ stage = 'bind-after-release'; floating = (& $h95InhibitState) })
        $h95InhibitPass = (-not $h95Steps[0].floating) -and $h95Steps[1].floating -and
            (-not $h95Steps[2].floating) -and (-not $h95Steps[3].floating) -and
            $h95Steps[4].floating -and (-not $h95Steps[5].floating)
        $h95Details.observations.Add([ordered]@{ stage = 'inhibit'; steps = $h95Steps.ToArray() })
        EvGate 'h95_shortcuts_inhibit_blocks_inhibitable_binds_only' $h95InhibitPass @{
            steps = @($h95Steps | ForEach-Object { $_.stage + '=' + $_.floating })
        }
        Shot 'h95-inhibit'
    } catch {
        $_ | Out-String | Set-Content (Join-Path $h95Dir 'group-inhibit-error.txt')
        $h95Details['error-inhibit'] = [string]$_
    }
} catch {
    $_ | Out-String | Set-Content (Join-Path $h95Dir 'lane-error.txt')
    $h95Details['error-lane'] = [string]$_
} finally {
    # Never strand a synthetic held button or Alt, even when a group threw mid-drag.
    EvReleaseAll
    if ($null -ne $h95Process -and -not $h95Process.HasExited) { Stop-WiriGracefully $h95Process 'h95-final' }
    foreach ($h95Form in $h95Forms) {
        if ([LT]::IsWindow($h95Form)) { [TestForms]::CloseForm($h95Form) }
    }
    $h95Details | ConvertTo-Json -Depth 12 | Set-Content -Encoding UTF8 (Join-Path $h95Dir 'evidence.json')
}
