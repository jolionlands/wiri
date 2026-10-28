[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$helper = Join-Path $PSScriptRoot 'package-release.ps1'
$root = Join-Path $repo ('.scratch\package-tests-' + [Guid]::NewGuid().ToString('N'))

function Assert([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-Throws([scriptblock]$Action, [string]$Pattern) {
    try { & $Action | Out-Null } catch {
        if ($_.Exception.Message -notmatch $Pattern) { throw }
        return
    }
    throw "Expected failure matching: $Pattern"
}
function New-PeFixture([string]$Path, [uint16]$Machine) {
    # DOS header + PE signature + COFF header; no executable code or Rust required.
    $bytes = [byte[]]::new(0x98)
    $bytes[0] = 0x4d; $bytes[1] = 0x5a
    [BitConverter]::GetBytes([int]0x80).CopyTo($bytes, 0x3c)
    $bytes[0x80] = 0x50; $bytes[0x81] = 0x45
    [BitConverter]::GetBytes($Machine).CopyTo($bytes, 0x84)
    [IO.File]::WriteAllBytes($Path, $bytes)
}

New-Item -ItemType Directory -Path (Join-Path $repo '.scratch') -Force | Out-Null
New-Item -ItemType Directory -Path $root | Out-Null
try {
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $source = Join-Path $root 'source [fixture]'
    New-Item -ItemType Directory -Path (Join-Path $source 'resources') -Force | Out-Null
    [IO.File]::WriteAllText((Join-Path $source 'README.md'), "Fixture README`n")
    [IO.File]::WriteAllText((Join-Path $source 'resources\default_config.kdl'), "fixture-config {} `n")
    $out = Join-Path $root 'out [keep]'
    New-Item -ItemType Directory -Path $out | Out-Null
    $keep = Join-Path $out 'unrelated.txt'
    [IO.File]::WriteAllText($keep, 'keep me')
    $tag = 'v1.2.3-rc.1+fixture'
    $machines = [ordered]@{
        'x86_64-pc-windows-msvc' = 0x8664
        'aarch64-pc-windows-msvc' = 0xAA64
    }
    foreach ($target in $machines.Keys) {
        $bin = Join-Path $root $target
        New-Item -ItemType Directory -Path $bin | Out-Null
        foreach ($name in @('wiri.exe', 'wiri-ctl.exe')) { New-PeFixture (Join-Path $bin $name) $machines[$target] }
        # Exercise both the optional-license and no-license layouts.
        if ($target -eq 'aarch64-pc-windows-msvc') {
            [IO.File]::WriteAllText((Join-Path $source 'LICENSE'), 'Fixture license')
        }
        $packageArgs = @{ Bin = $bin; Out = $out; Tag = $tag; Target = $target; SourceRoot = $source }
        $result = & $helper @packageArgs
        $zipName = "wiri-${tag}-${target}.zip"
        Assert ($result.Zip -ceq (Join-Path $out $zipName)) 'Wrong ZIP filename'
        Assert ($result.Sha256 -ceq "$($result.Zip).sha256") 'Wrong checksum filename'
        $expected = @{
            'wiri.exe' = (Join-Path $bin 'wiri.exe')
            'wiri-ctl.exe' = (Join-Path $bin 'wiri-ctl.exe')
            'README.md' = (Join-Path $source 'README.md')
            'default_config.kdl' = (Join-Path $source 'resources\default_config.kdl')
        }
        if (Test-Path -LiteralPath (Join-Path $source 'LICENSE')) { $expected['LICENSE'] = Join-Path $source 'LICENSE' }
        $archive = [IO.Compression.ZipFile]::OpenRead($result.Zip)
        try {
            $names = @($archive.Entries | ForEach-Object { $_.FullName })
            Assert (@(Compare-Object -CaseSensitive @($expected.Keys) $names).Count -eq 0) 'Wrong ZIP contents/layout'
            Assert ($archive.Entries.Count -eq $expected.Count) 'Duplicate ZIP entries'
            foreach ($entry in $archive.Entries) {
                $stream = $entry.Open()
                $buffer = [IO.MemoryStream]::new()
                try {
                    $stream.CopyTo($buffer)
                    $actual = [Convert]::ToBase64String($buffer.ToArray())
                    $wanted = [Convert]::ToBase64String([IO.File]::ReadAllBytes($expected[$entry.FullName]))
                    Assert ($actual -ceq $wanted) "Wrong bytes for $($entry.FullName)"
                    Assert ($entry.LastWriteTime.Year -eq 2000) 'Non-deterministic ZIP timestamp'
                } finally { $stream.Dispose(); $buffer.Dispose() }
            }
        } finally { $archive.Dispose() }
        $hash = (Get-FileHash -LiteralPath $result.Zip -Algorithm SHA256).Hash.ToLowerInvariant()
        $sum = [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($result.Sha256))
        Assert ($sum -ceq "$hash  $zipName`n") 'Checksum must match ZIP bytes exactly (UTF-8 without BOM, LF)'
        # Changed source mtimes must not affect the ZIP. Reruns replace only their own outputs.
        foreach ($path in $expected.Values) { (Get-Item -LiteralPath $path).LastWriteTime = [datetime]'2024-02-03' }
        $again = & $helper @packageArgs
        Assert ((Get-FileHash -LiteralPath $again.Zip -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $hash) 'ZIP is not reproducible'
        Assert ([IO.File]::ReadAllText($keep) -ceq 'keep me') 'Unrelated output was changed'
        Assert (@(Get-ChildItem -LiteralPath $out -Directory -Force).Count -eq 0) 'Staging directory leaked'
    }

    $bin = Join-Path $root 'x86_64-pc-windows-msvc'
    $packageArgs = @{ Bin = $bin; Out = $out; Tag = $tag; Target = 'x86_64-pc-windows-msvc'; SourceRoot = $source }
    foreach ($safeTag in @('v', 'v-snapshot')) {
        $valid = $packageArgs.Clone(); $valid.Tag = $safeTag
        $result = & $helper @valid
        Assert ((Split-Path -Leaf $result.Zip) -ceq "wiri-${safeTag}-x86_64-pc-windows-msvc.zip") 'Safe v* tag rejected'
    }
    foreach ($badTag in @('1.0', 'V1.0', 'v../escape', 'v1..2', 'v1/escape', 'v1\escape',
                          'v1:bad', 'v1?bad', 'v1*bad', "v1`nbad", 'v1.', 'v1.lock', ('v' + ('a' * 128)))) {
        $invalid = $packageArgs.Clone(); $invalid.Tag = $badTag
        Assert-Throws { & $helper @invalid } 'Tag must be'
    }
    foreach ($badTarget in @('i686-pc-windows-msvc', 'X86_64-pc-windows-msvc')) {
        $invalid = $packageArgs.Clone(); $invalid.Target = $badTarget
        Assert-Throws { & $helper @invalid } 'Unsupported target'
    }
    foreach ($name in @('wiri.exe', 'wiri-ctl.exe')) {
        $path = Join-Path $bin $name
        New-PeFixture $path 0xAA64
        Assert-Throws { & $helper @packageArgs } 'PE machine'
        [IO.File]::WriteAllBytes($path, [byte[]]::new(0x98))
        Assert-Throws { & $helper @packageArgs } 'PE DOS signature'
        [IO.File]::WriteAllBytes($path, [byte[]]::new(1))
        Assert-Throws { & $helper @packageArgs } 'Invalid PE file'
        New-PeFixture $path 0x8664
    }
    foreach ($name in @('README.md', 'resources\default_config.kdl')) {
        $path = Join-Path $source $name
        $bytes = [IO.File]::ReadAllBytes($path)
        [IO.File]::WriteAllBytes($path, [byte[]]::new(0))
        Assert-Throws { & $helper @packageArgs } 'Required nonempty file'
        [IO.File]::WriteAllBytes($path, $bytes)
    }
    foreach ($suffix in @('.zip', '.zip.sha256')) {
        $collision = Join-Path $root ('collision-' + $suffix.Replace('.', ''))
        $directory = Join-Path $collision "wiri-${tag}-x86_64-pc-windows-msvc$suffix"
        New-Item -ItemType Directory -Path $directory -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $directory 'keep.txt'), 'untouched')
        $invalid = $packageArgs.Clone(); $invalid.Out = $collision
        Assert-Throws { & $helper @invalid } 'Output is not a file'
        Assert ([IO.File]::ReadAllText((Join-Path $directory 'keep.txt')) -ceq 'untouched') 'Output directory was changed'
    }
    # Omitting SourceRoot must read repository docs, not the caller's working directory.
    $defaultArgs = $packageArgs.Clone(); $defaultArgs.Remove('SourceRoot')
    $defaultArgs.Out = 'default-source'
    Push-Location -LiteralPath $root
    try { $default = & $helper @defaultArgs } finally { Pop-Location }
    Assert ($default.Zip -ceq (Join-Path $root "default-source\wiri-${tag}-x86_64-pc-windows-msvc.zip")) 'Relative Out did not use caller location'
    Assert (Test-Path -LiteralPath $default.Zip -PathType Leaf) 'Default SourceRoot failed'
    Write-Host 'Package-release fixture tests passed (x64 and ARM64; no compilation).'
} finally {
    Remove-Item -LiteralPath $root -Recurse -Force
}
