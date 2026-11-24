# CLI 发行包冒烟的 Windows 入口。
#
# 判定逻辑只有一份，在 tools/verify_cli_release_smoke.py。

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
    Write-Error 'Python 3 is required to run the cross-platform CLI release smoke.'
    exit 1
}

& $python -X utf8 (Join-Path $PSScriptRoot 'verify_cli_release_smoke.py')
exit $LASTEXITCODE
