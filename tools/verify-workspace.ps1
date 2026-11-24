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

Invoke-CheckedCommand "cargo fmt" "cargo" @("fmt", "--all", "--check")
Invoke-CheckedCommand "cargo metadata" "cargo" @("metadata", "--no-deps", "--format-version", "1")
Invoke-CheckedCommand "cargo check" "cargo" @("check", "--workspace")
Invoke-CheckedCommand "cargo clippy" "cargo" @(
    "clippy",
    "--workspace",
    "--all-targets",
    "--",
    "-D",
    "warnings"
)
Invoke-CheckedCommand "cargo test" "cargo" @("test", "--workspace")
Write-Output "==> output filter acceptance"
& (Join-Path (Join-Path $WorkspaceRoot "tools") "verify-output-filter.ps1")
if ($LASTEXITCODE -ne 0) {
    Write-Error "output filter acceptance failed."
    exit 1
}
Write-Output "==> cli release smoke"
& (Join-Path (Join-Path $WorkspaceRoot "tools") "verify-cli-release-smoke.ps1") -WorkspaceRoot $WorkspaceRoot
if ($LASTEXITCODE -ne 0) {
    Write-Error "cli release smoke failed."
    exit 1
}
Write-Output "==> architecture and docs"
& (Join-Path (Join-Path $WorkspaceRoot "tools") "verify-architecture.ps1") -WorkspaceRoot $WorkspaceRoot
if ($LASTEXITCODE -ne 0) {
    Write-Error "architecture and docs failed."
    exit 1
}

Write-Output "Workspace verification OK"
