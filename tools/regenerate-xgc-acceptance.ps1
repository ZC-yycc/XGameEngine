# Regenerate the showcase screenshots on the GPU that will run the cutover gate.
# Keep this file ASCII for Windows PowerShell 5.1.
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$OutDir,
    [string]$Cargo = 'cargo'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$project = Join-Path $repo 'projects/XGame'
$scene = 'Scenes/showcase.ron'
$legacyManifest = Join-Path $repo 'Cargo.toml'
$xgcManifest = Join-Path $repo 'xgc/Cargo.toml'
$destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)
$destination = $destination.TrimEnd([char[]]@('\', '/'))
if (Test-Path -LiteralPath $destination) {
    throw "Output already exists: $destination"
}
$parent = Split-Path -Parent $destination
[void][IO.Directory]::CreateDirectory($parent)
$staging = Join-Path $parent ('.xgc-acceptance-' + [guid]::NewGuid().ToString('N'))
[void][IO.Directory]::CreateDirectory($staging)

try {
    $world = Join-Path $staging 'showcase.json'
    & $Cargo run --quiet --locked --manifest-path $xgcManifest -p xgc-cli -- migrate (Join-Path $project $scene) $world
    if ($LASTEXITCODE -ne 0) { throw "Showcase migration failed (exit $LASTEXITCODE)" }

    $reportFile = Join-Path $staging 'shot-report.json'
    $fingerprint = $null
    foreach ($frame in @(1, 150, 200, 300)) {
        $shot = Join-Path $staging "frame-$frame.png"
        & $Cargo run --quiet --locked --manifest-path $legacyManifest -p xgame-editor -- --headless --frames $frame --shot-size 1280x720 --screenshot $shot --report $reportFile $project | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Legacy screenshot failed at frame $frame (exit $LASTEXITCODE)" }
        $report = Get-Content -LiteralPath $reportFile -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($report.scene_current -cne $scene -or $report.frames -ne $frame -or
            $report.screenshot.width -ne 1280 -or $report.screenshot.height -ne 720 -or
            !(Test-Path -LiteralPath $shot -PathType Leaf)) {
            throw "Legacy run did not capture $scene at frame $frame in 1280x720"
        }
        $adapter = $report.screenshot.adapter
        if ([string]::IsNullOrWhiteSpace($adapter.gpu) -or [string]::IsNullOrWhiteSpace($adapter.backend) -or
            $null -eq $adapter.driver -or $null -eq $adapter.driver_info) {
            throw "Legacy screenshot report is missing adapter metadata at frame $frame"
        }
        $current = ConvertTo-Json -InputObject $adapter -Compress
        if ($null -ne $fingerprint -and $fingerprint -cne $current) {
            throw "Legacy GPU fingerprint changed at frame $frame"
        }
        $fingerprint = $current
    }
    Remove-Item -LiteralPath $reportFile
    [IO.File]::WriteAllText((Join-Path $staging 'env.json'), $fingerprint + "`n", [Text.UTF8Encoding]::new($false))

    # Reuse the versioned trace so generating images cannot bless a simulation regression.
    Copy-Item -LiteralPath (Join-Path $repo 'xgc/acceptance/trace.txt') -Destination $staging
    & $Cargo run --quiet --locked --manifest-path $xgcManifest -p xgc-cli -- gate $world $project $staging --strict-total
    if ($LASTEXITCODE -ne 0) { throw "Strict XGC gate failed (exit $LASTEXITCODE)" }

    # Rename on the same volume. Directory.Move refuses an existing destination.
    [IO.Directory]::Move($staging, $destination)
    Write-Host "Published XGC acceptance references: $destination"
} finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}
