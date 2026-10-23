param(
    [string]$WorkspaceRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

$ErrorActionPreference = "Stop"

Write-Host "=== Verifying UI Contract Gates ===" -ForegroundColor Cyan

$uiRoot = Join-Path $WorkspaceRoot "crates/axon-desktop/ui"
if (-not (Test-Path $uiRoot)) {
    Write-Error "Cannot find frontend directory: $uiRoot"
    exit 1
}

$srcFiles = Get-ChildItem -Path (Join-Path $uiRoot "src") -Recurse -Include *.ts, *.tsx, *.css
$htmlFile = Join-Path $uiRoot "index.html"
$allCheckedFiles = @($srcFiles) + @(Get-Item $htmlFile)

$failed = $false

# 1. Zero-dash assertion: forbid en-dash and em-dash characters
Write-Host "1. Checking zero-dash rule (no em-dash or en-dash)..." -NoNewline
$dashHits = @()
foreach ($file in $allCheckedFiles) {
    $content = Get-Content -LiteralPath $file.FullName -Raw -Encoding utf8
    if ($content -match "[\u2013\u2014]") {
        $dashHits += $file.FullName
    }
}
if ($dashHits.Count -gt 0) {
    Write-Host " [FAILED]" -ForegroundColor Red
    Write-Error ("Forbidden dash characters found in: `n" + ($dashHits -join "`n"))
    $failed = $true
} else {
    Write-Host " [PASSED] (All files strictly zero em/en-dash)" -ForegroundColor Green
}

# 2. No window.addEventListener('scroll')
Write-Host "2. Checking no window.addEventListener('scroll')..." -NoNewline
$scrollHits = @()
foreach ($file in $srcFiles) {
    $content = Get-Content -LiteralPath $file.FullName -Raw -Encoding utf8
    if ($content -match "addEventListener\(\s*['""]scroll['""]") {
        $scrollHits += $file.FullName
    }
}
if ($scrollHits.Count -gt 0) {
    Write-Host " [FAILED]" -ForegroundColor Red
    Write-Error ("Forbidden scroll listener found in: `n" + ($scrollHits -join "`n"))
    $failed = $true
} else {
    Write-Host " [PASSED] (Zero scroll listeners; IntersectionObserver compliant)" -ForegroundColor Green
}

# 3. No layout-property animations (only transform and opacity allowed)
Write-Host "3. Checking animation properties (only transform and opacity)..." -NoNewline
$animHits = @()
foreach ($file in $srcFiles) {
    $content = Get-Content -LiteralPath $file.FullName -Raw -Encoding utf8
    if ($content -match "transition-\[(top|left|width|height|margin|padding)\]" -or
        $content -match "animate-\[(top|left|width|height|margin|padding)\]") {
        $animHits += $file.FullName
    }
}
if ($animHits.Count -gt 0) {
    Write-Host " [FAILED]" -ForegroundColor Red
    Write-Error ("Layout animation forbidden in: `n" + ($animHits -join "`n"))
    $failed = $true
} else {
    Write-Host " [PASSED] (Animations strictly limited to transform and opacity)" -ForegroundColor Green
}

# 4. Zero external CDN dependencies (100% offline self-hosted)
Write-Host "4. Checking zero external CDN dependencies..." -NoNewline
$cdnPatterns = @("fonts.googleapis.com", "fonts.gstatic.com", "cdn.jsdelivr.net", "cdnjs.cloudflare.com", "unpkg.com")
$cdnHits = @()
foreach ($file in $allCheckedFiles) {
    $content = Get-Content -LiteralPath $file.FullName -Raw -Encoding utf8
    foreach ($pat in $cdnPatterns) {
        if ($content -match $pat) {
            $cdnHits += ("{0} matches {1}" -f $file.FullName, $pat)
        }
    }
}
if ($cdnHits.Count -gt 0) {
    Write-Host " [FAILED]" -ForegroundColor Red
    Write-Error ("External CDN dependencies detected: `n" + ($cdnHits -join "`n"))
    $failed = $true
} else {
    Write-Host " [PASSED] (100% offline self-hosted; zero external CDN)" -ForegroundColor Green
}

# 5. 字体契约：界面只走系统字体栈，零随包 webfont
Write-Host "5. Checking font contract (system stack only, zero bundled webfont)..." -NoNewline
$tokensCss = Join-Path $uiRoot "src/tokens.css"
$tokensContent = Get-Content -LiteralPath $tokensCss -Raw -Encoding utf8
$fontProblems = @()
if ($tokensContent -match "@font-face") {
    $fontProblems += "tokens.css 仍在声明 @font-face（界面只走系统字体栈）"
}
if ($tokensContent -notmatch "(?m)^\s*--font-sans:\s*-apple-system\b") {
    $fontProblems += "--font-sans 必须由系统字体栈 -apple-system 打头"
}
if ($tokensContent -match "--font-sans:\s*['""](Inter|Roboto|Arial)['""]") {
    $fontProblems += "--font-sans 仍以禁止的字体族打头（Inter / Roboto / Arial）"
}
if ($tokensContent -notmatch "(?m)^\s*--font-mono:[^;]*'Microsoft YaHei'") {
    $fontProblems += "--font-mono 缺少 Windows 中文兜底 'Microsoft YaHei'"
}
$bundledFonts = @(Get-ChildItem (Join-Path $uiRoot "public") -Recurse -File -Include *.woff2, *.woff, *.ttf, *.otf -ErrorAction SilentlyContinue)
if ($bundledFonts.Count -gt 0) {
    $fontProblems += ("随包字体文件：" + (($bundledFonts | ForEach-Object { $_.Name }) -join ", "))
}
if ($fontProblems.Count -gt 0) {
    Write-Host " [FAILED]" -ForegroundColor Red
    Write-Error ("字体契约不满足：`n" + ($fontProblems -join "`n"))
    $failed = $true
} else {
    Write-Host " [PASSED] (系统字体栈两支，零随包 webfont)" -ForegroundColor Green
}

# 6. Design tokens: 逐项从真实令牌文件读取并实算对比度
#    令牌值不再硬编码在脚本里，避免令牌改名后门禁悄悄空转。
Write-Host "6. Verifying Design Tokens Contrast Ratios (WCAG AA)..."
function Get-Luminance([string]$hex) {
    $hex = $hex.TrimStart('#')
    $r = [Convert]::ToInt32($hex.Substring(0, 2), 16) / 255.0
    $g = [Convert]::ToInt32($hex.Substring(2, 2), 16) / 255.0
    $b = [Convert]::ToInt32($hex.Substring(4, 2), 16) / 255.0
    $calc = {
        param($c)
        if ($c -le 0.03928) { return $c / 12.92 }
        return [Math]::Pow(($c + 0.055) / 1.055, 2.4)
    }
    $rl = & $calc $r
    $gl = & $calc $g
    $bl = & $calc $b
    return (0.2126 * $rl + 0.7152 * $gl + 0.0722 * $bl)
}

function Get-ContrastRatio([string]$hex1, [string]$hex2) {
    $l1 = Get-Luminance $hex1
    $l2 = Get-Luminance $hex2
    $brightest = [Math]::Max($l1, $l2)
    $darkest = [Math]::Min($l1, $l2)
    return [Math]::Round(($brightest + 0.05) / ($darkest + 0.05), 2)
}

$tokenFile = Join-Path $PSScriptRoot "..\crates\axon-desktop\ui\src\tokens.css"
$tokenText = Get-Content $tokenFile -Raw
$tokens = @{}
foreach ($match in [regex]::Matches($tokenText, '(?m)^\s*(--color-[a-z0-9-]+)\s*:\s*(#[0-9a-fA-F]{6})\s*;')) {
    $tokens[$match.Groups[1].Value] = $match.Groups[2].Value
}

# 每一项都声明自己的判定基准：正文字符要求 4.5:1，非文本的聚焦环按 1.4.11 要求 3:1。
# 同一个前景落在多块底色上时各写一行，并用 Bg 指明底色令牌（缺省是画布）。
$contrastChecks = @(
    @{ Token = "--color-canvas"; Label = "画布底色"; Req = 0.0 },
    @{ Token = "--color-text-primary"; Label = "主文本"; Req = 4.5 },
    @{ Token = "--color-text-secondary"; Label = "次要文本"; Req = 4.5 },
    @{ Token = "--color-text-tertiary"; Label = "三级文本"; Req = 4.5 },
    @{ Token = "--color-text-placeholder"; Label = "占位文本"; Req = 4.5 },
    @{ Token = "--color-text-caption"; Label = "说明与时间戳"; Req = 4.5 },
    @{ Token = "--color-text-link"; Label = "链接文本"; Req = 4.5 },
    @{ Token = "--color-ok-ink"; Label = "增量计数文字"; Req = 4.5 },
    @{ Token = "--color-danger"; Label = "危险语义色"; Req = 4.5 },
    @{ Token = "--color-gold-ink"; Label = "琥珀告警文字"; Req = 4.5 },
    @{ Token = "--color-skill-ink"; Label = "技能指令高亮"; Req = 4.5 },
    @{ Token = "--color-skill-ink"; Label = "技能指令高亮"; Req = 4.5; Bg = "--color-bubble" },
    @{ Token = "--color-focus"; Label = "键盘聚焦环"; Req = 3.0 }
)

$canvas = $tokens["--color-canvas"]
if ([string]::IsNullOrEmpty($canvas)) {
    Write-Host "   [FAILED] 令牌文件缺少 --color-canvas，无法计算对比度" -ForegroundColor Red
    $failed = $true
}

foreach ($check in $contrastChecks) {
    $value = $tokens[$check.Token]
    if ([string]::IsNullOrEmpty($value)) {
        Write-Host ("   {0,-24}: [FAILED] 令牌文件中不存在该令牌" -f $check.Token) -ForegroundColor Red
        $failed = $true
        continue
    }
    if ($check.Req -eq 0.0) {
        Write-Host ("   {0,-24}: {1} (画布基准)" -f $check.Token, $value)
        continue
    }
    $bgToken = if ($check.ContainsKey("Bg")) { $check.Bg } else { "--color-canvas" }
    $bgValue = $tokens[$bgToken]
    if ([string]::IsNullOrEmpty($bgValue)) {
        Write-Host ("   {0,-24}: [FAILED] 令牌文件中不存在底色令牌 {1}" -f $check.Token, $bgToken) -ForegroundColor Red
        $failed = $true
        continue
    }
    $ratio = Get-ContrastRatio $value $bgValue
    $isOk = $ratio -ge $check.Req
    $status = if ($isOk) { "[PASSED]" } else { "[FAILED]" }
    $fgColor = if ($isOk) { "Green" } else { "Red" }
    $label = if ($bgToken -eq "--color-canvas") { $check.Label } else { "$($check.Label) 对 $bgToken" }
    Write-Host ("   {0,-14} {1,-24}: {2,5}:1 (要求 >={3}:1) {4}" -f $label, $check.Token, $ratio, $check.Req, $status) -ForegroundColor $fgColor
    if (-not $isOk) { $failed = $true }
}

# 主操作按钮使用白字实心填充，必须单独核算填充色与白字的对比度。
$primary = $tokens["--color-primary"]
if ([string]::IsNullOrEmpty($primary)) {
    Write-Host "   [FAILED] 令牌文件缺少 --color-primary" -ForegroundColor Red
    $failed = $true
} else {
    $ratio = Get-ContrastRatio $primary "#FFFFFF"
    $isOk = $ratio -ge 4.5
    $status = if ($isOk) { "[PASSED]" } else { "[FAILED]" }
    $fgColor = if ($isOk) { "Green" } else { "Red" }
    Write-Host ("   {0,-14} {1,-24}: {2,5}:1 (要求 >=4.5:1) {3}" -f "主按钮白字", "--color-primary", $ratio, $status) -ForegroundColor $fgColor
    if (-not $isOk) { $failed = $true }
}

# 7. 源码不得出现外部设计系统品牌字样
Write-Host "7. Checking zero third-party design-system brand wording in desktop sources..."
$brandPattern = '(?i)\bdsh\b|deepseek-harness|deepseek harness'
$brandHits = @()
foreach ($root in @("crates\axon-desktop\src", "crates\axon-desktop\ui\src")) {
    $scanRoot = Join-Path $PSScriptRoot "..\$root"
    if (-not (Test-Path $scanRoot)) { continue }
    foreach ($file in Get-ChildItem $scanRoot -Recurse -File -Include *.rs, *.ts, *.tsx, *.css) {
        $hit = Select-String -Path $file.FullName -Pattern $brandPattern -AllMatches
        if ($hit) { $brandHits += $hit }
    }
}
if ($brandHits.Count -gt 0) {
    Write-Host "   [FAILED] 源码中残留品牌字样：" -ForegroundColor Red
    $brandHits | Select-Object -First 5 | ForEach-Object { Write-Host ("     {0}:{1}" -f $_.Path, $_.LineNumber) }
    $failed = $true
} else {
    Write-Host "   [PASSED] (桌面端源码零第三方设计系统品牌字样)" -ForegroundColor Green
}

if ($failed) {
    Write-Error "UI contract verification failed."
    exit 1
}

# 前端产物必须比源码新：桌面端二进制把 ui/dist 嵌进去，改了前端却只跑 cargo build
# 会把旧界面打进安装包（表现为「代码明明改了，界面没变」）。产物不存在时跳过。
$uiRoot = Join-Path $PSScriptRoot "..\crates\axon-desktop\ui"
$distEntry = Join-Path $uiRoot "dist\index.html"
if (Test-Path $distEntry) {
    $distTime = (Get-Item $distEntry).LastWriteTimeUtc
    $newestSource = Get-ChildItem (Join-Path $uiRoot "src") -Recurse -File |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -ne $newestSource -and $newestSource.LastWriteTimeUtc -gt $distTime) {
        Write-Host "   [FAILED] ui/dist 比前端源码旧：先 npm run build 再构建桌面端" -ForegroundColor Red
        Write-Host ("     源码: {0} ({1:u})" -f $newestSource.FullName, $newestSource.LastWriteTimeUtc)
        Write-Host ("     产物: {0} ({1:u})" -f $distEntry, $distTime)
        Write-Error "UI contract verification failed."
        exit 1
    }
    Write-Host "   [PASSED] (前端产物不比源码旧)" -ForegroundColor Green
} else {
    Write-Host "   [SKIPPED] (尚未构建 ui/dist)" -ForegroundColor Yellow
}

Write-Host "=== UI CONTRACT ALL GATES PASSED ===" -ForegroundColor Green
exit 0
