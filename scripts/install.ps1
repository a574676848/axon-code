# Axon Windows 一键安装脚本
# 用法：irm https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.ps1 | iex
# 或者指定安装目录：
#   & ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/a574676848/axon-code/master/scripts/install.ps1'))) -InstallDir "$HOME\.axon\bin"
# 安装完成后同时安装并启动 Daemon 用户服务（不要求预先配置 Remote/Feishu）。
#
# 特点：
#   - 版本包安装到用户目录；Windows 当前用户最高权限任务注册需要 UAC 授权
#   - 自动持久写入用户级 PATH
#   - SHA256 校验完整性
#   - 幂等：重复执行升级到最新版本
param(
    [string]$InstallDir = "",
    [string]$Version = "",
    [string]$ConfigPath = ""
)

$ErrorActionPreference = "Stop"

# ── 常量 ──────────────────────────────────────────────────────────────────────
$Repo          = "a574676848/axon-code"
$BinaryName    = "axon.exe"
$SkillName     = "operate-axon"
$BundledSkillsDirectory = "docs\skills"
$BundledSoulPath = "assets\defaults\SOUL.md"
$GitHubApiBase = "https://api.github.com/repos/$Repo"
$AxonUserDir  = Join-Path $HOME ".axon"
$SkillsDir    = Join-Path $AxonUserDir "skills"
$SoulFile     = Join-Path $AxonUserDir "SOUL.md"
$IdentityDir  = Join-Path $AxonUserDir "identity"
$OwnerFile    = Join-Path $IdentityDir "owner.json"
$OwnerSchemaVersion = 1

if ($ConfigPath -eq "" -and $env:AXON_CONFIG_PATH) {
    $ConfigPath = $env:AXON_CONFIG_PATH
}

# ── 颜色输出 ──────────────────────────────────────────────────────────────────
function Write-Info    { param($Msg) Write-Host "[axon] $Msg" -ForegroundColor Cyan }
function Write-Success { param($Msg) Write-Host "[axon] $Msg" -ForegroundColor Green }
function Write-Warn    { param($Msg) Write-Host "[axon] $Msg" -ForegroundColor Yellow }
function Write-Fatal   { param($Msg) Write-Host "[axon] ERROR: $Msg" -ForegroundColor Red; exit 1 }

# ── CPU 架构检测 ───────────────────────────────────────────────────────────────
function Get-RustTarget {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    switch ($arch) {
        'X64'   { return "x86_64-pc-windows-msvc" }
        'Arm64' { return "aarch64-pc-windows-msvc" }
        default { Write-Fatal "不支持的 CPU 架构：$arch" }
    }
}

# ── 版本获取 ───────────────────────────────────────────────────────────────────
function Get-LatestVersion {
    try {
        $release = Invoke-RestMethod "$GitHubApiBase/releases/latest" `
            -Headers @{ "User-Agent" = "axon-installer" } `
            -TimeoutSec 30
        return $release.tag_name.TrimStart('v')
    } catch {
        Write-Fatal "无法获取最新版本：$_`n请检查网络连接或访问 https://github.com/$Repo/releases 手动下载。"
    }
}

# ── 文件下载 ───────────────────────────────────────────────────────────────────
function Download-File {
    param([string]$Url, [string]$Dest)
    Write-Info "下载：$Url"
    try {
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest $Url -OutFile $Dest -UseBasicParsing -TimeoutSec 300
    } catch {
        Write-Fatal "下载失败：$Url`n错误：$_"
    }
}

function Install-BundledSkill {
    param([string]$PackageRoot)

    $SkillSource = Join-Path $PackageRoot "$BundledSkillsDirectory\$SkillName"
    $SkillManifest = Join-Path $SkillSource "SKILL.md"
    if (-not (Test-Path $SkillManifest -PathType Leaf)) {
        Write-Warn "发行包未包含 $SkillName Skill，跳过 Skill 安装。"
        return
    }

    $SkillDest = Join-Path $SkillsDir $SkillName
    $SkillStaging = Join-Path $SkillsDir ".$SkillName.tmp.$PID"
    New-Item -ItemType Directory -Force $SkillsDir | Out-Null
    Remove-Item $SkillStaging -Recurse -Force -ErrorAction SilentlyContinue
    Copy-Item $SkillSource $SkillStaging -Recurse -Force
    Remove-Item $SkillDest -Recurse -Force -ErrorAction SilentlyContinue
    Move-Item $SkillStaging $SkillDest -Force
    Write-Success "$SkillName Skill 已安装到 $SkillDest"
}

function Install-BundledSoul {
    param([string]$PackageRoot)

    if (Test-Path -LiteralPath $SoulFile) {
        Write-Info "保留现有 Soul：$SoulFile"
        return
    }
    $SoulSource = Join-Path $PackageRoot $BundledSoulPath
    if (-not (Test-Path -LiteralPath $SoulSource -PathType Leaf)) {
        Write-Warn "发行包未包含默认 SOUL.md，跳过 Soul 初始化。"
        return
    }

    New-Item -ItemType Directory -Force $AxonUserDir | Out-Null
    Copy-Item -LiteralPath $SoulSource -Destination $SoulFile
    Write-Success "默认 Soul 已创建：$SoulFile"
}

function Initialize-OwnerIdentity {
    if (Test-Path -LiteralPath $OwnerFile) {
        Write-Info "保留现有 owner identity：$OwnerFile"
        return
    }
    New-Item -ItemType Directory -Force $IdentityDir | Out-Null
    $OwnerId = "owner_$([Guid]::NewGuid().ToString('D').ToLowerInvariant())"
    $Content = @{
        schemaVersion = $OwnerSchemaVersion
        ownerId = $OwnerId
    } | ConvertTo-Json -Compress
    $Staging = "$OwnerFile.tmp.$PID"
    [IO.File]::WriteAllText($Staging, $Content, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $Staging -Destination $OwnerFile -Force
    Write-Success "owner identity 已创建：$OwnerFile"
}

function Install-DaemonService {
    param([string]$AxonPath)
    Write-Info "正在注册并启动当前用户最高权限 Windows 计划任务 AxonDaemon..."
    $arguments = if ($ConfigPath -eq "") {
        @("daemon", "install")
    } else {
        @("--config", $ConfigPath, "daemon", "install")
    }
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if ($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        & $AxonPath @arguments
        $daemonExitCode = $LASTEXITCODE
    } else {
        $script = "if ([Security.Principal.WindowsIdentity]::GetCurrent().User.Value -ne '$($identity.User.Value)') { exit 1 }; & '$($AxonPath.Replace("'", "''"))'"
        foreach ($argument in $arguments) {
            $script += " '$($argument.Replace("'", "''"))'"
        }
        $script += "; exit `$LASTEXITCODE"
        $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($script))
        $pwsh = (Get-Command pwsh.exe -CommandType Application -ErrorAction Stop).Source
        $process = Start-Process -FilePath $pwsh -ArgumentList @("-NoProfile", "-EncodedCommand", $encoded) -Verb RunAs -WindowStyle Hidden -Wait -PassThru
        $daemonExitCode = $process.ExitCode
    }
    if ($daemonExitCode -ne 0) {
        Write-Fatal "Windows 用户计划任务安装失败，exit=$daemonExitCode。"
    }
    Write-Success "Windows 当前用户计划任务 AxonDaemon 已安装并启动；未配置远控时仍可运行通用 Session/Cron 调度。"
}

# ── SHA256 校验 ────────────────────────────────────────────────────────────────
function Test-Sha256 {
    param([string]$FilePath, [string]$Expected)
    $actual = (Get-FileHash $FilePath -Algorithm SHA256).Hash.ToLower()
    if ($actual -ne $Expected.Trim().ToLower()) {
        Write-Fatal "SHA256 校验失败！`n  期望：$Expected`n  实际：$actual`n请重新运行安装脚本。"
    }
    Write-Success "SHA256 校验通过。"
}

# ── PATH 持久写入 ──────────────────────────────────────────────────────────────
function Add-ToUserPath {
    param([string[]]$Dirs)
    $currentPath = [Environment]::GetEnvironmentVariable("PATH", "User") ?? ""
    $entries = [System.Collections.Generic.List[string]]::new()
    foreach ($entry in ($currentPath -split ";")) {
        if (-not [string]::IsNullOrWhiteSpace($entry)) {
            $entries.Add($entry.TrimEnd('\'))
        }
    }
    $processEntries = [System.Collections.Generic.List[string]]::new()
    foreach ($dir in $Dirs) {
        if ([string]::IsNullOrWhiteSpace($dir)) {
            continue
        }
        $normalized = $dir.TrimEnd('\')
        if (-not ($entries | Where-Object { $_ -ieq $normalized })) {
            $entries.Add($normalized)
        }
        if (-not ($processEntries | Where-Object { $_ -ieq $normalized })) {
            $processEntries.Add($normalized)
        }
    }
    [Environment]::SetEnvironmentVariable("PATH", ($entries -join ";"), "User")
    if ($processEntries.Count -gt 0) {
        $env:PATH = "$($processEntries -join ';');$env:PATH"
        Write-Success "已将以下目录添加到用户 PATH（新终端生效）：$($processEntries -join '、')"
    }
}

function Remove-ObsoleteReleases {
    param([string]$Root)
    $VersionsDir = Join-Path $Root "versions"
    $CurrentPath = Join-Path $Root "current.json"
    if (-not (Test-Path $VersionsDir -PathType Container) -or -not (Test-Path $CurrentPath -PathType Leaf)) {
        return
    }
    $VersionsFull = [IO.Path]::GetFullPath($VersionsDir).TrimEnd([IO.Path]::DirectorySeparatorChar)
    $Protected = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $Current = (Get-Content -LiteralPath $CurrentPath -Raw | ConvertFrom-Json).versionId
    if (-not [string]::IsNullOrWhiteSpace($Current)) {
        $null = $Protected.Add([IO.Path]::GetFullPath((Join-Path $VersionsDir $Current)))
    }
    $LeaseDir = Join-Path $Root "state\runtime-release-leases"
    if (Test-Path $LeaseDir -PathType Container) {
        Get-ChildItem -LiteralPath $LeaseDir -File -ErrorAction SilentlyContinue | ForEach-Object {
            $LeasePidText = [IO.Path]::GetFileNameWithoutExtension($_.Name)
            $LeasePid = 0
            if (-not [int]::TryParse($LeasePidText, [ref]$LeasePid) -or $LeasePid -le 0) {
                Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
                return
            }
            try {
                Get-Process -Id $LeasePid -ErrorAction Stop | Out-Null
            } catch {
                Remove-Item -LiteralPath $_.FullName -Force -ErrorAction SilentlyContinue
                return
            }
            $Target = (Get-Content -LiteralPath $_.FullName -Raw).Trim()
            if (-not [string]::IsNullOrWhiteSpace($Target)) {
                $TargetFull = [IO.Path]::GetFullPath($Target)
                if ($TargetFull.StartsWith("$VersionsFull$([IO.Path]::DirectorySeparatorChar)", [StringComparison]::OrdinalIgnoreCase)) {
                    $null = $Protected.Add($TargetFull)
                }
            }
        }
    }
    Get-ChildItem -LiteralPath $VersionsDir -Directory -ErrorAction SilentlyContinue |
        Where-Object { -not $_.Name.StartsWith('.') } |
        ForEach-Object {
            $ReleaseFull = [IO.Path]::GetFullPath($_.FullName)
            if (-not $Protected.Contains($ReleaseFull)) {
                try {
                    [IO.Directory]::Delete($ReleaseFull, $true)
                    Write-Info "已回收旧 Axon release：$($_.Name)"
                } catch {
                    Write-Warn "旧 Axon release 暂时无法回收：$($_.Name)，将在下次启动重试。"
                }
            }
        }
}

function Enter-InstallLock {
    param([string]$LockPath)

    New-Item -ItemType Directory -Force (Split-Path -Parent $LockPath) | Out-Null
    while ($true) {
        try {
            return [System.IO.File]::Open(
                $LockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
        } catch [System.IO.IOException] {
            Start-Sleep -Milliseconds 250
        }
    }
}

# ── 主流程 ────────────────────────────────────────────────────────────────────
function Main {
    Write-Info "正在检测平台..."
    $Target = Get-RustTarget
    Write-Info "目标平台：$Target"

    # 先解析稳定 PATH 入口，确保“已是最新版本”的幂等安装也会修复 PATH。
    if ($InstallDir -eq "") {
        $InstallDir = Join-Path $HOME ".axon\bin"
    }
    $InstallRoot = if ($env:AXON_INSTALL_ROOT) {
        $env:AXON_INSTALL_ROOT
    } else {
        Join-Path $HOME ".axon\bin"
    }
    $LauncherDir = Join-Path $InstallRoot "launcher"

    # 确定版本
    if ($Version -eq "") {
        Write-Info "正在获取最新版本..."
        $VersionToInstall = Get-LatestVersion
    } else {
        $VersionToInstall = $Version.TrimStart('v')
    }
    Write-Info "将安装版本：v$VersionToInstall"

    # 检查是否已是最新
    $axonCmd = Get-Command axon -ErrorAction SilentlyContinue
    if ($axonCmd) {
        try {
            $current = & axon --version 2>$null | Select-String -Pattern '(\d+\.\d+\.\d+)' |
                ForEach-Object { $_.Matches[0].Value }
            if ($current -eq $VersionToInstall) {
                $InstalledSkill = Join-Path $SkillsDir "$SkillName\SKILL.md"
                if ((Test-Path $InstalledSkill -PathType Leaf) -and
                    (Test-Path -LiteralPath $SoulFile -PathType Leaf)) {
                    Add-ToUserPath @($InstallDir, $LauncherDir)
                    Install-DaemonService -AxonPath $axonCmd.Source
                    Write-Success "Axon v$VersionToInstall、$SkillName Skill 和 Soul 均已安装，无需更新。"
                    return
                }
                Write-Info "Axon v$VersionToInstall 已是最新版本，将补装 $SkillName Skill..."
            } elseif ($current) {
                Write-Info "当前版本 v$current，将升级到 v$VersionToInstall..."
            }
        } catch { }
    }

    # 稳定 PATH 入口与不可变 release 根目录分离，避免更新覆盖运行中的 axon.exe。
    $VersionsDir = Join-Path $InstallRoot "versions"
    $CurrentFile = Join-Path $InstallRoot "current.json"
    $LauncherPath = Join-Path $LauncherDir "axon-launcher.exe"
    $InstallLockPath = Join-Path $InstallRoot "install.lock"

    # 构造下载 URL
    $ArchiveName = "axon-v$VersionToInstall-$Target.zip"
    $BaseUrl     = "https://github.com/$Repo/releases/download/v$VersionToInstall"
    $ArchiveUrl  = "$BaseUrl/$ArchiveName"
    $Sha256Url   = "$ArchiveUrl.sha256"

    # 临时目录
    $TmpDir = New-TemporaryFile | ForEach-Object { Remove-Item $_; New-Item -Type Directory $_.FullName }
    $InstallLock = Enter-InstallLock -LockPath $InstallLockPath
    try {
        # 下载
        $ArchivePath = Join-Path $TmpDir $ArchiveName
        $Sha256Path  = "$ArchivePath.sha256"
        Download-File $ArchiveUrl $ArchivePath
        Download-File $Sha256Url  $Sha256Path

        # 校验
        $expectedSha = (Get-Content $Sha256Path -Raw).Trim().ToLower()
        Test-Sha256 $ArchivePath $expectedSha

        # 解压
        Write-Info "正在解压..."
        Expand-Archive $ArchivePath -DestinationPath $TmpDir -Force

        # 找到二进制
        $ExtractedBin = Get-ChildItem $TmpDir -Recurse -Filter $BinaryName | Select-Object -First 1
        if (-not $ExtractedBin) {
            Write-Fatal "解压后未找到 $BinaryName，请联系开发者。"
        }

        # 发布完整版本目录；安装前已停止旧 Daemon 和桌面端。
        $ReleaseName = $VersionToInstall
        $ReleaseDir = Join-Path $VersionsDir $ReleaseName
        $StagingDir = Join-Path $VersionsDir ".staging.$ReleaseName.$PID"
        New-Item -ItemType Directory -Force $VersionsDir, $LauncherDir | Out-Null
        if (Test-Path -LiteralPath $LauncherPath -PathType Leaf) {
            & $LauncherPath daemon stop 2>$null | Out-Null
        }
        Get-Process axon-desktop -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and $_.Path.StartsWith("$InstallRoot\", [StringComparison]::OrdinalIgnoreCase)
        } | Stop-Process -Force
        Remove-Item -LiteralPath $StagingDir -Recurse -Force -ErrorAction SilentlyContinue
        New-Item -ItemType Directory -Force $StagingDir | Out-Null
        Copy-Item (Join-Path $ExtractedBin.Directory.FullName "*") $StagingDir -Recurse -Force
        $StagedAxonPath = Join-Path $StagingDir $BinaryName
        & $StagedAxonPath --version *> $null
        if ($LASTEXITCODE -ne 0) {
            Write-Fatal "staging 版本无法通过 axon --version 验证。"
        }
        if (Test-Path -LiteralPath $ReleaseDir -PathType Container) {
            Remove-Item -LiteralPath $ReleaseDir -Recurse -Force
        }
        Move-Item -LiteralPath $StagingDir -Destination $ReleaseDir

        if (-not (Test-Path $LauncherPath -PathType Leaf)) {
            $BundledLauncher = Join-Path $ExtractedBin.Directory.FullName "launcher\axon-launcher.exe"
            if (-not (Test-Path $BundledLauncher -PathType Leaf)) {
                Write-Fatal "发行包未包含 axon-launcher.exe。"
            }
            Copy-Item $BundledLauncher $LauncherPath
        }
        $currentJson = @{ versionId = $ReleaseName } | ConvertTo-Json -Compress
        $currentNext = Join-Path $InstallRoot ".current.$PID.json"
        [IO.File]::WriteAllText($currentNext, "$currentJson`n", [Text.UTF8Encoding]::new($false))
        Move-Item -Force $currentNext $CurrentFile
        New-Item -ItemType Directory -Force (Split-Path -Parent $InstallDir) | Out-Null
        New-Item -ItemType Directory -Force $InstallDir | Out-Null
        $DestPath = Join-Path $InstallDir "axon.cmd"
        $cmdContent = "@echo off`r`n`"$LauncherPath`" %*`r`n"
        [IO.File]::WriteAllText($DestPath, $cmdContent, [Text.UTF8Encoding]::new($false))
        Write-Success "Axon v$VersionToInstall 已发布到 $ReleaseDir"

        Install-BundledSkill -PackageRoot $ReleaseDir
        Install-BundledSoul -PackageRoot $ReleaseDir
        Initialize-OwnerIdentity

        # 写入 PATH
        Add-ToUserPath @($InstallDir, $LauncherDir)

        # 验证
        try {
            $versionOutput = & $LauncherPath --version 2>&1
            Write-Success "安装验证通过：$versionOutput"
        } catch {
            Write-Warn "安装完成，但 axon --version 执行异常，请检查 $DestPath。"
        }

        Install-DaemonService -AxonPath $LauncherPath
        Remove-ObsoleteReleases -Root $InstallRoot

        Write-Host ""
        Write-Success "安装完成！请重新打开终端，然后运行 'axon --help' 开始使用。"
        Write-Info "如需更新，重新运行本脚本或执行：axon upgrade --apply"
    } finally {
        $InstallLock.Dispose()
        Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Main
