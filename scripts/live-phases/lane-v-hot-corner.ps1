# Lane V / H64. Runs only under live-test.ps1's shared desktop lock.
# Uses physical rcMonitor corners, restores the cursor/config, and stops only
# its retained daemon. No display, DPI, input-device or other system setting changes.
& {
    $hcDir=Join-Path $D 'lane-v-hot-corner'
    $null=New-Item -ItemType Directory -Path $hcDir -Force
    $hcChecks=[ordered]@{}
    $hcOwned=$null; $hcForm=[IntPtr]::Zero
    $hcSaved=[IO.File]::ReadAllBytes($cfg)
    $hcCursor=[Windows.Forms.Cursor]::Position
    $hcGates=@('h64_config_syntax','h64_near_corner_does_not_open','h64_default_corner_opens',
        'h64_parked_does_not_toggle','h64_ipc_close_while_parked','h64_leave_reenter_opens',
        'h64_fresh_entry_toggles_closed','h64_global_off','h64_per_output_off',
        'h64_output_override_reenables','h64_named_corners_replace_default','h64_owned_cleanup')
    foreach ($key in $hcGates) { $summary[$key]=$false }
    function HCGate([string]$key,[bool]$pass,$observed) {
        $summary[$key]=$pass; $hcChecks[$key]=[ordered]@{pass=$pass;observed=$observed}
    }
    function HCHost {
        $h=H19Host
        if ($h -ne [IntPtr]::Zero -and ($null -eq $hcOwned -or [LT]::Pid($h) -ne $hcOwned.Id)) {
            throw 'Refused foreign overview host'
        }
        return $h
    }
    function HCAction([string]$name) {
        if ($null -eq $hcOwned -or $hcOwned.HasExited) { throw 'Owned H64 daemon is not alive' }
        $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$hcOwned.Id
        try { } finally { $wire.Dispose() }
        $null=Ctl ('h64-'+$name) @('action',$name)
    }
    function HCMove([int]$x,[int]$y,[int]$ms=400) {
        [Windows.Forms.Cursor]::Position=New-Object Drawing.Point $x,$y
        Start-Sleep -Milliseconds $ms
    }
    function HCReload([string]$extra) {
        # Reload from away: configuration changes themselves must not count as entry.
        HCMove ($hcBounds[0]+45) ($hcBounds[1]+45)
        HCAction 'close-overview'
        [IO.File]::WriteAllText($cfg,($hcBase+"`r`n"+$extra),(New-Object Text.UTF8Encoding($false)))
        $null=Ctl 'h64-reload' @('reload-config')
        Start-Sleep -Milliseconds 700
    }
    $hcBase=@'
input { mod-key "ctrl-alt"; }
animations { off; }
binds { Mod+Q { close-window; }; }
'@
    try {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count) { throw 'H64 refuses an existing daemon' }
        if (-not ('LaneVCornerNative' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class LaneVCornerNative {
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct INFO {
        public int Size,L,T,R,B,WL,WT,WR,WB,Flags;
        [MarshalAs(UnmanagedType.ByValTStr,SizeConst=32)] public string Device;
    }
    [StructLayout(LayoutKind.Sequential)] struct PT { public int X,Y; }
    [DllImport("user32.dll")] static extern IntPtr MonitorFromPoint(PT p,uint flags);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern bool GetMonitorInfo(IntPtr h,ref INFO i);
    static INFO Read(int x,int y) {
        INFO i=new INFO(); i.Size=Marshal.SizeOf(typeof(INFO));
        if(!GetMonitorInfo(MonitorFromPoint(new PT{X=x,Y=y},2),ref i)) throw new InvalidOperationException("No physical monitor");
        return i;
    }
    public static int[] Bounds(int x,int y) { INFO i=Read(x,y);return new[]{i.L,i.T,i.R,i.B}; }
    public static string Device(int x,int y) {return Read(x,y).Device;}
}
'@
        }
        $hcBounds=[LaneVCornerNative]::Bounds($hcCursor.X,$hcCursor.Y)
        $hcOutput=[LaneVCornerNative]::Device($hcCursor.X,$hcCursor.Y).Replace('\','\\').Replace('"','\"')
        $hcFixture=Join-Path $hcDir 'corners.kdl'
        [IO.File]::WriteAllText($hcFixture,($hcBase+"`r`ngestures { hot-corners { top-right; bottom-left; bottom-right; }; }`r`noutput `"$hcOutput`" { hot-corners { off; }; }"),(New-Object Text.UTF8Encoding($false)))
        $validated=CtlRaw @('validate-config',(Q $hcFixture))
        HCGate 'h64_config_syntax' ($validated.rc -eq 0 -and $validated.text -match 'ok \(no issues\)') $validated
        HCMove ($hcBounds[0]+45) ($hcBounds[1]+45)
        [IO.File]::WriteAllText($cfg,$hcBase,(New-Object Text.UTF8Encoding($false)))
        $hcOwned=Start-Wiri (Join-Path $hcDir 'stderr.log') (Join-Path $hcDir 'stdout.log')
        $null=$hcOwned.Handle
        Start-Sleep -Seconds 3
        $hcForm=[TestForms]::Open('lane-v-hot-corner',150,130,180)
        Start-Sleep -Milliseconds 900
        FocusForm $hcForm
        if (-not (WaitForeground $hcForm 100 3000)) { throw 'Owned H64 probe is not foreground' }
        $near=@()
        foreach ($offset in @(@(1,0),@(0,1),@(1,1))) {
            HCMove ($hcBounds[0]+$offset[0]) ($hcBounds[1]+$offset[1])
            $near += (HCHost) -eq [IntPtr]::Zero
        }
        HCGate 'h64_near_corner_does_not_open' (-not ($near -contains $false)) $near
        HCMove $hcBounds[0] $hcBounds[1] 750
        $opened=HCHost
        HCGate 'h64_default_corner_opens' ($opened -ne [IntPtr]::Zero) ([int64]$opened)
        Start-Sleep -Milliseconds 700
        HCGate 'h64_parked_does_not_toggle' ((HCHost) -eq $opened -and $opened -ne [IntPtr]::Zero) ([int64](HCHost))
        HCAction 'close-overview'; Start-Sleep -Milliseconds 700
        HCGate 'h64_ipc_close_while_parked' ((HCHost) -eq [IntPtr]::Zero) ([int64](HCHost))
        HCMove ($hcBounds[0]+45) ($hcBounds[1]+45)
        HCMove $hcBounds[0] $hcBounds[1] 750
        HCGate 'h64_leave_reenter_opens' ((HCHost) -ne [IntPtr]::Zero) ([int64](HCHost))
        HCMove ($hcBounds[0]+45) ($hcBounds[1]+45)
        HCMove $hcBounds[0] $hcBounds[1] 750
        HCGate 'h64_fresh_entry_toggles_closed' ((HCHost) -eq [IntPtr]::Zero) ([int64](HCHost))
        HCReload 'gestures { hot-corners { off; }; }'
        HCMove $hcBounds[0] $hcBounds[1] 750
        HCGate 'h64_global_off' ((HCHost) -eq [IntPtr]::Zero) ([int64](HCHost))
        HCReload "output `"$hcOutput`" { hot-corners { off; }; }"
        HCMove $hcBounds[0] $hcBounds[1] 750
        HCGate 'h64_per_output_off' ((HCHost) -eq [IntPtr]::Zero) ([int64](HCHost))
        HCReload "gestures { hot-corners { off; }; }`r`noutput `"$hcOutput`" { hot-corners {}; }"
        HCMove $hcBounds[0] $hcBounds[1] 750
        HCGate 'h64_output_override_reenables' ((HCHost) -ne [IntPtr]::Zero) ([int64](HCHost))
        HCReload 'gestures { hot-corners { top-right; }; }'
        HCMove $hcBounds[0] $hcBounds[1] 750
        $leftClosed=(HCHost) -eq [IntPtr]::Zero
        HCMove ($hcBounds[2]-1) $hcBounds[1] 750
        HCGate 'h64_named_corners_replace_default' ($leftClosed -and (HCHost) -ne [IntPtr]::Zero) ([int64](HCHost))
        Shot 'lane-v-hot-corner'
        HCMove ($hcBounds[0]+45) ($hcBounds[1]+45)
        HCAction 'close-overview'; Start-Sleep -Milliseconds 250
        FocusForm $hcForm
        if (WaitForeground $hcForm 100 3000) { $null=CloseFocused $hcForm 'h64-close'; Start-Sleep -Milliseconds 500 }
    } catch { $hcChecks['error']=$_ | Out-String }
    finally {
        if ($null -ne $hcOwned -and -not $hcOwned.HasExited) {
            try {
                $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$hcOwned.Id
                try { $wire.Send('{"type":"quit"}'+"`n") } finally { $wire.Dispose() }
            } catch { $hcChecks['quitError']=[string]$_ }
            if (-not $hcOwned.WaitForExit(3500)) { $hcOwned.Kill(); $null=$hcOwned.WaitForExit(5000) }
        }
        if ($hcForm -ne [IntPtr]::Zero -and [LT]::IsWindow($hcForm)) { [TestForms]::CloseForm($hcForm) }
        [IO.File]::WriteAllBytes($cfg,$hcSaved)
        [Windows.Forms.Cursor]::Position=$hcCursor
        HCGate 'h64_owned_cleanup' ($null -ne $hcOwned -and $hcOwned.HasExited -and (HCHost) -eq [IntPtr]::Zero -and -not [LT]::IsWindow($hcForm)) $null
        $hcChecks | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $hcDir 'checks.json')
    }
}
