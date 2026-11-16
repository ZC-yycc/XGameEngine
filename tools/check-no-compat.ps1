# Compatibility-debt ratchet for XGameEngine.
#
# Report mode (default) prints current counts vs. the baseline and always exits 0.
# -Enforce fails when any pattern exceeds its baseline (new debt), so CI can adopt
# the rule before the existing debt is paid down.
# -WriteBaseline rewrites tools/no-compat-baseline.txt from the current tree.
#
# Usage:
#   powershell -File tools/check-no-compat.ps1
#   powershell -File tools/check-no-compat.ps1 -Enforce
#   powershell -File tools/check-no-compat.ps1 -WriteBaseline
#
# ASCII-only on purpose: Windows PowerShell 5.1 mis-decodes UTF-8 without BOM.

param(
    [switch]$Enforce,
    [switch]$WriteBaseline,
    [string]$Root = (Get-Location).Path,
    [string]$Baseline = ''
)

$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($Baseline)) {
    $Baseline = Join-Path $Root 'tools\no-compat-baseline.txt'
}

# name -> regex. Keep this list small and high-signal; false positives are
# worse than missed hits because they train people to ignore the report.
$patterns = [ordered]@{
    'v2-suffix'       = '\b[A-Za-z_][A-Za-z0-9_]*(V2|_v2)\b'
    'compat-naming'   = '\b(compat|compatibility|shim|backward_compat)\b'
    'deprecated-attr' = '#\[deprecated'
    'serde-alias'     = 'serde\([^)]*\balias\s*='
    'dead-code-allow' = 'allow\(\s*dead_code'
    'compat-module'   = '\b(mod|use)\s+(old|legacy|compat)\b'
    'migration-todo'  = 'TODO[^\r\n]*(migrat|compat)'
}

$skip = '\\(target|dist|outputs|\.git|\.workbuddy|node_modules|\.autosave)\\'

$files = Get-ChildItem -Path $Root -Recurse -File -Include *.rs -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch $skip }

if (-not $files -or @($files).Count -eq 0) {
    Write-Output 'check-no-compat: no Rust files found.'
    exit 0
}

$allowed = @{}
$hasBaseline = Test-Path $Baseline
if ($hasBaseline) {
    foreach ($line in Get-Content $Baseline) {
        if ($line -match '^\s*#' -or $line -match '^\s*$') { continue }
        $parts = $line -split '\s+'
        if ($parts.Count -ge 2) { $allowed[$parts[0]] = [int]$parts[1] }
    }
}

Write-Output ("check-no-compat: scanning {0} files under {1}" -f @($files).Count, $Root)
if ($hasBaseline) {
    Write-Output ("baseline: {0}" -f $Baseline.Replace($Root + '\', ''))
} else {
    Write-Output 'baseline: none (all patterns allowed at 0 when -Enforce is used)'
}
Write-Output ''

$current = [ordered]@{}
$total = 0

foreach ($name in $patterns.Keys) {
    $regex = $patterns[$name]
    $hits = @(Select-String -Path $files.FullName -Pattern $regex -AllMatches -CaseSensitive:$false -ErrorAction SilentlyContinue)
    $count = 0
    foreach ($hit in $hits) { $count += $hit.Matches.Count }
    $current[$name] = $count
    $total += $count
    $fileCount = @($hits | Select-Object -ExpandProperty Path -Unique).Count

    $max = 0
    if ($allowed.ContainsKey($name)) { $max = $allowed[$name] }
    $delta = $count - $max
    $marker = ''
    if ($hasBaseline) {
        if ($delta -gt 0) { $marker = "  +$delta NEW" }
        elseif ($delta -lt 0) { $marker = "  $delta" }
    }

    Write-Output ("{0,-18} {1,6} hits  {2,4} lines  {3,4} files{4}" -f $name, $count, $hits.Count, $fileCount, $marker)

    if ($count -gt 0) {
        $hits |
            Group-Object Path |
            Sort-Object Count -Descending |
            Select-Object -First 5 |
            ForEach-Object {
                $rel = $_.Name.Replace($Root + '\', '')
                Write-Output ("    {0,4}  {1}" -f $_.Count, $rel)
            }
    }
}

Write-Output ''
Write-Output ("check-no-compat: total {0} hits in {1} patterns" -f $total, $patterns.Count)

if ($WriteBaseline) {
    $lines = @(
        '# Compatibility-debt ratchet for tools/check-no-compat.ps1.',
        '# Lower these numbers as the convergence work lands. NEVER raise them.',
        '# Format: pattern<TAB>count'
    )
    foreach ($name in $current.Keys) {
        $lines += ("{0}`t{1}" -f $name, $current[$name])
    }
    Set-Content -Path $Baseline -Value $lines -Encoding ASCII
    Write-Output ("check-no-compat: baseline written to {0}" -f $Baseline.Replace($Root + '\', ''))
    exit 0
}

$newDebt = 0
foreach ($name in $current.Keys) {
    $max = 0
    if ($allowed.ContainsKey($name)) { $max = $allowed[$name] }
    if ($current[$name] -gt $max) { $newDebt += ($current[$name] - $max) }
}

if ($newDebt -eq 0 -and $total -eq 0) {
    Write-Output 'check-no-compat: clean.'
    exit 0
}

if ($newDebt -eq 0) {
    Write-Output 'check-no-compat: no new debt; existing debt is within baseline.'
    Write-Output 'Lower tools/no-compat-baseline.txt when you remove remnants.'
    exit 0
}

Write-Output ("check-no-compat: {0} new hit(s) over baseline." -f $newDebt)
Write-Output 'Remove them, or justify each as an external contract in the PR.'
Write-Output 'See Docs/架构收敛约定.md.'

if ($Enforce) {
    Write-Output 'check-no-compat: FAILED (-Enforce).'
    exit 1
}

Write-Output 'check-no-compat: report-only, exit 0. Use -Enforce to block.'
exit 0
