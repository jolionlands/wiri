# H84: native application bounds, using owned HWNDs with mutable MINMAXINFO.
# Dot-source only from live-test.ps1 while its controller holds the desktop mutex.
# No product test API: requests use the existing CLI/IPC; the fixture's native
# answer is independent of the allocation checked in the niri-shaped window reply.
& {
    $h84Dir = Join-Path $D 'h84-native-bounds'
    $null = New-Item -ItemType Directory -Force $h84Dir
    $h84Gates = @('h84_native_max_width_allocation', 'h84_native_min_height_allocation',
        'h84_native_max_height_allocation', 'h84_native_neighbors_do_not_overlap',
        'h84_fixed_height_defaults_floating', 'h84_explicit_tiled_overrides_fixed_height',
        'h84_fixed_width_stays_tiled', 'h84_zero_bounds_stay_tiled',
        'h84_native_bounds_refresh', 'h84_successful_zero_clears_bounds', 'h84_cleanup')
    foreach ($gate in $h84Gates) { $summary[$gate] = $false }
    $h84Evidence = [ordered]@{ scope = 'single-output physical native bounds; mixed-DPI/frame invalidation and timeout/stale-generation cases require separate evidence'; checks = [ordered]@{} }
    $h84Forms = New-Object System.Collections.ArrayList
    $h84Process = $null
    $h84Runtime = @{ serial = 0 }
    # ISMEX_SEND distinguishes cross-thread sent queries from in-thread sizing
    # messages; it cannot identify the sending process or prove a daemon query.
    # IPC allocation comparisons provide the behavior evidence. Override only
    # cross-thread sent queries: in-thread creation/sizing keeps OS defaults so
    # zero max-track answers cannot collapse the actual fixture window. A direct
    # bounded probe independently records the exact answer, including four zeros.
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;
public static class H84NativeBounds {
    [StructLayout(LayoutKind.Sequential)] public struct NativePoint { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct MinMax {
        public NativePoint Reserved, MaxSize, MaxPosition, MinTrack, MaxTrack;
    }
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int L,T,R,B; }
    public sealed class Observation {
        public int[] VisibleBounds, LastOuterAnswer, DefaultTrackBounds, Margins;
        public int Queries, CrossThreadSentQueries, InThreadQueries;
        public uint Dpi;
    }
    public sealed class QueryResult {
        public bool Success;
        public int Error;
        public int[] OuterAnswer;
    }
    public sealed class NativeDiagnostics {
        public bool Exists, Visible, Minimized, HasOuterRect, HasVisibleRect;
        public uint ProcessId, ThreadId, Dpi;
        public long Style, ExStyle;
        public int[] OuterRect, VisibleRect;
        public Observation Observation;
        public QueryResult DirectQuery;
    }
    sealed class ProbeForm : Form {
        public readonly object Sync = new object();
        public int[] BoundsAnswer = new int[4];
        public int[] LastOuter = new int[4], DefaultTrack = new int[4], LastMargins = new int[4];
        public int Queries, CrossThreadSentQueries, InThreadQueries;
        protected override void WndProc(ref Message m) {
            base.WndProc(ref m);
            if (m.Msg != 0x24 || m.LParam == IntPtr.Zero) return;
            var info = (MinMax)Marshal.PtrToStructure(m.LParam, typeof(MinMax));
            bool crossThreadSent = (InSendMessageEx(IntPtr.Zero) & 1) != 0;
            lock (Sync) {
                Queries++;
                if (!crossThreadSent) { InThreadQueries++; return; }
                CrossThreadSentQueries++;
                var margins = MeasureMargins(Handle);
                DefaultTrack = new int[] { info.MinTrack.X, info.MinTrack.Y, info.MaxTrack.X, info.MaxTrack.Y };
                LastMargins = margins;
                int horizontal = margins[0] + margins[2], vertical = margins[1] + margins[3];
                info.MinTrack.X = BoundsAnswer[0] == 0 ? 0 : BoundsAnswer[0] + horizontal;
                info.MinTrack.Y = BoundsAnswer[1] == 0 ? 0 : BoundsAnswer[1] + vertical;
                info.MaxTrack.X = BoundsAnswer[2] == 0 ? 0 : BoundsAnswer[2] + horizontal;
                info.MaxTrack.Y = BoundsAnswer[3] == 0 ? 0 : BoundsAnswer[3] + vertical;
                LastOuter = new int[] { info.MinTrack.X, info.MinTrack.Y, info.MaxTrack.X, info.MaxTrack.Y };
            }
            Marshal.StructureToPtr(info, m.LParam, false);
        }
    }
    sealed class Opening {
        public readonly object Sync = new object();
        public ProbeForm Form;
        public IntPtr Handle;
        public bool Cancelled, Ready, Finished;
        public Exception Error;
    }
    static readonly Dictionary<IntPtr, ProbeForm> Forms = new Dictionary<IntPtr, ProbeForm>();
    static readonly List<Opening> Openings = new List<Opening>();
    static readonly List<IntPtr> QueryBuffers = new List<IntPtr>();
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out Rect r);
    [DllImport("dwmapi.dll")] static extern int DwmGetWindowAttribute(IntPtr h, int a, out Rect r, int size);
    [DllImport("user32.dll")] static extern uint InSendMessageEx(IntPtr unused);
    [DllImport("user32.dll")] static extern uint GetDpiForWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] static extern IntPtr GetWindowLongPtr(IntPtr h, int index);
    [DllImport("user32.dll", SetLastError=true)] static extern IntPtr SendMessageTimeout(
        IntPtr h, uint message, IntPtr wparam, IntPtr lparam, uint flags, uint timeout, out IntPtr result);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
    static int[] MeasureMargins(IntPtr h) {
        Rect outer, visible;
        if (!GetWindowRect(h, out outer) || DwmGetWindowAttribute(h, 9, out visible, 16) != 0) return new int[4];
        int[] m = new int[] { visible.L - outer.L, visible.T - outer.T, outer.R - visible.R, outer.B - visible.B };
        foreach (int v in m) if (v < 0 || v > 32) return new int[4];
        return m;
    }
    static ProbeForm Find(IntPtr h) {
        lock (Forms) { ProbeForm f; if (!Forms.TryGetValue(h, out f)) throw new InvalidOperationException("Unknown H84 fixture"); return f; }
    }
    static void Cancel(Opening opening) {
        ProbeForm form;
        lock (opening.Sync) { opening.Cancelled = true; form = opening.Form; }
        // If no handle exists yet, Shown observes cancellation on its own UI
        // thread. A handle that already exists gets a queued close as well.
        if (form != null) {
            try {
                if (!form.IsDisposed && form.IsHandleCreated)
                    form.BeginInvoke((MethodInvoker)delegate { form.Close(); });
            } catch (InvalidOperationException) { } // UI thread already disposed it
        }
    }
    public static IntPtr Open(string title, int minW, int minH, int maxW, int maxH) {
        var opening = new Opening();
        // Track the attempt before starting the thread: cleanup owns even a
        // form that has not shown or whose Open call later times out.
        lock (Openings) Openings.Add(opening);
        var thread = new Thread(delegate() {
            ProbeForm form = null;
            try {
                SetThreadDpiAwarenessContext(new IntPtr(-4)); // fixture thread only, physical PMv2 coordinates
                form = new ProbeForm();
                lock (opening.Sync) {
                    opening.Form = form;
                    if (opening.Cancelled) return;
                }
                form.BoundsAnswer = new int[] { minW, minH, maxW, maxH };
                form.Text = title; form.Size = new Size(520, 380);
                form.StartPosition = FormStartPosition.Manual;
                form.Location = new Point(120, 120);
                form.BackColor = Color.FromArgb(55, 110, 130);
                form.Shown += delegate {
                    bool cancelled;
                    lock (opening.Sync) {
                        cancelled = opening.Cancelled;
                        if (!cancelled) {
                            opening.Handle = form.Handle;
                            lock (Forms) Forms.Add(opening.Handle, form);
                            opening.Ready = true;
                            Monitor.PulseAll(opening.Sync);
                        }
                    }
                    if (cancelled) form.Close();
                };
                lock (opening.Sync) { if (opening.Cancelled) return; }
                Application.Run(form);
            } catch (Exception error) {
                lock (opening.Sync) opening.Error = error;
            } finally {
                if (form != null) form.Dispose();
                lock (opening.Sync) { opening.Finished = true; Monitor.PulseAll(opening.Sync); }
            }
        });
        thread.SetApartmentState(ApartmentState.STA); thread.IsBackground = true; thread.Start();
        var clock = System.Diagnostics.Stopwatch.StartNew();
        Exception failure;
        lock (opening.Sync) {
            while (!opening.Ready && !opening.Finished && clock.ElapsedMilliseconds < 15000)
                Monitor.Wait(opening.Sync, (int)Math.Max(1, 15000 - clock.ElapsedMilliseconds));
            if (opening.Ready) return opening.Handle;
            opening.Cancelled = true;
            failure = opening.Error;
        }
        Cancel(opening);
        if (failure != null) throw new InvalidOperationException("H84 form failed to open", failure);
        throw new TimeoutException("H84 form did not open; its UI thread owns cancellation and disposal");
    }
    public static void SetBounds(IntPtr h, int minW, int minH, int maxW, int maxH) {
        var form = Find(h);
        lock (form.Sync) form.BoundsAnswer = new int[] { minW, minH, maxW, maxH };
        // No message, frame change or synthetic invalidation: focus/layout must
        // eventually refresh an ordinary application's changed native answer.
    }
    public static Observation Read(IntPtr h) {
        var form = Find(h);
        lock (form.Sync) return new Observation {
            VisibleBounds = (int[])form.BoundsAnswer.Clone(), LastOuterAnswer = (int[])form.LastOuter.Clone(),
            DefaultTrackBounds = (int[])form.DefaultTrack.Clone(), Margins = (int[])form.LastMargins.Clone(),
            Queries = form.Queries, CrossThreadSentQueries = form.CrossThreadSentQueries,
            InThreadQueries = form.InThreadQueries, Dpi = GetDpiForWindow(h)
        };
    }
    public static QueryResult Query(IntPtr h) {
        // The caller is the PowerShell thread, distinct from this form's UI
        // thread. This is an independent fixture validation, not daemon proof.
        IntPtr memory = Marshal.AllocHGlobal(Marshal.SizeOf(typeof(MinMax)));
        // A timed-out same-process send may still be executing on the UI thread.
        // Keep its small buffer owned until every fixture UI thread has exited.
        lock (QueryBuffers) QueryBuffers.Add(memory);
        Marshal.StructureToPtr(new MinMax(), memory, false);
        IntPtr ignored;
        bool ok = SendMessageTimeout(h, 0x24, IntPtr.Zero, memory, 3, 1000, out ignored) != IntPtr.Zero;
        int error = ok ? 0 : Marshal.GetLastWin32Error();
        var answer = ok ? (MinMax)Marshal.PtrToStructure(memory, typeof(MinMax)) : new MinMax();
        return new QueryResult { Success = ok, Error = error,
            OuterAnswer = ok ? new int[] { answer.MinTrack.X, answer.MinTrack.Y, answer.MaxTrack.X, answer.MaxTrack.Y } : null };
    }
    public static NativeDiagnostics Diagnose(IntPtr h) {
        Rect outer, visible;
        uint pid;
        uint tid = GetWindowThreadProcessId(h, out pid);
        bool hasOuter = GetWindowRect(h, out outer);
        bool hasVisible = DwmGetWindowAttribute(h, 9, out visible, 16) == 0;
        return new NativeDiagnostics { Exists = IsWindow(h), Visible = IsWindowVisible(h), Minimized = IsIconic(h),
            ProcessId = pid, ThreadId = tid, Dpi = GetDpiForWindow(h),
            Style = GetWindowLongPtr(h, -16).ToInt64(), ExStyle = GetWindowLongPtr(h, -20).ToInt64(),
            HasOuterRect = hasOuter, HasVisibleRect = hasVisible,
            OuterRect = new int[] { outer.L, outer.T, outer.R, outer.B },
            VisibleRect = new int[] { visible.L, visible.T, visible.R, visible.B },
            DirectQuery = Query(h), Observation = Read(h) };
    }
    public static void Close(IntPtr h) {
        var form = Find(h);
        if (!form.IsDisposed) form.BeginInvoke((MethodInvoker)delegate { form.Close(); });
    }
    public static void CloseAll() {
        Opening[] owned;
        lock (Openings) owned = Openings.ToArray();
        foreach (var opening in owned) Cancel(opening);
    }
    public static bool AllClosed {
        get {
            Opening[] owned;
            lock (Openings) owned = Openings.ToArray();
            foreach (var opening in owned) {
                lock (opening.Sync) { if (!opening.Finished) return false; }
            }
            lock (QueryBuffers) {
                foreach (IntPtr memory in QueryBuffers) Marshal.FreeHGlobal(memory);
                QueryBuffers.Clear();
            }
            return true;
        }
    }
}
'@
    function H84Command([string]$name, [string[]]$arguments) {
        $h84Runtime.serial++
        $raw = Ctl ('h84-{0:D3}-{1}' -f $h84Runtime.serial, $name) $arguments
        if ($steps[-1].rc -ne 0) { throw "H84 $name failed: $raw" }
        return $raw
    }
    function H84Windows {
        $reply = (H84Command 'windows' @('windows', '--json')) | ConvertFrom-Json
        if (-not $reply.success -or $null -eq $reply.result.windows) { throw 'H84 windows IPC shape changed' }
        return @($reply.result.windows)
    }
    function H84Window([IntPtr]$h) {
        $matches = @(H84Windows | Where-Object { $_.id -eq $h.ToInt64() })
        if ($matches.Count -ne 1) { throw "H84 expected managed fixture $h, got $($matches.Count)" }
        return $matches[0]
    }
    function H84Wait([scriptblock]$condition, [int]$timeoutMs = 6000) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if (& $condition) { return $true }
            Start-Sleep -Milliseconds 150
        } while ($clock.ElapsedMilliseconds -lt $timeoutMs)
        return $false
    }
    function H84Open([string]$title, [int]$minW, [int]$minH, [int]$maxW, [int]$maxH) {
        $h = [H84NativeBounds]::Open($title, $minW, $minH, $maxW, $maxH)
        $null = $h84Forms.Add($h)
        $probe = [H84NativeBounds]::Query($h)
        $h84Evidence["opening_$title"] = [ordered]@{ hwnd = $h.ToInt64(); direct_query = $probe; native = [H84NativeBounds]::Read($h) }
        if (-not $probe.Success) { throw "H84 independent fixture query failed: $title" }
        # Zero is an exact answer to the same message the daemon uses. Preserve
        # that proof separately from the configured desired bounds in Read().
        $expected = @($minW, $minH, $maxW, $maxH)
        $margins = [H84NativeBounds]::Read($h).Margins
        for ($axis = 0; $axis -lt 4; $axis++) {
            $border = if (($axis % 2) -eq 0) { $margins[0] + $margins[2] } else { $margins[1] + $margins[3] }
            $expectedOuter = if ($expected[$axis] -eq 0) { 0 } else { $expected[$axis] + $border }
            if ($probe.OuterAnswer[$axis] -ne $expectedOuter) { throw "H84 independent fixture answer mismatch: $title axis $axis" }
        }
        if (-not (H84Wait { @(H84Windows | Where-Object { $_.id -eq $h.ToInt64() }).Count -eq 1 })) {
            $diagnostic = [ordered]@{ title = $title; native = [H84NativeBounds]::Diagnose($h); windows = @(H84Windows) }
            $h84Evidence.admission_timeout = $diagnostic
            $diagnostic | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $h84Dir 'admission-timeout.json')
            throw "H84 admission timed out: $title"
        }
        if ([LT]::Pid($h) -ne $PID) { throw 'H84 fixture is not owned' }
        return $h
    }
    function H84Focus([IntPtr]$h) {
        $null = H84Command 'focus' @('focus-window', [string]$h.ToInt64())
        if (-not (H84Wait { [LT]::GetForegroundWindow() -eq $h } 3000)) { throw 'H84 fixture focus did not settle' }
    }
    function H84Observe([IntPtr]$h) {
        return [ordered]@{ hwnd = $h.ToInt64(); native = [H84NativeBounds]::Read($h); visible = Rc $h; engine = H84Window $h }
    }
    function H84Gate([string]$name, [bool]$pass, $observation) {
        $summary[$name] = $pass
        $h84Evidence.checks[$name] = [ordered]@{ pass = $pass; observed = $observation }
    }
    function H84Size([IntPtr]$h, [int]$axis, [double]$physical) {
        $window = H84Window $h
        $native = [H84NativeBounds]::Read($h)
        $scale = [Math]::Max(0.01, $native.Dpi / 96.0)
        # Window-size and tile-size are independently constrained IPC allocation,
        # in logical pixels. Border is disabled, so both must match the app bounds.
        $rect = Rc $h
        $visible = if ($axis -eq 0) { $rect.W } else { $rect.H }
        return (-not $window.is_floating -and
            (Near ($window.layout.window_size[$axis] * $scale) $physical 3) -and
            (Near ($window.layout.tile_size[$axis] * $scale) $physical 3) -and (Near $visible $physical 3))
    }
    try {
        $h84Config = @'
input { mod-key "ctrl-alt"; }
animations { off; }
layout { gaps 10; center-focused-column "never"; strip-frame false; focus-ring { off; }; border { off; }; shadow { off; }; }
window-rule { match title="^h84-"; default-column-width { fixed 700; }; }
window-rule { match title="^h84-fixed-tile$"; open-floating false; }
'@
        $h84Evidence.config = $h84Config
        $h84Process = Start-WiriWithConfig $h84Config (Join-Path $h84Dir 'stderr.log')
        $bounded = H84Open 'h84-bounded' 160 200 420 280
        $neighbor = H84Open 'h84-neighbor' 0 0 0 0
        H84Focus $bounded
        $null = H84Command 'consume-neighbor' @('action', 'consume-window-into-column')
        Start-Sleep -Milliseconds 700
        $bw = H84Window $bounded; $nw = H84Window $neighbor
        if ($null -eq $bw.layout.pos_in_scrolling_layout -or $null -eq $nw.layout.pos_in_scrolling_layout -or
            $bw.layout.pos_in_scrolling_layout[0] -ne $nw.layout.pos_in_scrolling_layout[0]) { throw 'H84 two-tile column setup failed' }
        H84Focus $bounded
        $null = H84Command 'request-wide' @('action', 'set-column-width', '700')
        Start-Sleep -Milliseconds 450
        H84Gate 'h84_native_max_width_allocation' (H84Size $bounded 0 420) @((H84Observe $bounded), (H84Observe $neighbor))
        $boundedScale = [H84NativeBounds]::Read($bounded).Dpi / 96.0
        if ($boundedScale -le 0) { throw 'H84 bounded fixture has no DPI' }
        $shortLogicalHeight = [Math]::Max(1, [Math]::Floor(100 / $boundedScale))
        if (($shortLogicalHeight * $boundedScale) -ge 200) { throw 'H84 short request must remain below the 200px native minimum' }
        $h84Evidence.short_request = [ordered]@{ logical_height = $shortLogicalHeight; physical_height = $shortLogicalHeight * $boundedScale }
        $null = H84Command 'request-short' @('action', 'set-window-height', [string]$shortLogicalHeight)
        Start-Sleep -Milliseconds 450
        H84Gate 'h84_native_min_height_allocation' (H84Size $bounded 1 200) @((H84Observe $bounded), (H84Observe $neighbor))
        $null = H84Command 'request-tall' @('action', 'set-window-height', '600')
        Start-Sleep -Milliseconds 450
        H84Gate 'h84_native_max_height_allocation' (H84Size $bounded 1 280) @((H84Observe $bounded), (H84Observe $neighbor))
        $br = Rc $bounded; $nr = Rc $neighbor; $bw = H84Window $bounded; $nw = H84Window $neighbor
        $scale = [H84NativeBounds]::Read($bounded).Dpi / 96.0
        $engineGap = [Math]::Abs($bw.layout.tile_pos_in_workspace_view[1] - $nw.layout.tile_pos_in_workspace_view[1])
        $upperHeight = if ($bw.layout.tile_pos_in_workspace_view[1] -lt $nw.layout.tile_pos_in_workspace_view[1]) { $bw.layout.tile_size[1] } else { $nw.layout.tile_size[1] }
        H84Gate 'h84_native_neighbors_do_not_overlap' (($br.B -le $nr.T -or $nr.B -le $br.T) -and
            ($engineGap -ge ($upperHeight - 1)) -and (Near ($nw.layout.tile_size[0] * $scale) 420 3)) @((H84Observe $bounded), (H84Observe $neighbor))
        # A positive minimum previously answered successfully must not be cached
        # forever. Repeated ordinary focus/layout visits allow up to 12s freshness.
        $refresh = H84Open 'h84-refresh' 160 0 0 0
        H84Focus $refresh
        $before = [H84NativeBounds]::Read($refresh)
        $refreshScale = $before.Dpi / 96.0
        if ($refreshScale -le 0) { throw 'H84 refresh fixture has no DPI' }
        $smallLogicalWidth = [Math]::Max(1, [Math]::Floor(220 / $refreshScale))
        $smallPhysicalWidth = $smallLogicalWidth * $refreshScale
        if ($smallPhysicalWidth -ge 620) { throw 'H84 small request must remain below the 620px native minimum' }
        $h84Evidence.refresh_request = [ordered]@{ logical_width = $smallLogicalWidth; expected_physical_width = $smallPhysicalWidth; dpi = $before.Dpi }
        [H84NativeBounds]::SetBounds($refresh, 620, 0, 0, 0)
        $changed = H84Wait {
            H84Focus $neighbor; H84Focus $refresh
            $null = H84Command 'request-refresh-small' @('action', 'set-column-width', [string]$smallLogicalWidth)
            return (H84Size $refresh 0 620)
        } 12000
        $after = [H84NativeBounds]::Read($refresh)
        H84Gate 'h84_native_bounds_refresh' ($changed -and $after.CrossThreadSentQueries -gt $before.CrossThreadSentQueries) @($before, (H84Observe $refresh))
        [H84NativeBounds]::SetBounds($refresh, 0, 0, 0, 0)
        $cleared = H84Wait {
            H84Focus $neighbor; H84Focus $refresh
            $null = H84Command 'request-cleared-small' @('action', 'set-column-width', [string]$smallLogicalWidth)
            return (H84Size $refresh 0 $smallPhysicalWidth)
        } 12000
        $zero = [H84NativeBounds]::Read($refresh)
        # Require the positive refresh first: old baseline 160 permits this request,
        # and must not falsely prove removal of a 620px bound it never learned.
        H84Gate 'h84_successful_zero_clears_bounds' ($changed -and $cleared -and $zero.CrossThreadSentQueries -gt $after.CrossThreadSentQueries) @($after, (H84Observe $refresh))
        foreach ($case in @(
            @{ title='h84-fixed-height'; minW=160; minH=240; maxW=0; maxH=240; gate='h84_fixed_height_defaults_floating'; floating=$true },
            @{ title='h84-fixed-tile'; minW=160; minH=240; maxW=0; maxH=240; gate='h84_explicit_tiled_overrides_fixed_height'; floating=$false },
            @{ title='h84-fixed-width'; minW=320; minH=0; maxW=320; maxH=0; gate='h84_fixed_width_stays_tiled'; floating=$false },
            @{ title='h84-zero'; minW=0; minH=0; maxW=0; maxH=0; gate='h84_zero_bounds_stay_tiled'; floating=$false }
        )) {
            $h = H84Open $case.title $case.minW $case.minH $case.maxW $case.maxH
            Start-Sleep -Milliseconds 450
            $window = H84Window $h
            H84Gate $case.gate ($window.is_floating -eq $case.floating) (H84Observe $h)
        }
        Shot 'h84-native-bounds'
    } catch {
        $h84Evidence.error = $_ | Out-String
        $h84Evidence.error | Set-Content (Join-Path $h84Dir 'error.txt')
    } finally {
        [H84NativeBounds]::CloseAll()
        if ($null -ne $h84Process) { Stop-WiriGracefully $h84Process 'h84' }
        $gone = H84Wait { [H84NativeBounds]::AllClosed -and @($h84Forms | Where-Object { [LT]::IsWindow($_) }).Count -eq 0 } 3000
        $summary.h84_cleanup = $gone -and ($null -eq $h84Process -or $h84Process.HasExited)
        $h84Evidence | ConvertTo-Json -Depth 14 | Set-Content (Join-Path $h84Dir 'evidence.json')
    }
}
