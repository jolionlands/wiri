[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [Parameter(Mandatory = $true)][string]$Out,
    [Parameter(Mandatory = $true)][string]$Tag,
    [Parameter(Mandatory = $true)][string]$Target,
    [string]$SourceRoot = (Join-Path $PSScriptRoot '..\..')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# Keep this policy in sync with release.yml's pre-checkout validation.
if ($Tag -cnotmatch '\Av[A-Za-z0-9._+-]{0,127}\z' -or
    $Tag.Contains('..') -or $Tag.EndsWith('.') -or
    $Tag.EndsWith('.lock', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Tag must be a filename-safe v* tag (1-128 ASCII characters, no ref metacharacters).'
}
$machines = @{
    'x86_64-pc-windows-msvc' = 0x8664
    'aarch64-pc-windows-msvc' = 0xAA64
}
if ($Target -cnotin @($machines.Keys)) { throw "Unsupported target: $Target" }

function Assert-PeMachine([string]$Path, [int]$Machine) {
    $file = Get-Item -LiteralPath $Path
    if ($file.PSIsContainer -or $file.Length -lt 64) { throw "Invalid PE file: $Path" }
    $reader = [IO.BinaryReader]::new([IO.File]::OpenRead($file.FullName))
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Invalid PE DOS signature: $Path" }
        $reader.BaseStream.Position = 0x3c
        $offset = $reader.ReadInt32()
        if ($offset -lt 64 -or $offset -gt ($reader.BaseStream.Length - 24)) {
            throw "Invalid PE header offset: $Path"
        }
        $reader.BaseStream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x00004550) { throw "Invalid PE signature: $Path" }
        if ($reader.ReadUInt16() -ne $Machine) { throw "PE machine does not match ${Target}: $Path" }
    } finally {
        $reader.Dispose()
    }
}

$Bin = (Get-Item -LiteralPath $Bin).FullName
$SourceRoot = (Get-Item -LiteralPath $SourceRoot).FullName
$files = [ordered]@{
    'wiri.exe' = (Join-Path $Bin 'wiri.exe')
    'wiri-ctl.exe' = (Join-Path $Bin 'wiri-ctl.exe')
    'README.md' = (Join-Path $SourceRoot 'README.md')
    'default_config.kdl' = (Join-Path $SourceRoot 'resources\default_config.kdl')
}
foreach ($name in @('wiri.exe', 'wiri-ctl.exe')) { Assert-PeMachine $files[$name] $machines[$Target] }
foreach ($name in @('README.md', 'default_config.kdl')) {
    $file = Get-Item -LiteralPath $files[$name]
    if ($file.PSIsContainer -or $file.Length -eq 0) { throw "Required nonempty file: $name" }
}
$license = Join-Path $SourceRoot 'LICENSE'
if (Test-Path -LiteralPath $license -PathType Leaf) { $files['LICENSE'] = $license }

$Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
$zipName = "wiri-${Tag}-${Target}.zip"
$zipPath = Join-Path $Out $zipName
$checksumPath = "$zipPath.sha256"
# Only these two named output files may be replaced; never clear an existing Out directory.
foreach ($path in @($zipPath, $checksumPath)) {
    if ((Test-Path -LiteralPath $path) -and -not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Output is not a file: $path"
    }
}
New-Item -ItemType Directory -Path $Out -Force | Out-Null
$stage = Join-Path $Out ('.package-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
    $stagedZip = Join-Path $stage 'package.zip'
    $archive = [IO.Compression.ZipFile]::Open($stagedZip, [IO.Compression.ZipArchiveMode]::Create)
    try {
        # Fixed entry order, timestamps and attributes make repeated packaging reproducible.
        foreach ($file in $files.GetEnumerator()) {
            $entry = $archive.CreateEntry($file.Key, [IO.Compression.CompressionLevel]::Optimal)
            $entry.LastWriteTime = [DateTimeOffset]::new(2000, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
            $entry.ExternalAttributes = 0
            $inputStream = [IO.File]::OpenRead($file.Value)
            try {
                $outputStream = $entry.Open()
                try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose() }
            } finally { $inputStream.Dispose() }
        }
    } finally {
        $archive.Dispose()
    }
    $hash = (Get-FileHash -LiteralPath $stagedZip -Algorithm SHA256).Hash.ToLowerInvariant()
    $stagedChecksum = "$stagedZip.sha256"
    [IO.File]::WriteAllText($stagedChecksum, "$hash  $zipName`n", [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $stagedZip -Destination $zipPath -Force
    Move-Item -LiteralPath $stagedChecksum -Destination $checksumPath -Force
} finally {
    # This invocation owns only its uniquely created staging directory.
    Remove-Item -LiteralPath $stage -Recurse -Force
}
[pscustomobject]@{ Zip = $zipPath; Sha256 = $checksumPath }
