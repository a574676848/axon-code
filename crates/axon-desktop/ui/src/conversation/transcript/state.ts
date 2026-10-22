/*
 * 对话投影的状态模型。
 *
 * 这一层只声明事实的形状与常量，不含任何归约逻辑：单元的判别联合、轮次的
 * 终态字段、会话级状态与归约动作。事实源是订阅流，因此这里没有任何可写副本。
 */

import type { FailureReport } from '../failureReport';
import type { RuntimeObservationFrame } from '../wire';
import type { TurnChanges } from './changes';
import type { AnswerItem, PendingQuestion, QuestionItem, QuestionStatus } from '../questions';

/** 时间线上的单元。 */
export type TranscriptCell =
  | {
      id: string;
      role: 'user';
      text: string;
      atMs: number | null;
      attachments: AttachmentRef[];
      /** 服务端始终没有认领这条消息（发送失败，或中断时还没被注入）。 */
      undelivered?: boolean;
    }
  | {
      id: string;
      role: 'assistant';
      text: string;
      streaming: boolean;
      /** 这一格第一次被看到的服务端记录时间；操作条与用户气泡同款时间。 */
      atMs: number | null;
    }
  | { id: string; role: 'reasoning'; text: string; done: boolean; durationMs: number | null; startedAtMs: number | null }
  /**
   * 折叠单元：一整条 Interaction 压成的一行。
   *
   * 它是折叠态下该轮**唯一**保留的单元，因此这里必须自带展开所需的一切：
   * 用户那一句、动作摘要与用来重新物化的标识。
   */
  | {
      id: string;
      role: 'folded';
      interactionId: string;
      prompt: string;
      summary: string;
      atMs: number | null;
    }
  | {
      id: string;
      role: 'tool';
      toolName: string;
      toolKind: string;
      command: string;
      /** 服务端参数摘要，展开时保留 JSON 输入结构。 */
      arguments: string;
      /** 模型自述的调用目的概要；折叠态优先显示它，为空时显示命令投影。 */
      intent: string;
      status: string;
      summary: string;
      output: string;
      durationMs: number | null;
      /** 这一格第一次被看到的服务端记录时间；运行中的耗时由它起算。 */
      startedAtMs: number | null;
      exitCode: number | null;
      jobId: string | null;
      fullOutputAvailable: boolean;
      filesystemWritePerformed: boolean;
      /** 该工具改动的文件路径；无写入时为 null。 */
      editPath: string | null;
      /** 改动新增与删除的行数。 */
      added: number;
      removed: number;
      /** unified diff 文本；仅在有文件变更时存在，用于展开查看。 */
      diff: string | null;
    }
  | {
      id: string;
      role: 'fileChange';
      path: string;
      changeType: string;
      added: number;
      removed: number;
      diff: string;
    }
  | { id: string; role: 'history'; title: string; status: string; summary: string }
  | { id: string; role: 'plan'; title: string; steps: { step: string; status: string }[] }
  | { id: string; role: 'notice'; title: string; summary: string; tone: 'info' | 'danger' }
  | { id: string; role: 'failure'; report: FailureReport | null; text: string }
  | { id: string; role: 'summary'; durationMs: number | null; usage: TokenUsage | null; firstTokenAvgMs: number | null; generationTpsX10: number | null }
  | {
      id: string;
      role: 'question';
      callId: string;
      status: QuestionStatus;
      questions: QuestionItem[];
      answers?: AnswerItem[];
      handoff?: string | null;
      deadlineAtMs: number;
      /** 提问被记录下来的时刻；与消息页脚同款时间标记，缺值时整行不渲染。 */
      atMs?: number | null;
    };

/** 用户消息携带的附件。 */
export interface AttachmentRef {
  /** 本地绝对路径。 */
  path: string;
  /** 展示用文件名。 */
  name: string;
  /** 是否为可预览的图片。 */
  image: boolean;
  /** 字节数；历史消息只带路径时为空。 */
  bytes: number | null;
}

/**
 * 运行中追加的用户消息标识前缀。
 *
 * durable 的用户消息事件到达前，这一条只是本地乐观气泡；事件按正文认领它并换成
 * 服务端标识，标记随之消失。因此「是否仍在排队等安全边界插入」由这个前缀判定，
 * 不额外维护第二份队列状态。
 */
export const LOCAL_STEER_PREFIX = 'local-steer:';

/** 子代理活动行；字段口径与 TUI 的子代理活动区一致。 */
export interface SubagentRow {
  agentId: string;
  label: string;
  status: 'starting' | 'running' | 'waiting' | 'completed' | 'interrupted' | 'failed';
  activity: string;
  elapsedMs: number;
  lastSeenAtMs: number;
  order: number;
  usage: number | null;
  cacheHitRateBasisPoints: number | null;
  model: string | null;
  summary: string | null;
  /** 该子代理的生命周期轨迹，用于在主区域回看它这一轮到底做了什么。 */
  events: SubagentEvent[];
}

/** 子代理轨迹上的一条事实。 */
export interface SubagentEvent {
  /** 事件记录时间。 */
  atMs: number;
  /** 运行阶段，取值来自后端 uiPhase。 */
  phase: string;
  /** 阶段状态。 */
  status: string;
  /** 展示标题。 */
  title: string;
  /** 阶段摘要；无则为 null。 */
  summary: string | null;
}

/** 单个子代理保留的轨迹条数上限，避免长任务把内存拖垮。 */
export const MAX_SUBAGENT_EVENTS = 200;

/** 后台作业行；事实源是订阅流上的 shell 与作业工具事件。 */
export interface JobRow {
  id: string;
  command: string;
  status: string;
  output: string;
  durationMs: number | null;
  /** 这一行第一次被看到的服务端记录时间；运行中的耗时由它按秒起算。 */
  startedAtMs: number | null;
  exitCode: number | null;
  jobId: string | null;
  fullOutputAvailable: boolean;
}

/**
 * 令牌累计口径，与 TUI 的 AgentTuiTokenUsage 同字段。
 *
 * 末尾三项是**上下文构成估算**：它们是「最近一次请求」的快照，不参与 `totalTokens`
 * 累加（构成是总量的分解，不是额外消耗），口径与 Provider 报出的上下文总量天然有差。
 */
export interface TokenUsage {
  inputTokens: number;
  cacheReadInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  currentContextTokens: number;
  systemPromptTokens: number;
  toolDefinitionTokens: number;
  conversationTokens: number;
}

/** 空用量。 */
export const EMPTY_USAGE: TokenUsage = {
  inputTokens: 0,
  cacheReadInputTokens: 0,
  cacheWriteInputTokens: 0,
  outputTokens: 0,
  reasoningOutputTokens: 0,
  currentContextTokens: 0,
  systemPromptTokens: 0,
  toolDefinitionTokens: 0,
  conversationTokens: 0,
};

/** 上下文构成估算总量；没有构成观测时为 0。 */
export function compositionTokens(usage: TokenUsage): number {
  return usage.systemPromptTokens + usage.toolDefinitionTokens + usage.conversationTokens;
}

/** 总 token。 */
export function totalTokens(usage: TokenUsage): number {
  return (
    usage.inputTokens +
    usage.cacheReadInputTokens +
    usage.cacheWriteInputTokens +
    usage.outputTokens +
    usage.reasoningOutputTokens
  );
}

/** 当前上下文 token；provider 未观测到时回落到归一化总量。 */
export function currentContextTokens(usage: TokenUsage): number {
  return usage.currentContextTokens > 0 ? usage.currentContextTokens : totalTokens(usage);
}

/** 完整输入 token。 */
export function completeInputTokens(usage: TokenUsage): number {
  return usage.inputTokens + usage.cacheReadInputTokens + usage.cacheWriteInputTokens;
}

/** 缓存读取占完整输入的万分比。 */
export function cacheHitRateBasisPoints(usage: TokenUsage): number {
  const denominator = completeInputTokens(usage);
  if (denominator === 0) return 0;
  return Math.min(10_000, Math.floor((usage.cacheReadInputTokens * 10_000) / denominator));
}

/** 一个轮次的投影。 */
export interface TurnState {
  /** 投影内部标识：绑定交互前是本地临时标识，绑定后即 Interaction 标识。 */
  id: string;
  /** 网关 Interaction 标识；提交回执到达前为空。 */
  interactionId: string | null;
  cells: TranscriptCell[];
  /** part_id 到单元下标的索引，增量归并只查这里。 */
  index: Record<string, number>;
  status: 'pending' | 'streaming' | 'completed' | 'failed' | 'rejected' | 'interrupted' | 'undelivered';
  /**
   * 本轮用量读数：已收口流的累计加上在跑流的快照，供界面直接读。
   *
   * 读取方只看这一个字段；它由 `usageFolded` 与 `usageStream` 派生，不单独维护。
   */
  usage: TokenUsage;
  /** 本轮已收口 provider 流的用量累计。 */
  usageFolded: TokenUsage;
  /**
   * 在跑 provider 流最近一次用量快照。
   *
   * 用量帧是该条流的累计快照而不是增量，同一条流的快照还会被重复投递，
   * 因此流内按覆盖处理、收口时并入累计，跨流才是相加。
   */
  usageStream: TokenUsage | null;
  startedAtMs: number | null;
  elapsedMs: number | null;
  firstTokenAvgMs: number | null;
  generationTpsX10: number | null;
  /** 本轮解码窗口：各条 provider 流的实测流式耗时之和减去首字；会话级速率按各轮窗口加权求和。 */
  decodeMs: number | null;
  /** 本轮 Provider 实测的输出 token 累计；未观测到用量时为 null。 */
  generationOutputTokens: number | null;
  /** 本轮 Provider 实测的推理输出 token 累计；未观测到用量时为 null。 */
  generationReasoningTokens: number | null;
  /** 生成中的实时状态行，来自 provider 状态事件。 */
  activity: string | null;
  /** 本轮第一条 durable 事件的记录时间；回放时用来还原真实耗时。 */
  firstEventAtMs: number | null;
  /** 本轮最后一条 durable 事件的记录时间。 */
  lastEventAtMs: number | null;
  /** 本轮的文件改动事实，按 part_id 归并；卡片与改动对比都读这一份。 */
  changes: TurnChanges;
  /**
   * 等待贴到用户消息上的历史附件。
   *
   * 回放时附件先于会话事件到达（历史索引是附件的唯一事实源），而用户消息要等
   * durable 事件重放才出现，因此这里先暂存，等第一条用户消息落地时再贴上。
   */
  pendingAttachments: AttachmentRef[];
  /**
   * 已折叠：正文单元已经**丢弃**，只剩一行摘要。
   *
   * 折叠不是「藏起来」而是「释放」：`cells` 被替换成单个折叠单元后，工具输出与推理
   * 正文在投影里不再有任何引用，内存立即回收。展开时按标识重新回放该 Interaction，
   * 把算好的单元整段换回来。
   */
  folded: boolean;
  /** 折叠时保留的那一行摘要（工具动作串）。 */
  summary: string;
  /** 用户显式展开过：不再自动折叠，避免刚点开就被收回去。 */
  keepExpanded: boolean;
}

/** 订阅连接状态。 */
export type ConnectionState = 'idle' | 'connecting' | 'subscribed' | 'failed';

/** 对话投影的完整状态。 */
export interface ConversationState {
  turns: TurnState[];
  connection: ConnectionState;
  failure: { message: string; retryable: boolean } | null;
  /** 断点续订游标。 */
  cursor: string | null;
  /** 最近一帧运行指标。 */
  observation: RuntimeObservationFrame | null;
  lastObservationAtMs: number;
  subagents: SubagentRow[];
  /**
   * 会话级计划投影。
   *
   * 指标条的完成度与输入坞上方的固定计划坞读的是同一份投影；时间线里的计划单元
   * 只是那一刻的快照，因此「当前计划」必须在这里留全量步骤，而不是只留计数。
   */
  plan: PlanProjection | null;
  lspActiveCount: number;
  activeTurnCount: number;
  /** 当前车道待决/已结算提问映射（按 callId 索引），O(1) 增量维护。 */
  pendingQuestions: Record<string, PendingQuestion>;
  /**
   * 已结算提问的权威结论（按 callId 索引）。
   *
   * 待答位只留还需要回答的那几条，因此结算过的条目会从 `pendingQuestions` 里离场；
   * 但它的结论必须留住：进入车道时对账与历史回放是两条异步路径，对账先到（此时
   * 单元还没被回放出来），随后到达的挂起帧就会在时间线上重新长出一张「已挂起」的
   * 单元，并给出注定被服务端拒绝的回入口。这里只留状态与作答内容，不留问题正文，
   * 因此它的体量随「答过的题」增长，而不是随问答载荷增长。
   */
  resolvedQuestions: Record<string, ResolvedQuestion>;
}

/** 一条已结算提问的权威结论。 */
export interface ResolvedQuestion {
  /** 已作答内容；对账未下发时留空。 */
  answers?: AnswerItem[];
}

/** 会话级计划投影。 */
export interface PlanProjection {
  /** 计划标题；后端未给出时为空串。 */
  title: string;
  /** 已完成步数。 */
  completed: number;
  /** 总步数。 */
  total: number;
  /** 步骤明细，顺序与后端下发一致。 */
  steps: { step: string; status: string }[];
}

/** 初始状态。 */
export const INITIAL_CONVERSATION: ConversationState = {
  turns: [],
  connection: 'idle',
  failure: null,
  cursor: null,
  observation: null,
  lastObservationAtMs: 0,
  subagents: [],
  plan: null,
  lspActiveCount: 0,
  activeTurnCount: 0,
  pendingQuestions: {},
  resolvedQuestions: {},
};

/** 归约动作。 */
export type ConversationAction =
  | { type: 'reset' }
  | { type: 'open-turn'; turnId: string; nowMs: number }
  | { type: 'submit'; turnId: string; text: string; nowMs: number; attachments: AttachmentRef[] }
  | { type: 'steer'; text: string; nowMs: number; attachments: AttachmentRef[] }
  /**
   * 提交失败：撤销这一轮的乐观状态。
   *
   * `turnId` 是本地那一轮的标识（运行中排队时为空，因为排队不新开轮次）；
   * `text` 用来定位排队时追加的那一格。
   */
  | { type: 'submit-failed'; turnId: string | null; text: string; message: string; nowMs: number }
  | { type: 'bind'; turnId: string; interactionId: string }
  | { type: 'attach-history'; interactionId: string; attachments: AttachmentRef[] }
  | { type: 'seed-history'; interactionId: string; text: string; attachments: AttachmentRef[] }
  /**
   * 把更早的一页轮次整段接到时间线头部。
   *
   * 「加载更早」逐条向前翻页时，更早那一页会先在独立投影里回放完，再作为已算好的
   * 轮次数组前置进来；这样主投影不必为了插入头部而重放整条会话。
   */
  | { type: 'prepend-turns'; turns: TurnState[] }
  /**
   * 折叠一轮：丢弃正文单元，只留一行摘要。
   *
   * 摘要在派发前算好：折叠之后就没有可依据的单元了。
   */
  | { type: 'fold-turn'; turnId: string; summary: string }
  /** 展开一轮：把重新回放算出来的单元整段换回来。 */
  | { type: 'replace-turn-cells'; turnId: string; cells: TranscriptCell[] }
  | { type: 'connecting' }
  | { type: 'frames'; frames: import('../wire').ConversationStreamEvent[]; nowMs: number }
  /**
   * 用服务端权威记录对账本地待答状态。
   *
   * `authoritative` 为真表示这一批就是服务端当前的全部问答记录：本地还在等回答、
   * 而这里没有出现的条目已经不再等用户了，按已结算收敛。
   */
  | { type: 'reconcile-questions'; questions: PendingQuestion[]; authoritative?: boolean }
  | { type: 'tick'; nowMs: number };
