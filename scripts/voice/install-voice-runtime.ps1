<#
.SYNOPSIS
  安装 Axon 语音所需的第三方运行时与模型（sherpa-onnx + VAD/ASR/TTS）。

.DESCRIPTION
  由安装脚本自动调用，不需要用户手工下载。下载物进入运行态根：
    <RuntimeRoot>\voice\runtime\sherpa-onnx\<版本>\
    <RuntimeRoot>\voice\models\{vad,asr,tts}\<名称>\
  并生成符合 WP-02 的 manifest.json。已安装且版本一致时直接跳过。

.PARAMETER RuntimeRoot
  运行态根，默认取 AXON_RUNTIME_ROOT，否则 $HOME\.axon。

.PARAMETER Force
  忽略已完成标记，重新下载与生成清单。
#>
[CmdletBinding()]
param(
    [string]$RuntimeRoot,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if (-not $RuntimeRoot) {
    $RuntimeRoot = if ($env:AXON_RUNTIME_ROOT) { $env:AXON_RUNTIME_ROOT } else { Join-Path $HOME '.axon' }
}

$sherpaVersion = '2.0.1'
$runtimeDir = Join-Path $RuntimeRoot "voice\runtime\sherpa-onnx\$sherpaVersion"
$modelsDir = Join-Path $RuntimeRoot 'voice\models'
$downloads = Join-Path $RuntimeRoot 'voice\downloads'
$marker = Join-Path $RuntimeRoot 'voice\.provisioned.json'

$assets = @{
    Runtime = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v$sherpaVersion/sherpa-onnx-v$sherpaVersion-win-x64-shared-MD-RelWithDebInfo.tar.bz2"
    Vad     = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx'
    Asr     = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20.tar.bz2'
    Tts     = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-zh-hf-fanchen-C.tar.bz2'
}

$models = @(
    @{ Family = 'vad'; Name = 'silero-vad-v4'; Source = 'Vad'; Dir = 'silero-vad-v4'; SampleRate = 16000; RtfP50 = 0.01; RtfP95 = 0.02; RamMb = 50; License = 'MIT'; Languages = @('zh') },
    @{ Family = 'asr'; Name = 'sherpa-streaming-zh-en'; Source = 'Asr'; Dir = 'sherpa-onnx-streaming-zipformer-bilingual-zh-en-2023-02-20'; SampleRate = 16000; RtfP50 = 0.25; RtfP95 = 0.45; RamMb = 420; License = 'Apache-2.0'; Languages = @('zh', 'en') },
    @{ Family = 'tts'; Name = 'vits-zh-fanchen'; Source = 'Tts'; Dir = 'vits-zh-hf-fanchen-C'; SampleRate = 16000; RtfP50 = 0.20; RtfP95 = 0.40; RamMb = 300; License = 'Apache-2.0'; Languages = @('zh') }
)

function Get-Asset([string]$name, [string]$url) {
    New-Item -ItemType Directory -Force $downloads | Out-Null
    $file = Join-Path $downloads (Split-Path $url -Leaf)
    if (-not (Test-Path $file) -or (Get-Item $file).Length -eq 0) {
        Write-Host "[voice] 下载 $name：$url" -ForegroundColor Cyan
        & curl.exe -L --fail --retry 3 --silent --show-error -o $file $url
        if ($LASTEXITCODE -ne 0) { throw "下载失败：$url" }
    }
    return $file
}

function Expand-TarBz2([string]$archive, [string]$destination) {
    New-Item -ItemType Directory -Force $destination | Out-Null
    & tar -xjf $archive -C $destination
    if ($LASTEXITCODE -ne 0) { throw "解压失败：$archive" }
}

function Write-Manifest([hashtable]$model, [string]$targetDir) {
    $files = Get-ChildItem -LiteralPath $targetDir -File -Recurse |
        Where-Object { $_.Name -ne 'manifest.json' } |
        Sort-Object FullName
    if ($files.Count -eq 0) { throw "模型目录为空：$targetDir" }

    $entries = foreach ($file in $files) {
        $relative = $file.FullName.Substring($targetDir.Length + 1).Replace('\', '/')
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        [ordered]@{ path = $relative; sha256 = $hash; bytes = $file.Length }
    }

    $manifest = [ordered]@{
        family     = $model.Family
        name       = $model.Name
        version    = '1.0.0'
        runtime    = 'sherpa-onnx'
        sampleRate = $model.SampleRate
        languages  = $model.Languages
        license    = $model.License
        rtfP50     = $model.RtfP50
        rtfP95     = $model.RtfP95
        ramMb      = $model.RamMb
        files      = @($entries)
    }
    $json = $manifest | ConvertTo-Json -Depth 6
    Set-Content -LiteralPath (Join-Path $targetDir 'manifest.json') -Value $json -Encoding utf8NoBOM
}

if (-not $Force -and (Test-Path $marker)) {
    $state = Get-Content -LiteralPath $marker -Raw | ConvertFrom-Json
    if ($state.sherpaVersion -eq $sherpaVersion -and $state.models -eq 3) {
        Write-Host "[voice] 运行时与模型已安装（$sherpaVersion），跳过。" -ForegroundColor DarkGray
        return
    }
}

New-Item -ItemType Directory -Force $runtimeDir, $modelsDir | Out-Null

$runtimeArchive = Get-Asset 'sherpa-onnx 运行时' $assets.Runtime
if (-not (Test-Path (Join-Path $runtimeDir 'sherpa-onnx.exe'))) {
    $staging = Join-Path $RuntimeRoot 'voice\runtime\.staging'
    if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
    Expand-TarBz2 $runtimeArchive $staging
    $inner = Get-ChildItem -LiteralPath $staging -Directory | Select-Object -First 1
    if (-not $inner) { throw '运行时压缩包结构异常' }
    if (Test-Path $runtimeDir) { Remove-Item $runtimeDir -Recurse -Force }
    Move-Item -LiteralPath $inner.FullName -Destination $runtimeDir
    Remove-Item $staging -Recurse -Force
}

foreach ($model in $models) {
    $target = Join-Path $modelsDir (Join-Path $model.Family $model.Name)
    if (Test-Path (Join-Path $target 'manifest.json')) { continue }

    $staging = Join-Path $RuntimeRoot "voice\models\.staging-$($model.Name)"
    if (Test-Path $staging) { Remove-Item $staging -Recurse -Force }
    New-Item -ItemType Directory -Force $staging | Out-Null

    $archive = Get-Asset $model.Name $assets[$model.Source]
    if ($archive.EndsWith('.onnx')) {
        Copy-Item $archive (Join-Path $staging 'silero_vad.onnx')
    } else {
        Expand-TarBz2 $archive $staging
        $inner = Get-ChildItem -LiteralPath $staging -Directory | Select-Object -First 1
        if (-not $inner) { throw "模型包结构异常：$($model.Name)" }
        Get-ChildItem -LiteralPath $inner.FullName -Force | Move-Item -Destination $staging
        Remove-Item $inner.FullName -Recurse -Force
    }

    if (Test-Path $target) { Remove-Item $target -Recurse -Force }
    New-Item -ItemType Directory -Force (Split-Path $target -Parent) | Out-Null
    Move-Item -LiteralPath $staging -Destination $target
    Write-Manifest $model $target
    Write-Host "[voice] 已安装模型 $($model.Family)/$($model.Name)" -ForegroundColor Green
}

@{ sherpaVersion = $sherpaVersion; models = $models.Count; installedAt = (Get-Date).ToString('o') } |
    ConvertTo-Json | Set-Content -LiteralPath $marker -Encoding utf8NoBOM
Write-Host "[voice] 语音运行时与模型就绪：$RuntimeRoot\voice" -ForegroundColor Green
