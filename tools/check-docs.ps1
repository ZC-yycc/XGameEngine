# Documentation gate for XGameEngine.
#
# Hard rules (exit 1):
#   - only README.md / ROADMAP.md / FLOW.md / AGENTS.md may live in the repo root
#   - active doc filenames must not contain a YYYY-MM-DD date
#   - relative Markdown links in active docs must resolve
#   - every archived Markdown/HTML file must carry an archive banner
#
# Warnings (exit 0 unless -Enforce):
#   - active Docs/*.md without a genre banner
#   - broken relative links inside frozen archives
#
# Usage:
#   powershell -File tools/check-docs.ps1
#   powershell -File tools/check-docs.ps1 -Enforce
#
# ASCII-only on purpose: Windows PowerShell 5.1 mis-decodes UTF-8 without BOM.
# The few Chinese markers are built from code points so this file stays ASCII.

param(
    [switch]$Enforce,
    [string]$Root = (Get-Location).Path
)

$ErrorActionPreference = 'Stop'

$rootFull = [System.IO.Path]::GetFullPath($Root)
$docsDir = Join-Path $rootFull 'Docs'
$historyName = [string]([char]0x5386) + [char]0x53F2
$historyDir = Join-Path $docsDir $historyName

$archiveTag = [string]([char]0x5F52) + [char]0x6863
$freezeTag = [string]([char]0x51BB) + [char]0x7ED3
$genreTag = [string]([char]0x4F53) + [char]0x88C1

$rootWhitelist = @('README.md', 'ROADMAP.md', 'FLOW.md', 'AGENTS.md')

$script:hard = 0
$script:warn = 0

function Fail([string]$Message) {
    Write-Output ("FAIL  " + $Message)
    $script:hard++
}

function Warn([string]$Message) {
    Write-Output ("WARN  " + $Message)
    $script:warn++
}

function Get-MarkdownLinks([System.IO.FileInfo]$File) {
    $lines = [System.IO.File]::ReadAllLines($File.FullName, [System.Text.Encoding]::UTF8)
    $inFence = $false
    $result = @()
    for ($i = 0; $i -lt $lines.Length; $i++) {
        $line = $lines[$i]
        if ($line -match '^\s*(```|~~~)') {
            $inFence = -not $inFence
            continue
        }
        if ($inFence) { continue }

        foreach ($m in [regex]::Matches($line, '!?\[[^\]]*\]\(([^)]+)\)')) {
            $raw = $m.Groups[1].Value.Trim()
            if ([string]::IsNullOrWhiteSpace($raw)) { continue }
            if ($raw -match '^(https?:|mailto:|tel:|data:|#)') { continue }

            $target = $raw
            if ($target.StartsWith('<')) {
                $end = $target.IndexOf('>')
                if ($end -gt 0) { $target = $target.Substring(1, $end - 1) }
            } else {
                $target = ($target -split '\s+')[0]
            }

            $cut = $target.IndexOf('#')
            if ($cut -ge 0) { $target = $target.Substring(0, $cut) }
            $cut = $target.IndexOf('?')
            if ($cut -ge 0) { $target = $target.Substring(0, $cut) }
            if ([string]::IsNullOrWhiteSpace($target)) { continue }

            try { $target = [System.Uri]::UnescapeDataString($target) } catch { }

            $result += [pscustomobject]@{ Line = $i + 1; Target = $target }
        }
    }
    return $result
}

function Test-Link([System.IO.FileInfo]$File, [int]$Line, [string]$Target) {
    $base = Split-Path -Parent $File.FullName
    $path = $Target.Replace('/', '\')
    if ($path.StartsWith('\')) {
        $base = $rootFull
        $path = $path.TrimStart('\')
    }
    try {
        $resolved = [System.IO.Path]::GetFullPath((Join-Path $base $path))
    } catch {
        return $false
    }
    return (Test-Path -LiteralPath $resolved)
}

Write-Output ("check-docs: root " + $rootFull)

# 1. Root Markdown whitelist.
$rootDocs = @(Get-ChildItem -LiteralPath $rootFull -File -Filter '*.md')
foreach ($doc in $rootDocs) {
    if ($rootWhitelist -notcontains $doc.Name) {
        Fail ("root doc not whitelisted: " + $doc.Name)
    }
}
Write-Output ("  root docs: " + (($rootDocs | ForEach-Object { $_.Name }) -join ', '))

# 2. Active filenames must not contain dates.
$activeDocs = @()
$activeDocs += $rootDocs
if (Test-Path -LiteralPath $docsDir) {
    $activeDocs += @(Get-ChildItem -LiteralPath $docsDir -File -Filter '*.md')
}
foreach ($doc in $activeDocs) {
    if ($doc.Name -match '\d{4}-\d{2}-\d{2}') {
        Fail ("active doc has a date in its name: " + $doc.Name)
    }
}

# 3. Relative links in active docs.
foreach ($doc in $activeDocs) {
    foreach ($link in (Get-MarkdownLinks $doc)) {
        if (-not (Test-Link $doc $link.Line $link.Target)) {
            Fail ("broken link: " + $doc.Name + ":" + $link.Line + " -> " + $link.Target)
        }
    }
}

# 4. Archive banners + archive link warnings.
if (Test-Path -LiteralPath $historyDir) {
    $archiveDocs = @(Get-ChildItem -LiteralPath $historyDir -File -Filter '*.md')
    foreach ($doc in $archiveDocs) {
        if ($doc.Name -eq 'README.md') { continue }
        $head = ([System.IO.File]::ReadAllLines($doc.FullName, [System.Text.Encoding]::UTF8) |
            Select-Object -First 20) -join "`n"
        if ($head -notmatch ($archiveTag + '|' + $freezeTag)) {
            Fail ("archive banner missing: " + $doc.Name)
        }
    }

    $archiveHtml = @(Get-ChildItem -LiteralPath $historyDir -File -Filter '*.html')
    foreach ($doc in $archiveHtml) {
        $head = ([System.IO.File]::ReadAllLines($doc.FullName, [System.Text.Encoding]::UTF8) |
            Select-Object -First 5) -join "`n"
        if ($head -notmatch $archiveTag) {
            Fail ("archive banner missing: " + $doc.Name)
        }
    }

    foreach ($doc in $archiveDocs) {
        foreach ($link in (Get-MarkdownLinks $doc)) {
            if (-not (Test-Link $doc $link.Line $link.Target)) {
                Warn ("archive link broken (frozen): " + $doc.Name + ":" + $link.Line + " -> " + $link.Target)
            }
        }
    }
}

# 5. Genre banner warning for active Docs/*.md.
if (Test-Path -LiteralPath $docsDir) {
    $docsMd = @(Get-ChildItem -LiteralPath $docsDir -File -Filter '*.md')
    foreach ($doc in $docsMd) {
        $head = ([System.IO.File]::ReadAllLines($doc.FullName, [System.Text.Encoding]::UTF8) |
            Select-Object -First 20) -join "`n"
        if ($head -notmatch $genreTag) {
            Warn ("genre banner missing: Docs/" + $doc.Name)
        }
    }
}

Write-Output ''
Write-Output ("check-docs: {0} hard failure(s), {1} warning(s)" -f $script:hard, $script:warn)

if ($script:hard -gt 0) {
    Write-Output 'check-docs: FAILED.'
    exit 1
}

if ($Enforce -and $script:warn -gt 0) {
    Write-Output 'check-docs: FAILED (-Enforce treats warnings as errors).'
    exit 1
}

Write-Output 'check-docs: OK.'
exit 0
