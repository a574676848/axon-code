import React from 'react';

import { FolderIcon } from '../../icons';
import type { GoalCardView, GoalDetailView } from '../../goalTypes';
import { executionStatusLabel, goalStatusMeta, hasSessionBinding } from '../../goals/model';
import { formatDateTime } from '../pageFormat';
import { Dialog } from '../ui/overlays';
import { Chip, StateDot, Text } from '../ui/surfaces';
import { GoalContractSections } from './GoalContractSections';

/*
 * 目标详情弹窗。
 *
 * 顶部以紧凑信息格呈现状态、所属工作区、最近活动与执行轮次，
 * 主体直接复用 `GoalContractSections` 呈现目标内容、序号化步骤与验收标准，
 * 与执行会话顶部的概览卡保持同一套阅读排版。
 */

export interface GoalDetailDialogProps {
  /** 当前打开的目标；为空表示关闭。 */
  goal: GoalCardView | null;
  detail: GoalDetailView | null;
  loading: boolean;
  error: string | null;
  onClose: () => void;
  /** 进入执行会话；目标没有可订阅会话时不给这个动作。 */
  onEnterSession: (() => void) | null;
}

export const GoalDetailDialog: React.FC<GoalDetailDialogProps> = ({
  goal,
  detail,
  loading,
  error,
  onClose,
  onEnterSession,
}) => {
  if (goal === null) {
    return (
      <Dialog open={false} onClose={onClose} title="目标详情" size="lg">
        {null}
      </Dialog>
    );
  }

  // 详情比列表多解析一步全局目标的工作区，因此面板里的工作区与入口都以详情为准。
  const located = detail?.goal ?? goal;
  const meta = goalStatusMeta(located.status);
  const objective =
    detail !== null && detail.objective.length > 0 ? detail.objective : located.objectivePreview;
  const latestExecution = located.latestExecution;
  const latestResult = detail?.latestResult ?? located.latestResult;

  return (
    <Dialog
      open
      onClose={onClose}
      title={located.title}
      size="lg"
      primary={
        onEnterSession !== null && hasSessionBinding(located)
          ? { label: '进入会话', onClick: onEnterSession }
          : undefined
      }
    >
      <div className="space-y-3">
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 items-stretch">
          <div className="h-full rounded-sm bg-surface-sunken/55 hairline border-border-hairline p-3 flex flex-col justify-between gap-1.5 min-w-0">
            <Text tone="secondary" size="xs">
              当前状态
            </Text>
            <div className="flex items-center gap-1.5">
              <StateDot
                state={meta.dot}
                tone={meta.dot === 'ongoing' ? 'accent' : 'default'}
              />
              <Chip tone={meta.chipTone}>{meta.label}</Chip>
            </div>
          </div>

          <div className="h-full rounded-sm bg-surface-sunken/55 hairline border-border-hairline p-3 flex flex-col justify-between gap-1.5 min-w-0">
            <Text tone="secondary" size="xs">
              所属工作区
            </Text>
            <div
              className="flex items-center gap-1.5 min-w-0"
              title={located.workspacePath ?? undefined}
            >
              <FolderIcon size={13} className="shrink-0 text-text-tertiary" />
              <Text size="row" strong truncate className="min-w-0">
                {located.workspaceName ?? '全局'}
              </Text>
            </div>
          </div>

          <div className="h-full rounded-sm bg-surface-sunken/55 hairline border-border-hairline p-3 flex flex-col justify-between gap-1.5 min-w-0">
            <Text tone="secondary" size="xs">
              {latestExecution !== null ? `最近执行（第 ${latestExecution.attempt} 次）` : '最近活动'}
            </Text>
            <Text size="row" strong className="block tabular-nums truncate">
              {latestExecution !== null
                ? `${executionStatusLabel(latestExecution.status)} · ${formatDateTime(latestExecution.createdAtMs)}`
                : formatDateTime(located.lastActivityAtMs)}
            </Text>
          </div>
        </div>

        <GoalContractSections
          variant="dialog"
          completed={located.status === 'completed'}
          objective={objective}
          steps={detail?.steps ?? []}
          acceptance={detail?.acceptance ?? []}
          objectiveTruncated={detail?.objectiveTruncated === true}
          listsTruncated={detail?.listsTruncated === true}
          cancellationReason={detail?.cancellationReason ?? null}
          latestResultSummary={latestResult?.summaryPreview ?? null}
          latestResultSucceeded={latestResult?.succeeded ?? null}
          loading={loading}
          error={error}
        />
      </div>
    </Dialog>
  );
};
