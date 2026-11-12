#!/usr/bin/env python3
"""桌面端 UI 契约门禁的跨平台实现。

Windows 由 verify-ui-contract.ps1 薄壳调用，Linux 直接以 python3 运行，
两端共用同一份判定逻辑，避免双实现漂移。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
UI_ROOT = REPO_ROOT / "crates" / "axon-desktop" / "ui"

DASH_RE = re.compile(r"[\u2013\u2014]")
SCROLL_RE = re.compile(r"addEventListener\(\s*['\"]scroll['\"]")
LAYOUT_ANIM_RE = re.compile(r"(transition|animate)-\[(top|left|width|height|margin|padding)\]")
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

SURFACES = ("--color-canvas", "--color-surface")
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
    index_html = UI_ROOT / "index.html"
    all_checked = src_files + ([index_html] if index_html.is_file() else [])
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
    hits = [
        str(path)
        for path in src_files
        if LAYOUT_ANIM_RE.search(read_text(path))
    ]
    if hits:
        gate.fail("布局属性动画禁用：\n     " + "\n     ".join(hits))
    else:
        gate.pass_msg("Animations strictly limited to transform and opacity")

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
    for match in re.finditer(r"html\[data-theme='([a-z0-9-]+)'\]\s*\{(.*?)\n\s*\}", tokens_text, re.S):
        name = match.group(1)
        overrides = token_map(match.group(2))
        resolved = dict(base_tokens)
        resolved.update(overrides)
        themes[name] = resolved
        overrides_by_theme[name] = overrides

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
    for rel in ("crates/axon-desktop/src", "crates/axon-desktop/ui/src"):
        scan_root = REPO_ROOT / rel
        if not scan_root.is_dir():
            continue
        for path in sorted(scan_root.rglob("*")):
            if not path.is_file() or path.suffix not in {".rs", ".ts", ".tsx", ".css"}:
                continue
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

    if gate.failed:
        print("UI contract verification failed.", file=sys.stderr)
        return 1

    dist_entry = UI_ROOT / "dist" / "index.html"
    if dist_entry.is_file():
        dist_time = dist_entry.stat().st_mtime
        newest = max(
            (path for path in src_root.rglob("*") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            default=None,
        )
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
