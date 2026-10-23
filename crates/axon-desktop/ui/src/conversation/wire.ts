/*
 * 会话网关协议类型。
 *
 * 字段名逐字段对齐后端源码，不做本地改名：
 * - 提交与订阅请求帧是 camelCase；
 * - 服务端事件帧的 eventType / payloadJson 外层是 snake_case，载荷内部才是 camelCase。
 *
 * 判别字段只有一处：事件帧的 eventType 决定载荷归属，wire 事件内部再由 kind.type 二次判别。
 */

/**
 * 网关推送给前端的会话流消息。
 *
 * 一条 Tauri 事件通道承载全部订阅的帧，因此每条都带 `subscribeId`：
 * 调用方按它过滤，属于别的目标的帧直接丢弃。
 */
export type ConversationStreamEvent =
  | { kind: 'subscribed'; subscribeId: string }
  | {
      kind: 'frame';
      subscribeId: string;
      eventType: string;
      payloadJson: string;
      cursor: string | null;
    }
  | { kind: 'failure'; subscribeId: string; message: string; retryable: boolean };

/**
 * 一条下行载荷：单帧，或桌面宿主攒批后的一次投递。
 *
 * 历史回放一次可能上万帧，逐帧投递会让宿主跟不上，因此桌面端按 16ms 窗口合并。
 * 批量只合并、不重排，展开后与逐帧投递同序。
 */
export type ConversationStreamPayload =
  | ConversationStreamEvent
  | { batch: ConversationStreamEvent[] };

/** 把一条下行载荷展开成按顺序逐帧的数组；调试桥的单帧载荷原样返回。 */
export function expandStreamPayload(
  payload: ConversationStreamPayload,
): ConversationStreamEvent[] {
  return 'batch' in payload ? payload.batch : [payload];
}

/** 服务端事件帧上出现的 durable 事件类型。 */
export const EVENT_TYPE_WIRE = 'wire_event';
export const EVENT_TYPE_TURN_COMPLETED = 'turn_completed';
export const EVENT_TYPE_TURN_FAILED = 'turn_failed';
export const EVENT_TYPE_INTERACTION_STATUS = 'interaction.status';
export const EVENT_TYPE_RUNTIME_OBSERVATION = 'runtime_observation';

/** 交互处理状态。 */
export type InteractionStatus =
  | 'received'
  | 'processing'
  | 'understood'
  | 'rejected'
  | 'interrupted';

/** 生命周期阶段。 */
export type WireLifecyclePhase = 'regularTurn' | 'compact' | 'review' | 'userShell' | 'governance';

/** 生命周期状态。 */
export type WireLifecycleStatus =
  | 'started'
  | 'running'
  | 'waiting'
  | 'completed'
  | 'failed'
  | 'cancelled'
  | 'aborted'
  | 'blocked';

/** wire 事件信封。 */
export interface WireEventEnvelope {
  schemaVersion: string;
  eventId: string;
  correlationId: string;
  session: { sessionId: string; runId?: string; parentSessionId?: string };
  sequence: number;
  recordedAtMs: number;
  kind: { type: string; payload?: unknown };
}

/** 轮次生命周期事件。 */
export interface WireTurnLifecycle {
  partId: string;
  turnId: string;
  phase: WireLifecyclePhase;
  status: WireLifecycleStatus;
  title: string;
  kind?: string;
  uiPhase?: string;
  toolName?: string;
  attemptIndex?: number;
  summary?: string;
  durationMs?: number;
  startedAtMs?: number;
  totalTokens?: number;
  cacheHitRateBasisPoints?: number;
}

/** Provider 请求实时状态。 */
export interface WireProviderStatus {
  provider: string;
  model: string;
  phase:
    | 'connecting'
    | 'connected'
    | 'generating'
    | 'retrying'
    | 'failed'
    | 'failover';
  transport: 'http' | 'webSocket';
  attemptIndex: number;
  httpStatus?: number;
  errorMessage?: string;
  nextProvider?: string;
  nextModel?: string;
}

/** 助手消息；delta 为真时 content 是片段，否则是整段快照。 */
export interface WireAssistantMessage {
  partId: string;
  messageId: string;
  content: string;
  delta?: boolean;
}

/** 思考增量。 */
export interface WireReasoningDelta {
  partId: string;
  reasoningId: string;
  delta: string;
}

/** 工具调用。 */
export interface WireToolCall {
  partId: string;
  toolCallId: string;
  requestId?: number;
  toolName: string;
  toolKind: string;
  summary: string;
  argumentsSummary?: string;
  /** 模型自述的调用目的概要；只用于折叠态展示。 */
  intent?: string;
  executionPermission?: string;
}

/** 工具结果。 */
export interface WireToolResult {
  partId: string;
  toolCallId: string;
  toolName?: string;
  toolKind?: string;
  status: WireLifecycleStatus;
  summary: string;
  outputPreview?: string;
  stream?: string;
  sequence?: number;
  progressPhase?: string;
  itemsCompleted?: number;
  itemsTotal?: number;
  currentItem?: string;
  durationMs?: number;
  metadata?: Record<string, unknown>;
  errorType?: string;
  filesystemWritePerformed?: boolean;
}

/** 文件变更。 */
export interface WireFileChange {
  partId: string;
  toolPartId?: string;
  toolCallId?: string;
  path: string;
  change:
    | { type: 'add'; content: string }
    | { type: 'delete'; content: string }
    | { type: 'update'; unified_diff: string; move_path?: string };
}

/** 计划更新。 */
export interface WirePlanUpdate {
  partId: string;
  title: string;
  explanation?: string;
  steps: { step: string; status: string; cycle?: number }[];
}

/** 用量更新。 */
export interface WireUsageUpdate {
  inputTokens: number;
  outputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens?: number;
  reasoningOutputTokens?: number;
  currentContextTokens?: number;
  /** 上下文构成估算：注入的系统提示词与上下文 fragment。 */
  systemPromptTokens?: number;
  /** 上下文构成估算：模型可见的工具定义。 */
  toolDefinitionTokens?: number;
  /** 上下文构成估算：真实对话消息（含工具回执）。 */
  conversationTokens?: number;
}

/** 运行时失败事实。 */
export interface WireRuntimeFailure {
  category: string;
  code: string;
  redactedMessage: string;
  httpStatus?: number;
  retryable: boolean;
  timestamp: number;
}

/** Token 预算预警。 */
export interface WireTokenBudgetReminder {
  turnId: number;
  tokensRemaining: number;
  reminderMessage: string;
}

/** 上下文代际窗口轮转。 */
export interface WireContextRollover {
  windowNumber: number;
  windowId: string;
  previousWindowId?: string;
  archivedMessagesCount: number;
  notesInjected: boolean;
}

/** 用户问答事件载荷。 */
export interface WireUserQuestion {
  partId: string;
  callId: string;
  status: import('./questions').QuestionStatus;
  questions: import('./questions').QuestionItem[];
  answers?: import('./questions').AnswerItem[];
  handoff?: string | null;
  deadlineAtMs: number;
}

/** 轮次完成事件载荷。 */
export interface TurnCompletedPayload {
  interactionId?: string;
  response?: string;
}

/** 生成性能快照。 */
export interface GenerationMetrics {
  ttftMs?: number;
  decodeMs?: number;
  generationSpeedTps?: number;
  totalInputTokens: number;
  cacheReadInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  cacheHitRateBasisPoints: number;
  costMicros?: number;
}

/** 回合分阶段耗时。 */
export interface TurnTimingMetrics {
  planningMs: number;
  providerMs: number;
  toolExecutionMs: number;
  totalTurnMs: number;
}

/** LSP 快照。 */
export interface LspMetricsSnapshot {
  activeClients: number;
  pendingRequests: number;
  diagnosticsCount: number;
  lastLatencyMs: number;
}

/** 网关运行指标。 */
export interface GatewayMetrics {
  activeTurnCount: number;
}

/** 运行态观测帧；不含正文，只承载指标。 */
export interface RuntimeObservationFrame {
  generatedAtMs: number;
  sessionId: string;
  turnId: string;
  /** 生成性能快照；后端未采集时为 null，与缺字段同义。 */
  generation?: GenerationMetrics | null;
  /** 回合分阶段耗时；后端未采集时为 null。 */
  timing?: TurnTimingMetrics | null;
  /** LSP 快照；后端未采集时为 null。 */
  lsp?: LspMetricsSnapshot | null;
  /** 网关运行指标；后端未采集时为 null。 */
  gateway?: GatewayMetrics | null;
}

/** 解析事件帧载荷；载荷不是合法 JSON 时返回 null。 */
export function parsePayload<T>(payloadJson: string): T | null {
  try {
    return JSON.parse(payloadJson) as T;
  } catch {
    return null;
  }
}

/** 按 kind.type 收窄 wire 事件载荷。 */
export function wirePayload<T>(kind: { type: string; payload?: unknown }, type: string): T | null {
  return kind.type === type ? ((kind.payload as T) ?? null) : null;
}
