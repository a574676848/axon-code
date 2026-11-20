/*
 * 历史一轮的投影：索引事实 + durable 帧 → 可渲染的轮次。
 *
 * 这里只放纯函数，不碰 React、不持有状态：打开会话、展开一轮与增量接管共用同一套
 * 口径，任何一条路径改了投影规则，另外两条自动跟上。
 *
 * 三条硬规则：
 * - **有帧就纯按帧投影**，不叠索引骨架。骨架自带的收口帧会先把这一轮判成结束，随后
 *   回放帧会另起一轮，过程行与上下文读数因此整段丢失。
 * - **分割线以索引读数为准**。回放可能因为帧预算只读到尾窗，而 `turn_metrics` 是这一
 *   轮读数的权威事实；索引给不出时才退回帧算出来的那份。
 * - **窗口占用只认收尾负载**。索引里没有窗口占用，不编近似值；上下文读数等回放补。
 * - **在跑的一轮不收口**。收口是服务端的事实（终态帧，或索引给出的 lifecycle），不是
 *   回放的默认值：把还在流式输出的一轮判成完成，`working` 会当场变假，输入坞的工作
 *   状态行、中断入口与侧栏的转圈一起消失，而正文其实还在长。
 */

import {
  INITIAL_CONVERSATION,
  applyFrames,
  conversationReducer,
  totalTokens,
  type JobRow,
  type TranscriptCell,
  type TurnState,
} from './transcript';
import {
  applyTurnSummary,
  seedTurnReadings,
  settleTurn,
  upsertCell,
  type IndexReadings,
  type TurnSummaryFacts,
} from './transcript/turns';
import type { SessionJobFact, TurnMetricsReading } from '../types';
import type { ConversationStreamEvent } from './wire';
import type { AttachmentRef } from './transcript';

/** 历史索引里一条轮次的补充事实：帧里没有、只有索引才有的那几项。 */
export interface HistoryFacts {
  /** 入站时落地的本地附件绝对路径。 */
  attachments: AttachmentRef[];
  /** 这一轮由哪个渠道发起；逐轮标注来源要用它。 */
  channel: string;
}

/**
 * 一轮的来源：逐轮客户端优先，缺省回落到渠道。
 *
 * 同一条会话里每一轮都必须用同一份判据，否则历史轮与实时轮会给出两套来源口径。
 */
export function historySource(item: { channel?: string; client?: string }): string {
  const client = item.client ?? '';
  return client !== '' ? client : (item.channel ?? '');
}

/**
 * 索引里的 lifecycle 是否表示「这一轮还没收口」。
 *
 * `received` 是排队待领、`processing` 是已被主 turn 领取；`understood`、`rejected`、
 * `interrupted` 都是终态。回放与骨架都只认这一条判据，「在不在跑」因此不会出现第二
 * 套说法。
 */
export function interactionStillRunning(status: string): boolean {
  return status === 'received' || status === 'processing';
}

/**
 * 回放投影的调用方事实：中段是否截断、这一轮是否已收口。
 *
 * 这两项都只有取数那一层知道，投影不自己推断：推断出来的收口会让运行中的一轮当场
 * 变成「已完成」。
 */
export interface FrameProjectionOptions {
  /** 中段是否被页数上限略过。 */
  truncated?: boolean;
  /** 这一轮在服务端是否已经收口；缺省为真（调用点确认过是历史）。 */
  settled?: boolean;
}

/**
 * 只按帧投影一轮（打开会话的最新一轮与展开共用）。
 *
 * 不叠索引骨架：骨架里那条合成的用户消息与帧里的真实用户消息各成一格，正文里就会
 * 出现两个一样的气泡。附件与来源从索引贴进来，帧里没有这两样东西；分割线的读数以
 * 索引里的 `turn_metrics` 定稿，回放只负责把过程正文长回来。
 *
 * 回放因页数上限只取到「首页 + 尾窗」时，这一轮中间一段是真的没有读到；`truncated`
 * 就是那件事，界面上要如实说一句，而不是把略过的过程当成从没发生过。
 *
 * 收口与起跑时刻都由调用方给的事实决定，投影不替服务端下结论：`settled` 为假表示这
 * 一轮还在跑，投影保持 `streaming`，收口交给随后到达的终态帧。
 */
export function projectFrames(
  interactionId: string,
  frames: readonly ConversationStreamEvent[],
  facts: HistoryFacts | undefined,
  reading: TurnMetricsReading | undefined,
  fallbackElapsedMs: number | null,
  options: FrameProjectionOptions = {},
): { turn: TurnState; jobs: JobRow[] } {
  const { truncated = false, settled = true } = options;
  const nowMs = Date.now();
  /*
   * 起点先在本地兜一档：帧自带记录时刻时，归约器会用首条 durable 事件把它盖掉
   * （见 `turnStartFromEvent`），因此这里只在没有可用帧时生效。用索引用时还原，
   * 而不是用回放这一刻，运行中的耗时才不会从零重算。
   */
  const startedAtMs = nowMs - Math.max(0, fallbackElapsedMs ?? 0);
  let state = conversationReducer(INITIAL_CONVERSATION, {
    type: 'open-turn',
    turnId: interactionId,
    nowMs: startedAtMs,
  });
  if (facts !== undefined) {
    state = conversationReducer(state, {
      type: 'attach-history',
      interactionId,
      attachments: facts.attachments,
      channel: facts.channel,
    });
  }
  state = applyFrames(state, [...frames], nowMs);
  const turn = state.turns[0];
  if (turn === undefined) {
    return { turn: state.turns[0], jobs: state.jobs };
  }
  const observed = truncated ? upsertCell(turn, truncationCell(interactionId)) : turn;
  const open = observed.status === 'streaming' || observed.status === 'pending';
  const closed =
    open && settled
      ? settleTurn(observed, 'completed', observed.elapsedMs ?? fallbackElapsedMs)
      : observed;
  const seeded = seedTurnReadings(closed, readingFactsOf(reading));
  return {
    turn: applyTurnSummary(seeded, turnSummaryFacts(reading, fallbackElapsedMs, seeded)),
    jobs: state.jobs,
  };
}

/** 中段被页数上限略过时的一句如实说明：不冒充过程完整。 */
function truncationCell(interactionId: string): TranscriptCell {
  return {
    id: `${interactionId}:truncated`,
    role: 'notice',
    title: '过程已截断',
    summary: '这一轮的过程过长，中间一段没有读回来；开头的用户消息与最近的记录都在。',
    tone: 'info',
  };
}

/** 索引读数换成归约器认得的形状：只带 token 分桶，窗口占用等收尾负载。 */
export function readingFactsOf(reading: TurnMetricsReading | undefined): IndexReadings | null {
  if (reading === undefined) return null;
  const measured =
    reading.inputTokens +
    reading.cacheReadTokens +
    reading.cacheWriteTokens +
    reading.outputTokens +
    reading.reasoningTokens;
  return {
    activeMs: reading.activeMs,
    ttftMs: reading.ttftMs ?? null,
    generationTpsX10: reading.generationSpeedTpsX10 ?? null,
    usage:
      measured > 0
        ? {
            inputTokens: reading.inputTokens,
            cacheReadInputTokens: reading.cacheReadTokens,
            cacheWriteInputTokens: reading.cacheWriteTokens,
            outputTokens: reading.outputTokens,
            reasoningOutputTokens: reading.reasoningTokens,
            // 窗口占用只由收尾负载给出：索引里没有这一项，不在这里编一个近似值。
            currentContextTokens: 0,
            systemPromptTokens: 0,
            toolDefinitionTokens: 0,
            conversationTokens: 0,
          }
        : null,
  };
}

/**
 * 一轮的分割线读数。
 *
 * 索引里的 `turn_metrics` 是这一轮读数的权威事实（与 `session_metrics` 同源），因此
 * 只要它在就用它；老记录没有指标事件时才退回帧算出来的那份。用量整段为零时不冒充
 * 一份有效读数：那一行会只留用时，绝不写出一排 `Token 0`。
 */
export function turnSummaryFacts(
  reading: TurnMetricsReading | undefined,
  fallbackElapsedMs: number | null,
  turn: TurnState,
): TurnSummaryFacts {
  const measuredTokens =
    reading === undefined
      ? 0
      : reading.inputTokens +
        reading.cacheReadTokens +
        reading.cacheWriteTokens +
        reading.outputTokens +
        reading.reasoningTokens;
  const usage: TurnSummaryFacts['usage'] =
    reading !== undefined && measuredTokens > 0
      ? {
          inputTokens: reading.inputTokens,
          cacheReadInputTokens: reading.cacheReadTokens,
          cacheWriteInputTokens: reading.cacheWriteTokens,
          outputTokens: reading.outputTokens,
          reasoningOutputTokens: reading.reasoningTokens,
          // 窗口占用只由实时用量帧给出：索引里没有这一项，不在这里编一个近似值。
          currentContextTokens: 0,
          systemPromptTokens: 0,
          toolDefinitionTokens: 0,
          conversationTokens: 0,
        }
      : totalTokens(turn.usage) > 0
        ? turn.usage
        : null;
  const duration = reading?.activeMs ?? fallbackElapsedMs ?? turn.elapsedMs ?? null;
  const firstTokenAvgMs = reading?.ttftMs ?? turn.firstTokenAvgMs;
  const generationTpsX10 =
    reading?.generationSpeedTpsX10 !== undefined && reading.generationSpeedTpsX10 !== null
      ? reading.generationSpeedTpsX10
      : turn.generationTpsX10;
  return {
    durationMs: duration !== null && duration > 0 ? duration : null,
    usage,
    firstTokenAvgMs,
    generationTpsX10,
  };
}

/** 一条合成 wire 帧：索引给出的事实按事件形态喂进同一个归约器。 */
export function wireFrame(
  interactionId: string,
  recordedAtMs: number,
  kind: string,
  payload: unknown,
): ConversationStreamEvent {
  return {
    kind: 'frame',
    subscribeId: `interaction:${interactionId}`,
    eventType: 'wire_event',
    cursor: null,
    payloadJson: JSON.stringify({
      schemaVersion: 'axon.wire.v3',
      eventId: `${interactionId}:${recordedAtMs}`,
      correlationId: 'history',
      session: { sessionId: interactionId },
      sequence: 1,
      recordedAtMs,
      kind: { type: kind, payload },
    }),
  };
}

/** 收口帧：索引里的最终交付正文按 `turn_completed` 形态喂进去。 */
export function completedFrame(
  interactionId: string,
  response: string,
): ConversationStreamEvent {
  return {
    kind: 'frame',
    subscribeId: `interaction:${interactionId}`,
    eventType: 'turn_completed',
    cursor: null,
    payloadJson: JSON.stringify({ interactionId, response, status: 'completed' }),
  };
}

/** 交互状态帧：中断轮按同一形态收口。 */
export function statusFrame(
  interactionId: string,
  recordedAtMs: number,
  status: string,
): ConversationStreamEvent {
  void recordedAtMs;
  return {
    kind: 'frame',
    subscribeId: `interaction:${interactionId}`,
    eventType: 'interaction.status',
    cursor: null,
    payloadJson: JSON.stringify(status),
  };
}

/** 把折叠行插在开场输入之后（这一轮只有索引时用它当展开入口）。 */
export function insertControlAfterInput(turn: TurnState, control: TranscriptCell): TurnState {
  const cells: TranscriptCell[] = [...turn.cells];
  let lead = 0;
  while (lead < cells.length && cells[lead].role === 'user') lead += 1;
  cells.splice(lead, 0, control);
  const index: Record<string, number> = {};
  cells.forEach((cell, position) => {
    index[cell.id] = position;
  });
  const summary = control.role === 'folded' ? control.summary : '';
  return { ...turn, cells, index, folded: true, summary };
}

/**
 * 会话级作业事实 → 台账行。
 *
 * 作业属于整条会话：打开历史会话时，早先几轮跑过的作业只在这份清单里。行标识带前缀，
 * 与帧里派生的行（按 tool part 标识）分开，两边不会互相覆盖。
 */
export function sessionJobRows(jobs: readonly SessionJobFact[] | undefined): JobRow[] {
  if (jobs === undefined || jobs.length === 0) return [];
  return jobs.map((job) => ({
    id: `session-job:${job.jobId}`,
    command: job.kind ?? '',
    status: job.status ?? 'running',
    output: '',
    durationMs: job.elapsedMs ?? null,
    startedAtMs: job.startedAtMs ?? null,
    exitCode: job.exitCode ?? null,
    jobId: job.jobId,
    jobStatus: job.status ?? null,
    fullOutputAvailable: false,
  }));
}