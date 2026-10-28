import React from 'react';

import { ChecklistIcon, ChevronRightIcon } from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import {
  cardActionLabel,
  goalLocatorText,
  goalStatusMeta,
  hasSessionBinding,
} from '../../goals/model';
import { formatDateTime } from '../pageFormat';
import { EntityCard } from '../ui/cards';
import { Chip, StateDot } from '../ui/surfaces';

/*
 * 目标卡：看板里一张目标卡片。
 *
 * 复用条目卡外壳（`EntityCard`），只填既有的四个信息位，不新写第二套卡片视觉：
 * 行首状态点、标题与状态标签、目标预览、下一步或等待原因、
 * 工作区与最近活动时间、右下唯一入口。
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

const GOAL_ICON_SIZE = 13;

export const GoalCard: React.FC<GoalCardProps> = ({ goal, onOpen, onInspect, now }) => {
  const meta = goalStatusMeta(goal.status);
  const activity = formatDateTime(goal.lastActivityAtMs, now);
  const workspace = goal.workspaceName ?? '未绑定工作区';
  return (
    <div data-goal-card="" data-goal-id={goal.goalId}>
      <EntityCard
        leading={<StateDot state={meta.dot} />}
        title={goal.title}
        badges={<Chip>{meta.label}</Chip>}
        description={goal.objectivePreview}
        locator={goalLocatorText(goal)}
        locatorMono={false}
        meta={`${workspace} · 最近活动 ${activity}`}
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
