Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Assert-True($condition, $message) {
    if (-not $condition) {
        throw "ASSERTION FAILED: $message"
    }
}

$repoRoot = Split-Path -Parent $PSScriptRoot
$tmpDir = Join-Path $env:TEMP ("axon-smoke-setup-" + [Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tmpDir -Force | Out-Null

try {
    Write-Host "Building axon binary..."
    cargo build --bin axon --quiet
    $axonBin = Join-Path $repoRoot "target\debug\axon.exe"
    Assert-True (Test-Path $axonBin) "axon binary should exist"

    # 1. Secret-only test
    Write-Host "Testing Secret-only setup..."
    $cfgPath1 = Join-Path $tmpDir "config1\config.toml"
    $envPath1 = Join-Path $tmpDir "config1\.env"
    $output1 = & $axonBin --config $cfgPath1 setup --non-interactive --format json --api-key "secret-smoke-123" | ConvertFrom-Json
    Assert-True (-not (Test-Path $cfgPath1)) "Secret-only should not create config.toml"
    Assert-True (Test-Path $envPath1) "Secret-only should create .env"
    Assert-True ($output1.status -eq "ok") "Status should be ok"
    Assert-True ($output1.credentialStatus -eq "present") "Credential status should be present"
    Assert-True (-not ($output1 | ConvertTo-Json).Contains("secret-smoke-123")) "Report should not contain raw secret"

    # 2. Quick without fields
    Write-Host "Testing Quick without fields..."
    $cfgPath2 = Join-Path $tmpDir "config2\config.toml"
    $output2 = & $axonBin --config $cfgPath2 setup --non-interactive --quick --format json | ConvertFrom-Json
    Assert-True (-not (Test-Path $cfgPath2)) "Quick with no fields should not create config.toml"

    # 3. Quick with explicit provider/model
    Write-Host "Testing Quick with explicit provider and model..."
    $cfgPath3 = Join-Path $tmpDir "config3\config.toml"
    $output3 = & $axonBin --config $cfgPath3 setup --non-interactive --quick --format json --provider-name "smoke-p" --model "smoke-m" | ConvertFrom-Json
    Assert-True (Test-Path $cfgPath3) "Quick with explicit provider/model should create config.toml"
    Assert-True ($output3.providerName -eq "smoke-p") "Provider name should be smoke-p"

    # 4. Provider Reset
    Write-Host "Testing Provider Reset..."
    $cfgPath4 = Join-Path $tmpDir "config4\config.toml"
    & $axonBin --config $cfgPath4 setup --non-interactive --provider-name "reset-p" --model "reset-m" | Out-Null
    Assert-True (Test-Path $cfgPath4) "Config before reset should exist"
    & $axonBin --config $cfgPath4 setup --non-interactive --reset-provider "reset-p" | Out-Null
    $content4 = Get-Content $cfgPath4 -Raw
    Assert-True (-not ($content4 -match "reset-p")) "Config should no longer contain reset-p"

    # 5. No-mutation run SHA-256 invariance
    Write-Host "Testing No-mutation SHA-256 invariance..."
    $cfgPath5 = Join-Path $tmpDir "config5\config.toml"
    & $axonBin --config $cfgPath5 setup --non-interactive --provider-name "no-mut-p" --model "no-mut-m" | Out-Null
    $shaBefore = (Get-FileHash $cfgPath5 -Algorithm SHA256).Hash
    & $axonBin --config $cfgPath5 setup --non-interactive --quick --provider-name "no-mut-p" --model "no-mut-m" | Out-Null
    $shaAfter = (Get-FileHash $cfgPath5 -Algorithm SHA256).Hash
    Assert-True ($shaBefore -eq $shaAfter) "Config file SHA-256 should remain identical on no-mutation setup"

    Write-Host "All setup smoke tests passed successfully!" -ForegroundColor Green
}
finally {
    if (Test-Path $tmpDir) {
        Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
