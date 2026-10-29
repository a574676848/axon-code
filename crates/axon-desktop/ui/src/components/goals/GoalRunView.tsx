import React from 'react';

import {
  AlertCircleIcon,
  ChecklistIcon,
  ChevronLeftIcon,
  ClockIcon,
  FolderIcon,
} from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import { executionStatusLabel, goalStatusMeta } from '../../goals/model';
import { useGoalDetail } from '../../goals/useGoalBoard';
import { useConversation } from '../../conversation/useConversation';
import { formatDateTime } from '../pageFormat';
import { ConversationStream } from '../conversation/ConversationStream';
import { Button } from '../ui/controls';
import { Banner, Chip, EmptyState, LoadingState, Panel, StateDot, Text } from '../ui/surfaces';
import { GoalContractBar } from './GoalContractBar';
import { GoalDetailDialog } from './GoalDetailDialog';

/*
 * 目标执行会话：只读视图。
 *
 * 与子代理会话同一条管线：同一个控制器、同一份 durable 事件流、同一个归约器、
 * 同一套渲染，只是订阅目标换成一个固定会话。
 * 顶栏采用标准 40px 原生拖拽头（与左侧品牌行齐平并避让右上角窗口控制键），
 * 下方采用全宽满高纵向工作台布局：顶部为目标概览三列面板，底部为占满余下高度的「执行记录」面板。
 */

export interface GoalRunViewProps {
  goal: GoalCardView;
  /** 返回看板。 */
  onBack: () => void;
  /** 返回看板后要把焦点交回来源卡片。 */
  backRef?: React.RefObject<HTMLButtonElement | null>;
}

/** 执行状态到空态说明的映射；未列出的状态统一按「暂无执行记录」处理。 */
function emptyStateTitle(executionStatus: string | undefined): string {
  if (executionStatus === 'queued') return '正在等待调度执行';
  return '暂无执行记录';
}

export const GoalRunView: React.FC<GoalRunViewProps> = ({ goal, onBack, backRef }) => {
  const [showDetail, setShowDetail] = React.useState(false);
  const detail = useGoalDetail(goal.goalId, true);
  const located = detail.detail?.goal ?? goal;
  const sessionId = located.latestExecution?.sessionId ?? goal.latestExecution?.sessionId ?? null;
  const workspaceId = located.workspaceId ?? goal.workspaceId;
  const target = React.useMemo(
    () =>
      workspaceId === null || sessionId === null
        ? null
        : ({ kind: 'session', workspaceId, sessionId } as const),
    [workspaceId, sessionId],
  );
  const conversation = useConversation(target);
  const meta = goalStatusMeta(located.status);
  const lastActivityAtMs = located.lastActivityAtMs;
  const latestExecution = located.latestExecution ?? goal.latestExecution;
  const executionStatus = latestExecution?.status;
  const failure = conversation.state.failure?.message ?? null;

  // 进入会话视图时焦点落到返回按钮：键盘用户不必再从页面头部摸索。
  React.useEffect(() => {
    backRef?.current?.focus();
  }, [backRef]);

  // 无弹窗打开时按 Esc 直接返回看板，符合原生主从导航习惯。
  React.useEffect(() => {
    if (showDetail) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (document.querySelectorAll('[role="dialog"][aria-modal="true"]').length > 0) return;
      event.preventDefault();
      onBack();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [showDetail, onBack]);

  return (
    <div className="view-swap-in flex-1 min-h-0 flex flex-col bg-canvas overflow-clip">
      <header
        data-tauri-drag-region
        style={{ paddingRight: 'calc(24px + var(--window-controls-inset, 0px))' }}
        className="shrink-0 h-10 pl-6 pr-6 flex items-center gap-2.5 hairline-b border-border-hairline bg-canvas select-none"
      >
        <button
          ref={backRef}
          type="button"
          onClick={onBack}
          className="btn-tactile shrink-0 h-7 pl-1.5 pr-2.5 inline-flex items-center gap-1 rounded-sm text-row font-medium text-text-secondary hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
        >
          <ChevronLeftIcon size={13} />
          返回看板
        </button>

        <span aria-hidden="true" className="h-3.5 w-hairline bg-border-strong shrink-0" />

        <div className="min-w-0 flex-1 flex items-center gap-2">
          <StateDot
            state={meta.dot}
            tone={meta.dot === 'ongoing' ? 'accent' : 'default'}
          />
          <Text size="sm" strong truncate className="min-w-0">
            {located.title}
          </Text>
          <Chip tone={meta.chipTone}>{meta.label}</Chip>
        </div>

        <div className="shrink-0 flex items-center gap-2.5">
          {located.workspaceName !== null && (
            <span className="hidden sm:inline-flex items-center gap-1 text-xs text-text-tertiary">
              <FolderIcon size={12} className="shrink-0 text-text-caption" />
              <span className="truncate max-w-[160px]">{located.workspaceName}</span>
            </span>
          )}
          <Text tone="caption" size="xs" className="hidden md:inline tabular-nums">
            {formatDateTime(lastActivityAtMs)}
          </Text>
          <Button variant="ghost" size="sm" onClick={() => setShowDetail(true)}>
            <ChecklistIcon size={13} />
            详情
          </Button>
        </div>
      </header>

      <div className="flex-1 min-h-0 flex flex-col gap-3.5 px-6 py-4">
        <div className="shrink-0">
          <GoalContractBar
            detail={detail.detail}
            objectivePreview={goal.objectivePreview}
            stepCount={goal.stepCount}
            defaultOpen={conversation.state.turns.length === 0}
            error={detail.error}
          />
        </div>

        {failure !== null && (
          <div className="shrink-0">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={14} />}
              title={failure}
              action={
                <Button variant="outline" size="sm" onClick={onBack}>
                  返回看板
                </Button>
              }
            />
          </div>
        )}

        <Panel clip className="flex-1 min-h-0 flex flex-col">
          <div className="shrink-0 h-9 px-4 flex items-center justify-between gap-3 bg-surface-sunken/30 hairline-b border-border-hairline">
            <Text size="row" strong>
              执行记录
            </Text>
            {latestExecution !== null && (
              <Text tone="tertiary" size="xs" className="tabular-nums">
                {`${executionStatusLabel(latestExecution.status)} · 第 ${latestExecution.attempt} 次 · ${formatDateTime(latestExecution.createdAtMs)}`}
              </Text>
            )}
          </div>

          {conversation.opening ? (
            <LoadingState label="正在读取执行记录" className="flex-1" />
          ) : conversation.state.turns.length === 0 ? (
            <div className="flex-1 min-h-0 flex items-center justify-center px-6">
              <EmptyState
                icon={
                  executionStatus === 'queued' ? (
                    <ClockIcon size={22} />
                  ) : (
                    <ChecklistIcon size={22} />
                  )
                }
                title={emptyStateTitle(executionStatus)}
                description="开始执行后，这里会按时间顺序呈现完整推进过程。"
              />
            </div>
          ) : (
            <ConversationStream
              sessionKey={`goal:${goal.goalId}`}
              turns={conversation.state.turns}
              followToken={conversation.state.turns.length}
              readOnly
            />
          )}
        </Panel>
      </div>

      <GoalDetailDialog
        goal={showDetail ? located : null}
        detail={detail.detail}
        loading={detail.loading}
        error={detail.error}
        onClose={() => setShowDetail(false)}
        onEnterSession={null}
      />
    </div>
  );
};
