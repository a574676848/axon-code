import React from 'react';

import type { GoalCardView, GoalDetailView } from '../../goalTypes';
import { executionStatusLabel, goalStatusMeta, hasSessionBinding } from '../../goals/model';
import { formatDateTime } from '../pageFormat';
import { CardDetailDialog, type DetailField } from '../ui/cards';

/*
 * 目标详情弹窗。
 *
 * 复用条目卡的统一详情弹窗：字段名在上、值在下，没有第二套详情排版。
 * 无执行记录、全局目标解析不到工作区，或详情读取失败时，这里都是唯一的看处，
 * 因此字段必须如实说明「缺什么」，不补占位。
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

/** 步骤与验收按行呈现，空集合给「无」。 */
function listField(label: string, items: readonly string[]): DetailField {
  return { label, value: items.length === 0 ? '无' : items.join('\n') };
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
      <CardDetailDialog open={false} onClose={onClose} title="目标详情" fields={[]} />
    );
  }
  const meta = goalStatusMeta(goal.status);
  // 详情比列表多解析一步全局目标的工作区，因此面板里的工作区与入口都以详情为准。
  const located = detail?.goal ?? goal;
  const fields: DetailField[] = [
    { label: '状态', value: meta.label },
    { label: '目标', value: loading ? '正在读取' : (detail?.objective ?? goal.objectivePreview) },
  ];
  if (detail !== null) {
    fields.push(listField('步骤', detail.steps));
    fields.push(listField('验收', detail.acceptance));
    if (detail.objectiveTruncated) {
      fields.push({ label: '截断说明', value: '目标正文超过 20000 字，此处已截断' });
    }
    if (detail.listsTruncated) {
      fields.push({
        label: '截断说明',
        value: '步骤或验收超过 64 条或单条超过 2000 字，此处已截断',
      });
    }
    if (detail.cancellationReason !== null) {
      fields.push({ label: '取消原因', value: detail.cancellationReason });
    }
  }
  fields.push({
    label: '工作区',
    value:
      located.workspaceName === null
        ? '全局目标，未绑定工作区'
        : `${located.workspaceName}（${located.workspacePath ?? '路径未知'}）`,
  });
  fields.push({ label: '最近活动', value: formatDateTime(goal.lastActivityAtMs) });
  fields.push({
    label: '目标标识',
    value: goal.goalId,
    mono: true,
  });
  if (goal.latestExecution !== null) {
    const attempt = goal.latestExecution.attempt;
    fields.push({
      label: '最近执行',
      value: `${executionStatusLabel(goal.latestExecution.status)} · 第 ${attempt} 次 · ${formatDateTime(goal.latestExecution.createdAtMs)}`,
    });
  }
  if (goal.latestResult !== null) {
    fields.push({ label: '最近结果', value: goal.latestResult.summaryPreview });
  }
  if (error !== null) {
    fields.push({ label: '读取失败', value: error });
  }

  return (
    <CardDetailDialog
      open
      onClose={onClose}
      title={goal.title}
      fields={fields}
      primary={
        onEnterSession !== null && hasSessionBinding(located)
          ? { label: '进入会话', onClick: onEnterSession }
          : undefined
      }
    />
  );
};
