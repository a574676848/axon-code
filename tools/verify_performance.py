#!/usr/bin/env python3
"""性能契约门禁的跨平台实现（对应 tools/perf-baseline.json）。"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from datetime import datetime
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
REGRESSION_RATIO = 1.05
GATEWAY_PREFIXES = ("/api/v1/files", "/api/v1/fs/dirs", "/api/v1/attachments")
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


def auto_measurements() -> dict[str, int]:
    measurements: dict[str, int] = {}
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
    return measurements


def main() -> int:
    parser = argparse.ArgumentParser(description="性能契约门禁")
    parser.add_argument("--baseline", default=str(REPO_ROOT / "tools" / "perf-baseline.json"))
    parser.add_argument("--record", action="store_true")
    parser.add_argument("--manual-json", default="")
    parser.add_argument("--allow-incomplete", action="store_true")
    args = parser.parse_args()

    baseline = Path(args.baseline)
    manual: dict[str, float] = {}
    if baseline.is_file():
        existing = json.loads(baseline.read_text(encoding="utf-8"))
        manual.update(existing.get("manual") or {})
    if args.manual_json:
        manual.update(json.loads(args.manual_json))

    measurements = auto_measurements()
    pending = [
        f"{label}（未登记读数）" for key, label in MANUAL_ITEMS if key not in manual
    ]
    if "p12_file_route_hits" not in measurements:
        pending.append("P12 本机操作零网关命中（当日日志不存在，需先跑一次真机操作再测）")

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
            "measurements": measurements,
            "manual": manual,
            "pending": pending,
        }
        baseline.write_text(
            json.dumps(document, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )
        print(f"[perf] 已写入基线：{baseline}")

    failures = 0
    if baseline.is_file():
        baseline_document = json.loads(baseline.read_text(encoding="utf-8"))
        for bucket, current_map in (
            ("measurements", measurements),
            ("manual", manual),
        ):
            previous_map = baseline_document.get(bucket) or {}
            for name, current in current_map.items():
                previous = previous_map.get(name)
                if previous is None:
                    continue
                if float(current) > float(previous) * REGRESSION_RATIO:
                    print(
                        f"[perf] 回退：{name} 从 {previous} 变成 {current}，"
                        f"超过 {(REGRESSION_RATIO - 1):.0%} 容忍"
                    )
                    failures += 1
    else:
        print(f"[perf] 没有基线文件：{baseline}（先用 --record 生成一次）")

    if pending:
        print("[perf] 待人工真机测量的项：")
        for item in pending:
            print(f"  - {item}")
        if not args.allow_incomplete:
            print("[perf] 有未登记读数的项；CI 可用 --allow-incomplete 放行，交付前必须补齐")

    if failures:
        print(f"[perf] 性能契约未通过：{failures} 项回退", file=sys.stderr)
        return 1
    print("[perf] 性能契约通过（待测量项见上）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
