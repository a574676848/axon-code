/*
 * 子代理投影。
 *
 * 事实源是 `turnLifecycle` 上带 `kind = "subagent_activity"` 的真实事件：后端没有
 * 「列出子代理」的接口，因此面板只能由这一串事件驱动，口径与 TUI 的子代理活动区一致。
 * 终态保留在面板里而不是即时移除，方便回看刚刚跑完的是什么。
 */

import type { WireEventEnvelope, WireTurnLifecycle } from '../wire';
import { MAX_SUBAGENT_EVENTS, type SubagentRow } from './state';
import { elapsedSince } from './turns';

/** 从一条生命周期事件投影出子代理行；列目录事件不产生行。 */
export function subagentRow(
  lifecycle: WireTurnLifecycle,
  envelope: WireEventEnvelope,
  nowMs: number,
): SubagentRow | null {
  if (lifecycle.uiPhase === 'listed') return null;
  const phase = lifecycle.uiPhase ?? 'unknown';
  const summary = lifecycle.summary?.trim();
  const agentId = summary !== undefined && summary.length > 0
    ? `subagent:${extractAgentPath(summary) ?? lifecycle.partId}`
    : lifecycle.partId;
  return {
    agentId,
    label: subagentLabel(lifecycle.title, summary),
    status: subagentStatus(phase, lifecycle.status),
    activity: subagentActivityLabel(phase, lifecycle.status),
    elapsedMs: lifecycle.durationMs ?? elapsedSince(lifecycle.startedAtMs ?? null, nowMs) ?? 0,
    lastSeenAtMs: nowMs,
    order: envelope.sequence,
    usage: (lifecycle.totalTokens ?? 0) > 0 ? (lifecycle.totalTokens ?? null) : null,
    cacheHitRateBasisPoints: lifecycle.cacheHitRateBasisPoints ?? null,
    model: lifecycle.toolName?.trim().length ? (lifecycle.toolName ?? null) : null,
    summary: summary !== undefined && summary.length > 0 ? summary : null,
    events: [
      {
        atMs: envelope.recordedAtMs ?? nowMs,
        phase,
        status: lifecycle.status,
        title: lifecycle.title,
        summary: summary !== undefined && summary.length > 0 ? summary : null,
      },
    ],
  };
}

/** 合并一条子代理事实；轨迹只追加，不被后一条覆盖。 */
export function mergeSubagent(rows: SubagentRow[], incoming: SubagentRow): SubagentRow[] {
  const position = rows.findIndex((row) => row.agentId === incoming.agentId);
  if (position < 0) return [...rows, incoming];
  const previous = rows[position];
  const merged: SubagentRow = {
    ...previous,
    ...incoming,
    events: [...previous.events, ...incoming.events].slice(-MAX_SUBAGENT_EVENTS),
    elapsedMs: Math.max(previous.elapsedMs, incoming.elapsedMs),
    usage: incoming.usage ?? previous.usage,
    cacheHitRateBasisPoints:
      incoming.cacheHitRateBasisPoints ?? previous.cacheHitRateBasisPoints,
    model: incoming.model ?? previous.model,
    label: incoming.label.length > 0 ? incoming.label : previous.label,
  };
  const next = [...rows];
  next[position] = merged;
  return next;
}

/** 取 summary 中的 AgentPath。 */
function extractAgentPath(summary: string): string | null {
  const token = summary
    .split(/[\s：:，,。]/)
    .find((part) => part.startsWith('/root'));
  return token ?? null;
}

/** 子代理标签的可见字符上限。 */
const MAX_SUBAGENT_LABEL_CHARS = 18;

/** 从标题与摘要里取一个可读的短标签。 */
function subagentLabel(title: string, summary: string | undefined): string {
  if (summary !== undefined) {
    const marker = ['已启动：', '任务：', '目标：'].find((candidate) => summary.includes(candidate));
    if (marker !== undefined) {
      const intent = summary.slice(summary.indexOf(marker) + marker.length).trim();
      if (intent.length > 0) {
        const normalized = intent.split(/\s+/).join(' ');
        return normalized.length > MAX_SUBAGENT_LABEL_CHARS
          ? `${normalized.slice(0, MAX_SUBAGENT_LABEL_CHARS)}...`
          : normalized;
      }
    }
    const path = extractAgentPath(summary);
    if (path !== null) return semanticAgentPath(path);
  }
  const titlePath = extractAgentPath(title);
  if (titlePath !== null) return semanticAgentPath(titlePath);
  return title.trim().length > 0 ? title.trim() : '子代理';
}

/** 把委派类型翻译成人话。 */
function semanticAgentPath(path: string): string {
  const leaf = path.split('/').pop() ?? path;
  const parts = leaf.split('_');
  const suffix = /^\d+$/.test(parts[parts.length - 1] ?? '') ? parts.pop() : undefined;
  const base = parts.join('_');
  const label: Record<string, string> = {
    review: '代码审查',
    delegate_fast: '快速委派',
    research: '调研',
    implement: '实现',
    implementation: '实现',
    test: '测试',
    fix: '修复',
    explore: '探索',
    plan: '规划',
  };
  const name = label[base] ?? '子任务';
  return suffix === undefined ? name : `${name} ${suffix}`;
}

/** 阶段与状态到面板状态的映射。 */
function subagentStatus(phase: string, status: string): SubagentRow['status'] {
  switch (phase) {
    case 'started':
      return 'starting';
    case 'waiting_begin':
    case 'waiting':
    case 'message_queued':
    case 'followup_queued':
    case 'resumed':
    case 'resuming_begin':
      return 'running';
    case 'completed':
    case 'closed':
      return 'completed';
    case 'interrupted':
      return 'interrupted';
    case 'errored':
      return 'failed';
    default:
      break;
  }
  switch (status) {
    case 'started':
      return 'starting';
    case 'waiting':
      return 'waiting';
    case 'completed':
      return 'completed';
    case 'failed':
    case 'aborted':
    case 'blocked':
      return 'failed';
    case 'cancelled':
      return 'interrupted';
    default:
      return 'running';
  }
}

/** 阶段到活动文案的映射。 */
function subagentActivityLabel(phase: string, status: string): string {
  const labels: Record<string, string> = {
    started: '启动中',
    provider_connecting: '连接供应商',
    provider_connected: '供应商已连接',
    provider_generating: '生成中',
    provider_retrying: '重试连接',
    provider_failed: '请求失败，准备恢复',
    provider_failover: '切换供应商',
    tool_running: '执行工具',
    tool_completed: '整理工具结果',
    waiting_begin: '父代理等待结果',
    waiting: '父代理等待结果',
    message_queued: '任务已排队',
    followup_queued: '任务已排队',
    resumed: '恢复执行',
    resuming_begin: '恢复执行',
    wait_elapsed: '运行中',
    completed: '已完成',
    closed: '已完成',
    interrupted: '已中断',
    errored: '执行失败',
  };
  const direct = labels[phase];
  if (direct !== undefined) return direct;
  const fallback: Record<SubagentRow['status'], string> = {
    starting: '启动中',
    running: '运行中',
    waiting: '等待中',
    completed: '已完成',
    interrupted: '已中断',
    failed: '执行失败',
  };
  return fallback[subagentStatus(phase, status)];
}
