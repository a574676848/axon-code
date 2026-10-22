#!/usr/bin/env bash
set -euo pipefail

# 真机重启验收（R9-P2；复用 R7-01 / R7-02 / R7-09 基础能力）：
# 对常驻守护进程发送 SIGKILL，验证宿主监管自动拉起新实例，
# 且新实例以新的 generation 建立 Host lease 并继续响应 health。
#
# 该脚本会触碰真实运行时，必须显式开启：
#   JARVIS_REAL_DEVICE=1 tools/verify-daemon-restart.sh
# 可用环境变量覆盖默认约定：
#   JARVIS_DAEMON_PATTERN             守护进程命令行匹配串（默认 "axon daemon run"）
#   JARVIS_DAEMON_RESTART_WAIT_SECONDS 等待重启的上限秒数（默认 30）
#   JARVIS_DAEMON_POLL_INTERVAL_SECONDS 轮询间隔秒数（默认 1）

if [[ "${JARVIS_REAL_DEVICE:-0}" != "1" ]]; then
  echo "请设置 JARVIS_REAL_DEVICE=1 后再运行守护进程重启验收" >&2
  exit 2
fi

DAEMON_PATTERN="${JARVIS_DAEMON_PATTERN:-axon daemon run}"
RESTART_WAIT_SECONDS="${JARVIS_DAEMON_RESTART_WAIT_SECONDS:-30}"
POLL_INTERVAL_SECONDS="${JARVIS_DAEMON_POLL_INTERVAL_SECONDS:-1}"

for command_name in python3 pgrep; do
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    echo "缺少命令 ${command_name}，无法执行重启验收" >&2
    exit 3
  fi
done

read_field() {
  python3 -c 'import json,sys; print(json.load(sys.stdin)'"$1"')'
}

started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

echo "==> 重启前 daemon status"
cargo run --quiet -p axon-cli -- daemon status

health_before="$(cargo run --quiet -p axon-cli -- daemon health)"
generation_before="$(printf '%s' "${health_before}" | read_field "['host']['generation']")"

pids_before="$(pgrep -f "${DAEMON_PATTERN}" || true)"
if [[ -z "${pids_before}" ]]; then
  echo "未找到匹配 '${DAEMON_PATTERN}' 的守护进程，无法验证宿主监管；请用 JARVIS_DAEMON_PATTERN 指定。" >&2
  exit 4
fi
killed_pid="${pids_before%%$'\n'*}"

echo "==> 以 SIGKILL 终止守护进程 ${killed_pid}"
kill -9 "${killed_pid}"

restarted_pid=""
deadline=$(( $(date +%s) + RESTART_WAIT_SECONDS ))
while [[ $(date +%s) -lt ${deadline} ]]; do
  sleep "${POLL_INTERVAL_SECONDS}"
  for candidate in $(pgrep -f "${DAEMON_PATTERN}" || true); do
    if [[ "${candidate}" != "${killed_pid}" ]]; then
      restarted_pid="${candidate}"
      break
    fi
  done
  if [[ -n "${restarted_pid}" ]]; then
    break
  fi
done

if [[ -z "${restarted_pid}" ]]; then
  echo "守护进程未在 ${RESTART_WAIT_SECONDS} 秒内被宿主监管拉起" >&2
  exit 5
fi
echo "==> 宿主监管已拉起新进程 ${restarted_pid}"

echo "==> 重启后 daemon status"
cargo run --quiet -p axon-cli -- daemon status

health_after="$(cargo run --quiet -p axon-cli -- daemon health)"
generation_after="$(printf '%s' "${health_after}" | read_field "['host']['generation']")"
lease_remaining_ms="$(printf '%s' "${health_after}" | read_field "['host']['leaseRemainingMs']")"

if [[ "${generation_after}" == "${generation_before}" ]]; then
  echo "重启后 generation 未变化（${generation_after}），新实例未接管 Host lease" >&2
  exit 6
fi
if [[ "${lease_remaining_ms}" -le 0 ]]; then
  echo "重启后 Host lease 未生效（leaseRemainingMs=${lease_remaining_ms}）" >&2
  exit 7
fi

cat <<JSON
{"suite":"jarvis-r9-daemon-restart","startedAt":"${started_at}","status":"passed","killedPid":${killed_pid},"restartedPid":${restarted_pid},"generationBefore":"${generation_before}","generationAfter":"${generation_after}","leaseRemainingMs":${lease_remaining_ms}}
JSON
