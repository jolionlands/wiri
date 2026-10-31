param([string]$Path)
$errors = $null
$tokens = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path $Path).Path, [ref]$tokens, [ref]$errors)
if ($errors -and $errors.Count -gt 0) {
    foreach ($item in $errors) { Write-Output ($item.Extent.StartLineNumber.ToString() + ': ' + $item.Message) }
    exit 1
}
Write-Output 'PARSE OK'
