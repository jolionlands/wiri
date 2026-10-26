# H54-H57: part-time compositor. Runs under the parent desktop lock/profile.
# Inspection while animating is passive: no IPC or injected input between samples.
& {
    $kDir = Join-Path $D 'lane-k'
    $null = New-Item -ItemType Directory -Force $kDir
    $kEvidence = [ordered]@{}
    $kForms = New-Object 'System.Collections.Generic.List[System.IntPtr]'
    $kDaemon = $null
    $kNames = @('h54_open_cloaks_source_and_creates_passive_layer', 'h54_open_reaches_final_rect_and_uncloaks',
        'h55_close_hwnd_dies_while_snapshot_animates', 'h55_close_layer_is_destroyed',
        'h56_both_workspaces_slide_in_two_midway_screenshots', 'h56_workspace_final_visibility',
        'h57_resize_crossfade_layer_and_final_width', 'h57_input_cancels_and_snaps',
        'h57_all_effects_leave_no_layer_at_idle', 'h57_global_off_never_creates_layer')
    foreach ($kName in $kNames) { $summary[$kName] = $false }
    function KCtl([string]$tag,[string[]]$words) {
        $kStep = $steps.Count
        $kReply = Ctl $tag $words
        if ($steps.Count -ne $kStep+1 -or [string]$steps[$kStep].rc -ne '0') { throw "Required IPC failed: $tag" }
        return $kReply
    }
    function KLayers { return @([LaneKNative]::Layers($kDaemon.Id)) }
    function KWaitLayer([int]$kind,[int]$milliseconds=1600) {
        $kClock = [Diagnostics.Stopwatch]::StartNew()
        do {
            $kFound = @(KLayers | Where-Object { $_.Kind -eq $kind })
            if ($kFound.Count -gt 0) { return $kFound[0] }
            Start-Sleep -Milliseconds 10
        } while ($kClock.ElapsedMilliseconds -lt $milliseconds)
        return $null
    }
    function KOpen([string]$title,[int]$red,[int]$green,[int]$blue) {
        $kHandle = [TestForms]::Open($title,$red,$green,$blue)
        $kForms.Add($kHandle)
        return $kHandle
    }
    function KConfig([string]$animation) {
        return @"
input { mod-key "ctrl-alt"; }
animations { $animation }
workspace "lane-k-outside"
workspace "lane-k-one"
workspace "lane-k-two"
layout {
    center-focused-column "never"
    default-column-width { fixed 400; }
    focus-ring { off; }
    border { off; }
}
window-rule { match title="^lane-k-"; open-on-workspace "lane-k-one"; }
window-rule { match title="^lane-k-blue$"; open-on-workspace "lane-k-two"; }
binds { Ctrl+Alt+Q { close-window; }; }
"@
    }
    function KReload([string]$animation) {
        [IO.File]::WriteAllText($cfg,(KConfig $animation),(New-Object Text.UTF8Encoding($false)))
        $null = KCtl 'k-reload' @('reload-config')
        Start-Sleep -Milliseconds 1400
    }
    try {
        Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Text;
public static class LaneKNative {
    delegate bool EnumProc(IntPtr h,IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p,IntPtr l);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetClassName(IntPtr h,StringBuilder s,int n);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr GetProp(IntPtr h,string key);
    [DllImport("user32.dll",EntryPoint="GetWindowLongW")] static extern int GetLong(IntPtr h,int n);
    public class Layer { public long Hwnd,Source,Frames,Thumbnails,IncomingY,OutgoingY; public int Kind,Style; }
    public static Layer[] Layers(int pid) {
        var found = new List<Layer>();
        EnumWindows(delegate(IntPtr h,IntPtr unused) {
            uint owner; GetWindowThreadProcessId(h,out owner);
            var cls = new StringBuilder(256); GetClassName(h,cls,256);
            if (owner==(uint)pid && cls.ToString()=="WiriAnimationLayer" && IsWindowVisible(h))
            found.Add(new Layer {Hwnd=h.ToInt64(),Source=GetProp(h,"WiriAnimationSource").ToInt64(),
                Frames=GetProp(h,"WiriAnimationFrames").ToInt64(),Thumbnails=GetProp(h,"WiriAnimationThumbnailCount").ToInt64(),
                IncomingY=GetProp(h,"WiriAnimationIncomingY").ToInt64(),OutgoingY=GetProp(h,"WiriAnimationOutgoingY").ToInt64(),
                Kind=(int)GetProp(h,"WiriAnimationKind").ToInt64(),Style=GetLong(h,-20)});
            return true;
        },IntPtr.Zero);
        return found.ToArray();
    }
    public static Rectangle ColorBounds(string path,int red,int green,int blue) {
        using(var bmp = new Bitmap(path)) {
            int l=bmp.Width,t=bmp.Height,r=-1,b=-1;
            for(int y=0;y<bmp.Height;y++) for(int x=0;x<bmp.Width;x++) {
                Color p=bmp.GetPixel(x,y);
                if(p.R==red && p.G==green && p.B==blue) {l=Math.Min(l,x);t=Math.Min(t,y);r=Math.Max(r,x);b=Math.Max(b,y);}
            }
            return r<l ? Rectangle.Empty : Rectangle.FromLTRB(l,t,r+1,b+1);
        }
    }
}
'@
        if (-not ('LaneE.Native' -as [type])) { Add-Type -Path (Join-Path $PSScriptRoot '..\perf\Native.cs') }
        if (@(Get-Process wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'Lane K refuses an existing daemon' }
        $kLog = Join-Path $kDir 'wiri.stderr.log'
        $kDaemon = Start-WiriWithConfig (KConfig '') $kLog
        $null = KCtl 'k-workspace-one' @('action','focus-workspace','2')
        Start-Sleep -Seconds 2
        # Before endpoint: same enabled process, before this lane's transition sequence.
        $kEvidence.resources_before = [LaneE.Native]::ReadResources($kDaemon.Id)
        $kEvidence.idle_before = [LaneE.Native]::Idle($kDaemon.Id,30000)
        $kDefault = KOpen 'lane-k-default-speed' 51 172 93
        Start-Sleep -Seconds 2
        FocusForm $kDefault
        if (-not (WaitForeground $kDefault 0 3000)) { throw 'Default fixture not foreground' }
        $null = KCtl 'k-default-resize' @('action','set-column-width','520')
        Start-Sleep -Seconds 2
        $null = KCtl 'k-default-close' @('action','close-window','--id',[string][int64]$kDefault)
        Start-Sleep -Seconds 2
        # Slow only for lifecycle/screenshots. Explicit linear workspace easing gives
        # two well-separated frames with both workspaces intersecting the output.
        KReload 'slowdown 10; workspace-switch { duration-ms 150; curve linear; };'
        $kRed = KOpen 'lane-k-red' 213 47 67
        $kLayer = KWaitLayer 1
        $kCloaked = [LT]::Cloaked($kRed)
        $kEvidence.open_mid = [ordered]@{ layer=$kLayer; cloaked=$kCloaked; rect=(Rc $kRed) }
        $summary.h54_open_cloaks_source_and_creates_passive_layer = ($null -ne $kLayer) -and ($kCloaked -gt 0) -and
            (($kLayer.Style -band 0x080800A0) -eq 0x080800A0) -and (($kLayer.Style -band 0x40000) -eq 0) -and ($kLayer.Thumbnails -gt 0)
        Shot 'h54-open-mid'
        Start-Sleep -Milliseconds 2300
        $kFinal = Rc $kRed
        $kEvidence.open_final = $kFinal
        $summary.h54_open_reaches_final_rect_and_uncloaks = ((KLayers).Count -eq 0) -and ([LT]::Cloaked($kRed) -eq 0) -and
            (Near $kFinal.W 400 4) -and ($kFinal.T -ge 0) -and ($kFinal.B -le [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Bottom)
        # Resize: old image and live resized content coexist behind a source cloak.
        FocusForm $kRed
        $null = KCtl 'k-resize' @('action','set-column-width','600')
        $kResize = KWaitLayer 4
        $kEvidence.resize_mid = $kResize
        Start-Sleep -Seconds 5
        $summary.h57_resize_crossfade_layer_and_final_width = ($null -ne $kResize) -and (Near (Rc $kRed).W 600 4) -and ((KLayers).Count -eq 0)
        $null = KCtl 'k-resize-interrupt' @('action','set-column-width','400')
        $kInterrupted = KWaitLayer 4
        Chord 'esc'
        Start-Sleep -Milliseconds 350
        $summary.h57_input_cancels_and_snaps = ($null -ne $kInterrupted) -and ((KLayers).Count -eq 0) -and ([LT]::Cloaked($kRed) -eq 0) -and (Near (Rc $kRed).W 400 4)
        $kBlue = KOpen 'lane-k-blue' 41 101 219
        Start-Sleep -Seconds 2
        FocusForm $kRed
        $null = KCtl 'k-workspace-two' @('action','focus-workspace','3')
        $kWs = KWaitLayer 3
        Start-Sleep -Milliseconds 330
        $kSlide1 = @(KLayers)
        Shot 'h56-workspaces-mid-1'
        Start-Sleep -Milliseconds 360
        $kSlide2 = @(KLayers)
        Shot 'h56-workspaces-mid-2'
        Start-Sleep -Seconds 2
        $kPath1 = Join-Path $D 'h56-workspaces-mid-1.png'
        $kPath2 = Join-Path $D 'h56-workspaces-mid-2.png'
        $kR1 = [LaneKNative]::ColorBounds($kPath1,213,47,67); $kR2 = [LaneKNative]::ColorBounds($kPath2,213,47,67)
        $kB1 = [LaneKNative]::ColorBounds($kPath1,41,101,219); $kB2 = [LaneKNative]::ColorBounds($kPath2,41,101,219)
        $kEvidence.workspace = [ordered]@{start=$kWs; mid1=$kSlide1; mid2=$kSlide2; red1=$kR1; red2=$kR2; blue1=$kB1; blue2=$kB2}
        $summary.h56_both_workspaces_slide_in_two_midway_screenshots = ($kSlide1.Count -eq 1) -and ($kSlide2.Count -eq 1) -and
            ($kR1.Width -gt 40) -and ($kR2.Width -gt 40) -and ($kB1.Width -gt 40) -and ($kB2.Width -gt 40) -and
            ($kR2.Bottom -lt $kR1.Bottom-10) -and ($kB2.Top -lt $kB1.Top-10)
        $summary.h56_workspace_final_visibility = ((KLayers).Count -eq 0) -and ([LT]::Cloaked($kBlue) -eq 0) -and
            [LT]::IsWindowVisible($kBlue) -and ((-not [LT]::IsWindowVisible($kRed)) -or ([LT]::Cloaked($kRed) -gt 0))
        FocusForm $kBlue
        $null = KCtl 'k-close-blue' @('action','close-window','--id',[string][int64]$kBlue)
        $kClose = KWaitLayer 2
        $kEvidence.close_mid = [ordered]@{layer=$kClose; source_exists=[LT]::IsWindow($kBlue)}
        $summary.h55_close_hwnd_dies_while_snapshot_animates = ($null -ne $kClose) -and (-not [LT]::IsWindow($kBlue))
        Shot 'h55-close-mid'
        Start-Sleep -Seconds 2
        $summary.h55_close_layer_is_destroyed = ((KLayers).Count -eq 0) -and (-not [LT]::IsWindow($kBlue))
        $null = KCtl 'k-return-one' @('action','focus-workspace','2')
        Start-Sleep -Seconds 3
        $kRest = Rc $kRed
        Start-Sleep -Seconds 1
        $kRest2 = Rc $kRed
        $summary.h57_all_effects_leave_no_layer_at_idle = ((KLayers).Count -eq 0) -and ([LT]::Cloaked($kRed) -eq 0) -and
            ($kRest.L -eq $kRest2.L) -and ($kRest.T -eq $kRest2.T) -and ($kRest.W -eq $kRest2.W)
        KReload 'off;'
        $kOff = KOpen 'lane-k-off' 119 72 161
        $kOffObserved = $false
        $kClock = [Diagnostics.Stopwatch]::StartNew()
        while ($kClock.ElapsedMilliseconds -lt 500) { if ((KLayers).Count -ne 0) { $kOffObserved=$true }; Start-Sleep -Milliseconds 10 }
        FocusForm $kOff
        $null = KCtl 'k-off-resize' @('action','set-column-width','450')
        if ((KLayers).Count -ne 0) { $kOffObserved=$true }
        $null = KCtl 'k-off-close' @('action','close-window','--id',[string][int64]$kOff)
        if ((KLayers).Count -ne 0) { $kOffObserved=$true }
        $null = KCtl 'k-off-workspace' @('action','focus-workspace','3')
        if ((KLayers).Count -ne 0) { $kOffObserved=$true }
        $summary.h57_global_off_never_creates_layer = (-not $kOffObserved) -and (-not [LT]::IsWindow($kOff))
        # Restore the same enabled configuration before the after endpoint.
        KReload ''
        $null = KCtl 'k-enabled-rest' @('action','focus-workspace','2')
        Start-Sleep -Seconds 5
        $kEvidence.resources_after = [LaneE.Native]::ReadResources($kDaemon.Id)
        $kEvidence.idle_after = [LaneE.Native]::Idle($kDaemon.Id,30000)
        $kEvidence.transition_log = @([LaneE.Native]::ReadSharedText($kLog) -split "`n" | Where-Object { $_ -match 'compositor transition' })
        $kEvidence.binary_sha256 = (Get-FileHash $wiri -Algorithm SHA256).Hash
        $kEvidence.measuring_note = 'cpu_ms is whole-process kernel+user delta; not isolated renderer CPU. Idle is one input/IPC-free 30s sleep at each endpoint; context switches are endpoint lower bounds. Shared desktop noise is uncontrolled.'
    } catch { $_ | Out-String | Set-Content (Join-Path $kDir 'error.txt') }
    finally {
        if ($null -ne $kDaemon -and -not $kDaemon.HasExited) {
            foreach ($kHandle in $kForms) {
                if ([LT]::IsWindow($kHandle)) { FocusForm $kHandle; $null = CloseFocused $kHandle 'k-cleanup'; Start-Sleep -Milliseconds 350 }
            }
            Stop-WiriGracefully $kDaemon 'k'
        }
        foreach ($kHandle in $kForms) { if ([LT]::IsWindow($kHandle)) { [TestForms]::CloseForm($kHandle) } }
        $kEvidence | ConvertTo-Json -Depth 9 | Set-Content (Join-Path $kDir 'measurement.json')
    }
}
