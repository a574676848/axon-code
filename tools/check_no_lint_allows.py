#!/usr/bin/env python3
"""编译期 lint 豁免检查。

扫描 Rust 源码中的 `#![allow(...)]` 与 `#[allow(...)]`，只放行在
`tools/lint-allow-allowlist.txt` 中显式登记且带理由的条目。

设计约束：
- fail-closed：未登记的豁免一律失败，不返回模糊的「未发现问题」。
- 白名单必须精确匹配「文件 + lint 名称」，不接受通配，避免一次登记放过整类豁免。
- 登记后若源码中已不存在该豁免，同样失败，强制清理过期的白名单条目。
- 注释行（`//` 开头）不参与匹配，避免文档示例被误判。
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCAN_ROOTS = ("crates", "apps", "tools")
SKIP_SEGMENTS = ("/target/", "/node_modules/", "/.git/")
ALLOW_PATTERN = re.compile(r"^\s*#!?\[\s*allow\s*\(([^)]*)\)")
ALLOWLIST_PATH = Path(__file__).resolve().parent / "lint-allow-allowlist.txt"

# Windows runner 的 stdout 默认使用 cp1252，直接打印中文会抛 UnicodeEncodeError。
# 显式切到 UTF-8，使三平台输出一致，且不因 runner 的 locale 设置而失败。
for _stream in (sys.stdout, sys.stderr):
    _reconfigure = getattr(_stream, "reconfigure", None)
    if _reconfigure is not None:
        _reconfigure(encoding="utf-8", errors="replace")


def load_allowlist() -> dict[tuple[str, str], int]:
    """读取白名单，返回 (相对路径, lint) 到登记次数的映射。"""
    entries: dict[tuple[str, str], int] = {}
    if not ALLOWLIST_PATH.exists():
        return entries

    for number, raw in enumerate(ALLOWLIST_PATH.read_text(encoding="utf-8").splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = [part.strip() for part in line.split("|")]
        if len(parts) < 3 or not parts[0] or not parts[1] or not parts[2]:
            raise SystemExit(
                f"白名单格式错误（第 {number} 行）：需要「相对路径|lint 名称|理由」三段，实际为：{raw}"
            )
        key = (parts[0], parts[1])
        entries[key] = 0
    return entries


def iter_rust_files():
    """遍历需要检查的 Rust 源码，产出 (绝对路径, 仓库相对路径)。"""
    for root_name in SCAN_ROOTS:
        base = ROOT / root_name
        if not base.is_dir():
            continue
        for path in sorted(base.rglob("*.rs")):
            relative = path.relative_to(ROOT).as_posix()
            if any(segment in f"/{relative}" for segment in SKIP_SEGMENTS):
                continue
            yield path, relative


def collect_occurrences() -> list[tuple[str, int, str]]:
    """收集所有 lint 豁免，返回 (相对路径, 行号, lint 名称) 列表。"""
    occurrences: list[tuple[str, int, str]] = []
    for path, relative in iter_rust_files():
        text = path.read_text(encoding="utf-8", errors="replace")
        for line_number, line in enumerate(text.splitlines(), 1):
            if line.lstrip().startswith("//"):
                continue
            match = ALLOW_PATTERN.match(line)
            if match is None:
                continue
            for lint in match.group(1).split(","):
                lint_name = lint.strip()
                if lint_name:
                    occurrences.append((relative, line_number, lint_name))
    return occurrences


def main() -> int:
    allowlist = load_allowlist()
    occurrences = collect_occurrences()
    scanned_files = sum(1 for _ in iter_rust_files())

    unregistered: list[str] = []
    for relative, line_number, lint_name in occurrences:
        key = (relative, lint_name)
        if key in allowlist:
            allowlist[key] += 1
        else:
            unregistered.append(f"{relative}:{line_number}: {lint_name}")

    stale = [
        f"{relative}: {lint_name}"
        for (relative, lint_name), hits in allowlist.items()
        if hits == 0
    ]

    if unregistered:
        print("发现未登记的编译期 lint 豁免：", file=sys.stderr)
        print("\n".join(unregistered), file=sys.stderr)
        print(
            "处理方式：消除豁免；确属必要时在 tools/lint-allow-allowlist.txt 登记并写明理由。",
            file=sys.stderr,
        )
        return 1

    if stale:
        print("白名单存在过期条目（源码中已无对应豁免）：", file=sys.stderr)
        print("\n".join(stale), file=sys.stderr)
        print("处理方式：从 tools/lint-allow-allowlist.txt 删除这些条目。", file=sys.stderr)
        return 1

    print(
        f"编译期 lint 豁免检查通过：扫描 {scanned_files} 个 Rust 文件，"
        f"{len(occurrences)} 处豁免均已登记。"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
