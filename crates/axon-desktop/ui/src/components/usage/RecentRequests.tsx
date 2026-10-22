import React from 'react';
import { Select } from '../ui/overlays';
import { Pagination } from '../ui/pagination';
import { Banner, EmptyState, LoadingState, Panel, StateDot, Text } from '../ui/surfaces';
import { AlertCircleIcon } from '../../icons';
import type { UsageAuditEvent, UsageEventFilter } from '../../types';
import { PanelHeader } from '../settings/PanelHeader';
import { EVENT_PAGE_SIZES } from './usageData';
import {
  costLabel,
  formatCount,
  formatDateTime,
  formatLatency,
  formatPercent,
  formatThroughput,
  formatTokens,
  sourceLabel,
  statusLabel,
  statusTone,
} from './usageFormat';

/*
 * 请求事件日志：最近请求的明细表。
 *
 * 图给出趋势，表给出「到底是哪一次」。因此这一段的职责是把单次请求的原样事实
 * 摊平：时间、模型（含上游真实模型）、来源、终态、耗时、首字延迟、Token 分类、
 * 缓存命中与成本。不做任何二次聚合，也不隐藏未定价的条目。
 *
 * 过滤与翻页都走网关：`limit` 与游标由后端执行，界面不把全量拉到本地再筛。
 */

/** 一行的读数列宽：等宽数字右对齐，位数变化不带着标题抖。 */
const COLUMNS = [
  { key: 'time', label: '时间', width: 'w-[92px]', align: 'text-left' },
  { key: 'model', label: '模型', width: 'w-[180px]', align: 'text-left' },
  { key: 'source', label: '来源', width: 'w-[68px]', align: 'text-left' },
  { key: 'status', label: '结果', width: 'w-[80px]', align: 'text-left' },
  { key: 'duration', label: '总耗时', width: 'w-[80px]', align: 'text-right' },
  { key: 'ttft', label: '首字', width: 'w-[76px]', align: 'text-right' },
  { key: 'speed', label: '速率', width: 'w-[84px]', align: 'text-right' },
  { key: 'input', label: '输入', width: 'w-[76px]', align: 'text-right' },
  { key: 'output', label: '输出', width: 'w-[68px]', align: 'text-right' },
  { key: 'reasoning', label: '推理', width: 'w-[68px]', align: 'text-right' },
  { key: 'cache', label: '缓存命中', width: 'w-[76px]', align: 'text-right' },
  { key: 'cost', label: '成本', width: 'w-[84px]', align: 'text-right' },
] as const;

const EVENT_DOT = {
  ok: 'done',
  danger: 'error',
  warning: 'warning',
  idle: 'idle',
} as const;

const StatusCell: React.FC<{ status: string }> = ({ status }) => (
  <span className="inline-flex items-center gap-1.5">
    <StateDot state={EVENT_DOT[statusTone(status)]} />
    <Text size="row">{statusLabel(status)}</Text>
  </span>
);

const Row: React.FC<{ event: UsageAuditEvent }> = ({ event }) => (
  <tr className="h-9 hover:bg-hover">
    <td className="px-3 align-middle whitespace-nowrap">
      <Text tone="secondary" size="row">
        {formatDateTime(event.recordedAtMs)}
      </Text>
    </td>
    <td className="px-3 align-middle min-w-0">
      <Text size="row" truncate title={event.upstreamModel ?? event.model}>
        {event.model}
      </Text>
      {event.upstreamModel !== null &&
        event.upstreamModel !== undefined &&
        event.upstreamModel !== event.model && (
          <Text tone="tertiary" size="2xs" truncate className="block">
            上游 {event.upstreamModel}
          </Text>
        )}
    </td>
    <td className="px-3 align-middle whitespace-nowrap">
      <Text tone="secondary" size="row">
        {sourceLabel(event.source)}
      </Text>
    </td>
    <td className="px-3 align-middle whitespace-nowrap">
      <StatusCell status={event.status} />
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text size="row">{formatLatency(event.totalDurationMs)}</Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text size="row">{formatLatency(event.ttftMs)}</Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text tone="secondary" size="row">
        {formatThroughput(event.generationSpeedTps)}
      </Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text tone="secondary" size="row" mono>
        {formatTokens(event.inputTokens)}
      </Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text tone="secondary" size="row" mono>
        {formatTokens(event.outputTokens)}
      </Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text tone="secondary" size="row" mono>
        {formatTokens(event.reasoningTokens)}
      </Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text tone="secondary" size="row" mono>
        {formatPercent(event.cacheHitRateBasisPoints, 0)}
      </Text>
    </td>
    <td className="px-3 align-middle text-right whitespace-nowrap">
      <Text size="row">{costLabel(event.costMicros ?? 0, 1)}</Text>
    </td>
  </tr>
);

/** 分页条由共用组件承担；这里只保留表的列定义与行渲染。 */
function eventRowKey(event: UsageAuditEvent, index: number): string {
  return `${event.recordedAtMs}-${index}`;
}

export const RecentRequests: React.FC<{
  events: UsageAuditEvent[] | null;
  totalCount: number;
  loading: boolean;
  error: string | null;
  page: number;
  pageCount: number;
  /** 已经解锁的最大页码：游标分页下更远的页要按顺序翻过去。 */
  reachable: number;
  pageSize: number;
  onPageChange: (page: number) => void;
  onPageSizeChange: (size: number) => void;
  filter: UsageEventFilter;
  onFilterChange: (filter: UsageEventFilter) => void;
  /** 模型候选来自同一份分布数据，避免再取一次全量模型清单。 */
  modelOptions: string[];
}> = ({
  events,
  totalCount,
  loading,
  error,
  page,
  pageCount,
  reachable,
  pageSize,
  onPageChange,
  onPageSizeChange,
  filter,
  onFilterChange,
  modelOptions,
}) => {
  const rows = events ?? [];
  const none = rows.length === 0;
  const rangeStart = totalCount === 0 ? 0 : (page - 1) * pageSize + 1;
  const rangeEnd = (page - 1) * pageSize + rows.length;

  return (
    <Panel className="flex flex-col min-w-0 shadow-panel">
      {/* 标题层：这张表是什么，以及过滤条件。 */}
      <PanelHeader
        title="请求事件日志"
        description="单次请求的原样事实，按时间倒序；过滤与分页都由网关执行"
        action={
          <>
            <Select
              label="按模型过滤"
              value={filter.model ?? ''}
              className="w-[168px]"
              options={[
                { value: '', label: '全部模型' },
                ...modelOptions.map((model) => ({ value: model, label: model })),
              ]}
              onChange={(value) =>
                onFilterChange({ ...filter, model: value === '' ? undefined : value })
              }
            />
            <Select
              label="按来源过滤"
              value={filter.source ?? ''}
              className="w-[120px]"
              options={[
                { value: '', label: '全部来源' },
                { value: 'axon_cli', label: '主 Agent' },
                { value: 'subagent', label: '子代理' },
              ]}
              onChange={(value) =>
                onFilterChange({ ...filter, source: value === '' ? undefined : value })
              }
            />
            <Select
              label="按结果过滤"
              value={filter.status ?? ''}
              className="w-[120px]"
              options={[
                { value: '', label: '全部结果' },
                { value: 'completed', label: '成功' },
                { value: 'failed', label: '失败' },
                { value: 'interrupted', label: '中断' },
              ]}
              onChange={(value) =>
                onFilterChange({ ...filter, status: value === '' ? undefined : value })
              }
            />
          </>
        }
      />

      {/* 表格层 */}
      <div className="flex-1 min-w-0 px-3.5 py-3 space-y-2">
        {error !== null && (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={13} />}
            title={`读取事件日志失败：${error}`}
          />
        )}
        {loading && none ? (
          <LoadingState label="正在读取事件日志" />
        ) : none ? (
          <EmptyState
            title="没有匹配的请求事件"
            description="换一个过滤条件，或等下一批请求写入后再看"
          />
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full border-collapse">
              <thead>
                <tr className="hairline-b border-border-hairline">
                  {COLUMNS.map((column) => (
                    <th
                      key={column.key}
                      scope="col"
                      className={`px-3 pb-1.5 ${column.width} ${column.align}`}
                    >
                      <Text tone="secondary" size="xs" strong>
                        {column.label}
                      </Text>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((event, index) => (
                  <Row key={eventRowKey(event, index)} event={event} />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {/* 分页层：传统分页，页码加每页条数；一条都没有时不画，空态已经把话说完。 */}
      {rows.length > 0 && (
        <Pagination
          label="请求事件分页"
          page={page}
          pageCount={pageCount}
          reachable={reachable}
          pageSize={pageSize}
          pageSizeOptions={EVENT_PAGE_SIZES}
          onPageChange={onPageChange}
          onPageSizeChange={onPageSizeChange}
          busy={loading}
          summary={`第 ${rangeStart} 至 ${rangeEnd} 条，共 ${formatCount(totalCount)} 条`}
        />
      )}
    </Panel>
  );
};
