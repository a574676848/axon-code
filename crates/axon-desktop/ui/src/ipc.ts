import type {
  AnswerQuestionReceipt,
  ConfigReadResult,
  ConfigValidateResult,
  ConfigWriteResult,
  DiscoverModelsResult,
  DocumentItem,
  DocumentText,
  FileEntryItem,
  FileTextItem,
  FileBytesItem,
  FileApplicationItem,
  ShellChoiceItem,
  TerminalInfoItem,
  TerminalOutputFrame,
  TerminalExitFrame,
  GatewayHealth,
  InteractionSummary,
  InterruptReceipt,
  JobStopReceipt,
  LaneJobsRoster,
  LaneSubagents,
  SubagentMessageReceipt,
  ServiceAction,
  McpServerInput,
  McpServerItem,
  ModelInput,
  ProviderInput,
  StagedAttachment,
  SubmissionReceipt,
  SystemInfo,
  UpdateCheck,
  SkillItem,
  WorkspaceItem,
  DesktopPreferences,
  CronListResult,
  CronJob,
  CronHistoryResult,
  CronConfigResult,
  MemoryCursor,
  MemoryKind,
  MemoryScopesResult,
  MemoryStatsResult,
  MemoryTimelineResult,
  MemoryWriteInput,
  MemoryWriteReceipt,
  PriceRule,
  PriceRuleInput,
  PricingAutoSyncReport,
  RecalculatePricingResult,
  SavePriceRuleResult,
  SyncPricingResult,
  UsageAuditLogsReport,
  UsageDimension,
  UsageDiagnosticsReport,
  UsageDistributionReport,
  UsageEventFilter,
  UsageOverviewReport,
  UsagePeriod,
  UsageRealtimeReport,
  UsageRealtimeWindow,
  ThemeId,
} from './types';
import type { PersonaPortrait, PersonaSummary } from './personaTypes';

/** 会话事件通道名，与 Rust 侧 `CONVERSATION_EVENT` 一致；车道流与会话流共用这一条。 */
import type { ConversationStreamPayload, ConversationStreamEvent } from './conversation/wire';
import { expandStreamPayload } from './conversation/wire';
import type { AnswerItem, QuestionSnapshot } from './conversation/questions';
import { BRIDGE_PORT, call, isDesktopHost } from './backend';

const CONVERSATION_EVENT = 'axon://conversation';

/** 读取输入历史（最早在前）；与 TUI 共用同一份文件。 */
export const inputHistoryList = (): Promise<string[]> => call('input_history_list');

/** 记录一条输入历史；返回归一化之后的完整历史。 */
export const inputHistoryAppend = (text: string): Promise<string[]> =>
  call('input_history_append', { text });

/*
 * 传输层（Tauri IPC 与浏览器调试桥）与「后端服务未启用」的统一拦截都在
 * `backend.ts`：这里按领域封装具体命令，并把它整体转出去，
 * 因此调用方仍然只从 `ipc` 取后端能力，不必知道传输细节。
 */
export * from './backend';
// ---------------------------------------------------------------- 配置读写

export const readConfig = (): Promise<ConfigReadResult> => call('config_read');

export const validateConfig = (tomlContent: string): Promise<ConfigValidateResult> =>
  call('config_validate', { tomlContent });

export const writeConfig = (tomlContent: string): Promise<ConfigWriteResult> =>
  call('config_write', { tomlContent });

/** 批量应用配置键值，后端校验并备份后直接写入。 */
export const applyConfigValues = (values: Record<string, string>): Promise<ConfigWriteResult> =>
  call('config_apply_values', { values });

/** 把单个配置项恢复为内置默认值。 */
export const resetConfigKey = (key: string): Promise<ConfigWriteResult> =>
  call('config_reset_key', { key });

/** 对一批待应用变更做纯内存试算校验，不落盘。 */
export const validateConfigValues = (
  values: Record<string, string>,
): Promise<ConfigValidateResult> => call('config_validate_values', { values });

/** 在系统默认文本编辑器中打开配置文件。 */
export const openConfigFile = (): Promise<void> => call('open_config_file');

// ------------------------------------------------------- 供应商档案与模型

/** 整体保存一个供应商档案：供应商配置加上它名下的完整模型集合。 */
export const saveProviderProfile = (
  input: ProviderInput,
  models: ModelInput[],
  originalName?: string,
): Promise<ConfigWriteResult> => call('provider_profile_upsert', { input, models, originalName });

/** 移除一个供应商档案，连同它名下的全部模型。 */
export const removeProviderProfile = (name: string): Promise<ConfigWriteResult> =>
  call('provider_profile_remove', { name });

/** 向供应商端点获取可用模型列表；候选不会被自动写入配置。 */
export const discoverModels = (
  provider?: string,
  baseUrl?: string,
  protocol?: string,
  apiKeyEnv?: string,
): Promise<DiscoverModelsResult> => call('discover_models', { provider, baseUrl, protocol, apiKeyEnv });

// --------------------------------------------------------- 工作区与车道

export const fetchWorkspaces = (): Promise<WorkspaceItem[]> => call('workspace_list');

export const addWorkspace = (path: string): Promise<WorkspaceItem[]> =>
  call('workspace_add', { path });

export const removeWorkspace = (id: string): Promise<WorkspaceItem[]> =>
  call('workspace_remove', { id });

/** 打开系统原生目录选择器，返回选中路径；取消或不可用时返回 null。 */
export const pickDirectory = (): Promise<string | null> => call('pick_directory');

export const createLane = (workspaceId: string): Promise<WorkspaceItem[]> =>
  call('lane_create', { workspaceId });

export const setLanePinned = (laneId: string, pinned: boolean): Promise<WorkspaceItem[]> =>
  call('lane_set_pinned', { laneId, pinned });

export const setLaneArchived = (laneId: string, archived: boolean): Promise<WorkspaceItem[]> =>
  call('lane_set_archived', { laneId, archived });

export const renameLane = (laneId: string, title: string): Promise<WorkspaceItem[]> =>
  call('lane_rename', { laneId, title });

/** 保存某个工作区下车道的自定义顺序，用于手动排序。 */
export const laneReorder = (
  workspaceId: string,
  orderedIds: string[],
): Promise<WorkspaceItem[]> => call('lane_reorder', { workspaceId, orderedIds });

// ------------------------------------------------- 文档、目录与 MCP 管理

/** 读取已持久化的人格状态摘要。 */
export const personaSummary = (): Promise<PersonaSummary> => call('persona_summary');

/** 读取形象资产；尚未放置形象时抛出可读错误。 */
export const personaPortrait = (): Promise<PersonaPortrait> => call('persona_portrait');

/** 列出设置内可打开的文件与目录及其存在状态。 */
export const documentCatalog = (): Promise<DocumentItem[]> => call('document_catalog');

/** 用系统默认方式打开指定文件或目录。 */
export const openDocument = (id: string): Promise<void> => call('open_document', { id });

/** 用系统默认浏览器打开一个外部地址；只接受 http 与 https。 */
export const openExternalUrl = (url: string): Promise<void> => call('open_external_url', { url });

/** 列出 MCP 服务器；只含环境变量名，不含值。 */
export const mcpList = (): Promise<McpServerItem[]> => call('mcp_list');

/** 新增或更新一台 MCP 服务器。 */
export const mcpUpsert = (input: McpServerInput, originalName?: string): Promise<void> =>
  call('mcp_upsert', { input, originalName });

/** 以完整 JSON 配置新增或更新一台 MCP 服务器。 */
export const mcpUpsertRaw = (name: string, entryJson: string): Promise<void> =>
  call('mcp_upsert_raw', { name, entryJson });

/** 移除一台 MCP 服务器。 */
export const mcpRemove = (name: string): Promise<void> => call('mcp_remove', { name });

/** 列出已安装技能。 */
export const skillsList = (): Promise<SkillItem[]> => call('skills_list');

/** 读取人设或行为准则文档。 */
export const documentRead = (id: string): Promise<DocumentText> => call('document_read', { id });

/** 写入人设或行为准则文档。 */
export const documentWrite = (id: string, content: string): Promise<void> =>
  call('document_write', { id, content });

// ------------------------------------------------------------------ 密钥

/** 查询某个环境变量是否已经配置密钥。 */
export const secretStatus = (key: string): Promise<boolean> => call('secret_status_of', { key });

/** 写入或更新一个密钥到配置相邻的 .env。 */
export const secretSet = (key: string, value: string): Promise<void> =>
  call('secret_set', { key, value });

/** 从 .env 中清除一个密钥。 */
export const secretClear = (key: string): Promise<void> => call('secret_clear', { key });

// ------------------------------------------------------------ 系统信息

/** 汇总运行态服务、远控状态与版本。 */
export const systemInfo = (): Promise<SystemInfo> => call('system_info');

/** 经 GitHub 发布渠道检测更新。 */
export const checkUpdate = (): Promise<UpdateCheck> => call('check_update');

// ------------------------------------------------------------ 网关可用性

/** 探测网关是否可用；应用启动即调用。 */
export const gatewayHealth = (): Promise<GatewayHealth> => call('gateway_health');

/** 启动、注册或重启 Axon 后台服务。 */
export const gatewayStartService = (mode: 'start' | 'install' | 'restart'): Promise<ServiceAction> =>
  call('gateway_start_service', { mode });

// ------------------------------------------------------------ 会话事件流

/** 提交一条用户消息，返回服务端回执。 */
export const conversationSubmit = (
  workspaceId: string,
  destination: string,
  content: string,
  attachments: string[] = [],
): Promise<SubmissionReceipt> =>
  call('conversation_submit', { workspaceId, destination, content, attachments });

/** 提交待决提问的答案。 */
export const conversationAnswerQuestion = (
  laneId: string,
  callId: string,
  answers: AnswerItem[],
): Promise<AnswerQuestionReceipt> =>
  call('conversation_answer_question', { laneId, callId, answers });

/**
 * 按网关权威记录对账某条车道下的问答状态。
 *
 * 后端已完成字段适配（状态翻译、作答内容与 partId 补齐），这里拿到的就是可直接
 * 覆盖本地投影的形状。`state` 缺省表示不加状态过滤：待决与挂起是两种状态，
 * 只查其一会把另一种误判成已结算。
 */
export const conversationLaneQuestions = (
  laneId: string,
  state: string | null = null,
): Promise<QuestionSnapshot[]> =>
  call('conversation_lane_questions', { laneId, state });

/** 登记当前会话，供系统通知的标题与点击跳转使用。 */
export const conversationSetActiveLane = (laneId: string, title: string): Promise<void> =>
  call('conversation_set_active_lane', { laneId, title });

/**
 * 按车道读取历史 Interaction。
 *
 * 分页口径是「一次一条」：不给游标取最近一条，给出游标取它之前的一条。
 * 会话正文靠逐条订阅回放，因此这里只负责按需把游标往前推。
 */
export const conversationHistory = (
  laneId: string,
  limit = 1,
  before: { receivedAtMs: number; interactionId: string } | null = null,
): Promise<InteractionSummary[]> =>
  call('conversation_history', {
    laneId,
    limit,
    beforeMs: before?.receivedAtMs ?? null,
    beforeId: before?.interactionId ?? null,
  });

/** 历史帧页里的一帧；字段与事件通道上的帧同形。 */
export interface HistoryFrame {
  eventType: string;
  payloadJson: string;
  cursor: string | null;
  replay: boolean;
}

/** 一条 Interaction 的一段历史帧。 */
export interface HistoryPage {
  interactionId: string;
  events: HistoryFrame[];
  nextAfter: number;
  hasMore: boolean;
  tailSequence: number;
  tailCursor: string | null;
}

/**
 * 读取一段历史帧。
 *
 * 正文走这条 HTTP 通道，事件通道只负责读完之后从 `tailCursor` 起的实时尾：
 * 大会话的上万帧因此不再挤在事件通道上。
 */
export const conversationHistoryEvents = (
  interactionId: string,
  after = 0,
  limit?: number,
): Promise<HistoryPage> =>
  call('conversation_history_events', { interactionId, after, limit });

/** 订阅一条 Interaction 的事件流。 */
export const conversationWatch = (
  workspaceId: string,
  interactionId: string,
  resumeCursor: string | null,
): Promise<void> => call('conversation_watch', { workspaceId, interactionId, resumeCursor });

/** 订阅一个**会话**的事件流（子代理会话）；与车道订阅并存于同一条连接上。 */
export const conversationWatchSession = (
  workspaceId: string,
  sessionId: string,
  resumeCursor: string | null,
): Promise<void> => call('conversation_watch_session', { workspaceId, sessionId, resumeCursor });

/**
 * 取消一条订阅。
 *
 * 必须按订阅标识精确取消：一条连接上并存多路订阅，整体关闭会把仍在服务的那一路
 * 一起掐掉（子代理视图与车道视图正是这种并存关系）。
 */
export const conversationUnsubscribe = (subscribeId: string): Promise<void> =>
  call('conversation_unsubscribe', { subscribeId });

/** 中断某条车道正在运行的轮次。 */
export const conversationInterrupt = (laneId: string): Promise<InterruptReceipt> =>
  call('conversation_interrupt', { laneId });

/** 用户手动停止某条车道上的后台作业。 */
export const conversationStopJob = (laneId: string, jobId: string): Promise<JobStopReceipt> =>
  call('conversation_stop_job', { laneId, jobId });

/** 向正在运行的子代理投递一条用户消息。 */
export const conversationSendSubagentMessage = (
  laneId: string,
  agentId: string,
  content: string,
): Promise<SubagentMessageReceipt> =>
  call('conversation_send_subagent_message', { laneId, agentId, content });

/** 读取一条车道上仍在运行的后台作业名册。 */
export const conversationLaneJobs = (laneId: string): Promise<LaneJobsRoster> =>
  call('conversation_lane_jobs', { laneId });

/** 读取一条车道下的子代理会话清单。 */
export const conversationLaneSubagents = (laneId: string): Promise<LaneSubagents> =>
  call('conversation_lane_subagents', { laneId });

/**
 * 监听会话事件流。
 *
 * 桌面宿主走 Tauri 事件通道，浏览器调试走调试桥的 SSE；两条通道承载同一种帧结构，
 * 且**车道流与会话流共用这一条通道**（帧内自带 `subscribeId`，调用方按它分发），
 * 因此这里只有一个入口、一个 SSE 端点。
 */
export async function listenConversation(
  handler: (event: ConversationStreamEvent) => void,
): Promise<() => void> {
  return listenStream(CONVERSATION_EVENT, handler);
}

async function listenStream(
  eventName: string,
  handler: (event: ConversationStreamEvent) => void,
): Promise<() => void> {
  if (isDesktopHost()) {
    const { listen } = await import('@tauri-apps/api/event');
    // 桌面宿主把回放帧攒成一批下发：一次载荷里有几帧就按序喂几帧，
    // 上层仍然只面对「逐帧」这一种形状。
    const unlisten = await listen<ConversationStreamPayload>(eventName, (event) => {
      for (const item of expandStreamPayload(event.payload)) handler(item);
    });
    return unlisten;
  }
  // 浏览器调试走调试桥的同一条 SSE。
  const source = new EventSource(`http://127.0.0.1:${BRIDGE_PORT}/events`);
  source.onmessage = (message) => {
    try {
      handler(JSON.parse(message.data) as ConversationStreamEvent);
    } catch {
      // 单帧解析失败不终止整条流：后续帧仍然有效。
    }
  };
  return () => source.close();
}

// ------------------------------------------------------------ 附件

/** 落地一个粘贴进来的附件，返回可直接提交的绝对路径。 */
export const attachmentStage = (name: string, dataBase64: string): Promise<StagedAttachment> =>
  call('attachment_stage', { name, dataBase64 });

/** 打开系统文件选择器并直接登记为附件。 */
export const attachmentPick = (): Promise<StagedAttachment[]> => call('attachment_pick');

/** 取一张附件图片的可渲染地址：桌面窗口走 IPC 数据地址，调试桥走字节路由。 */
export async function attachmentSource(path: string): Promise<string> {
  return isDesktopHost()
    ? call('attachment_preview', { path })
    : `http://127.0.0.1:${BRIDGE_PORT}/attachment/${encodeURIComponent(path)}`;
}

// ------------------------------------------------------------ 剪贴板

/*
 * 剪贴板走原生通道，不用网页层的 navigator.clipboard：
 * 后者对读取要弹一次系统授权、对写入要求窗口处于焦点，都会让右键菜单的复制粘贴时灵时不灵。
 */

/** 读取剪贴板文本；剪贴板里没有文本时返回空串。 */
export const clipboardReadText = (): Promise<string> => call('clipboard_read_text');

/** 写入剪贴板文本。 */
export const clipboardWriteText = (text: string): Promise<void> =>
  call('clipboard_write_text', { text });

// ------------------------------------------------------------ 正文导出

/**
 * 把一段正文另存为 Markdown 文件。
 *
 * 另存为对话框由后端调起，界面因此不需要文件系统权限；用户取消时返回 null。
 */
export const exportMarkdown = (suggestedName: string, content: string): Promise<string | null> =>
  call('export_markdown', { suggestedName, content });

// ------------------------------------------------------------ 工作区文件

/** 列出工作区内某个目录的条目。 */
export const listWorkspaceEntries = (
  root: string,
  relativePath: string,
): Promise<FileEntryItem[]> =>
  call('workspace_list_entries', { root, relativePath });

/** 读取工作区内的文本文件。 */
export const readWorkspaceFile = (
  root: string,
  relativePath: string,
): Promise<FileTextItem> =>
  call('workspace_read_file', { root, relativePath });

/**
 * 探测一个路径是文件还是目录。
 *
 * 目录不该按文本读（只会得到「目标不是文件」）：侧栏据此把目录按目录树展开，
 * 与工作区文件页同一套浏览方式。
 */
export const statWorkspacePath = (
  root: string,
  relativePath: string,
): Promise<{ is_dir: boolean; absolute_path: string; display_path: string }> =>
  call('workspace_stat_path', { root, relativePath });

/**
 * 读取工作区内一个文件的原始字节。
 *
 * 图片、PDF、HTML 必须拿到未解码的字节：按文本读会把非 UTF-8 的部分换成占位符，
 * 渲染出来的图或文档就是坏的。
 */
export const readWorkspaceBytes = (root: string, relativePath: string): Promise<FileBytesItem> =>
  call('workspace_read_bytes', { root, relativePath });

/**
 * 列出系统为某个文件登记的打开方式。
 *
 * 候选由操作系统回答（Windows 走 Shell 的关联处理程序枚举，Linux 走 XDG 桌面条目），
 * 因此换一台机器、装一个新应用，菜单会自己跟着变：这里没有一份精选目录。
 */
export const fileApplications = (
  root: string,
  relativePath: string,
): Promise<FileApplicationItem[]> =>
  call('workspace_applications', { root, relativePath });

/** 用系统默认程序打开工作区内的文件。 */
export const openWorkspaceFile = (root: string, relativePath: string): Promise<void> =>
  call('workspace_open_default', { root, relativePath });

/** 用系统登记的某个应用打开工作区内的文件；标识来自 `fileApplications`。 */
export const openWorkspaceFileWith = (
  root: string,
  relativePath: string,
  application: string,
): Promise<void> => call('workspace_open_with', { root, relativePath, application });

/** 在文件管理器中定位工作区内的文件。 */
export const revealWorkspaceFile = (root: string, relativePath: string): Promise<void> =>
  call('workspace_reveal', { root, relativePath });

// ------------------------------------------------------------ 终端

/** 终端输出事件通道名，与 Rust 侧 `TERMINAL_OUTPUT_EVENT` 一致。 */
const TERMINAL_OUTPUT_EVENT = 'axon://terminal-output';

/** 终端退出事件通道名，与 Rust 侧 `TERMINAL_EXIT_EVENT` 一致。 */
const TERMINAL_EXIT_EVENT = 'axon://terminal-exit';

/**
 * 终端是否可用。
 *
 * 终端靠事件通道把输出送回界面，而浏览器调试桥没有这条通道，
 * 因此只有桌面宿主内才是完整能力；调试桥下界面给出明确说明而不是开一个看不见输出的终端。
 */
export const terminalSupported = (): boolean => isDesktopHost();

/** 列出本机可用的 shell。 */
export const terminalShells = (): Promise<ShellChoiceItem[]> => call('terminal_shells');

/** 在工作区里开一个终端。 */
export const terminalOpen = (
  root: string,
  shell: string | null,
  cols: number,
  rows: number,
): Promise<TerminalInfoItem> => call('terminal_open', { root, shell, cols, rows });

/** 把键盘输入写进终端。 */
export const terminalWrite = (id: string, data: string): Promise<void> =>
  call('terminal_write', { id, data });

/** 调整终端尺寸。 */
export const terminalResize = (id: string, cols: number, rows: number): Promise<void> =>
  call('terminal_resize', { id, cols, rows });

/** 关闭终端。 */
export const terminalClose = (id: string): Promise<void> => call('terminal_close', { id });

/** 订阅终端输出；调试桥没有事件通道，返回空订阅。 */
export async function listenTerminalOutput(
  handler: (frame: TerminalOutputFrame) => void,
): Promise<() => void> {
  if (!isDesktopHost()) return () => undefined;
  const { listen } = await import('@tauri-apps/api/event');
  return listen<TerminalOutputFrame>(TERMINAL_OUTPUT_EVENT, (event) => handler(event.payload));
}

/** 订阅终端退出；调试桥没有事件通道，返回空订阅。 */
export async function listenTerminalExit(
  handler: (frame: TerminalExitFrame) => void,
): Promise<() => void> {
  if (!isDesktopHost()) return () => undefined;
  const { listen } = await import('@tauri-apps/api/event');
  return listen<TerminalExitFrame>(TERMINAL_EXIT_EVENT, (event) => handler(event.payload));
}

// ------------------------------------------------------------ 桌面偏好

/** 读取桌面偏好。 */
export const desktopPreferences = (): Promise<DesktopPreferences> => call('desktop_preferences');

/** 设置是否发送系统通知。 */
export const desktopSetNotifications = (enabled: boolean): Promise<DesktopPreferences> =>
  call('desktop_set_notifications', { enabled });

/** 切换界面主题。 */
export const desktopSetTheme = (theme: ThemeId): Promise<DesktopPreferences> =>
  call('desktop_set_theme', { theme });

const CRON_PAGE_SIZE = 20;
export const cronList = (page = 1, pageSize = CRON_PAGE_SIZE, status = '', search = ''): Promise<CronListResult> =>
  call('cron_list', { page, pageSize, status, search });
export const cronConfig = (): Promise<CronConfigResult> => call('cron_config');
export const cronGet = (jobId: string): Promise<CronJob> => call('cron_get', { jobId });
export const cronHistory = (jobId: string, page = 1, pageSize = CRON_PAGE_SIZE): Promise<CronHistoryResult> =>
  call('cron_history', { jobId, page, pageSize });
export const cronCreate = (input: Record<string, unknown>): Promise<CronJob> =>
  call('cron_create', { input });
export const cronUpdate = (jobId: string, input: Record<string, unknown>): Promise<CronJob> =>
  call('cron_update', { jobId, input });
export const cronPause = (jobId: string): Promise<CronJob> => call('cron_pause', { jobId });
export const cronResume = (jobId: string): Promise<CronJob> => call('cron_resume', { jobId });
export const cronRun = (jobId: string): Promise<Record<string, unknown>> => call('cron_run', { jobId });
export const cronRemove = (jobId: string): Promise<CronJob> => call('cron_remove', { jobId });

const MEMORY_PAGE_SIZE = 50;
export const memoryList = (
  memoryKind: MemoryKind,
  scope: string | undefined,
  limit = MEMORY_PAGE_SIZE,
  cursor?: MemoryCursor,
): Promise<MemoryTimelineResult> =>
  call('memory_list', {
    memoryKind,
    scope,
    limit,
    cursorUpdatedAtMs: cursor?.updatedAtMs,
    cursorMemoryId: cursor?.memoryId,
  });
export const memoryStats = (scope?: string): Promise<MemoryStatsResult> =>
  call('memory_stats', { scope });
export const memoryScopes = (): Promise<MemoryScopesResult> => call('memory_scopes', {});
export const memoryGet = (memoryId: string): Promise<{ record: MemoryTimelineResult['records'][number] }> =>
  call('memory_get', { memoryId });
export const memoryWrite = (input: MemoryWriteInput): Promise<MemoryWriteReceipt> =>
  call('memory_write', { input });
export const memoryWriteBatch = (records: MemoryWriteInput[]): Promise<MemoryWriteReceipt> =>
  call('memory_write', { input: { records } });
export const memoryForget = (memoryId: string): Promise<MemoryWriteReceipt> =>
  call('memory_forget', { memoryId });
export const memoryRemove = (memoryId: string): Promise<MemoryWriteReceipt> =>
  call('memory_remove', { memoryId });

// ------------------------------------------------------------ 用量统计

/*
 * 用量统计全部经网关 `/api/v1/observability/*` 读取，桌面端不直连统计库。
 * 过滤条件缺省即整项省略，让网关使用自己的默认窗口，两边不各存一份默认值。
 */

/** 大盘概览：汇总、迷你走势、活动热力图与健康时间线。 */
export const usageOverview = (
  period: UsagePeriod,
  filter: { provider?: string; model?: string; source?: string } = {},
): Promise<UsageOverviewReport> =>
  call('usage_overview', {
    period,
    provider: filter.provider,
    model: filter.model,
    source: filter.source,
  });

/** 用量分布与排行：按模型或供应商维度。 */
export const usageDistribution = (
  dimension: UsageDimension,
  period: UsagePeriod,
): Promise<UsageDistributionReport> => call('usage_distribution', { dimension, period });

/** 延迟与效率诊断；网关只接受 today 与 7d 两个窗口。 */
export const usageDiagnostics = (period: 'today' | '7d'): Promise<UsageDiagnosticsReport> =>
  call('usage_diagnostics', { period });

/** 实时滑窗读数。 */
export const usageRealtime = (window: UsageRealtimeWindow): Promise<UsageRealtimeReport> =>
  call('usage_realtime', { window });

/** 请求事件日志；游标分页，过滤条件为空时不过滤。 */
export const usageEvents = (
  filter: UsageEventFilter = {},
  cursor?: string | null,
  limit = 20,
): Promise<UsageAuditLogsReport> =>
  call('usage_events', {
    model: filter.model,
    source: filter.source,
    status: filter.status,
    cursor: cursor ?? undefined,
    limit,
  });

// ------------------------------------------------------------ 模型定价

/*
 * 价格规则落在运行态的用量库里，同样只经网关读写。
 * 「编辑」在界面上是先建后删两步：网关的保存接口只接受新建语义（规则 ID 由服务端
 * 生成），先建成新规则再删旧规则，中途不会出现「这个模型没有价格」的窗口。
 */

/** 列出全部价格规则，按服务端的四级全序返回。 */
export const pricingRules = (): Promise<PriceRule[]> => call('pricing_rules');

/** 新建一条价格规则。 */
export const pricingSaveRule = (payload: PriceRuleInput): Promise<SavePriceRuleResult> =>
  call('pricing_save_rule', { payload });

/** 按 ID 删除一条价格规则。 */
export const pricingDeleteRule = (ruleId: string): Promise<{ id: string; status: string }> =>
  call('pricing_delete_rule', { ruleId });

/** 从 models.dev 同步费率；服务端有手动同步防抖。 */
export const pricingSync = (): Promise<SyncPricingResult> => call('pricing_sync');

/** 按当前规则重算已落库的历史成本。 */
export const pricingRecalculate = (): Promise<RecalculatePricingResult> => call('pricing_recalculate');

/** 读取每日自动同步任务的状态。 */
export const pricingAutoSync = (): Promise<PricingAutoSyncReport> => call('pricing_auto_sync');

/** 打开或关闭每日自动同步；打开时由后端创建或启用系统任务。 */
export const pricingSetAutoSync = (enabled: boolean): Promise<PricingAutoSyncReport> =>
  call('pricing_set_auto_sync', { enabled });
