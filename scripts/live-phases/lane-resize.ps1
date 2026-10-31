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
    Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;

public static class H94SelfResize {
    [StructLayout(LayoutKind.Sequential)] public struct OuterRect { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out OuterRect r);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);

    sealed class ProbeForm : Form {
        public readonly object Sync = new object();
        public int Writes;
        protected override void WndProc(ref Message m) {
            // Every external placement, wiri's included, arrives as WM_WINDOWPOSCHANGED.
            if (m.Msg == 0x0047) { lock (Sync) { Writes = Writes + 1; } }
            base.WndProc(ref m);
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
        ProbeForm form = null;
        Thread thread = new Thread(delegate() {
            try {
                form = new ProbeForm();
                form.Text = title;
                form.FormBorderStyle = FormBorderStyle.Sizable;
                form.StartPosition = FormStartPosition.Manual;
                form.Location = new Point(160, 140);
                form.ClientSize = new System.Drawing.Size(420, 320);
                form.MinimumSize = new System.Drawing.Size(0, 0);
                form.ShowInTaskbar = true;
                form.Shown += delegate {
                    lock (opening.Sync) {
                        opening.Handle = form.Handle;
                        lock (Forms) Forms.Add(opening.Handle, form);
                        opening.Ready = true;
                        Monitor.PulseAll(opening.Sync);
                    }
                };
                lock (opening.Sync) { if (opening.Cancelled) { form.Close(); return; } }
                Application.Run(form);
            } catch (Exception error) {
                lock (opening.Sync) opening.Error = error;
            } finally {
                if (form != null) {
                    lock (Forms) Forms.Remove(form.Handle);
                    form.Dispose();
                }
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
        ProbeForm form = Find(h);
        if (!form.IsDisposed) form.BeginInvoke((MethodInvoker)delegate { form.Close(); });
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
    try {
        try {
            $first = H94Open 'h94-self-one'
            $before = [H94SelfResize]::Outer($first)
            [H94SelfResize]::ChooseSize($first, 700, 520)
            $chosen = H94Wait { $o = [H94SelfResize]::Outer($first); $null -ne $o -and $o[2] -eq 700 } 5000
            if (-not $chosen) { throw 'H94 the fixture did not take its own size' }
            $kept = H94HoldsWidth $first 700 1800
            $outer = [H94SelfResize]::Outer($first)
            $h94Evidence.checks['keeps_its_width'] = [ordered]@{
                before = $before; chosen = @(700, 520); outer = $outer; held = $kept
                writes = [H94SelfResize]::Writes($first)
                windows = @(H94Windows | Where-Object { $_.id -eq $first.ToInt64() })
            }
            $summary['h94_self_resize_window_keeps_its_width'] = [bool]$kept
        } catch { $h94Evidence.checks['keeps_its_width_error'] = ($_ | Out-String).Trim() }

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            $second = H94Open 'h94-self-two'
            $null = H94Wait { $o = [H94SelfResize]::Outer($second); $null -ne $o } 3000
            # Order the two fixtures left to right; both are their own column.
            $pair = @(@{ h = $first; rect = [H94SelfResize]::Outer($first) }, @{ h = $second; rect = [H94SelfResize]::Outer($second) })
            $pair = @($pair | Sort-Object { $_.rect[0] })
            $left = $pair[0]; $right = $pair[1]
            $rightBefore = $right.rect
            # The left window grows by 180 px on its own.
            [H94SelfResize]::ChooseSize($left.h, 880, 520)
            $took = H94Wait { $o = [H94SelfResize]::Outer($left.h); $null -ne $o -and $o[2] -eq 880 } 5000
            $moved = H94Wait {
                $o = [H94SelfResize]::Outer($right.h)
                $null -ne $o -and $o[0] -ge $rightBefore[0] + 100
            } 6000
            $leftOuter = [H94SelfResize]::Outer($left.h)
            $rightOuter = [H94SelfResize]::Outer($right.h)
            $noOverlap = $leftOuter[0] + $leftOuter[2] -le $rightOuter[0] + 2
            $h94Evidence.checks['grows_and_pushes'] = [ordered]@{
                left = $left.h.ToInt64(); right = $right.h.ToInt64(); took = $took
                left_outer = $leftOuter; right_before = $rightBefore; right_outer = $rightOuter
                moved = $moved; no_overlap = $noOverlap
            }
            $summary['h94_self_resize_grows_and_pushes_the_neighbour'] = [bool]($took -and $moved -and $noOverlap)

            # Shrinking itself below the column's own width moves the neighbour back.
            [H94SelfResize]::ChooseSize($left.h, 420, 520)
            $tookBack = H94Wait { $o = [H94SelfResize]::Outer($left.h); $null -ne $o -and $o[2] -eq 420 } 5000
            $back = H94Wait {
                $o = [H94SelfResize]::Outer($right.h)
                $null -ne $o -and $o[0] -lt $rightOuter[0]
            } 6000
            $leftBack = [H94SelfResize]::Outer($left.h)
            $rightBack = [H94SelfResize]::Outer($right.h)
            $h94Evidence.checks['shrink_moves_back'] = [ordered]@{
                took = $tookBack; left_outer = $leftBack; right_before = $rightOuter; right_outer = $rightBack
                moved_back = $back; no_overlap = ($leftBack[0] + $leftBack[2] -le $rightBack[0] + 2)
            }
            $summary['h94_self_resize_shrink_moves_the_neighbour_back'] = [bool]($tookBack -and $back)
        } catch { $h94Evidence.checks['neighbour_error'] = ($_ | Out-String).Trim() }

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            # Wider than the 1366 px work area: the window keeps its size, wiri does not crash
            # and still answers, and the layout pushes the strip past the view.
            [H94SelfResize]::ChooseSize($first, 1400, 520)
            $took = H94Wait { $o = [H94SelfResize]::Outer($first); $null -ne $o -and $o[2] -eq 1400 } 5000
            $held = H94HoldsWidth $first 1400 1500
            $alive = $false
            try { $null = H94Command 'state' @('state', '--json'); $alive = $true } catch { $alive = $false }
            $h94Evidence.checks['past_work_area'] = [ordered]@{
                took = $took; held = $held; daemon_answers = $alive
                outer = [H94SelfResize]::Outer($first)
            }
            $summary['h94_self_resize_past_the_work_area'] = [bool]($took -and $held -and $alive)
        } catch { $h94Evidence.checks['past_work_area_error'] = ($_ | Out-String).Trim() }

        try {
            if ($null -eq $first) { throw 'H94 the first fixture is missing' }
            # Once the window has its size, wiri must not write the geometry again: no
            # SetWindowPos means no WM_WINDOWPOSCHANGED, and no oscillation over the period.
            [H94SelfResize]::ChooseSize($first, 600, 520)
            $null = H94Wait { $o = [H94SelfResize]::Outer($first); $null -ne $o -and $o[2] -eq 600 } 5000
            $null = H94HoldsWidth $first 600 800
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
    } finally {
        try { [H94SelfResize]::CloseAll() } catch { }
        $null = H94Wait { [H94SelfResize]::AllClosed } 12000
        $h94Evidence['forms'] = @($h94Forms | ForEach-Object { $_.ToInt64() })
        ($h94Evidence | ConvertTo-Json -Depth 10) | Set-Content -Encoding UTF8 (Join-Path $h94Dir 'evidence.json')
    }
}
