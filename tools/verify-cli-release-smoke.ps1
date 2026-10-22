param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

function Invoke-CheckedCommand {
    param(
        [string]$Label,
        [string]$Command,
        [string[]]$Arguments
    )

    Write-Output "==> $Label"
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        Write-Error "$Label failed."
        exit 1
    }
}

Set-Location $WorkspaceRoot

Invoke-CheckedCommand "cargo build release axon-cli" "cargo" @("build", "-p", "axon-cli", "--release")
Invoke-CheckedCommand "axon doctor provider" "cargo" @("run", "--quiet", "-p", "axon-cli", "--", "doctor", "--check", "provider")
Invoke-CheckedCommand "axon doctor smoke" "cargo" @("run", "--quiet", "-p", "axon-cli", "--", "doctor", "--check", "smoke")

$distRoot = Join-Path $WorkspaceRoot "dist"
$packageDir = Join-Path $distRoot "smoke"
if (Test-Path -LiteralPath $packageDir) {
    Remove-Item -LiteralPath $packageDir -Recurse -Force
}

Invoke-CheckedCommand "axon package" "cargo" @("run", "--quiet", "-p", "axon-cli", "--", "package", "--output-dir", $packageDir)

$manifest = Get-ChildItem -LiteralPath $packageDir -Recurse -Filter "axon-release-manifest.json" | Select-Object -First 1
if (-not $manifest) {
    Write-Error "package smoke failed: manifest not found."
    exit 1
}

Invoke-CheckedCommand "axon package verify" "cargo" @(
    "run",
    "--quiet",
    "-p",
    "axon-cli",
    "--",
    "package",
    "--verify-manifest",
    $manifest.FullName
)

Write-Output "CLI release smoke OK"
