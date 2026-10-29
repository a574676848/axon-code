import React from 'react';

import { ChecklistIcon, ChevronRightIcon, FolderIcon } from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import {
  cardActionLabel,
  goalProgressCue,
  goalStatusMeta,
  hasSessionBinding,
} from '../../goals/model';
import { formatDateTime } from '../pageFormat';
import { EntityCard } from '../ui/cards';
import { Chip, StateDot } from '../ui/surfaces';

/*
 * 目标卡：看板里一张目标卡片。
 *
 * 复用条目卡外壳（`EntityCard`），锁定统一高度（`uniformHeight`）：
 * 行首状态点、单行标题与语义状态标签、固定两行目标预览槽、单行行内进展提示、
 * 36px 底栏（工作区与活动时刻 + 右下唯一入口），全看板卡片高度与比例完全一致。
 */

export interface GoalCardProps {
  goal: GoalCardView;
  /** 卡片主体：能进会话就进会话，否则打开详情。 */
  onOpen: () => void;
  /** 右下入口。 */
  onInspect: () => void;
  /** 当前时间，供时间格式化使用。 */
  now: number;
}

const GOAL_ICON_SIZE = 12;

export const GoalCard: React.FC<GoalCardProps> = ({ goal, onOpen, onInspect, now }) => {
  const meta = goalStatusMeta(goal.status);
  const activity = formatDateTime(goal.lastActivityAtMs, now);
  const workspace = goal.workspaceName ?? '全局';
  const cue = goalProgressCue(goal) ?? {
    kind: 'next' as const,
    label: '进度',
    text: goal.stepCount > 0 ? `共 ${goal.stepCount} 个执行步骤` : meta.label,
    tone: 'default' as const,
  };
  const metaTooltip = `${workspace} · ${activity}`;

  return (
    <div data-goal-card="" data-goal-id={goal.goalId} className="goal-card-in">
      <EntityCard
        uniformHeight
        leading={<StateDot state={meta.dot} tone={meta.dot === 'ongoing' ? 'accent' : 'default'} />}
        title={goal.title}
        badges={<Chip tone={meta.chipTone}>{meta.label}</Chip>}
        description={goal.objectivePreview}
        statusCue={cue}
        meta={metaTooltip}
        metaNode={
          <span
            title={metaTooltip}
            className="flex items-center gap-1.5 min-w-0 text-micro text-text-tertiary tabular-nums"
          >
            <FolderIcon size={12} className="shrink-0 text-text-caption" />
            <span className="truncate">{workspace}</span>
            <span aria-hidden="true" className="shrink-0 text-text-caption">
              ·
            </span>
            <span className="shrink-0 font-mono text-text-caption">{activity}</span>
          </span>
        }
        onOpen={onOpen}
        onInspect={onInspect}
        inspectLabel={cardActionLabel(goal)}
        actionIcon={
          hasSessionBinding(goal) ? (
            <ChevronRightIcon size={GOAL_ICON_SIZE} />
          ) : (
            <ChecklistIcon size={GOAL_ICON_SIZE} />
          )
        }
      />
    </div>
  );
};
