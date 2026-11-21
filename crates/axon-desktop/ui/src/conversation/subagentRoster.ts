/*
 * 车道子代理名册与转录行的合并。
 *
 * 子代理事实有两条来源，寿命完全不同：
 *
 * - 转录里的行来自轮次事件流，历史轮一旦折叠（或只按索引立成一行）就随正文一起释放；
 * - 名册来自网关的线程存储，按车道持久化，重新打开会话照样能列出来。
 *
 * 头部入口原先只读转录，于是「会话跑过子代理、重新打开后入口整块消失」。这里把
 * 名册里转录没有的子代理补成行，入口在会话重开后仍然数得出来；状态取末轮状态，
 * 因此补出来的行不会伪装成还在跑。
 */

import type { SessionSubagentSummary } from '../types';
import type { SubagentRow } from './transcript';

/**
 * 代理标识的规范形态：`subagent:/root/x`。
 *
 * 两侧来源不同：转录行的标识来自事件摘要里的绝对路径，网关名册的 `agentId` 来自库里
 * 的 `agent_path`（不带前导斜杠）。匹配与去重都必须先过这一层，否则同一子代理会被
 * 算成两条、入口也永远定位不到。
 */
export function canonicalAgentId(agentId: string | null | undefined): string {
  const raw = (agentId ?? '').trim();
  if (raw.length === 0) return '';
  const path = raw.replace(/^subagent:/, '').replace(/^\/+/, '');
  // 没有层级分隔的标识不是代理路径（例如事件里退化出来的 partId），不参与对账。
  return path.includes('/') ? `subagent:/${path}` : '';
}

/** 名册里的线程状态与末轮状态折算成行状态。 */
function rosterStatus(entry: SessionSubagentSummary): SubagentRow['status'] {
  const last = entry.lastTurnStatus ?? null;
  if (last === 'completed') return 'completed';
  if (last === 'failed') return 'failed';
  if (last === 'interrupted') return 'interrupted';
  const status = entry.status ?? '';
  if (status === 'systemError') return 'failed';
  if (status === 'active' || last === 'active') return 'running';
  // 既没有末轮、也不是在跑：这一次委派没有留下可执行的回合，按已结束呈现，
  // 不写成「运行中」让入口一直亮着。
  return 'completed';
}

/**
 * 把名册里转录没有的子代理补成行。
 *
 * 只补缺，不覆盖：转录里的行带着 token、耗时与阶段，信息比名册多。名册为空
 * （还没对过账）时原样返回转录行，界面不会因此少一行。
 */
export function mergeSubagentRoster(
  rows: readonly SubagentRow[],
  roster: readonly SessionSubagentSummary[] | null,
): SubagentRow[] {
  if (roster === null || roster.length === 0) return [...rows];
  // 转录行与名册行按同一枚规范代理标识对账；空标识的行没有可匹配的对象，直接跳过。
  const known = new Set(rows.map((row) => canonicalAgentId(row.agentId)).filter((id) => id.length > 0));
  // 名册已经知道会话标识：顺手补给转录行，打开子代理视图时不必再定位一次。
  const sessionById = new Map<string, string>();
  for (const entry of roster) {
    const agentId = canonicalAgentId(entry.agentId);
    if (agentId.length > 0 && entry.sessionId !== null && entry.sessionId !== '') {
      sessionById.set(agentId, entry.sessionId);
    }
  }
  const withSession = rows.map((row) => {
    const sessionId = sessionById.get(canonicalAgentId(row.agentId));
    return sessionId === undefined || row.sessionId === sessionId ? row : { ...row, sessionId };
  });
  const extra: SubagentRow[] = [];
  for (const entry of roster) {
    const agentId = canonicalAgentId(entry.agentId);
    if (agentId.length === 0 || known.has(agentId)) continue;
    known.add(agentId);
    extra.push({
      agentId,
      label: (entry.title ?? '').trim() || agentId,
      status: rosterStatus(entry),
      // 名册没有阶段、用量与耗时：留空，渲染层按「缺读数就不占位」处理。
      activity: '',
      elapsedMs: 0,
      lastSeenAtMs: entry.updatedAtMs ?? 0,
      order: extra.length,
      usage: null,
      cacheHitRateBasisPoints: null,
      model: (entry.model ?? '').trim() || null,
      summary: null,
      sessionId: entry.sessionId ?? null,
      events: [],
    });
  }
  if (extra.length === 0) return withSession;
  return [...withSession, ...extra];
}

/** 名册对一行子代理给出的活线索。 */
export interface RosterLiveStatus {
  /** 线程状态：只有 `active` 与 `idle` 两个有效取值。 */
  status: string;
  /** 末轮状态：只有它能给出失败与中断这两种终态。 */
  lastTurnStatus: string | null;
}

/** 行里仍在跑的三档：只有这三档需要被纠正。 */
function claimsRunning(status: SubagentRow['status']): boolean {
  return status === 'running' || status === 'starting' || status === 'waiting';
}

/**
 * 名册与转录冲突时的最终状态。
 *
 * 名册只提供 `active / idle` 线索，它不是运行态本身：线程索引里那一轮没有终态时，
 * 名册会一直说 `active`，而转录里的行也一直停在运行中，两处互相作证，计数永不回落。
 * 因此名册的 `active` 只在本视图确实还有在跑的轮次时才算数；转录一旦收口，行的终态
 * 由末轮状态给出（失败、中断），没有终态事实就按已完成收口。
 *
 * 没对过账（`live` 为空）时原样沿用行内状态：没有第二份事实可比时，纠正就是猜测。
 */
export function reconciledSubagentStatus(
  row: SubagentRow,
  live: RosterLiveStatus | undefined,
  transcriptSettled: boolean,
): SubagentRow['status'] {
  if (live === undefined || live === null) return row.status;
  if (live.status === 'active' && !transcriptSettled) return row.status;
  if (!claimsRunning(row.status)) return row.status;
  if (live.lastTurnStatus === 'failed') return 'failed';
  if (live.lastTurnStatus === 'interrupted') return 'interrupted';
  return 'completed';
}

/**
 * 名册还要不要继续轮询。
 *
 * 名册接口每次都要列线程存储里的子代理目录，并逐个读一次线程末轮状态，代价不在
 * 客户端而在后端，因此不能像时钟一样一直拉：
 *
 * - 这条车道一行子代理都没有：一次都不再拉，界面本来也没有入口可画；
 * - 有一轮正在跑：拉到轮次收口为止，子代理的状态可能还在变；
 * - 轮次已经收口、但行里还挂着未结束：继续拉，直到名册把它判成终态。
 *
 * 其余情况（收口且全部终态）不再请求：历史行是持久事实，不会自己变。
 */
export function shouldPollSubagentRoster(
  working: boolean,
  rows: readonly SubagentRow[],
): boolean {
  if (rows.length === 0) return false;
  if (working) return true;
  return rows.some(
    (row) => row.status === 'running' || row.status === 'starting' || row.status === 'waiting',
  );
}
