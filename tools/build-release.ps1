#Requires -Version 5.1
<#
    XGameEngine - one-click release build.

    Produces a self-contained, redistributable package in .\dist :

        dist\xgame-editor.exe          the editor
        dist\xgame-game.exe            the player. The editor COPIES this file when
                                       you press "Build", so it must stay next to
                                       xgame-editor.exe.
        dist\projects\SampleScene\     the bundled sample project
        dist\README.txt                short usage note

    Usage:
        powershell -NoProfile -ExecutionPolicy Bypass -File tools\build-release.ps1
      or just double-click  一键构建.cmd  in the repository root.

    ---------------------------------------------------------------------------
    Why this script exists instead of a bare "cargo build"
    ---------------------------------------------------------------------------

    Two environment quirks make a plain command line unreliable here:

    1. cargo is not on PATH.
       rustup installs it into %USERPROFILE%\.cargo\bin but does not add that
       directory to PATH. A freshly spawned shell (which is exactly what a
       double-clicked .cmd gets) therefore has no `cargo` at all. Resolve-Cargo
       falls back to the default rustup location.

    2. Linking may fail with an incomplete Visual Studio desktop lib set.
       It is possible to have a full MSVC install whose desktop LIB folder is
       missing msvcrt.lib, while the "onecore" lib set links a working exe.
       Hard-coding machine-specific paths would break on any other machine, so
       instead we build once as-is, and ONLY if that fails do we locate an MSVC
       toolset + Windows SDK and retry with explicit LIB / INCLUDE.

    ---------------------------------------------------------------------------
    Encoding note
    ---------------------------------------------------------------------------
    Everything in this file is deliberately ASCII-only. Windows PowerShell 5.1
    reads .ps1 files using the system ANSI code page (GBK on this machine)
    unless the file carries a UTF-8 BOM, so non-ASCII comments can be
    mis-decoded into garbage. Chinese documentation lives in the *.md files.
#>

[CmdletBinding()]
param(
    # Skip copying the bundled sample project into dist\projects.
    [switch]$SkipSample
)

$ErrorActionPreference = 'Stop'

# tools\ -> repository root
$RepoRoot = Split-Path -Parent $PSScriptRoot
$DistDir  = Join-Path $RepoRoot 'dist'

# ---------------------------------------------------------------------------
# Console helpers
# ---------------------------------------------------------------------------

function Write-Step {
    param([string]$Text)
    Write-Host ''
    Write-Host "==> $Text" -ForegroundColor Cyan
}

function Write-Ok {
    param([string]$Text)
    Write-Host "    $Text" -ForegroundColor Green
}

function Write-Note {
    param([string]$Text)
    Write-Host "    $Text" -ForegroundColor DarkGray
}

function Write-Warn {
    param([string]$Text)
    Write-Host "    $Text" -ForegroundColor Yellow
}

# ---------------------------------------------------------------------------
# Toolchain discovery
# ---------------------------------------------------------------------------

function Resolve-Cargo {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }

    # rustup's default install location. Not on PATH after a clean install,
    # which is the usual reason a double-clicked build script "does nothing".
    $fallback = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path $fallback) { return $fallback }

    throw @"
cargo was not found.

Install Rust via rustup (https://rustup.rs), or add this folder to PATH:
    %USERPROFILE%\.cargo\bin
"@
}

function Find-VsInstall {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path $vswhere) {
        $found = & $vswhere -latest -products '*' -property installationPath 2>$null
        if ($found) { return ($found | Select-Object -First 1).Trim() }
    }

    # Fallback: scan the conventional install roots for anything that has a
    # VC\Tools\MSVC folder inside.
    $roots = @()
    if ($env:ProgramFiles)       { $roots += (Join-Path $env:ProgramFiles 'Microsoft Visual Studio') }
    if (${env:ProgramFiles(x86)}) { $roots += (Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio') }

    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }
        $candidate = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
            ForEach-Object { Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue } |
            Where-Object { Test-Path (Join-Path $_.FullName 'VC\Tools\MSVC') } |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($candidate) { return $candidate.FullName }
    }
    return $null
}

function Find-MsvcToolset {
    param([string]$VsRoot)
    if (-not $VsRoot) { return $null }

    $msvcRoot = Join-Path $VsRoot 'VC\Tools\MSVC'
    if (-not (Test-Path $msvcRoot)) { return $null }

    # Versions are compared as plain strings. Fine in practice: a VS install
    # carries one MSVC toolset per major version.
    $toolset = Get-ChildItem $msvcRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $toolset) { return $null }

    $include = Join-Path $toolset.FullName 'include'
    if (-not (Test-Path $include)) { return $null }

    # Prefer onecore/x64: the desktop lib set can be incomplete (missing
    # msvcrt.lib) on the same machine where onecore links fine.
    $lib = $null
    foreach ($candidate in @(
        (Join-Path $toolset.FullName 'lib\onecore\x64'),
        (Join-Path $toolset.FullName 'lib\x64')
    )) {
        if (Test-Path $candidate) { $lib = $candidate; break }
    }
    if (-not $lib) { return $null }

    return @{ Lib = $lib; Include = $include }
}

function Find-WindowsSdk {
    $roots = @()
    if (${env:ProgramFiles(x86)}) { $roots += (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10') }
    if ($env:ProgramFiles)        { $roots += (Join-Path $env:ProgramFiles 'Windows Kits\10') }

    foreach ($root in $roots) {
        if (-not (Test-Path $root)) { continue }

        $libRoot = Join-Path $root 'Lib'
        $incRoot = Join-Path $root 'Include'
        if (-not (Test-Path $libRoot) -or -not (Test-Path $incRoot)) { continue }

        $ver = Get-ChildItem $libRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending | Select-Object -First 1
        if (-not $ver) { continue }

        $libs = @(
            (Join-Path $ver.FullName 'ucrt\x64'),
            (Join-Path $ver.FullName 'um\x64')
        ) | Where-Object { Test-Path $_ }

        $incs = @(
            (Join-Path $incRoot "$($ver.Name)\ucrt"),
            (Join-Path $incRoot "$($ver.Name)\um"),
            (Join-Path $incRoot "$($ver.Name)\shared")
        ) | Where-Object { Test-Path $_ }

        if ($libs.Count -gt 0 -and $incs.Count -gt 0) {
            return @{ Lib = $libs; Include = $incs }
        }
    }
    return $null
}

function Enable-MsvcEnv {
    $toolset = Find-MsvcToolset -VsRoot (Find-VsInstall)
    $sdk = Find-WindowsSdk

    if (-not $toolset -or -not $sdk) {
        Write-Warn 'Could not locate a complete MSVC + Windows SDK toolset.'
        Write-Warn 'Retrying as-is. If linking fails, install the Visual Studio'
        Write-Warn 'workload "Desktop development with C++".'
        return $false
    }

    $env:LIB     = (@($toolset.Lib) + @($sdk.Lib)) -join ';'
    $env:INCLUDE = (@($toolset.Include) + @($sdk.Include)) -join ';'

    Write-Ok "MSVC lib : $($toolset.Lib)"
    Write-Ok "SDK lib  : $($sdk.Lib[0])"
    return $true
}

function Invoke-CargoBuild {
    param([string]$Cargo)
    Push-Location $RepoRoot

    # cargo writes compiler warnings to stderr. With $ErrorActionPreference = 'Stop',
    # PowerShell promotes the first stderr line from a native command into a
    # terminating NativeCommandError and kills the build mid-way. Relax the
    # preference for the native call only, and judge success by the exit code.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & $Cargo build --release --workspace
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previous
        Pop-Location
    }
}

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

# Keep a transcript next to the build. Handy when the console window closes
# faster than the message can be read, and easy to attach to a bug report.
$LogPath = Join-Path $RepoRoot 'build-release.log'
Start-Transcript -Path $LogPath -Force | Out-Null

Write-Host ''
Write-Host '  XGameEngine - release build' -ForegroundColor White
Write-Note "repository : $RepoRoot"
Write-Note "output     : $DistDir"

Write-Step 'Locating cargo'
$cargo = Resolve-Cargo
Write-Ok $cargo

Write-Step 'Building (cargo build --release --workspace)'
Write-Note 'Full optimized build. The first run can take several minutes.'
Write-Host ''

$exit = Invoke-CargoBuild -Cargo $cargo

if ($exit -ne 0) {
    Write-Warn "First attempt failed (exit code $exit)."
    Write-Step 'Retrying with an explicit MSVC link environment'
    [void](Enable-MsvcEnv)
    Write-Host ''
    $exit = Invoke-CargoBuild -Cargo $cargo
}

if ($exit -ne 0) {
    throw "cargo build failed with exit code $exit."
}
Write-Ok 'build succeeded'

# ---------------------------------------------------------------------------
# Collect artifacts
# ---------------------------------------------------------------------------

Write-Step 'Collecting artifacts'

$editorExe = Join-Path $RepoRoot 'target\release\xgame-editor.exe'
$gameExe   = Join-Path $RepoRoot 'target\release\xgame-game.exe'

foreach ($exe in @($editorExe, $gameExe)) {
    if (-not (Test-Path $exe)) { throw "Expected build output is missing: $exe" }
}

# Only ever wipe the dist folder we own. Going through the .NET API rather than
# Remove-Item: it is unambiguous about what gets removed, does not route through
# any shell-level recycle-bin wrapper, and is not subject to bulk-delete limits.
if ([System.IO.Directory]::Exists($DistDir)) {
    [System.IO.Directory]::Delete($DistDir, $true)
}
[System.IO.Directory]::CreateDirectory($DistDir) | Out-Null

Copy-Item $editorExe $DistDir -Force
Copy-Item $gameExe   $DistDir -Force
Write-Ok 'xgame-editor.exe'
Write-Ok 'xgame-game.exe'

if (-not $SkipSample) {
    $sample = Join-Path $RepoRoot 'projects\SampleScene'
    if (Test-Path $sample) {
        $sampleRoot = Join-Path $DistDir 'projects'
        New-Item -ItemType Directory -Path $sampleRoot -Force | Out-Null
        Copy-Item $sample (Join-Path $sampleRoot 'SampleScene') -Recurse -Force
        Write-Ok 'projects\SampleScene\  (sample project)'
    } else {
        Write-Warn 'Bundled sample project not found; skipped.'
    }
}

$usage = @'
XGameEngine
===========

Running the editor
------------------
Double-click xgame-editor.exe. The Project Hub opens first: choose "Open" to
pick an existing project folder, or create a new one. The bundled
projects\SampleScene is ready to open as-is.

Building a game from a project
------------------------------
Open a project in the editor, then press "Build" in the top-right corner.
The packaged game is written to:

    <your project>\Build\<project name>\

Double-click the .exe in that folder to play. It locates its project sitting
right next to it, so it needs no command-line arguments and no launcher script.

What xgame-game.exe is for
--------------------------
It is the player runtime. The editor copies it every time you press "Build",
so keep it next to xgame-editor.exe.
'@

Set-Content -Path (Join-Path $DistDir 'README.txt') -Value $usage -Encoding UTF8
Write-Ok 'README.txt'

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

Write-Step 'Done'
Get-ChildItem $DistDir | ForEach-Object {
    $kind = if ($_.PSIsContainer) { '<DIR>' } else { '{0,10:N0} B' -f $_.Length }
    Write-Host ("    {0}  {1}" -f $kind, $_.Name)
}

Write-Host ''
Write-Host "Output folder: $DistDir" -ForegroundColor Green
Write-Note "log        : $LogPath"

Stop-Transcript | Out-Null
