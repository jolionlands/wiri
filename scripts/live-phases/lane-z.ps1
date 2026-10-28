# Lane Z / H45: a tile re-raises itself AFTER activation, on its own UI thread.
# The parent owns the desktop mutex and isolated profile. Timers below belong only to
# the test probes, are explicitly armed one-shots, and are disposed with each form.
& {
    $zDir = Join-Path $D 'lane-z'
    $null = New-Item -ItemType Directory -Force $zDir
    $zNames = @(
        'h45_delayed_raise_reproduced', 'h45_floats_above_delayed_tile',
        'h45_float_mru_order', 'h45_tile_keeps_foreground', 'h45_floats_never_topmost',
        'h45_settled_stack_does_not_restack', 'h45_unmanaged_foreground_not_covered',
        'h45_inactive_workspace_floats_stay_hidden'
    )
    foreach ($zName in $zNames) { $summary[$zName] = $false }
    $zProcess = $null
    $zOwnedForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $zTool = [IntPtr]::Zero
    function ZCtl([string]$tag, [string[]]$a) {
        $n = $steps.Count
        $text = Ctl $tag $a
        if ($steps.Count -ne $n+1 -or [string]$steps[$n].rc -ne '0') { throw "Lane Z IPC failed: $tag" }
        return $text
    }
    try {
        Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing,System.Core -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Drawing;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Windows.Forms;
using Microsoft.Win32.SafeHandles;
public static class LaneZ {
    [DllImport("user32.dll")] static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", EntryPoint="GetWindowLongW")] static extern int GetLong(IntPtr h, int index);
    [DllImport("user32.dll", SetLastError=true)] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int ht, uint flags);
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetNamedPipeServerProcessId(SafePipeHandle pipe, out uint pid);
    [StructLayout(LayoutKind.Sequential)] struct WP { public IntPtr h,after; public int x,y,cx,cy; public uint flags; }
    public static bool Above(IntPtr a, IntPtr b) {
        var seen=new HashSet<IntPtr>();
        for (IntPtr h=GetWindow(b,3); h!=IntPtr.Zero && seen.Add(h) && seen.Count<4096; h=GetWindow(h,3)) if (h==a) return true;
        return false;
    }
    public static bool Topmost(IntPtr h) { return (GetLong(h,-20)&8)!=0; }
    sealed class Probe : Form {
        public int requests,raises,inversions,foregroundAtRaise;
        public IntPtr firstFloat,secondFloat,after;
        public bool armed;
        readonly System.Windows.Forms.Timer delay=new System.Windows.Forms.Timer();
        public Probe() {
            delay.Interval=450;
            Activated+=delegate { if (armed) { armed=false; delay.Start(); } };
            delay.Tick+=delegate {
                delay.Stop();
                if (!SetWindowPos(Handle,after,0,0,0,0,0x13)) throw new Win32Exception(Marshal.GetLastWin32Error());
                // The owner thread has not returned to its message pump: an async repair
                // cannot run here before we record the actual forced inversion.
                foregroundAtRaise=GetForegroundWindow()==Handle ? 1 : 0;
                if (Above(Handle,firstFloat) && Above(Handle,secondFloat)) Interlocked.Increment(ref inversions);
                Interlocked.Increment(ref raises);
            };
            FormClosed+=delegate { delay.Dispose(); };
        }
        public void Arm(IntPtr a,IntPtr b) { firstFloat=a; secondFloat=b; after=IntPtr.Zero; armed=true; }
        public void RaiseLater(IntPtr target,IntPtr a,IntPtr b) { firstFloat=a; secondFloat=b; after=target; delay.Start(); }
        protected override void WndProc(ref Message m) {
            if (m.Msg==0x46) {
                WP p=(WP)Marshal.PtrToStructure(m.LParam,typeof(WP));
                if ((p.flags&4)==0) Interlocked.Increment(ref requests);
            }
            base.WndProc(ref m);
        }
    }
    static readonly Dictionary<IntPtr,Probe> forms=new Dictionary<IntPtr,Probe>();
    public static IntPtr Open(string name,bool red) {
        IntPtr h=IntPtr.Zero;
        using (var ready=new ManualResetEvent(false)) {
            var t=new Thread(delegate() {
                var f=new Probe(); f.Text=name; f.Size=new Size(480,340);
                f.BackColor=red ? Color.FromArgb(210,43,43) : Color.FromArgb(40,140,180);
                f.Shown+=delegate { h=f.Handle; lock(forms) { forms.Add(h,f); } ready.Set(); };
                Application.Run(f);
            });
            t.IsBackground=true; t.SetApartmentState(ApartmentState.STA); t.Start();
            if (!ready.WaitOne(15000)) throw new TimeoutException("Lane Z probe did not open");
        }
        return h;
    }
    public static void Arm(IntPtr h,IntPtr a,IntPtr b) {
        Probe f; lock(forms) { f=forms[h]; }
        f.Invoke((MethodInvoker)delegate { f.Arm(a,b); });
    }
    public static void RaiseLater(IntPtr h,IntPtr after,IntPtr a,IntPtr b) {
        Probe f; lock(forms) { f=forms[h]; }
        f.Invoke((MethodInvoker)delegate { f.RaiseLater(after,a,b); });
    }
    public static int[] State(IntPtr h) {
        lock(forms) { var f=forms[h]; return new int[]{f.requests,f.raises,f.inversions,f.foregroundAtRaise}; }
    }
    public static void Close(IntPtr h) {
        Probe f; lock(forms) { if (!forms.TryGetValue(h,out f)) return; forms.Remove(h); }
        if (!f.IsDisposed) f.BeginInvoke((MethodInvoker)delegate { f.Close(); });
    }
    public static string QuitOwned(string name,int pid) {
        using(var p=new NamedPipeClientStream(".",name,PipeDirection.InOut,PipeOptions.Asynchronous)) {
            p.Connect(1000); uint server;
            if (!GetNamedPipeServerProcessId(p.SafePipeHandle,out server)) throw new Win32Exception(Marshal.GetLastWin32Error());
            if (server!=(uint)pid) throw new Exception("Refusing another daemon's pipe");
            byte[] bytes=Encoding.UTF8.GetBytes("{\"type\":\"quit\"}");
            var write=p.BeginWrite(bytes,0,bytes.Length,null,null);
            using(var done=write.AsyncWaitHandle) { if (!done.WaitOne(1500)) return "owned-quit-timeout"; p.EndWrite(write); }
            return "owned-quit-sent";
        }
    }
}
'@
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane Z refuses an existing daemon' }
        $zConfig = @'
input { mod-key "ctrl-alt"; }
animations { off; }
workspace "lane-z-outside"
workspace "lane-z"
workspace "lane-z-away"
layout { default-column-width { proportion 1.0; }; }
window-rule { match title="^lane-z-(tile|float)"; open-on-workspace "lane-z"; }
window-rule { match title="^lane-z-float"; open-floating true; }
'@
        $zProcess = Start-WiriWithConfig $zConfig (Join-Path $zDir 'wiri.stderr.log')
        $null = $zProcess.Handle
        if ($zProcess.HasExited) { throw 'Lane Z daemon exited' }
        foreach ($zRun in 1..$FloatRuns) {
            $zRunDir = Join-Path $zDir ([string]$zRun)
            $null = New-Item -ItemType Directory -Force $zRunDir
            $zResult = [ordered]@{}
            ZCtl "z-$zRun-workspace" @('action','focus-workspace','2') | Out-Null
            $zTile = [LaneZ]::Open(('lane-z-tile-'+$zRun),$false); $zOwnedForms.Add($zTile)
            Start-Sleep -Milliseconds 1000
            $zLower = [LaneZ]::Open(('lane-z-float-lower-'+$zRun),$false); $zOwnedForms.Add($zLower)
            Start-Sleep -Milliseconds 1000
            $zUpper = [LaneZ]::Open(('lane-z-float-red-'+$zRun),$true); $zOwnedForms.Add($zUpper)
            Start-Sleep -Milliseconds 1200
            FocusForm $zLower
            FocusForm $zUpper
            if (-not (WaitForeground $zUpper 0 3000)) { throw 'Lane Z float did not take foreground' }
            $zInitial = @([LaneZ]::State($zTile))
            [LaneZ]::Arm($zTile,$zUpper,$zLower)
            FocusForm $zTile
            $zFocused = WaitForeground $zTile 200 3000
            Start-Sleep -Milliseconds 800
            $zRaised = @([LaneZ]::State($zTile))
            $zResult['h45_delayed_raise_reproduced'] = $zFocused -and ($zRaised[1] -gt $zInitial[1]) -and ($zRaised[2] -gt $zInitial[2]) -and ($zRaised[3] -eq 1)
            $zRect = Rc $zUpper
            $zTopAtCentre = [LT]::TopWindowAt([int]($zRect.L+$zRect.W/2),[int]($zRect.T+$zRect.H/2))
            $zResult['h45_floats_above_delayed_tile'] = [LaneZ]::Above($zUpper,$zTile) -and [LaneZ]::Above($zLower,$zTile) -and ($zTopAtCentre -eq $zUpper) -and (OverlapsRect $zTile $zRect)
            $zResult['h45_float_mru_order'] = [LaneZ]::Above($zUpper,$zLower)
            $zResult['h45_tile_keeps_foreground'] = ([LT]::GetForegroundWindow() -eq $zTile)
            $zResult['h45_floats_never_topmost'] = -not ([LaneZ]::Topmost($zUpper) -or [LaneZ]::Topmost($zLower))
            Shot ('h45-'+$zRun+'-delayed-tile')
            $zRest0 = @([LaneZ]::State($zTile)[0]; [LaneZ]::State($zUpper)[0]; [LaneZ]::State($zLower)[0])
            Start-Sleep -Milliseconds 900
            $zRest1 = @([LaneZ]::State($zTile)[0]; [LaneZ]::State($zUpper)[0]; [LaneZ]::State($zLower)[0])
            $zResult['h45_settled_stack_does_not_restack'] = (SameList $zRest0 $zRest1)

            # A real unmanaged tool takes focus. Cause a tile/float inversion BELOW it:
            # wiri must not promote the floats over that application's window.
            $zTool = [TestForms]::OpenTool(('lane-z-unmanaged-'+$zRun))
            [LT]::SwitchToThisWindow($zTool,$true)
            $zExternalFocused = WaitForeground $zTool 500 3000
            [LaneZ]::RaiseLater($zTile,$zTool,$zUpper,$zLower)
            Start-Sleep -Milliseconds 1300
            $zExternalState = @([LaneZ]::State($zTile))
            $zResult['h45_unmanaged_foreground_not_covered'] = $zExternalFocused -and ($zExternalState[1] -gt $zRaised[1]) -and ($zExternalState[2] -gt $zRaised[2]) -and ([LT]::GetForegroundWindow() -eq $zTool) -and [LaneZ]::Above($zTool,$zUpper) -and [LaneZ]::Above($zTool,$zLower)
            $zResult['h45_floats_never_topmost'] = $zResult['h45_floats_never_topmost'] -and -not ([LaneZ]::Topmost($zUpper) -or [LaneZ]::Topmost($zLower))
            Shot ('h45-'+$zRun+'-unmanaged')
            [TestForms]::CloseForm($zTool); $zTool = [IntPtr]::Zero

            ZCtl "z-$zRun-away" @('action','focus-workspace','3') | Out-Null
            Start-Sleep -Milliseconds 900
            [LaneZ]::RaiseLater($zTile,[IntPtr]::Zero,$zUpper,$zLower)
            Start-Sleep -Milliseconds 1100
            $zResult['h45_inactive_workspace_floats_stay_hidden'] = -not ([LT]::IsWindowVisible($zUpper) -or [LT]::IsWindowVisible($zLower))
            ZCtl "z-$zRun-back" @('action','focus-workspace','2') | Out-Null
            Start-Sleep -Milliseconds 800
            foreach ($zHwnd in @($zUpper,$zLower,$zTile)) {
                FocusForm $zHwnd
                if (-not (CloseFocused $zHwnd ('z-'+$zRun+'-close-'+[int64]$zHwnd))) { throw 'Lane Z could not safely close its probe' }
                Start-Sleep -Milliseconds 500
                [LaneZ]::Close($zHwnd)
            }
            $zOwnedForms.Clear()
            ([pscustomobject]$zResult) | ConvertTo-Json | Set-Content (Join-Path $zRunDir 'summary.json')
            @{ initial=$zInitial; raised=$zRaised; external=$zExternalState; rest_before=$zRest0; rest_after=$zRest1; tile=[int64]$zTile; upper=[int64]$zUpper; lower=[int64]$zLower; top_at_centre=[int64]$zTopAtCentre; rectangle=$zRect } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $zRunDir 'observed.json')
            foreach ($zName in $zNames) {
                if ($zRun -eq 1) { $summary[$zName] = [bool]$zResult[$zName] }
                else { $summary[$zName] = $summary[$zName] -and [bool]$zResult[$zName] }
            }
        }
    } catch {
        $_ | Out-String | Set-Content (Join-Path $zDir 'error.txt')
        foreach ($zName in $zNames) { $summary[$zName] = $false }
    } finally {
        if ($zTool -ne [IntPtr]::Zero) { [TestForms]::CloseForm($zTool) }
        foreach ($zHwnd in $zOwnedForms) { [LaneZ]::Close($zHwnd) }
        if ($null -ne $zProcess -and -not $zProcess.HasExited) {
            try { [LaneZ]::QuitOwned((WiriPipeName),$zProcess.Id) | Set-Content (Join-Path $zDir 'quit.txt') }
            finally { if (-not $zProcess.WaitForExit(5000)) { $zProcess.Kill(); $null = $zProcess.WaitForExit(5000) } }
        }
    }
}
