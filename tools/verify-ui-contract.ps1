# UI 契约门禁的 Windows 入口。
#
# 判定逻辑只有一份，在 tools/verify_ui_contract.py。本脚本只负责挑解释器，
# Linux 侧直接 python3 tools/verify_ui_contract.py，两端行为完全一致。

param(
    [string]$WorkspaceRoot = ''
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
    Write-Error 'Python 3 is required to run the cross-platform UI contract verifier.'
    exit 1
}

& $python -X utf8 (Join-Path $PSScriptRoot 'verify_ui_contract.py')
exit $LASTEXITCODE
