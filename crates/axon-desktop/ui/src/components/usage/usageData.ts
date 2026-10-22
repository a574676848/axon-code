import { useCallback, useEffect, useRef, useState } from 'react';
import * as ipc from '../../ipc';
import type {
  UsageAuditLogsReport,
  UsageDiagnosticsReport,
  UsageDimension,
  UsageDistributionReport,
  UsageEventFilter,
  UsageOverviewReport,
  UsagePeriod,
  UsageRealtimeReport,
  UsageRealtimeWindow,
} from '../../types';

/*
 * 用量看板的取数层。
 *
 * 四个面板各有自己的窗口（周期、维度、实时滑窗），因此四个请求并发发出、
 * 各自成败：某一面暂时取不到时，其余三面照常上屏，只在顶部给出一行失败原因，
 * 不用一次失败把整页清空。
 *
 * 刷新期间保留上一份数据（旧读数配「正在刷新」提示），避免整个看板在每次
 * 轮询时闪成骨架：读数的跳动比晚一帧更难读。
 */

/** 实时滑窗的自刷新间隔：滑窗本身以分钟为单位滚动。 */
const REALTIME_REFRESH_MS = 30_000;

export interface UsageDataState {
  overview: UsageOverviewReport | null;
  distribution: UsageDistributionReport | null;
  /** 模型维度的一份分布：事件日志的模型候选取自它，与当前排行维度无关。 */
  models: string[];
  diagnostics: UsageDiagnosticsReport | null;
  realtime: UsageRealtimeReport | null;
  /** 首次加载（还没有任何数据可显示）。 */
  loading: boolean;
  /** 已有数据、正在取新的一份。 */
  refreshing: boolean;
  /** 第一处失败原因；多面失败时按面板顺序取第一条。 */
  error: string | null;
  /** 最近一次成功取数的时刻。 */
  refreshedAt: number | null;
  refresh: () => void;
}

/** 诊断接口的窗口上限是 7 天，比它长的周期一律落到 7 天。 */
export function diagnosticsPeriod(period: UsagePeriod): 'today' | '7d' {
  return period === 'today' ? 'today' : '7d';
}

function messageOf(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure);
}

/**
 * 并发取四面板数据。
 *
 * `live` 为 true 时实时面按 30 秒自刷新：其余三面读的是落库的聚合桶，
 * 由 Daemon 每分钟投影一次，跟着 30 秒轮询只会重复读到同一份数据。
 */
export function useUsageData(
  period: UsagePeriod,
  dimension: UsageDimension,
  realtimeWindow: UsageRealtimeWindow,
  live: boolean,
): UsageDataState {
  const [overview, setOverview] = useState<UsageOverviewReport | null>(null);
  const [distribution, setDistribution] = useState<UsageDistributionReport | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [diagnostics, setDiagnostics] = useState<UsageDiagnosticsReport | null>(null);
  const [realtime, setRealtime] = useState<UsageRealtimeReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshedAt, setRefreshedAt] = useState<number | null>(null);
  /** 请求代际：迟到的响应不许覆盖新一轮的结果。 */
  const generation = useRef(0);

  const load = useCallback(async () => {
    const current = generation.current + 1;
    generation.current = current;
    setRefreshing(true);
    // 维度就是模型时复用同一个请求：候选清单与排行读的是同一份数据。
    const distributionRequest = ipc.usageDistribution(dimension, period);
    const modelRequest = dimension === 'model' ? distributionRequest : ipc.usageDistribution('model', period);
    const [overviewResult, distributionResult, modelResult, diagnosticsResult, realtimeResult] =
      await Promise.allSettled([
        ipc.usageOverview(period),
        distributionRequest,
        modelRequest,
        ipc.usageDiagnostics(diagnosticsPeriod(period)),
        ipc.usageRealtime(realtimeWindow),
      ]);
    if (generation.current !== current) return;

    const failures: string[] = [];
    if (overviewResult.status === 'fulfilled') setOverview(overviewResult.value);
    else failures.push(`概览：${messageOf(overviewResult.reason)}`);
    if (distributionResult.status === 'fulfilled') setDistribution(distributionResult.value);
    else failures.push(`分布：${messageOf(distributionResult.reason)}`);
    if (modelResult.status === 'fulfilled') {
      setModels(modelResult.value.items.map((item) => item.key));
    }
    if (diagnosticsResult.status === 'fulfilled') setDiagnostics(diagnosticsResult.value);
    else failures.push(`诊断：${messageOf(diagnosticsResult.reason)}`);
    if (realtimeResult.status === 'fulfilled') setRealtime(realtimeResult.value);
    else failures.push(`实时：${messageOf(realtimeResult.reason)}`);

    setError(failures.length === 0 ? null : failures.join('；'));
    setRefreshedAt(Date.now());
    setRefreshing(false);
    setLoading(false);
  }, [period, dimension, realtimeWindow]);

  useEffect(() => {
    void load();
  }, [load]);

  // 实时面的自刷新只覆盖实时读数，不重取聚合面。
  useEffect(() => {
    if (!live) return undefined;
    const timer = window.setInterval(() => {
      void ipc
        .usageRealtime(realtimeWindow)
        .then((report) => setRealtime(report))
        .catch(() => {
          // 自刷新失败保持上一份读数：这里不覆盖顶部那条手动刷新的结论。
        });
    }, REALTIME_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [live, realtimeWindow]);

  const refresh = useCallback(() => {
    void load();
  }, [load]);

  return {
    overview,
    distribution,
    models,
    diagnostics,
    realtime,
    loading,
    refreshing,
    error,
    refreshedAt,
    refresh,
  };
}

/** 请求事件日志的状态。 */
export interface UsageEventsState {
  report: UsageAuditLogsReport | null;
  loading: boolean;
  error: string | null;
  /** 当前页码，从 1 起。 */
  page: number;
  /** 总页数，由总数与每页条数算出。 */
  pageCount: number;
  /** 已经能翻到的最大页码：游标只能向前推，未走过的页要按顺序解锁。 */
  reachable: number;
  pageSize: number;
  setPage: (page: number) => void;
  setPageSize: (size: number) => void;
  /** 重新取当前页。 */
  reload: () => void;
}

/** 每页条数档位；与网关 limit 上限 500 保持同一量级。 */
export const EVENT_PAGE_SIZES = [20, 50, 100] as const;

/**
 * 事件日志取数：传统分页。
 *
 * 网关按游标分页（只给「下一页」的游标），因此这里为每一页记住进入它的游标：
 * 第 1 页用空游标，取回时把 `nextCursor` 记成第 2 页的游标，依此类推。于是
 * 「上一页」随时可回（游标已经拿在手里），而「下一页」与更远的页码按顺序解锁，
 * 不做「先偷偷把中间几页全取一遍」这种多打请求的事。
 */
export function useUsageEvents(
  filter: UsageEventFilter,
  enabled: boolean,
  initialPageSize = 20,
): UsageEventsState {
  const [report, setReport] = useState<UsageAuditLogsReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(initialPageSize);
  const [reloadToken, setReloadToken] = useState(0);
  /** 进入第 N 页要用到的游标；下标 0 是第 1 页（空游标）。 */
  const cursors = useRef<(string | null)[]>([null]);
  /** 已经解锁到第几页：初值 1，每取回一页就往后推一格。 */
  const [reachable, setReachable] = useState(1);
  const generation = useRef(0);
  const filterKey = `${filter.model ?? ''}|${filter.source ?? ''}|${filter.status ?? ''}`;

  // 过滤条件或每页条数一变就回到第一页：换口径后停在旧页码只会给出错位的结果。
  useEffect(() => {
    cursors.current = [null];
    setReachable(1);
    setPage(1);
  }, [filterKey, pageSize]);

  useEffect(() => {
    // 没打开日志页就不发请求：明细是全表里最贵的一批读，按需取。
    if (!enabled) return;
    const cursor = cursors.current[page - 1] ?? null;
    const current = generation.current + 1;
    generation.current = current;
    setLoading(true);
    void ipc
      .usageEvents(filter, cursor, pageSize)
      .then((next) => {
        if (generation.current !== current) return;
        setReport(next);
        setError(null);
        if (next.nextCursor !== null && next.nextCursor !== undefined) {
          cursors.current[page] = next.nextCursor;
          setReachable((previous) => Math.max(previous, page + 1));
        }
      })
      .catch((failure: unknown) => {
        if (generation.current !== current) return;
        setError(messageOf(failure));
      })
      .finally(() => {
        if (generation.current === current) setLoading(false);
      });
    // filterKey 已经把过滤条件压成稳定字符串：值没变就不重取，对象本身每次渲染都是新的。
  }, [filterKey, pageSize, page, enabled, reloadToken]);

  const totalCount = report?.totalCount ?? 0;
  const pageCount = Math.max(1, Math.ceil(totalCount / Math.max(1, pageSize)));

  return {
    report,
    loading,
    error,
    page,
    pageCount,
    reachable,
    pageSize,
    setPage,
    setPageSize,
    reload: () => setReloadToken((token) => token + 1),
  };
}
