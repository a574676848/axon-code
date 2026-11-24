import React from 'react';

import type { GoalCardView } from '../../goalTypes';
import { GOAL_COLUMNS, boardColumns, type GoalColumnId } from '../../goals/model';
import { GoalCard } from './GoalCard';
import { StateDot, Text } from '../ui/surfaces';

/*
 * 看板：四个状态栏。
 *
 * 版面规则来自设计系统：栏目是下沉面、卡片是浮起面，因此只有两层，不再套第三层；
 * 四栏等高拉伸（items-stretch）铺满工作台舞台余下高度，彻底消除四栏高低参差与底部大块留白；
 * 栅格用容器查询，默认一列，520 / 760 / 1000 逐级加到四列。
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
    <div className="@container flex-1 min-h-0 flex flex-col">
      <div className="flex-1 min-h-0 grid grid-cols-1 items-stretch gap-3.5 @[520px]:grid-cols-2 @[760px]:grid-cols-3 @[1000px]:grid-cols-4">
        {columns.map((column) => (
          <section
            key={column.id}
            aria-label={`${column.label}，${column.total} 个目标`}
            className="h-full min-h-[360px] rounded-md bg-surface-sunken/80 hairline border-border-hairline p-3.5 flex flex-col gap-3 min-w-0 overflow-y-auto scroll-visible"
          >
            <header className="shrink-0 flex items-center gap-2 px-0.5">
              <StateDot
                state={column.dot}
                tone={column.dot === 'ongoing' ? 'accent' : 'default'}
              />
              <Text size="row" strong>
                {column.label}
              </Text>
              <span className="ml-auto inline-flex items-center justify-center min-w-5 h-4 px-1.5 rounded-full bg-surface/80 hairline border-border-hairline font-mono text-micro tabular-nums text-text-tertiary">
                {column.total}
              </span>
            </header>
            {column.visible.length === 0 ? (
              <div className="flex-1 flex items-center justify-center">
                <Text tone="caption" size="xs">
                  暂无目标
                </Text>
              </div>
            ) : (
              <div className="flex flex-col gap-2.5">
                {column.visible.map((goal) => (
                  <GoalCard
                    key={goal.goalId}
                    goal={goal}
                    now={now}
                    onOpen={() => onOpenGoal(goal)}
                    onInspect={() => onOpenGoal(goal)}
                  />
                ))}
              </div>
            )}
            {column.hidden > 0 && (
              <button
                type="button"
                onClick={() => onExpand(column.id)}
                className="btn-tactile mt-auto h-7 shrink-0 rounded-sm bg-surface/60 hairline border-border-hairline text-xs font-medium text-text-secondary hover:bg-surface hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
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
