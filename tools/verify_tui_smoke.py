#!/usr/bin/env python3
"""Agent TUI 真机冒烟的跨平台校验器。

断言清单在 tools/tui_smoke_contract.json，Windows 的 verify-tui-smoke.ps1
与 Linux 的 python3 直跑共用同一份数据，避免清单漂移。
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

for _stream in (sys.stdout, sys.stderr):
    if hasattr(_stream, "reconfigure"):
        _stream.reconfigure(encoding="utf-8", errors="replace")

REPO_ROOT = Path(__file__).resolve().parent.parent
CONTRACT_PATH = Path(__file__).resolve().parent / "tui_smoke_contract.json"

MANAGED_ENV_KEYS = (
    "TERM",
    "TERM_PROGRAM",
    "WT_SESSION",
    "WSL_DISTRO_NAME",
    "WEZTERM_EXECUTABLE",
    "ALACRITTY_SOCKET",
    "GHOSTTY_RESOURCES_DIR",
    "ITERM_SESSION_ID",
    "TMUX",
    "STY",
    "SSH_CONNECTION",
    "SSH_TTY",
    "VSCODE_INJECTION",
    "AXON_TUI_SSH_SLOW_LINK",
)


class SmokeFailure(RuntimeError):
    pass


def run_cli_json(extra_env: dict[str, str] | None = None) -> dict:
    env = dict(os.environ)
    if extra_env is not None:
        for key in MANAGED_ENV_KEYS:
            env.pop(key, None)
        env.update(extra_env)
    proc = subprocess.run(
        ["cargo", "run", "--quiet", "-p", "axon-cli", "--", "--format", "json"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
    )
    if proc.returncode != 0:
        raise SmokeFailure(
            f"axon-cli JSON smoke failed with exit code {proc.returncode}: "
            f"{proc.stderr.strip()[:400]}"
        )
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError as error:
        raise SmokeFailure(f"axon-cli 输出不是合法 JSON：{error}") from error


def require(mapping: dict, key: str, label: str) -> object:
    if key not in mapping or mapping[key] is None:
        raise SmokeFailure(f"{label} 缺少字段 {key}")
    return mapping[key]


def check_ability_matrix(report: dict, spec: dict) -> dict[str, dict]:
    matrix = report.get("abilityMatrix")
    if matrix is None:
        raise SmokeFailure("abilityMatrix missing from Agent TUI launch report")
    total = int(matrix.get("totalCount", 0))
    covered = int(matrix.get("smokeCoveredCount", 0))
    if total < spec["minTotalCount"]:
        raise SmokeFailure(f"ability matrix totalCount too small: {total}")
    if covered < spec["minSmokeCoveredCount"]:
        raise SmokeFailure(f"ability matrix smokeCoveredCount too small: {covered}")
    abilities = matrix.get("abilities") or []
    if len(abilities) != total:
        raise SmokeFailure(
            f"ability matrix count mismatch: abilities={len(abilities)} total={total}"
        )
    categories = {item.get("category") for item in matrix.get("categories") or []}
    for category in spec["categories"]:
        if category not in categories:
            raise SmokeFailure(f"ability category missing: {category}")

    by_id: dict[str, dict] = {}
    for ability in abilities:
        ability_id = str(ability.get("id") or "").strip()
        if not ability_id:
            raise SmokeFailure("ability id missing in abilityMatrix")
        if ability_id in by_id:
            raise SmokeFailure(
                f"duplicate ability id: {ability_id} target={ability.get('targetEntry')}"
            )
        for field in ("category", "oldEntry", "targetEntry", "automatedVerification", "targetToken"):
            if not str(ability.get(field) or "").strip():
                raise SmokeFailure(
                    f"ability {ability_id} missing {field} target={ability.get('targetEntry')}"
                )
        if not ability.get("smokeCovered"):
            raise SmokeFailure(
                f"ability {ability_id} is not smoke covered target={ability.get('targetEntry')}"
            )
        by_id[ability_id] = ability

    for required in spec["requiredIds"]:
        if required not in by_id:
            raise SmokeFailure(f"required ability missing: {required}")

    print(
        f"ability matrix: total={total} smoke={covered} "
        f"categories={len(matrix.get('categories') or [])}"
    )
    return by_id


def check_smoke_contract(report: dict, spec: dict, ability_by_id: dict[str, dict]) -> None:
    contract = report.get("smokeContract")
    if contract is None:
        raise SmokeFailure("smokeContract missing from Agent TUI launch report")
    total = int(contract.get("totalPoints", 0))
    required = int(contract.get("requiredPoints", 0))
    if total < spec["minTotalPoints"]:
        raise SmokeFailure(f"smokeContract totalPoints too small: {total}")
    if required < spec["minRequiredPoints"]:
        raise SmokeFailure(f"smokeContract requiredPoints too small: {required}")
    points = contract.get("points") or []
    if len(points) != total:
        raise SmokeFailure(
            f"smokeContract point count mismatch: points={len(points)} total={total}"
        )
    categories = {item.get("category") for item in contract.get("categories") or []}
    for category in spec["categories"]:
        if category not in categories:
            raise SmokeFailure(f"smokeContract category missing: {category}")

    seen: dict[str, dict] = {}
    for point in points:
        point_id = str(point.get("id") or "").strip()
        target = str(point.get("targetEntry") or "")
        if not point_id:
            raise SmokeFailure(f"smoke point failed: id=<missing> target={target} reason=id missing")
        if point_id in seen:
            raise SmokeFailure(f"smoke point failed: id={point_id} target={target} reason=duplicate id")
        for field in ("category", "targetEntry", "evidence", "verification"):
            if not str(point.get(field) or "").strip():
                raise SmokeFailure(
                    f"smoke point failed: id={point_id} target={target} reason={field} missing"
                )
        ability_id = str(point.get("abilityId") or "").strip()
        if ability_id and ability_id not in ability_by_id:
            raise SmokeFailure(
                f"smoke point failed: id={point_id} target={target} "
                f"reason=abilityId not found: {ability_id}"
            )
        seen[point_id] = point

    for required_id in spec["requiredPointIds"]:
        if required_id not in seen:
            raise SmokeFailure(
                f"smoke point failed: id={required_id} target=<missing> "
                "reason=required smoke point missing"
            )
    print(
        f"smoke contract: total={total} required={required} "
        f"categories={len(contract.get('categories') or [])}"
    )


def check_terminal_fixture(fixture: dict) -> None:
    report = run_cli_json(fixture["env"])
    compat = report.get("terminalCompatibility")
    if compat is None:
        raise SmokeFailure(f"{fixture['name']} terminal compatibility snapshot missing")
    if compat.get("profile") != fixture["profile"]:
        raise SmokeFailure(
            f"{fixture['name']} expected profile={fixture['profile']} actual={compat.get('profile')}"
        )
    if compat.get("hyperlinkMode") != fixture["hyperlinkMode"]:
        raise SmokeFailure(
            f"{fixture['name']} expected hyperlink={fixture['hyperlinkMode']} "
            f"actual={compat.get('hyperlinkMode')}"
        )
    if compat.get("mouseMode") != fixture["mouseMode"]:
        raise SmokeFailure(
            f"{fixture['name']} expected mouseMode={fixture['mouseMode']} "
            f"actual={compat.get('mouseMode')}"
        )
    print(
        f"terminal fixture: {fixture['name']} profile={compat.get('profile')} "
        f"hyperlink={compat.get('hyperlinkMode')} copy={compat.get('copyMode')} "
        f"mouseMode={compat.get('mouseMode')} replayRows={compat.get('replayMaxRows')}"
    )


def main() -> int:
    spec = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))
    try:
        report = run_cli_json()
        telemetry = report.get("telemetry")
        if telemetry is None:
            raise SmokeFailure("telemetry snapshot missing from Agent TUI launch report")
        tracing = telemetry.get("tracing")
        if tracing is None:
            raise SmokeFailure("tracing snapshot missing from telemetry")
        if tracing.get("containsSensitiveFields"):
            raise SmokeFailure("tracing snapshot contains sensitive field names")
        compat = report.get("terminalCompatibility")
        if compat is None:
            raise SmokeFailure("terminal compatibility snapshot missing from Agent TUI launch report")

        ability_by_id = check_ability_matrix(report, spec["ability"])
        check_smoke_contract(report, spec["smokeContract"], ability_by_id)

        ttfr = telemetry.get("ttfrMs")
        print(f"TTFR: {'pending' if ttfr is None else str(ttfr) + 'ms'}")
        print(
            f"frame latency: last={telemetry.get('lastFrameLatencyMs')} "
            f"max={telemetry.get('maxFrameLatencyMs')}"
        )
        print(f"frame coalesced: {telemetry.get('frameCoalescedCount')}")
        print(f"stable duplicate frames: {telemetry.get('stableDuplicateFrameCount')}")
        print(f"cache hit rate: {(telemetry.get('cacheHitRateBps') or 0) / 100:.2f}%")
        print(
            f"frame coalescing rate: "
            f"{(telemetry.get('frameCoalescingRateBps') or 0) / 100:.2f}%"
        )
        print(f"tracing spans: {tracing.get('spanCount')}")
        print(
            "terminal compatibility: "
            f"profile={compat.get('profile')} transport={compat.get('transport')} "
            f"osc8={compat.get('osc8Hyperlinks')} osc52={compat.get('osc52Clipboard')} "
            f"mouse={compat.get('mouse')} unicode={compat.get('unicodeWidth')} "
            f"resize={compat.get('resizeReflow')} hyperlink={compat.get('hyperlinkMode')} "
            f"copy={compat.get('copyMode')} mouseMode={compat.get('mouseMode')} "
            f"replayRows={compat.get('replayMaxRows')}"
        )

        for fixture in spec["terminalFixtures"]:
            check_terminal_fixture(fixture)
    except SmokeFailure as error:
        print(f"TUI smoke failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
