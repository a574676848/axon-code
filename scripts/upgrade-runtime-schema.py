#!/usr/bin/env python3
"""一次性运行态库升级（控制面 v10 -> v11）：给 inbox 补 terminal_kind 并升版本号。

这不是产品的迁移通道。目标态策略是「版本不符即拒绝打开、不做 ALTER、不做回填」，
本脚本只用于把本机已有的 v10 控制面库一次性转档，避免升级后必须删库重来。

用法（默认只预览，确认后加 --apply 才写盘）：
    python scripts/upgrade-runtime-schema.py            # 预览
    python scripts/upgrade-runtime-schema.py --apply    # 备份后升级

库路径解析顺序：--db > $AXON_RUNTIME_ROOT/data/control.sqlite3 > $HOME/.axon/data/control.sqlite3。
"""

from __future__ import annotations

import argparse
import os
import sqlite3
import sys
import time
from pathlib import Path

TARGET_VERSION = 11
REQUIRED_COLUMN = "terminal_kind"
REQUIRED_TABLE = "inbox"


def runtime_root(explicit: str | None) -> Path:
    if explicit:
        return Path(explicit)
    env_root = os.environ.get("AXON_RUNTIME_ROOT")
    if env_root:
        return Path(env_root)
    return Path.home() / ".axon"


def read_state(db: Path) -> tuple[int, bool]:
    """只读探针：返回 (user_version, inbox 是否已有 terminal_kind)。"""
    connection = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)
    try:
        version = connection.execute("PRAGMA user_version").fetchone()[0]
        columns = [row[1] for row in connection.execute(f"PRAGMA table_info({REQUIRED_TABLE})")]
        return version, REQUIRED_COLUMN in columns
    finally:
        connection.close()


def backup_database(connection: sqlite3.Connection, db: Path) -> Path:
    """用备份 API 复制一致快照：直接拷贝文件会漏掉 WAL 里的已提交事务。"""
    stamp = time.strftime("%Y%m%d-%H%M%S")
    target = db.with_name(f"{db.name}.bak-{stamp}")
    destination = sqlite3.connect(target)
    try:
        connection.backup(destination)
    finally:
        destination.close()
    return target


def upgrade(db: Path, apply: bool) -> int:
    if not db.exists() or db.stat().st_size == 0:
        print(f"跳过 {db}：库不存在或为空，程序会按 v{TARGET_VERSION} 直接建库")
        return 0
    version, has_column = read_state(db)
    if version == TARGET_VERSION and has_column:
        print(f"{db.name}: 已是 v{TARGET_VERSION} 且包含 {REQUIRED_COLUMN}，无需升级")
        return 0
    if version != 10:
        print(
            f"拒绝升级 {db.name}: 期望 v10，实际 v{version}。"
            "目标态不支持跨版本迁移，请核对来源或重建运行态数据。",
            file=sys.stderr,
        )
        return 2

    print(f"{db.name}: v10 -> v{TARGET_VERSION}，补列 {REQUIRED_TABLE}.{REQUIRED_COLUMN}")
    if not apply:
        print("预览模式：未写盘。确认加 --apply 执行（会先做一致性备份）。")
        return 0

    connection = sqlite3.connect(db)
    try:
        connection.execute("PRAGMA busy_timeout=5000")
        backup = backup_database(connection, db)
        print(f"已备份: {backup}")
        _, has_column = read_state(db)
        connection.execute("BEGIN IMMEDIATE")
        try:
            if not has_column:
                connection.execute(f"ALTER TABLE {REQUIRED_TABLE} ADD COLUMN {REQUIRED_COLUMN} TEXT")
            connection.execute(f"PRAGMA user_version={TARGET_VERSION}")
            connection.commit()
        except Exception:
            connection.rollback()
            raise
    finally:
        connection.close()

    version, has_column = read_state(db)
    ok = version == TARGET_VERSION and has_column
    print(f"{db.name}: 升级校验 {'通过' if ok else '失败'} (v{version}, 列={has_column})")
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="一次性把控制面运行态库从 v10 升到 v11")
    parser.add_argument("--db", help="控制面库路径；缺省按运行态根解析")
    parser.add_argument(
        "--apply",
        action="store_true",
        help="真正写盘（默认只预览）；写盘前会做一次一致性备份",
    )
    args = parser.parse_args()
    db = Path(args.db) if args.db else runtime_root(None) / "data" / "control.sqlite3"
    return upgrade(db, args.apply)


if __name__ == "__main__":
    sys.exit(main())
