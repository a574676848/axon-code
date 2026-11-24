/*
 * 轮次的终态收口：完成、失败、不确定与交互状态。
 *
 * 四类帧说的是同一件事：这一轮不会再有下一段过程。它们只对**尚未收口**的轮次生效，
 * 已结算的轮次不再被迟到帧改写；否则运行侧与调度侧各写一条终态时，界面上会出现
 * 两张失败卡或一条重复的分割线。
 *
 * 幂等不只是「不重复落卡」：迟到的重放帧曾经按到达时刻重算已经结算的用时（实测把
 * 29.9 秒写成 1 分 45 秒），也曾经把已经中断的轮次改写成完成。因此终态帧一律先按
 * 归属找回那一轮，只有它还在跑时才允许写状态与用时；已经收口的轮次最多补齐缺失的
 * 分割线，读数一个字都不改。
 *
 * 归属分两类：完成、失败与不确定由载荷里的 Interaction 标识归属，缺省退回活跃轮次；
 * 交互状态帧的载荷只有一个状态字符串，唯一归属事实是订阅标识，落帧规则见
 * `applyInteractionStatus`。
 */

import { parsePayload, type InteractionStatus } from '../wire';
import { failureReportFromPayload, parseFailureReport, type FailureReport } from '../failureReport';
import {
  activeTurnIndex,
  appendTurnSummary,
  closeAssistant,
  durationFromEvents,
  hasTurnSummary,
  settleTurn,
  upsertCell,
  withTurn,
} from './turns';
import type { ConversationState, TurnState } from './state';

/** 是否还有尚未收口的轮次。 */
export function hasOpenTurn(state: ConversationState): boolean {
  return state.turns.some((turn) => turn.status === 'pending' || turn.status === 'streaming');
}

/**
 * 这一轮是否还没收口。
 *
 * 收口过的轮次已经写下终态与用时，任何后到的帧都只是重复事实，不得再改它。
 */
function isOpenTurn(turn: TurnState | undefined): boolean {
  return turn !== undefined && (turn.status === 'pending' || turn.status === 'streaming');
}

/**
 * 终态帧落到哪一轮。
 *
 * 载荷带着 Interaction 标识时以它为准：上一轮的终态帧重放得比新一轮的提交还晚时，
 * 按「最后一条未收口轮次」会把它结算到新一轮上，新一轮的正文当场被提前收口。
 * 没有标识的老载荷仍然按活跃轮次落帧，口径与既有实现一致。
 */
function targetTurnIndex(
  state: ConversationState,
  interactionId: string | null | undefined,
): number {
  const target = (interactionId ?? '').trim();
  return target.length === 0 ? activeTurnIndex(state) : statusTurnIndex(state, target);
}

/** 失败帧：载荷可能带结构化失败回执，也可能只有一句错误正文。 */
export function applyTurnFailed(
  state: ConversationState,
  payloadJson: string,
  nowMs: number,
  interactionId: string | null = null,
): ConversationState {
  const position = targetTurnIndex(state, interactionId);
  // 已收口：迟到或重复的失败帧不再改状态，也不再补第二张失败卡。
  if (!isOpenTurn(state.turns[position])) return state;
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
  interactionId: string | null = null,
): ConversationState {
  const position = targetTurnIndex(state, interactionId);
  if (!isOpenTurn(state.turns[position])) return state;
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

/**
 * 交互状态帧：只在终态发出，收到即代表**这条 Interaction** 已经结束。
 *
 * 落点由帧的归属决定，不是「最后一条还在跑的轮次」。运行中追加的消息同样是一条
 * Interaction：它被注入活跃回合之后状态即为 `understood`（服务端回放会为它补一条
 * 状态帧，见网关的终态补帧），而那只是「这条消息送进去了」，不是那一轮跑完了。
 * 拿它去收口当前轮次，正在流式输出的一轮会被当场判成完成，于是 `working` 变假、
 * 输入坞的工作状态行（供应商/模型与「另有 N 条等待注入」）连同中断入口一起消失，
 * 而正文其实还在长，并且不会自愈。
 *
 * 归属对不上任何轮次的帧因此一律不改变投影：运行中追加的消息、审计记录、子代理
 * 派发这类 Interaction 本来就不是时间线上的一轮。没有归属标识的老帧（合成帧、
 * 早期载荷）仍按「当前活跃轮次」处理，口径与本次改动之前一致。
 */
export function applyInteractionStatus(
  state: ConversationState,
  status: InteractionStatus,
  nowMs: number,
  interactionId: string,
): ConversationState {
  if (status === 'received' || status === 'processing') return state;
  const position = statusTurnIndex(state, interactionId);
  if (position < 0) return state;
  return withTurn(state, position, (turn) => {
    /*
     * 已收口：这一帧只是重复事实。既不改状态，也不按到达时刻重算用时，
     * 只把可能缺失的分割线补上（收口事件丢失时它就是唯一的收尾）。
     */
    if (!isOpenTurn(turn)) {
      return hasTurnSummary(turn)
        ? turn
        : appendTurnSummary(turn, turn.elapsedMs ?? durationFromEvents(turn, nowMs));
    }
    const mapped: TurnState['status'] =
      status === 'rejected' ? 'rejected' : status === 'interrupted' ? 'interrupted' : 'completed';
    const elapsed = turn.elapsedMs ?? durationFromEvents(turn, nowMs);
    const closed = settleTurn(turn, mapped, elapsed);
    // 终态处理必须幂等：状态帧是这一轮结束的独立事实，
    // 即使收口事件缺失或乱序，分割线也要补齐，不能靠某一帧必到。
    return hasTurnSummary(turn) ? closed : appendTurnSummary(closed, elapsed);
  });
}

/**
 * 状态帧落到哪一轮。
 *
 * 归属标识能对上投影里的某条轮次时才落帧；对不上返回 -1（这一帧不属于任何一轮）。
 * 绑定了交互标识的轮次两个标识相同，比较两个字段是为了兼容只绑定其一的旧投影。
 */
function statusTurnIndex(state: ConversationState, interactionId: string): number {
  const target = interactionId.trim();
  if (target.length === 0) return activeTurnIndex(state);
  return state.turns.findIndex((turn) => turn.id === target || turn.interactionId === target);
}

/** 服务端显式收口的判据到正文。 */
function silentSettleSummary(verdict: string): string {
  switch (verdict) {
    case 'lease_expired':
      return '这一轮已经没有执行者（租约已过期），服务端已把它标记为中断';
    case 'first_token_deadline_exceeded':
      return '首次响应超时，服务端已把这一轮标记为中断';
    case 'no_in_flight_turn':
      return '服务端已经没有这一轮的在途轮次，等于它早就结束了';
    case 'already_settled':
      return '这一轮此前已经在服务端收口';
    default:
      return `服务端结论：${verdict}`;
  }
}

/**
 * 运行证据判过期之后，按服务端判据在本地收口这一轮。
 *
 * 界面不再拿「重启后台服务」当唯一出路：收口由服务端给出判据（租约过期、首字超时、
 * 本来就不在途），界面只把结论落成正文与终态，因此计时停下、工作状态行退场、
 * 正文末尾留一条可追溯的结论行。已经收口的轮次不动，重复调用与迟到帧都安全。
 */
export function applySettledSilentTurn(
  state: ConversationState,
  interactionId: string,
  verdict: string,
  nowMs: number,
): ConversationState {
  const position = statusTurnIndex(state, interactionId);
  if (position < 0) return state;
  return withTurn(state, position, (turn) => {
    if (!isOpenTurn(turn)) return turn;
    const elapsed = turn.elapsedMs ?? durationFromEvents(turn, nowMs);
    const notified = upsertCell(turn, {
      id: `turn-server-settled:${turn.id}`,
      role: 'notice',
      title: '已由服务端收口',
      summary: silentSettleSummary(verdict),
      tone: 'info',
    });
    return appendTurnSummary(settleTurn(notified, 'interrupted', elapsed), elapsed);
  });
}

/** 本轮已有的失败事实；没有失败卡时返回 null。 */
function payloadFailure(turn: TurnState): FailureReport | null {
  const cell = turn.cells.find((candidate) => candidate.role === 'failure');
  return cell !== undefined && cell.role === 'failure' ? cell.report : null;
}

/** 轮次完成帧：中断是独立分支，正文不当作助手回答渲染。 */
export function applyTurnCompleted(
  state: ConversationState,
  response: string | null,
  status: string | null,
  nowMs: number,
  generation: GenerationMetricsPayload | null,
  failure: FailureReport | null,
  interactionId: string | null = null,
): ConversationState {
  const position = targetTurnIndex(state, interactionId);
  // 已收口：迟到的收尾帧不得重写用时、分割线或中断结论。
  if (!isOpenTurn(state.turns[position])) return state;
  return withTurn(state, position, (turn) => {
    if (status === 'interrupted') {
      const elapsed = durationFromEvents(turn, nowMs);
      const settled = settleTurn(turn, 'interrupted', elapsed);
      /*
       * 服务端在中断时已经把结论写成一条助手正文（「本轮已由用户中断。」），
       * 再补一条同义的收尾提示就是重复；只有当正文里没有这句话时（例如中断是
       * 由交互状态帧给出的）才补提示。
       */
      const stated =
        response !== null &&
        turn.cells.some(
          (cell) => cell.role === 'assistant' && cell.text.trim() === response.trim(),
        );
      const summarized = stated
        ? appendTurnSummary(settled, elapsed)
        : appendTurnSummary(
            upsertCell(settled, {
              id: `turn-interrupted:${turn.id}`,
              role: 'notice',
              title: '已中断',
              summary: response ?? '本轮已由用户中断。',
              tone: 'info',
            }),
            elapsed,
          );
      // 中断同样可能带着本轮已测到的首字与解码窗口，不能因为没跑完就把它们丢掉。
      return generation === null ? summarized : withGenerationMetrics(summarized, generation);
    }
    /*
     * 失败终态：正文不是回答，而是后端写下的失败结论。
     * 能解读出结构就给一张失败卡（不再重复渲染那段文本）；解读不出才原样显示。
     */
    if (status === 'failed') {
      const elapsed = durationFromEvents(turn, nowMs);
      const report = failure ?? payloadFailure(turn) ?? parseFailureReport(response ?? '');
      const settled = closeAssistant(
        turn,
        report === null ? response : null,
        turn.lastEventAtMs ?? turn.firstEventAtMs ?? nowMs,
      );
      const withCard =
        report === null
          ? settled
          : upsertCell(settled, {
              id: `turn-failure:${turn.id}`,
              role: 'failure',
              report,
              text: response ?? '',
            });
      const summarized = appendTurnSummary(settleTurn(withCard, 'failed', elapsed), elapsed);
      return generation === null ? summarized : withGenerationMetrics(summarized, generation);
    }
    // 收口补齐的正文：时间取本轮最后一条 durable 事件的记录时间，回放时才是历史时间。
    const settled = closeAssistant(turn, response, turn.lastEventAtMs ?? turn.firstEventAtMs ?? nowMs);
    const elapsed = durationFromEvents(turn, nowMs);
    /*
     * 持久化负载里的生成指标要落在分割线之后：
     * 分割线是这里刚补出来的，指标回填必须看得见它，重放历史时才有首字与速率。
     */
    const summarized = appendTurnSummary(settleTurn(settled, 'completed', elapsed), elapsed);
    return generation === null ? summarized : withGenerationMetrics(summarized, generation);
  });
}

/** 生成指标：实时观测帧与持久收尾负载同名同形。 */
export interface GenerationMetricsPayload {
  ttftMs?: number | null;
  decodeMs?: number | null;
  outputTokens?: number | null;
  reasoningOutputTokens?: number | null;
}

/**
 * 把一组生成指标写进这一轮与它的分割线。
 *
 * 两个来源共用：实时观测帧（本轮正在跑时），以及持久化的收尾负载（重放历史时）。
 * 后者是首字与速率在重启客户端之后仍然存在的前提：观测帧只走广播、不进持久日志。
 */
export function withGenerationMetrics(
  turn: TurnState,
  generation: GenerationMetricsPayload,
): TurnState {
  const decodeMs = generation.decodeMs ?? 0;
  // token 数优先用指标里的；没有时用本轮已收到的用量，
  // 这样解码窗口一到就能算出速率，不必等两个数凑齐。该轮速率含思考输出，与头部同口径。
  const measuredTokens = (generation.outputTokens ?? 0) + (generation.reasoningOutputTokens ?? 0);
  const outputTokens =
    measuredTokens > 0 ? measuredTokens : turn.usage.outputTokens + turn.usage.reasoningOutputTokens;
  const firstTokenAvgMs = generation.ttftMs ?? turn.firstTokenAvgMs;
  const generationTpsX10 =
    decodeMs > 0 && outputTokens > 0
      ? Math.floor((outputTokens * 10_000 + Math.floor(decodeMs / 2)) / decodeMs)
      : turn.generationTpsX10;
  /*
   * Provider 实测的本轮输出按原值留存：用量帧是每条流的累计快照且会重复投递，
   * 会话级速率的分子必须用这一份，才不会被重复帧翻倍。
   */
  const updated: TurnState = {
    ...turn,
    firstTokenAvgMs,
    generationTpsX10,
    decodeMs: decodeMs > 0 ? decodeMs : turn.decodeMs,
    generationOutputTokens:
      measuredTokens > 0 ? (generation.outputTokens ?? 0) : turn.generationOutputTokens,
    generationReasoningTokens:
      measuredTokens > 0
        ? (generation.reasoningOutputTokens ?? 0)
        : turn.generationReasoningTokens,
  };
  // 分割线可能先于指标落地（durable 生命周期事件更早到），
  // 指标到了就回填到那一行，否则用户看到的分割线永远缺首字与速率。
  return {
    ...updated,
    cells: updated.cells.map((cell) =>
      cell.role === 'summary' ? { ...cell, firstTokenAvgMs, generationTpsX10 } : cell,
    ),
  };
}
