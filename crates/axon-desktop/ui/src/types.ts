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
}

/** 车道（等同通用工作台的会话）。 */
export interface LaneItem {
  id: string;
  title: string;
  /** 会话表面渠道，例如 tui、cli、feishu 或本应用的 desktop。 */
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
  lanes: LaneItem[];
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
export type NavTab = 'workspace' | 'chat' | 'automation' | 'memory';

export type MemoryKind = 'profile' | 'preference' | 'experience' | 'knowledge' | 'procedure';

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

export interface CronDelivery {
  adapter: string;
  sessionId: number;
  recipientId: string;
  threadId?: string | null;
}

export interface CronJob {
  schemaVersion: number;
  jobId: string;
  name: string;
  schedule: string;
  prompt: string;
  skills: string[];
  delivery: CronDelivery;
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

/** 界面主题标识。纸白是默认，森林是第二套。 */
export type ThemeId = 'paper' | 'forest';

/** 桌面偏好。 */
export interface DesktopPreferences {
  /** 是否发送系统通知。 */
  notificationsEnabled: boolean;
  /** 界面主题标识。 */
  theme: ThemeId;
}

export interface FileTextItem {
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** 工作区内的绝对路径；查看页的路径栏显示这一份。 */
  absolute_path: string;
  content: string;
  truncated: boolean;
  binary: boolean;
}

/** 问答结算回执。 */
export interface AnswerQuestionReceipt {
  laneId: string;
  callId: string;
  questionId: string;
  state: string;
  delivered: boolean;
  deliveryMode?: string | null;
}

/** 工作区内一个文件的原始字节。 */
export interface FileBytesItem {
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** 工作区内的绝对路径。 */
  absolute_path: string;
  /** 由扩展名判定的 MIME 类型。 */
  mime: string;
  /** 原始字节的 base64。 */
  data_base64: string;
  /** 是否因超出上限被截断。 */
  truncated: boolean;
}

/** 系统为某个文件登记的一条打开方式。 */
export interface FileApplicationItem {
  /** 平台原生标识：Windows 是关联处理程序的 ProgID／AppID，Linux 是桌面条目 id。 */
  id: string;
  /** 展示名；系统没有给出名字时是标识的文件名部分。 */
  name: string;
  /** 图标：`data:image/png;base64,...`；取不到时为 null。 */
  icon: string | null;
  /** 是否为系统认定的默认打开方式。 */
  default: boolean;
}

/** 本机可用的一个 shell。 */
export interface ShellChoiceItem {
  /** 启动用的程序：绝对路径，或 PATH 上的命令名。 */
  path: string;
  /** 展示名。 */
  name: string;
}

/** 已建立的终端会话。 */
export interface TerminalInfoItem {
  id: string;
  shell: string;
  cwd: string;
  cols: number;
  rows: number;
}

/** 一帧终端输出；`data` 是原始字节的 base64。 */
export interface TerminalOutputFrame {
  id: string;
  data: string;
}

/** 一次终端退出。 */
export interface TerminalExitFrame {
  id: string;
  /** 退出码；拿不到时为 null。 */
  code: number | null;
  /** 是否由用户主动关闭。 */
  closed: boolean;
}

/** 网关监听地址探测结果。 */
export interface ServiceProbe {
  name: string;
  target: string;
  listening: boolean;
  detail: string;
}

/** 远控 adapter 状态。 */
export interface RemoteStatus {
  connected: boolean;
  state_label: string;
  detail: string;
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
  services: ServiceProbe[];
  remote_feishu?: RemoteStatus;
  github: GithubInfo;
  runtime_root: string;
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

/** 网关可用性。 */
export interface GatewayHealth {
  connected: boolean;
  listening: boolean;
  /** 网关是否有 HTTP 响应（即便状态码不是 2xx）。 */
  responding: boolean;
  target: string;
  reason: string;
  /** 修复动作标识，由后端按故障类型给出。 */
  actions: string[];
}

/** 服务操作结果。 */
export interface ServiceAction {
  success: boolean;
  message: string;
  output: string;
}

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

/** 提交回执；Interaction 标识由服务端派生，客户端无法预知。 */
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
}

/** 中断回执。 */
export interface InterruptReceipt {
  laneId: string;
  /** 服务端的真实结论：没有活跃轮次时为 false。 */
  interrupted: boolean;
  interactionId?: string | null;
}

/** 用户手动停止后台作业的回执。 */
export interface JobStopReceipt {
  laneId: string;
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
  laneId: string;
  agentId: string;
  /** 是否已进入目标 mailbox。 */
  delivered: boolean;
  /** 目标当前状态标签。 */
  recipientStatus?: string | null;
}

/** 一条车道上仍在运行的后台作业名册。 */
export interface LaneJobsRoster {
  laneId: string;
  /** 后端进程代际；重启即变化。 */
  epoch: string;
  /** 仍在运行的作业标识。 */
  jobIds: string[];
}

/** 车道下的一条子代理会话。 */
export interface LaneSubagentSummary {
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
export interface LaneSubagents {
  laneId: string;
  runSessionId?: string | null;
  subagents: LaneSubagentSummary[];
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

/** 工作区内的一个文件或目录。 */
export interface FileEntryItem {
  name: string;
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** dir 或 file。 */
  kind: string;
}

/** 工作区内一个文本文件的内容。 */
export interface FileTextItem {
  /** 相对工作区根目录的路径，统一正斜杠。 */
  path: string;
  /** 工作区内的绝对路径；查看页的路径栏显示这一份。 */
  absolute_path: string;
  content: string;
  truncated: boolean;
  binary: boolean;
}

/*
 * 用量统计的传输模型单独成篇（`usageTypes.ts`）：领域模型按块拆分，
 * 单文件才守得住行数硬限。这里整体转出去，调用方仍旧只从 `types` 取模型。
 */
export type * from './usageTypes';
