/*
 * 单元级归约。
 *
 * 一个 wire 事件落到时间线上的哪一格、以什么形态呈现，全部由这一层决定：
 * 助手增量按 part_id 归并、工具调用与其结果共用一格、文件改动并入发起它的工具行，
 * 计划、用量与提示各有自己的单元。判定只认事件载荷本身，不看到达顺序之外的信号。
 */

import { formatDecimalCount } from '../format';
import type {
  WireAssistantMessage,
  WireContextRollover,
  WireFileChange,
  WirePlanUpdate,
  WireReasoningDelta,
  WireRuntimeFailure,
  WireTokenBudgetReminder,
  WireToolCall,
  WireToolResult,
  WireUsageUpdate,
} from '../wire';
import {
  accumulateFileChange,
  lineCountsOf,
  toolChangeSummary,
  type TurnChanges,
} from './changes';
import { failureReportFromRuntimeFailure } from '../failureReport';
import { LOCAL_STEER_PREFIX } from './state';
import { visibleUserText } from '../references';
import {
  booleanField,
  nested,
  numberField,
  previewCommand,
  stringField,
} from './derive';
import type { ConversationState, TokenUsage, TurnState } from './state';
import { elapsedSince, patchCell, streamOnlyAssistant, upsertCell, withTurn } from './turns';

/** 助手正文：delta 为真时追加，否则整段覆盖。 */
export function applyAssistant(
  state: ConversationState,
  position: number,
  message: WireAssistantMessage,
  recordedAtMs: number,
): ConversationState {
  return withTurn(state, position, (turn) => {
    const known = turn.index[message.partId];
    if (known === undefined) {
      return streamOnlyAssistant(
        upsertCell(turn, {
          id: message.partId,
          role: 'assistant',
          text: message.content,
          streaming: message.delta === true,
          // 时间取这一格第一次被看到的服务端记录时间，与用户气泡同口径。
          atMs: recordedAtMs,
        }),
        message.delta === true ? message.partId : null,
      );
    }
    return streamOnlyAssistant(
      patchCell(turn, message.partId, (cell) =>
        cell.role === 'assistant'
          ? {
              ...cell,
              text: message.delta === true ? cell.text + message.content : message.content,
              streaming: message.delta === true,
            }
          : cell,
      ),
      message.delta === true ? message.partId : null,
    );
  });
}

/** 思考开始：先落一个空单元，增量到达时原位追加。 */
export function applyReasoningStart(turn: TurnState, partId: string, recordedAtMs: number): TurnState {
  // 进入思考阶段说明上一段正文已经写完，光标交给思考行自己的运行态。
  return streamOnlyAssistant(
    upsertCell(turn, {
      id: partId,
      role: 'reasoning',
      text: '',
      done: false,
      durationMs: null,
      startedAtMs: recordedAtMs,
    }),
    null,
  );
}

/** 思考增量。 */
export function applyReasoningDelta(turn: TurnState, delta: WireReasoningDelta): TurnState {
  const known = turn.index[delta.partId];
  if (known === undefined) {
    return upsertCell(turn, {
      id: delta.partId,
      role: 'reasoning',
      text: delta.delta,
      done: false,
      durationMs: null,
      startedAtMs: null,
    });
  }
  return patchCell(turn, delta.partId, (cell) =>
    cell.role === 'reasoning' ? { ...cell, text: cell.text + delta.delta } : cell,
  );
}

/** 思考结束：记下这一段思考的耗时。 */
export function applyReasoningEnd(
  turn: TurnState,
  partId: string,
  recordedAtMs: number,
  nowMs: number,
): TurnState {
  return patchCell(turn, partId, (cell) =>
    cell.role === 'reasoning'
      ? {
          ...cell,
          done: true,
          durationMs:
            cell.startedAtMs !== null
              ? Math.max(0, recordedAtMs - cell.startedAtMs)
              : elapsedSince(turn.startedAtMs, nowMs),
        }
      : cell,
  );
}

/** 用户消息：认领本地或历史索引补出的占位消息，保留附件并绑定事件标识。 */
export function applyUserMessage(
  state: ConversationState,
  position: number,
  partId: string,
  content: string,
  recordedAtMs: number,
): ConversationState {
  return withTurn(state, position, (turn) => {
    if (turn.index[partId] !== undefined) return turn;
    // 正文里可能带着提交时附上的结构化输入块；展示与认领都只认可见正文。
    const visible = visibleUserText(content);
    const optimistic = turn.cells.find(
      (cell) =>
        cell.role === 'user' &&
        (cell.id.startsWith('local-user:') || cell.id.startsWith(LOCAL_STEER_PREFIX) ||
          cell.id.startsWith('history-user:')) &&
        cell.text === visible,
    );
    if (optimistic !== undefined) {
      const cells = turn.cells.map((cell) =>
        cell.id === optimistic.id ? { ...cell, id: partId, atMs: recordedAtMs } : cell,
      );
      const index = { ...turn.index };
      delete index[optimistic.id];
      index[partId] = cells.findIndex((cell) => cell.id === partId);
      return { ...turn, cells, index };
    }
    const attachments = turn.pendingAttachments;
    const withCell = upsertCell(turn, {
      id: partId,
      role: 'user',
      text: visible,
      atMs: recordedAtMs,
      attachments,
    });
    return attachments.length > 0 ? { ...withCell, pendingAttachments: [] } : withCell;
  });
}

/** 工具调用：开一行，结果到达时原位补齐。 */
export function applyToolCall(
  turn: TurnState,
  call: WireToolCall,
  recordedAtMs: number,
): TurnState {
  // 工具开始执行：上一段正文已经落定，不该再挂流式光标。
  return streamOnlyAssistant(
    upsertCell(turn, {
      id: call.partId,
      role: 'tool',
      toolName: call.toolName,
      toolKind: call.toolKind,
      command: previewCommand(call.argumentsSummary),
      arguments: call.argumentsSummary ?? '',
      intent: call.intent ?? '',
      status: 'running',
      summary: call.summary,
      output: '',
      durationMs: null,
      startedAtMs: recordedAtMs,
      exitCode: null,
      jobId: null,
      fullOutputAvailable: false,
      filesystemWritePerformed: false,
      editPath: null,
      added: 0,
      removed: 0,
      diff: null,
    }),
    null,
  );
}

/** 工具结果：补齐状态、耗时、输出与作业归属。 */
export function applyToolResult(
  turn: TurnState,
  result: WireToolResult,
  recordedAtMs: number,
): TurnState {
  const metadata = result.metadata ?? {};
  const jobId = stringField(metadata, 'jobId') ?? stringField(metadata, 'job_id');
  const exitCode = numberField(metadata, 'exitCode');
  const output = result.outputPreview ?? result.summary;
  const fullOutputAvailable = booleanField(nested(metadata, 'output'), 'fullOutputAvailable');
  const known = turn.index[result.partId];
  if (known === undefined) {
    // 流式分块或先到结果后到调用：补一条完整的工具单元，保证过程不丢。
    return upsertCell(turn, {
      id: result.partId,
      role: 'tool',
      toolName: result.toolName ?? result.toolKind ?? '工具',
      toolKind: result.toolKind ?? '',
      command: stringField(metadata, 'argumentsPreview') ?? '',
      arguments: stringField(metadata, 'argumentsPreview') ?? '',
      intent: '',
      status: result.status,
      summary: result.summary,
      output,
      durationMs: result.durationMs ?? null,
      startedAtMs: recordedAtMs,
      exitCode,
      jobId,
      fullOutputAvailable,
      filesystemWritePerformed: result.filesystemWritePerformed === true,
      editPath: null,
      added: 0,
      removed: 0,
      diff: null,
    });
  }
  return patchCell(turn, result.partId, (cell) => {
    if (cell.role !== 'tool') return cell;
    const metadataKind = stringField(metadata, 'kind');
    return {
      ...cell,
      toolKind: cell.toolKind.length > 0 ? cell.toolKind : (metadataKind ?? ''),
      status: result.status,
      summary: result.summary,
      output: output.length > 0 ? output : cell.output,
      durationMs: result.durationMs ?? cell.durationMs,
      exitCode: exitCode ?? cell.exitCode,
      jobId: jobId ?? cell.jobId,
      fullOutputAvailable: fullOutputAvailable || cell.fullOutputAvailable,
      filesystemWritePerformed:
        result.filesystemWritePerformed === true || cell.filesystemWritePerformed,
      toolName: result.toolName ?? cell.toolName,
      command: cell.command.length > 0 ? cell.command : (stringField(metadata, 'argumentsPreview') ?? ''),
      arguments: cell.arguments || stringField(metadata, 'argumentsPreview') || '',
    };
  });
}

/**
 * 文件变更单元。
 *
 * 两份事实同时更新：轮次上的改动集合按 part_id 归并（卡片与改动对比的唯一来源），
 * 工具行上的改动摘要由这份集合派生。派生而非累加，后端的重复宣告不会把行数算两遍。
 *
 * 能经由 `toolPartId` 认领到发起它的工具调用时并入那一行：同一次编辑只该有一条
 * 记录，否则时间线上会出现「工具调用」与「文件变更」两条重复信息。
 */
export function applyFileChange(turn: TurnState, change: WireFileChange): TurnState {
  const changes: TurnChanges = accumulateFileChange(turn.changes, change);
  const owner = change.toolPartId;
  if (owner !== undefined && turn.index[owner] !== undefined) {
    const withFiles = patchCell(turn, owner, (cell) => {
      if (cell.role !== 'tool') return cell;
      const summary = toolChangeSummary(changes, owner);
      if (summary === null) return cell;
      return {
        ...cell,
        editPath: summary.path,
        added: summary.added,
        removed: summary.removed,
        diff: summary.diff.length > 0 ? summary.diff : cell.diff,
      };
    });
    return { ...withFiles, changes };
  }
  const text = change.change.type === 'update' ? change.change.unified_diff : change.change.content;
  const counts = lineCountsOf(change.change.type, text);
  return {
    ...upsertCell(turn, {
      id: change.partId,
      role: 'fileChange',
      path: change.path,
      changeType: change.change.type,
      added: counts.added,
      removed: counts.removed,
      diff: text,
    }),
    changes,
  };
}

/** 计划更新：单元与指标条上的进度同源。 */
export const PLAN_LABEL = '计划';

/**
 * 计划卡副标题：后端默认下发「计划」或空串时不重复渲染，
 * 避免与固定标签拼成「计划 · 计划」；仅在给出独立标题时展示。
 */
export function planHeading(title: string): string | undefined {
  const trimmed = title.trim();
  return trimmed.length === 0 || trimmed === PLAN_LABEL ? undefined : trimmed;
}

export function applyPlan(
  state: ConversationState,
  position: number,
  plan: WirePlanUpdate,
): ConversationState {
  const steps = plan.steps.map((step) => ({ step: step.step, status: step.status }));
  const next = withTurn(state, position, (turn) =>
    upsertCell(turn, {
      id: plan.partId,
      role: 'plan',
      title: plan.title,
      steps,
    }),
  );
  const completed = steps.filter((step) => step.status === 'completed').length;
  // 投影带全量步骤：固定计划坞与指标条读同一份事实，不再各自推算。
  return {
    ...next,
    plan: { title: plan.title, completed, total: steps.length, steps },
  };
}

/*
 * 用量帧按「provider 流」记账。
 *
 * 一条流的用量帧是**该条流的累计快照**，且共享 producer 会在流收口前把同一份快照
 * 再投一次：逐帧累加会让整轮读数翻倍（实测 138/3.05K 对真值 69/1.525K）。
 * 因此流内按覆盖处理、流收口时并入累计，只有跨流才相加。
 */

/** 记一条用量观测：落在在跑流的快照上，本轮读数随之刷新。 */
export function applyUsage(turn: TurnState, usage: WireUsageUpdate): TurnState {
  const stream = streamUsage(turn.usageStream, usage);
  return {
    ...turn,
    usageStream: stream,
    usage: mergeUsage(turn.usageFolded, stream),
  };
}

/**
 * 收口一条 provider 流：把在跑流的快照并入本轮累计。
 *
 * 此刻 `usage` 已经等于「累计 + 该流快照」，因此直接把它当成新的累计，
 * 下一条流的快照就会累加到它之上，而不是把它替换掉。
 */
export function foldStreamUsage(turn: TurnState): TurnState {
  if (turn.usageStream === null) return turn;
  return { ...turn, usageFolded: turn.usage, usageStream: null };
}

/** 流内快照：累加量整组替换，构成与上下文占用沿用「新观测非零才覆盖」。 */
function streamUsage(previous: TokenUsage | null, usage: WireUsageUpdate): TokenUsage {
  return {
    inputTokens: usage.inputTokens,
    cacheReadInputTokens: usage.cachedInputTokens,
    cacheWriteInputTokens: usage.cacheWriteInputTokens ?? 0,
    outputTokens: usage.outputTokens,
    reasoningOutputTokens: usage.reasoningOutputTokens ?? 0,
    currentContextTokens:
      (usage.currentContextTokens ?? 0) > 0
        ? (usage.currentContextTokens ?? 0)
        : (previous?.currentContextTokens ?? 0),
    // 构成是「最近一次真实请求」的快照而不是累加量：只在新观测非零时整组替换。
    systemPromptTokens: compositionValue(usage.systemPromptTokens, previous?.systemPromptTokens ?? 0),
    toolDefinitionTokens: compositionValue(
      usage.toolDefinitionTokens,
      previous?.toolDefinitionTokens ?? 0,
    ),
    conversationTokens: compositionValue(usage.conversationTokens, previous?.conversationTokens ?? 0),
  };
}

/** 累计 + 在跑流快照：累加量相加，快照类读数以在跑流为准并在其缺观测时回落到累计。 */
function mergeUsage(folded: TokenUsage, stream: TokenUsage): TokenUsage {
  return {
    inputTokens: folded.inputTokens + stream.inputTokens,
    cacheReadInputTokens: folded.cacheReadInputTokens + stream.cacheReadInputTokens,
    cacheWriteInputTokens: folded.cacheWriteInputTokens + stream.cacheWriteInputTokens,
    outputTokens: folded.outputTokens + stream.outputTokens,
    reasoningOutputTokens: folded.reasoningOutputTokens + stream.reasoningOutputTokens,
    currentContextTokens: stream.currentContextTokens > 0 ? stream.currentContextTokens : folded.currentContextTokens,
    systemPromptTokens: compositionValue(stream.systemPromptTokens, folded.systemPromptTokens),
    toolDefinitionTokens: compositionValue(stream.toolDefinitionTokens, folded.toolDefinitionTokens),
    conversationTokens: compositionValue(stream.conversationTokens, folded.conversationTokens),
  };
}

/** 构成快照取值：新观测为 0 或缺失时保留上一份。 */
function compositionValue(incoming: number | undefined, previous: number): number {
  return (incoming ?? 0) > 0 ? (incoming ?? 0) : previous;
}

/** 运行时失败：落一张失败卡，正文用后端脱敏后的可读事实。 */
export function applyRuntimeError(
  turn: TurnState,
  eventId: string,
  error: WireRuntimeFailure,
): TurnState {
  return upsertCell(turn, {
    id: `${eventId}:error`,
    role: 'failure',
    report: failureReportFromRuntimeFailure(error),
    text: error.redactedMessage,
  });
}

/** Token 预算预警。 */
export function applyTokenBudgetReminder(
  turn: TurnState,
  eventId: string,
  reminder: WireTokenBudgetReminder,
): TurnState {
  return upsertCell(turn, {
    id: `${eventId}:budget`,
    role: 'notice',
    title: 'Token 预算',
    summary: `剩余 ${formatDecimalCount(reminder.tokensRemaining)} token：${reminder.reminderMessage}`,
    tone: 'info',
  });
}

/** 上下文窗口轮转。 */
export function applyContextRollover(
  turn: TurnState,
  eventId: string,
  rollover: WireContextRollover,
): TurnState {
  return upsertCell(turn, {
    id: `${eventId}:rollover`,
    role: 'notice',
    title: '上下文轮转',
    summary: `窗口 #${rollover.windowNumber}：归档 ${rollover.archivedMessagesCount} 条消息${
      rollover.notesInjected ? '，已注入战术笔记' : ''
    }`,
    tone: 'info',
  });
}
