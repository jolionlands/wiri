# H94: windows keep the final say over their size and the layout adapts (niri
# `src/layout/scrolling.rs:4860-4875` `Column::width` is the size the windows committed,
# `:1287-1305` `ScrollingSpace::update_window` moves the neighbours with it, `:1293-1300`
# the column width intent is only what the next request asks for).
# Dot-source only from live-test.ps1 while its controller holds the desktop mutex.
# The fixtures are ordinary WinForms windows of this process that resize *themselves* from
# their own UI thread, exactly like an application that keeps a size of its own.  No product
# test API: fixtures are found through the existing `windows --json` CLI.
& {
    $h94Dir = Join-Path $D 'h94-self-resize'
    $null = New-Item -ItemType Directory -Force $h94Dir
    $h94Gates = @(
        'h94_self_resize_window_keeps_its_width',
        'h94_self_resize_grows_and_pushes_the_neighbour',
        'h94_self_resize_shrink_moves_the_neighbour_back',
        'h94_self_resize_past_the_work_area',
        'h94_self_resize_settles_without_writes',
        'h94_cleanup'
    )
    foreach ($gate in $h94Gates) { $summary[$gate] = $false }
    $h94Evidence = [ordered]@{
        scope = 'single output, harness-owned WinForms windows resizing themselves; the stacked-column height and the in-flight native-loop cases are pure engine tests'
        checks = [ordered]@{}
    }
    $h94Forms = New-Object System.Collections.ArrayList
    $h94Runtime = @{ serial = 0 }
    $h94Proc = $null
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;

public static class H94SelfResize {
    [StructLayout(LayoutKind.Sequential)] public struct OuterRect { public int L, T, R, B; }
    [StructLayout(LayoutKind.Sequential)] public struct NativePoint { public int X, Y; }
    [StructLayout(LayoutKind.Sequential)] public struct MinMaxInfo {
        public NativePoint Reserved, MaxSize, MaxPosition, MinTrack, MaxTrack;
    }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out OuterRect r);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);

    sealed class ProbeForm : Form {
        public readonly object Sync = new object();
        public int Writes;
        public Action Ready;
        protected override void OnShown(EventArgs e) {
            base.OnShown(e);
            if (Ready != null) Ready();
        }
        protected override void WndProc(ref Message m) {
            // Every external placement, wiri's included, arrives as WM_WINDOWPOSCHANGED.
            if (m.Msg == 0x0047) { lock (Sync) { Writes = Writes + 1; } }
            base.WndProc(ref m);
            if (m.Msg == 0x0024 && m.LParam != IntPtr.Zero) {
                // After the default processing, which clamps an ordinary window to the virtual
                // screen: an application that takes a size of its own raises its maximum
                // tracking size, and the work-area gate needs a window wider than that.
                MinMaxInfo info = (MinMaxInfo)Marshal.PtrToStructure(m.LParam, typeof(MinMaxInfo));
                info.MaxTrack.X = 6000;
                info.MaxTrack.Y = 6000;
                Marshal.StructureToPtr(info, m.LParam, false);
            }
        }
        public void ChooseSize(int width, int height) {
            // The window's own decision: from its own UI thread, like an application that
            // resizes itself.  Never a SetWindowPos from the harness thread.
            Size = new System.Drawing.Size(width, height);
        }
    }

    sealed class Opening {
        public readonly object Sync = new object();
        public IntPtr Handle = IntPtr.Zero;
        public bool Ready, Cancelled, Finished;
        public Exception Error;
    }

    static readonly List<Opening> Openings = new List<Opening>();
    static readonly Dictionary<IntPtr, ProbeForm> Forms = new Dictionary<IntPtr, ProbeForm>();

    public static bool AllClosed {
        get {
            Opening[] owned;
            lock (Openings) owned = Openings.ToArray();
            foreach (Opening opening in owned) {
                lock (opening.Sync) { if (!opening.Finished) return false; }
            }
            return true;
        }
    }

    static void Cancel(Opening opening) {
        lock (opening.Sync) opening.Cancelled = true;
    }

    static ProbeForm Find(IntPtr h) {
        lock (Forms) {
            if (!Forms.ContainsKey(h)) throw new InvalidOperationException("H94 fixture is not owned: " + h);
            return Forms[h];
        }
    }

    public static IntPtr Open(string title) {
        Opening opening = new Opening();
        lock (Openings) Openings.Add(opening);
        Thread thread = new Thread(delegate() {
            // Nothing may escape this thread: an unhandled exception on any thread ends the
            // whole harness process, and the fixtures belong to the harness.
            ProbeForm form = null;
            try {
                form = new ProbeForm();
                form.Text = title;
                form.FormBorderStyle = FormBorderStyle.Sizable;
                form.StartPosition = FormStartPosition.Manual;
                form.Location = new Point(160, 140);
                form.ClientSize = new System.Drawing.Size(420, 320);
                form.MinimumSize = new System.Drawing.Size(0, 0);
                // The largest tracking size the form can get. Windows still caps an ordinary
                // window at the virtual screen plus its borders (1386 px here), so the gate
                // that needs a window wider than the work area uses the width the window
                // actually took and requires only that it exceed the work area.
                form.MaximumSize = new System.Drawing.Size(6000, 6000);
                form.ShowInTaskbar = true;
                ProbeForm owned = form;
                form.Ready = delegate {
                    try {
                        lock (opening.Sync) {
                            if (opening.Cancelled) return;
                            opening.Handle = owned.Handle;
                            lock (Forms) Forms.Add(opening.Handle, owned);
                            opening.Ready = true;
                            Monitor.PulseAll(opening.Sync);
                        }
                    } catch (Exception error) {
                        lock (opening.Sync) { opening.Error = error; Monitor.PulseAll(opening.Sync); }
                        try { owned.Close(); } catch (Exception) { }
                    }
                };
                lock (opening.Sync) { if (opening.Cancelled) return; }
                Application.Run(form);
            } catch (Exception error) {
                lock (opening.Sync) opening.Error = error;
            } finally {
                try {
                    if (form != null) {
                        IntPtr handle = IntPtr.Zero;
                        try { handle = form.Handle; } catch (Exception) { }
                        if (handle != IntPtr.Zero) { lock (Forms) Forms.Remove(handle); }
                        form.Dispose();
                    }
                } catch (Exception) { }
                lock (opening.Sync) { opening.Finished = true; Monitor.PulseAll(opening.Sync); }
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
        System.Diagnostics.Stopwatch clock = System.Diagnostics.Stopwatch.StartNew();
        Exception failure;
        lock (opening.Sync) {
            while (!opening.Ready && !opening.Finished && clock.ElapsedMilliseconds < 15000)
                Monitor.Wait(opening.Sync, (int)Math.Max(1, 15000 - clock.ElapsedMilliseconds));
            if (opening.Ready) return opening.Handle;
            opening.Cancelled = true;
            failure = opening.Error;
        }
        Cancel(opening);
        if (failure != null) throw new InvalidOperationException("H94 form failed to open", failure);
        throw new TimeoutException("H94 form did not open; its UI thread owns cancellation");
    }

    public static void ChooseSize(IntPtr h, int width, int height) {
        ProbeForm form = Find(h);
        form.BeginInvoke((MethodInvoker)delegate { form.ChooseSize(width, height); });
    }

    public static int Writes(IntPtr h) { ProbeForm form = Find(h); lock (form.Sync) { return form.Writes; } }
    public static void ResetWrites(IntPtr h) { ProbeForm form = Find(h); lock (form.Sync) { form.Writes = 0; } }

    public static int[] Outer(IntPtr h) {
        OuterRect r;
        if (!GetWindowRect(h, out r)) return null;
        return new int[] { r.L, r.T, r.R - r.L, r.B - r.T };
    }

    public static void Close(IntPtr h) {
        try {
            ProbeForm form = Find(h);
            if (!form.IsDisposed) form.BeginInvoke((MethodInvoker)delegate { try { form.Close(); } catch (Exception) { } });
        } catch (Exception) { }
    }

    public static void CloseAll() {
        Opening[] owned;
        lock (Openings) owned = Openings.ToArray();
        foreach (Opening opening in owned) Cancel(opening);
        IntPtr[] handles;
        lock (Forms) { handles = new List<IntPtr>(Forms.Keys).ToArray(); }
        foreach (IntPtr h in handles) { try { Close(h); } catch (Exception) { } }
    }
}
'@
    function H94Command([string]$name, [string[]]$arguments) {
        $h94Runtime.serial++
        $raw = Ctl ('h94-{0:D3}-{1}' -f $h94Runtime.serial, $name) $arguments
        if ($steps[-1].rc -ne 0) { throw "H94 $name failed: $raw" }
        return $raw
    }
    function H94Windows {
        $reply = (H94Command 'windows' @('windows', '--json')) | ConvertFrom-Json
        if (-not $reply.success -or $null -eq $reply.result.windows) { throw 'H94 windows IPC shape changed' }
        return @($reply.result.windows)
    }
    function H94Wait([scriptblock]$condition, [int]$timeoutMs = 6000, [int]$intervalMs = 120) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        do {
            if (& $condition) { return $true }
            Start-Sleep -Milliseconds $intervalMs
        } while ($clock.ElapsedMilliseconds -lt $timeoutMs)
        return $false
    }
    # Written after every gate group, so a phase error still leaves the checks that ran.
    function Save-H94Evidence {
        try {
            $h94Evidence['gates'] = [ordered]@{}
            foreach ($gate in $h94Gates) { $h94Evidence.gates[$gate] = $summary[$gate] }
            $h94Evidence['forms'] = @($h94Forms | ForEach-Object { $_.ToInt64() })
            ($h94Evidence | ConvertTo-Json -Depth 10) | Set-Content -Encoding UTF8 (Join-Path $h94Dir 'evidence.json')
        } catch { }
    }
    # The window keeps `$width` for a whole settle period: the layout must not pull it back.
    function H94HoldsWidth([IntPtr]$h, [int]$width, [int]$holdMs = 1500) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt $holdMs) {
            $outer = [H94SelfResize]::Outer($h)
            if ($null -eq $outer -or [Math]::Abs($outer[2] - $width) -gt 1) { return $false }
            Start-Sleep -Milliseconds 100
        }
        return $true
    }
    # Wait until the fixture has stopped receiving placements, so the window's next resize of
    # itself cannot fall inside wiri's 250 ms "this was my own write" guard (which correctly
    # refuses to read a placement it just made as the window's choice).
    function H94Quiet([IntPtr]$h, [int]$quietMs = 500) {
        $previous = @(-1)
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt ($quietMs + 3000)) {
            $writes = [H94SelfResize]::Writes($h)
            if ($writes -eq $previous[0]) { return $true }
            $previous[0] = $writes
            Start-Sleep -Milliseconds $quietMs
        }
        return $false
    }
    # The strip geometry of two fixtures as the IPC layout reports it: positions in the
    # workspace view, so the difference and the gap between them do not depend on how far the
    # camera has scrolled (niri keeps the neighbours' widths and moves them,
    # scrolling.rs:1287-1305).
    function H94Span([IntPtr]$a, [IntPtr]$b) {
        $list = @(H94Windows)
        $leftEntry = @($list | Where-Object { $_.id -eq $a.ToInt64() })
        $rightEntry = @($list | Where-Object { $_.id -eq $b.ToInt64() })
        if ($leftEntry.Count -ne 1 -or $rightEntry.Count -ne 1) { throw 'H94 fixture is not managed' }
        $leftPos = [double]$leftEntry[0].layout.tile_pos_in_workspace_view[0]
        $rightPos = [double]$rightEntry[0].layout.tile_pos_in_workspace_view[0]
        [ordered]@{
            span = $rightPos - $leftPos
            gap = $rightPos - $leftPos - [double]$leftEntry[0].layout.tile_size[0]
            left_size = [double]$leftEntry[0].layout.tile_size[0]
            right_size = [double]$rightEntry[0].layout.tile_size[0]
        }
    }
    function H94Open([string]$title) {
        $h = [H94SelfResize]::Open($title)
        $null = $h94Forms.Add($h)
        if (-not (H94Wait { @(H94Windows | Where-Object { $_.id -eq $h.ToInt64() }).Count -eq 1 }) ) {
            $h94Evidence.checks[$title + '_admission'] = [ordered]@{ hwnd = $h.ToInt64(); windows = @(H94Windows) }
            throw "H94 admission timed out: $title"
        }
        if ([LT]::Pid($h) -ne $PID) { throw 'H94 fixture is not owned' }
        # Let the admission placement and its 250 ms self-apply guard pass before the window
        # takes its own size, so the report that follows cannot be one of wiri's writes:
        # wait until two consecutive samples show no native placement at all.
        $quiet = @(-1)
        $null = H94Wait {
            $writes = [H94SelfResize]::Writes($h)
            $same = ($writes -eq $quiet[0])
            $quiet[0] = $writes
            $same
        } 4000 200
        return $h
    }
    # The lane owns its daemon: dot-sourced late in the cycle, it used to run after the
    # harness's main daemon had already quit, so every `windows --json` call (and therefore
    # every H94 gate) failed with rc=1 against no pipe at all.
    $h94Base = @'
input { mod-key "ctrl-alt" }
animations { off; }
layout {
    gaps 16
    focus-ring { off; }
    border { off; }
}
'@
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count) { throw 'H94 refuses an existing daemon' }
        $h94Proc = Start-WiriWithConfig $h94Base (Join-Path $h94Dir 'wiri.stderr.log')
        if ($null -eq $h94Proc -or $h94Proc.HasExited) { throw 'H94 daemon did not start' }
        # A workspace of its own: the shared desktop already holds other applications'
        # windows, and the two fixtures must be the columns their strip positions describe.
        $null = Ctl 'h94-workspace' @('action', 'focus-workspace', '2')
        Start-Sleep -Milliseconds 800
        try {
            $first = H94Open 'h94-self-one'
            $before = [H94SelfResize]::Outer($first)
            # Grow by 205 px on its own, whatever width the layout gave the column.
            $chosenWidth = $before[2] + 205
            $null = H94Quiet $first
            [H94SelfResize]::ChooseSize($first, $chosenWidth, 520)
            $chosen = H94Wait { $o = [H94SelfResize]::Outer($first); $null -ne $o -and $o[2] -eq $chosenWidth } 5000
            if (-not $chosen) { throw 'H94 the fixture did not take its own size' }
            $kept = H94HoldsWidth $first $chosenWidth 1800
            $outer = [H94SelfResize]::Outer($first)
            $h94Evidence.checks['keeps_its_width'] = [ordered]@{
                before = $before; chosen = @($chosenWidth, 520); outer = $outer; held = $kept
                writes = [H94SelfResize]::Writes($first)
                windows = @(H94Windows | Where-Object { $_.id -eq $first.ToInt64() })
            }
            $summary['h94_self_resize_window_keeps_its_width'] = [bool]$kept
        } catch { $h94Evidence.checks['keeps_its_width_error'] = ($_ | Out-String).Trim() }
        Save-H94Evidence

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            $second = H94Open 'h94-self-two'
            $null = H94Wait { $o = [H94SelfResize]::Outer($second); $null -ne $o } 3000
            # Order the two fixtures left to right; both are their own column.
            $pair = @(@{ h = $first; rect = [H94SelfResize]::Outer($first) }, @{ h = $second; rect = [H94SelfResize]::Outer($second) })
            $pair = @($pair | Sort-Object { $_.rect[0] })
            $left = $pair[0]; $right = $pair[1]
            $rightBefore = $right.rect
            $spanBefore = H94Span $left.h $right.h
            # The left window grows by 200 px on its own.
            $grownWidth = $left.rect[2] + 200
            $null = H94Quiet $left.h
            [H94SelfResize]::ChooseSize($left.h, $grownWidth, 520)
            $took = H94Wait { $o = [H94SelfResize]::Outer($left.h); $null -ne $o -and $o[2] -eq $grownWidth } 5000
            $moved = H94Wait {
                $span = H94Span $left.h $right.h
                $span.span -ge $spanBefore.span + 150
            } 6000 200
            $spanAfter = H94Span $left.h $right.h
            $leftOuter = [H94SelfResize]::Outer($left.h)
            $rightOuter = [H94SelfResize]::Outer($right.h)
            $h94Evidence.checks['grows_and_pushes'] = [ordered]@{
                left = $left.h.ToInt64(); right = $right.h.ToInt64(); took = $took
                left_outer = $leftOuter; right_before = $rightBefore; right_outer = $rightOuter
                span_before = $spanBefore; span_after = $spanAfter; moved = $moved
            }
            $summary['h94_self_resize_grows_and_pushes_the_neighbour'] = [bool]($took -and $moved -and $spanAfter.gap -ge -2 -and $spanAfter.right_size -eq $spanBefore.right_size)

            # Shrinking itself below the column's own width moves the neighbour back.
            $shrinkWidth = [Math]::Max(300, $left.rect[2] - 80)
            $null = H94Quiet $left.h
            [H94SelfResize]::ChooseSize($left.h, $shrinkWidth, 520)
            $tookBack = H94Wait { $o = [H94SelfResize]::Outer($left.h); $null -ne $o -and $o[2] -eq $shrinkWidth } 5000
            $back = H94Wait {
                $span = H94Span $left.h $right.h
                $span.span -le $spanAfter.span - 60
            } 6000 200
            $leftBack = [H94SelfResize]::Outer($left.h)
            $rightBack = [H94SelfResize]::Outer($right.h)
            $spanBack = H94Span $left.h $right.h
            $h94Evidence.checks['shrink_moves_back'] = [ordered]@{
                took = $tookBack; left_outer = $leftBack; right_before = $rightOuter; right_outer = $rightBack
                span_before = $spanAfter; span_after = $spanBack; moved_back = $back
            }
            $summary['h94_self_resize_shrink_moves_the_neighbour_back'] = [bool]($tookBack -and $back -and $spanBack.gap -ge -2)
        } catch { $h94Evidence.checks['neighbour_error'] = ($_ | Out-String).Trim() }
        Save-H94Evidence

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            # Wider than the work area: the strip has to scroll, and the window keeps the size
            # it took (the layout must not pull it back to the view).
            $workWidth = [System.Windows.Forms.Screen]::PrimaryScreen.WorkingArea.Width
            $null = H94Quiet $first
            [H94SelfResize]::ChooseSize($first, 1400, 520)
            $took = H94Wait {
                $o = [H94SelfResize]::Outer($first)
                $null -ne $o -and $o[2] -gt $workWidth
            } 5000
            $chosenOuter = [H94SelfResize]::Outer($first)
            $held = $false
            if ($null -ne $chosenOuter) { $held = H94HoldsWidth $first $chosenOuter[2] 1500 }
            $alive = $false
            try { $null = H94Command 'state' @('state', '--json'); $alive = $true } catch { $alive = $false }
            $h94Evidence.checks['past_work_area'] = [ordered]@{
                took = $took; held = $held; daemon_answers = $alive
                work_area_width = $workWidth; chosen = $chosenOuter
                outer = [H94SelfResize]::Outer($first)
            }
            $summary['h94_self_resize_past_the_work_area'] = [bool]($took -and $held -and $alive)
        } catch { $h94Evidence.checks['past_work_area_error'] = ($_ | Out-String).Trim() }
        Save-H94Evidence

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            # Once the window has its size, wiri must not write the geometry again: no
            # SetWindowPos means no WM_WINDOWPOSCHANGED, and no oscillation over the period.
            $settledWidth = [Math]::Max(320, $shrinkWidth - 60)
            $null = H94Quiet $first
            [H94SelfResize]::ChooseSize($first, $settledWidth, 520)
            $null = H94Wait { $o = [H94SelfResize]::Outer($first); $null -ne $o -and $o[2] -eq $settledWidth } 5000
            $null = H94HoldsWidth $first $settledWidth 1200
            [H94SelfResize]::ResetWrites($first)
            $start = [H94SelfResize]::Outer($first)
            $quiet = $true
            $clock = [Diagnostics.Stopwatch]::StartNew()
            while ($clock.ElapsedMilliseconds -lt 2200) {
                if ([H94SelfResize]::Writes($first) -ne 0) { $quiet = $false; break }
                $now = [H94SelfResize]::Outer($first)
                if ($null -eq $now -or $now[2] -ne $start[2] -or $now[3] -ne $start[3]) { $quiet = $false; break }
                Start-Sleep -Milliseconds 100
            }
            $h94Evidence.checks['settles_without_writes'] = [ordered]@{
                quiet = $quiet; writes = [H94SelfResize]::Writes($first); start = $start
                end = [H94SelfResize]::Outer($first); hold_ms = 2200
            }
            $summary['h94_self_resize_settles_without_writes'] = [bool]$quiet
        } catch { $h94Evidence.checks['settles_error'] = ($_ | Out-String).Trim() }
        Save-H94Evidence

        try {
            [H94SelfResize]::CloseAll()
            $closed = H94Wait { [H94SelfResize]::AllClosed } 12000
            $h94Ids = @($h94Forms | ForEach-Object { $_.ToInt64() })
            $listed = @(H94Windows | Where-Object { $h94Ids -contains [long]$_.id }).Count
            if ($closed -and $listed -ne 0) {
                $null = H94Wait {
                    @(H94Windows | Where-Object { $h94Ids -contains [long]$_.id }).Count -eq 0
                } 8000
                $listed = @(H94Windows | Where-Object { $h94Ids -contains [long]$_.id }).Count
            }
            $h94Evidence.checks['cleanup'] = [ordered]@{ all_closed = $closed; still_managed = $listed }
            $summary['h94_cleanup'] = [bool]($closed -and $listed -eq 0)
        } catch { $h94Evidence.checks['cleanup_error'] = ($_ | Out-String).Trim() }
        Save-H94Evidence
    } finally {
        try { [H94SelfResize]::CloseAll() } catch { }
        $null = H94Wait { [H94SelfResize]::AllClosed } 12000
        if ($null -ne $h94Proc -and -not $h94Proc.HasExited) { Stop-WiriGracefully $h94Proc 'h94' }
        Save-H94Evidence
    }
}
