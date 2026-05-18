# Rebuild and restart wiri.
#
# Usage:
#   .\scripts\dev-restart.ps1            # release build, default config
#   .\scripts\dev-restart.ps1 -Verbose   # -v on the new process
#   .\scripts\dev-restart.ps1 -Debug     # cargo build (debug, faster compile)
#
# Run from anywhere; the script anchors to its own directory.

param(
    [switch]$Verbose,
    [switch]$Debug
)

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

# Stop any running daemon — graceful first, force-kill only if needed.
$ctl = Join-Path $repoRoot 'target\release\wiri-ctl.exe'
if (Test-Path $ctl) {
    try {
        & $ctl quit 2>$null | Out-Null
        Start-Sleep -Seconds 1
    } catch { }
}
Get-Process wiri -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# Build.
$cargoArgs = if ($Debug) { @('build') } else { @('build', '--release') }
$buildLog = Join-Path $repoRoot 'dev-build.stderr.log'
$proc = Start-Process -FilePath 'cargo' -ArgumentList $cargoArgs `
    -WorkingDirectory $repoRoot -NoNewWindow -Wait `
    -RedirectStandardError $buildLog -PassThru

if ($proc.ExitCode -ne 0) {
    Write-Host "Build failed (exit $($proc.ExitCode)). Tail of $buildLog`:"
    Get-Content $buildLog -Tail 30
    exit $proc.ExitCode
}

# Launch the fresh binary in the background; logs to dev-run.log.
$binDir = if ($Debug) { 'target\debug' } else { 'target\release' }
$wiri = Join-Path $repoRoot "$binDir\wiri.exe"
$runLog = Join-Path $repoRoot 'dev-run.log'

$runArgs = @()
if ($Verbose) { $runArgs += '-v' }

$started = Start-Process -FilePath $wiri -ArgumentList $runArgs `
    -WorkingDirectory $repoRoot -NoNewWindow `
    -RedirectStandardError $runLog -PassThru

Start-Sleep -Seconds 2

if (Get-Process -Id $started.Id -ErrorAction SilentlyContinue) {
    Write-Host "wiri running as PID $($started.Id). Log: $runLog"
    Write-Host "Stop with: wiri-ctl quit"
} else {
    Write-Host "wiri exited within 2s. Tail of $runLog`:"
    Get-Content $runLog -Tail 20
    exit 1
}
