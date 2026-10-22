#!/usr/bin/env bash
set -euo pipefail

# Fixed, offline acceptance receipt. Each command exercises the persisted
# target-state contract without requiring credentials or a live provider.
started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cargo test --quiet -p axon-cli --lib doctor_
cargo test --quiet -p axon-session --lib control_and_continuation_priorities_are_ordered
cargo test --quiet -p axon-agent-runtime remote_notification_outbox --lib
cargo test --quiet -p axon-remote-feishu provider_fault_fails_closed_before_delivery_and_keeps_pending_record --lib
cargo test --quiet -p axon-session-runtime permanent_input_errors_dead_letter_on_first_failure --lib
cat <<JSON
{"suite":"jarvis-target-acceptance","startedAt":"$started_at","status":"passed","evidence":["doctor-check","session-action-priority","feishu-lru-outbox","provider-fail-closed","unknown-result-dead-letter"]}
JSON
