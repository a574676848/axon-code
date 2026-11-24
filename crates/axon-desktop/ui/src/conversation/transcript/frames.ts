/*
 * 帧路由与轮次生命周期。
 *
 * 订阅流上只有四类事件：wire 事件、轮次终态、交互状态与运行指标。这里把每一帧
 * 接到对应的归约上，并决定一件更要紧的事：这一帧属于哪一轮。归属规则是「最后
 * 一条尚未收口的轮次」，收口之后到达的迟到帧不再改写已结算的结论。
 *
 * 唯一的例外是交互状态帧：它的载荷只有一个状态字符串，因此归属只认订阅标识
 * （落帧规则在 `terminal.ts` 的 `applyInteractionStatus`）。按「最后一条未收口
 * 轮次」去落，会让别的 Interaction 的收口把一个还在跑的轮次当场判死。
 */

import {
  EVENT_TYPE_INTERACTION_STATUS,
  EVENT_TYPE_RUNTIME_OBSERVATION,
  EVENT_TYPE_TURN_COMPLETED,
  EVENT_TYPE_TURN_FAILED,
  EVENT_TYPE_TURN_UNCERTAIN,
  EVENT_TYPE_WIRE,
  parsePayload,
  subscribedInteractionId,
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
  applyTurnCompletedUsage,
  applyUsage,
  applyUserMessage,
  foldStreamUsage,
} from './cells';
import { failureReportFromPayload } from '../failureReport';
import { replayReceiptOf, replayTruncationNotice } from '../replayTruncation';
import {
  applyInteractionStatus,
  applyTurnCompleted,
  applyTurnFailed,
  applyTurnUncertain,
  hasOpenTurn,
  withGenerationMetrics,
  type GenerationMetricsPayload,
} from './terminal';
import { pruneSettledQuestions } from '../questions';
import { mergeSubagent, subagentRow } from './subagents';
import type { ConversationState, TranscriptCell, TurnState } from './state';
import {
  activeTurnIndex,
  appendTurnSummary,
  durationFromEvents,
  findQuestionCell,
  hasTurnSummary,
  openTurn,
  refreshLiveSummary,
  settleTurn,
  upsertCell,
  withJobLedger,
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
  /*
   * 计划投影不随轮次收口撤下：它挂在输入坞上方的计划坞里，是「这条会话当前的那个计划」。
   *
   * 收口就清掉的话，一轮跑完计划坞会整块消失、下一轮再长回来，读起来像计划丢了；而折叠
   * 只管正文区里的过程行，两者不共享生命周期。换会话由 `reset` 收口，下一次
   * `update_plan` 则整份替换。
   */
  return next;
}

/** 单帧路由：先按事件类型分流，wire 事件内部再由载荷类型二次判别。 */
function applyFrame(
  state: ConversationState,
  frame: ConversationStreamEvent,
  nowMs: number,
): ConversationState {
  if (frame.kind === 'subscribed') {
    // 确认帧带回这次回放是否读到尾：残缺时把事实落到投影上，界面据此说明显示到第几帧。
    return { ...state, connection: 'subscribed', failure: null, replayNotice: replayTruncationNotice(replayReceiptOf(frame)) };
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
    return status === null
      ? withCursor
      : applyInteractionStatus(
          withCursor,
          status,
          nowMs,
          subscribedInteractionId(frame.subscribeId),
        );
  }
  if (frame.eventType === EVENT_TYPE_TURN_COMPLETED) {
    const payload = parsePayload<{
      interactionId?: string;
      response?: string;
      status?: string;
      generation?: GenerationMetricsPayload;
      usage?: WireUsageUpdate;
    }>(frame.payloadJson);
    /*
     * 用量帧在实时流里是「该条 provider 流的快照」，收尾负载另存了同形的一份。
     * 先把还在跑的那条流并入累计：累计一旦有过观测，这份快照就与最后一条流同源，
     * 再应用一次就是重复计入；只有整轮一次都没观测过的历史才拿它兜底。
     * 必须在分割线落地之前并入本轮，否则 Token 一行永远不会出现。
     */
    const withUsage = applyTurnCompletedUsage(
      withCursor,
      payload?.usage ?? null,
      payload?.interactionId ?? null,
    );
    return applyTurnCompleted(
      withUsage,
      payload?.response ?? null,
      payload?.status ?? null,
      nowMs,
      payload?.generation ?? null,
      // 终态载荷里的结构化失败优先；没有时由正文里的回执解读兜底。
      failureReportFromPayload(payload),
      payload?.interactionId ?? null,
    );
  }
  if (frame.eventType === EVENT_TYPE_TURN_FAILED) {
    return applyTurnFailed(withCursor, frame.payloadJson, nowMs, payloadInteractionId(frame.payloadJson));
  }
  if (frame.eventType === EVENT_TYPE_TURN_UNCERTAIN) {
    return applyTurnUncertain(withCursor, frame.payloadJson, nowMs, payloadInteractionId(frame.payloadJson));
  }
  return withCursor;
}

/** 终态载荷里的 Interaction 标识；没有时为空，落帧退回活跃轮次。 */
function payloadInteractionId(payloadJson: string): string | null {
  return parsePayload<{ interactionId?: string }>(payloadJson)?.interactionId ?? null;
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
  const incomingUser = wirePayload<{ partId: string; content: string }>(
    envelope.kind,
    'userMessage',
  );
  /*
   * 独立会话（目标执行、cron）一轮一条轮次：上一条已经收口时，新的用户消息
   * 就是下一轮的开场。没有这条规则，后台跑几轮都会挤进同一条轮次里。
   *
   * 轮次标识优先用帧里的 Interaction 标识：索引骨架轮就以它为标识，若改用
   * 用户消息的 partId（`user:<action-id>`），同一轮会先有一份骨架、再长出一份帧轮，
   * 正文里就会出现两条一样的用户消息。
   */
  if (incomingUser !== null && state.turns.length > 0 && !hasOpenTurn(state)) {
    state = openTurn(
      state,
      envelope.interactionId ?? incomingUser.partId,
      envelope.recordedAtMs,
    );
  }
  if (state.turns.length === 0) {
    if (incomingUser === null && question === null) return state;
    // 轮次键取归属标识：`session` 只是旧载荷的附带事实，子代理的帧不带它，
    // 直接解引用会抛错并丢掉整批帧（现场表现为详情空白）。
    const turnKey = envelope.interactionId ?? envelope.session?.sessionId ?? '';
    if (turnKey.length === 0) return state;
    state = openTurn(state, turnKey, envelope.recordedAtMs);
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
    /*
     * provider 状态帧只驱动本轮实时状态行。
     *
     * 输入坞显示的「会话真实模型」由网关按车道最新一轮记录单条给出（见 history
     * 载荷的 effectiveModel）：它不受回放进度影响，也不会被更早轮次的帧改写。
     */
    return withTurn(state, position, (turn) => ({
      ...turn,
      activity: providerActivity(provider),
    }));
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
    const updated = withTurn(state, position, (turn) =>
      applyToolResult(turn, toolResult, envelope.recordedAtMs),
    );
    return withJobLedger(updated, position, toolResult.partId);
  }

  const fileChange = wirePayload<WireFileChange>(kind, 'fileChange');
  if (fileChange !== null) return withTurn(state, position, (turn) => applyFileChange(turn, fileChange));

  const plan = wirePayload<WirePlanUpdate>(kind, 'planUpdate');
  if (plan !== null) return applyPlan(state, position, plan);

  const usage = wirePayload<WireUsageUpdate>(kind, 'usageUpdate');
  if (usage !== null) {
    // 用量帧一到就把分割线立起来：读数与耗时都是活的，不必等收口才出现。
    return withTurn(state, position, (turn) =>
      refreshLiveSummary(applyUsage(turn, usage), nowMs),
    );
  }

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
    /*
     * 一条 provider 流收口了，但这一轮还在跑（可能接着跑工具、接着换渠道）：
     * 阶段行保留最后一次真实供应商事实，不退回配置里那条从未被请求过的路由。
     * 下一次尝试的 provider 帧会原位覆盖它。
     */
    return withTurn(state, position, (turn) => foldStreamUsage(turn));
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
      // 已收口：一次 run 的终态可能重放（重连、回放与实时各一条），不得重写用时。
      if (turn.status !== 'pending' && turn.status !== 'streaming') return turn;
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
    case 'failover': {
      // 换渠道要把「从哪家换到哪家」都写出来：只写目标的话，前一家已经失败的
      // 事实就从这一行消失了，读起来像是凭空多出一个供应商。
      const next = `${status.nextProvider ?? ''}/${status.nextModel ?? ''}`;
      return next === '/' ? `${route} 正在切换渠道` : `${route} → ${next} 正在切换渠道`;
    }
    default:
      return route;
  }
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