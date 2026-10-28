import React from 'react';

import type { GoalCardView } from '../../goalTypes';
import { GOAL_COLUMNS, boardColumns, type GoalColumnId } from '../../goals/model';
import { GoalCard } from './GoalCard';
import { EmptyHint, Text } from '../ui/surfaces';

/*
 * 看板：四个状态栏。
 *
 * 版面规则来自设计系统：栏目是下沉面、卡片是浮起面，因此只有两层，不再套第三层；
 * 页面只有一层滚动，栏目自己不滚，长列表靠「显示其余 N 个」展开；
 * 栅格用容器查询，默认一列，560 / 820 / 1080 逐级加到四列。
 */

export interface GoalBoardProps {
  goals: readonly GoalCardView[];
  /** 已展开的栏目；展开后该栏不再限制张数。 */
  expanded: readonly GoalColumnId[];
  onExpand: (column: GoalColumnId) => void;
  /** 卡片唯一入口：能进会话就进会话，否则打开详情。页脚与卡片主体走同一个动作。 */
  onOpenGoal: (goal: GoalCardView) => void;
  /** 当前时间，供时间格式化使用。 */
  now: number;
}

export const GoalBoard: React.FC<GoalBoardProps> = ({
  goals,
  expanded,
  onExpand,
  onOpenGoal,
  now,
}) => {
  const columns = boardColumns(goals, expanded);
  return (
    <div className="@container">
      <div className="grid grid-cols-1 gap-3 @[560px]:grid-cols-2 @[820px]:grid-cols-3 @[1080px]:grid-cols-4">
        {columns.map((column) => (
          <section
            key={column.id}
            aria-label={`${column.label}，${column.total} 个目标`}
            className="rounded-md bg-surface-sunken hairline border-border-hairline p-2.5 flex flex-col gap-2 min-w-0"
          >
            <header className="flex items-baseline gap-2 px-0.5">
              <Text size="xs" strong>
                {column.label}
              </Text>
              <Text tone="tertiary" size="2xs" mono className="ml-auto tabular-nums">
                {column.total}
              </Text>
            </header>
            {column.visible.length === 0 ? (
              <EmptyHint className="py-2">暂无</EmptyHint>
            ) : (
              column.visible.map((goal) => (
                <GoalCard
                  key={goal.goalId}
                  goal={goal}
                  now={now}
                  onOpen={() => onOpenGoal(goal)}
                  onInspect={() => onOpenGoal(goal)}
                />
              ))
            )}
            {column.hidden > 0 && (
              <button
                type="button"
                onClick={() => onExpand(column.id)}
                className="h-7 shrink-0 rounded-sm text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
              >
                显示其余 {column.hidden} 个
              </button>
            )}
          </section>
        ))}
      </div>
    </div>
  );
};

/** 看板的栏目标题顺序，供工具栏与测试共用同一份来源。 */
export const BOARD_COLUMNS = GOAL_COLUMNS;
