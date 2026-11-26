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
import { attachedFilePaths, visibleUserText } from '../references';
import {
  attachmentsFromPaths,
  booleanField,
  mergeAttachments,
  nested,
  numberField,
  previewCommand,
  stringField,
} from './derive';
import type {
  AttachmentRef,
  ConversationState,
  PlanProjection,
  TokenUsage,
  TranscriptCell,
  TurnState,
} from './state';
import {
  absorbClaimedUserCells,
  activeTurnIndex,
  detachCells,
  elapsedSince,
  indexCells,
  isLocalPlaceholder,
  patchCell,
  streamOnlyAssistant,
  upsertCell,
  withTurn,
} from './turns';

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

/** 用户消息单元，认领与合并只在这一族字段上动手。 */
type UserCell = Extract<TranscriptCell, { role: 'user' }>;

/** 一格待认领的本地占位：它在哪一轮、以及它自己。 */
interface LocalClaim {
  turnPosition: number;
  cell: UserCell;
}

/**
 * 这一格是不是同一条消息的本地占位。
 *
 * 只比可见正文：索引骨架与 durable 帧可能只在首尾空白上不同，比原始串会认领不上。
 */
function isSameMessage(cell: TranscriptCell, visible: string): cell is UserCell {
  return cell.role === 'user' && isLocalPlaceholder(cell) && cell.text.trim() === visible.trim();
}

/** 投影里全部同文本的本地占位，按轮次与单元顺序列出。 */
function localClaims(state: ConversationState, visible: string): LocalClaim[] {
  const claims: LocalClaim[] = [];
  state.turns.forEach((turn, turnPosition) => {
    for (const cell of turn.cells) {
      if (isSameMessage(cell, visible)) claims.push({ turnPosition, cell });
    }
  });
  return claims;
}

/**
 * 认领之后的那一格：把几格本地占位的事实并成一份。
 *
 * 索引骨架带着服务端位置，运行中排队的乐观格带着到达顺序、结构化输入与附件，
 * 两边各缺一半事实；合并后只留一格，字段取较全的那一份。
 *
 * **「未送达」不参与合并**：它是对投递的结论，不是消息本身的属性；而认领这件事本身
 * 就是「服务端收到了」的直接证据。轮次可能先收口（把还在等注入的乐观格标成未送达），
 * 随后这条消息才被注入并回放出来，若把那份旧结论一起搬过来，一条真的送到的消息
 * 就会永远挂着未送达，用户只能重发一遍。
 */
function claimedUserCell(
  owner: UserCell,
  all: readonly UserCell[],
  visible: string,
  partId: string,
  recordedAtMs: number,
  pendingChannel: string | null,
  declared: readonly AttachmentRef[],
): UserCell {
  const withAttachments = all.find((cell) => cell.attachments.length > 0);
  const withStructured = all.find((cell) => cell.structuredInput !== undefined);
  const withChannel = all.find((cell) => cell.channel !== undefined);
  const sequences = all
    .map((cell) => cell.steerSeq)
    .filter((value): value is number => typeof value === 'number');
  return {
    id: partId,
    role: 'user',
    text: visible,
    atMs: recordedAtMs,
    // 本地那一份带真实文件名与体量，正文反解出来的那一份只有路径，因此本地在前。
    attachments: mergeAttachments(
      owner.attachments.length > 0 ? owner.attachments : (withAttachments?.attachments ?? []),
      declared,
    ),
    channel: owner.channel ?? withChannel?.channel ?? pendingChannel ?? undefined,
    structuredInput: owner.structuredInput ?? withStructured?.structuredInput,
    // 排队与到达顺序都跟着这一格走：同一条消息被几次预测时，只有最早那次是用户的动作。
    steer: all.some((cell) => cell.steer === true) ? true : undefined,
    steerSeq: sequences.length > 0 ? Math.min(...sequences) : undefined,
    requeued: all.some((cell) => cell.requeued === true) ? true : undefined,
  };
}

/**
 * 用户消息：把同一条消息的全部本地占位合并成一格，并换成服务端标识。
 *
 * 运行中排队时本地乐观格用 `local-steer:` 立起，索引重载又会为同一交互立起
 * `local-user:` 骨架；durable 用户帧到达时若只换第一格，另一格会原地留下，
 * 正文里就出现两条一样的气泡（一条还挂着「等待注入」）。因此认领的范围是
 * **全部同可见正文的本地格**：一格改成服务端标识，其余一并摘掉。
 *
 * 认领格优先取本轮里「运行中排队」的那一格：它带着到达位置与提交时的结构化输入，
 * 认领之后仍要按 `transcript/order` 的规则留在原位或排到轮末。
 */
export function applyUserMessage(
  state: ConversationState,
  position: number,
  partId: string,
  content: string,
  recordedAtMs: number,
): ConversationState {
  const turn = state.turns[position];
  if (turn === undefined || turn.index[partId] !== undefined) return state;
  // 正文里可能带着提交时附上的结构化输入块；展示与认领都只认可见正文。
  const visible = visibleUserText(content);
  /*
   * 附件只以 `已附加文件：<路径>` 的形式留在正文里，随结构化块一起被剥掉。
   * 而 wire 帧不承载附件、历史索引只记这一轮开场那一份，因此这一份是「这条消息带了
   * 哪些附件」在链路中途唯一的可复用事实，气泡的附件卡必须由它反解回来。
   */
  const declared = attachmentsFromPaths(attachedFilePaths(content));
  const claims = localClaims(state, visible);
  const inTurn = claims.filter((claim) => claim.turnPosition === position);
  if (inTurn.length === 0) {
    // 本轮没有可认领的本地格：这条消息先到，按服务端事实新落一格。
    const withCell = withTurn(state, position, (current) => {
      const filled = upsertCell(current, {
        id: partId,
        role: 'user',
        text: visible,
        atMs: recordedAtMs,
        attachments: mergeAttachments(declared, current.pendingAttachments),
        channel: current.pendingChannel ?? undefined,
      });
      return {
        ...filled,
        pendingAttachments: [],
        pendingChannel: null,
      };
    });
    /*
     * 这一条可能正是「服务端另开一轮」接手的那条运行中追加消息：别的轮次里还留着它的
     * 乐观占位（随后收口会被标成未送达）。按服务端已认领的正文反向收一遍，同一条消息
     * 在正文里只留这一格。
     */
    return absorbClaimedUserCells(withCell, [withCell.turns[position]]);
  }
  const owner = inTurn.find((claim) => claim.cell.steer === true) ?? inTurn[0];
  const others = claims.filter((claim) => claim.cell !== owner.cell);
  const claimed = claimedUserCell(
    owner.cell,
    [owner.cell, ...others.map((claim) => claim.cell)],
    visible,
    partId,
    recordedAtMs,
    turn.pendingChannel,
    declared,
  );
  const dropping = new Map<number, string[]>();
  for (const claim of others) {
    /*
     * 摘掉这一格会让整轮空掉时不摘。
     *
     * 只剩一格占位的轮次一旦被摘空，会以「运行中」永远留在时间线上（状态不再有任何
     * 帧推进它），界面上就是一个不消失的转圈。宁可留一格气泡，也不制造这种空轮。
     */
    if (claim.turnPosition !== position && state.turns[claim.turnPosition].cells.length <= 1) {
      continue;
    }
    const ids = dropping.get(claim.turnPosition) ?? [];
    ids.push(claim.cell.id);
    dropping.set(claim.turnPosition, ids);
  }
  const turns = state.turns.map((current, index) => {
    if (index === position) {
      const drop = dropping.get(index) ?? [];
      const cells = current.cells
        .map((cell) => (cell.id === owner.cell.id ? claimed : cell))
        .filter((cell) => !drop.includes(cell.id));
      // 附件与来源是「还没落地的那条消息」的暂存事实：认领后已贴到这一格上，
      // 再留着会被同一轮后面的消息继承，界面上多出一张重复的附件卡。
      return {
        ...current,
        cells,
        index: indexCells(cells),
        pendingChannel: null,
        pendingAttachments: [],
      };
    }
    const drop = dropping.get(index);
    return drop === undefined ? current : detachCells(current, drop);
  });
  return { ...state, turns };
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
      jobStatus: null,
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
  const jobStatus = stringField(metadata, 'jobStatus');
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
      jobStatus,
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
      jobStatus: jobStatus ?? cell.jobStatus,
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

/**
 * 计划投影：标题、步骤与计数同源。
 *
 * 指标条的「计划 N/M」与输入坞上方的计划坞都只读这一份，因此计数规则只有一处：
 * 状态恰好是 `completed` 的步骤计入已完成，其余（含来源未识别的状态）都算未完成。
 */
export function planProjectionOf(
  title: string,
  steps: { step: string; status: string }[],
): PlanProjection {
  return {
    title,
    completed: steps.filter((step) => step.status === 'completed').length,
    total: steps.length,
    steps,
  };
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
  // 投影带全量步骤：固定计划坞与指标条读同一份事实，不再各自推算。
  return { ...next, plan: planProjectionOf(plan.title, steps) };
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

/**
 * 流内快照：累加量整组替换，上下文占用只认这一帧自己报的读数。
 *
 * 帧里不带 `currentContextTokens` 就是「这一帧没有报占用」，按未知处理（记 0），
 * 绝不沿用上一帧的旧值：那条粘连正是「换窗之后头部还停在 825K」的来源。
 * 构成估算仍在缺项时保留上一份，它是同一请求的分解，与占用不是同一个事实。
 */
function streamUsage(previous: TokenUsage | null, usage: WireUsageUpdate): TokenUsage {
  return {
    inputTokens: usage.inputTokens,
    cacheReadInputTokens: usage.cachedInputTokens,
    cacheWriteInputTokens: usage.cacheWriteInputTokens ?? 0,
    outputTokens: usage.outputTokens,
    reasoningOutputTokens: usage.reasoningOutputTokens ?? 0,
    currentContextTokens: usage.currentContextTokens ?? 0,
    // 构成是「最近一次真实请求」的快照而不是累加量：只在新观测非零时整组替换。
    systemPromptTokens: compositionValue(usage.systemPromptTokens, previous?.systemPromptTokens ?? 0),
    toolDefinitionTokens: compositionValue(
      usage.toolDefinitionTokens,
      previous?.toolDefinitionTokens ?? 0,
    ),
    conversationTokens: compositionValue(usage.conversationTokens, previous?.conversationTokens ?? 0),
  };
}

/** 累计 + 在跑流快照：累加量相加，快照类读数以在跑流为准（它没报就是未知，不回头取值）。 */
function mergeUsage(folded: TokenUsage, stream: TokenUsage): TokenUsage {
  return {
    inputTokens: folded.inputTokens + stream.inputTokens,
    cacheReadInputTokens: folded.cacheReadInputTokens + stream.cacheReadInputTokens,
    cacheWriteInputTokens: folded.cacheWriteInputTokens + stream.cacheWriteInputTokens,
    outputTokens: folded.outputTokens + stream.outputTokens,
    reasoningOutputTokens: folded.reasoningOutputTokens + stream.reasoningOutputTokens,
    currentContextTokens: stream.currentContextTokens,
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

/**
 * 收尾负载里的用量快照落到本轮。
 *
 * 累计已有观测时直接丢弃快照：它与最后一条流同源，而流收口时已经并进累计一次。
 * 只有整轮一条观测都没有（更早的历史帧没有用量帧，或窗口把用量帧截掉）时才用它兜底，
 * 此时先把仍在跑的流并进累计，快照按新一轮流写入，读数不会凭空消失。
 */
export function applyTurnCompletedUsage(
  state: ConversationState,
  usage: WireUsageUpdate | null,
  interactionId: string | null = null,
): ConversationState {
  const position = activeTurnIndex(state);
  if (position < 0) return state;
  /*
   * 收尾负载带着归属时只落到同一轮：上一轮的收尾帧重放得比新一轮的提交还晚时，
   * 不认归属就会把上一轮的用量写进新一轮的累计里。
   */
  const target = (interactionId ?? '').trim();
  if (target.length > 0) {
    const turn = state.turns[position];
    if (turn.id !== target && turn.interactionId !== target) return state;
  }
  const folded = withTurn(state, position, (turn) => foldStreamUsage(turn));
  if (usage === null) return folded;
  const observed = folded.turns[position].usage;
  const observedTokens =
    observed.inputTokens +
    observed.cacheReadInputTokens +
    observed.outputTokens +
    observed.currentContextTokens;
  if (observedTokens > 0) {
    /*
     * 累加量不能再加一遍，但上下文构成要补。
     *
     * 上下文占用与构成是「最近一次请求」的快照，不是累加量；流里的用量帧常常只报
     * token 增量、不带窗口占用，此时若把收尾快照整份丢掉，指标条的上下文段会在历史
     * 会话里整段消失（实测正是这条路径）。补的只是快照类字段，累加量一个都不动。
     */
    return observed.currentContextTokens > 0
      ? folded
      : withTurn(folded, position, (turn) => applyContextComposition(turn, usage));
  }
  return withTurn(folded, position, (turn) => applyUsage(turn, usage));
}

/** 只把快照类读数（窗口占用与构成）写进本轮，不动任何累加量。 */
function applyContextComposition(turn: TurnState, usage: WireUsageUpdate): TurnState {
  const currentContextTokens = usage.currentContextTokens ?? 0;
  if (currentContextTokens <= 0) return turn;
  return {
    ...turn,
    usage: {
      ...turn.usage,
      currentContextTokens,
      systemPromptTokens: compositionValue(usage.systemPromptTokens, turn.usage.systemPromptTokens),
      toolDefinitionTokens: compositionValue(
        usage.toolDefinitionTokens,
        turn.usage.toolDefinitionTokens,
      ),
      conversationTokens: compositionValue(usage.conversationTokens, turn.usage.conversationTokens),
    },
  };
}
