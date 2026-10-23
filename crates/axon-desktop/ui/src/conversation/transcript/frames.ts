/*
 * 帧路由与轮次生命周期。
 *
 * 订阅流上只有四类事件：wire 事件、轮次终态、交互状态与运行指标。这里把每一帧
 * 接到对应的归约上，并决定一件更要紧的事：这一帧属于哪一轮。归属规则是「最后
 * 一条尚未收口的轮次」，收口之后到达的迟到帧不再改写已结算的结论。
 */

import {
  EVENT_TYPE_INTERACTION_STATUS,
  EVENT_TYPE_RUNTIME_OBSERVATION,
  EVENT_TYPE_TURN_COMPLETED,
  EVENT_TYPE_TURN_FAILED,
  EVENT_TYPE_WIRE,
  parsePayload,
  wirePayload,
  type ConversationStreamEvent,
  type InteractionStatus,
  type RuntimeObservationFrame,
  type WireAssistantMessage,
  type WireContextRollover,
  type WireEventEnvelope,
  type WireFileChange,
  type WirePlanUpdate,
  type WireProviderStatus,
  type WireReasoningDelta,
  type WireRuntimeFailure,
  type WireTokenBudgetReminder,
  type WireToolCall,
  type WireToolResult,
  type WireTurnLifecycle,
  type WireUsageUpdate,
  type WireUserQuestion,
} from '../wire';
import {
  applyAssistant,
  applyContextRollover,
  applyFileChange,
  applyPlan,
  applyReasoningDelta,
  applyReasoningEnd,
  applyReasoningStart,
  applyRuntimeError,
  applyTokenBudgetReminder,
  applyToolCall,
  applyToolResult,
  applyUsage,
  applyUserMessage,
  foldStreamUsage,
} from './cells';
import { failureReportFromPayload, parseFailureReport, type FailureReport } from '../failureReport';
import { pruneSettledQuestions } from '../questions';
import { mergeSubagent, subagentRow } from './subagents';
import type { ConversationState, TranscriptCell, TurnState } from './state';
import {
  activeTurnIndex,
  appendTurnSummary,
  closeAssistant,
  durationFromEvents,
  findQuestionCell,
  hasTurnSummary,
  openTurn,
  settleTurn,
  upsertCell,
  withTurn,
} from './turns';

/** 逐帧归约。 */
export function applyFrames(
  state: ConversationState,
  frames: ConversationStreamEvent[],
  nowMs: number,
): ConversationState {
  let next: ConversationState = {
    ...state,
    turns: state.turns.map((turn) => ({ ...turn })),
  };
  for (const frame of frames) {
    next = applyFrame(next, frame, nowMs);
  }
  return withoutSettledPlan(next);
}

/**
 * 整轮收口即撤掉计划投影。
 *
 * 固定计划坞读的是「会话当前计划」：这一轮跑完之后它已经过期，继续钉在输入坞上方
 * 只会让人以为还在执行。时间线里的计划单元是每次 planUpdate 的原子记录，不在这里
 * 清除，下一轮再给出 planUpdate 时投影照常长回来。
 */
function withoutSettledPlan(state: ConversationState): ConversationState {
  if (state.plan === null) return state;
  const running = state.turns.some(
    (turn) => turn.status === 'pending' || turn.status === 'streaming',
  );
  return running ? state : { ...state, plan: null };
}

/** 单帧路由：先按事件类型分流，wire 事件内部再由载荷类型二次判别。 */
function applyFrame(
  state: ConversationState,
  frame: ConversationStreamEvent,
  nowMs: number,
): ConversationState {
  if (frame.kind === 'subscribed') {
    return { ...state, connection: 'subscribed', failure: null };
  }
  if (frame.kind === 'failure') {
    return {
      ...state,
      connection: 'failed',
      failure: { message: frame.message, retryable: frame.retryable },
    };
  }
  const cursor = frame.cursor ?? state.cursor;
  const withCursor = cursor === state.cursor ? state : { ...state, cursor };
  if (frame.eventType === EVENT_TYPE_WIRE) {
    const envelope = parsePayload<WireEventEnvelope>(frame.payloadJson);
    if (envelope === null) return withCursor;
    return applyWireEvent(withCursor, envelope, nowMs);
  }
  if (frame.eventType === EVENT_TYPE_RUNTIME_OBSERVATION) {
    const observation = parsePayload<RuntimeObservationFrame>(frame.payloadJson);
    return observation === null ? withCursor : applyObservation(withCursor, observation);
  }
  if (frame.eventType === EVENT_TYPE_INTERACTION_STATUS) {
    const status = parsePayload<InteractionStatus>(frame.payloadJson);
    return status === null ? withCursor : applyInteractionStatus(withCursor, status, nowMs);
  }
  if (frame.eventType === EVENT_TYPE_TURN_COMPLETED) {
    const payload = parsePayload<{
      interactionId?: string;
      response?: string;
      status?: string;
      generation?: GenerationMetricsPayload;
    }>(frame.payloadJson);
    return applyTurnCompleted(
      withCursor,
      payload?.response ?? null,
      payload?.status ?? null,
      nowMs,
      payload?.generation ?? null,
      // 终态载荷里的结构化失败优先；没有时由正文里的回执解读兜底。
      failureReportFromPayload(payload),
    );
  }
  if (frame.eventType === EVENT_TYPE_TURN_FAILED) {
    return applyTurnFailed(withCursor, frame.payloadJson, nowMs);
  }
  return withCursor;
}

/** 一条 wire 事件落到当前活跃轮次上。 */
function applyWireEvent(
  state: ConversationState,
  envelope: WireEventEnvelope,
  nowMs: number,
): ConversationState {
  // UQ-102: 提问状态权威驻留于 state.pendingQuestions，零轮次或历史卸载不影响其权威性
  const question = wirePayload<WireUserQuestion>(envelope.kind, 'userQuestion');
  if (question !== null) {
    /*
     * 提问状态只能向前推进：对账把权威的「已结算」写进单元之后，重放的历史帧
     * 不允许把它改回待答态（迟到作答只有结算事件，没有可依附的帧）。单元的
     * 状态因此同样按单调规则夹一次，待答位与单元取同一个结论。
     */
    const previous = findQuestionCell(state.turns, question.callId);
    const resolved = state.resolvedQuestions[question.callId];
    const status = questionSettled(state, question.callId) ? 'settled' : question.status;
    state = {
      ...state,
      // 新提问到达时清掉已结算的旧条目：待答位只关心还需要用户回答的那几条，
      // 结算过的留着既没有消费者，也会随会话时长无界增长。
      pendingQuestions: {
        ...pruneSettledQuestions(state.pendingQuestions, question.callId),
        [question.callId]: {
          partId: question.partId,
          callId: question.callId,
          status,
          questions: question.questions,
          answers: question.answers ?? previous?.answers ?? resolved?.answers,
          handoff: question.handoff,
          deadlineAtMs: question.deadlineAtMs,
        },
      },
    };
  }

  // 一条会话流可以从一条用户消息开始：子代理会话就是这样（它的第一条事实是
  // 委派给它的任务）。没有这条规则，零回合的会话流会被整段丢弃。
  if (state.turns.length === 0) {
    const opening = wirePayload<{ partId: string; content: string }>(
      envelope.kind,
      'userMessage',
    );
    if (opening === null && question === null) return state;
    state = openTurn(state, envelope.session.sessionId, envelope.recordedAtMs);
  }
  const position = activeTurnIndex(state);
  if (position < 0) return state;
  const kind = envelope.kind;
  state = withTurn(state, position, (turn) => ({
    ...turn,
    /*
     * 起点以服务端记录的第一条事件为准。
     *
     * 进入一条车道时，回放会把「我开始看」的那一刻当成开轮时间；如果这条轮次其实
     * 已经跑了二十分钟，工作状态行就会显示成刚起步。首条 durable 事件的记录时间
     * 早于本地开轮时间时，后者只是观察时刻，必须让位。
     */
    startedAtMs: turnStartFromEvent(turn, envelope.recordedAtMs),
    firstEventAtMs: turn.firstEventAtMs ?? envelope.recordedAtMs,
    lastEventAtMs: envelope.recordedAtMs,
  }));

  const lifecycle = wirePayload<WireTurnLifecycle>(kind, 'turnLifecycle');
  if (lifecycle !== null) return applyLifecycle(state, position, lifecycle, nowMs, envelope);

  const provider = wirePayload<WireProviderStatus>(kind, 'providerStatus');
  if (provider !== null) {
    return withTurn(state, position, (turn) => ({ ...turn, activity: providerActivity(provider) }));
  }

  const assistant = wirePayload<WireAssistantMessage>(kind, 'assistantMessage');
  if (assistant !== null) return applyAssistant(state, position, assistant, envelope.recordedAtMs);

  const reasoningDelta = wirePayload<WireReasoningDelta>(kind, 'reasoningDelta');
  if (reasoningDelta !== null) {
    return withTurn(state, position, (turn) => applyReasoningDelta(turn, reasoningDelta));
  }
  const reasoningStart = wirePayload<{ partId: string }>(kind, 'reasoningStart');
  if (reasoningStart !== null) {
    return withTurn(state, position, (turn) =>
      applyReasoningStart(turn, reasoningStart.partId, envelope.recordedAtMs),
    );
  }
  const reasoningEnd = wirePayload<{ partId: string }>(kind, 'reasoningEnd');
  if (reasoningEnd !== null) {
    return withTurn(state, position, (turn) =>
      applyReasoningEnd(turn, reasoningEnd.partId, envelope.recordedAtMs, nowMs),
    );
  }

  const user = wirePayload<{ partId: string; content: string }>(kind, 'userMessage');
  if (user !== null) {
    return applyUserMessage(state, position, user.partId, user.content, envelope.recordedAtMs);
  }

  const toolCall = wirePayload<WireToolCall>(kind, 'toolCall');
  if (toolCall !== null) {
    return withTurn(state, position, (turn) =>
      applyToolCall(turn, toolCall, envelope.recordedAtMs),
    );
  }

  const toolResult = wirePayload<WireToolResult>(kind, 'toolResult');
  if (toolResult !== null) {
    return withTurn(state, position, (turn) =>
      applyToolResult(turn, toolResult, envelope.recordedAtMs),
    );
  }

  const fileChange = wirePayload<WireFileChange>(kind, 'fileChange');
  if (fileChange !== null) return withTurn(state, position, (turn) => applyFileChange(turn, fileChange));

  const plan = wirePayload<WirePlanUpdate>(kind, 'planUpdate');
  if (plan !== null) return applyPlan(state, position, plan);

  const usage = wirePayload<WireUsageUpdate>(kind, 'usageUpdate');
  if (usage !== null) return withTurn(state, position, (turn) => applyUsage(turn, usage));

  const error = wirePayload<WireRuntimeFailure>(kind, 'runtimeError');
  if (error !== null) {
    return withTurn(state, position, (turn) =>
      applyRuntimeError(turn, envelope.eventId, error),
    );
  }

  const reminder = wirePayload<WireTokenBudgetReminder>(kind, 'tokenBudgetReminder');
  if (reminder !== null) {
    return withTurn(state, position, (turn) =>
      applyTokenBudgetReminder(turn, envelope.eventId, reminder),
    );
  }

  const rollover = wirePayload<WireContextRollover>(kind, 'contextRollover');
  if (rollover !== null) {
    return withTurn(state, position, (turn) =>
      applyContextRollover(turn, envelope.eventId, rollover),
    );
  }

  if (question !== null) {
    // 单元与待答位取同一个状态结论：已结算的不被重放的历史帧改回待答态。
    const previousCell = findQuestionCell(state.turns, question.callId);
    const status = questionSettled(state, question.callId) ? 'settled' : question.status;
    /*
     * 等待与延长相位不物化时间线单元：同一件待答事实此刻由输入坞卡片完整呈现
     * （问题、选项、倒计时、作答入口），时间线再画一遍就是同一件事说两次。
     * 提问挂起或结算后卡片让位，这时才落单元，并由此承担回入口与状态留痕。
     */
    if (status === 'pending' || status === 'extended') {
      return state;
    }
    const cell: TranscriptCell = {
      id: question.partId,
      role: 'question',
      callId: question.callId,
      status,
      questions: question.questions,
      answers:
        question.answers ??
        previousCell?.answers ??
        state.resolvedQuestions[question.callId]?.answers,
      handoff: question.handoff,
      deadlineAtMs: question.deadlineAtMs,
      // 时间取信封的记录时刻：与消息页脚同一口径，便于把「这条什么时候问的」对齐到正文。
      atMs: envelope.recordedAtMs,
    };
    return withTurn(state, position, (turn) => upsertCell(turn, cell));
  }

  return state;
}

/** 轮次生命周期：压缩过程落历史行，子代理活动落面板，常规轮次终态收口。 */
function applyLifecycle(
  state: ConversationState,
  position: number,
  lifecycle: WireTurnLifecycle,
  nowMs: number,
  envelope: WireEventEnvelope,
): ConversationState {
  if (lifecycle.kind === 'subagent_activity') {
    const row = subagentRow(lifecycle, envelope, nowMs);
    return row === null ? state : { ...state, subagents: mergeSubagent(state.subagents, row) };
  }
  /*
   * 窗口轮转按 kind 归位，不按 phase：内联压缩的 phase 同样是 regularTurn，
   * 只有 uiPhase 标成 history。按 phase 判定会让「窗口轮转失败」落进下面的终态
   * 分支，把还在执行的整轮误判成失败。
   */
  if (isCompactionKind(lifecycle.kind)) {
    return withTurn(state, position, (turn) =>
      upsertCell(turn, {
        id: lifecycle.partId,
        role: 'history',
        title: lifecycle.title.length > 0 ? lifecycle.title : COMPACTION_TITLE,
        status: compactionStatus(lifecycle.kind),
        summary: lifecycle.summary ?? '',
      }),
    );
  }
  if (lifecycle.kind === STREAMING_STARTED_KIND) {
    // 上一条流若没等到收口帧（失败重试），在开新流时先把它的用量并入累计，读数不丢。
    return withTurn(state, position, (turn) => foldStreamUsage(turn));
  }
  if (lifecycle.kind === 'StreamingCompleted') {
    return withTurn(state, position, (turn) => ({
      ...foldStreamUsage(turn),
      activity: null,
    }));
  }
  /*
   * 只有 RunLifecycle 是整轮自己的终态事实（一次 run 一条），它比收口事件更可靠：
   * 终态帧缺失时靠它收口这一轮并补上分割线，界面不会停在「生成中」。
   *
   * StreamingStarted / StreamingCompleted / ProviderRetry 都是轮次内部的阶段事件，
   * 一次 run 里会出现很多条：拿它们收口会把还在执行的轮次提前判死，工作状态行随之
   * 消失、失败横幅常驻，实时首字也无处落地。
   */
  if (
    lifecycle.kind === RUN_LIFECYCLE_KIND &&
    (lifecycle.status === 'completed' || lifecycle.status === 'failed')
  ) {
    return withTurn(state, position, (turn) => {
      const elapsed = turn.elapsedMs ?? durationFromEvents(turn, nowMs);
      const settled = settleTurn(
        turn,
        lifecycle.status === 'failed' ? 'failed' : 'completed',
        elapsed,
      );
      return hasTurnSummary(turn) ? settled : appendTurnSummary(settled, elapsed);
    });
  }
  return state;
}

/** 整轮自身终态的生命周期类别：一次 run 只发一条，收口只看它。 */
const RUN_LIFECYCLE_KIND = 'RunLifecycle';
/** 一条 provider 流开始的生命周期类别；用量快照按流记账的分界。 */
const STREAMING_STARTED_KIND = 'StreamingStarted';
/** 窗口轮转（压缩）生命周期类别；与后端 run_compaction_projection 的判定同集。 */
const COMPACTION_KINDS = [
  'CompactionStarted',
  'CompactionCompleted',
  'CompactionFailed',
  'CompactionPlanned',
];
/** 窗口轮转的缺省标题。 */
const COMPACTION_TITLE = '窗口轮转';

/** 是否为窗口轮转的生命周期类别。 */
function isCompactionKind(kind: string | undefined): boolean {
  return kind !== undefined && COMPACTION_KINDS.includes(kind);
}

/** 压缩阶段状态文案。 */
function compactionStatus(kind: string | undefined): string {
  switch (kind) {
    case 'CompactionStarted':
      return '进行中';
    case 'CompactionCompleted':
      return '完成';
    case 'CompactionFailed':
      return '失败';
    case 'CompactionPlanned':
      return '已排期';
    default:
      return '历史';
  }
}

/** provider 实时阶段文案。 */
function providerActivity(status: WireProviderStatus): string {
  const route = `${status.provider}/${status.model}`;
  switch (status.phase) {
    case 'connecting':
      return `${route} 正在连接`;
    case 'connected':
      return `${route} 已连接`;
    case 'generating':
      return `${route} 生成中`;
    case 'retrying':
      return `${route} 传输重试（第 ${status.attemptIndex} 次）`;
    case 'failed':
      return status.errorMessage !== undefined
        ? `${route} 请求失败：${status.errorMessage}`
        : `${route} 请求失败`;
    case 'failover':
      return `切换供应商：${status.nextProvider ?? ''}/${status.nextModel ?? ''}`;
    default:
      return route;
  }
}

/** 交互状态帧：只在终态发出，收到即代表这一轮真的结束。 */
function applyInteractionStatus(
  state: ConversationState,
  status: InteractionStatus,
  nowMs: number,
): ConversationState {
  if (status === 'received' || status === 'processing') return state;
  const position = activeTurnIndex(state);
  if (position < 0) return state;
  const mapped: TurnState['status'] =
    status === 'rejected' ? 'rejected' : status === 'interrupted' ? 'interrupted' : 'completed';
  return withTurn(state, position, (turn) => {
    const elapsed = turn.elapsedMs ?? durationFromEvents(turn, nowMs);
    const closed = settleTurn(
      turn,
      turn.status === 'completed' && mapped === 'completed' ? turn.status : mapped,
      elapsed,
    );
    // 终态处理必须幂等：状态帧是这一轮结束的独立事实，
    // 即使收口事件缺失或乱序，分割线也要补齐，不能靠某一帧必到。
    return hasTurnSummary(turn) ? closed : appendTurnSummary(closed, elapsed);
  });
}

/** 轮次完成帧：中断是独立分支，正文不当作助手回答渲染。 */
/// 本轮已有的失败事实；没有失败卡时返回 null。
function payloadFailure(turn: TurnState): FailureReport | null {
  const cell = turn.cells.find((candidate) => candidate.role === 'failure');
  return cell !== undefined && cell.role === 'failure' ? cell.report : null;
}

function applyTurnCompleted(
  state: ConversationState,
  response: string | null,
  status: string | null,
  nowMs: number,
  generation: GenerationMetricsPayload | null,
  failure: FailureReport | null,
): ConversationState {
  const position = activeTurnIndex(state);
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

/** 轮次失败帧。 */
function applyTurnFailed(state: ConversationState, payloadJson: string, nowMs: number): ConversationState {
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

/** 运行指标帧：只写指标，不产生正文单元。 */
function applyObservation(
  state: ConversationState,
  frame: RuntimeObservationFrame,
): ConversationState {
  const next: ConversationState = {
    ...state,
    lspActiveCount: frame.lsp?.activeClients ?? state.lspActiveCount,
    activeTurnCount: frame.gateway?.activeTurnCount ?? state.activeTurnCount,
  };
  // 未采集的子结构在线上是 null 而不只是缺字段，两者都必须视作「本帧无该指标」。
  if (frame.generation === undefined || frame.generation === null) return next;
  const position = activeTurnIndex(next);
  if (position < 0) return next;
  const turn = next.turns[position];
  // 收尾指标帧按广播投递，可能属于别的会话：turnId 为空表示全局帧，
  // 非空时必须与本轮一致，否则会把别人的速率写到这一轮上。
  if (frame.turnId !== undefined && frame.turnId !== '' && frame.turnId !== turn.id) return next;
  // 乱序帧丢弃；终态回合不再丢弃，因为首字与速率正是在收口那一刻才补齐的。
  if (frame.generatedAtMs < next.lastObservationAtMs) {
    return { ...next, observation: frame, lastObservationAtMs: Math.max(next.lastObservationAtMs, frame.generatedAtMs) };
  }
  const generation = frame.generation;
  return {
    ...withTurn(next, position, (turn) => withGenerationMetrics(turn, generation)),
    observation: frame,
    lastObservationAtMs: Math.max(next.lastObservationAtMs, frame.generatedAtMs),
  };
}

/** 生成指标：实时观测帧与持久收尾负载同名同形。 */
interface GenerationMetricsPayload {
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
function withGenerationMetrics(turn: TurnState, generation: GenerationMetricsPayload): TurnState {
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

/** 本轮真正的起点：首条 durable 事件的记录时间不晚于本地开轮时间时以它为准。 */
function turnStartFromEvent(turn: TurnState, recordedAtMs: number): number | null {
  if (turn.firstEventAtMs !== null) return turn.startedAtMs;
  if (turn.startedAtMs === null) return recordedAtMs;
  return Math.min(turn.startedAtMs, recordedAtMs);
}

/**
 * 这条提问是否已经有权威的「已结算」结论。
 *
 * 两个来源都要看：时间线上的单元（结算帧或早先的对账写过它），以及对账台账
 * （结论先于回放到达时留下的那一份）。只看单元会漏掉后者：对账先到、单元还没
 * 被回放出来，随后到达的挂起帧就会在时间线上重新长出一张「已挂起」的单元。
 */
function questionSettled(state: ConversationState, callId: string): boolean {
  if (state.resolvedQuestions[callId] !== undefined) return true;
  return findQuestionCell(state.turns, callId)?.status === 'settled';
}