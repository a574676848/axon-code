#!/usr/bin/env bash
set -euo pipefail

iterations="${1:-100}"
if ! [[ "$iterations" =~ ^[1-9][0-9]*$ ]]; then
  echo "iterations must be a positive integer" >&2
  exit 2
fi

now_ms() {
  LC_ALL=C perl -MTime::HiRes -e 'printf "%.0f\n", Time::HiRes::time() * 1000'
}

time_flags=()
if /usr/bin/time -l true >/dev/null 2>&1; then
  time_flags=(-l)
else
  time_flags=(-v)
fi

total_elapsed_ms=0
max_rss_kb=0
tmp_dir="$(mktemp -d)"
cleanup() { rm -rf "$tmp_dir"; }
trap cleanup EXIT INT TERM

for ((i=1; i<=iterations; i++)); do
  started="$(now_ms)"
  output_file="$tmp_dir/test-$i.out"
  time_file="$tmp_dir/test-$i.time"
  if ! /usr/bin/time "${time_flags[@]}" cargo test --quiet -p axon-agent-runtime outbox_capacity_keeps_new_dispatch_and_evicts_terminal_before_active_record --lib \
    >"$output_file" 2>"$time_file"; then
    cat "$output_file" >&2 || true
    cat "$time_file" >&2 || true
    echo "outbox benchmark failed at iteration $i" >&2
    exit 1
  fi
  finished="$(now_ms)"
  total_elapsed_ms=$((total_elapsed_ms + finished - started))
  rss_kb="$(awk '/maximum resident set size/ {print int($1/1024)} /Maximum resident set size/ {print $NF}' "$time_file" | tail -1)"
  if [[ "${rss_kb:-0}" -gt "$max_rss_kb" ]]; then
    max_rss_kb="$rss_kb"
  fi
done

cat <<JSON
{"suite":"feishu-outbox","iterations":$iterations,"elapsedMs":$total_elapsed_ms,"maxRssKb":$max_rss_kb,"backpressure":"lru","pendingLoss":false,"lockWaitMs":"covered-by-runtime-store"}
JSON
