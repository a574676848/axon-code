import React from 'react';

import { AlertCircleIcon, ChecklistIcon, RefreshIcon, SearchIcon } from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import {
  GOAL_COLUMNS,
  countGoals,
  countSummary,
  hasSessionBinding,
  hasUnsettled,
  selectGoals,
  type GoalColumnId,
  type GoalFilter,
} from '../../goals/model';
import { useGoalBoard, useGoalDetail } from '../../goals/useGoalBoard';
import { GOAL_LIST_LIMIT } from '../../goalIpc';
import { Button, Input, SegmentedControl } from '../ui/controls';
import { PageShell } from '../ui/shell';
import { Banner, EmptyState, LoadingState, Text } from '../ui/surfaces';
import { GoalBoard } from './GoalBoard';
import { GoalDetailDialog } from './GoalDetailDialog';
import { GoalRunView } from './GoalRunView';

/*
 * 目标看板页。
 *
 * 两种页面状态共用一层外壳：看板（PageShell 40px 顶栏）与只读执行会话
 * （自带 36px 顶栏）。两态互斥，因此任何时候都只有一条顶栏，不会叠出两行标题。
 *
 * 数据只有一条来源：网关的只读 goals 面。页面不直连数据库，也不缓存第二份事实。
 */

const FILTER_OPTIONS: readonly { value: GoalFilter; label: string }[] = [
  { value: 'all', label: '全部' },
  { value: 'open', label: '未收口' },
  { value: 'settled', label: '已收口' },
];

const PAGE_TITLE = '目标看板';
const PAGE_DESCRIPTION =
  '按状态分栏呈现主 turn 建立的目标。点开卡片进入该目标的只读执行会话，没有可订阅会话的目标先落到详情；后台推进由守护进程负责。';

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
    <PageShell
      title={PAGE_TITLE}
      description={PAGE_DESCRIPTION}
      actions={refreshAction}
    >
      <div className="space-y-3">
        <div className="flex flex-wrap items-center gap-2">
          <div className="min-w-[220px] flex-1">
            <Input
              icon={<SearchIcon size={13} />}
              value={query}
              placeholder="搜索目标、工作区或下一步"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
          <SegmentedControl
            label="按收口状态筛选"
            options={FILTER_OPTIONS}
            value={filter}
            onChange={setFilter}
          />
          <Text tone="tertiary" size="2xs" className="tabular-nums">
            {countSummary(counts, truncated, GOAL_LIST_LIMIT)}
          </Text>
        </div>

        {feed.pollError !== null && (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={feed.pollError}
            action={refreshAction}
          />
        )}

        {feed.loading ? (
          <LoadingState label="正在读取目标" />
        ) : feed.view === null ? (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={feed.error ?? '目标看板暂时读不到数据'}
            action={refreshAction}
          />
        ) : goals.length === 0 ? (          <EmptyState
            icon={<ChecklistIcon size={20} />}
            title="还没有目标"
            description="在本机对话里让 Axon 建立一个跨轮目标后，它会出现在这里。"
          />
        ) : (
          <>
            {!hasUnsettled(goals) && (
              <Text tone="tertiary" size="2xs" className="block">
                全部目标已收口，看板停止自动刷新。
              </Text>
            )}
            <GoalBoard
              goals={visible}
              expanded={expanded}
              onExpand={expandColumn}
              onOpenGoal={openGoal}
              now={Date.now()}
            />
            <Text tone="tertiary" size="2xs" className="block">
              {`看板共 ${GOAL_COLUMNS.length} 栏，每栏最多显示 20 张卡片，其余按需展开。`}
            </Text>
          </>
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
