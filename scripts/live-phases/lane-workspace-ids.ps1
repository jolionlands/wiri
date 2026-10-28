# H86: actual IPC workspace identities and native foreground preservation.
# Dot-source under the root live-test mutex. This phase owns one private daemon,
# profile, PID-verified pipe, and three forms. It never changes display settings.
# A second physical output enables relocation checks; one output records an
# explicit unverified hardware limitation, with no skipped Boolean pass.
& {
    $ErrorActionPreference = 'Stop'
    $h86Dir = Join-Path $D ('h86-workspace-ids-' + [Guid]::NewGuid().ToString('N'))
    $null = New-Item -ItemType Directory -Force -Path $h86Dir
    $h86Utf8 = New-Object Text.UTF8Encoding($false)
    $h86Evidence = [ordered]@{
        scope = 'Real typed JSON workspace IDs and GetForegroundWindow; no synthetic outputs or hardware claims'
        gates = [ordered]@{}
    }
    $h86Gates = @(
        'h86_typed_window_id_transfer_preserves_native_focus',
        'h86_typed_column_id_transfer', 'h86_typed_workspace_id_focus',
        'h86_workspace_id_survives_rename_and_reorder', 'h86_workspace_id_survives_unname',
        'h86_unknown_id_is_handled_without_mutation_or_focus',
        'h86_malformed_id_is_rejected_without_mutation_or_focus',
        'h86_inactive_fullscreen_id_preserves_native_focus',
        'h86_inactive_height_id_preserves_native_focus', 'h86_owned_cleanup'
    )
    foreach ($gate in $h86Gates) {
        $summary[$gate] = $false
        $h86Evidence.gates[$gate] = [ordered]@{ pass = $false; status = 'not reached' }
    }
    $h86State = @{ daemon = $null; rpc = $null }
    $h86Forms = New-Object System.Collections.ArrayList
    $h86Prefix = 'lane-h86-' + $PID + '-' + [Guid]::NewGuid().ToString('N') + '-'
    $h86FormsReady = $false
    $h86SavedPipe = $env:WIRI_PIPE
    $h86SavedConfigOverride = $env:WIRI_CONFIG
    $h86SavedNiriConfigOverride = $env:NIRI_CONFIG
    $h86Pipe = '\\.\pipe\wiri_live_h86_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
    # Caller variables are shadowed within this phase, following Lane F's private
    # profile convention. Start-Wiri isolates only the daemon's environment.
    $testRoaming = Join-Path $h86Dir 'profile\roaming'
    $testLocal = Join-Path $h86Dir 'profile\local'
    $cfg = Join-Path $testRoaming 'wiri\config.kdl'
    foreach ($path in @((Split-Path $cfg), $testLocal)) {
        $null = New-Item -ItemType Directory -Force -Path $path
    }
    function H86Save {
        [IO.File]::WriteAllText((Join-Path $h86Dir 'evidence.json'),
            ($h86Evidence | ConvertTo-Json -Depth 24), $h86Utf8)
    }
    function H86Gate([string]$name, [bool]$pass, $observed) {
        $summary[$name] = $pass
        $h86Evidence.gates[$name] = [ordered]@{ pass = $pass; status = 'checked'; observed = $observed }
        H86Save
    }
    function H86Owned([IntPtr]$h) {
        if (-not $h86Forms.Contains($h) -or -not [LT]::IsWindow($h) -or [LT]::Pid($h) -ne $PID) {
            throw "H86 refuses an unowned form: $h"
        }
    }
    function H86Request([string]$tag, $request) {
        if ($null -eq $h86State.daemon -or $h86State.daemon.HasExited -or
            $env:WIRI_PIPE -ne $h86Pipe -or $null -eq $h86State.rpc) {
            throw 'H86 retained daemon/pipe ownership is no longer valid'
        }
        $json = $request | ConvertTo-Json -Depth 16 -Compress
        $h86State.rpc.Send($json + "`n")
        $line = $h86State.rpc.ReadLine(5000)
        [IO.File]::AppendAllText((Join-Path $h86Dir 'wire.ndjson'),
            (([ordered]@{ tag = $tag; request = $json; reply = $line } |
                ConvertTo-Json -Compress) + "`n"), $h86Utf8)
        if ($null -eq $line) { throw "H86 $tag reply timed out" }
        return ($line | ConvertFrom-Json)
    }
    function H86Handled($reply) {
        return $null -ne $reply.PSObject.Properties['Ok'] -and $reply.Ok -eq 'Handled'
    }
    function H86Action([string]$tag, [string]$name, $fields) {
        $action = @{}; $action[$name] = $fields
        $reply = H86Request $tag @{ Action = $action }
        if (-not (H86Handled $reply)) { throw "H86 action $tag rejected: $($reply | ConvertTo-Json -Compress)" }
        Start-Sleep -Milliseconds 650
        return $reply
    }
    function H86Snapshot([string]$tag) {
        $workspaces = H86Request ($tag + '-workspaces') 'Workspaces'
        $windows = H86Request ($tag + '-windows') 'Windows'
        if ($null -eq $workspaces.Ok.PSObject.Properties['Workspaces'] -or
            $null -eq $windows.Ok.PSObject.Properties['Windows']) { throw "H86 $tag snapshot failed" }
        return [pscustomobject]@{
            workspaces = @($workspaces.Ok.Workspaces)
            windows = @($windows.Ok.Windows)
            foreground = [int64][LT]::GetForegroundWindow()
        }
    }
    function H86Signature($snapshot) {
        return ([ordered]@{
            workspaces = @($snapshot.workspaces | Sort-Object id)
            windows = @($snapshot.windows | Sort-Object id)
            foreground = $snapshot.foreground
        } | ConvertTo-Json -Depth 22 -Compress)
    }
    function H86Workspace($snapshot, [uint64]$id) {
        $matches = @($snapshot.workspaces | Where-Object { [uint64]$_.id -eq $id })
        if ($matches.Count -ne 1) { throw "H86 expected exactly one current IPC workspace $id" }
        return $matches[0]
    }
    function H86Window($snapshot, [IntPtr]$h) {
        H86Owned $h
        $matches = @($snapshot.windows | Where-Object { [uint64]$_.id -eq [uint64][int64]$h })
        if ($matches.Count -ne 1) { throw "H86 expected one managed owned window $h" }
        return $matches[0]
    }
    function H86NativeFocus([IntPtr]$h) {
        H86Owned $h
        $null = H86Action 'focus-owned-window' 'FocusWindow' @{ id = [int64]$h }
        if (-not (WaitForeground $h 250 4000)) { throw "H86 owned form did not gain native foreground: $h" }
        Start-Sleep -Milliseconds 350
    }
    function H86Open([string]$suffix) {
        $title = $h86Prefix + $suffix
        $h = [OwnedLiveForms]::Open($title, 70, 140, 195)
        if ($h -eq [IntPtr]::Zero) { throw "H86 form creation failed: $title" }
        [void]$h86Forms.Add($h)
        H86Owned $h
        Start-Sleep -Milliseconds 1300
        return $h
    }
    function H86Rect([IntPtr]$h) {
        H86Owned $h
        $r = [LT]::Visible($h)
        return [pscustomobject]@{ L = $r.L; T = $r.T; W = $r.R - $r.L; H = $r.B - $r.T }
    }
    function H86SameRect($a, $b) {
        foreach ($key in @('L', 'T', 'W', 'H')) {
            if ([Math]::Abs($a.$key - $b.$key) -gt 4) { return $false }
        }
        return $true
    }
    try {
        if (-not $ownsLiveLock) { throw 'H86 requires the root Global\wiri-live-test mutex' }
        if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
            throw 'H86 refuses an existing wiri process'
        }
        . (Join-Path (Split-Path $PSScriptRoot -Parent) 'live-fixtures\owned-forms.ps1')
        $h86FormsReady = $true
        $h86Evidence['owned_form_prefix'] = $h86Prefix
        if (-not ('H86WorkspacePipe' -as [type])) {
            # Same persistent-read/PID-verification contract as LaneHConnection;
            # included here so focused execution does not require running Lane H.
            Add-Type -ReferencedAssemblies System.Core -TypeDefinition @'
using System;
using System.ComponentModel;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;
public sealed class H86WorkspacePipe : IDisposable {
    readonly NamedPipeClientStream pipe;
    readonly StreamReader reader;
    Task<string> pending;
    [DllImport("kernel32.dll", SetLastError=true)]
    static extern bool GetNamedPipeServerProcessId(SafePipeHandle handle, out uint pid);
    public H86WorkspacePipe(string path, int ownedPid) {
        const string prefix = @"\\.\pipe\";
        if (path == null || !path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            throw new ArgumentException("Expected local Win32 named pipe path");
        pipe = new NamedPipeClientStream(".", path.Substring(prefix.Length),
            PipeDirection.InOut, PipeOptions.Asynchronous);
        try {
            pipe.Connect(1500);
            uint pid;
            if (!GetNamedPipeServerProcessId(pipe.SafePipeHandle, out pid))
                throw new Win32Exception(Marshal.GetLastWin32Error());
            if (pid != (uint)ownedPid) throw new InvalidOperationException("Refused unowned pipe server");
            reader = new StreamReader(pipe, new UTF8Encoding(false, true), false, 4096, true);
        } catch { pipe.Dispose(); throw; }
    }
    public void Send(string text) {
        byte[] bytes = Encoding.UTF8.GetBytes(text);
        if (bytes.Length > 65536) throw new ArgumentException("H86 request is oversized");
        Task write = pipe.WriteAsync(bytes, 0, bytes.Length);
        if (!write.Wait(1500)) { Dispose(); throw new TimeoutException("H86 pipe write timed out"); }
        write.GetAwaiter().GetResult();
    }
    public string ReadLine(int milliseconds) {
        if (pending == null) pending = reader.ReadLineAsync();
        if (!pending.Wait(milliseconds)) return null;
        string line = pending.GetAwaiter().GetResult(); pending = null;
        if (line == null) throw new EndOfStreamException("H86 owned pipe closed");
        return line;
    }
    public void Dispose() { pipe.Dispose(); if (reader != null) reader.Dispose(); }
}
'@ -ErrorAction Stop
        }
        $env:WIRI_PIPE = $h86Pipe
        $env:WIRI_CONFIG = ''; $env:NIRI_CONFIG = ''
        $h86Config = @'
animations { off; }
input { focus-follows-mouse false; workspace-auto-back-and-forth false; }
layout { gaps 8; center-focused-column "never"; }
workspace "h86-source"
workspace "h86-target"
workspace "h86-spare"
'@
        $h86State.daemon = Start-WiriWithConfig $h86Config (Join-Path $h86Dir 'daemon.stderr.log') -NoDefaultBinds
        $null = $h86State.daemon.Handle
        if ($h86State.daemon.HasExited) { throw 'H86 daemon exited during startup' }
        $h86State.rpc = New-Object H86WorkspacePipe $h86Pipe, $h86State.daemon.Id
        $h86Evidence['daemon_pid'] = $h86State.daemon.Id
        $h86Evidence['pipe'] = $h86Pipe
        $null = H86Action 'setup-source' 'FocusWorkspace' @{ reference = @{ Name = 'h86-source' } }
        $h86Initial = H86Snapshot 'initial'
        $h86SourceRows = @($h86Initial.workspaces | Where-Object { $_.name -eq 'h86-source' })
        $h86TargetRows = @($h86Initial.workspaces | Where-Object { $_.name -eq 'h86-target' })
        $h86SpareRows = @($h86Initial.workspaces | Where-Object { $_.name -eq 'h86-spare' })
        if ($h86SourceRows.Count -ne 1 -or $h86TargetRows.Count -ne 1 -or $h86SpareRows.Count -ne 1) {
            throw 'H86 named workspace fixture did not materialize'
        }
        [uint64]$h86SourceId = $h86SourceRows[0].id
        [uint64]$h86TargetId = $h86TargetRows[0].id
        [uint64]$h86SpareId = $h86SpareRows[0].id
        $h86OriginalOutput = $h86TargetRows[0].output
        $h86Evidence['initial_actual_workspaces'] = $h86Initial.workspaces
        if (@(@($h86SourceId, $h86TargetId, $h86SpareId) | Select-Object -Unique).Count -ne 3) {
            throw 'H86 fixture workspace IPC IDs are not distinct'
        }
        $h86A = H86Open 'target'
        $h86C = H86Open 'stackmate'
        $h86B = H86Open 'foreground'
        H86NativeFocus $h86B
        $h86BeforeTransfer = H86Snapshot 'window-transfer-before'
        $h86GuardRect = H86Rect $h86B
        $null = H86Action 'window-transfer-id' 'MoveWindowToWorkspace' @{
            window_id = [int64]$h86A; reference = @{ Id = $h86TargetId }; focus = $false
        }
        $h86AfterTransfer = H86Snapshot 'window-transfer-after'
        H86Gate 'h86_typed_window_id_transfer_preserves_native_focus' (
            (H86Window $h86AfterTransfer $h86A).workspace_id -eq $h86TargetId -and
            $h86AfterTransfer.foreground -eq [int64]$h86B -and
            $h86BeforeTransfer.foreground -eq [int64]$h86B -and
            (H86Workspace $h86AfterTransfer $h86SourceId).is_focused -and
            (H86SameRect $h86GuardRect (H86Rect $h86B))) @{
                before = $h86BeforeTransfer; after = $h86AfterTransfer; native_guard = [int64]$h86B
            }
        H86NativeFocus $h86C
        $null = H86Action 'column-transfer-id' 'MoveColumnToWorkspace' @{
            reference = @{ Id = $h86SpareId }; focus = $false
        }
        $h86Column = H86Snapshot 'column-transfer-after'
        H86Gate 'h86_typed_column_id_transfer' (
            (H86Window $h86Column $h86C).workspace_id -eq $h86SpareId -and
            (H86Window $h86Column $h86B).workspace_id -eq $h86SourceId -and
            (H86Window $h86Column $h86A).workspace_id -eq $h86TargetId) $h86Column
        $null = H86Action 'move-stackmate-to-target' 'MoveWindowToWorkspace' @{
            window_id = [int64]$h86C; reference = @{ Id = $h86TargetId }; focus = $false
        }
        $null = H86Action 'focus-workspace-id' 'FocusWorkspace' @{ reference = @{ Id = $h86TargetId } }
        $h86Focused = H86Snapshot 'focus-id-after'
        H86Gate 'h86_typed_workspace_id_focus' (
            (H86Workspace $h86Focused $h86TargetId).is_focused -and
            (@([int64]$h86A, [int64]$h86C) -contains $h86Focused.foreground)) $h86Focused
        H86NativeFocus $h86A
        $null = H86Action 'stack-owned-targets' 'ConsumeWindowIntoColumn' @{}
        $h86Stack = H86Snapshot 'stacked'
        $h86StackA = H86Window $h86Stack $h86A
        $h86StackC = H86Window $h86Stack $h86C
        if ($h86StackA.layout.pos_in_scrolling_layout[0] -ne $h86StackC.layout.pos_in_scrolling_layout[0]) {
            throw 'H86 height fixture requires both owned targets in the same column'
        }
        $null = H86Action 'return-to-source-id' 'FocusWorkspace' @{ reference = @{ Id = $h86SourceId } }
        H86NativeFocus $h86B
        $h86BeforeMetadata = H86Snapshot 'metadata-before'
        $null = H86Action 'rename-workspace-id' 'SetWorkspaceName' @{
            name = 'h86-renamed'; workspace = @{ Id = $h86TargetId }
        }
        $null = H86Action 'reorder-workspace-id' 'MoveWorkspaceToIndex' @{
            index = 1; reference = @{ Id = $h86TargetId }
        }
        $h86Metadata = H86Snapshot 'metadata-after'
        $h86Target = H86Workspace $h86Metadata $h86TargetId
        H86Gate 'h86_workspace_id_survives_rename_and_reorder' (
            $h86Target.name -eq 'h86-renamed' -and $h86Target.idx -eq 1 -and
            (H86Workspace $h86BeforeMetadata $h86TargetId).idx -ne $h86Target.idx -and
            (H86Window $h86Metadata $h86A).workspace_id -eq $h86TargetId -and
            (H86Window $h86Metadata $h86C).workspace_id -eq $h86TargetId -and
            $h86Metadata.foreground -eq [int64]$h86B -and
            (H86Workspace $h86Metadata $h86SourceId).is_focused) @{
                before = $h86BeforeMetadata; after = $h86Metadata
            }

        # A valid but unknown u64 ID follows pinned niri's handled no-op contract.
        # Check each of the seven shapes; never substitute a visible/local index.
        [uint64]$h86UnknownId = [uint64]::MaxValue
        if (@($h86Metadata.workspaces | Where-Object { [uint64]$_.id -eq $h86UnknownId }).Count -ne 0) {
            throw 'H86 unknown-ID sentinel unexpectedly exists'
        }
        $h86UnknownBefore = H86Snapshot 'unknown-before'
        $h86UnknownSignature = H86Signature $h86UnknownBefore
        $h86UnknownChecks = New-Object System.Collections.ArrayList
        $h86UnknownPass = $true
        foreach ($case in @(
            @{ FocusWorkspace = @{ reference = @{ Id = $h86UnknownId } } },
            @{ MoveWindowToWorkspace = @{ window_id = [int64]$h86A; reference = @{ Id = $h86UnknownId }; focus = $true } },
            @{ MoveColumnToWorkspace = @{ reference = @{ Id = $h86UnknownId }; focus = $true } },
            @{ MoveWorkspaceToIndex = @{ index = 1; reference = @{ Id = $h86UnknownId } } },
            @{ SetWorkspaceName = @{ name = 'must-not-exist'; workspace = @{ Id = $h86UnknownId } } },
            @{ UnsetWorkspaceName = @{ reference = @{ Id = $h86UnknownId } } },
            @{ MoveWorkspaceToMonitor = @{ output = $h86OriginalOutput; reference = @{ Id = $h86UnknownId } } }
        )) {
            $reply = H86Request 'unknown-id' @{ Action = $case }
            Start-Sleep -Milliseconds 200
            $after = H86Snapshot 'unknown-after'
            $pass = (H86Handled $reply) -and ((H86Signature $after) -ceq $h86UnknownSignature)
            $h86UnknownPass = $h86UnknownPass -and $pass
            [void]$h86UnknownChecks.Add(@{ action = $case; reply = $reply; unchanged = $pass; after = $after })
        }
        H86Gate 'h86_unknown_id_is_handled_without_mutation_or_focus' (
            $h86UnknownPass -and $h86UnknownBefore.foreground -eq [int64]$h86B) $h86UnknownChecks
        $h86MalformedPass = $true
        $h86MalformedChecks = New-Object System.Collections.ArrayList
        foreach ($badId in @(-1, 1.5, '42')) {
            $reply = H86Request 'malformed-id' @{ Action = @{ FocusWorkspace = @{ reference = @{ Id = $badId } } } }
            $after = H86Snapshot 'malformed-after'
            $pass = ($null -ne $reply.PSObject.Properties['Err']) -and ((H86Signature $after) -ceq $h86UnknownSignature)
            $h86MalformedPass = $h86MalformedPass -and $pass
            [void]$h86MalformedChecks.Add(@{ invalid_id = $badId; reply = $reply; unchanged = $pass })
        }
        H86Gate 'h86_malformed_id_is_rejected_without_mutation_or_focus' $h86MalformedPass $h86MalformedChecks

        $h86Inactive = H86Snapshot 'inactive-target-before'
        if ((H86Workspace $h86Inactive $h86TargetId).is_active -or
            $h86Inactive.foreground -ne [int64]$h86B) { throw 'H86 inactive-target focus precondition failed' }
        $h86GuardBefore = H86Rect $h86B
        $null = H86Action 'inactive-fullscreen-id-on' 'FullscreenWindow' @{ id = [int64]$h86A }
        $h86Fullscreen = H86Snapshot 'inactive-fullscreen-on'
        $null = H86Action 'inactive-fullscreen-id-off' 'FullscreenWindow' @{ id = [int64]$h86A }
        $h86FullscreenOff = H86Snapshot 'inactive-fullscreen-off'
        H86Gate 'h86_inactive_fullscreen_id_preserves_native_focus' (
            (H86Window $h86Fullscreen $h86A).is_fullscreen -and
            -not (H86Window $h86FullscreenOff $h86A).is_fullscreen -and
            $h86Fullscreen.foreground -eq [int64]$h86B -and
            $h86FullscreenOff.foreground -eq [int64]$h86B -and
            -not (H86Workspace $h86Fullscreen $h86TargetId).is_active -and
            -not (H86Workspace $h86FullscreenOff $h86TargetId).is_active -and
            (H86SameRect $h86GuardBefore (H86Rect $h86B))) @{
                before = $h86Inactive; on = $h86Fullscreen; off = $h86FullscreenOff
            }
        # Establish a fresh, visible two-window baseline after fullscreen exits.
        # Revealing/focusing changes selection only; no height/reset operation is
        # allowed between this baseline and the one fixed-height request below.
        $null = H86Action 'reveal-post-fullscreen-baseline' 'FocusWorkspace' @{ reference = @{ Id = $h86TargetId } }
        H86NativeFocus $h86A
        $h86HeightBaseline = H86Snapshot 'post-fullscreen-height-baseline'
        $h86HeightBaseA = H86Window $h86HeightBaseline $h86A
        $h86HeightBaseC = H86Window $h86HeightBaseline $h86C
        $h86HeightBefore = H86Rect $h86A
        $h86MateBefore = H86Rect $h86C
        $h86DpiBefore = [OwnedLiveForms]::Dpi($h86A)
        if ($h86DpiBefore -eq 0 -or [OwnedLiveForms]::Dpi($h86C) -ne $h86DpiBefore) {
            throw 'H86 height fixture requires a measured common nonzero window DPI'
        }
        $h86Scale = [double]$h86DpiBefore / 96.0
        $h86LogicalTolerance = 3.0 / $h86Scale
        if ($h86HeightBaseA.is_fullscreen -or $h86HeightBaseC.is_fullscreen -or
            $h86HeightBaseA.layout.pos_in_scrolling_layout[0] -ne $h86HeightBaseC.layout.pos_in_scrolling_layout[0] -or
            $h86HeightBaseA.layout.pos_in_scrolling_layout[1] -ge $h86HeightBaseC.layout.pos_in_scrolling_layout[1] -or
            [double]$h86HeightBaseA.layout.window_size[1] -le 180.0 -or
            [Math]::Abs($h86HeightBefore.H - [double]$h86HeightBaseA.layout.window_size[1] * $h86Scale) -gt 3.0 -or
            [Math]::Abs($h86MateBefore.H - [double]$h86HeightBaseC.layout.window_size[1] * $h86Scale) -gt 3.0) {
            throw 'H86 post-fullscreen baseline is not a distinct ordered stacked-height fixture'
        }
        $null = H86Action 'return-to-inactive-height-fixture' 'FocusWorkspace' @{ reference = @{ Id = $h86SourceId } }
        H86NativeFocus $h86B
        $h86HeightPreRequest = H86Snapshot 'inactive-height-before'
        $h86GuardBefore = H86Rect $h86B
        if ((H86Workspace $h86HeightPreRequest $h86TargetId).is_active -or
            $h86HeightPreRequest.foreground -ne [int64]$h86B) {
            throw 'H86 height target must be inactive before the fixed-height request'
        }
        $null = H86Action 'inactive-height-id' 'SetWindowHeight' @{
            id = [int64]$h86A; change = @{ SetFixed = 160 }
        }
        $h86HeightInactive = H86Snapshot 'inactive-height-after'
        $h86HeightInactiveA = H86Window $h86HeightInactive $h86A
        $h86HeightKeptFocus = $h86HeightInactive.foreground -eq [int64]$h86B -and
            (H86Workspace $h86HeightInactive $h86SourceId).is_focused -and
            -not (H86Workspace $h86HeightInactive $h86TargetId).is_active -and
            (H86SameRect $h86GuardBefore (H86Rect $h86B))
        # Reveal deliberately only after recording the inactive operation's
        # foreground. This verifies its physical result without claiming a hidden
        # HWND was already laid out while its workspace was inactive.
        $null = H86Action 'reveal-height-result' 'FocusWorkspace' @{ reference = @{ Id = $h86TargetId } }
        H86NativeFocus $h86A
        $h86HeightRevealed = H86Snapshot 'height-result-revealed'
        $h86HeightResultA = H86Window $h86HeightRevealed $h86A
        $h86HeightResultC = H86Window $h86HeightRevealed $h86C
        $h86HeightAfter = H86Rect $h86A
        $h86MateAfter = H86Rect $h86C
        # Fixed 160 names the window's logical height, excluding wiri's tile
        # insets. The visible native frame includes Win32 decorations; DWM's
        # Visible rectangle excludes the invisible resize shadow. Check the
        # requested size directly, then its allocation/inset and stackmate
        # conservation relationships, rather than reproducing height weights.
        $h86ExpectedPhysicalHeight = 160.0 * $h86Scale
        $h86InsetLogical = [double]$h86HeightResultA.layout.window_offset_in_tile[1]
        $h86PairBefore = [double]$h86HeightBaseA.layout.tile_size[1] + [double]$h86HeightBaseC.layout.tile_size[1]
        $h86PairAfter = [double]$h86HeightResultA.layout.tile_size[1] + [double]$h86HeightResultC.layout.tile_size[1]
        $h86GapBefore = [double]$h86HeightBaseC.layout.tile_pos_in_workspace_view[1] -
            ([double]$h86HeightBaseA.layout.tile_pos_in_workspace_view[1] + [double]$h86HeightBaseA.layout.tile_size[1])
        $h86GapAfter = [double]$h86HeightResultC.layout.tile_pos_in_workspace_view[1] -
            ([double]$h86HeightResultA.layout.tile_pos_in_workspace_view[1] + [double]$h86HeightResultA.layout.tile_size[1])
        $h86ExactHeight = (
            [OwnedLiveForms]::Dpi($h86A) -eq $h86DpiBefore -and
            [OwnedLiveForms]::Dpi($h86C) -eq $h86DpiBefore -and
            [Math]::Abs([double]$h86HeightInactiveA.layout.window_size[1] - 160.0) -le $h86LogicalTolerance -and
            [Math]::Abs($h86HeightAfter.H - $h86ExpectedPhysicalHeight) -le 3.0 -and
            [Math]::Abs([double]$h86HeightResultA.layout.window_size[1] - 160.0) -le $h86LogicalTolerance -and
            [Math]::Abs([double]$h86HeightResultA.layout.tile_size[1] - (160.0 + 2.0 * $h86InsetLogical)) -le $h86LogicalTolerance -and
            [Math]::Abs($h86MateAfter.H - [double]$h86HeightResultC.layout.window_size[1] * $h86Scale) -le 3.0 -and
            [Math]::Abs($h86PairAfter - $h86PairBefore) -le $h86LogicalTolerance -and
            [Math]::Abs($h86GapAfter - $h86GapBefore) -le $h86LogicalTolerance -and $h86GapAfter -ge 0 -and
            $h86HeightResultA.layout.pos_in_scrolling_layout[0] -eq $h86HeightResultC.layout.pos_in_scrolling_layout[0] -and
            $h86HeightResultA.layout.pos_in_scrolling_layout[1] -lt $h86HeightResultC.layout.pos_in_scrolling_layout[1] -and
            $h86HeightAfter.H -lt ($h86HeightBefore.H - 20.0) -and
            $h86MateAfter.H -gt ($h86MateBefore.H + 20.0) -and
            [Math]::Abs(($h86HeightBefore.H - $h86HeightAfter.H) - ($h86MateAfter.H - $h86MateBefore.H)) -le 4.0)
        H86Gate 'h86_inactive_height_id_preserves_native_focus' (
            $h86HeightKeptFocus -and $h86ExactHeight) @{
                baseline_after_fullscreen_exit = $h86HeightBaseline
                inactive_before = $h86HeightPreRequest; inactive_after = $h86HeightInactive
                revealed_after = $h86HeightRevealed; dpi = $h86DpiBefore; scale = $h86Scale
                physical_before = $h86HeightBefore; stackmate_before = $h86MateBefore
                physical_after_deliberate_reveal = $h86HeightAfter; stackmate_after = $h86MateAfter
                requested_logical_height = 160; expected_physical_visible_height = $h86ExpectedPhysicalHeight
                logical_tile_inset_each_side = $h86InsetLogical
                pair_height_before = $h86PairBefore; pair_height_after = $h86PairAfter
                gap_before = $h86GapBefore; gap_after = $h86GapAfter
            }
        $null = H86Action 'restore-source-after-height' 'FocusWorkspace' @{ reference = @{ Id = $h86SourceId } }
        H86NativeFocus $h86B
        $null = H86Action 'unname-workspace-id' 'UnsetWorkspaceName' @{ reference = @{ Id = $h86TargetId } }
        $h86Unnamed = H86Snapshot 'unnamed-after'
        H86Gate 'h86_workspace_id_survives_unname' (
            $null -eq (H86Workspace $h86Unnamed $h86TargetId).name -and
            (H86Window $h86Unnamed $h86A).workspace_id -eq $h86TargetId -and
            (H86Window $h86Unnamed $h86C).workspace_id -eq $h86TargetId -and
            $h86Unnamed.foreground -eq [int64]$h86B) $h86Unnamed

        $h86OutputsReply = H86Request 'actual-outputs' 'Outputs'
        if ($null -eq $h86OutputsReply.Ok.PSObject.Properties['Outputs']) { throw 'H86 output query failed' }
        $h86OutputNames = @($h86OutputsReply.Ok.Outputs.PSObject.Properties |
            Where-Object { $null -ne $_.Value.current_mode -and $null -ne $_.Value.logical } |
            ForEach-Object { $_.Name })
        $h86OtherOutputs = @($h86OutputNames | Where-Object { $_ -ne $h86OriginalOutput })
        $h86Evidence['actual_output_names'] = $h86OutputNames
        if ($h86OtherOutputs.Count -gt 0) {
            $summary['h86_monitor_relocation_preserves_id'] = $false
            $h86Evidence.gates['h86_monitor_relocation_preserves_id'] = @{ pass = $false; status = 'not reached' }
            $h86OtherOutput = $h86OtherOutputs[0]
            $null = H86Action 'relocate-workspace-id' 'MoveWorkspaceToMonitor' @{
                output = $h86OtherOutput; reference = @{ Id = $h86TargetId }
            }
            $h86Relocated = H86Snapshot 'relocated-after'
            $null = H86Action 'return-relocated-id' 'MoveWorkspaceToMonitor' @{
                output = $h86OriginalOutput; reference = @{ Id = $h86TargetId }
            }
            $h86Returned = H86Snapshot 'relocated-returned'
            H86Gate 'h86_monitor_relocation_preserves_id' (
                (H86Workspace $h86Relocated $h86TargetId).output -eq $h86OtherOutput -and
                (H86Workspace $h86Returned $h86TargetId).output -eq $h86OriginalOutput -and
                (H86Window $h86Relocated $h86A).workspace_id -eq $h86TargetId -and
                (H86Window $h86Returned $h86C).workspace_id -eq $h86TargetId -and
                $h86Relocated.foreground -eq [int64]$h86B -and
                $h86Returned.foreground -eq [int64]$h86B) @{ moved = $h86Relocated; returned = $h86Returned }
        } else {
            $h86Evidence['monitor_relocation_info'] = [ordered]@{
                status = 'unverified: requires a second connected physical output'
                observed_output_count = $h86OutputNames.Count
                typed_action = 'MoveWorkspaceToMonitor'; counted_as_pass = $false
            }
            $summary['h86_monitor_relocation_info'] = $h86Evidence.monitor_relocation_info
        }
        $h86Evidence['stage'] = 'gates-complete'
    } catch {
        $h86Evidence['error'] = ($_ | Out-String).Trim()
    } finally {
        $h86Cleanup = New-Object System.Collections.ArrayList
        # The immutable original-title prefix owns every opening attempt,
        # including a cancelled attempt that never returned an HWND.
        try {
            foreach ($h in $h86Forms) {
                try {
                    if ([LT]::IsWindow($h) -and [LT]::Pid($h) -eq $PID) { $null = [OwnedLiveForms]::Close($h) }
                } catch { [void]$h86Cleanup.Add('Owned HWND close: ' + [string]$_) }
            }
            if ($h86FormsReady) { [OwnedLiveForms]::ClosePrefix($h86Prefix) }
            $clock = [Diagnostics.Stopwatch]::StartNew()
            do {
                $open = @($h86Forms | Where-Object { [LT]::IsWindow($_) -and [LT]::Pid($_) -eq $PID })
                $allClosed = -not $h86FormsReady
                if ($h86FormsReady) { $allClosed = [OwnedLiveForms]::AllClosedForPrefix($h86Prefix) }
                if ($open.Count -eq 0 -and $allClosed) { break }
                Start-Sleep -Milliseconds 100
            } while ($clock.ElapsedMilliseconds -lt 5000)
            $h86Evidence['opening_threads_finished'] = $allClosed
            if ($h86FormsReady) { $h86Evidence['pending_openings'] = [OwnedLiveForms]::PendingForPrefix($h86Prefix) }
            if ($open.Count -gt 0 -or -not $allClosed) {
                [void]$h86Cleanup.Add('Owned HWNDs or registered UI threads did not close within 5000 ms')
            }
        } catch { [void]$h86Cleanup.Add([string]$_) }
        if ($null -ne $h86State.rpc) { try { $h86State.rpc.Dispose() } catch { [void]$h86Cleanup.Add([string]$_) } }
        $process = $h86State.daemon
        if ($null -ne $process) {
            try {
                if (-not $process.HasExited) {
                    $quit = $null
                    try {
                        $quit = New-Object H86WorkspacePipe $h86Pipe, $process.Id
                        $quit.Send("{`"type`":`"quit`"}`n")
                    } catch { [void]$h86Cleanup.Add([string]$_) }
                    finally { if ($null -ne $quit) { $quit.Dispose() } }
                    if (-not $process.WaitForExit(3000)) {
                        [void]$h86Cleanup.Add('Retained owned daemon required forced stop')
                        $process.Kill(); $null = $process.WaitForExit(5000)
                    }
                }
                if (-not $process.HasExited) { [void]$h86Cleanup.Add('Owned daemon is still running') }
            } catch { [void]$h86Cleanup.Add([string]$_) }
            finally { $process.Dispose() }
        }
        $env:WIRI_PIPE = $h86SavedPipe
        $env:WIRI_CONFIG = $h86SavedConfigOverride
        $env:NIRI_CONFIG = $h86SavedNiriConfigOverride
        H86Gate 'h86_owned_cleanup' ($h86Cleanup.Count -eq 0) @($h86Cleanup)
        $summary['lane_workspace_ids_details'] = $h86Evidence
        H86Save
    }
}
