import React from "react";
import type { CronJob } from "../../types";
import { ClockIcon, PlusIcon, SearchIcon } from "../../icons";
import { Button, Input, SegmentedControl, type SegmentedOption } from "../ui/controls";
import {
  Chip,
  EmptyState,
  Hairline,
  ListRow,
  LoadingState,
  Panel,
  StateDot,
  Text,
} from "../ui/surfaces";
import { Pagination } from "../ui/pagination";
import { formatMoment } from "../pageFormat";
import { scheduleText } from "./schedule";
import { jobState } from "./status";

/** 状态筛选：后端只认空串、active 与 paused 三档。 */
export type JobFilter = "all" | "active" | "paused";

const FILTERS: readonly SegmentedOption<JobFilter>[] = [
  { value: "all", label: "全部" },
  { value: "active", label: "运行中" },
  { value: "paused", label: "已暂停" },
];

const pageCount = (total: number, pageSize: number) =>
  Math.max(1, Math.ceil(total / pageSize));

const rangeSummary = (page: number, pageSize: number, total: number): string => {
  const start = (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, total);
  return `第 ${start} 至 ${end} 条，共 ${total} 条`;
};

export interface AutomationListProps {
  jobs: CronJob[];
  selectedId: string | null;
  query: string;
  filter: JobFilter;
  page: number;
  total: number;
  pageSize: number;
  busy: boolean;
  /** 首屏还在读第一页：不要先画一次「还没有任务」的假空态。 */
  loading: boolean;
  /** 列表里同时存在系统任务与用户任务时，才逐行标注「系统」。 */
  showSystemChip: boolean;
  className?: string;
  onQueryChange: (value: string) => void;
  onFilterChange: (value: JobFilter) => void;
  onSelect: (jobId: string) => void;
  onCreate: () => void;
  onClearFilters: () => void;
  onPageChange: (page: number) => void;
}

/**
 * 任务列表：搜索、状态筛选与分页固定在卡片首尾，只有行区域在列内滚动。
 */
export const AutomationList: React.FC<AutomationListProps> = ({
  jobs,
  selectedId,
  query,
  filter,
  page,
  total,
  pageSize,
  busy,
  loading,
  showSystemChip,
  className = "",
  onQueryChange,
  onFilterChange,
  onSelect,
  onCreate,
  onClearFilters,
  onPageChange,
}) => {
  const filtered = query.trim().length > 0 || filter !== "all";
  return (
    <Panel clip className={`flex min-h-0 flex-col ${className}`}>
      <div className="shrink-0 p-4 space-y-2.5">
        <Input
          icon={<SearchIcon size={14} />}
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          placeholder="搜索任务"
          aria-label="搜索任务"
          onClear={() => onQueryChange("")}
        />
        <SegmentedControl
          label="任务状态筛选"
          options={FILTERS}
          value={filter}
          onChange={onFilterChange}
        />
      </div>
      <Hairline />
      <div className="flex-1 min-h-0 overflow-y-auto scroll-visible">
        {loading && jobs.length === 0 ? (
          <LoadingState label="正在读取任务" />
        ) : jobs.length === 0 ? (
          <EmptyState
            icon={filtered ? <SearchIcon size={24} /> : <ClockIcon size={24} />}
            title={filtered ? "没有匹配的任务" : "还没有自动化任务"}
            description={
              filtered
                ? "换个关键词或状态筛选再看一次。"
                : "新建一个任务，让它按计划自动运行；每次执行都会留下记录。"
            }
            action={
              filtered ? (
                <Button variant="outline" size="sm" onClick={onClearFilters}>
                  清除筛选
                </Button>
              ) : (
                <Button variant="primary" size="sm" onClick={onCreate}>
                  <PlusIcon size={14} />
                  新建任务
                </Button>
              )
            }
          />
        ) : (
          jobs.map((job) => {
            const running = job.running === true;
            return (
              <ListRow
                key={job.jobId}
                selected={job.jobId === selectedId}
                onClick={() => onSelect(job.jobId)}
              >
                <StateDot state={jobState(job)} />
                <div className="min-w-0 flex-1">
                  <div className="flex items-baseline gap-2">
                    <Text size="sm" strong truncate className="min-w-0 flex-1">
                      {job.name}
                    </Text>
                    <Text
                      tone="caption"
                      size="xs"
                      className="shrink-0 tabular-nums"
                    >
                      {job.nextRunAtMs == null
                        ? "未安排"
                        : `下次 ${formatMoment(job.nextRunAtMs)}`}
                    </Text>
                  </div>
                  <div className="mt-0.5 flex items-center gap-1.5 min-w-0">
                    {running && (
                      <>
                        <Text tone="secondary" size="xs" className="shrink-0">
                          运行中
                        </Text>
                        <Text tone="tertiary" size="xs" className="shrink-0">
                          ·
                        </Text>
                      </>
                    )}
                    <Text tone="secondary" size="xs" truncate className="min-w-0">
                      {scheduleText(job.schedule)}
                    </Text>
                    {showSystemChip && job.system === true && (
                      <Chip className="ml-auto">系统</Chip>
                    )}
                  </div>
                </div>
              </ListRow>
            );
          })
        )}
      </div>
      {total > pageSize && (
        <Pagination
          page={page}
          pageCount={pageCount(total, pageSize)}
          pageSize={pageSize}
          busy={busy}
          summary={rangeSummary(page, pageSize, total)}
          onPageChange={onPageChange}
          label="任务分页"
        />
      )}
    </Panel>
  );
};
