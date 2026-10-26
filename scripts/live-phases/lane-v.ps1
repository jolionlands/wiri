# Lane V / H62-H63. Runs under the parent's desktop lock, after H19 and Lane H.
# Only owned probe HWNDs and the retained daemon handle are used. Mouse checks
# post complete pointer pairs to that daemon's host (not a foreign desktop app).
& {
    $vDir = Join-Path $D 'lane-v'
    $null = New-Item -ItemType Directory -Path $vDir -Force
    $vDetails = [ordered]@{}
    $vOwned = $null; $vForms = @()
    $vSavedConfig = [IO.File]::ReadAllText($cfg)
    $vGates = @('h62_configured_zoom_and_backdrop','h62_source_sizes_preserved',
        'h62_reversible_open_close','h62_config_reload_updates_camera',
        'h63_click_window_focuses_and_closes','h63_background_click_closes',
        'h63_wheel_changes_workspace_without_close','h63_right_drag_scrolls_view',
        'h63_drag_to_existing_workspace','h63_drag_inserts_above',
        'h63_drag_inserts_between','h63_drag_inserts_below',
        'h63_drag_dwell_changes_workspace','h63_normal_binds_keep_working',
        'h63_closed_source_is_unregistered','h63_host_is_destroyed_on_quit')
    foreach ($vGate in $vGates) { $summary[$vGate] = $false }
    function VGate([string]$name,[bool]$pass,$observed) {
        $summary[$name] = $pass
        $vDetails[$name] = [ordered]@{pass=$pass; observed=$observed}
    }
    function VHost {
        $h = H19Host
        if ($h -ne [IntPtr]::Zero -and ($null -eq $vOwned -or [LT]::Pid($h) -ne $vOwned.Id)) { throw 'Refused unowned overview host' }
        return $h
    }
    function VProp([string]$suffix) { return (H19Prop (VHost) $suffix) }
    function VWait([bool]$open) {
        for ($vTry=0; $vTry -lt 50; $vTry++) {
            $h=VHost
            if (($h -ne [IntPtr]::Zero) -eq $open) { return $h }
            Start-Sleep -Milliseconds 100
        }
        return (VHost)
    }
    function VAction([string]$name,[string[]]$words=@()) {
        $null=Ctl ('lane-v-'+$name) (@('action',$name)+$words)
    }
    function VOpen {
        if ((VHost) -eq [IntPtr]::Zero) { VAction 'open-overview' }
        if ((VWait $true) -eq [IntPtr]::Zero) { throw 'Overview did not open' }
        Start-Sleep -Milliseconds 850
    }
    function VClose { VAction 'close-overview'; $null=VWait $false }
    function VWindows([string]$tag) {
        $text=Ctl ('lane-v-'+$tag) @('--json','windows')
        return @(($text | ConvertFrom-Json).result)
    }
    function VWindow($list,$h) { return @($list | Where-Object { [int64]$_.id -eq [int64]$h })[0] }
    function VFocus($h) {
        $null=Ctl 'lane-v-focus' @('focus-window',[string][int64]$h)
        Start-Sleep -Milliseconds 450
    }
    function VPoint([uint32]$message,[int]$x,[int]$y,[int64]$buttons=0) {
        $h=VHost; if ($h -eq [IntPtr]::Zero) { throw 'Pointer check has no owned host' }
        $r=Rc $h; $localX=$x-$r.L; $localY=$y-$r.T
        $packed=[IntPtr](($localY -shl 16) -bor ($localX -band 0xffff))
        if (-not [H19Native]::PostMessage($h,$message,[IntPtr]$buttons,$packed)) { throw 'Pointer message failed' }
        Start-Sleep -Milliseconds 80
    }
    function VDrag([int]$x,[int]$y,[int]$hold=0) {
        $r=H19Rect (VHost)
        $sx=[int]($r[0]+$r[2]/2); $sy=[int]($r[1]+$r[3]/2)
        VPoint 0x201 $sx $sy 1
        VPoint 0x200 $x $y 1
        if ($hold -gt 0) { Start-Sleep -Milliseconds $hold }
        VPoint 0x202 $x $y
        Start-Sleep -Milliseconds 500
    }
    function VStop {
        if ($null -eq $vOwned -or $vOwned.HasExited) { return }
        try {
            $wire=New-Object LaneHConnection ('\\.\pipe\'+(WiriPipeName)),$vOwned.Id
            try { $wire.Send('{"type":"quit"}'+"`n") } finally { $wire.Dispose() }
        } catch { $vDetails['stop-error']=[string]$_ }
        if (-not $vOwned.WaitForExit(3500)) { $vOwned.Kill(); $null=$vOwned.WaitForExit(5000) }
    }
    function VStart([string]$text,[string]$tag) {
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count) { throw 'Lane V refuses an existing daemon' }
        [IO.File]::WriteAllText($cfg,$text,(New-Object Text.UTF8Encoding($false)))
        $p=Start-Wiri (Join-Path $vDir ($tag+'.stderr.log')) (Join-Path $vDir ($tag+'.stdout.log'))
        $null=$p.Handle
        return $p
    }
    $vCameraConfig=@'
input { mod-key "ctrl-alt"; }
overview { zoom 0.35; backdrop-color "#123456"; workspace-shadow { off; }; }
binds { Mod+Q { close-window; }; Mod+L { focus-column-right; }; }
'@
    try {
        $vOwned=VStart $vCameraConfig 'camera'
        Start-Sleep -Seconds 3
        foreach ($i in 1..3) { $vForms += [TestForms]::Open("lane-v-$i",(60*$i),90,150); Start-Sleep -Milliseconds 950 }
        FocusForm $vForms[0]
        if (-not (WaitForeground $vForms[0] 100 3000)) { throw 'Owned camera probe is not foreground' }
        $before=Rc $vForms[0]
        VOpen
        $during=Rc $vForms[0]
        VGate 'h62_configured_zoom_and_backdrop' ((VProp 'ZoomMillionths') -eq 350000 -and (VProp 'BackdropRgb') -eq 0x123456) @(VProp 'ZoomMillionths'; VProp 'BackdropRgb')
        VGate 'h62_source_sizes_preserved' ($before.W -eq $during.W -and $before.H -eq $during.H) @($before,$during)
        # Explicit open reverses a pending close on the same session host.
        $oldHost=VHost
        VAction 'close-overview'; Start-Sleep -Milliseconds 70; VAction 'open-overview'
        Start-Sleep -Milliseconds 900
        VGate 'h62_reversible_open_close' ((VHost) -eq $oldHost -and (VProp 'ZoomMillionths') -eq 350000) (VProp 'ZoomMillionths')
        [IO.File]::WriteAllText($cfg,$vCameraConfig.Replace('zoom 0.35','zoom 0.4').Replace('#123456','#654321'),(New-Object Text.UTF8Encoding($false)))
        $null=Ctl 'lane-v-reload' @('reload-config'); Start-Sleep -Milliseconds 1100
        VGate 'h62_config_reload_updates_camera' ((VProp 'ZoomMillionths') -eq 400000 -and (VProp 'BackdropRgb') -eq 0x654321) @(VProp 'ZoomMillionths'; VProp 'BackdropRgb')
        VClose
        VStop; $vOwned=$null
        $vInteractionConfig=$vCameraConfig.Replace('overview {','animations { off; }'+"`noverview {")+@'

workspace "v-first"
workspace "v-second"
workspace "v-third"
'@
        $vOwned=VStart $vInteractionConfig 'pointer'
        Start-Sleep -Seconds 3
        FocusForm $vForms[0]; VOpen
        $r=H19Rect (VHost); $selected=VProp 'FocusedWindow'
        VPoint 0x201 ([int]($r[0]+$r[2]/2)) ([int]($r[1]+$r[3]/2)) 1
        VPoint 0x202 ([int]($r[0]+$r[2]/2)) ([int]($r[1]+$r[3]/2))
        VGate 'h63_click_window_focuses_and_closes' ((VWait $false) -eq [IntPtr]::Zero -and (WaitForeground ([IntPtr]$selected) 100 3000)) $selected
        VOpen; $h=VHost; $bounds=Rc $h
        VPoint 0x201 ($bounds.L+5) ($bounds.T+($bounds.H/2)) 1
        VPoint 0x202 ($bounds.L+5) ($bounds.T+($bounds.H/2))
        VGate 'h63_background_click_closes' ((VWait $false) -eq [IntPtr]::Zero) $null
        VOpen
        $active=VProp 'ActiveWorkspace'; $h=VHost
        $null=[H19Native]::PostMessage($h,0x20a,[IntPtr](-120 -shl 16),[IntPtr]::Zero)
        Start-Sleep -Milliseconds 500
        VGate 'h63_wheel_changes_workspace_without_close' ((VHost) -ne [IntPtr]::Zero -and (VProp 'ActiveWorkspace') -ne $active) @(VProp 'ActiveWorkspace')
        VFocus $vForms[0]
        $r=H19Rect (VHost); $oldX=$r[0]; $x=[int]($r[0]+$r[2]/2); $y=[int]($r[1]+$r[3]/2)
        VPoint 0x204 $x $y 2; VPoint 0x200 ($x-45) $y 2; VPoint 0x205 ($x-45) $y
        Start-Sleep -Milliseconds 450
        VGate 'h63_right_drag_scrolls_view' ((VHost) -ne [IntPtr]::Zero -and (H19Rect (VHost))[0] -ne $oldX) @($oldX,(H19Rect (VHost))[0])
        # The next workspace is partially visible at this zoom. Dwell changes the camera;
        # release then commits the move without closing the overview.
        VFocus $vForms[0]; $beforeMove=VWindow (VWindows 'before-drag') $vForms[0]
        $targetId=VProp 'Workspace1Id'; $tx=[int]((VProp 'Workspace1X')+(VProp 'Workspace1W')/2)
        $ty=[int]((VProp 'Workspace1Y')+25)
        $r=H19Rect (VHost); $sx=[int]($r[0]+$r[2]/2); $sy=[int]($r[1]+$r[3]/2)
        VPoint 0x201 $sx $sy 1; VPoint 0x200 $tx $ty 1; Start-Sleep -Milliseconds 600
        VGate 'h63_drag_dwell_changes_workspace' ((VProp 'ActiveWorkspace') -eq $targetId) @(VProp 'ActiveWorkspace'; $targetId)
        # The dwell recentered the target workspace; drop in its current centre.
        $ty=[int]((VProp 'Workspace1Y')+50)
        VPoint 0x200 $tx $ty 1; VPoint 0x202 $tx $ty; Start-Sleep -Milliseconds 500
        $afterMove=VWindow (VWindows 'after-drag') $vForms[0]
        VGate 'h63_drag_to_existing_workspace' ((VHost) -ne [IntPtr]::Zero -and $afterMove.workspace_id -ne $beforeMove.workspace_id) @($beforeMove,$afterMove)
        # Each source is a configured workspace, so it survives becoming empty.
        $sourceIndex=0
        foreach ($region in @('above','between','below')) {
            $source=$vForms[$sourceIndex]; $sourceIndex++
            VFocus $source
            $countBefore=VProp 'WorkspaceCount'
            $windowBefore=VWindow (VWindows ('before-'+$region)) $source
            $dropX=[int]((VProp 'Workspace0X')+(VProp 'Workspace0W')/2)
            if ($region -eq 'below') {
                # Start on the visible source, then wheel/keyboard-scroll the camera
                # while the pointer is captured; never synthesize an offscreen press.
                $r=H19Rect (VHost); $sx=[int]($r[0]+$r[2]/2); $sy=[int]($r[1]+$r[3]/2)
                VPoint 0x201 $sx $sy 1
                VAction 'focus-workspace' @([string]$countBefore); Start-Sleep -Milliseconds 400
                $last=$countBefore-1
                $expectedTarget=(VProp ("Workspace${last}Id"))+1
                $dropY=[int]((VProp ("Workspace${last}Y"))+(VProp ("Workspace${last}H"))+8)
                VPoint 0x200 $dropX $dropY 1; VPoint 0x202 $dropX $dropY
                Start-Sleep -Milliseconds 500
            } else {
                $r=H19Rect (VHost); $sx=[int]($r[0]+$r[2]/2); $sy=[int]($r[1]+$r[3]/2)
                VPoint 0x201 $sx $sy 1
                VAction 'focus-workspace' @('1'); Start-Sleep -Milliseconds 400
                if ($region -eq 'above') { $expectedTarget=VProp 'Workspace0Id'; $dropY=[int]((VProp 'Workspace0Y')-8) }
                else { $expectedTarget=VProp 'Workspace1Id'; $dropY=[int]((VProp 'Workspace0Y')+(VProp 'Workspace0H')+8) }
                VPoint 0x200 $dropX $dropY 1; VPoint 0x202 $dropX $dropY
                Start-Sleep -Milliseconds 500
            }
            $windowAfter=VWindow (VWindows ('after-'+$region)) $source
            $grew=if ($region -eq 'below') { (VProp 'WorkspaceCount') -ge $countBefore } else { (VProp 'WorkspaceCount') -gt $countBefore }
            VGate ('h63_drag_inserts_'+$region) ((VHost) -ne [IntPtr]::Zero -and $grew -and (VProp 'ActiveWorkspace') -eq $expectedTarget -and ([uint64]$windowAfter.workspace_id -band 4294967295) -eq $expectedTarget) @($countBefore,(VProp 'WorkspaceCount'),$dropY,$expectedTarget,$windowBefore,$windowAfter)
        }
        VClose; FocusForm $vForms[2]; VOpen
        $beforeCount=VProp 'ThumbnailCount'
        $sent=CloseFocused $vForms[2] 'lane-v-close-bind'; Start-Sleep -Milliseconds 750
        VGate 'h63_normal_binds_keep_working' ($sent -and -not [LT]::IsWindow($vForms[2]) -and (VHost) -ne [IntPtr]::Zero) $sent
        VGate 'h63_closed_source_is_unregistered' ((VProp 'ThumbnailCount') -eq ($beforeCount-1)) @($beforeCount,(VProp 'ThumbnailCount'))
        Shot 'lane-v-final-overview'
    } catch { $vDetails['error']=[string]$_; $_ | Out-String | Set-Content (Join-Path $vDir 'error.txt') }
    finally {
        VStop
        VGate 'h63_host_is_destroyed_on_quit' ((VHost) -eq [IntPtr]::Zero) $null
        foreach ($h in $vForms) { if ([LT]::IsWindow($h)) { [TestForms]::CloseForm($h) } }
        [IO.File]::WriteAllText($cfg,$vSavedConfig,(New-Object Text.UTF8Encoding($false)))
        $vDetails | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $vDir 'checks.json')
    }
}
