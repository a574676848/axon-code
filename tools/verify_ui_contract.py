#!/usr/bin/env python3
"""桌面端 UI 契约门禁的跨平台实现。

Windows 由 verify-ui-contract.ps1 薄壳调用，Linux 直接以 python3 运行，
两端共用同一份判定逻辑，避免双实现漂移。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

REPO_ROOT = Path(__file__).resolve().parent.parent
UI_ROOT = REPO_ROOT / "crates" / "axon-desktop" / "ui"

DASH_RE = re.compile(r"[\u2013\u2014]")
SCROLL_RE = re.compile(r"addEventListener\(\s*['\"]scroll['\"]")
# 布局属性白名单的反面：这些属性一旦进入过渡，滚动与流式正文就会逐帧重排。
LAYOUT_PROPS = (
    r"(?:top|right|bottom|left|inset|width|height|min-width|max-width|min-height|max-height"
    r"|margin(?:-[a-z]+)?|padding(?:-[a-z]+)?|gap|row-gap|column-gap|border-width"
    r"|flex-basis|font-size|line-height|letter-spacing)"
)
LAYOUT_ANIM_RE = re.compile(r"(?:transition|animate)-\[" + LAYOUT_PROPS + r"\]")
# `transition-all` 没有列出属性，效果等于「所有布局属性一起动画」，与逐条写 width 等价。
TRANSITION_ALL_RE = re.compile(r"\btransition-all\b")
# CSS 与行内样式里的过渡声明：`transition: width 200ms` 这类写法绕开了 Tailwind 类名。
TRANSITION_DECL_RE = re.compile(r"(?m)(?:^|[;{\s])transition(?:-property)?\s*:\s*([^;{}\n]+)")
LAYOUT_TRANSITION_VALUE_RE = re.compile(r"(?<![a-z-])" + LAYOUT_PROPS + r"(?![a-z-])")
SPIN_RE = re.compile(r"animate-spin")
CDN_HOSTS = (
    "fonts.googleapis.com",
    "fonts.gstatic.com",
    "cdn.jsdelivr.net",
    "cdnjs.cloudflare.com",
    "unpkg.com",
)
FORBIDDEN_KEYFRAME_PROP_RE = re.compile(
    r"(?m)^\s*(?:top|right|bottom|left|width|height|min-width|min-height|max-width|max-height"
    r"|margin|margin-top|margin-right|margin-bottom|margin-left"
    r"|padding|padding-top|padding-right|padding-bottom|padding-left"
    r"|stroke-dasharray|stroke-dashoffset|background-position|box-shadow|filter)\s*:"
)
RULE_RE = re.compile(r"([^{}]+)\{([^{}]*)\}", re.S)
BRAND_RE = re.compile(r"\bdsh\b|deepseek-harness|deepseek harness", re.I)
TOKEN_RE = re.compile(r"(--color-[a-z0-9-]+)\s*:\s*(#[0-9a-fA-F]{6})\s*;")
# 兜底页与主界面之间比对的取值形态：颜色令牌还可能是 rgba(...) 写法。
ANY_TOKEN_RE = re.compile(r"(--color-[a-z0-9-]+)\s*:\s*([^;]+);")

# 承载文字的底色。`surface-sunken`（分段槽、输入托槽）与 `bubble`（用户气泡）
# 同样承载正文，因此与画布、卡片并列校验；只校这两档会让「换了凹槽底色之后
# 占位与三级文本掉到 4.5 以下」这类改动悄悄过关。
SURFACES = (
    "--color-canvas",
    "--color-surface",
    "--color-surface-sunken",
    "--color-bubble",
)
CONTRAST_CHECKS = (
    ("--color-text-primary", "主文本", 4.5),
    ("--color-text-prose", "叙述正文", 4.5),
    ("--color-text-secondary", "次要文本", 4.5),
    ("--color-text-tertiary", "三级文本", 4.5),
    ("--color-text-placeholder", "占位文本", 4.5),
    ("--color-text-caption", "说明与时间戳", 4.5),
    ("--color-text-link", "链接文本", 4.5),
    ("--color-skill-ink", "技能指令高亮", 4.5),
    ("--color-code-ink", "行内代码墨色", 4.5),
    ("--color-ok-ink", "增量计数文字", 4.5),
    ("--color-danger", "危险语义色", 4.5),
    ("--color-gold-ink", "琥珀告警文字", 4.5),
)


def read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def channel_luminance(value: float) -> float:
    if value <= 0.03928:
        return value / 12.92
    return ((value + 0.055) / 1.055) ** 2.4


def luminance(hex_color: str) -> float:
    raw = hex_color.lstrip("#")
    r = int(raw[0:2], 16) / 255.0
    g = int(raw[2:4], 16) / 255.0
    b = int(raw[4:6], 16) / 255.0
    return (
        0.2126 * channel_luminance(r)
        + 0.7152 * channel_luminance(g)
        + 0.0722 * channel_luminance(b)
    )


def contrast_ratio(foreground: str, background: str) -> float:
    l1 = luminance(foreground)
    l2 = luminance(background)
    brighter = max(l1, l2)
    darker = min(l1, l2)
    return round((brighter + 0.05) / (darker + 0.05), 2)


def token_map(block: str) -> dict[str, str]:
    return {m.group(1): m.group(2) for m in TOKEN_RE.finditer(block)}


def keyframe_body(text: str, start: int) -> str:
    depth = 1
    index = start
    while index < len(text) and depth > 0:
        char = text[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
        index += 1
    return text[start : max(start, index - 1)]


class Gate:
    def __init__(self) -> None:
        self.failed = False

    def pass_msg(self, message: str) -> None:
        print(f" [PASSED] ({message})")

    def fail(self, message: str) -> None:
        print(" [FAILED]")
        print(f"   {message}", file=sys.stderr)
        self.failed = True


def check_ui_contract() -> int:
    if not UI_ROOT.is_dir():
        print(f"找不到前端目录：{UI_ROOT}", file=sys.stderr)
        return 1

    src_root = UI_ROOT / "src"
    src_files = [
        path
        for path in sorted(src_root.rglob("*"))
        if path.is_file() and path.suffix in {".ts", ".tsx", ".css"}
    ]
    # 随包发布的静态页（应用入口与兜底页）同样受契约约束：它们不是 bundle 的一部分，
    # 因此源码扫描看不到它们，必须显式纳入。
    shell_pages: list[Path] = []
    index_html = UI_ROOT / "index.html"
    if index_html.is_file():
        shell_pages.append(index_html)
    public_root = UI_ROOT / "public"
    if public_root.is_dir():
        shell_pages.extend(
            path
            for path in sorted(public_root.rglob("*"))
            if path.is_file() and path.suffix in {".html", ".css", ".js"}
        )
    all_checked = src_files + shell_pages
    gate = Gate()

    print("=== Verifying UI Contract Gates ===")

    print("1. Checking zero-dash rule (no em-dash or en-dash)...", end="")
    hits = [str(path) for path in all_checked if DASH_RE.search(read_text(path))]
    if hits:
        gate.fail("发现禁用的破折号字符：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("All files strictly zero em/en-dash")

    print("2. Checking no window.addEventListener('scroll')...", end="")
    hits = [str(path) for path in src_files if SCROLL_RE.search(read_text(path))]
    if hits:
        gate.fail("发现禁用 scroll 监听：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("Zero scroll listeners; IntersectionObserver compliant")

    print("3. Checking animation properties (only transform and opacity)...", end="")
    hits = []
    for path in src_files + shell_pages:
        content = read_text(path)
        if LAYOUT_ANIM_RE.search(content):
            hits.append(f"{path.name}: 任意值类里出现布局属性")
        if TRANSITION_ALL_RE.search(content):
            hits.append(f"{path.name}: transition-all 会连带布局属性一起动画")
    if hits:
        gate.fail("布局属性动画禁用：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("Animations strictly limited to transform and opacity")

    print("3d. Checking transition declarations (no layout property)...", end="")
    hits = []
    for path in src_files + shell_pages:
        content = read_text(path)
        for match in TRANSITION_DECL_RE.finditer(content):
            value = match.group(1).strip()
            if value == "none":
                continue
            if LAYOUT_TRANSITION_VALUE_RE.search(value):
                line = content.count("\n", 0, match.start()) + 1
                hits.append(f"{path.name}:{line} transition: {value}")
    if hits:
        gate.fail("过渡里出现布局属性（会逐帧重排）：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("transition 只作用于合成器属性")

    print("3b. Checking @keyframes properties (no SVG geometry or box-model)...", end="")
    hits = []
    for path in src_files:
        if path.suffix != ".css":
            continue
        content = read_text(path)
        for match in re.finditer(r"@keyframes\s+([\w-]+)\s*\{", content):
            body = keyframe_body(content, match.end())
            if FORBIDDEN_KEYFRAME_PROP_RE.search(body):
                hits.append(f"{path.name}: @keyframes {match.group(1)}")
    if hits:
        gate.fail("键帧里出现非合成属性：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("@keyframes 只动 transform 与 opacity")

    print("3c. Checking single spinner implementation (no animate-spin)...", end="")
    hits = [path.name for path in src_files if SPIN_RE.search(read_text(path))]
    if hits:
        gate.fail("转圈必须走 .state-spinner，禁止 animate-spin：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("转圈只有 .state-spinner 一份实现")

    print("4. Checking zero external CDN dependencies...", end="")
    hits = []
    for path in all_checked:
        content = read_text(path)
        for host in CDN_HOSTS:
            if host in content:
                hits.append(f"{path} matches {host}")
    if hits:
        gate.fail("发现外部 CDN 依赖：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("100% offline self-hosted; zero external CDN")

    print("5. Checking font contract (system stack only, zero bundled webfont)...", end="")
    tokens_css = src_root / "tokens.css"
    tokens_text = read_text(tokens_css) if tokens_css.is_file() else ""
    problems = []
    if "@font-face" in tokens_text:
        problems.append("tokens.css 仍在声明 @font-face")
    if not re.search(r"(?m)^\s*--font-sans:\s*-apple-system\b", tokens_text):
        problems.append("--font-sans 必须由系统字体栈 -apple-system 打头")
    if re.search(r"--font-sans:\s*['\"](Inter|Roboto|Arial)['\"]", tokens_text):
        problems.append("--font-sans 仍以禁止的字体族打头（Inter / Roboto / Arial）")
    if not re.search(r"(?m)^\s*--font-mono:[^;]*'Microsoft YaHei'", tokens_text):
        problems.append("--font-mono 缺少 Windows 中文兜底 'Microsoft YaHei'")
    public_root = UI_ROOT / "public"
    if public_root.is_dir():
        bundled = [
            path.name
            for path in public_root.rglob("*")
            if path.is_file() and path.suffix in {".woff2", ".woff", ".ttf", ".otf"}
        ]
        if bundled:
            problems.append("随包字体文件：" + ", ".join(bundled))
    if problems:
        gate.fail("字体契约不满足：\n     " + "\n     ".join(problems))
    else:
        gate.pass_msg("系统字体栈两支，零随包 webfont")

    print("6. Verifying Design Tokens Contrast Ratios (WCAG AA)...")
    base_match = re.search(r"@theme\s*\{(.*?)\n\}", tokens_text, re.S)
    if not base_match:
        print("   [FAILED] tokens.css 里找不到 @theme 块")
        gate.failed = True
    base_tokens = token_map(base_match.group(1)) if base_match else {}
    themes: dict[str, dict[str, str]] = {"纸白": dict(base_tokens)}
    overrides_by_theme: dict[str, dict[str, str]] = {}
    # 引号单双都要认：只认单引号时，把选择器改成双引号就能让整套主题从校验里消失。
    for match in re.finditer(
        r"html\[data-theme=['\"]([a-z0-9-]+)['\"]\]\s*\{(.*?)\n\s*\}", tokens_text, re.S
    ):
        name = match.group(1)
        overrides = token_map(match.group(2))
        resolved = dict(base_tokens)
        resolved.update(overrides)
        themes[name] = resolved
        overrides_by_theme[name] = overrides

    # 主题选择器必须带 `html` 前缀（特异性 0,1,1）：写成裸 `[data-theme=...]`
    # 与 `:root` 同级，只能靠源码顺序决胜，主题取值会随文件顺序静默失效。
    for match in re.finditer(r"(?<!html)\[data-theme=", tokens_text):
        line = tokens_text.count("\n", 0, match.start()) + 1
        print(f"   [FAILED] tokens.css:{line} 主题选择器必须以 html 前缀限定")
        gate.failed = True

    for name, overrides in overrides_by_theme.items():
        canvas = overrides.get("--color-canvas", "")
        if not canvas:
            print(f"   [FAILED] 主题 [{name}] 没有声明 --color-canvas")
            gate.failed = True
            continue
        if canvas == base_tokens.get("--color-canvas"):
            print(f"   [FAILED] 主题 [{name}] 的画布与默认主题相同，切换后看不出变化")
            gate.failed = True

    if not themes:
        print("   [FAILED] 一个主题都没解析出来")
        gate.failed = True

    for name, tokens in themes.items():
        print(f"   主题 [{name}]")
        for surface in SURFACES:
            surface_value = tokens.get(surface, "")
            if not surface_value:
                print(f"      [FAILED] 主题缺少底色令牌 {surface}")
                gate.failed = True
                continue
            for token, label, required in CONTRAST_CHECKS:
                value = tokens.get(token, "")
                if not value:
                    print(f"      [FAILED] 缺少令牌 {token}")
                    gate.failed = True
                    continue
                ratio = contrast_ratio(value, surface_value)
                if ratio < required:
                    print(
                        f"      {label}({token}) 对 {surface}: {ratio}:1 "
                        f"(要求 >= {required}) [FAILED]"
                    )
                    gate.failed = True
        focus = tokens.get("--color-focus", "")
        if not focus:
            print("      [FAILED] 缺少 --color-focus")
            gate.failed = True
        else:
            ratio = contrast_ratio(focus, tokens.get("--color-canvas", "#FFFFFF"))
            if ratio < 3.0:
                print(f"      --color-focus 对画布: {ratio}:1 (要求 >= 3) [FAILED]")
                gate.failed = True
        primary = tokens.get("--color-primary", "")
        if not primary:
            print("      [FAILED] 缺少 --color-primary")
            gate.failed = True
        else:
            ratio = contrast_ratio(primary, "#FFFFFF")
            if ratio < 4.5:
                print(f"      主按钮白字: {ratio}:1 (要求 >= 4.5) [FAILED]")
                gate.failed = True
    print(f"   共校验 {len(themes)} 套主题，逐主题通过则本条通过")

    print("7. Checking zero third-party design-system brand wording in desktop sources...")
    brand_hits: list[str] = []
    brand_files: list[Path] = []
    for rel in ("crates/axon-desktop/src", "crates/axon-desktop/ui/src"):
        scan_root = REPO_ROOT / rel
        if not scan_root.is_dir():
            continue
        brand_files.extend(
            path
            for path in sorted(scan_root.rglob("*"))
            if path.is_file() and path.suffix in {".rs", ".ts", ".tsx", ".css"}
        )
    # 入口页与兜底页同样是桌面端界面，品牌字样同样不许出现。
    brand_files.extend(shell_pages)
    for path in brand_files:
        for number, line in enumerate(read_text(path).splitlines(), start=1):
            if BRAND_RE.search(line):
                brand_hits.append(f"{path}:{number}")
    if brand_hits:
        print("   [FAILED] 源码中残留品牌字样：")
        for hit in brand_hits[:5]:
            print(f"     {hit}")
        gate.failed = True
    else:
        print("   [PASSED] (桌面端源码零第三方设计系统品牌字样)")

    print("8. Checking no transform animation on display:inline rules...", end="")
    hits = []
    for path in src_files:
        for rule in RULE_RE.finditer(read_text(path)):
            selector, body = rule.group(1), rule.group(2)
            if not re.search(r"display\s*:\s*inline\s*;", body):
                continue
            if not re.search(r"(animation|transition)[^;]*transform", body):
                continue
            hits.append(f"{path.name}: {re.sub(r'\s+', ' ', selector.strip())}")
    if hits:
        gate.fail("display:inline 元素上声明了 transform 动画：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("行内元素只做 opacity，不做 transform")

    print("9. Checking backdrop-filter is never animated...", end="")
    hits = []
    for path in src_files:
        for rule in RULE_RE.finditer(read_text(path)):
            selector, body = rule.group(1), rule.group(2)
            if not re.search(r"(animation|transition)[^;]*backdrop-filter", body):
                continue
            hits.append(f"{path.name}: {re.sub(r'\s+', ' ', selector.strip())}")
    if hits:
        gate.fail("遮罩的 backdrop-filter 被动画化：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("模糊半径保持常量，遮罩只动 opacity")

    print("10. Checking theme transition lock (.disable-transitions)...", end="")
    if ".disable-transitions" in tokens_text:
        gate.pass_msg("tokens.css 含 .disable-transitions 瞬切锁")
    else:
        gate.fail("tokens.css 缺少 .disable-transitions 主题瞬切锁")

    print("11. Checking fallback page tokens match tokens.css...", end="")
    error_page = public_root / "error.html"
    if not error_page.is_file():
        gate.pass_msg("没有兜底页，跳过")
    else:
        page_text = read_text(error_page)

        def normalize(value: str) -> str:
            return value.replace(" ", "").upper()

        base_any = {k: v.strip() for k, v in ANY_TOKEN_RE.findall(base_match.group(1))} if base_match else {}
        themes_any: dict[str, dict[str, str]] = {"paper": dict(base_any)}
        for match in re.finditer(
            r"html\[data-theme=['\"]([a-z0-9-]+)['\"]\]\s*\{(.*?)\n\s*\}", tokens_text, re.S
        ):
            resolved = dict(base_any)
            resolved.update({k: v.strip() for k, v in ANY_TOKEN_RE.findall(match.group(2))})
            themes_any[match.group(1)] = resolved

        problems: list[str] = []
        page_root = re.search(r":root\s*\{(.*?)\n\s*\}", page_text, re.S)
        page_base = {k: v.strip() for k, v in ANY_TOKEN_RE.findall(page_root.group(1))} if page_root else {}
        for name, value in page_base.items():
            declared = base_any.get(name)
            if declared is not None and normalize(value) != normalize(declared):
                problems.append(f":root {name} = {value}，tokens.css 为 {declared}")
        for name, resolved in themes_any.items():
            if name == "paper":
                continue
            block = re.search(
                r"html\[data-theme=['\"]%s['\"]\]\s*\{(.*?)\n\s*\}" % re.escape(name), page_text, re.S
            )
            if block is None:
                problems.append(f"兜底页缺少主题 [{name}]，该主题在兜底页看不清")
                continue
            for token, value in ANY_TOKEN_RE.findall(block.group(1)):
                declared = resolved.get(token)
                if declared is not None and normalize(value.strip()) != normalize(declared):
                    problems.append(f"[{name}] {token} = {value.strip()}，tokens.css 为 {declared}")
        if problems:
            gate.fail("兜底页与 tokens.css 的取值已漂移：\n     " + "\n     ".join(problems))
        else:
            gate.pass_msg("兜底页与 tokens.css 取值一致")

    if gate.failed:
        print("UI contract verification failed.", file=sys.stderr)
        return 1

    dist_entry = UI_ROOT / "dist" / "index.html"
    if dist_entry.is_file():
        dist_time = dist_entry.stat().st_mtime
        # 产物新鲜度要覆盖全部构建输入：入口页、构建配置与随包静态页同样是 dist 的来源，
        # 只看 src 会让「改了 index.html 或 public/ 却忘了重建」悄悄过关。
        inputs = [path for path in src_root.rglob("*") if path.is_file()]
        for extra in ("index.html", "vite.config.ts", "package.json", "tsconfig.json"):
            candidate = UI_ROOT / extra
            if candidate.is_file():
                inputs.append(candidate)
        if public_root.is_dir():
            inputs.extend(path for path in public_root.rglob("*") if path.is_file())
        newest = max(inputs, key=lambda path: path.stat().st_mtime, default=None)
        if newest is not None and newest.stat().st_mtime > dist_time:
            print("   [FAILED] ui/dist 比前端源码旧：先 npm run build 再构建桌面端")
            print(f"     源码: {newest}")
            print(f"     产物: {dist_entry}")
            print("UI contract verification failed.", file=sys.stderr)
            return 1
        print("   [PASSED] (前端产物不比源码旧)")
    else:
        print("   [SKIPPED] (尚未构建 ui/dist)")

    print("=== UI CONTRACT ALL GATES PASSED ===")
    return 0


if __name__ == "__main__":
    sys.exit(check_ui_contract())
