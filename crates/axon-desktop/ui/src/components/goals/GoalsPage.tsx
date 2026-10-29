import React from 'react';

import { AlertCircleIcon, ChecklistIcon, RefreshIcon, SearchIcon } from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import {
  countGoals,
  countSummary,
  hasSessionBinding,
  selectGoals,
  type GoalColumnId,
  type GoalFilter,
} from '../../goals/model';
import { useGoalBoard, useGoalDetail } from '../../goals/useGoalBoard';
import { GOAL_LIST_LIMIT } from '../../goalIpc';
import { Button, Input, SegmentedControl } from '../ui/controls';
import { PageShell } from '../ui/shell';
import { Banner, EmptyState, LoadingState, Panel, Text } from '../ui/surfaces';
import { GoalBoard } from './GoalBoard';
import { GoalDetailDialog } from './GoalDetailDialog';
import { GoalRunView } from './GoalRunView';

/*
 * 目标看板页。
 *
 * 两种页面状态共用一层外壳：看板（PageShell 40px 顶栏）与执行会话
 * （自带 40px 顶栏）。两态互斥，因此任何时候都只有一条顶栏，不会叠出两行标题。
 *
 * 数据只有一条来源：网关的只读 goals 面。页面不直连数据库，也不缓存第二份事实。
 */

const FILTER_OPTIONS: readonly { value: GoalFilter; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'open', label: '未完成' },
  { value: 'settled', label: '已结束' },
];

const PAGE_TITLE = '目标看板';

/** 看板里查找一张卡片；用于返回时把焦点交回来源。 */
function findCardElement(goalId: string): HTMLElement | null {
  const cards = Array.from(
    document.querySelectorAll<HTMLElement>('[data-goal-card]'),
  );
  const card = cards.find((element) => element.dataset.goalId === goalId);
  return card?.querySelector<HTMLElement>('button') ?? null;
}

export const GoalsPage: React.FC = () => {
  const [query, setQuery] = React.useState('');
  const [filter, setFilter] = React.useState<GoalFilter>('all');
  const [expanded, setExpanded] = React.useState<GoalColumnId[]>([]);
  const [activeGoal, setActiveGoal] = React.useState<GoalCardView | null>(null);
  const [detailGoal, setDetailGoal] = React.useState<GoalCardView | null>(null);
  /** 返回看板后要聚焦的那张卡；为空表示不需要回位。 */
  const [pendingFocusGoalId, setPendingFocusGoalId] = React.useState<string | null>(null);
  const backRef = React.useRef<HTMLButtonElement | null>(null);
  // 进入会话视图后看板不再轮询：这一屏不显示列表，继续拉只是白花一组请求。
  const feed = useGoalBoard(activeGoal !== null);
  const detailFeed = useGoalDetail(detailGoal?.goalId ?? null, detailGoal !== null);

  const goals = feed.view?.goals ?? [];
  const visible = React.useMemo(
    () => selectGoals(goals, query, filter),
    [goals, query, filter],
  );
  // 计数口径取网关回执的 returned；筛选只改变看板里摆哪些卡。
  const counts = React.useMemo(
    () => countGoals(goals, feed.view?.returned ?? 0),
    [goals, feed.view?.returned],
  );
  const truncated = feed.view?.truncated === true;
  const hasActiveFilter = query.trim().length > 0 || filter !== 'all';

  /** 栏目一旦展开就保持展开：用户刚要求看全部，不该在下一拍轮询时被收回。 */
  const expandColumn = React.useCallback((column: GoalColumnId) => {
    setExpanded((current) => (current.includes(column) ? current : [...current, column]));
  }, []);

  const enterSession = React.useCallback((goal: GoalCardView) => {
    setDetailGoal(null);
    setActiveGoal(goal);
  }, []);

  const openGoal = React.useCallback(
    (goal: GoalCardView) => {
      if (hasSessionBinding(goal)) enterSession(goal);
      else setDetailGoal(goal);
    },
    [enterSession],
  );

  const leaveSession = React.useCallback(() => {
    setPendingFocusGoalId(activeGoal?.goalId ?? null);
    setActiveGoal(null);
  }, [activeGoal]);

  const clearFilters = React.useCallback(() => {
    setQuery('');
    setFilter('all');
  }, []);

  // 焦点回位：只在真的切回看板之后执行一次，找得到卡片才移动焦点。
  React.useEffect(() => {
    if (activeGoal !== null || pendingFocusGoalId === null) return;
    const target = findCardElement(pendingFocusGoalId);
    setPendingFocusGoalId(null);
    target?.focus();
  }, [activeGoal, pendingFocusGoalId]);

  if (activeGoal !== null) {
    return (
      <GoalRunView goal={activeGoal} onBack={leaveSession} backRef={backRef} />
    );
  }

  const refreshAction = (
    <Button variant="ghost" size="sm" onClick={feed.refresh}>
      <RefreshIcon size={13} />
      刷新
    </Button>
  );

  return (
    <PageShell title={PAGE_TITLE} actions={refreshAction} fill wide>
      <div className="flex-1 min-h-0 flex flex-col gap-3.5">
        <div className="shrink-0 flex flex-wrap items-center gap-2.5">
          <div className="w-full sm:w-80">
            <Input
              icon={<SearchIcon size={13} />}
              value={query}
              placeholder="搜索目标、工作区或下一步"
              aria-label="搜索目标、工作区或下一步"
              onChange={(event) => setQuery(event.target.value)}
              onClear={query.length > 0 ? () => setQuery('') : undefined}
            />
          </div>
          <SegmentedControl
            label="按状态筛选"
            options={FILTER_OPTIONS}
            value={filter}
            onChange={setFilter}
          />
          <Text tone="tertiary" size="xs" className="ml-auto tabular-nums">
            {countSummary(counts, truncated, GOAL_LIST_LIMIT)}
          </Text>
        </div>

        {feed.pollError !== null && (
          <div className="shrink-0">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={14} />}
              title={feed.pollError}
              action={refreshAction}
            />
          </div>
        )}

        {feed.loading ? (
          <LoadingState label="正在读取目标" className="flex-1" />
        ) : feed.view === null ? (
          <div className="shrink-0">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={14} />}
              title={feed.error ?? '目标看板暂时读不到数据'}
              action={refreshAction}
            />
          </div>
        ) : goals.length === 0 ? (
          <Panel className="flex-1 min-h-0 flex items-center justify-center">
            <EmptyState
              icon={<ChecklistIcon size={20} />}
              title="还没有目标"
              description="在对话中交代需要持续推进的目标，进度会在这里集中跟进。"
            />
          </Panel>
        ) : visible.length === 0 && hasActiveFilter ? (
          <Panel className="flex-1 min-h-0 flex items-center justify-center">
            <EmptyState
              icon={<SearchIcon size={20} />}
              title="没有匹配的目标"
              description="换个关键词或切换状态筛选再看一次。"
              action={
                <Button variant="outline" size="sm" onClick={clearFilters}>
                  清除筛选
                </Button>
              }
            />
          </Panel>
        ) : (
          <GoalBoard
            goals={visible}
            expanded={expanded}
            onExpand={expandColumn}
            onOpenGoal={openGoal}
            now={Date.now()}
          />
        )}
      </div>

      <GoalDetailDialog
        goal={detailGoal}
        detail={detailFeed.detail}
        loading={detailFeed.loading}
        error={detailFeed.error}
        onClose={() => setDetailGoal(null)}
        // 详情比列表多解析一步全局目标的工作区，因此「能否进会话」以详情为准：
        // 列表卡上没有工作区时，用户在这里仍然拿得到入口。
        onEnterSession={
          detailGoal !== null && hasSessionBinding(detailFeed.detail?.goal ?? detailGoal)
            ? () => enterSession(detailFeed.detail?.goal ?? detailGoal)
            : null
        }
      />
    </PageShell>
  );
};
