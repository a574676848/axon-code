//! 定时、记忆、用量、定价与语音查询的 IPC 封装。
//!
//! 从 `ipc.ts` 按职责拆出：本文件只做参数整形与命令名映射，不承载界面逻辑。

import type {
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
} from './types';
import type { VoicePresetsPayload } from './speech/presets';
import { call } from './backend';

// ------------------------------------------------------------ 语音

/** 音色预设清单：可用性由网关按模型安装情况给出，前端不改写。 */
export const voicePresets = (): Promise<VoicePresetsPayload> => call('voice_presets');

/** 查询本地语音运行时是否就绪（与建连时的 ready 同一判据）。 */
export const voiceRuntimeStatus = (): Promise<Record<string, unknown>> =>
  call('voice_runtime_status');

/** 查询操作系统的麦克风隐私状态。 */
export const voiceMicrophoneStatus = (): Promise<Record<string, unknown>> =>
  call('voice_microphone_status');

/** 打开操作系统的麦克风隐私设置。 */
export const voiceOpenMicrophoneSettings = (): Promise<void> =>
  call('voice_open_microphone_settings');

/** 通知宿主该轮下行音频已播完或已清空。 */
export const voicePlaybackDrained = (epoch: number): Promise<void> =>
  call('voice_playback_drained', { epoch });

/** 打断当前语音轮次或收线。 */
export const voiceCancel = (reason: 'user_cancel' | 'client_close' = 'user_cancel'): Promise<void> =>
  call('voice_cancel', { reason });

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
