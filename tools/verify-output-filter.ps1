Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    cargo run --manifest-path tools/output-filter-acceptance/Cargo.toml --quiet
}
finally {
    Pop-Location
}
