#!/usr/bin/env python3
"""Cross-platform architecture, naming, and documentation verification."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path
from urllib.parse import unquote, urlparse


ROOT = Path(__file__).resolve().parent.parent
EXCLUDED_PARTS = {
    "target",
    "_reference_repos",
    ".git",
    ".gitnexus",
    # 依赖与产物目录：它们不是本仓源码，行数不受本仓规范约束。
    "node_modules",
    "dist",
    "__pycache__",
    ".venv",
}

# 单文件行数硬限（AGENTS.md）：超过即必须按职责拆分。
MAX_SOURCE_LINES = 700
SOURCE_SUFFIXES = (".rs", ".ts", ".tsx")
SOURCE_DIRS = ("crates", "apps", "tools")
# 字符串字面量（含 raw string）在判注释前先抠掉。
STRING_LITERAL = re.compile(r'r#*".*?"#*|b?"(?:[^"\\]|\\.)*"', re.DOTALL)
DASH = re.compile(r"[\u2013\u2014]")


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


def rel(root: Path, path: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def is_excluded(root: Path, path: Path) -> bool:
    return bool(set(path.resolve().relative_to(root.resolve()).parts) & EXCLUDED_PARTS)


def files(root: Path, relative_dir: str, suffix: str = ".rs") -> list[Path]:
    directory = root / relative_dir
    if not directory.exists():
        return []
    return [path for path in directory.rglob(f"*{suffix}") if path.is_file() and not is_excluded(root, path)]


def verify_file_size(root: Path) -> None:
    """单文件行数硬限：超过 700 行必须按职责拆分。

    这条规则以前只写在文档里，因而漂移到十几个文件超限；现在由本函数执行，
    任何新增的超限文件都会直接失败。
    """
    violations: list[str] = []
    for relative_dir in SOURCE_DIRS:
        directory = root / relative_dir
        if not directory.exists():
            continue
        for path in sorted(directory.rglob("*")):
            if not path.is_file() or path.suffix not in SOURCE_SUFFIXES or is_excluded(root, path):
                continue
            line_count = len(path.read_text(encoding="utf-8").splitlines())
            if line_count > MAX_SOURCE_LINES:
                violations.append(f"{rel(root, path)} has {line_count} lines (limit {MAX_SOURCE_LINES})")
    if violations:
        fail("file exceeds line limit:\n" + "\n".join(violations))


def verify_comment_dashes(root: Path) -> None:
    """注释里不使用 en/em 破折号：与 UI 契约同一口径。

    判定前先抠掉字符串字面量（含 raw string）：像 `["src/**/*.rs"]` 这类正文里
    本来就带 `/*` 与 `*/`，不先剥离就会把块注释状态算错。字符串正文不在此列,
    它们是给模型或用户看的文本，改动会改变行为。
    """
    hits: list[str] = []
    for relative_dir in SOURCE_DIRS:
        directory = root / relative_dir
        if not directory.exists():
            continue
        for path in sorted(directory.rglob("*.rs")):
            if is_excluded(root, path):
                continue
            block_depth = 0
            for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                line = STRING_LITERAL.sub('""', raw)
                inside_comment = block_depth > 0 or line.lstrip().startswith("//")
                if inside_comment and DASH.search(line):
                    hits.append(f"{rel(root, path)}:{number}: {raw.strip()}")
                code = line if block_depth > 0 else line.split("//", 1)[0]
                block_depth = max(0, block_depth + code.count("/*") - code.count("*/"))
    if hits:
        fail("comment contains en/em dash:\n" + "\n".join(hits))


def rule_hits(root: Path, rules: list[dict]) -> list[str]:
    hits: list[str] = []
    for rule in rules:
        patterns = [re.compile(pattern) for pattern in rule["patterns"]]
        for path in files(root, rule["directory"]):
            for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if any(pattern.search(line) for pattern in patterns):
                    hits.append(f"{rel(root, path)}:{line_number}: {rule['message']}: {line.strip()}")
    return hits


def scoped_rule_hits(root: Path, rules: list[dict]) -> list[str]:
    hits: list[str] = []
    for rule in rules:
        allowed = set(rule.get("allowed", []))
        patterns = [re.compile(pattern) for pattern in rule["patterns"]]
        for path in files(root, rule["directory"]):
            relative = rel(root, path)
            if relative in allowed:
                continue
            for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if any(pattern.search(line) for pattern in patterns):
                    hits.append(f"{relative}:{line_number}: {rule['message']}: {line.strip()}")
    return hits


def verify_workspace(root: Path) -> None:
    try:
        metadata = json.loads(
            subprocess.check_output(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                cwd=root,
                text=True,
            )
        )
    except (OSError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        fail(f"cargo metadata failed: {error}")

    domain = next((package for package in metadata["packages"] if package["name"] == "axon-domain"), None)
    if domain is None:
        fail("axon-domain crate was not found.")
    forbidden = [
        re.compile(pattern)
        for pattern in (
            r"^\s*use\s+(tokio|axon_agent_runtime|axon_cli|ratatui|crossterm|std::fs|std::path|std::io)\b",
            r"^\s*extern\s+crate\s+(tokio|axon_agent_runtime|axon_cli|ratatui|crossterm)\b",
            r"^\s*#\[\s*path\s*=\s*\"[^\"]*(runtime|provider|tui|cli)[^\"]*\"\s*\]",
            r"\b(tokio|ratatui|crossterm)::",
            r"\b(axon_agent_runtime|axon_cli)::",
            r"\bstd::(fs|path|io)::",
        )
    ]
    domain_hits = []
    for path in files(root, "crates/axon-domain"):
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if any(pattern.search(line) for pattern in forbidden):
                domain_hits.append(f"{rel(root, path)}:{line_number}: {line.strip()}")
    if domain_hits:
        fail("axon-domain must not reference runtime/provider/UI/tokio/filesystem APIs:\n" + "\n".join(domain_hits))

    for package in metadata["packages"]:
        for dependency in package["dependencies"]:
            if "_reference_repos" in str(dependency.get("path", "")) or "_reference_repos" in str(
                dependency.get("source", "")
            ):
                fail(f"reference repo dependency found: {package['name']} -> {dependency['name']}")

    manifests = [root / "Cargo.toml", root / "Cargo.lock"]
    manifests.extend((root / "crates").rglob("Cargo.toml"))
    manifest_hits = [
        str(path)
        for path in manifests
        if path.exists() and ("_reference_repos" in path.read_text(encoding="utf-8") or re.search(r"\bgit\s*=", path.read_text(encoding="utf-8")))
    ]
    if manifest_hits:
        fail("Cargo manifest/lock contains reference repo or git dependency:\n" + "\n".join(manifest_hits))

    marker_hits = []
    for path in (root / "crates").rglob("*.rs"):
        if is_excluded(root, path):
            continue
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if re.search(r"\bTODO\b|\bFIXME\b|\bmock\b", line, re.IGNORECASE):
                marker_hits.append(f"{rel(root, path)}:{line_number}: {line.strip()}")
    if marker_hits:
        fail("source contains TODO/FIXME/mock markers:\n" + "\n".join(marker_hits))


def verify_layers(root: Path) -> None:
    rules = [
        {
            "directory": "crates/axon-tui-runtime/src/surface",
            "message": "surface must not depend on app shell, widgets, loop host, CLI, or terminal UI",
            "patterns": [r"crate::app_shell\b", r"crate::widgets\b", r"crate::r#loop::agent_tui\b", r"\baxon_cli::", r"\bratatui::", r"\bcrossterm::"],
        },
        {
            "directory": "crates/axon-tui-runtime/src/widgets",
            "message": "widgets must remain pure renderers and must not own workflow/runtime state",
            "patterns": [r"crate::ui_workflow\b", r"\bRuntimeSubmissionDriver\b", r"\bAgentTuiRuntimeAdapter\b", r"\bAgentTuiSubmit\b", r"&mut\s+CellStore\b"],
        },
        {
            "directory": "crates/axon-tui-runtime/src/ui_workflow",
            "message": "ui workflow must not execute runtime submit or depend on terminal widgets",
            "patterns": [r"\bRuntimeSubmissionDriver\b", r"\bAgentTuiRuntimeAdapter\b", r"\bAgentTuiSubmit\b", r"\bRuntimeSubmit\s*\(\s*AgentTuiSubmit\s*\)", r"\bratatui::", r"\bcrossterm::", r"\.submit\s*\("],
        },
        {
            "directory": "crates/axon-tui-runtime/src/app_shell",
            "message": "app shell must not depend on CLI, provider/session internals, or runtime submit driver",
            "patterns": [r"\baxon_cli::", r"\baxon_session::", r"\bRuntimeSubmissionDriver\b", r"\bAgentTuiRuntimeAdapter\b"],
        },
        {
            "directory": "crates/axon-tui-runtime/src/surface/payloads",
            "message": "protocol DTO must not depend on terminal UI, provider/session, widgets, or render abstractions",
            "patterns": [r"\bratatui::", r"\bcrossterm::", r"\baxon_session::", r"\baxon_agent_runtime::", r"crate::widgets\b", r"\bRenderCell\b", r"\bRenderPolicy\b", r"\bCellRegistry\b", r"\bWidget\b", r"\bSpan\b", r"\bLine\b", r"\bStyle\b", r"\bParagraph\b"],
        },
        {
            "directory": "crates/axon-cli/src/run_commands/interactive_tui",
            "message": "CLI interactive TUI host must not reference terminal widget internals",
            "patterns": [r"\bratatui::", r"\bcrossterm::", r"\baxon_tui_runtime::widgets\b", r"\bcrate::widgets\b", r"\bwidgets::", r"\bratatui::widgets::(Widget|Paragraph)\b", r"\bratatui::text::(Span|Line)\b", r"\bratatui::style::Style\b", r"use\s+ratatui::.*\b(Widget|Paragraph|Span|Line|Style)\b"],
        },
    ]
    hits = rule_hits(root, rules)
    if hits:
        fail("TUI architecture layer violation detected:\n" + "\n".join(hits))

    single_owner = [
        {"directory": "crates/axon-tui-runtime/src", "message": "spinner frames and spinner interval must be owned by surface/animation.rs", "allowed": ["crates/axon-tui-runtime/src/surface/animation.rs"], "patterns": [r"\bconst\s+SPINNER_FRAMES\b", r"\bSPINNER_INTERVAL\s*:\s*Duration\b"]},
        {"directory": "crates/axon-tui-runtime/src", "message": "target-state palette tokens must be owned by app_shell/style_tokens.rs", "allowed": ["crates/axon-tui-runtime/src/app_shell/style_tokens.rs"], "patterns": [r"Color::Rgb\(191,\s*203,\s*219\)", r"Color::Rgb\(165,\s*180,\s*202\)", r"Color::Rgb\(119,\s*130,\s*150\)", r"Color::Rgb\(83,\s*205,\s*137\)", r"Color::Rgb\(147,\s*167,\s*194\)", r"Color::Rgb\(103,\s*197,\s*226\)", r"Color::Rgb\(229,\s*194,\s*103\)", r"Color::Rgb\(192,\s*162,\s*255\)", r"Color::Rgb\(255,\s*111,\s*111\)"]},
    ]
    hits = scoped_rule_hits(root, single_owner)
    if hits:
        fail("TUI single-owner rule violation detected:\n" + "\n".join(hits))

    leaks = [
        {"directory": "crates/axon-tui-runtime/src/surface", "message": "runtime selectability belongs to RenderPolicy, not RenderCell or payload", "allowed": ["crates/axon-tui-runtime/src/surface/render_policy.rs"], "patterns": [r"\bis_runtime_selectable\b"]},
        {"directory": "crates/axon-tui-runtime/src/surface/payloads", "message": "payload kind must use CellPayloadKind and registry, not raw static strings", "patterns": [r"kind\s*:\s*&'static\s+str\b", r"fn\s+kind\s*\([^)]*\)\s*->\s*&'static\s+str\b"]},
    ]
    hits = scoped_rule_hits(root, leaks)
    if hits:
        fail("TUI abstraction leak violation detected:\n" + "\n".join(hits))


def verify_naming(root: Path) -> None:
    target_dirs = ["crates/axon-tui-runtime/src/surface", "crates/axon-tui-runtime/src/app_shell", "crates/axon-tui-runtime/src/ui_workflow"]
    target_files = [path for directory in target_dirs for path in files(root, directory)]
    public_types: dict[str, list[str]] = defaultdict(list)
    type_pattern = re.compile(r"\bpub\s+(?:\([^)]*\)\s+)?(?:struct|enum|trait|type)\s+([A-Z][A-Za-z0-9_]*)\b")
    for path in target_files:
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = type_pattern.search(line)
            if match:
                public_types[match.group(1)].append(f"{rel(root, path)}:{line_number}: {line.strip()}")
    duplicate_hits = [f"{item}: duplicated target-state TUI public type: {name}" for name, items in public_types.items() if len(items) > 1 for item in items]
    if duplicate_hits:
        fail("TUI duplicated public type detected:\n" + "\n".join(duplicate_hits))

    forbidden = re.compile(r"\b(RenderSurfaceCell|SurfaceRenderCell|CellSnapshot|TranscriptCellSnapshot|SnapshotCell|TuiRenderSnapshot|SessionSnapshot|RenderSurface)\b")
    hits = []
    for path in target_files:
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if forbidden.search(line):
                hits.append(f"{rel(root, path)}:{line_number}: temporary target-state TUI name is forbidden: {line.strip()}")
    if hits:
        fail("TUI temporary naming drift detected:\n" + "\n".join(hits))


def quality_files(root: Path) -> list[Path]:
    paths = [root / name for name in ("README.md", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml")]
    for directory in (root / "crates", root / "docs", root / "tools"):
        if directory.exists():
            for path in directory.rglob("*"):
                if path.is_file() and path.suffix in {".md", ".rs", ".toml", ".json", ".ps1", ".py"}:
                    paths.append(path)
    return [path for path in paths if path.exists() and not is_excluded(root, path)]


def resolve_markdown_link(source: Path, target: str) -> Path | None | str:
    normalized = target.strip()
    angle_wrapped = normalized.startswith("<") and normalized.endswith(">")
    if angle_wrapped:
        normalized = normalized[1:-1]
    candidates = [normalized]
    if not angle_wrapped and re.match(r"^\S+\s+.*$", normalized):
        candidates.append(normalized.split(None, 1)[0])
    for candidate in candidates:
        without_anchor = candidate.split("#", 1)[0]
        if not without_anchor:
            continue
        if urlparse(without_anchor).scheme:
            return None
        try:
            resolved = (source.parent / unquote(without_anchor)).resolve()
        except (OSError, ValueError):
            return f"__invalid_path__::{without_anchor}"
        if resolved.exists():
            return resolved
    fallback = candidates[0].split("#", 1)[0]
    if not fallback:
        return None
    try:
        return (source.parent / unquote(fallback)).resolve()
    except (OSError, ValueError):
        return f"__invalid_path__::{fallback}"


def verify_docs(root: Path) -> None:
    paths = quality_files(root)
    bom_hits = [str(path) for path in paths if path.read_bytes().startswith(b"\xef\xbb\xbf")]
    if bom_hits:
        fail("UTF-8 BOM file detected:\n" + "\n".join(bom_hits))

    markdown_files = [path for path in paths if path.suffix == ".md" and (path == root / "README.md" or root / "docs" in path.parents)]
    link_pattern = re.compile(r"(?<!\!)\[[^\]]+\]\(([^)]+)\)")
    broken_links = []
    for path in markdown_files:
        content = path.read_text(encoding="utf-8")
        for match in link_pattern.finditer(content):
            target = match.group(1).strip()
            resolved = resolve_markdown_link(path, target)
            if resolved is not None and (isinstance(resolved, str) or not resolved.exists()):
                broken_links.append(f"{rel(root, path)} -> {target}")
    if broken_links:
        fail("broken local Markdown link detected:\n" + "\n".join(broken_links))

    term_rules = [(re.compile(r"(?<!\.)\bmcp\.json\b"), "MCP config file should be written as .mcp.json"), (re.compile(r"UTF-8 with BOM"), "encoding baseline should be UTF-8 without BOM"), (re.compile(r"GitNexus"), "Axon must not depend on GitNexus")]
    term_hits = []
    for path in markdown_files:
        if rel(root, path) == "AGENTS.md":
            continue
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            for pattern, message in term_rules:
                if pattern.search(line):
                    term_hits.append(f"{rel(root, path)}:{line_number}: {message}")
    if term_hits:
        fail("terminology consistency issue detected:\n" + "\n".join(term_hits))


def verify_target_state_forbidden_symbols(root: Path) -> None:
    """Ensure legacy Goal Brain and Target symbols/tables do not regress (R0-01)."""
    forbidden_pattern = re.compile(
        r"\b(SubmitGoal|PlanRecord|TargetActionRecord|brain_goals|brain_plans|brain_actions|brain_projections)\b"
    )
    hits = []
    for path in (root / "crates").rglob("*.rs"):
        if is_excluded(root, path):
            continue
        for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if forbidden_pattern.search(line):
                hits.append(f"{rel(root, path)}:{line_number}: {line.strip()}")
    if hits:
        fail("Target-state forbidden symbol or table detected:\n" + "\n".join(hits))


# 会话标识不得由字符串拼接表达：标识一律是服务端生成的 ULID，关系落列、不落前缀。
#
# 只扫生产源码：测试里的历史字面量用来证明旧数据仍能被读出来（它们现在只是普通字符串），
# 不属于拼接面。
SESSION_ID_CONCAT_RULES = [
    (
        re.compile(r"(?i)\b(?:format!|starts_with|ends_with|contains|strip_prefix)\s*\([^\n]*--subagent--"),
        "session id must not be built from or detected by `--subagent--`; subagent identity is (root_session_id, agent_path)",
    ),
    (
        re.compile(r"(?i)\b(?:format!|starts_with|ends_with|contains|strip_prefix)\s*\([^\n]*goal-session-"),
        "session id must not be built from or detected by `goal-session-`; goal session identity is sessions.goal_id",
    ),
    (
        re.compile(r"(?im)^\s*const\s+[A-Z0-9_]*SUBAGENT[A-Z0-9_]*\s*:\s*&str\s*=\s*\"--subagent--\""),
        "subagent session separator constant is forbidden",
    ),
    (
        re.compile(r"(?im)^\s*const\s+[A-Z0-9_]*SESSION_ID_PREFIX[^=\n]*=\s*\"goal-session-\""),
        "goal session id prefix constant is forbidden",
    ),
]


def is_test_source(path: Path) -> bool:
    """测试源码：文件名带 test/tests，或位于 tests 目录。"""
    parts = {part.lower() for part in path.parts}
    return "tests" in parts or "test" in path.name.lower() or "tests" in path.name.lower()


def verify_session_id_concatenation(root: Path) -> None:
    """会话标识必须生成一次，不得拼接。"""
    hits: list[str] = []
    for relative_dir in ("crates", "apps"):
        directory = root / relative_dir
        if not directory.exists():
            continue
        for path in sorted(directory.rglob("*.rs")):
            if not path.is_file() or is_excluded(root, path) or is_test_source(path):
                continue
            text = path.read_text(encoding="utf-8")
            for pattern, message in SESSION_ID_CONCAT_RULES:
                for match in pattern.finditer(text):
                    line_number = text.count("\n", 0, match.start()) + 1
                    hits.append(f"{rel(root, path)}:{line_number}: {message}")
    if hits:
        fail("session id concatenation detected:\n" + "\n".join(hits))


# 会话标识不得再长出数值摘要副本：`stable_numeric_id` 只服务于已知的内存句柄
# （agent 控制树根、记忆写入格、执行回执、工具编排 SessionId），这些句柄不参与会话身份。
# 白名单之外出现即视为新的数值摘要，必须改为直接使用会话标识。
SESSION_NUMERIC_DIGEST_SYMBOL = "stable_numeric_id"
SESSION_NUMERIC_DIGEST_ALLOWLIST = {
    "crates/axon-cli/src/runtime/stable_ids.rs",
    "crates/axon-cli/src/run_commands/run_reflection_apply.rs",
    "crates/axon-cli/src/run_commands/run_request_subagents.rs",
    "crates/axon-cli/src/run_commands/run_self_iteration_subagent.rs",
    "crates/axon-cli/src/run_commands/run_turn_outcome.rs",
    "crates/axon-cli/src/tools/orchestrator_models.rs",
}


def verify_session_numeric_digest(root: Path) -> None:
    """会话标识不得被摘要成数字当身份用。"""
    hits: list[str] = []
    for relative_dir in ("crates", "apps"):
        directory = root / relative_dir
        if not directory.exists():
            continue
        for path in sorted(directory.rglob("*.rs")):
            if not path.is_file() or is_excluded(root, path) or is_test_source(path):
                continue
            relative = rel(root, path)
            if relative in SESSION_NUMERIC_DIGEST_ALLOWLIST:
                continue
            text = path.read_text(encoding="utf-8")
            for line_number, line in enumerate(text.splitlines(), 1):
                stripped = line.lstrip()
                if stripped.startswith(("//", "/*", "*")):
                    continue
                if SESSION_NUMERIC_DIGEST_SYMBOL in line:
                    hits.append(
                        f"{relative}:{line_number}: {SESSION_NUMERIC_DIGEST_SYMBOL} "
                        "outside the approved in-memory handles"
                    )
    if hits:
        fail(
            "session id numeric digest appeared outside the approved in-memory handles:\n"
            + "\n".join(hits)
        )


# 运行态库的行解码失败必须走 `decode::decode_error`：对外一句人话，原始字段与取值进日志。
# `rusqlite::Error::InvalidQuery` 的 Display 是「Query is not read-only」，既误导又不可自证，
# 一旦有新解码点写回它，用户又会看到一句与真实原因无关的话。
ROW_DECODE_FORBIDDEN = "rusqlite::Error::InvalidQuery"
ROW_DECODE_SCOPE = "crates/axon-session/src/infrastructure/sqlite_runtime_store"
ROW_DECODE_EXEMPT = {"sqlite_runtime_store/decode.rs"}


def verify_row_decode_errors(root: Path) -> None:
    """运行态库行解码不得回通用 InvalidQuery。"""
    directory = root / ROW_DECODE_SCOPE
    if not directory.exists():
        return
    hits: list[str] = []
    for path in sorted(directory.rglob("*.rs")):
        if not path.is_file():
            continue
        relative = rel(root, path)
        if any(relative.endswith(exempt) for exempt in ROW_DECODE_EXEMPT):
            continue
        text = path.read_text(encoding="utf-8")
        for line_number, line in enumerate(text.splitlines(), 1):
            stripped = line.lstrip()
            if stripped.startswith(("//", "/*", "*")):
                continue
            if ROW_DECODE_FORBIDDEN in line:
                hits.append(
                    f"{relative}:{line_number}: 行解码失败必须走 decode_error，"
                    "不得回 rusqlite::Error::InvalidQuery"
                )
    if hits:
        fail("row decode errors must use decode::decode_error:\n" + "\n".join(hits))


def verify_legacy_external_entries(root: Path) -> None:
    LEGACY_EXTERNAL_ENTRY_PATTERN = re.compile(
        r"\b(?:CreateSession|ResetSession|SubmitAction|ResumeSession|AgentTuiStartupAction|"
        r"run_agent_tui_cli_with_startup_action|run_agent_tui_resume_entry|"
        r"is_session_reset_command|CARD_ACTION_CONTROL_RESET|CARD_ACTION_CONTROL_NEW)\b"
    )
    hits = []
    for path in (root / "crates").rglob("*.rs"):
        if path.is_file() and LEGACY_EXTERNAL_ENTRY_PATTERN.search(
            path.read_text(encoding="utf-8")
        ):
            hits.append(rel(root, path))
    if hits:
        fail(
            "legacy external Session/Action entry detected:\n"
            + "\n".join(sorted(hits))
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace-root", type=Path, default=ROOT)
    args = parser.parse_args()
    root = args.workspace_root.resolve()
    verify_workspace(root)
    verify_file_size(root)
    verify_comment_dashes(root)
    verify_layers(root)
    verify_naming(root)
    verify_target_state_forbidden_symbols(root)
    verify_docs(root)
    verify_legacy_external_entries(root)
    verify_session_id_concatenation(root)
    verify_session_numeric_digest(root)
    verify_row_decode_errors(root)
    print("TUI architecture layer verification OK")
    print("File size verification OK")
    print("Comment dash verification OK")
    print("TUI naming verification OK")
    print("Target-state forbidden symbols verification OK")
    print("Documentation verification OK")
    print("Session identity concatenation verification OK")
    print("Session identity numeric digest verification OK")
    print("Row decode error verification OK")
    print("Architecture verification OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
