# Axon Windows PowerShell 源码安装脚本
# 源码安装与发行版共用 $HOME\.axon\bin 版本树，稳定 PATH 入口就是该 bin 目录。
#
# 组件安装的唯一入口是 -Components，取值为 backend、desktop、all：
#   backend  仅装服务端：编译 axon-cli，版本目录只写 axon.exe，初始化 config.toml，注册并启动 Daemon。
#   desktop  仅装桌面端：编译 axon-desktop，版本目录只写 axon-desktop.exe，不写 config.toml，不注册也不启动 Daemon。
#   all      两个都装。缺省即 all，不传 -Components 与传 all 等价，保持既有升级行为。
# 多值可用逗号分隔（-Components backend,desktop），也可重复传参（-Components backend -Components desktop）。
# 三种组合都会写入 launcher、current.json、axon.cmd 与用户 PATH，它们是桌面入口的硬前置。
#
# 迁移说明：-BinariesOnly 已移除，请改用 -Components backend。
# 旧参数会跳过 UAC 与 daemon install 却仍执行 daemon start，语义自相矛盾；现在 UAC 与 Daemon 全部由组件集合决定。
[CmdletBinding()]
param(
    # 原登录用户 SID。仅提权重入时由脚本自身回传，用于确认提权账号与登录账号一致。
    [string]$ExpectedUserSid = "",
    # 组件集合：backend、desktop、all。可逗号分隔多值，可重复传参；缺省等价 all。
    # -BinariesOnly 已移除，请改用 -Components backend。
    [string[]]$Components = @()
)
$ErrorActionPreference = "Stop"
$utf8 = [Text.UTF8Encoding]::new($false)
$OutputEncoding = $utf8
[Console]::OutputEncoding = $utf8
[Console]::InputEncoding = $utf8
$PSStyle.OutputRendering = "PlainText"

function Stop-Install([string]$reason) {
    Write-Host "[axon] $reason" -ForegroundColor Red
    exit 1
}

# 组件解析只有一份实现：安装脚本与自检脚本共用 scripts/install-components.psm1。
try {
    Import-Module (Join-Path $PSScriptRoot "install-components.psm1") -Force -ErrorAction Stop
} catch {
    Stop-Install "组件解析模块导入失败：$($_.Exception.Message)"
}
try {
    $plan = Resolve-AxonComponents -Components $Components
} catch {
    Stop-Install "组件解析失败：$($_.Exception.Message)"
}
$componentText = if ($plan.InstallBackend -and $plan.InstallDesktop) { "服务端与桌面端" } elseif ($plan.InstallBackend) { "仅服务端" } else { "仅桌面端" }
Write-Host "[axon] 本次安装组件：$componentText" -ForegroundColor Cyan

$installRoot = if ($env:AXON_INSTALL_ROOT) { $env:AXON_INSTALL_ROOT } else { Join-Path $HOME ".axon\bin" }
$installDir = if ($env:AXON_INSTALL_DIR) { $env:AXON_INSTALL_DIR } else { $installRoot }
$launcherDir = Join-Path $installRoot "launcher"
$versionsDir = Join-Path $installRoot "versions"
$currentFile = Join-Path $installRoot "current.json"
$launcherPath = Join-Path $launcherDir "axon-launcher.exe"
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }
$cargoAxon = Join-Path $cargoHome "bin\axon.exe"
$cargoDesktop = Join-Path $cargoHome "bin\axon-desktop.exe"
$stagingRoot = Join-Path ([IO.Path]::GetTempPath()) "axon-source-install-$PID"
$daemonStopped = $false
$taskDisabled = $false
$previousLauncher = Test-Path $launcherPath

<#
 * 版本目录按组件裁剪写入：未选中的组件不会进入版本目录。
 * 若本机已装另一组件，仅装一侧会让 current.json 指向缺少该二进制的版本目录，
 * launcher 转发时找不到目标，等于把可用安装改坏。因此这类组合直接拒绝，并提示改用 all。
 * 判定放在提权之前：注定被拒绝的组合不该再弹一次 UAC。
#>
$installedVersionDir = $null
if (Test-Path $currentFile -PathType Leaf) {
    try {
        $installedVersionId = (Get-Content -LiteralPath $currentFile -Raw | ConvertFrom-Json).versionId
        if ($installedVersionId) { $installedVersionDir = Join-Path $versionsDir $installedVersionId }
    } catch {
        $installedVersionDir = $null
    }
}
if ($installedVersionDir) {
    if (-not $plan.InstallBackend -and (Test-Path (Join-Path $installedVersionDir "axon.exe") -PathType Leaf)) {
        Stop-Install "本机已安装服务端组件（$installedVersionDir\axon.exe），仅装桌面端会让 current.json 指向缺少服务端二进制的版本目录。请改用 -Components all。"
    }
    if (-not $plan.InstallDesktop -and (Test-Path (Join-Path $installedVersionDir "axon-desktop.exe") -PathType Leaf)) {
        Stop-Install "本机已安装桌面端组件（$installedVersionDir\axon-desktop.exe），仅装服务端会让 current.json 指向缺少桌面端二进制的版本目录。请改用 -Components all。"
    }
}

<#
 * 安装成功后把桌面端拉起来。
 *
 * 只在本次装了桌面端组件时执行；已有安装树内的桌面端进程时不重复拉起，避免开出两个窗口。
 * 入口一律走稳定 launcher 的 desktop 转发：它会脱离控制台并丢弃标准输入输出，因此既不占用
 * 安装会话的输出，也不会因为控制台被选中写满而把安装脚本卡住，后续升级换目录也不影响入口。
 * 提权重入的实例不调用本函数，改由外层未提权进程拉起，界面因此不会继承管理员令牌。
#>
function Start-AxonDesktop {
    if (-not $plan.InstallDesktop) { return }
    <#
     * 拉起失败只告警，绝不改写已经完成的安装：本函数在成功路径末尾被调用，
     * 一旦异常逃逸到外层 catch，会把「装好了」判成「装失败了」。
    #>
    try {
        $running = @(Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
                $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
            })
        if ($running.Count -gt 0) {
            Write-Host "[axon] 桌面端已在运行（PID $($running[0].Id)），不重复启动。" -ForegroundColor Cyan
            return
        }
        Start-Process -FilePath $launcherPath -ArgumentList "desktop" -WorkingDirectory $installRoot | Out-Null
        $launchDeadline = [DateTime]::UtcNow.AddSeconds(10)
        while ($true) {
            Start-Sleep -Milliseconds 200
            $started = @(Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
                    $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
                })
            if ($started.Count -gt 0) {
                Write-Host "[axon] 桌面端已启动（PID $($started[0].Id)）。" -ForegroundColor Cyan
                return
            }
            if ([DateTime]::UtcNow -gt $launchDeadline) {
                Write-Host "[axon] 桌面端未能在 10 秒内启动，请手动运行 axon desktop 排查原因。" -ForegroundColor Yellow
                return
            }
        }
    } catch {
        Write-Host "[axon] 拉起桌面端失败：$($_.Exception.Message)。请手动运行 axon desktop。" -ForegroundColor Yellow
    }
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
if ($ExpectedUserSid -and $identity.User.Value -ne $ExpectedUserSid) { throw "提权账号与原登录用户不同，已停止安装。" }
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
# UAC 提权只服务服务端安装：Daemon 注册要写计划任务；仅装桌面端不碰系统服务，直接以当前用户身份继续。
if ($plan.InstallBackend -and -not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    $logPath = Join-Path ([IO.Path]::GetTempPath()) "axon-source-install-$PID.log"
    $scriptPath = $PSCommandPath.Replace("'", "''")
    $componentArgs = [System.Collections.Generic.List[string]]::new()
    if ($plan.InstallBackend) { $componentArgs.Add("'backend'") }
    if ($plan.InstallDesktop) { $componentArgs.Add("'desktop'") }
    $componentsArg = $componentArgs -join ','
    $bootstrap = "Start-Transcript -LiteralPath '$($logPath.Replace("'", "''"))' -Force; try { & '$scriptPath' -ExpectedUserSid '$($identity.User.Value)' -Components $componentsArg; if (-not `$?) { exit 1 } } catch { Write-Output `$_; exit 1 } finally { Stop-Transcript }"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($bootstrap))
    Write-Host "[axon] 需要 UAC 授权，安装日志：$logPath" -ForegroundColor Cyan
    $process = Start-Process -FilePath (Join-Path $PSHOME "pwsh.exe") -ArgumentList @("-NoProfile", "-EncodedCommand", $encoded) -Verb RunAs -WindowStyle Hidden -Wait -PassThru
    if (Test-Path $logPath) { Get-Content $logPath }
    if ($process.ExitCode -ne 0) { throw "源码安装失败，退出码：$($process.ExitCode)" }
    # 提权只服务服务端安装：桌面端由本进程（原登录用户、未提权）拉起，界面不继承管理员令牌。
    Start-AxonDesktop
    return
}
$root = Split-Path -Parent $PSScriptRoot
$cargo = Get-Command cargo -CommandType Application -ErrorAction SilentlyContinue
if ($null -eq $cargo) {
    $cargoPath = Join-Path $HOME ".cargo\bin\cargo.exe"
    if (Test-Path $cargoPath) { $cargo = [pscustomobject]@{ Source = $cargoPath } }
}
if ($null -eq $cargo) { throw "未找到 cargo，请先安装 Rust 工具链。" }

function Wait-DaemonStopped([string]$path) {
    $deadline = [DateTime]::UtcNow.AddSeconds(60)
    while ([DateTime]::UtcNow -lt $deadline) {
        $rawStatus = (& $path daemon status 2>$null | Out-String).Trim()
        $status = $null
        if ($rawStatus) {
            try { $status = $rawStatus | ConvertFrom-Json -ErrorAction Stop } catch { $status = $null }
        }
        if (($null -ne $status -and -not $status.running) -or
            ($null -eq $status -and -not (Get-Process axon -ErrorAction SilentlyContinue))) { return $true }
        Start-Sleep -Milliseconds 200
    }
    return $false
}

function Set-AxonPath([string]$directory) {
    $directory = $directory.TrimEnd('\')
    $launcher = (Join-Path $installRoot "launcher").TrimEnd('\')
    $entries = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in (([Environment]::GetEnvironmentVariable("PATH", "User") ?? "") -split ";")) {
        $item = $entry.TrimEnd('\')
        if ($item -and $item -ine $launcher -and -not ($entries | Where-Object { $_ -ieq $item })) { $entries.Add($item) }
    }
    $null = $entries.RemoveAll([Predicate[string]]{ param($item) $item -ieq $directory })
    $entries.Insert(0, $directory)
    [Environment]::SetEnvironmentVariable("PATH", ($entries -join ";"), "User")
    $env:Path = "$directory;$(($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $launcher -and $_.TrimEnd('\') -ine $directory } | Select-Object -Unique) -join ';')"
}

try {
    Remove-Item $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stagingRoot | Out-Null
    <#
     * 准备阶段一律不动运行态。
     *
     * 编译要好几分钟，期间停掉 Daemon 等于把后台任务、定时调度与远控连接一起掐断；
     * 真正需要停机的只有「替换版本目录」那几步。凡是编译、自检、暂存目录拷贝这类
     * 工作，全部排在停机之前完成。
    #>
    $releaseBin = Join-Path $root "target\release"
    # 唯一必须提前停的是本仓库调试进程：cargo 要重写 target\release 里的同一批映像，按组件裁剪到本次要编的二进制。
    $rebuildNames = [System.Collections.Generic.List[string]]::new()
    if ($plan.InstallBackend) { $rebuildNames.Add("axon") }
    if ($plan.InstallDesktop) { $rebuildNames.Add("axon-desktop") }
    $rebuildNames.Add("axon-launcher")
    Get-Process -Name $rebuildNames -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and ([IO.Path]::GetDirectoryName($_.Path) -ieq $releaseBin)
    } | Stop-Process -Force
    if ($plan.InstallDesktop) {
        Push-Location (Join-Path $root "crates\axon-desktop\ui")
        try {
            & npm ci
            if ($LASTEXITCODE -ne 0) { throw "前端依赖安装失败" }
            & npm run build
            if ($LASTEXITCODE -ne 0) { throw "前端构建失败" }
        } finally { Pop-Location }
    }
    $packages = [System.Collections.Generic.List[string]]::new()
    if ($plan.InstallBackend) { $packages.Add("axon-cli") }
    if ($plan.InstallDesktop) { $packages.Add("axon-desktop") }
    $packages.Add("axon-launcher")
    $buildArgs = [System.Collections.Generic.List[string]]::new()
    foreach ($argument in @("build", "--release", "--locked")) { $buildArgs.Add($argument) }
    foreach ($package in $packages) { $buildArgs.Add("-p"); $buildArgs.Add($package) }
    Write-Host "[axon] 正在编译 $($packages -join '、')。" -ForegroundColor Cyan
    Push-Location $root
    try {
        & $cargo.Source @buildArgs
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败，退出码：$LASTEXITCODE" }
    } finally { Pop-Location }
    $cli = Join-Path $releaseBin "axon.exe"
    $desktop = Join-Path $releaseBin "axon-desktop.exe"
    $builtLauncher = Join-Path $releaseBin "axon-launcher.exe"
    $builtBinaries = [System.Collections.Generic.List[string]]::new()
    $builtBinaries.Add($builtLauncher)
    if ($plan.InstallBackend) { $builtBinaries.Add($cli) }
    if ($plan.InstallDesktop) { $builtBinaries.Add($desktop) }
    foreach ($path in $builtBinaries) { if (-not (Test-Path $path)) { throw "编译完成但缺少二进制：$path" } }
    if ($plan.InstallBackend) {
        & $cli --version *> $null
        if ($LASTEXITCODE -ne 0) { throw "CLI 自检失败" }
        $version = (& $cli --version).ToString().Trim().Split()[-1]
    } else {
        <#
         * 仅装桌面端时不编译 CLI，版本号取同一份 Cargo 清单里 axon-cli 的版本，
         * 与 axon --version 的输出同源，避免版本目录命名与 CLI 版本分叉。
        #>
        $metadataJson = (& $cargo.Source metadata --no-deps --format-version 1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { throw "cargo metadata 失败，退出码：$LASTEXITCODE" }
        $cliPackage = ($metadataJson | ConvertFrom-Json).packages | Where-Object { $_.name -eq "axon-cli" } | Select-Object -First 1
        if (-not $cliPackage) { throw "cargo metadata 中缺少 axon-cli 包，无法确定版本号" }
        $version = [string]$cliPackage.version
    }
    if ($plan.InstallDesktop) {
        <#
         * 桌面端是 GUI 子系统二进制：PowerShell 用 & 调用不会等待进程结束，
         * $LASTEXITCODE 保留的是上一条命令的值，自检会空转、永远判不出失败。
         * 这里必须 Start-Process -Wait 读 ExitCode，并把单行 JSON 留在暂存目录里备查。
        #>
        $selfCheckOut = Join-Path $stagingRoot "desktop-self-check.json"
        $selfCheckErr = Join-Path $stagingRoot "desktop-self-check.err"
        $selfCheckArgs = @{
            FilePath               = $desktop
            ArgumentList           = "--self-check"
            Wait                   = $true
            PassThru               = $true
            NoNewWindow            = $true
            RedirectStandardOutput = $selfCheckOut
            RedirectStandardError  = $selfCheckErr
        }
        $selfCheck = Start-Process @selfCheckArgs
        if ($selfCheck.ExitCode -ne 0) {
            $detail = Get-Content -LiteralPath $selfCheckOut -Raw -ErrorAction SilentlyContinue
            if (-not $detail) { $detail = Get-Content -LiteralPath $selfCheckErr -Raw -ErrorAction SilentlyContinue }
            $detail = ("$detail" -replace '\s+', ' ').Trim()
            throw "桌面端自检失败（退出码 $($selfCheck.ExitCode)）：$detail"
        }
    }
    $releaseId = $version
    $releaseDir = Join-Path $versionsDir $releaseId
    $stagedRelease = Join-Path $versionsDir ".staging.$PID"
    # 上一次中断留下的暂存目录先清掉；它不在任何版本的生效路径上，删它不影响运行态。
    Get-ChildItem -LiteralPath $versionsDir -Directory -Force -ErrorAction SilentlyContinue |
        Where-Object { $_.Name.StartsWith('.staging.') } |
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $stagedRelease, $launcherDir, $installDir | Out-Null
    # 暂存目录只放本次选中的组件，未选中的组件不会进入版本目录。
    if ($plan.InstallBackend) { Copy-Item $cli (Join-Path $stagedRelease "axon.exe") }
    if ($plan.InstallDesktop) { Copy-Item $desktop (Join-Path $stagedRelease "axon-desktop.exe") }

    <#
     * 停机窗口从这里开始，到版本目录替换完成为止。
     *
     * 顺序固定：先停 Daemon（它可能正在领取任务与写运行态），再停桌面端（它持有
     * versions 目录内的映像文件锁）。两者都停了，下面的删除与替换才不会被占用卡住。
     * 仅装桌面端时 Daemon 不参与本次替换，既不停止也不重启。
    #>
    if ($plan.InstallBackend -and $previousLauncher) {
        & schtasks.exe /change /tn AxonDaemon /disable 2>$null | Out-Null
        $taskDisabled = $true
        & $launcherPath daemon stop 2>$null | Out-Host
        if (-not (Wait-DaemonStopped $launcherPath)) { throw "Daemon 未能在 60 秒内停止" }
        $daemonStopped = $true
    }
    Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
    } | Stop-Process -Force

    $swapDeadline = [DateTime]::UtcNow.AddSeconds(30)
    while ($true) {
        $swapBlocked = $false
        try {
            if (Test-Path $releaseDir) { Remove-Item $releaseDir -Recurse -Force -ErrorAction Stop }
        } catch {
            $swapBlocked = $true
            Write-Host "[axon] 旧版本目录仍被占用，正在停止占用进程后重试：$($_.Exception.Message)" -ForegroundColor Yellow
        }
        if (-not $swapBlocked) { break }
        if ([DateTime]::UtcNow -gt $swapDeadline) { throw "旧版本目录 30 秒内仍被占用，无法完成替换：$releaseDir" }
        Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and $_.Path.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)
        } | Stop-Process -Force
        Start-Sleep -Milliseconds 500
    }
    Move-Item $stagedRelease $releaseDir
    <#
     * 旧版本目录属于残留，不参与本次生效路径：新版已经落位，删不掉只是多占一份磁盘。
     * Windows 在 Stop-Process 之后仍可能短暂持有映像句柄，这里按名再停一次并有限重试，
     * 仍失败只告警不中止，避免一次顺手的清理把已经装好的版本判成安装失败。
    #>
    Get-ChildItem -LiteralPath $versionsDir -Directory -ErrorAction SilentlyContinue |
        Where-Object { -not $_.Name.StartsWith('.') -and $_.Name -ne $releaseId } |
        ForEach-Object {
            $stale = $_
            $removed = $false
            for ($attempt = 0; $attempt -lt 5 -and -not $removed; $attempt++) {
                try {
                    Remove-Item $stale.FullName -Recurse -Force -ErrorAction Stop
                    $removed = $true
                } catch {
                    Get-Process axon-desktop -ErrorAction SilentlyContinue | Stop-Process -Force
                    Start-Sleep -Milliseconds 400
                }
            }
            if (-not $removed) {
                Write-Host "[axon] 旧版本目录仍被占用，已跳过清理：$($stale.Name)" -ForegroundColor Yellow
            }
        }
    Copy-Item $builtLauncher "$launcherPath.tmp.$PID" -Force
    Move-Item "$launcherPath.tmp.$PID" $launcherPath -Force
    $currentTmp = Join-Path $installRoot ".current.$PID.json"
    [IO.File]::WriteAllText($currentTmp, (@{ versionId = $releaseId } | ConvertTo-Json -Compress) + "`n", $utf8)
    Move-Item $currentTmp $currentFile -Force
    # 只清理本次安装组件在 cargo bin 目录里的同名残留，避免 PATH 遮蔽；未选中的组件不动。
    if ($plan.InstallBackend) { Remove-Item $cargoAxon -Force -ErrorAction SilentlyContinue }
    if ($plan.InstallDesktop) { Remove-Item $cargoDesktop -Force -ErrorAction SilentlyContinue }
    Set-AxonPath $installDir
    [IO.File]::WriteAllText((Join-Path $installDir "axon.cmd"), "@echo off`r`n`"$launcherPath`" %*`r`n", [Text.UTF8Encoding]::new($false))

    <#
     * 停机窗口到此结束：新版本已经就位，马上把后台服务带回来。
     * 配置初始化、Daemon 注册与启动都属于服务端组件的职责，仅装桌面端时整段跳过：
     * 不写 config.toml、不注册计划任务、不启动 Daemon。
    #>
    if ($plan.InstallBackend) {
        $configPath = Join-Path $HOME ".axon\config.toml"
        if (-not (Test-Path $configPath -PathType Leaf) -or (Get-Item $configPath).Length -eq 0) {
            & $launcherPath setup --provider-name openai --model gpt-4o --non-interactive --no-validate | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "Axon 默认配置初始化失败，退出码：$LASTEXITCODE" }
        }
        & $launcherPath daemon install | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Daemon 安装失败，退出码：$LASTEXITCODE" }
        & $launcherPath daemon start | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "Daemon 启动失败，退出码：$LASTEXITCODE" }
        $status = & $launcherPath daemon status | Out-String | ConvertFrom-Json
        if (-not $status.running) { throw "Daemon 未正常运行，请检查已注册服务和 data/logs 日志。" }
    } else {
        Write-Host "[axon] 仅安装桌面端：未写入 config.toml，未注册也未启动 Daemon。" -ForegroundColor Cyan
    }
    if (-not $plan.InstallDesktop) {
        Write-Host "[axon] 仅安装服务端：未铺放桌面端二进制（本脚本不创建桌面快捷方式）。" -ForegroundColor Cyan
    }
    Write-Host "[axon] 源码安装完成（$componentText）：$releaseDir" -ForegroundColor Green
    # 提权重入的实例到此为止：桌面端由外层未提权进程拉起，避免界面继承管理员令牌。
    if (-not $ExpectedUserSid) { Start-AxonDesktop }
} catch {
    if ($taskDisabled) { & schtasks.exe /change /tn AxonDaemon /enable 2>$null | Out-Null }
    if ($daemonStopped -and (Test-Path $launcherPath)) { & $launcherPath daemon start 2>$null | Out-Host }
    throw
} finally { Remove-Item $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue }
