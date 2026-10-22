param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"
$python = Get-Command python -ErrorAction SilentlyContinue
if ($null -eq $python) {
    $python = Get-Command py -ErrorAction SilentlyContinue
}
if ($null -eq $python) {
    Write-Error "Python 3 is required to run the cross-platform architecture verifier."
    exit 1
}

$invalidConfigUses = @(
    Get-ChildItem -LiteralPath (Join-Path $WorkspaceRoot "crates") -Recurse -File -Filter "*.rs" |
        Select-String -Pattern "AxonError::config_invalid" |
        ForEach-Object { "{0}:{1}: {2}" -f $_.Path, $_.LineNumber, $_.Line.Trim() }
)
$approvedConfigFiles = @(
    "\\axon-config\\.*\.rs:",
    "\\axon-agent-runtime\\src\\provider\\provider_stream_runner\.rs:"
)
$outsideConfig = @($invalidConfigUses | Where-Object {
    $line = $_
    -not ($approvedConfigFiles | Where-Object { $line -match $_ })
})
if ($outsideConfig.Count -gt 0) {
    $outsideConfig | ForEach-Object { Write-Error "config_invalid is only allowed for axon-config validation: $_" }
    exit 1
}

& $python.Source (Join-Path $PSScriptRoot "verify-architecture.py") --workspace-root $WorkspaceRoot
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

function Assert-NoCargoDependency {
    param(
        [string]$Crate,
        [string[]]$Forbidden
    )
    $manifest = Join-Path $WorkspaceRoot ("crates\{0}\Cargo.toml" -f $Crate)
    if (-not (Test-Path -LiteralPath $manifest)) {
        Write-Error "Missing crate manifest: $manifest"
        exit 1
    }
    $content = Get-Content -LiteralPath $manifest -Raw
    foreach ($dependency in $Forbidden) {
        if ($content -match ("(?m)^\s*{0}\s*=" -f [regex]::Escape($dependency))) {
            Write-Error "$Crate must not depend on $dependency; onion architecture boundary violation."
            exit 1
        }
    }
}

Assert-NoCargoDependency -Crate "axon-domain" -Forbidden @(
    "axon-session", "axon-session-runtime", "axon-daemon", "axon-cli", "axon-remote-feishu", "rusqlite", "tokio"
)
Assert-NoCargoDependency -Crate "axon-session-runtime" -Forbidden @(
    "axon-cli", "axon-remote-feishu"
)
Assert-NoCargoDependency -Crate "axon-daemon" -Forbidden @(
    "axon-cli", "axon-remote-feishu"
)
Assert-NoCargoDependency -Crate "axon-apply" -Forbidden @(
    "axon-agent-runtime"
)
Assert-NoCargoDependency -Crate "axon-desktop" -Forbidden @(
    "axon-cli", "axon-daemon", "axon-remote-feishu",
    # 表现层必须经网关协议访问运行态，禁止直连会话子系统或运行态数据库。
    "axon-session", "axon-session-runtime", "axon-domain", "rusqlite"
)
