import React, { useMemo, useState } from 'react';
import { Banner, LoadingState, Text } from '../ui/surfaces';
import { IconButton, SegmentedControl } from '../ui/controls';
import { AlertCircleIcon, RefreshIcon } from '../../icons';
import type { UsageDimension, UsageEventFilter, UsagePeriod, UsageRealtimeWindow } from '../../types';
import { useUsageData, useUsageEvents } from '../usage/usageData';
import {
  calendarWeeks,
  granularityForPeriod,
  healthRows,
  periodStartMs,
  trendBuckets,
} from '../usage/usageBuckets';
import { UsageTabStrip } from '../usage/UsageTabs';
import { UsageKpiStrip } from '../usage/charts/UsageKpiStrip';
import { TokenTrendChart, type TrendMode } from '../usage/charts/TokenTrendChart';
import { ThroughputChart, RealtimeWindows } from '../usage/charts/ThroughputChart';
import { TokenCompositionDonut } from '../usage/charts/TokenCompositionDonut';
import { RankingBars } from '../usage/charts/RankingBars';
import { ActivityHeatmap } from '../usage/charts/ActivityHeatmap';
import { HealthTimeline } from '../usage/charts/HealthTimeline';
import { LatencyLadder } from '../usage/charts/LatencyLadder';
import { LatencyScatter } from '../usage/charts/LatencyScatter';
import { RecentRequests } from '../usage/RecentRequests';
import { formatDateTime, formatRelative } from '../usage/usageFormat';

/*
 * 用量统计分区。
 *
 * 版面按「页签分屏 + 每屏若干独立面板」组织：一屏里的面板各自成块（白底、发丝描边、
 * 柔光、三层结构），页面底板下沉一档，因此每一块都有明确边界，块与块之间靠底板而
 * 不是靠留白分开。
 *
 * 页签按问题的层次分：概览回答「用了多少」，实时回答「此刻在跑什么」，
 * 活动回答「什么时候在跑、跑得好不好」，诊断回答「慢在哪里」，日志回答「具体是哪一次」。
 * 周期与刷新是跨页签的口径控件，停在页签条右侧；维度、指标、窗口这些只影响单块的
 * 筛选放在各自面板的标题行里，不堆到页面级。
 *
 * 全部数据经网关 `/api/v1/observability/*` 读取：桌面端不直连统计库，
 * 也不在本地重算分位、分级与计费口径，网关给什么事实就画什么事实。
 */

const TABS = [
  { value: 'overview', label: '概览' },
  { value: 'realtime', label: '实时' },
  { value: 'activity', label: '活动' },
  { value: 'latency', label: '诊断' },
  { value: 'events', label: '请求日志' },
] as const;

type UsageTab = (typeof TABS)[number]['value'];

const PERIODS: { value: UsagePeriod; label: string }[] = [
  { value: 'today', label: '今天' },
  { value: '7d', label: '7 天' },
  { value: '30d', label: '30 天' },
  { value: 'all', label: '全部' },
];

const PERIOD_LABEL: Record<UsagePeriod, string> = {
  today: '今天',
  '7d': '最近 7 天',
  '30d': '最近 30 天',
  all: '全部时间',
};

const DIMENSIONS: { value: UsageDimension; label: string }[] = [
  { value: 'model', label: '按模型' },
  { value: 'provider', label: '按供应商' },
];

const DIMENSION_LABEL: Record<UsageDimension, string> = {
  model: '模型',
  provider: '供应商',
};

/** 日历热力图覆盖的周数：窗口越长回看越远，但不超过一屏。 */
const WEEKS_BY_PERIOD: Record<UsagePeriod, number> = {
  today: 5,
  '7d': 8,
  '30d': 16,
  all: 26,
};

export const UsageStatistics: React.FC = () => {
  const [tab, setTab] = useState<UsageTab>('overview');
  const [period, setPeriod] = useState<UsagePeriod>('7d');
  const [dimension, setDimension] = useState<UsageDimension>('model');
  const [trendMode, setTrendMode] = useState<TrendMode>('composition');
  const [realtimeWindow, setRealtimeWindow] = useState<UsageRealtimeWindow>('60m');
  const [eventFilter, setEventFilter] = useState<UsageEventFilter>({});

  // 请求日志按需取数：没打开那一页就不发这一批请求。
  const data = useUsageData(period, dimension, realtimeWindow, tab === 'realtime');
  const events = useUsageEvents(eventFilter, tab === 'events');

  /** 时间窗与派生几何：跟着数据与周期走，不在渲染里反复取当前时间。 */
  const derived = useMemo(() => {
    const now = Date.now();
    const points = data.distribution?.timeSeries ?? [];
    const earliest = points.reduce<number | null>(
      (min, point) => (min === null || point.timestampMs < min ? point.timestampMs : min),
      null,
    );
    const fromMs =
      period === 'all' ? (earliest ?? now - 30 * 86_400_000) : periodStartMs(period, now);
    const trend = trendBuckets(points, granularityForPeriod(period), fromMs, now);
    return {
      now,
      granularity: trend.granularity,
      buckets: trend.buckets,
      calendar: calendarWeeks(
        data.overview?.activityHeatmap.buckets ?? [],
        WEEKS_BY_PERIOD[period],
        now,
      ),
      health: healthRows(data.overview?.healthTimeline.buckets ?? [], fromMs, now),
    };
  }, [data.distribution, data.overview, period]);

  const firstLoad = data.loading && data.overview === null && data.distribution === null;

  return (
    <div className="space-y-3 min-w-0">
      <UsageTabStrip
        label="用量看板视图"
        tabs={TABS}
        value={tab}
        onChange={setTab}
        trailing={
          <>
            <SegmentedControl
              label="统计周期"
              value={period}
              onChange={setPeriod}
              options={PERIODS}
            />
            <Text tone="secondary" size="meta">
              {data.refreshing
                ? '正在刷新'
                : data.refreshedAt === null
                  ? '尚未取数'
                  : `更新于 ${formatRelative(data.refreshedAt, derived.now)}`}
            </Text>
            <IconButton
              label="刷新用量数据"
              disabled={data.refreshing}
              onClick={data.refresh}
              className="hairline border-border-hairline bg-surface"
            >
              <RefreshIcon size={14} />
            </IconButton>
          </>
        }
      />

      {data.error !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={14} />}
          title={`部分用量数据暂时不可用：${data.error}`}
        />
      )}

      {firstLoad ? (
        <LoadingState label="正在读取用量数据" />
      ) : (
        <div
          role="tabpanel"
          aria-label={TABS.find((item) => item.value === tab)?.label}
          className="@container"
        >
          {tab === 'overview' && (
            <div className="space-y-3">
              <UsageKpiStrip overview={data.overview} />
              <TokenTrendChart
                buckets={derived.buckets}
                granularity={derived.granularity}
                mode={trendMode}
                onModeChange={setTrendMode}
                periodLabel={PERIOD_LABEL[period]}
                totalTokens={data.overview?.summary.totalTokens ?? 0}
              />
              <RankingBars
                items={data.distribution?.items ?? []}
                dimensionLabel={DIMENSION_LABEL[dimension]}
                periodLabel={PERIOD_LABEL[period]}
                action={
                  <SegmentedControl
                    label="排行维度"
                    value={dimension}
                    onChange={setDimension}
                    options={DIMENSIONS}
                  />
                }
              />
            </div>
          )}

          {tab === 'realtime' && (
            <div className="grid grid-cols-1 @[900px]:grid-cols-2 gap-3">
              <ThroughputChart
                report={data.realtime}
                window={realtimeWindow}
                action={
                  <SegmentedControl
                    label="实时窗口"
                    value={realtimeWindow}
                    onChange={setRealtimeWindow}
                    options={RealtimeWindows}
                  />
                }
              />
              <TokenCompositionDonut report={data.realtime} />
            </div>
          )}

          {tab === 'activity' && (
            <div className="grid grid-cols-1 @[900px]:grid-cols-2 gap-3">
              <ActivityHeatmap
                cells={derived.calendar}
                metricLabel={`口径：按当天的 Token 总量（输入、输出与推理之和）分为 5 档，档位由网关按窗口内最大值计算，当前度量 ${data.overview?.activityHeatmap.metric ?? 'tokens'}`}
              />
              <HealthTimeline rows={derived.health} />
            </div>
          )}

          {tab === 'latency' && (
            <div className="grid grid-cols-1 @[900px]:grid-cols-2 gap-3">
              <LatencyLadder percentiles={data.diagnostics?.percentiles ?? null} />
              <LatencyScatter
                points={data.diagnostics?.scatterPoints ?? []}
                percentiles={data.diagnostics?.percentiles ?? null}
              />
            </div>
          )}

          {tab === 'events' && (
            <RecentRequests
              events={events.report?.events ?? null}
              totalCount={events.report?.totalCount ?? 0}
              loading={events.loading}
              error={events.error}
              page={events.page}
              pageCount={events.pageCount}
              reachable={events.reachable}
              pageSize={events.pageSize}
              onPageChange={events.setPage}
              onPageSizeChange={events.setPageSize}
              filter={eventFilter}
              onFilterChange={setEventFilter}
              modelOptions={data.models}
            />
          )}

          <Text tone="tertiary" size="meta" className="block pt-3 pb-1">
            数据来自 Daemon 的用量投影库，后台每 60 秒增量投影一次；诊断窗口上限为 7 天，
            实时读数来自进程内的滑动窗口，窗口随 Daemon 重启清空。
            {data.diagnostics?.sampling !== undefined &&
              ` 诊断样本抽稀：从 ${data.diagnostics.sampling.sampledOf} 条中采纳 ${data.diagnostics.sampling.sampleCount} 条。`}
            {` 页面数据取于 ${formatDateTime(derived.now)}。`}
          </Text>
        </div>
      )}
    </div>
  );
};
