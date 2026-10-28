# Run scripts/live-test.ps1 while recording the desktop, so every live run leaves a video next to
# its screenshots. Light on purpose (6 fps, 960 px wide, x264 ultrafast) so the live gates are not
# disturbed. Fragmented MP4 survives the hard stop at the end. Without ffmpeg the test still runs.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\record-live.ps1 [-OutDir C:\wiri-dev\live-rec]
param(
    [string]$Harness = (Join-Path $PSScriptRoot 'live-test.ps1'),
    [string]$OutDir = 'C:\wiri-dev\live-rec',
    [string]$Label = ''
)

New-Item -ItemType Directory -Force $OutDir | Out-Null
if (-not $Label) {
    $Label = 'unknown'
    try { $Label = (& git -C (Split-Path $PSScriptRoot -Parent) rev-parse --short HEAD 2>$null) } catch {}
}
$file = Join-Path $OutDir ("{0}-{1}.mp4" -f (Get-Date -Format 'yyyyMMdd-HHmmss'), $Label)

$ff = (Get-Command ffmpeg.exe -ErrorAction SilentlyContinue).Source
if (-not $ff) {
    $ff = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages') -Recurse -Filter ffmpeg.exe -ErrorAction SilentlyContinue |
        Select-Object -First 1 -ExpandProperty FullName
}

$rec = $null
if ($ff) {
    $argv = @('-hide_banner', '-loglevel', 'error', '-y', '-f', 'gdigrab', '-framerate', '6', '-draw_mouse', '1',
              '-i', 'desktop', '-vf', 'scale=960:-2', '-c:v', 'libx264', '-preset', 'ultrafast', '-crf', '30',
              '-pix_fmt', 'yuv420p', '-movflags', 'frag_keyframe+empty_moov', '-t', '900', $file)
    $rec = Start-Process -FilePath $ff -ArgumentList $argv -WindowStyle Hidden -PassThru `
        -RedirectStandardError (Join-Path $OutDir 'ffmpeg.log')
    Start-Sleep -Seconds 1
} else {
    Write-Output 'ffmpeg not found; running the live test without a recording'
}

try {
    & powershell -NoProfile -ExecutionPolicy Bypass -File $Harness
} finally {
    if ($rec -and -not $rec.HasExited) { Stop-Process -Id $rec.Id -Force }
    if ($rec) { Set-Content -Path (Join-Path $OutDir 'last.txt') -Value $file }
}
