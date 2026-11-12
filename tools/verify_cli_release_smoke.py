#!/usr/bin/env python3
"""CLI 发行包冒烟的跨平台实现（release 构建、doctor、package 与清单校验）。"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent


def run(label: str, args: list[str]) -> None:
    print(f"==> {label}")
    result = subprocess.run(args, cwd=REPO_ROOT)
    if result.returncode != 0:
        print(f"{label} failed.", file=sys.stderr)
        raise SystemExit(1)


def main() -> int:
    run("cargo build release axon-cli", ["cargo", "build", "-p", "axon-cli", "--release"])
    run(
        "axon doctor provider",
        ["cargo", "run", "--quiet", "-p", "axon-cli", "--", "doctor", "--check", "provider"],
    )
    run(
        "axon doctor smoke",
        ["cargo", "run", "--quiet", "-p", "axon-cli", "--", "doctor", "--check", "smoke"],
    )

    package_dir = REPO_ROOT / "dist" / "smoke"
    if package_dir.exists():
        shutil.rmtree(package_dir)
    run(
        "axon package",
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "axon-cli",
            "--",
            "package",
            "--output-dir",
            str(package_dir),
        ],
    )

    manifests = sorted(package_dir.rglob("axon-release-manifest.json"))
    if not manifests:
        print("package smoke failed: manifest not found.", file=sys.stderr)
        return 1

    run(
        "axon package verify",
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "axon-cli",
            "--",
            "package",
            "--verify-manifest",
            str(manifests[0]),
        ],
    )
    print("CLI release smoke OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
