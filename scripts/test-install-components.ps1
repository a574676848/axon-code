# Axon 源码安装组件自检脚本
# 只做断言，不安装任何东西：导入与安装脚本共用的组件解析模块，并对安装脚本正文做静态检查。
# 全部断言通过输出 OK: 行并以 0 退出；任一失败输出 ERR: 行并以 1 退出。
[CmdletBinding()]
param()
$ErrorActionPreference = "Stop"
$utf8 = [Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8
[Console]::OutputEncoding = $utf8
[Console]::InputEncoding = $utf8
$PSStyle.OutputRendering = "PlainText"

$installScriptPath = Join-Path $PSScriptRoot "install-from-source.ps1"
$modulePath = Join-Path $PSScriptRoot "install-components.psm1"
$failures = [System.Collections.Generic.List[string]]::new()
$script:total = 0

function Assert-True([bool]$condition, [string]$description) {
    $script:total++
    if ($condition) {
        Write-Host "OK: $description"
    } else {
        Write-Host "ERR: $description" -ForegroundColor Red
        $failures.Add($description)
    }
}

try {
    Import-Module $modulePath -Force -ErrorAction Stop
} catch {
    Write-Host "ERR: 组件解析模块导入失败：$($_.Exception.Message)" -ForegroundColor Red
    Write-Host "ERR: 共 1 项断言失败，组件安装自检未通过。" -ForegroundColor Red
    exit 1
}
Assert-True $true "组件解析模块导入成功：scripts/install-components.psm1"

$cases = @(
    [pscustomobject]@{ Label = "backend"; Call = { Resolve-AxonComponents -Components "backend" }; Backend = $true; Desktop = $false }
    [pscustomobject]@{ Label = "desktop"; Call = { Resolve-AxonComponents -Components "desktop" }; Backend = $false; Desktop = $true }
    [pscustomobject]@{ Label = "all"; Call = { Resolve-AxonComponents -Components "all" }; Backend = $true; Desktop = $true }
    [pscustomobject]@{ Label = "backend,desktop"; Call = { Resolve-AxonComponents -Components "backend,desktop" }; Backend = $true; Desktop = $true }
    [pscustomobject]@{ Label = "空值（缺省，不传参数）"; Call = { Resolve-AxonComponents }; Backend = $true; Desktop = $true }
    [pscustomobject]@{ Label = "重复传参 backend + desktop"; Call = { Resolve-AxonComponents -Components @("backend", "desktop") }; Backend = $true; Desktop = $true }
    [pscustomobject]@{ Label = "空数组"; Call = { Resolve-AxonComponents -Components @() }; Backend = $true; Desktop = $true }
)
foreach ($case in $cases) {
    $label = $case.Label
    $expectBackend = [bool]$case.Backend
    $expectDesktop = [bool]$case.Desktop
    $expected = "InstallBackend={0}、InstallDesktop={1}" -f $expectBackend, $expectDesktop
    try {
        $resolved = & $case.Call
        $isBoolean = ($resolved.InstallBackend -is [bool]) -and ($resolved.InstallDesktop -is [bool])
        $matched = ([bool]$resolved.InstallBackend -eq $expectBackend) -and ([bool]$resolved.InstallDesktop -eq $expectDesktop)
        Assert-True ($isBoolean -and $matched) "-Components $label 解析为 $expected"
    } catch {
        Assert-True $false "-Components $label 解析抛错：$($_.Exception.Message)，期望 $expected"
    }
}

$threw = $false
$errorMessage = ""
try {
    Resolve-AxonComponents -Components "foo" | Out-Null
} catch {
    $threw = $true
    $errorMessage = $_.Exception.Message
}
$invalidOk = $threw -and $errorMessage.Contains("未知组件名") -and $errorMessage.Contains("backend") -and
    $errorMessage.Contains("desktop") -and $errorMessage.Contains("all")
Assert-True $invalidOk "非法组件名 foo 抛出中文异常，并列出 backend、desktop、all"

Assert-True (Test-Path -LiteralPath $installScriptPath -PathType Leaf) "安装脚本存在：scripts/install-from-source.ps1"

$migrationPattern = '-BinariesOnly 已移除，请改用 -Components backend'
$binariesOnlyHits = @(Select-String -LiteralPath $installScriptPath -Pattern '-BinariesOnly' -SimpleMatch)
$migrationHits = @(Select-String -LiteralPath $installScriptPath -Pattern $migrationPattern -SimpleMatch)
# 迁移说明本身必须写出旧参数名，因此零命中的口径是「除迁移说明外不再出现旧参数名」。
$strayHits = @($binariesOnlyHits | Where-Object { $_.Line -notlike "*$migrationPattern*" })
Assert-True ($strayHits.Count -eq 0) "除迁移说明外，安装脚本正文不出现 -BinariesOnly（其余命中 $($strayHits.Count) 处）"
Assert-True ($migrationHits.Count -ge 2) "顶部注释块与 param 注释都含迁移说明（命中 $($migrationHits.Count) 处）"

$binariesOnlyParamHits = @(Select-String -LiteralPath $installScriptPath -Pattern '$BinariesOnly' -SimpleMatch)
Assert-True ($binariesOnlyParamHits.Count -eq 0) "安装脚本不再声明或判断 -BinariesOnly 参数（Select-String 命中 0 处）"

$componentsHits = @(Select-String -LiteralPath $installScriptPath -Pattern '$Components' -SimpleMatch)
Assert-True ($componentsHits.Count -ge 1) "安装脚本以 -Components 作为组件入口"

$resolveHits = @(Select-String -LiteralPath $installScriptPath -Pattern 'Resolve-AxonComponents -Components $Components' -SimpleMatch)
Assert-True ($resolveHits.Count -eq 1) "安装脚本把 -Components 交给 Resolve-AxonComponents 解析"

$localDefHits = @(Select-String -LiteralPath $installScriptPath -Pattern 'function Resolve-AxonComponents' -SimpleMatch)
Assert-True ($localDefHits.Count -eq 0) "组件解析只有一份实现（安装脚本不重复定义该函数）"

$moduleHits = @(Select-String -LiteralPath $installScriptPath -Pattern 'install-components.psm1' -SimpleMatch)
Assert-True ($moduleHits.Count -ge 1) "安装脚本导入共用组件解析模块"

$driveHits = @(Select-String -LiteralPath $installScriptPath -Pattern '[A-Za-z]:\\')
Assert-True ($driveHits.Count -eq 0) "安装脚本不含盘符绝对路径（路径无关性）"

$backendCopyHits = @(Select-String -LiteralPath $installScriptPath -Pattern 'if ($plan.InstallBackend) { Copy-Item $cli' -SimpleMatch)
$desktopCopyHits = @(Select-String -LiteralPath $installScriptPath -Pattern 'if ($plan.InstallDesktop) { Copy-Item $desktop' -SimpleMatch)
Assert-True (($backendCopyHits.Count -eq 1) -and ($desktopCopyHits.Count -eq 1)) "版本目录写入按组件裁剪：只拷本次选中的二进制"

# 用语法树核对组件门控的落点，避免只看文字导致门控被挪到分支之外却仍「通过」。
$tokens = $null
$parseErrors = $null
$installAst = [System.Management.Automation.Language.Parser]::ParseFile($installScriptPath, [ref]$tokens, [ref]$parseErrors)
Assert-True ($parseErrors.Count -eq 0) "安装脚本语法解析无错误（解析错误 $($parseErrors.Count) 个）"

$ifStatements = @($installAst.FindAll({ param($node) $node -is [System.Management.Automation.Language.IfStatementAst] }, $true))
$backendBlockText = (@($ifStatements | Where-Object { $_.Clauses[0].Item1.Extent.Text -eq '$plan.InstallBackend' }) |
        ForEach-Object { $_.Clauses[0].Item2.Extent.Text }) -join "`n"
$daemonGated = $backendBlockText.Contains('setup ') -and $backendBlockText.Contains('daemon install') -and
    $backendBlockText.Contains('daemon start') -and $backendBlockText.Contains('daemon status')
Assert-True $daemonGated "配置初始化与 Daemon 注册、启动、运行断言都在服务端组件分支内"

$desktopBlockText = (@($ifStatements | Where-Object { $_.Clauses[0].Item1.Extent.Text -eq '$plan.InstallDesktop' }) |
        ForEach-Object { $_.Clauses[0].Item2.Extent.Text }) -join "`n"
$desktopGated = $desktopBlockText.Contains('npm run build') -and $desktopBlockText.Contains('--self-check')
Assert-True $desktopGated "前端构建与桌面端自检都在桌面端组件分支内"

if ($failures.Count -eq 0) {
    Write-Host "OK: 全部 $($script:total) 项断言通过，组件安装自检通过。" -ForegroundColor Green
    exit 0
}
Write-Host "ERR: 共 $($failures.Count) 项断言失败，组件安装自检未通过。" -ForegroundColor Red
exit 1
