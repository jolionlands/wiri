# Tests the real H69 launch helper without starting a daemon or child process.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$path = Join-Path $PSScriptRoot '..\live-phases\lane-c2-session.ps1'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw $errors[0] }
$helper = $ast.Find({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq 'C2LaunchProbe' }, $true)
if ($null -eq $helper) { throw 'Missing C2LaunchProbe' }
. ([scriptblock]::Create($helper.Extent.Text))
$c2PowerShell = 'fixture-powershell'
$c2State = @{ owned = [pscustomobject]@{ Id=100 } }
$c2Isolation = New-Object 'Collections.Generic.List[object]'
$probe = [pscustomobject]@{ label='owned-fixture'; args=@('fixture'); shell='fixture' }
$script:reads = 0; $script:requests = 0
function C2SessionRequest($request) { $script:requests++; return $script:reply }
function C2ReadProbe($unused) { $script:reads++; return $script:observed }
function Reset-Fixture {
    $script:reads = 0; $script:requests = 0; $c2Isolation.Clear()
    $script:observed = [pscustomobject]@{ count=1; snapshot=[pscustomobject]@{ pid=200; parent_pid=100; label='owned-fixture' } }
    $script:reply = [pscustomobject]@{ success=$true; pid=200 }
}
function Assert-Rejected([string]$name) {
    $rejected = $false
    try { $null = C2LaunchProbe $probe $false } catch { $rejected = $true }
    if (-not $rejected) { throw "Accepted $name" }
}
Reset-Fixture
$null = C2LaunchProbe $probe $false
if ($script:requests -ne 1 -or $script:reads -ne 1) { throw 'Synchronous spawn was not observed once' }
Reset-Fixture
$script:reply = [pscustomobject]@{ success=$false; error='wiri is busy (the window layout is being updated); the request is queued and will be handled shortly' }
$result = C2LaunchProbe $probe $false
if ($script:requests -ne 1 -or $script:reads -ne 1 -or -not $result.queued -or $c2Isolation.Count -ne 1) { throw 'Queued spawn must be observed once without a resend' }
Reset-Fixture
$script:reply = [pscustomobject]@{ success=$false; error='CreateProcess failed' }
Assert-Rejected 'a genuine spawn rejection'
if ($script:reads -ne 0) { throw 'Rejected spawn waited for a probe' }
Reset-Fixture
$script:observed.count = 2
Assert-Rejected 'duplicate probes'
Reset-Fixture
$script:observed.snapshot.pid = 201
Assert-Rejected 'a mismatched synchronous PID'
Reset-Fixture
$script:reply = [pscustomobject]@{ success=$false; error='wiri is busy (the window layout is being updated); the request is queued and will be handled shortly' }
$script:observed.snapshot.parent_pid = 101
Assert-Rejected 'a queued probe with a foreign parent'
Reset-Fixture
$script:observed.snapshot.label = 'another-fixture'
Assert-Rejected 'a foreign probe label'
'Session probe fixture checks passed'
