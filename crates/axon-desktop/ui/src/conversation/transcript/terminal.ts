/*
 * 轮次的失败与不确定收口。
 *
 * 这两类帧说的是同一件事：这一轮没有正常结论。它们只对**尚未收口**的轮次生效，
 * 已结算的轮次不再被迟到帧改写；否则运行侧与调度侧各写一条终态时，界面上会出现
 * 两张失败卡或一条重复的分割线。
 */

import { parsePayload } from '../wire';
import { failureReportFromPayload, parseFailureReport, type FailureReport } from '../failureReport';
import {
  activeTurnIndex,
  appendTurnSummary,
  durationFromEvents,
  settleTurn,
  upsertCell,
  withTurn,
} from './turns';
import type { ConversationState } from './state';

/** 是否还有尚未收口的轮次。 */
export function hasOpenTurn(state: ConversationState): boolean {
  return state.turns.some((turn) => turn.status === 'pending' || turn.status === 'streaming');
}

/** 失败帧：载荷可能带结构化失败回执，也可能只有一句错误正文。 */
export function applyTurnFailed(
  state: ConversationState,
  payloadJson: string,
  nowMs: number,
): ConversationState {
  if (!hasOpenTurn(state)) return state;
  const position = activeTurnIndex(state);
  const payload = parsePayload<{ message?: string; reason?: string; failure?: FailureReport }>(
    payloadJson,
  );
  const message = payload?.message ?? payload?.reason ?? '本轮执行失败';
  return withTurn(state, position, (turn) => {
    const elapsed = durationFromEvents(turn, nowMs);
    const warned = upsertCell(turn, {
      id: `turn-failed:${turn.id}`,
      role: 'failure',
      // 载荷里已有结构化失败时优先用它；否则从正文里的回执解读，解读不出就只显示原文。
      report: payload?.failure ?? failureReportFromPayload(payload) ?? parseFailureReport(message),
      text: message,
    });
    return appendTurnSummary(settleTurn(warned, 'failed', elapsed), elapsed);
  });
}

/**
 * 结果不确定帧。
 *
 * 后台执行在进程中断、租约失效这类情况下无法证明外部副作用是否发生，
 * 调度侧据此给出不确定结论。轮次必须在此收口，否则界面会一直停在运行态。
 */
export function applyTurnUncertain(
  state: ConversationState,
  payloadJson: string,
  nowMs: number,
): ConversationState {
  if (!hasOpenTurn(state)) return state;
  const position = activeTurnIndex(state);
  const payload = parsePayload<{ reason?: string; classification?: string }>(payloadJson);
  return withTurn(state, position, (turn) => {
    const elapsed = durationFromEvents(turn, nowMs);
    const notified = upsertCell(turn, {
      id: `turn-uncertain:${turn.id}`,
      role: 'notice',
      title: '结果不确定',
      summary: uncertainSummary(payload?.reason),
      tone: 'danger',
    });
    return appendTurnSummary(settleTurn(notified, 'failed', elapsed), elapsed);
  });
}

/** 不确定原因的人话；未知取值如实回落，不猜。 */
function uncertainSummary(reason: string | undefined): string {
  if (reason === 'process_interrupted') return '执行进程中断，本轮结果未确认。';
  if (reason === undefined || reason.trim().length === 0) return '本轮结果未确认。';
  return `本轮结果未确认（${reason}）。`;
}