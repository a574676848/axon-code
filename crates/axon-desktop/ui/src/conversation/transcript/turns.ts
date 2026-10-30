/*
 * 轮次与单元的原语。
 *
 * 归约的每一步都经由这里的不可变更新函数：替换某一轮、追加或原位覆盖单元、
 * 收口轮次终态。所有更新都返回新对象，界面拿到的是新引用，因此不需要额外的
 * 变更标记，也不会出现同一份事实被就地改写的第二份副本。
 */

import { EMPTY_USAGE, LOCAL_STEER_PREFIX, totalTokens, type ConversationState, type TranscriptCell, type TurnState } from './state';
import { EMPTY_TURN_CHANGES } from './changes';

/** 新建一个空轮次。 */
export function emptyTurn(turnId: string, nowMs: number): TurnState {
  return {
    id: turnId,
    interactionId: null,
    cells: [],
    index: {},
    status: 'streaming',
    usage: { ...EMPTY_USAGE },
    usageFolded: { ...EMPTY_USAGE },
    usageStream: null,
    startedAtMs: nowMs,
    elapsedMs: null,
    firstTokenAvgMs: null,
    generationTpsX10: null,
    decodeMs: null,
    generationOutputTokens: null,
    generationReasoningTokens: null,
    activity: null,
    firstEventAtMs: null,
    lastEventAtMs: null,
    changes: EMPTY_TURN_CHANGES,
    pendingAttachments: [],
    folded: false,
    summary: '',
    keepExpanded: false,
  };
}

/** 本地乐观插入的用户消息身份。 */
export function localUserId(turnId: string): string {
  return `local-user:${turnId}`;
}

/** 开一条新轮次；已存在同标识轮次时原样返回。 */
export function openTurn(state: ConversationState, turnId: string, nowMs: number): ConversationState {
  if (state.turns.some((turn) => turn.id === turnId)) return state;
  return {
    ...state,
    turns: [...state.turns, emptyTurn(turnId, nowMs)],
    connection: 'connecting',
    failure: null,
  };
}

/** 当前活跃轮次：最后一条尚未收口的轮次，缺省时用最后一条真实轮次。 */
export function activeTurnIndex(state: ConversationState): number {
  for (let position = state.turns.length - 1; position >= 0; position -= 1) {
    const status = state.turns[position].status;
    if (status === 'pending' || status === 'streaming') return position;
  }
  // 未送达的轮次从未到达后端，不会有属于它的帧：迟到帧要落到最后一条真实轮次上，
  // 否则一次发送失败会把随后的服务端结论写到一条后端根本不知道的消息上。
  for (let position = state.turns.length - 1; position >= 0; position -= 1) {
    if (state.turns[position].status !== 'undelivered') return position;
  }
  return state.turns.length - 1;
}

/** 以不可变方式替换某一轮。 */
export function withTurn(
  state: ConversationState,
  position: number,
  update: (turn: TurnState) => TurnState,
): ConversationState {
  if (position < 0 || position >= state.turns.length) return state;
  const turns = [...state.turns];
  turns[position] = update(turns[position]);
  return { ...state, turns };
}

/** 追加或原位覆盖一个单元。 */
export function upsertCell(turn: TurnState, cell: TranscriptCell): TurnState {
  const known = turn.index[cell.id];
  if (known !== undefined) {
    const cells = [...turn.cells];
    cells[known] = cell;
    return { ...turn, cells };
  }
  return {
    ...turn,
    cells: [...turn.cells, cell],
    index: { ...turn.index, [cell.id]: turn.cells.length },
  };
}

/**
 * 找出提问单元：它是提问状态的长期记忆。
 *
 * 待答位只保留还没答完的条目，答完即离场，因此「这条已经结算过」这一事实只能
 * 由单元记住；重放历史帧时按它把状态夹住，才不会被更旧的帧改回待答态。
 */
export function findQuestionCell(
  turns: readonly TurnState[],
  callId: string,
): Extract<TranscriptCell, { role: 'question' }> | undefined {
  for (const turn of turns) {
    for (const cell of turn.cells) {
      if (cell.role === 'question' && cell.callId === callId) return cell;
    }
  }
  return undefined;
}

/** 就地改写一个已知单元；单元不存在时原样返回。 */
export function patchCell(
  turn: TurnState,
  cellId: string,
  patch: (cell: TranscriptCell) => TranscriptCell,
): TurnState {
  const known = turn.index[cellId];
  if (known === undefined) return turn;
  const cells = [...turn.cells];
  cells[known] = patch(cells[known]);
  return { ...turn, cells };
}

/** 让运行中的轮次耗时持续走字。 */
export function refreshLiveElapsed(turn: TurnState, nowMs: number): TurnState {
  // `pending` 也要走字：提交之后、首个流式帧到达之前的等待同样属于「这次请求
  // 已经发出多久」，只认 `streaming` 会让工作状态行的耗时空着一段才出现。
  if (
    (turn.status !== 'pending' && turn.status !== 'streaming') ||
    turn.startedAtMs === null
  ) {
    return turn;
  }
  return { ...turn, elapsedMs: nowMs - turn.startedAtMs };
}

/** 相对某个起点的耗时。 */
export function elapsedSince(startedAtMs: number | null, nowMs: number): number | null {
  return startedAtMs === null ? null : Math.max(0, nowMs - startedAtMs);
}

/**
 * 从 durable 事件时间戳推导本轮耗时。
 *
 * 回放一条历史会话时，帧是在毫秒内集中到达的，用本地墙钟会把真实耗时算成几十毫秒；
 * 事件自带的记录时间才是这一轮真正花了多久。缺任一端时回落到墙钟。
 */
export function durationFromEvents(turn: TurnState, nowMs: number): number | null {
  if (turn.firstEventAtMs !== null && turn.lastEventAtMs !== null) {
    const duration = turn.lastEventAtMs - turn.firstEventAtMs;
    if (duration > 0) return duration;
  }
  return elapsedSince(turn.startedAtMs, nowMs);
}

/** 本轮是否已经有收尾分割线。 */
export function hasTurnSummary(turn: TurnState): boolean {
  return turn.cells.some((cell) => cell.role === 'summary');
}

/**
 * 收口一个轮次：写终态字段，并把还没被服务端认领的乐观排队消息标为未送达。
 *
 * 这类消息只有三种归宿：被注入本轮（durable 事件按正文认领它，标识早已换掉）、
 * 落 durable 队列等下一轮（那条 Interaction 会作为新一轮重放出来），或随着本轮
 * 一起被取消。第三种情况下内容是找不回来的，因此**不删除这一格**：留在正文里
 * 标成未送达，用户看得见、也点得到「重新发送」，而不是让消息静默消失。
 */
export function settleTurn(
  turn: TurnState,
  status: TurnState['status'],
  elapsedMs: number | null,
): TurnState {
  const settled = closeRunningCells(markUnclaimedSteersUndelivered(turn), status);
  return { ...settled, status, activity: null, elapsedMs };
}

/** 把仍在等待注入的本地乐观用户消息标成未送达；没有时原样返回。 */
function markUnclaimedSteersUndelivered(turn: TurnState): TurnState {
  let changed = false;
  const cells = turn.cells.map((cell) => {
    if (
      cell.role !== 'user' ||
      !cell.id.startsWith(LOCAL_STEER_PREFIX) ||
      cell.undelivered ||
      // 已经交给转投队列的那几条不算未送达：它们马上会作为下一条消息送出。
      cell.requeued
    ) {
      return cell;
    }
    changed = true;
    return { ...cell, undelivered: true };
  });
  return changed ? { ...turn, cells } : turn;
}

/**
 * 只保留指定助手单元的流式光标。
 *
 * 一轮里 agent 会交替输出「正文 → 工具 → 正文 → 工具」，光标必须只跟着正在书写的那
 * 一段走：`partId` 有值时只保留它，传 null（发生了工具调用、思考等非正文事件）时全部
 * 关掉。否则每写完一段就留下一根常亮竖线，一轮下来正文后面挂满光标。
 */
export function streamOnlyAssistant(turn: TurnState, partId: string | null): TurnState {
  let changed = false;
  const cells = turn.cells.map((cell) => {
    if (cell.role !== 'assistant' || !cell.streaming) return cell;
    if (partId !== null && cell.id === partId) return cell;
    changed = true;
    return { ...cell, streaming: false };
  });
  return changed ? { ...turn, cells } : turn;
}

/**
 * 收口仍在「运行中」的单元。
 *
 * 轮次结束之后不会再有增量到达：助手正文要停掉流式光标，思考要停掉转圈。
 * 被中断的轮次里还没落定结果的工具行同理：服务端对未启动的调用给出的结论就是
 * 「已中断」，留着转圈等于永远在跑（中断后残留的那根光标就是这么来的）。
 */
function closeRunningCells(turn: TurnState, status: TurnState['status']): TurnState {
  let changed = false;
  const cells = turn.cells.map((cell) => {
    if (cell.role === 'assistant' && cell.streaming) {
      changed = true;
      return { ...cell, streaming: false };
    }
    if (cell.role === 'reasoning' && !cell.done) {
      changed = true;
      return { ...cell, done: true };
    }
    if (cell.role === 'tool' && cell.status === 'running' && status === 'interrupted') {
      changed = true;
      return { ...cell, status: 'interrupted' };
    }
    return cell;
  });
  return changed ? { ...turn, cells } : turn;
}

/**
 * 摘掉指定单元并重建索引。
 *
 * 只有「这一格已经离开本轮」时才用：它必须连同索引一起消失，否则同一标识
 * 再插回来会命中旧下标，正文里那一格就永远不会被替换。
 */
export function detachCells(turn: TurnState, cellIds: readonly string[]): TurnState {
  const dropping = new Set(cellIds);
  const cells = turn.cells.filter((cell) => !dropping.has(cell.id));
  if (cells.length === turn.cells.length) return turn;
  const index: Record<string, number> = {};
  cells.forEach((cell, position) => {
    index[cell.id] = position;
  });
  return { ...turn, cells, index };
}

/**
 * 在轮次收口时补一条摘要行。
 *
 * 与 TUI 同口径：只有真的产生过工作（工具、思考或正文）才补，纯问答不补，
 * 否则每一轮都会多出一条没有信息量的分隔线。
 */
export function appendTurnSummary(turn: TurnState, elapsedMs: number | null): TurnState {
  const worked = turn.cells.some(
    (cell) =>
      cell.role === 'tool' ||
      cell.role === 'reasoning' ||
      cell.role === 'fileChange' ||
      cell.role === 'assistant',
  );
  if (!worked) return turn;
  const total = totalTokens(turn.usage);
  return upsertCell(turn, {
    id: `turn-summary:${turn.id}`,
    role: 'summary',
    durationMs: elapsedMs,
    usage: total > 0 ? turn.usage : null,
    firstTokenAvgMs: turn.firstTokenAvgMs,
    generationTpsX10: turn.generationTpsX10,
  });
}

/** 收口助手正文：流式增量缺失时用轮次终态的权威正文补齐。 */
export function closeAssistant(
  turn: TurnState,
  response: string | null,
  recordedAtMs: number,
): TurnState {
  const assistantIds = turn.cells.filter((cell) => cell.role === 'assistant').map((cell) => cell.id);
  let next = turn;
  for (const id of assistantIds) {
    next = patchCell(next, id, (cell) =>
      cell.role === 'assistant' ? { ...cell, streaming: false } : cell,
    );
  }
  if (assistantIds.length > 0 || response === null || response.trim().length === 0) {
    return next;
  }
  return upsertCell(next, {
    id: `turn-response:${turn.id}`,
    role: 'assistant',
    text: response,
    streaming: false,
    // 收口补齐的正文没有更细的时间事实：用收口这一刻的记录时间。
    atMs: recordedAtMs,
  });
}
