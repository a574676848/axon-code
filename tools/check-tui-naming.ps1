param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

function Fail([string]$Message) {
    Write-Error $Message
    exit 1
}

function Get-NormalizedWorkspacePath([string]$Root, [string]$Path) {
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    $rootUri = New-Object System.Uri(($rootFull + [System.IO.Path]::DirectorySeparatorChar))
    $pathUri = New-Object System.Uri($pathFull)
    return [System.Uri]::UnescapeDataString($rootUri.MakeRelativeUri($pathUri).ToString()).Replace("\", "/")
}

function Get-RustFiles([string]$Root, [string[]]$RelativeDirs) {
    $files = @()
    foreach ($relativeDir in $RelativeDirs) {
        $dir = Join-Path $Root $relativeDir
        if (Test-Path -LiteralPath $dir) {
            $files += Get-ChildItem -Path $dir -Recurse -File -Filter *.rs
        }
    }
    return @($files)
}

function Get-PublicRustTypes([string]$Root, [System.IO.FileInfo[]]$Files) {
    $types = @()
    $pattern = '\bpub\s+(?:\([^)]*\)\s+)?(struct|enum|trait|type)\s+([A-Z][A-Za-z0-9_]*)\b'
    foreach ($file in $Files) {
        $lineNumber = 0
        foreach ($line in [System.IO.File]::ReadLines($file.FullName, [System.Text.Encoding]::UTF8)) {
            $lineNumber += 1
            $match = [regex]::Match($line, $pattern)
            if ($match.Success) {
                $types += [pscustomobject]@{
                    Name = $match.Groups[2].Value
                    Path = Get-NormalizedWorkspacePath $Root $file.FullName
                    Line = $lineNumber
                    Source = $line.Trim()
                }
            }
        }
    }
    return $types
}

function Find-NameHits([string]$Root, [System.IO.FileInfo[]]$Files, [string[]]$Names, [string]$Message) {
    $hits = @()
    foreach ($file in $Files) {
        $lineNumber = 0
        foreach ($line in [System.IO.File]::ReadLines($file.FullName, [System.Text.Encoding]::UTF8)) {
            $lineNumber += 1
            foreach ($name in $Names) {
                if ($line -match "\b$([regex]::Escape($name))\b") {
                    $relativePath = Get-NormalizedWorkspacePath $Root $file.FullName
                    $hits += "${relativePath}:${lineNumber}: ${Message}: ${name}: $($line.Trim())"
                }
            }
        }
    }
    return $hits
}

function Find-DuplicatePublicTypeHits([object[]]$PublicTypes) {
    $hits = @()
    $groups = $PublicTypes | Group-Object -Property Name | Where-Object { $_.Count -gt 1 }
    foreach ($group in $groups) {
        foreach ($item in $group.Group) {
            $hits += "$($item.Path):$($item.Line): duplicated target-state TUI public type: $($item.Name): $($item.Source)"
        }
    }
    return $hits
}

$targetDirs = @(
    "crates/axon-tui-runtime/src/surface",
    "crates/axon-tui-runtime/src/app_shell",
    "crates/axon-tui-runtime/src/ui_workflow"
)
$targetFiles = Get-RustFiles $WorkspaceRoot $targetDirs
$publicTypes = Get-PublicRustTypes $WorkspaceRoot $targetFiles

$duplicatePublicTypes = Find-DuplicatePublicTypeHits $publicTypes
if ($duplicatePublicTypes.Count -gt 0) {
    Fail ("TUI duplicated public type detected:" + [Environment]::NewLine + ($duplicatePublicTypes -join [Environment]::NewLine))
}

$forbiddenTemporaryNames = @(
    "RenderSurfaceCell",
    "SurfaceRenderCell",
    "CellSnapshot",
    "TranscriptCellSnapshot",
    "SnapshotCell",
    "TuiRenderSnapshot",
    "SessionSnapshot",
    "RenderSurface"
)

$temporaryHits = Find-NameHits $WorkspaceRoot $targetFiles $forbiddenTemporaryNames "temporary target-state TUI name is forbidden"
if ($temporaryHits.Count -gt 0) {
    Fail ("TUI temporary naming drift detected:" + [Environment]::NewLine + ($temporaryHits -join [Environment]::NewLine))
}

Write-Output "TUI naming verification OK"
