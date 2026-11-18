import type { SessionUsageTotals } from './conversation/sessionUsage';

export type ConfigValueKind = 'bool' | 'unsigned' | 'string' | 'string_list';

/** 单个配置键的权威规格。 */
export interface ConfigKeySpec {
  key: string;
  kind: ConfigValueKind;
  editable: boolean;
  description: string;
  setup_module: string;
  default_value: string;
  options: string[];
  depends_on: string[];
}

/** 供应商条目。 */
export interface ProviderItem {
  name: string;
  enabled: boolean;
  protocol: string;
  base_url: string;
  api_key_env?: string;
  api_key_configured: boolean;
  wire_transport: string;
  priority: number;
  weight: number;
}

/** 模型条目。 */
export interface ModelItem {
  name: string;
  id: string;
  provider: string;
  context_window: number;
  max_output_tokens?: number;
  supports_vision: boolean;
  /** 是否支持向量检索。 */
  supports_embedding: boolean;
  /** 向量维度；仅 supports_embedding 为 true 时有效。 */
  embedding_dimension: number;
  /** 是否要求历史助手消息逐条回传推理内容。 */
  requires_reasoning_content_on_assistant_messages: boolean;
}

/** 车道（等同通用工作台的会话）。 */
export interface SessionItem {
  id: string;
  title: string;
  /** 会话表面渠道，例如 tui、cli 或本应用的 desktop。 */
  channel: string;
  /** 会话表面目标。 */
  destination: string;
  /** 受众边界标签。 */
  audience: string;
  /** 该车道下的 Interaction 总数。 */
  interaction_count: number;
  created_at: number;
  updated_at: number;
  pinned: boolean;
  archived: boolean;
  /** 是否为本应用新建、尚未产生 Interaction 的车道。 */
  pending: boolean;
  /** 该车道当前是否有 Interaction 正在处理中。 */
  running: boolean;
  /** 待答提问数量。 */
  pending_question_count?: number;
}

/** 工作区。 */
export interface WorkspaceItem {
  id: string;
  name: string;
  path: string;
  active: boolean;
  removable: boolean;
  sessions: SessionItem[];
}

/** 配置读取结果。 */
export interface ConfigReadResult {
  flat_values: Record<string, unknown>;
  toml_content: string;
  env_vars: string[];
  specs: ConfigKeySpec[];
  config_path: string;
  providers: ProviderItem[];
  models: ModelItem[];
}

/** 配置校验结果。 */
export interface ConfigValidateResult {
  valid: boolean;
  errors: string[];
  warnings: string[];
  diff_summary: string[];
}

/** 配置写入结果。 */
export interface ConfigWriteResult {
  written: boolean;
  message: string;
  backup_created: boolean;
}

/** 供应商写入入参（不含任何明文密钥字段）。 */
export interface ProviderInput {
  name: string;
  enabled: boolean;
  protocol: string;
  base_url: string;
  api_key_env?: string;
  wire_transport: string;
  priority: number;
  weight: number;
}

/** 模型写入入参。 */
export interface ModelInput {
  name: string;
  id: string;
  provider: string;
  context_window: number;
  max_output_tokens?: number;
  supports_vision: boolean;
  /** 是否支持向量检索。 */
  supports_embedding: boolean;
  embedding_dimension: number;
  /** 是否要求历史助手消息逐条回传推理内容。 */
  requires_reasoning_content_on_assistant_messages: boolean;
}

/** 从上游探测到的候选模型。 */
export interface DiscoveredModel {
  id: string;
  name?: string;
  context_window?: number;
  max_output_tokens?: number;
}

/** 模型列表探测结果。 */
export interface DiscoverModelsResult {
  endpoint: string;
  models: DiscoveredModel[];
}

/** 顶层导航目标。 */
export type NavTab = 'workspace' | 'chat' | 'automation' | 'goals' | 'memory';

export type MemoryKind = 'profile' | 'preference' | 'experience' | 'knowledge' | 'procedure';

/** 记忆时间轴的归档状态筛选。 */
export type MemoryArchiveStatus = 'active' | 'archived' | 'all';

export interface MemoryRecord {
  /** 记忆 ID 是 u64，超出 JS 安全整数区间，统一以十进制字符串传输。 */
  memoryId: string;
  memoryKind: MemoryKind;
  state: 'weak' | 'strong' | 'quarantined' | 'tombstone';
  memorySummary?: string | null;
  contentRef?: string | null;
  createdAtMs: number;
  updatedAtMs: number;
  lastActiveAtMs?: number | null;
  archivedAtMs?: number | null;
  confidenceBasisPoints?: number;
  importanceBasisPoints?: number;
  decayedScoreBasisPoints?: number;
  halfLifeDays?: number;
  scope: 'personal' | 'workspace';
}

export interface MemoryCursor {
  updatedAtMs: number;
  memoryId: string;
}

export interface MemoryTimelineResult {
  status: string;
  memoryKind: MemoryKind;
  /** 本次读取使用的归档状态筛选。 */
  archiveStatus?: MemoryArchiveStatus;
  scope: string;
  namespaceRef?: string | null;
  limit: number;
  records: MemoryRecord[];
  nextCursor?: MemoryCursor | null;
  needsRepair: boolean;
}

export interface MemoryKindCount {
  memoryKind: MemoryKind;
  count: number;
}

export interface MemoryStatsResult {
  status: string;
  /** 本次统计使用的归档状态筛选；页签计数与列表同口径。 */
  archiveStatus?: MemoryArchiveStatus;
  scope: string;
  namespaceRef?: string | null;
  totalRecords: number;
  activeRecords: number;
  archivedRecords: number;
  memoryKindCounts: MemoryKindCount[];
  needsRepair: boolean;
  legacyRecordTables: string[];
}

export interface MemoryScopeOption {
  scope: string;
  scopeKind: 'personal' | 'workspace';
  namespaceRef: string;
  label: string;
  recordCount: number;
}

export interface MemoryScopesResult {
  status: string;
  scopes: MemoryScopeOption[];
  needsRepair: boolean;
}

export interface MemoryWriteInput {
  memoryKind: MemoryKind;
  summary: string;
  evidence: string;
  workspacePath?: string;
  supersedesMemoryIds?: string[];
}

export interface MemoryWriteReceipt {
  status: string;
  records: Array<{
    memoryKind: MemoryKind;
    memoryId?: string | null;
    sqliteWritten: boolean;
    textIndexWritten: boolean;
    vectorProjectionWritten: boolean;
    needsRepair: boolean;
  }>;
  needsRepair: boolean;
}

export interface CronJob {
  schemaVersion: number;
  jobId: string;
  name: string;
  schedule: string;
  prompt: string;
  skills: string[];
  /**
   * 完成后的唤醒目标，形如 `session:<channel>:<destination>[:<workspace>]`。
   * `null` 表示执行完只记录运行事实，不产生任何投递。
   */
  wake: string | null;
  workdir?: string | null;
  noAgent: boolean;
  script?: string | null;
  status: 'active' | 'paused' | 'removed' | 'completed';
  revision: number;
  nextRunAtMs?: number | null;
  lastRunAtMs?: number | null;
  createdAtMs: number;
  updatedAtMs: number;
  system?: boolean;
  sessionId?: string;
  workspaceId?: string | null;
  workspacePath?: string;
  destination?: string;
  running?: boolean;
  model?: string | null;
  reasoningEffort?: string | null;
  canDelete?: boolean;
  lastRunSuccess?: boolean | null;
  lastRunError?: string | null;
}

export interface CronRun {
  schemaVersion: number;
  runId: string;
  jobId: string;
  jobRevision: number;
  startedAtMs: number;
  finishedAtMs: number;
  success: boolean;
  agentExecuted: boolean;
  deliveryEnqueued: boolean;
  stdoutEmpty: boolean;
  dispatchId?: string | null;
  errorSummary?: string | null;
}

export interface CronListResult {
  jobs: CronJob[];
  total: number;
  page: number;
  pageSize: number;
  hasMore: boolean;
}

export interface CronHistoryResult {
  jobId: string;
  runs: CronRun[];
  total: number;
  page: number;
  pageSize: number;
  hasMore: boolean;
}

export interface CronConfigResult {
  axonRoot: string;
  configPath: string;
  storePath: string;
  systemJobIds: string[];
}

export interface CronSessionTarget {
  workspaceId: string;
  sessionId: string;
  title: string;
}

/** 设置弹窗的二级分区。 */
export type SettingsSection =
  | 'connection'
  | 'general'
  | 'models'
  | 'pricing'
  | 'balancer'
  | 'remote'
  | 'memory'
  | 'mcp'
  | 'skills'
  | 'soul'
  | 'agents'
  | 'usage'
  | 'system'
  | 'about';

/** 设置内可打开的文件或目录。 */
export interface DocumentItem {
  id: string;
  label: string;
  path: string;
  /** file 或 dir。 */
  kind: string;
  exists: boolean;
}

/** 一台 MCP 服务器；只含环境变量名，不含值。 */
export interface McpServerItem {
  name: string;
  /** stdio 或 http。 */
  kind: string;
  command: string;
  args: string[];
  url: string;
  env_keys: string[];
}

/** MCP 服务器写入入参；env 中留空的键保留原值。 */
export interface McpServerInput {
  name: string;
  kind: string;
  command: string;
  args: string[];
  url: string;
  env: Record<string, string>;
}

/** MCP 运行态载荷；定义在 mcpTypes，这里只做类型出口。 */
export type { McpRuntimeServer, McpRuntimeStatus } from './mcpTypes';

/** 一项已安装技能。 */
export interface SkillItem {
  id: string;
  name: string;
  description: string;
  path: string;
  /** 技能描述文件路径；不存在时为空。 */
  entry_path: string;
}

/** 文本类文档读取结果。 */
export interface DocumentText {
  /** 文档内容；文件不存在时为空串。 */
  content: string;
  /** 文档绝对路径。 */
  path: string;
  /** 文件是否已存在。 */
  exists: boolean;
}

/** 界面主题标识。纸白（paper）是默认，黑曜石（obsidian）为可选暗色主题。 */
export type ThemeId = 'paper' | 'obsidian';

/** 桌面偏好。 */
export interface DesktopPreferences {
  /** 是否发送系统通知。 */
  notificationsEnabled: boolean;
  /** 界面主题标识。 */
  theme: ThemeId;
  /** 左侧导航是否折叠；空值表示用户还没有手动表态。 */
  sidebarCollapsed: boolean | null;
  /** 输入坞最近一次显式选择的模型名称；空值表示从未选过。 */
  composerModel: string | null;
  /** 输入坞最近一次显式选择的思考强度；空值表示从未选过。 */
  composerReasoningEffort: string | null;
  /** 上次打开的工作区目录；空值表示还没有打开过。 */
  lastWorkspaceDir?: string | null;
  /** 上次打开的会话标识；空值表示还没有打开过。 */
  lastSessionId?: string | null;
  /** 侧栏展开着的工作区标识；启动时据此恢复展开态。 */
  expandedWorkspaceIds?: string[];
}

/** 问答结算回执。 */
export interface AnswerQuestionReceipt {
  sessionId: string;
  callId: string;
  questionId: string;
  state: string;
  delivered: boolean;
  deliveryMode?: string | null;
}

/** GitHub 发布渠道：关于页展示的全部真实坐标。 */
export interface GithubInfo {
  /** `owner/name` 形态的仓库标识。 */
  repo: string;
  repository_url: string;
  releases_url: string;
  issues_url: string;
  license: string;
}

/** daemon 后台健康快照：租约与排队读数。 */
export interface DaemonHealth {
  /** 后台 host 代号。 */
  host_id: string;
  /** 租约剩余毫秒；0 表示租约已失效、后台 Worker 已停机。 */
  lease_remaining_ms: number;
  /** 已接收但尚未被领取的会话数。 */
  interactions_pending: number;
  /** 已被领取、正在处理的会话数。 */
  interactions_processing: number;
  /** 待投递的回复数。 */
  deliveries_pending: number;
}

/** 系统信息汇总。 */
export interface SystemInfo {
  app_version: string;
  /** daemon 是否正在运行。 */
  daemon_running: boolean;
  /** daemon 状态的中文说明。 */
  daemon_detail: string;
  /** daemon 状态结论：running、workers_stopped、degraded 或 stopped。 */
  daemon_state: string;
  /** daemon 后台健康快照；读不到时为空。 */
  daemon_health?: DaemonHealth | null;
  /** 后台健康快照读不到的原因；成功时为空串。 */
  daemon_health_error: string;
  github: GithubInfo;
}

/** 更新检查结果。 */
export interface UpdateCheck {
  current: string;
  latest: string;
  update_available: boolean;
  release_url: string;
  download_url: string;
  published_at: string;
  error: string;
  /** 失败时的下一步动作；成功时为空。 */
  hint: string;
}

// 网关可用性与服务操作的契约类型独立成文件，这里回导出以保持既有导入路径不变。
export type { GatewayHealth, ServiceAction } from './typesGateway';

/** 已注册模型列表。 */
export interface ConfigReadResult {
  flat_values: Record<string, unknown>;
  toml_content: string;
  env_vars: string[];
  specs: ConfigKeySpec[];
  config_path: string;
  providers: ProviderItem[];
  models: ModelItem[];
  /** 供应商协议候选值。 */
  provider_protocols: string[];
  /** 供应商链路候选值。 */
  provider_wire_transports: string[];
}

/** 提交回执；Interaction 标识由服务端生成，客户端无法预知。 */
export interface SubmissionReceipt {
  interactionId: string;
  accepted: boolean;
  /** 是否未新增记录：幂等命中，或输入被注入到同车道正在运行的回合。 */
  deduplicated: boolean;
}

/** 车道内一条历史 Interaction。 */
export interface InteractionSummary {
  interactionId: string;
  status: string;
  channel: string;
  receivedAtMs: number;
  /** 入站正文。 */
  content: string;
  /** 入站时落地的本地附件绝对路径。 */
  attachments: string[];
  /** 该轮显式指定的模型；入口没有选择时缺省。 */
  model?: string | null;
  /** 该轮显式指定的推理强度；入口没有选择时缺省。 */
  reasoningEffort?: string | null;
  /** 该轮用过的工具名（逗号分隔、已去重）；只有尚无指标事件的老记录才有。 */
  toolSummary?: string;
  /** 本轮最终交付正文；老网关、非正常收口或正文已过保留期时缺省。 */
  answerText?: string | null;
  /**
   * 本轮用过的工具名，按出现顺序、已去重。
   *
   * 索引轮的过程抬头据此立起与实时同一句动作摘要，不必先展开这一轮。
   */
  toolNames?: string[];
  /**
   * 这一轮的读数（用时、token、首字、速率）。
   *
   * 事实源是该轮自己的 `turn_metrics` 事件，与整条会话的累计投影同源：索引页因此
   * 不必为了显示一行指标去回放整轮正文。读不到任何事实时缺省。
   */
  metrics?: TurnMetricsReading | null;
  /** 已收口轮次的墙钟用时（毫秒）；只在没有指标事件的老记录上出现。 */
  elapsedMs?: number | null;
}

/** 一轮的读数：字段与 `turn_metrics` 事件一一对应，未测得的一项缺省。 */
export interface TurnMetricsReading {
  /** 本轮墙钟用时（毫秒）。 */
  activeMs: number;
  /** 首字延迟（毫秒）；未测得时缺省。 */
  ttftMs?: number | null;
  /** 生成速率（0.1 tok/s 的整数刻度）；未测得时缺省。 */
  generationSpeedTpsX10?: number | null;
  /** 工具调用次数。 */
  toolCalls: number;
  /** 未缓存输入。 */
  inputTokens: number;
  /** 缓存读取。 */
  cacheReadTokens: number;
  /** 缓存写入。 */
  cacheWriteTokens: number;
  /** 输出。 */
  outputTokens: number;
  /** 推理输出。 */
  reasoningTokens: number;
}

/** 会话级后台作业的一行事实。 */
export interface SessionJobFact {
  /** 服务端作业标识。 */
  jobId: string;
  /** 作业类型（shell / subagent 等）；只有终态事件时为空串。 */
  kind?: string;
  /** 启动时刻（Unix 毫秒）。 */
  startedAtMs?: number | null;
  /** 终态名称；只有启动事件时缺省，界面按「在跑」呈现。 */
  status?: string | null;
  /** 从启动到终态的耗时；未测得时缺省。 */
  elapsedMs?: number | null;
  /** 退出码（若可得）。 */
  exitCode?: number | null;
}

/**
 * 一页车道历史：这一页的 Interaction，加上整条车道的累计读数。
 *
 * 两个累计读数与翻到第几页无关，由网关按车道聚合；读不到结论时各自为 null。
 * 用量的形状只定义一份（`conversation/sessionUsage`），这里只引用。
 */
export interface SessionInteractions {
  interactions: InteractionSummary[];
  /**
   * 会话级后台作业清单。
   *
   * 作业是整条会话的事实，不属于某一轮：重开会话时清单由这里还原，不看这一轮跑过什么。
   * 事实源是该会话事件流里的 `job_started` / `job_completed`。
   */
  jobs?: SessionJobFact[];
  sessionUsage: SessionUsageTotals | null;
  sessionElapsedMs: number | null;
  effectiveModel?: import('./conversation/viewConfig').SessionEffectiveModel | null;
  /** 最近一次记忆反思的提示文本；该会话还没有记忆动作时为 null。 */
  reflectionNotice?: string | null;
}

/** 中断回执。 */
export interface InterruptReceipt {
  sessionId: string;
  /** 服务端的真实结论：没有活跃轮次时为 false。 */
  interrupted: boolean;
  interactionId?: string | null;
}

/** 用户手动停止后台作业的回执。 */
export interface JobStopReceipt {
  sessionId: string;
  jobId: string;
  /** 服务端的真实结论；归属不符等拒绝走错误分支。 */
  stopped: boolean;
  /** 终态状态标签。 */
  status?: string | null;
  /** 终态原因，例如「用户手动停止」。 */
  reason?: string | null;
}

/** 向子代理投递用户消息的回执。 */
export interface SubagentMessageReceipt {
  sessionId: string;
  agentId: string;
  /** 是否已进入目标 mailbox。 */
  delivered: boolean;
  /** 目标当前状态标签。 */
  recipientStatus?: string | null;
}

/** 一条车道上仍在运行的后台作业名册。 */
export interface SessionJobsRoster {
  sessionId: string;
  /** 后端进程代际；重启即变化。 */
  epoch: string;
  /** 仍在运行的作业标识。 */
  jobIds: string[];
}

/** 车道下的一条子代理会话。 */
export interface SessionSubagentSummary {
  sessionId: string;
  /** 会话标识里属于代理路径的那段，用于与 agent 路径对齐。 */
  slug: string;
  title?: string | null;
  model?: string;
  provider?: string;
  updatedAtMs?: number;
  /** 线程真实状态：`active` 才是此刻仍在运行。 */
  status?: string;
  turnCount?: number;
  /** 末轮状态：结束/中断/失败由它决定。 */
  lastTurnStatus?: string | null;
  lastTurnAtMs?: number | null;
  /** 是否有运行态会话行；false 表示这次执行早于「子代理会话化」。 */
  hasRuntime?: boolean;
}

/** 一条车道的子代理会话清单。 */
export interface SessionSubagents {
  sessionId: string;
  runSessionId?: string | null;
  subagents: SessionSubagentSummary[];
}

/** 已落地的附件。 */
export interface StagedAttachment {
  /** 绝对路径，用于提交与预览。 */
  path: string;
  /** 展示用文件名。 */
  name: string;
  /** 是否为可预览的图片。 */
  image: boolean;
  /** 字节数，用于展示体量。 */
  bytes: number;
}

/*
 * 用量统计的传输模型单独成篇（`usageTypes.ts`）：领域模型按块拆分，
 * 单文件才守得住行数硬限。这里整体转出去，调用方仍旧只从 `types` 取模型。
 */
export type * from './usageTypes';

/*
 * 工作区文件与终端的传输模型单独成篇（`typesFiles.ts`）：模型按域拆分，
 * 单文件才守得住行数硬限。这里整体转出去，调用方仍旧只从 `types` 取模型。
 */
export type {
  FileEntryItem,
  FileTextItem,
  FileBytesItem,
  FileApplicationItem,
  ShellChoiceItem,
  TerminalInfoItem,
  TerminalOutputFrame,
  TerminalExitFrame,
} from './typesFiles';
