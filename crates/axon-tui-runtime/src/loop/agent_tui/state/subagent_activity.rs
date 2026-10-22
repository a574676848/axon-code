//! Composer 下方子 Agent 活动区状态。

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use axon_protocol::wire::{WireLifecycleStatus, WireTurnLifecycle};

pub(super) use crate::surface::subagent_activity::SUBAGENT_ACTIVITY_KIND;
use crate::surface::subagent_activity::{
    SUBAGENT_ACTIVITY_MAX_VISIBLE_ROWS, SubagentActivityRow, SubagentActivitySnapshot,
    SubagentActivityStatus, SubagentActivityUpdate,
};

use super::AgentTuiState;

const SUBAGENT_WAIT_ELAPSED_PHASE: &str = "wait_elapsed";
const SUBAGENT_WAITING_PHASE_LABEL: &str = "父代理等待结果";
const SUBAGENT_FAST_TASK_PREFIX: &str = "delegate_fast";
const SUBAGENT_FAST_ROLE_LABEL: &str = "Fast";

#[derive(Debug, Clone, Default)]
pub(in crate::r#loop::agent_tui) struct SubagentActivityState {
    entries: BTreeMap<String, SubagentActivityEntry>,
    order: u64,
    suppressed: bool,
}

#[derive(Debug, Clone)]
struct SubagentActivityEntry {
    update: SubagentActivityUpdate,
    elapsed_at_update: Duration,
    last_seen_at: Instant,
    order: u64,
}

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_subagent_activity_lifecycle(
        &mut self,
        lifecycle: WireTurnLifecycle,
        execution_id: Option<String>,
        now: Instant,
    ) {
        let Some(update) = subagent_activity_update(lifecycle, execution_id) else {
            return;
        };
        self.bridge.subagent_activity.apply(update, now);
    }

    pub(in crate::r#loop::agent_tui) fn subagent_activity_snapshot(
        &self,
        now: Instant,
    ) -> SubagentActivitySnapshot {
        self.bridge
            .subagent_activity
            .snapshot(now, self.bridge.spinner_frame)
    }

    pub(in crate::r#loop::agent_tui) fn subagent_activity_visible_height(
        &self,
        now: Instant,
    ) -> u16 {
        self.subagent_activity_snapshot(now).visible_height()
    }

    pub(in crate::r#loop::agent_tui) fn has_visible_subagent_activity(&self, now: Instant) -> bool {
        !self.subagent_activity_snapshot(now).rows.is_empty()
    }
}

impl SubagentActivityState {
    fn apply(&mut self, update: SubagentActivityUpdate, now: Instant) {
        if self.suppressed {
            return;
        }
        if update.phase == "listed" {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&update.agent_id) {
            if entry.update.execution_id != update.execution_id {
                if update.status.is_terminal() {
                    return;
                }
                let order = self.order;
                self.order = self.order.saturating_add(1);
                *entry = SubagentActivityEntry {
                    elapsed_at_update: update.elapsed.unwrap_or_default(),
                    update,
                    last_seen_at: now,
                    order,
                };
                return;
            }
            if update.status.is_terminal() {
                self.entries.remove(&update.agent_id);
                return;
            }
            if is_wait_projection_phase(&update.phase)
                && !is_wait_projection_phase(&entry.update.phase)
            {
                return;
            }
            let current_elapsed = entry.elapsed(now);
            let elapsed = update
                .elapsed
                .map_or(current_elapsed, |incoming| incoming.max(current_elapsed));
            let usage = merge_usage(entry.update.usage, update.usage);
            let cache_hit_rate_basis_points = update
                .cache_hit_rate_basis_points
                .or(entry.update.cache_hit_rate_basis_points);
            let model = update.model.clone().or_else(|| entry.update.model.clone());
            let label = if update.label_from_task {
                update.label.clone()
            } else {
                entry.update.label.clone()
            };
            entry.update = update;
            entry.update.label = label;
            entry.update.usage = usage;
            entry.update.cache_hit_rate_basis_points = cache_hit_rate_basis_points;
            entry.update.model = model;
            entry.elapsed_at_update = elapsed;
            entry.last_seen_at = now;
            return;
        }
        if update.status.is_terminal() {
            return;
        }
        let order = self.order;
        self.order = self.order.saturating_add(1);
        self.entries.insert(
            update.agent_id.clone(),
            SubagentActivityEntry {
                elapsed_at_update: update.elapsed.unwrap_or_default(),
                update,
                last_seen_at: now,
                order,
            },
        );
    }

    fn snapshot(&self, now: Instant, animation_frame: usize) -> SubagentActivitySnapshot {
        let mut entries = self.entries.values().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .last_seen_at
                .cmp(&left.last_seen_at)
                .then_with(|| left.order.cmp(&right.order))
        });
        let rows = entries
            .into_iter()
            .take(SUBAGENT_ACTIVITY_MAX_VISIBLE_ROWS)
            .map(|entry| SubagentActivityRow {
                agent_id: entry.update.agent_id.clone(),
                label: entry.update.label.clone(),
                status: entry.update.status,
                activity: subagent_activity_label(&entry.update.phase, entry.update.status),
                elapsed: entry.elapsed(now),
                usage: entry.update.usage,
                cache_hit_rate_basis_points: entry.update.cache_hit_rate_basis_points,
                model: entry.update.model.clone(),
                summary: entry.update.summary.clone(),
            })
            .collect();
        SubagentActivitySnapshot {
            rows,
            max_visible_rows: SUBAGENT_ACTIVITY_MAX_VISIBLE_ROWS,
            animation_frame,
        }
    }

    pub(in crate::r#loop::agent_tui) fn clear_and_suppress(&mut self) {
        self.entries.clear();
        self.order = 0;
        self.suppressed = true;
    }

    pub(in crate::r#loop::agent_tui) fn resume(&mut self) {
        self.suppressed = false;
    }
}

impl SubagentActivityEntry {
    fn elapsed(&self, now: Instant) -> Duration {
        self.elapsed_at_update
            .saturating_add(now.saturating_duration_since(self.last_seen_at))
    }
}

fn merge_usage(current: Option<u64>, incoming: Option<u64>) -> Option<u64> {
    match (current, incoming) {
        (Some(current), Some(incoming)) => Some(current.max(incoming)),
        (current, None) | (None, current) => current,
    }
}

fn subagent_activity_label(phase: &str, status: SubagentActivityStatus) -> String {
    let label = match phase {
        "started" => "启动中",
        "provider_connecting" => "连接供应商",
        "provider_connected" => "供应商已连接",
        "provider_generating" => "生成中",
        "provider_retrying" => "重试连接",
        "provider_failed" => "请求失败，准备恢复",
        "provider_failover" => "切换供应商",
        "tool_running" => "执行工具",
        "tool_completed" => "整理工具结果",
        "waiting_begin" | "waiting" => SUBAGENT_WAITING_PHASE_LABEL,
        "message_queued" | "followup_queued" => "任务已排队",
        "resumed" | "resuming_begin" => "恢复执行",
        SUBAGENT_WAIT_ELAPSED_PHASE => "运行中",
        "completed" | "closed" => "已完成",
        "interrupted" => "已中断",
        "errored" => "执行失败",
        _ => match status {
            SubagentActivityStatus::Starting => "启动中",
            SubagentActivityStatus::Running => "运行中",
            SubagentActivityStatus::Waiting => "等待中",
            SubagentActivityStatus::Completed => "已完成",
            SubagentActivityStatus::Interrupted => "已中断",
            SubagentActivityStatus::Failed => "执行失败",
        },
    };
    label.to_owned()
}

fn subagent_activity_update(
    lifecycle: WireTurnLifecycle,
    execution_id: Option<String>,
) -> Option<SubagentActivityUpdate> {
    let kind = lifecycle.kind.as_deref()?;
    if kind != SUBAGENT_ACTIVITY_KIND {
        return None;
    }
    let elapsed = subagent_elapsed(&lifecycle, now_millis());
    let phase = lifecycle.ui_phase.unwrap_or_else(|| "unknown".to_owned());
    let summary = lifecycle.summary.filter(|value| !value.trim().is_empty());
    let status = subagent_status(&phase, lifecycle.status);
    let usage = subagent_usage(lifecycle.total_tokens);
    let (label, label_from_task) = subagent_label(&lifecycle.title, summary.as_deref());
    let agent_id = subagent_agent_id(summary.as_deref(), &lifecycle.part_id);
    Some(SubagentActivityUpdate {
        agent_id,
        execution_id,
        label,
        phase,
        status,
        elapsed,
        model: lifecycle.tool_name.filter(|value| !value.trim().is_empty()),
        summary,
        usage,
        cache_hit_rate_basis_points: lifecycle.cache_hit_rate_basis_points,
        label_from_task,
    })
}

fn subagent_elapsed(lifecycle: &WireTurnLifecycle, now_ms: u64) -> Option<Duration> {
    lifecycle
        .duration_ms
        .or_else(|| {
            lifecycle
                .started_at_ms
                .map(|started_at_ms| now_ms.saturating_sub(started_at_ms))
        })
        .map(Duration::from_millis)
}

fn subagent_status(phase: &str, lifecycle_status: WireLifecycleStatus) -> SubagentActivityStatus {
    match phase {
        "started" => SubagentActivityStatus::Starting,
        "waiting_begin" | "waiting" => SubagentActivityStatus::Running,
        "completed" | "closed" => SubagentActivityStatus::Completed,
        "interrupted" => SubagentActivityStatus::Interrupted,
        "errored" => SubagentActivityStatus::Failed,
        "message_queued" | "followup_queued" | "resumed" | "resuming_begin" => {
            SubagentActivityStatus::Running
        }
        _ => match lifecycle_status {
            WireLifecycleStatus::Started => SubagentActivityStatus::Starting,
            WireLifecycleStatus::Waiting => SubagentActivityStatus::Waiting,
            WireLifecycleStatus::Completed => SubagentActivityStatus::Completed,
            WireLifecycleStatus::Failed
            | WireLifecycleStatus::Aborted
            | WireLifecycleStatus::Blocked => SubagentActivityStatus::Failed,
            WireLifecycleStatus::Cancelled => SubagentActivityStatus::Interrupted,
            WireLifecycleStatus::Running => SubagentActivityStatus::Running,
        },
    }
}

pub(super) fn subagent_label(title: &str, summary: Option<&str>) -> (String, bool) {
    if let Some(intent) = summary.and_then(extract_task_intent) {
        let label = shorten_task_intent(intent);
        let role = summary
            .and_then(extract_agent_path)
            .and_then(delegate_role_label);
        return (
            role.map_or_else(|| label.clone(), |role| format!("{role} · {label}")),
            true,
        );
    }
    if let Some(path) = summary.and_then(extract_agent_path) {
        return (semantic_agent_path(path), false);
    }
    if let Some(path) = extract_agent_path(title) {
        return (semantic_agent_path(path), false);
    }
    (
        non_empty_title(title).unwrap_or_else(|| "子代理".to_owned()),
        false,
    )
}

fn extract_task_intent(summary: &str) -> Option<&str> {
    ["已启动：", "任务：", "目标："]
        .iter()
        .find_map(|marker| summary.split_once(marker).map(|(_, value)| value.trim()))
        .filter(|value| !value.is_empty())
}

fn shorten_task_intent(intent: &str) -> String {
    const MAX_TASK_LABEL_CHARS: usize = 18;
    let normalized = intent.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_TASK_LABEL_CHARS {
        return normalized;
    }
    let mut label = normalized
        .chars()
        .take(MAX_TASK_LABEL_CHARS)
        .collect::<String>();
    label.push_str("...");
    label
}

fn semantic_agent_path(path: &str) -> String {
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let mut parts = leaf.split('_').collect::<Vec<_>>();
    let suffix = parts
        .last()
        .filter(|part| part.chars().all(|ch| ch.is_ascii_digit()))
        .map(|part| (*part).to_owned());
    if suffix.is_some() {
        parts.pop();
    }
    let base = parts.join("_");
    let label = match base.as_str() {
        "review" => "代码审查",
        "delegate_fast" => "快速委派",
        "research" => "调研",
        "implement" | "implementation" => "实现",
        "test" => "测试",
        "fix" => "修复",
        "explore" => "探索",
        "plan" => "规划",
        _ => "子任务",
    };
    suffix.map_or_else(|| label.to_owned(), |value| format!("{label} {value}"))
}

fn subagent_agent_id(summary: Option<&str>, fallback: &str) -> String {
    summary
        .and_then(extract_agent_path)
        .map(|path| format!("subagent:{path}"))
        .unwrap_or_else(|| fallback.to_owned())
}

fn non_empty_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

pub(super) fn extract_agent_path(summary: &str) -> Option<&str> {
    summary
        .split(|ch: char| ch.is_whitespace() || matches!(ch, '：' | ':' | '，' | ',' | '。'))
        .find(|token| token.starts_with("/root"))
}

fn delegate_role_label(path: &str) -> Option<&'static str> {
    let leaf = path.rsplit('/').next()?;
    is_delegate_task(leaf, SUBAGENT_FAST_TASK_PREFIX).then_some(SUBAGENT_FAST_ROLE_LABEL)
}

fn is_delegate_task(leaf: &str, prefix: &str) -> bool {
    leaf == prefix
        || leaf
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('_'))
}

fn is_wait_projection_phase(phase: &str) -> bool {
    matches!(
        phase,
        "waiting_begin" | "waiting" | SUBAGENT_WAIT_ELAPSED_PHASE
    )
}

fn subagent_usage(total_tokens: Option<u64>) -> Option<u64> {
    total_tokens.filter(|tokens| *tokens > 0)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}
