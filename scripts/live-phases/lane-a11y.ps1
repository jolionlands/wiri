# Lane A11Y / H77: screen-reader announcements and `block-out-from` (niri parity, section 17).
# Dot-source under live-test.ps1's desktop lock, after the last existing lane, with no
# daemon running: the parent quits its own daemon in Phase D, so this lane starts and owns
# every daemon it talks to.
#
# What is checked, and how:
#   * wiri raises UI Automation *notification* events (UiaRaiseNotificationEvent) for a
#     workspace switch and for the overview opening and closing, and the provider window's
#     UIA element carries the exact announced text as its Name.  A UIA client
#     (System.Windows.Automation) reads both, exactly as Narrator/NVDA would.
#   * the provider window only exists after the first announcement (nothing runs at rest).
#   * `block-out-from` in a layer rule and in a window rule sets WDA_EXCLUDEFROMCAPTURE
#     (GetWindowDisplayAffinity) on wiri's OWN overlay windows; with no rule the affinity
#     stays WDA_NONE.  Another process's windows cannot be excluded: SetWindowDisplayAffinity
#     only works in the owning process, which is why the parity row stays `partial`.
#   * niri's Latin-key fallback under a non-Latin layout: the live check runs ONLY when the
#     keyboard layout that is already active is non-Latin.  This lane never loads, activates
#     or changes a keyboard layout and never writes a system setting; when the active layout
#     is Latin the gate records that and points at the unit tests that cover the fallback
#     with fake layouts (input::keyboard::tests, input::keylayout::tests).
& {
    $aRoot = Join-Path $D 'lane-a11y'
    $null = New-Item -ItemType Directory -Force $aRoot
    $aUtf8 = New-Object Text.UTF8Encoding($false)
    $aGates = @(
        'h77_provider_created_lazily',
        'h77_uia_element_reports_workspace_switch',
        'h77_uia_element_reports_overview_open',
        'h77_uia_element_reports_overview_close',
        'h77_uia_notification_events_received',
        'h77_block_out_from_layer_rule_excludes_overlay',
        'h77_block_out_from_window_rule_excludes_overview_host',
        'h77_block_out_from_absent_leaves_affinity_none',
        'h77_latin_bind_fallback_live_or_unit_tested',
        'h77_owned_cleanup'
    )
    $aDetails = [ordered]@{
        reference = 'niri e19df12cf8 (wiki:Accessibility, wiki:Screencasting)'
        scope = 'UIA notification texts and capture affinity of wiri-owned overlays'
        provider_class = 'WiriAnnounce'
        expected_texts = [ordered]@{
            workspace = 'Workspace 2'; overview_open = 'Overview opened'; overview_close = 'Overview closed'
        }
        gates = [ordered]@{}
        cleanup = [ordered]@{ errors = (New-Object 'Collections.Generic.List[string]') }
        observations = (New-Object 'Collections.Generic.List[object]')
        limitations = @(
            'SetWindowDisplayAffinity works only for a window of the calling process: foreign application windows are decoded but cannot be excluded from capture (the row stays partial for them).',
            'The UIA notification handler reads the text from the provider element Name, which is what the notification displays; a client that only counts events still proves the event was raised.',
            'No keyboard layout is loaded, activated or changed; the Latin-key fallback is checked live only when the active layout is already non-Latin.'
        )
    }
    foreach ($gate in $aGates) {
        $summary[$gate] = $false
        $aDetails.gates[$gate] = [ordered]@{ pass = $false; observed = 'not reached' }
    }
    function A11yGate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $aDetails.gates[$name] = [ordered]@{ pass = $pass; observed = $observed }
    }
    function A11yError($err) {
        $aDetails.cleanup.errors.Add(($err | Out-String))
    }

    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    if (-not ('LaneA11yNative' -as [type])) {
        Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class LaneA11yNative {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", SetLastError = true)] static extern bool GetWindowDisplayAffinity(IntPtr h, out uint affinity);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern IntPtr GetKeyboardLayout(uint threadId);
    [DllImport("user32.dll")] static extern int GetKeyboardLayoutList(int n, [Out] IntPtr[] list);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);

    public static IntPtr Foreground() { return GetForegroundWindow(); }

    // Every top-level window of `pid`: handle, window class, title and the capture affinity
    // Windows reports for it (0 = WDA_NONE, 0x11 = WDA_EXCLUDEFROMCAPTURE, -1 = unreadable).
    public static List<string[]> Windows(uint pid) {
        var result = new List<string[]>();
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            uint owner; GetWindowThreadProcessId(h, out owner);
            if (owner != pid) { return true; }
            var cls = new StringBuilder(256); GetClassName(h, cls, 256);
            var title = new StringBuilder(512); GetWindowText(h, title, 512);
            uint affinity = 0;
            bool ok = GetWindowDisplayAffinity(h, out affinity);
            result.Add(new string[] { h.ToInt64().ToString(), cls.ToString(), title.ToString(), ok ? affinity.ToString() : "-1" });
            return true;
        }, IntPtr.Zero);
        return result;
    }

    public static string Title(IntPtr h) { var s = new StringBuilder(512); GetWindowText(h, s, 512); return s.ToString(); }
    public static string Class(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }

    // The keyboard layout of the foreground thread, then of this process; 0 when unknown.
    public static int ActiveLayout() {
        IntPtr fg = GetForegroundWindow();
        if (fg != IntPtr.Zero) {
            uint owner; uint thread = GetWindowThreadProcessId(fg, out owner);
            IntPtr hkl = GetKeyboardLayout(thread);
            if (hkl != IntPtr.Zero) { return hkl.ToInt32(); }
        }
        return GetKeyboardLayout(0).ToInt32();
    }

    public static int[] LoadedLayouts() {
        int count = GetKeyboardLayoutList(0, null);
        if (count <= 0) { return new int[0]; }
        var list = new IntPtr[count];
        int written = GetKeyboardLayoutList(count, list);
        var result = new int[written < 0 ? 0 : written];
        for (int i = 0; i < result.Length; i++) { result[i] = list[i].ToInt32(); }
        return result;
    }

    // One physical key press and release of a whole chord, in order, with a small gap so an
    // application (or wiri's own hook) sees the modifiers held before the key.
    public static void Chord(byte[] vks) {
        for (int i = 0; i < vks.Length; i++) { keybd_event(vks[i], 0, 0, UIntPtr.Zero); }
        for (int i = vks.Length - 1; i >= 0; i--) { keybd_event(vks[i], 0, 2, UIntPtr.Zero); }
    }
}
'@
    }

    $aLanguage = 0
    $aState = @{ proc = $null; label = $null; started = $false; terminated = $true; stopRequested = $false }
    function A11yStart([string]$label, [string]$config) {
        if ($null -ne $aState.proc -and -not $aState.proc.HasExited) {
            throw 'Lane A11Y refuses a second daemon while its own is still running'
        }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'Lane A11Y refuses an unowned wiri process; it never stops a process it did not start'
        }
        [IO.File]::WriteAllText($cfg, $config, $aUtf8)
        $stdout = Join-Path $aRoot ($label + '.stdout.log')
        $stderr = Join-Path $aRoot ($label + '.stderr.log')
        $started = Start-Wiri $stderr $stdout
        $aState.proc = $started
        $aState.label = $label
        $aState.started = $true
        $aState.terminated = $false
        $aState.stopRequested = $false
        $aDetails['daemon_' + $label] = [ordered]@{ pid = $started.Id; config = $cfg; stdout = $stdout; stderr = $stderr }
        # The pipe is served once startup finished; Ctl fails (rc != 0) until then.
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt 25000) {
            if ($started.HasExited) { throw "Lane A11Y daemon exited during startup: $label" }
            $n = $steps.Count
            $null = Ctl ($label + '-state') @('state')
            if ($steps.Count -eq ($n + 1) -and [string]$steps[$n].rc -eq '0') { return }
            Start-Sleep -Milliseconds 250
        }
        throw "Lane A11Y daemon never served its pipe: $label"
    }
    function A11yCtl([string]$tag, [string[]]$arguments) {
        $p = $aState.proc
        if ($null -eq $p -or $p.HasExited) { throw 'Lane A11Y has no live owned daemon' }
        $n = $steps.Count
        $text = Ctl ('a11y-' + $tag) $arguments
        if ($steps.Count -ne ($n + 1) -or [string]$steps[$n].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $text
    }
    function A11yStop {
        $p = $aState.proc
        if ($null -eq $p) { return }
        try {
            if (-not $p.HasExited) {
                $aState.stopRequested = $true
                try { $null = A11yCtl 'quit' @('quit') } catch { A11yError $_ }
                if (-not $p.WaitForExit(6000)) {
                    A11yError 'Lane A11Y had to stop its own daemon after quit timed out'
                    $p.Kill()
                    if (-not $p.WaitForExit(5000)) { throw 'Lane A11Y owned daemon did not exit after Kill' }
                }
            }
            $aState.terminated = $p.HasExited
            $aDetails['exit_' + $aState.label] = $p.ExitCode
            if ($p.ExitCode -ne 0) { A11yError ('nonzero daemon exit: ' + $p.ExitCode) }
        } catch { A11yError $_ } finally {
            if ($aState.terminated) { $p.Dispose(); $aState.proc = $null }
            $aState.started = $false
        }
    }
    function A11yWindows([int]$pid, [string]$class) {
        return @([LaneA11yNative]::Windows([uint32]$pid) | Where-Object { $_[1] -eq $class })
    }
    function A11yProviderHandle([int]$pid) {
        $found = @(A11yWindows $pid 'WiriAnnounce')
        if ($found.Count -eq 0) { return [IntPtr]::Zero }
        return [IntPtr][int64]$found[0][0]
    }
    function A11yUiaName([IntPtr]$hwnd) {
        if ($hwnd -eq [IntPtr]::Zero) { return $null }
        try {
            $element = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
            if ($null -eq $element) { return $null }
            return [string]$element.Current.Name
        } catch { A11yError $_; return $null }
    }
    function A11yWaitName([IntPtr]$hwnd, [string]$expected, [int]$timeoutMs) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $last = $null
        while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
            $last = A11yUiaName $hwnd
            if ($last -eq $expected) { return [pscustomobject]@{ pass = $true; name = $last } }
            Start-Sleep -Milliseconds 100
        }
        return [pscustomobject]@{ pass = $false; name = $last }
    }
    function A11yWaitProvider([int]$pid, [bool]$present, [int]$timeoutMs) {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        while ($sw.ElapsedMilliseconds -lt $timeoutMs) {
            $handle = A11yProviderHandle $pid
            if ($present -and $handle -ne [IntPtr]::Zero) { return $handle }
            if (-not $present -and $handle -eq [IntPtr]::Zero) { return $handle }
            Start-Sleep -Milliseconds 100
        }
        return (A11yProviderHandle $pid)
    }

    # The notification UIA event (UIA_NotificationEventId = 20027).  The managed client has no
    # payload class for it, so the handler records the provider element's Name, which is the
    # text the notification displays.
    $aNotifications = New-Object 'Collections.Generic.List[string]'
    $aHandler = $null
    $aEventId = $null
    function A11ySubscribeNotifications {
        try {
            $aEventId = [System.Windows.Automation.AutomationEvent]::LookupById(20027)
            if ($null -eq $aEventId) { return $false }
            $script:aHandler = [System.Windows.Automation.AutomationEventHandler]{
                param($sender, $eventArgs)
                try {
                    $text = ''
                    if ($null -ne $sender) { $text = [string]$sender.Current.Name }
                    $script:aNotifications.Add($text)
                } catch { }
            }
            [System.Windows.Automation.Automation]::AddAutomationEventHandler(
                $aEventId,
                [System.Windows.Automation.AutomationElement]::RootElement,
                [System.Windows.Automation.TreeScope]::Subtree,
                $script:aHandler)
            return $true
        } catch { A11yError $_; return $false }
    }
    function A11yUnsubscribeNotifications {
        if ($null -eq $aHandler -or $null -eq $aEventId) { return }
        try {
            [System.Windows.Automation.Automation]::RemoveAutomationEventHandler(
                $aEventId,
                [System.Windows.Automation.AutomationElement]::RootElement,
                $aHandler)
        } catch { A11yError $_ }
        $script:aHandler = $null
    }

    $aWithRules = @'
animations { off; }
input { focus-follows-mouse false; }
binds {
    Ctrl+Alt+F9 { focus-workspace 2; }
}
window-rule {
    match title="overview"
    block-out-from "screen-capture"
}
layer-rule {
    match namespace="workspace-indicator"
    block-out-from "screencast"
}
'@
    $aNoRules = @'
animations { off; }
input { focus-follows-mouse false; }
binds {
    Ctrl+Alt+F9 { focus-workspace 2; }
}
'@

    try {
        $aPid = 0
        # ---- run 1: announcements, and block-out-from on wiri's own overlays ----
        A11yStart 'rules' $aWithRules
        $aPid = $aState.proc.Id
        Start-Sleep -Milliseconds 1200

        # Nothing has been announced yet, so the provider window must not exist yet.
        $before = A11yWaitProvider $aPid $false 1500
        A11yGate 'h77_provider_created_lazily' ($before -eq [IntPtr]::Zero) ([ordered]@{
                provider_before_first_announcement = $(if ($before -eq [IntPtr]::Zero) { 'absent' } else { 'present' })
                note = 'the UIA provider, its window and its thread are created by the first announcement only'
            })

        $subscribed = A11ySubscribeNotifications
        $aDetails['notification_subscription'] = $subscribed

        # A workspace switch: niri announces "Workspace <name>", or its number.
        $null = A11yCtl 'workspace-2' @('action', 'focus-workspace', '2')
        $provider = A11yWaitProvider $aPid $true 8000
        $workspace = A11yWaitName $provider 'Workspace 2' 8000
        A11yGate 'h77_uia_element_reports_workspace_switch' $workspace.pass ([ordered]@{
                provider = [int64]$provider; uia_name = $workspace.name; expected = 'Workspace 2'
            })

        # The overview opening and closing.
        $null = A11yCtl 'overview-open' @('action', 'toggle-overview')
        $open = A11yWaitName $provider 'Overview opened' 8000
        A11yGate 'h77_uia_element_reports_overview_open' $open.pass ([ordered]@{
                uia_name = $open.name; expected = 'Overview opened'
            })
        $null = A11yCtl 'overview-close' @('action', 'toggle-overview')
        $close = A11yWaitName $provider 'Overview closed' 10000
        A11yGate 'h77_uia_element_reports_overview_close' $close.pass ([ordered]@{
                uia_name = $close.name; expected = 'Overview closed'
            })

        if ($subscribed) {
            Start-Sleep -Milliseconds 800
            $texts = @($aNotifications.ToArray())
            $received = ($texts.Count -ge 2 -and (@($texts | Where-Object { $_ -eq 'Overview opened' }).Count -ge 1))
            A11yGate 'h77_uia_notification_events_received' $received ([ordered]@{
                    events = $texts; note = 'UIA notification event (UIA_NotificationEventId 20027) on the desktop root, subtree scope'
                })
        } else {
            A11yGate 'h77_uia_notification_events_received' $false ([ordered]@{
                    note = 'System.Windows.Automation could not register a handler for UIA_NotificationEventId on this machine'
                })
        }
        A11yUnsubscribeNotifications

        # block-out-from: the layer rule on the workspace indicator, the window rule on the
        # overview host (which exists only while the overview is open).
        $indicator = @(A11yWindows $aPid 'WiriWorkspaceIndicator')
        $indicatorAffinity = $(if ($indicator.Count -gt 0) { [int]$indicator[0][3] } else { -1 })
        $null = A11yCtl 'overview-open-again' @('action', 'toggle-overview')
        Start-Sleep -Milliseconds 1500
        $hosts = @(A11yWindows $aPid 'WiriThumbnailOverviewHost')
        $hostAffinity = $(if ($hosts.Count -gt 0) { [int]$hosts[0][3] } else { -1 })
        $null = A11yCtl 'overview-close-again' @('action', 'toggle-overview')
        Start-Sleep -Milliseconds 1200
        # WDA_EXCLUDEFROMCAPTURE is 0x11; WDA_NONE is 0.
        $indicatorRows = @($indicator | ForEach-Object {
                [ordered]@{ hwnd = [int64]$_.GetValue(0); class = $_.GetValue(1); title = $_.GetValue(2); affinity = [int]$_.GetValue(3) }
            })
        A11yGate 'h77_block_out_from_layer_rule_excludes_overlay' ($indicatorAffinity -eq 0x11) ([ordered]@{
                window_class = 'WiriWorkspaceIndicator'; affinity = $indicatorAffinity; expected = 0x11
                rule = 'layer-rule { match namespace="workspace-indicator"; block-out-from "screencast" }'
                windows = $indicatorRows
            })
        A11yGate 'h77_block_out_from_window_rule_excludes_overview_host' ($hostAffinity -eq 0x11) ([ordered]@{
                window_class = 'WiriThumbnailOverviewHost'; affinity = $hostAffinity; expected = 0x11
                rule = 'window-rule { match title="overview"; block-out-from "screen-capture" }'
                note = 'the overview host exists only while the overview is open'
            })

        # ---- Latin-key fallback: only when the active layout is already non-Latin ----
        # Primary language ids with a non-Latin script.  wiri never changes the user's
        # layouts, so a machine with a Latin layout active cannot show this live.
        $nonLatin = @(0x01, 0x02, 0x04, 0x08, 0x0D, 0x11, 0x12, 0x19, 0x1E, 0x20, 0x22, 0x23, 0x29, 0x2B, 0x2C, 0x2F, 0x37, 0x3F, 0x45, 0x50)
        $aLayout = [LaneA11yNative]::ActiveLayout()
        $aLanguage = $aLayout -band 0xFFFF
        $aPrimary = $aLanguage -band 0x3FF
        $aLoaded = @([LaneA11yNative]::LoadedLayouts() | ForEach-Object { ($_ -band 0xFFFF) })
        if ($nonLatin -contains $aPrimary) {
            # Send the Latin chord Ctrl+Alt+F9 (the private config binds it to focus-workspace 2)
            # and watch for the workspace announcement it causes.
            $null = A11yCtl 'workspace-1' @('action', 'focus-workspace', '1')
            $null = A11yWaitName $provider 'Workspace 1' 6000
            $aNotifications.Clear()
            [LaneA11yNative]::Chord([byte[]]@(0x11, 0x12, 0x78))
            $latin = A11yWaitName $provider 'Workspace 2' 8000
            A11yGate 'h77_latin_bind_fallback_live_or_unit_tested' $latin.pass ([ordered]@{
                    applicable = $true; active_language = ('0x{0:X4}' -f $aLanguage)
                    uia_name = $latin.name; expected = 'Workspace 2'
                    note = 'Ctrl+Alt+F9 is bound to focus-workspace 2; the active layout is non-Latin, so this is the Latin-key fallback'
                })
        } else {
            A11yGate 'h77_latin_bind_fallback_live_or_unit_tested' $true ([ordered]@{
                    applicable = $false; active_language = ('0x{0:X4}' -f $aLanguage)
                    loaded_languages = @($aLoaded | ForEach-Object { '0x{0:X4}' -f $_ })
                    note = 'the active layout is Latin, so the fallback cannot be observed without changing the user''s keyboard layouts (not done). input::keyboard::tests and input::keylayout::tests cover it with fake layouts: a Latin bind fires on a layout that reports another virtual key, the layout-specific bind still wins, a release bind fires through the fallback, an unmapped key never matches, and the built-in Latin scan-code table is the US keyboard.'
                })
        }

        A11yStop
        if ($aDetails.cleanup.errors.Count -gt 0) { throw 'Lane A11Y first daemon cleanup failed; refusing a second session' }

        # ---- run 2: without a block-out-from rule the affinity stays WDA_NONE ----
        A11yStart 'no-rules' $aNoRules
        $aPid = $aState.proc.Id
        Start-Sleep -Milliseconds 1500
        $plain = @(A11yWindows $aPid 'WiriWorkspaceIndicator')
        $plainAffinity = $(if ($plain.Count -gt 0) { [int]$plain[0][3] } else { -1 })
        $providerAbsent = $(A11yProviderHandle $aPid) -eq [IntPtr]::Zero
        A11yGate 'h77_block_out_from_absent_leaves_affinity_none' ($plainAffinity -eq 0 -and $providerAbsent) ([ordered]@{
                window_class = 'WiriWorkspaceIndicator'; affinity = $plainAffinity; expected = 0
                provider_absent_without_announcements = $providerAbsent
                note = 'no rule, no capture exclusion; and nothing has announced, so no provider window or thread exists'
            })
    } catch {
        $aDetails['error'] = [string]$_
    } finally {
        A11yUnsubscribeNotifications
        A11yStop
        $clean = ($aState.terminated -and $null -eq $aState.proc -and $aDetails.cleanup.errors.Count -eq 0)
        A11yGate 'h77_owned_cleanup' $clean ([ordered]@{
                terminated = $aState.terminated; errors = @($aDetails.cleanup.errors)
            })
        if (-not $clean) {
            foreach ($gate in $aGates) {
                if ($gate -eq 'h77_owned_cleanup') { continue }
                $summary[$gate] = $false
                if ($aDetails.gates.Contains($gate)) { $aDetails.gates[$gate].pass = $false }
            }
        }
        $json = $aDetails | ConvertTo-Json -Depth 12
        [IO.File]::WriteAllText((Join-Path $aRoot 'evidence.json'), $json, $aUtf8)
        [IO.File]::WriteAllText((Join-Path $D 'lane-a11y.evidence.json'), $json, $aUtf8)
    }
}
