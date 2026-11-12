# 性能契约门禁的 Windows 入口。
#
# 判定逻辑只有一份，在 tools/verify_performance.py。Linux 侧直接
# python3 tools/verify_performance.py --allow-incomplete。

param(
    [string]$Baseline = '',
    [switch]$Record,
    [string]$ManualJson = '',
    [switch]$AllowIncomplete
)

$ErrorActionPreference = 'Stop'

$python = $null
foreach ($candidate in @('python', 'py')) {
    $command = Get-Command $candidate -ErrorAction SilentlyContinue
    if ($null -eq $command) { continue }
    & $command.Source -c 'import sys; raise SystemExit(0 if sys.version_info[0] == 3 else 1)' 2>$null
    if ($LASTEXITCODE -eq 0) {
        $python = $command.Source
        break
    }
}

if ($null -eq $python) {
    Write-Error 'Python 3 is required to run the cross-platform performance verifier.'
    exit 1
}

$arguments = @('-X', 'utf8', (Join-Path $PSScriptRoot 'verify_performance.py'))
if ($Baseline -ne '') { $arguments += @('--baseline', $Baseline) }
if ($Record) { $arguments += '--record' }
if ($ManualJson -ne '') { $arguments += @('--manual-json', $ManualJson) }
if ($AllowIncomplete) { $arguments += '--allow-incomplete' }

& $python @arguments
exit $LASTEXITCODE
