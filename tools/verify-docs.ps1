param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"
$Utf8NoBomEncoding = [System.Text.UTF8Encoding]::new($false)

function Fail([string]$Message) {
    Write-Error $Message
    exit 1
}

function Test-Utf8Bom([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    return $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
}

function Resolve-MarkdownLink([string]$SourcePath, [string]$Target) {
    $normalizedTarget = $Target.Trim()
    $angleWrapped = $false
    if ($normalizedTarget.StartsWith("<") -and $normalizedTarget.EndsWith(">")) {
        $normalizedTarget = $normalizedTarget.Substring(1, $normalizedTarget.Length - 2)
        $angleWrapped = $true
    }

    $baseDir = Split-Path -Parent $SourcePath
    $targetCandidates = @($normalizedTarget)
    if (-not $angleWrapped -and $normalizedTarget -match '^([^\s]+)\s+.*$') {
        $targetCandidates += $Matches[1]
    }

    foreach ($targetCandidate in $targetCandidates) {
        $withoutAnchor = ($targetCandidate -split "#", 2)[0]
        if ([string]::IsNullOrWhiteSpace($withoutAnchor)) {
            continue
        }
        if ($withoutAnchor -match "^[a-zA-Z][a-zA-Z0-9+.-]*:") {
            return $null
        }

        $decoded = [System.Uri]::UnescapeDataString($withoutAnchor)
        try {
            $resolved = [System.IO.Path]::GetFullPath((Join-Path $baseDir $decoded))
            if (Test-Path -LiteralPath $resolved) {
                return $resolved
            }
        }
        catch {
            return "__invalid_path__::$withoutAnchor"
        }
    }

    $fallback = ($targetCandidates[0] -split "#", 2)[0]
    if ([string]::IsNullOrWhiteSpace($fallback)) {
        return $null
    }
    try {
        $decodedFallback = [System.Uri]::UnescapeDataString($fallback)
        return [System.IO.Path]::GetFullPath((Join-Path $baseDir $decodedFallback))
    }
    catch {
        return "__invalid_path__::$fallback"
    }
}

function Get-WorkspaceRelativePath([string]$Root, [string]$Path) {
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $rootUri = New-Object System.Uri(($rootFull + [System.IO.Path]::DirectorySeparatorChar))
    $pathUri = New-Object System.Uri($pathFull)
    return [System.Uri]::UnescapeDataString($rootUri.MakeRelativeUri($pathUri).ToString()).Replace("/", [System.IO.Path]::DirectorySeparatorChar)
}

function Test-InDocumentScope([string]$Path, [string[]]$Roots) {
    foreach ($root in $Roots) {
        $fullRoot = [System.IO.Path]::GetFullPath($root)
        if ($Path -eq $fullRoot -or $Path.StartsWith($fullRoot + [System.IO.Path]::DirectorySeparatorChar)) {
            return $true
        }
    }
    return $false
}

function Get-NormalizedWorkspacePath([string]$Root, [string]$Path) {
    $relative = Get-WorkspaceRelativePath $Root $Path
    return $relative.Replace('\', '/')
}

function Test-WorkspacePathExcluded([string]$Root, [string]$Path) {
    $relative = Get-NormalizedWorkspacePath $Root $Path
    return $relative -like "target/*" -or
        $relative -like "_reference_repos/*" -or
        $relative -like ".git/*" -or
        $relative -like ".gitnexus/*"
}

function Get-QualityFiles([string]$Root, [string[]]$Extensions) {
    $rootFiles = @(
        (Join-Path $Root "README.md"),
        (Join-Path $Root "CONTRIBUTING.md"),
        (Join-Path $Root "SECURITY.md"),
        (Join-Path $Root "CODE_OF_CONDUCT.md"),
        (Join-Path $Root "CHANGELOG.md"),
        (Join-Path $Root "Cargo.toml"),
        (Join-Path $Root "Cargo.lock"),
        (Join-Path $Root "rust-toolchain.toml")
    ) | Where-Object { Test-Path -LiteralPath $_ } | ForEach-Object { Get-Item -LiteralPath $_ }
    $scanRoots = @("crates", "docs", "tools") |
        ForEach-Object { Join-Path $Root $_ } |
        Where-Object { Test-Path -LiteralPath $_ }
    $nestedFiles = foreach ($scanRoot in $scanRoots) {
        Get-ChildItem -Path $scanRoot -Recurse -File -Include $Extensions
    }
    return @($rootFiles) + @($nestedFiles)
}

$fileExtensions = @("*.md", "*.rs", "*.toml", "*.json", "*.ps1")
$qualityFiles = Get-QualityFiles $WorkspaceRoot $fileExtensions |
    Where-Object { -not (Test-WorkspacePathExcluded $WorkspaceRoot $_.FullName) }

$bomHits = @()
foreach ($file in $qualityFiles) {
    if (Test-Utf8Bom $file.FullName) {
        $bomHits += $file.FullName
    }
}

if ($bomHits.Count -gt 0) {
    Fail ("UTF-8 BOM file detected:" + [Environment]::NewLine + ($bomHits -join [Environment]::NewLine))
}

$documentRoots = @(
    (Join-Path $WorkspaceRoot "README.md"),
    (Join-Path $WorkspaceRoot "CONTRIBUTING.md"),
    (Join-Path $WorkspaceRoot "SECURITY.md"),
    (Join-Path $WorkspaceRoot "CODE_OF_CONDUCT.md"),
    (Join-Path $WorkspaceRoot "CHANGELOG.md"),
    (Join-Path $WorkspaceRoot "docs")
)

$markdownFiles = $qualityFiles | Where-Object {
    $_.Extension -eq ".md" -and (Test-InDocumentScope $_.FullName $documentRoots)
}
$brokenLinks = @()
$linkPattern = "(?<!\!)\[[^\]]+\]\(([^)]+)\)"
foreach ($file in $markdownFiles) {
    $content = [System.IO.File]::ReadAllText($file.FullName, $Utf8NoBomEncoding)
    foreach ($match in [regex]::Matches($content, $linkPattern)) {
        $target = $match.Groups[1].Value.Trim()
        $targetPath = Resolve-MarkdownLink $file.FullName $target
        if ($null -ne $targetPath -and (
            $targetPath.StartsWith("__invalid_path__::") -or
            -not (Test-Path -LiteralPath $targetPath)
        )) {
            $relativeSource = Get-WorkspaceRelativePath $WorkspaceRoot $file.FullName
            $brokenLinks += "$relativeSource -> $target"
        }
    }
}

if ($brokenLinks.Count -gt 0) {
    Fail ("broken local Markdown link detected:" + [Environment]::NewLine + ($brokenLinks -join [Environment]::NewLine))
}

$termRules = @(
    @{
        Pattern = "(?<!\.)\bmcp\.json\b";
        Message = "MCP config file should be written as .mcp.json"
    },
    @{
        Pattern = "UTF-8 with BOM";
        Message = "encoding baseline should be UTF-8 without BOM"
    },
    @{
        Pattern = "GitNexus";
        Message = "Axon must not depend on GitNexus"
    }
)

$termHits = @()
foreach ($file in $markdownFiles) {
    $relativePath = Get-WorkspaceRelativePath $WorkspaceRoot $file.FullName
    if ($relativePath -eq "AGENTS.md") {
        continue
    }
    $lines = [System.IO.File]::ReadAllLines($file.FullName, $Utf8NoBomEncoding)
    for ($index = 0; $index -lt $lines.Count; $index++) {
        foreach ($rule in $termRules) {
            if ($lines[$index] -match $rule.Pattern) {
                $lineNumber = $index + 1
                $termHits += "${relativePath}:${lineNumber}: $($rule.Message)"
            }
        }
    }
}

if ($termHits.Count -gt 0) {
    Fail ("terminology consistency issue detected:" + [Environment]::NewLine + ($termHits -join [Environment]::NewLine))
}

Write-Output "Documentation verification OK"
