# Collector checks only: no wiri, desktop input, window creation or artificial load.
$ErrorActionPreference = 'Stop'
foreach ($file in @((Join-Path $PSScriptRoot '..\live-phases\lane-e-perf.ps1'), (Join-Path $PSScriptRoot '..\live-test.ps1'))) {
    $tokens = $null; $errors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($file, [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
}
Add-Type -Path (Join-Path $PSScriptRoot 'Native.cs')
foreach ($case in @(
    @{ values = [double[]]@(); expected = -1 },
    @{ values = [double[]]@(3); expected = 3 },
    @{ values = [double[]]@(3, 1, 2); expected = 2 },
    @{ values = [double[]]@(4, 1, 3, 2); expected = 2.5 }
)) {
    if ([LaneE.Native]::Median($case.values) -ne $case.expected) { throw 'Median check failed' }
}
# Regression: the live daemon keeps its debug file open for writing. Read with write sharing.
$sharedPath = Join-Path $PSScriptRoot ('.self-test-' + [Guid]::NewGuid().ToString('N') + '.log')
$writer = $null
try {
    $writer = New-Object IO.FileStream($sharedPath, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, ([IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete))
    $bytes = [Text.Encoding]::UTF8.GetBytes('shared writer')
    $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
    if ([LaneE.Native]::ReadSharedText($sharedPath) -ne 'shared writer') { throw 'Shared log read failed' }
} finally {
    if ($null -ne $writer) { $writer.Dispose() }
    if (Test-Path $sharedPath) { Remove-Item $sharedPath }
}
$a = [LaneE.Native]::ReadThreads($PID)
$r = [LaneE.Native]::ReadResources($PID)
if ($a.Threads.Count -lt 1 -or $r.Threads -lt 1 -or $r.PrivateBytes -le 0 -or $r.WorkingSetBytes -le 0) { throw 'Invalid self snapshot' }
if (@($a.Threads | Where-Object { $_.Id -le 0 -or $_.Created -le 0 -or $_.CpuMs -lt 0 }).Count) { throw 'Invalid thread counters' }
# A single short sleep validates delta arithmetic; it is not a workload or a wiri baseline.
$idle = [LaneE.Native]::Idle($PID, 100)
if ($idle.Seconds -lt 0.09 -or $idle.CpuMs -lt 0 -or $idle.SwitchesPerSecond -lt 0) { throw 'Invalid counter delta' }
'PASS: PowerShell syntax, native x64 layouts/PID validation, process resources, per-thread counters, delta arithmetic, medians'
[pscustomobject]@{ threads = $a.Threads.Count; private_bytes = $r.PrivateBytes; cpu_ms = $a.CpuMs; idle_check_seconds = $idle.Seconds } | ConvertTo-Json
