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

# 6. Design tokens: 按主题块解析，逐主题实算对比度
#    旧实现把整个文件扫进一个扁平哈希表，同名令牌后写覆盖先写：
#    多主题共存时会静默只校验最后一个主题，因此改为按块解析并逐主题求值。
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

function Get-ContrastRatio([string]$foreground, [string]$background) {
    $l1 = Get-Luminance $foreground
    $l2 = Get-Luminance $background
    $brightest = [Math]::Max($l1, $l2)
    $darkest = [Math]::Min($l1, $l2)
    return [Math]::Round(($brightest + 0.05) / ($darkest + 0.05), 2)
}

function Get-TokenMap([string]$block) {
    $map = @{}
    foreach ($match in [regex]::Matches($block, '(--color-[a-z0-9-]+)\s*:\s*(#[0-9a-fA-F]{6})\s*;')) {
        $map[$match.Groups[1].Value] = $match.Groups[2].Value
    }
    return $map
}

$tokenFile = Join-Path $PSScriptRoot "..\crates\axon-desktop\ui\src\tokens.css"
$tokenText = Get-Content $tokenFile -Raw

$baseMatch = [regex]::Match($tokenText, '(?s)@theme\s*\{(.*?)\n\}')
if (-not $baseMatch.Success) {
    Write-Host "   [FAILED] tokens.css 里找不到 @theme 块" -ForegroundColor Red
    $failed = $true
}
$baseTokens = Get-TokenMap $baseMatch.Groups[1].Value

$themeMaps = [ordered]@{}
$themeOverrides = @{}
$themeMaps['纸白'] = $baseTokens
foreach ($match in [regex]::Matches($tokenText, "(?s)html\[data-theme='([a-z0-9-]+)'\]\s*\{(.*?)\n\s*\}")) {
    $name = $match.Groups[1].Value
    $overrides = Get-TokenMap $match.Groups[2].Value
    $resolved = @{}
    foreach ($key in $baseTokens.Keys) { $resolved[$key] = $baseTokens[$key] }
    foreach ($key in $overrides.Keys) { $resolved[$key] = $overrides[$key] }
    $themeMaps[$name] = $resolved
    $themeOverrides[$name] = $overrides
}

# 主题不能是空覆盖：只换了名字、值仍是纸白的那一套，对比度当然全过，
# 但用户在设置里切过去看不到任何变化。画布必须与默认主题不同。
foreach ($name in $themeOverrides.Keys) {
    $overrides = $themeOverrides[$name]
    $canvas = $overrides["--color-canvas"]
    if ([string]::IsNullOrEmpty($canvas)) {
        Write-Host ("   [FAILED] 主题 [" + $name + "] 没有声明 --color-canvas") -ForegroundColor Red
        $failed = $true
        continue
    }
    if ($canvas -eq $baseTokens["--color-canvas"]) {
        Write-Host ("   [FAILED] 主题 [" + $name + "] 的画布与默认主题相同，切换后看不出变化") -ForegroundColor Red
        $failed = $true
    }
}

if ($themeMaps.Count -eq 0) {
    Write-Host "   [FAILED] 一个主题都没解析出来" -ForegroundColor Red
    $failed = $true
}

# 承载文字的底色只有画布与卡片两档，二者都校验。
$surfaces = @("--color-canvas", "--color-surface")
$contrastChecks = @(
    @{ Token = "--color-text-primary"; Label = "主文本"; Req = 4.5 },
    @{ Token = "--color-text-secondary"; Label = "次要文本"; Req = 4.5 },
    @{ Token = "--color-text-tertiary"; Label = "三级文本"; Req = 4.5 },
    @{ Token = "--color-text-placeholder"; Label = "占位文本"; Req = 4.5 },
    @{ Token = "--color-text-caption"; Label = "说明与时间戳"; Req = 4.5 },
    @{ Token = "--color-text-link"; Label = "链接文本"; Req = 4.5 },
    @{ Token = "--color-skill-ink"; Label = "技能指令高亮"; Req = 4.5 },
    @{ Token = "--color-ok-ink"; Label = "增量计数文字"; Req = 4.5 },
    @{ Token = "--color-danger"; Label = "危险语义色"; Req = 4.5 },
    @{ Token = "--color-gold-ink"; Label = "琥珀告警文字"; Req = 4.5 }
)

foreach ($themeName in $themeMaps.Keys) {
    $tokens = $themeMaps[$themeName]
    Write-Host ("   主题 [" + $themeName + "]") -ForegroundColor Cyan

    foreach ($surfaceToken in $surfaces) {
        $surfaceValue = $tokens[$surfaceToken]
        if ([string]::IsNullOrEmpty($surfaceValue)) {
            Write-Host ("      [FAILED] 主题缺少底色令牌 " + $surfaceToken) -ForegroundColor Red
            $failed = $true
            continue
        }
        foreach ($check in $contrastChecks) {
            $value = $tokens[$check.Token]
            if ([string]::IsNullOrEmpty($value)) {
                Write-Host ("      [FAILED] 缺少令牌 " + $check.Token) -ForegroundColor Red
                $failed = $true
                continue
            }
            $ratio = Get-ContrastRatio $value $surfaceValue
            $isOk = $ratio -ge $check.Req
            if (-not $isOk) {
                Write-Host ("      {0} 对 {1}: {2}:1 (要求 >= {3}) [FAILED]" -f $check.Token, $surfaceToken, $ratio, $check.Req) -ForegroundColor Red
                $failed = $true
            }
        }
    }

    # 聚焦环属于非文本元素，按 1.4.11 要求 3:1。
    $focusValue = $tokens["--color-focus"]
    if ([string]::IsNullOrEmpty($focusValue)) {
        Write-Host "      [FAILED] 缺少 --color-focus" -ForegroundColor Red
        $failed = $true
    } else {
        $ratio = Get-ContrastRatio $focusValue $tokens["--color-canvas"]
        if ($ratio -lt 3.0) {
            Write-Host ("      --color-focus 对画布: {0}:1 (要求 >= 3) [FAILED]" -f $ratio) -ForegroundColor Red
            $failed = $true
        }
    }

    # 主操作是实心填充，必须单独核算白字。
    $primaryValue = $tokens["--color-primary"]
    if ([string]::IsNullOrEmpty($primaryValue)) {
        Write-Host "      [FAILED] 缺少 --color-primary" -ForegroundColor Red
        $failed = $true
    } else {
        $ratio = Get-ContrastRatio $primaryValue "#FFFFFF"
        if ($ratio -lt 4.5) {
            Write-Host ("      主按钮白字: {0}:1 (要求 >= 4.5) [FAILED]" -f $ratio) -ForegroundColor Red
            $failed = $true
        }
    }
}

Write-Host ("   共校验 {0} 套主题，逐主题通过则本条通过" -f $themeMaps.Count) -ForegroundColor Green
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
