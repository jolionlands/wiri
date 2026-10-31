# Standalone probe runner for scripts/live-phases/lane-synth.ps1 (H93).
#
# The lead's official cycle dot-sources that lane into scripts/live-test.ps1 and gets the
# harness helpers ($D, $cfg, $steps, $summary, Ctl, Rc, FocusForm, Start-Wiri, ...) from it.
# This runner supplies the same small surface so the *same gate file* can be exercised on the
# shared desktop in a short, owned probe: it takes Global\wiri-live-test, starts its own wiri
# from this worktree with its own config, runs only lane-synth's own fixtures, quits that wiri
# and releases the mutex in a finally block.
param(
    [string]$Out = '',
    [string]$Root = '',
    # Queue for the desktop instead of failing while another run holds it.
    [int]$LockMinutes = 10
)
$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..')) }
$bin = Join-Path $Root 'target\release'
$wiri = Join-Path $bin 'wiri.exe'
$ctl = Join-Path $bin 'wiri-ctl.exe'
if (-not (Test-Path $wiri)) { throw "No release binary: $wiri" }
if (-not $Out) { $Out = Join-Path $Root ('.live\synth-probe-' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
$D = [System.IO.Path]::GetFullPath($Out)
New-Item -ItemType Directory -Force $D | Out-Null
$testRoaming = Join-Path $D 'profile\roaming'
$testLocal = Join-Path $D 'profile\local'
New-Item -ItemType Directory -Force $testRoaming, $testLocal | Out-Null
$cfg = Join-Path $testRoaming 'wiri\config.kdl'
New-Item -ItemType Directory -Force (Split-Path $cfg) | Out-Null
$liveLock = New-Object System.Threading.Mutex($false, 'Global\wiri-live-test')
$ownsLiveLock = $false
try { $ownsLiveLock = $liveLock.WaitOne([TimeSpan]::FromMinutes($LockMinutes)) }
catch [System.Threading.AbandonedMutexException] { $ownsLiveLock = $true }
if (-not $ownsLiveLock) { throw 'The live desktop is busy; this probe refuses to run beside another run.' }
# Never touch this machine's own keyboard/pointer settings or profile.
$env:WIRI_KEYBOARD_DRY_RUN = '1'
$env:WIRI_CONFIG = ''
$env:NIRI_CONFIG = ''
$env:WIRI_PIPE = '\\.\pipe\wiri_synth_probe_' + $PID + '_' + [Guid]::NewGuid().ToString('N')
$steps = New-Object System.Collections.Generic.List[object]
$summary = [ordered]@{}
Add-Type -AssemblyName System.Drawing, System.Windows.Forms

function Ctl([string]$name, [string[]]$ctlArgs) {
    $outFile = Join-Path $D "$name.txt"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $psi = New-Object Diagnostics.ProcessStartInfo $ctl
    $psi.Arguments = $ctlArgs -join ' '
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [Text.Encoding]::UTF8
    $p = [Diagnostics.Process]::Start($psi)
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if ($p.WaitForExit(5000)) { $p.WaitForExit(); $rc = $p.ExitCode } else { try { $p.Kill() } catch { }; $rc = 'timeout' }
    $utf8 = New-Object Text.UTF8Encoding($false)
    [IO.File]::WriteAllText($outFile, $(if ($out.Wait(2000)) { $out.Result } else { '' }), $utf8)
    [IO.File]::WriteAllText("$outFile.err", $(if ($err.Wait(2000)) { $err.Result } else { '' }), $utf8)
    $steps.Add([pscustomobject]@{ step = $name; args = ($ctlArgs -join ' '); rc = $rc; ms = $sw.ElapsedMilliseconds })
    return (Get-Content $outFile -Raw -ErrorAction SilentlyContinue)
}
function LogLines([string]$path, [string]$needle) {
    if (-not (Test-Path $path)) { return @() }
    return @(Select-String -Path $path -Pattern $needle -SimpleMatch -ErrorAction SilentlyContinue)
}
function Rc($h) { $v = [LaneSynth]::Visible($h); return [pscustomobject]@{ L = $v.L; T = $v.T; R = $v.R; B = $v.B; W = ($v.R - $v.L); H = ($v.B - $v.T) } }
function Shot([string]$name) {
    $b = [System.Windows.Forms.SystemInformation]::VirtualScreen
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
    $bmp.Save((Join-Path $D "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}
function Near([double]$a, [double]$b, [double]$tol) { return ([Math]::Abs($a - $b) -le $tol) }
function WaitForeground($h, [int]$minMs, [int]$maxMs) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $minMs -or ([LaneSynth]::Foreground() -ne $h -and $sw.ElapsedMilliseconds -lt $maxMs)) {
        Start-Sleep -Milliseconds 50
    }
    return ([LaneSynth]::Foreground() -eq $h)
}
function FocusForm($h) {
    $null = Ctl 'probe-focus-window' @('focus-window', [string][int64]$h)
    if (-not (WaitForeground $h 1300 4000)) {
        $null = Ctl 'probe-focus-window-again' @('focus-window', [string][int64]$h)
        $null = WaitForeground $h 800 4000
    }
}
function Start-Wiri([string]$stderrLog, [string]$stdoutLog) {
    $savedRoaming = $env:APPDATA; $savedLocal = $env:LOCALAPPDATA
    try {
        $env:APPDATA = $testRoaming; $env:LOCALAPPDATA = $testLocal
        $p = Start-Process -FilePath $wiri -ArgumentList '-v', '--no-startup-overlay' -RedirectStandardError $stderrLog -RedirectStandardOutput $stdoutLog -PassThru
    } finally {
        $env:APPDATA = $savedRoaming; $env:LOCALAPPDATA = $savedLocal
    }
    $pipe = '\\.\pipe\' + ($env:WIRI_PIPE -replace '^\\\\\.\\pipe\\', '')
    $sw = [Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt 20000 -and -not $p.HasExited) {
        try { if ([IO.Directory]::GetFiles('\\.\pipe\') -contains $pipe) { break } } catch { break }
        Start-Sleep -Milliseconds 50
    }
    return $p
}
function Stop-WiriGracefully($proc, [string]$tag) {
    Ctl "$tag-quit" @('quit') | Out-Null
    $null = $proc.WaitForExit(10000)
    if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
    Start-Sleep -Milliseconds 800
}

$failures = New-Object 'System.Collections.Generic.List[string]'
$started = Get-Date
try {
    if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) { throw 'An existing wiri daemon is running; refusing to stop an unowned process.' }
    . (Join-Path $Root 'scripts\live-phases\lane-synth.ps1')
} catch {
    $failures.Add("runner: $($_.Exception.Message)")
} finally {
    if (@(Get-Process -Name wiri -ErrorAction SilentlyContinue).Count -ne 0) {
        # Only a daemon this probe started can still be alive after the lane's own quit.
        $failures.Add('a wiri process is still running after the lane')
    }
    $summary['probe_seconds'] = [int]((Get-Date) - $started).TotalSeconds
    $summary['steps'] = @($steps | ForEach-Object { [pscustomobject]@{ step = $_.step; rc = $_.rc; ms = $_.ms } })
    ([pscustomobject]$summary) | ConvertTo-Json -Depth 12 | Set-Content (Join-Path $D 'summary.json')
    Write-Host ('evidence: ' + $D)
    foreach ($key in $summary.Keys) {
        if ($key -eq 'steps' -or $key -eq 'probe_seconds') { continue }
        $value = $summary[$key]
        $state = if ($value -is [bool]) { if ($value) { 'PASS' } else { 'FAIL' } } else { 'info' }
        Write-Host ('{0,-64} {1,-5} {2}' -f $key, $state, $value)
        if ($value -is [bool] -and -not $value) { $failures.Add($key) }
    }
    foreach ($step in $steps) { if ([string]$step.rc -ne '0') { $failures.Add("step $($step.step) rc=$($step.rc)") } }
    $liveLock.ReleaseMutex()
    $liveLock.Dispose()
}
if ($failures.Count) { Write-Host ('PROBE FAILURES: ' + ($failures -join '; ')); exit 1 }
Write-Host 'PROBE: every gate passed'
exit 0
