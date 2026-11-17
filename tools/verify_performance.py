#!/usr/bin/env python3
"""性能契约门禁的跨平台实现（对应 tools/perf-baseline.json）。

存储架构 §9 的四项硬上限全部由本脚本产出读数：
- 事件写入 p99 与控制面事务 p95 由 Rust 侧 ignored 测量用例打印，脚本解析后比对；
- 会话列表首屏由 CLI 的索引验收用例打印；
- 运行态 data 目录体积直接扫描本机安装树。
桌面端 P2/P9/P10/P11 属于真机交互读数，只能人工登记。

判定分三道：
1. 绝对上限：跨平台契约，任何机器每次运行都判；四项硬上限只要本次没有读数就判
   失败（不是静默跳过），确实无法运行 Rust 测量时用 `--skip-rust-measurements`
   显式跳过；
2. 相对基线回退：只在录制该基线的那台机器上比较，并要求增量越过噪声下限。毫秒
   级读数在别的机器或别的输入规模上差一个数量级是常态，按比例硬比会把测量噪声
   当成回退；
3. 复测确认：可疑读数先复测一次并取较小值再判。同一台机器上背靠背复测的离群度
   实测到 3.4 倍（event p99 基线 0.931ms，同机一次负载尖峰读到 3.176ms），单次
   读数直接判回退等于把负载尖峰当退化；真实退化是持续的，复测仍会复现。

`--allow-incomplete` 只放行人工真机读数（P2/P9/P10/P11）与 P12 的缺测，不覆盖四项
硬上限：那四项在所有平台都能自动测量，用开关放行等于把盲区写进绿门禁。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import socket
import subprocess
import sys
from datetime import datetime
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

REPO_ROOT = Path(__file__).resolve().parent.parent
REGRESSION_RATIO = 1.05
# 相对回退的噪声下限：毫秒级读数的 1 毫秒抖动就是 100%，只看比例会把测量噪声
# 判成回退。因此回退必须同时越过比例与绝对增量两道线。下限按各指标在本机的实测
# 抖动设定（背靠背复测的离散区间）：event p99 1.22~1.49ms、控制面 p95 0.15~0.24ms
# （负载尖峰 0.52ms）、首屏 1~2ms（读数以整毫秒量化）。下限刻意贴近抖动区间，
# 不能更大：下限一旦超过基线值的一半，相对判定就再也无法在 1.5 倍基线前报警。
REGRESSION_FLOOR = {
    "event_append_p99_ms": 0.5,
    "control_tx_p95_ms": 0.15,
    "sessions_first_page_ms": 2.0,
    "runtime_data_mib": 8.0,
    "p2_offline_start_ms": 150.0,
    "p10_cold_start_ms": 150.0,
    "p11_upload_1mib_ms": 5.0,
    "p11_upload_8mib_ms": 5.0,
    "p11_upload_32mib_ms": 5.0,
    "p9_idle_cpu_percent": 0.5,
}
# §9 的绝对上限：读数超过它就是不合格，与「相对基线回退」是两道独立检查。
HARD_LIMITS = {
    "event_append_p99_ms": 5.0,
    "control_tx_p95_ms": 50.0,
    "sessions_first_page_ms": 30.0,
    "runtime_data_mib": 50.0,
}
GATEWAY_PREFIXES = ("/api/v1/files", "/api/v1/fs/dirs", "/api/v1/attachments")

# 由 Rust 侧 ignored 用例产出的读数：命令、输出模式、超时（秒）。
RUST_MEASUREMENTS = (
    (
        "event_append_p99_ms",
        [
            "cargo",
            "test",
            "-p",
            "axon-eventlog",
            "--test",
            "contract",
            "--",
            "--ignored",
            "event_append_p99",
            "--nocapture",
        ],
        r"event_append_p99_ms=([0-9.]+)",
    ),
    (
        "control_tx_p95_ms",
        [
            "cargo",
            "test",
            "-p",
            "axon-session",
            "--lib",
            "--",
            "--ignored",
            "control_transaction_p95",
            "--nocapture",
        ],
        r"control_tx_p95_ms=([0-9.]+)",
    ),
    (
        "sessions_first_page_ms",
        [
            "cargo",
            "test",
            "-p",
            "axon-cli",
            "--lib",
            "--",
            "--ignored",
            "session_list_query",
            "--nocapture",
        ],
        r"indexed=([0-9]+)ms",
    ),
)
RUST_COMMANDS = {
    name: (command, pattern) for name, command, pattern in RUST_MEASUREMENTS
}
_CONFIRMED: dict[str, float | None] = {}

# 只能人工登记的真机读数（自动测量覆盖不到交互与冷启动）。
MANUAL_ITEMS = (
    ("p2_offline_start_ms", "P2 断网启动到可见界面（毫秒）"),
    ("p9_idle_cpu_percent", "P9 空闲 60 秒渲染进程 CPU（百分比）"),
    ("p10_cold_start_ms", "P10 冷启动到可交互（毫秒）"),
    ("p11_upload_1mib_ms", "P11 上传 1 MiB（毫秒）"),
    ("p11_upload_8mib_ms", "P11 上传 8 MiB（毫秒）"),
    ("p11_upload_32mib_ms", "P11 上传 32 MiB（毫秒）"),
)


def runtime_home() -> Path:
    override = os.environ.get("AXON_RUNTIME_ROOT")
    if override:
        return Path(override)
    home = os.environ.get("HOME") or os.environ.get("USERPROFILE") or str(Path.home())
    return Path(home) / ".axon"


def run_rust_measurement(command: list[str], pattern: str) -> tuple[float | None, str]:
    """执行一个 ignored 测量用例并解析它打印的读数，返回 (读数, 问题描述)。

    问题描述用于把「没测到」的三种原因分开：工具链缺失、用例不存在（筛选后 0 个
    测试被执行，通常是改名或删除）、读数解析失败。三种都必须能被判失败看见，
    否则门禁会因为一条消失的测量用例永久变绿。
    """
    if shutil.which(command[0]) is None:
        return None, f"当前环境没有 {command[0]}"
    try:
        completed = subprocess.run(
            command,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=1800,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return None, f"测量命令无法执行：{error}"
    output = f"{completed.stdout}\n{completed.stderr}"
    match = re.search(pattern, output)
    if match is None:
        if "running 0 tests" in output or "0 passed; 0 failed" in output:
            return None, "测量用例不存在（筛选后 0 个测试被执行，可能已改名或删除）"
        return None, f"读数解析失败（退出码 {completed.returncode}）"
    return float(match.group(1)), ""


def confirm_reading(name: str) -> float | None:
    """对可疑读数复测一次并取较小值（同一指标只复测一次）。"""
    if name in _CONFIRMED:
        return _CONFIRMED[name]
    retested: float | None = None
    entry = RUST_COMMANDS.get(name)
    if entry is not None:
        retested, problem = run_rust_measurement(*entry)
        if retested is None:
            print(
                f"[perf] {name} 复测未取到读数：{problem}（退回首测读数判定）",
                file=sys.stderr,
            )
    _CONFIRMED[name] = retested
    return retested


def auto_measurements() -> dict[str, float]:
    measurements: dict[str, float] = {}
    endpoint_test = REPO_ROOT / "crates/axon-desktop/src/connection/endpoint_tests.rs"
    present = endpoint_test.is_file() and (
        "sixty_times_reads_the_config_at_most_sixty_times"
        in endpoint_test.read_text(encoding="utf-8", errors="replace")
    )
    if not present:
        print("[perf] P1：找不到端点解析读盘次数的断言，视为缺失", file=sys.stderr)
    measurements["p1_endpoint_reads_assertion"] = 1 if present else 0

    log_path = runtime_home() / "data" / "logs" / f"axon-{datetime.now():%Y-%m-%d}.log"
    if log_path.is_file():
        text = log_path.read_text(encoding="utf-8", errors="replace")
        measurements["p12_file_route_hits"] = sum(
            len(re.findall(re.escape(prefix), text)) for prefix in GATEWAY_PREFIXES
        )
    else:
        print(f"[perf] P12 待测量：当日 daemon 日志不存在（{log_path}）")
    data_root = runtime_home() / "data"
    total = 0
    if data_root.is_dir():
        for path in data_root.rglob("*"):
            try:
                if path.is_file():
                    total += path.stat().st_size
            except OSError:
                # 扫描期间 daemon 仍在写（WAL 轮转、日志截断），单文件消失不应中断门禁。
                continue
    else:
        print(f"[perf] 运行态 data 目录不存在，按 0 MiB 计（{data_root}）")
    # 保留两位小数：整 MiB 向下取整会把 0.9 MiB 与「没有数据」抹成同一个 0，
    # 也会让 50 MiB 上限多放行近 1 MiB。
    measurements["runtime_data_mib"] = round(total / (1024 * 1024), 2)
    return measurements


def rust_measurements() -> tuple[dict[str, float], dict[str, str]]:
    """跑三个 ignored 测量用例，返回 (读数, 未测到的原因)。"""
    measurements: dict[str, float] = {}
    gaps: dict[str, str] = {}
    for name, command, pattern in RUST_MEASUREMENTS:
        value, problem = run_rust_measurement(command, pattern)
        if value is None:
            gaps[name] = problem
            print(f"[perf] {name} 未测到：{problem}", file=sys.stderr)
        else:
            measurements[name] = value
    return measurements, gaps


def main() -> int:
    parser = argparse.ArgumentParser(description="性能契约门禁")
    parser.add_argument("--baseline", default=str(REPO_ROOT / "tools" / "perf-baseline.json"))
    parser.add_argument("--record", action="store_true")
    parser.add_argument("--manual-json", default="")
    parser.add_argument(
        "--skip-rust-measurements",
        action="store_true",
        help="跳过 Rust 侧的 ignored 测量用例（无工具链或只需人工读数时使用）",
    )
    parser.add_argument("--allow-incomplete", action="store_true")
    args = parser.parse_args()

    baseline = Path(args.baseline)
    baseline_document: dict | None = None
    manual: dict[str, float] = {}
    if baseline.is_file():
        # 先留存基线原文：--record 会在判定前覆盖同文件，重新读取等于拿本次读数
        # 跟本次读数比，相对回退永远不可能触发。
        baseline_document = json.loads(baseline.read_text(encoding="utf-8"))
        manual.update(baseline_document.get("manual") or {})
    if args.manual_json:
        manual.update(json.loads(args.manual_json))

    measurements = auto_measurements()
    gaps: dict[str, str] = {}
    if not args.skip_rust_measurements:
        measured, gaps = rust_measurements()
        measurements.update(measured)
    pending = [
        f"{label}（未登记读数）" for key, label in MANUAL_ITEMS if key not in manual
    ]
    if "p12_file_route_hits" not in measurements:
        pending.append("P12 本机操作零网关命中（当日日志不存在，需先跑一次真机操作再测）")
    if measurements.get("p1_endpoint_reads_assertion") == 0:
        pending.append("P1 端点解析读盘次数断言缺失（axon-desktop 用例里找不到该断言）")

    print("[perf] 自动读数：")
    for name, value in measurements.items():
        print(f"  {name} = {value}")
    if manual:
        print("[perf] 人工真机读数：")
        for name, value in manual.items():
            print(f"  {name} = {value}")

    if args.record:
        document = {
            "recordedAt": datetime.now().astimezone().isoformat(),
            # 相对基线比较只在录制它的那台机器上有意义：CI 的 Windows runner 与
            # Linux runner 在同一个用例上差一个数量级，跨机器比相对值只会误报回退。
            "recordedOn": {"platform": sys.platform, "node": socket.gethostname()},
            "measurements": measurements,
            "manual": manual,
            "pending": pending,
        }
        baseline.write_text(
            json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"[perf] 已写入基线：{baseline}")

    failures = 0
    rust_names = set(RUST_COMMANDS)
    judged = 0
    print(f"[perf] 硬上限判定（{len(HARD_LIMITS)} 项，未测到即判失败）：")
    for name, limit in HARD_LIMITS.items():
        # 自动读数优先：这四项都能自动测量，手测读数若排在前头就会永久遮住真读数。
        auto_value = measurements.get(name)
        current = auto_value if auto_value is not None else manual.get(name)
        from_auto = auto_value is not None
        if current is None:
            if args.skip_rust_measurements and name in rust_names:
                print(f"  - {name}: 跳过（--skip-rust-measurements 显式跳过）")
                continue
            print(f"  - {name}: 未判定，判失败（上限 {limit}；{gaps.get(name, '本次没有读数')}）")
            failures += 1
            continue
        value = float(current)
        if value > float(limit) and from_auto and name in rust_names:
            # 超限也可能是负载尖峰：先复测，取两次较小值。
            retested = confirm_reading(name)
            if retested is not None and retested < value:
                print(f"  - {name}: 首测 {value} 超限，复测 {retested}，按复测判定")
                value = retested
        judged += 1
        if value > float(limit):
            print(f"  - {name} = {value} / 上限 {limit} → 超限")
            failures += 1
        else:
            print(f"  - {name} = {value} / 上限 {limit} → 通过")
    print(f"[perf] 本次判定 {judged}/{len(HARD_LIMITS)} 项硬上限")
    if baseline_document is None:
        print(f"[perf] 没有基线文件：{baseline}（先用 --record 生成一次）")
    else:
        recorder = baseline_document.get("recordedOn") or {}
        same_machine = recorder.get("platform") == sys.platform and (
            recorder.get("node") or ""
        ).casefold() == socket.gethostname().casefold()
        if not same_machine:
            print(
                "[perf] 基线录制于另一台机器（"
                f"{recorder.get('platform') or '未知平台'} / {recorder.get('node') or '未知主机'}"
                "）：本次只按绝对上限判定，不做相对回退比较"
            )
        else:
            suspects: list[tuple[str, float, float, str]] = []
            for bucket, current_map in (
                ("measurements", measurements),
                ("manual", manual),
            ):
                previous_map = baseline_document.get(bucket) or {}
                for name, current in current_map.items():
                    previous = previous_map.get(name)
                    if previous is None:
                        continue
                    floor = REGRESSION_FLOOR.get(name, 0.0)
                    grew = float(current) - float(previous)
                    if float(previous) == 0.0:
                        # 基线读作 0 时比例没有意义（从零开始的任何增长都是无穷大
                        # 倍数）。配了噪声下限的项按绝对增量判，其余只受绝对上限约束。
                        if floor > 0.0 and grew > floor:
                            print(
                                f"[perf] 回退：{name} 从 0 涨到 {current}，"
                                f"超过噪声下限 {floor}"
                            )
                            failures += 1
                        elif grew > 0.0:
                            print(
                                f"[perf] {name} 基线为 0，只按绝对上限判定（当前 {current}）"
                            )
                        continue
                    if (
                        float(current) > float(previous) * REGRESSION_RATIO
                        and grew > floor
                    ):
                        suspects.append((name, float(previous), float(current), bucket))
            for name, previous, current, bucket in suspects:
                floor = REGRESSION_FLOOR.get(name, 0.0)
                value = current
                retested = (
                    confirm_reading(name)
                    if bucket == "measurements" and name in rust_names
                    else None
                )
                if retested is not None:
                    value = min(current, retested)
                if value > previous * REGRESSION_RATIO and value - previous > floor:
                    print(
                        f"[perf] 回退：{name} 从 {previous} 变成 {value}，"
                        f"超过 {(REGRESSION_RATIO - 1):.0%} 容忍且增量大于噪声 {floor}"
                    )
                    failures += 1
                else:
                    suffix = (
                        f"复测 {retested} 后判为测量噪声"
                        if retested is not None
                        else "判为测量噪声"
                    )
                    print(f"[perf] {name} 首测 {current} 疑似回退，{suffix}")

    if pending:
        print("[perf] 待人工真机测量的项：")
        for item in pending:
            print(f"  - {item}")
        if args.allow_incomplete:
            print("[perf] 以上缺测项由 --allow-incomplete 放行（交付前必须补齐）")
        else:
            print(
                "[perf] 有未登记读数的项，判失败；CI 用 --allow-incomplete 放行，"
                "交付前必须补齐",
                file=sys.stderr,
            )
            failures += len(pending)

    if failures:
        # failures 里既有超限/回退，也有硬上限缺测与未登记读数，故不写「项回退」。
        print(f"[perf] 性能契约未通过：{failures} 项不合格", file=sys.stderr)
        return 1
    print("[perf] 性能契约通过（待测量项见上）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
