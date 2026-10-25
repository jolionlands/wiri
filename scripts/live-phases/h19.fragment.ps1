# Dot-source after the existing $summary has been assembled, before summary.json is written.
# Deliberately not a standalone runner: uses the parent's Ctl/FocusForm/Shot/TestForms helpers.
$summary.h19_live_dwm_thumbnail = $false
$summary.h19_keyboard_focus_highlight = $false
$summary.h19_mouse_selects_and_closes = $false
$summary.h19_escape_and_toggle_close = $false
$summary.h19_zoomed_multiple_workspaces = $false
$summary.h19_source_sizes_preserved = $false
$summary.h19_ipc_responsive_with_overlay = $false
if (-not ('H19Native' -as [type])) {
    Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Drawing;
using System.Windows.Forms;
public static class H19Native {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string name);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public static void Paint(IntPtr h, int r, int g, int b) {
    Control c = Control.FromHandle(h);
    if (c != null) c.BeginInvoke((MethodInvoker)delegate { c.BackColor=Color.FromArgb(r,g,b); c.Invalidate(); c.Update(); });
  }
  public static int ColorInTile(string path, int x, int y, int w, int h, int r, int g, int b) {
    Rectangle screen=SystemInformation.VirtualScreen; int n=0;
    using (Bitmap bmp=new Bitmap(path)) {
      int left=Math.Max(0,x-screen.Left), top=Math.Max(0,y-screen.Top);
      int right=Math.Min(bmp.Width,x-screen.Left+w), bottom=Math.Min(bmp.Height,y-screen.Top+h);
      for(int yy=top; yy<bottom; yy++) for(int xx=left; xx<right; xx++) {
        Color c=bmp.GetPixel(xx,yy); if(c.R==r && c.G==g && c.B==b) n++;
      }
    }
    return n;
  }
}
'@
}
function H19Host { return [H19Native]::FindWindow('WiriThumbnailOverviewHost', $null) }
function H19Prop($h, [string]$name) { return [H19Native]::GetProp($h, ('WiriOverview' + $name)).ToInt64() }
function H19Wait([bool]$open) {
    for ($i=0; $i -lt 40; $i++) {
        $h = H19Host
        if (($h -ne [IntPtr]::Zero) -eq $open) { return $h }
        Start-Sleep -Milliseconds 100
    }
    return (H19Host)
}
function H19Toggle([string]$tag) { $null = Ctl $tag @('action','toggle-overview'); Start-Sleep -Milliseconds 600 }
function H19Key([byte]$vk) {
    [LT]::keybd_event($vk,0,0,[UIntPtr]::Zero)
    [LT]::keybd_event($vk,0,2,[UIntPtr]::Zero)
    Start-Sleep -Milliseconds 600
}
function H19Rect($h) { return @(H19Prop $h 'FocusX'; H19Prop $h 'FocusY'; H19Prop $h 'FocusW'; H19Prop $h 'FocusH') }
$h19Proc = $null; $h19A = [IntPtr]::Zero; $h19B = [IntPtr]::Zero; $h19C = [IntPtr]::Zero
try {
    $null = Ctl 'h19-preflight-quit' @('quit')
    Start-Sleep -Milliseconds 1200
    $h19Proc = Start-WiriWithConfig @'
input {
    mod-key "ctrl-alt"
}
binds {
    Mod+F10 overview-right
    Mod+F11 overview-select
}
'@ (Join-Path $D 'h19-wiri.stderr.log')
    $h19A = [TestForms]::Open('wiri-h19-red',212,46,73)
    $h19B = [TestForms]::Open('wiri-h19-blue',31,133,217)
    $h19C = [TestForms]::Open('wiri-h19-workspace-2',190,120,31)
    Start-Sleep -Milliseconds 1800
    FocusForm $h19C
    $null = Ctl 'h19-move-workspace-2' @('action','move-to-workspace','2')
    $null = Ctl 'h19-workspace-1' @('action','focus-workspace','1')
    FocusForm $h19A
    $h19Before = New-Object LT+RECT
    $null = [LT]::GetWindowRect($h19A,[ref]$h19Before)
    H19Toggle 'h19-open'
    $h19Host = H19Wait $true
    $h19Rect = H19Rect $h19Host
    $summary.h19_zoomed_multiple_workspaces = ($h19Host -ne [IntPtr]::Zero -and (H19Prop $h19Host 'WorkspaceCount') -ge 2 -and (H19Prop $h19Host 'ThumbnailCount') -ge 3 -and $h19Rect[2] -gt 0 -and $h19Rect[2] -lt ($h19Before.Right-$h19Before.Left))
    $h19During = New-Object LT+RECT
    $null = [LT]::GetWindowRect($h19A,[ref]$h19During)
    $summary.h19_source_sizes_preserved = (($h19Before.Right-$h19Before.Left) -eq ($h19During.Right-$h19During.Left) -and ($h19Before.Bottom-$h19Before.Top) -eq ($h19During.Bottom-$h19During.Top))
    Shot 'h19-live-before'
    [H19Native]::Paint($h19A,65,210,101)
    Start-Sleep -Milliseconds 1000
    Shot 'h19-live-after'
    $h19Old = [H19Native]::ColorInTile((Join-Path $D 'h19-live-before.png'),$h19Rect[0],$h19Rect[1],$h19Rect[2],$h19Rect[3],212,46,73)
    $h19New = [H19Native]::ColorInTile((Join-Path $D 'h19-live-after.png'),$h19Rect[0],$h19Rect[1],$h19Rect[2],$h19Rect[3],65,210,101)
    $summary.h19_live_dwm_thumbnail = ($h19Old -gt 30 -and $h19New -gt 30)
    $h19Watch = [Diagnostics.Stopwatch]::StartNew()
    $h19Reply = Ctl 'h19-ipc-while-open' @('--json','windows')
    $summary.h19_ipc_responsive_with_overlay = ($h19Watch.ElapsedMilliseconds -lt 4500 -and -not [string]::IsNullOrWhiteSpace($h19Reply))
    $h19Focused = H19Prop $h19Host 'FocusedWindow'
    [LT]::keybd_event(0x11,0,0,[UIntPtr]::Zero); [LT]::keybd_event(0x12,0,0,[UIntPtr]::Zero)
    H19Key 0x79
    [LT]::keybd_event(0x12,0,2,[UIntPtr]::Zero); [LT]::keybd_event(0x11,0,2,[UIntPtr]::Zero)
    $h19Next = H19Rect $h19Host
    $summary.h19_keyboard_focus_highlight = ((H19Prop $h19Host 'FocusedWindow') -ne $h19Focused -and $h19Next[2] -gt 0 -and ($h19Next[0] -ne $h19Rect[0] -or $h19Next[1] -ne $h19Rect[1]))
    Shot 'h19-keyboard-highlight'
    # WM_LBUTTONUP uses client coordinates; convert the focused tile's screen centre.
    $h19HostRect = New-Object LT+RECT
    $null = [LT]::GetWindowRect($h19Host,[ref]$h19HostRect)
    $h19Selected = H19Prop $h19Host 'FocusedWindow'
    $h19X = [int]($h19Next[0]+$h19Next[2]/2-$h19HostRect.Left)
    $h19Y = [int]($h19Next[1]+$h19Next[3]/2-$h19HostRect.Top)
    $h19Point = [IntPtr](($h19Y -shl 16) -bor ($h19X -band 0xffff))
    $null = [H19Native]::PostMessage($h19Host,0x201,[IntPtr]1,$h19Point)
    $null = [H19Native]::PostMessage($h19Host,0x202,[IntPtr]0,$h19Point)
    $h19Closed = H19Wait $false
    Start-Sleep -Milliseconds 700
    $summary.h19_mouse_selects_and_closes = ($h19Closed -eq [IntPtr]::Zero -and [LT]::GetForegroundWindow().ToInt64() -eq $h19Selected)
    H19Toggle 'h19-reopen-escape'
    H19Key 0x1b
    $h19EscapeClosed = (H19Wait $false) -eq [IntPtr]::Zero
    H19Toggle 'h19-reopen-toggle'
    $h19Reopened = (H19Wait $true) -ne [IntPtr]::Zero
    H19Toggle 'h19-toggle-close'
    $summary.h19_escape_and_toggle_close = ($h19EscapeClosed -and $h19Reopened -and (H19Wait $false) -eq [IntPtr]::Zero)
} catch {
    $_ | Out-String | Set-Content (Join-Path $D 'h19-error.txt')
} finally {
    if ($h19Proc) { Stop-WiriGracefully $h19Proc 'h19' }
    foreach ($h19Form in @($h19A,$h19B,$h19C)) {
        if ($h19Form -ne [IntPtr]::Zero) { [TestForms]::CloseForm($h19Form) }
    }
}
