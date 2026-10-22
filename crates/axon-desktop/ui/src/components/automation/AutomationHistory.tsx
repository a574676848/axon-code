import React from "react";
import type { CronHistoryResult } from "../../types";
import { AlertCircleIcon, ClockIcon } from "../../icons";
import { Button } from "../ui/controls";
import {
  Banner,
  EmptyState,
  LoadingState,
  Panel,
  StateDot,
  Text,
} from "../ui/surfaces";
import { Pagination } from "../ui/pagination";
import { formatDateTime, formatDuration } from "../pageFormat";

const pageCount = (total: number, pageSize: number) =>
  Math.max(1, Math.ceil(total / pageSize));

const rangeSummary = (page: number, pageSize: number, total: number): string => {
  const start = (page - 1) * pageSize + 1;
  const end = Math.min(page * pageSize, total);
  return `第 ${start} 至 ${end} 条，共 ${total} 条`;
};

export interface AutomationHistoryProps {
  history: CronHistoryResult | null;
  loading: boolean;
  error: string | null;
  page: number;
  pageSize: number;
  onLoad: (page: number) => void;
}

/**
 * 执行历史：读取中、没有记录与读取失败是三种不同状态，
 * 首帧不能先按「暂无执行记录」画一次，否则每次展开都会闪一下假空态。
 */
export const AutomationHistory: React.FC<AutomationHistoryProps> = ({
  history,
  loading,
  error,
  page,
  pageSize,
  onLoad,
}) => {
  if (loading) return <LoadingState label="正在读取执行历史" />;
  if (error !== null) {
    return (
      <Banner
        tone="danger"
        icon={<AlertCircleIcon size={14} />}
        title={error}
        action={
          <Button variant="outline" size="sm" onClick={() => onLoad(page)}>
            重试
          </Button>
        }
      />
    );
  }
  if (history === null) return null;
  if (history.runs.length === 0) {
    return (
      <EmptyState
        icon={<ClockIcon size={24} />}
        title="还没有执行记录"
        description="任务每次被调度都会在这里留下一条记录。"
      />
    );
  }
  return (
    <Panel clip>
      <div>
        {history.runs.map((run) => (
          <RunRow key={run.runId} run={run} />
        ))}
      </div>
      {history.total > pageSize && (
        <Pagination
          page={page}
          pageCount={pageCount(history.total, pageSize)}
          pageSize={pageSize}
          summary={rangeSummary(page, pageSize, history.total)}
          onPageChange={onLoad}
          label="执行历史分页"
        />
      )}
    </Panel>
  );
};

const RunRow: React.FC<{ run: CronHistoryResult["runs"][number] }> = ({ run }) => (
  <div className="flex items-start gap-2 px-3 py-2.5 hairline-b border-border-hairline last:border-b-0">
    <StateDot state={run.success ? "done" : "error"} className="mt-1" />
    <div className="min-w-0 flex-1">
      <div className="flex items-baseline gap-2">
        <Text size="sm" strong>
          {run.success ? "执行成功" : "执行失败"}
        </Text>
        <Text tone="caption" size="xs" className="ml-auto shrink-0 tabular-nums">
          {formatDateTime(run.startedAtMs)}
        </Text>
      </div>
      <div className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-0.5">
        <Text tone="secondary" size="xs">
          耗时 {formatDuration(run.finishedAtMs - run.startedAtMs)}
        </Text>
        <Text tone="secondary" size="xs">
          {run.agentExecuted ? "Agent 会话" : "脚本任务"}
        </Text>
        {run.deliveryEnqueued && (
          <Text tone="secondary" size="xs">
            已进入投递队列
          </Text>
        )}
      </div>
      {run.errorSummary != null && run.errorSummary.length > 0 && (
        <pre className="mt-1 text-code text-danger whitespace-pre-wrap break-words font-mono">
          {run.errorSummary}
        </pre>
      )}
    </div>
  </div>
);
