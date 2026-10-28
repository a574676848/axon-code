import React from 'react';

import { AlertCircleIcon, ChevronRightIcon } from '../../icons';
import type { GoalCardView } from '../../goalTypes';
import { goalStatusMeta } from '../../goals/model';
import { useGoalDetail } from '../../goals/useGoalBoard';
import { useConversation } from '../../conversation/useConversation';
import { formatDateTime } from '../pageFormat';
import { ConversationStream } from '../conversation/ConversationStream';
import { GoalContractBar } from './GoalContractBar';
import { Banner, EmptyState, LoadingState, StateDot, Text } from '../ui/surfaces';

/*
 * 目标执行会话：只读视图。
 *
 * 与子代理会话同一条管线：同一个控制器、同一份 durable 事件流、同一个归约器、
 * 同一套渲染，只是订阅目标换成一个固定会话。**不渲染输入坞**：后台执行由守护进程
 * 推进，界面在这里只负责看，因此不提供启停与追发消息（合同第 2 条裁决）。
 */

export interface GoalRunViewProps {
  goal: GoalCardView;
  /** 返回看板。 */
  onBack: () => void;
  /** 返回看板后要把焦点交回来源卡片。 */
  backRef?: React.RefObject<HTMLButtonElement | null>;
}

/** 执行状态到空态说明的映射；未列出的状态统一按「还没有执行事件」处理。 */
function emptyStateTitle(executionStatus: string | undefined): string {
  if (executionStatus === 'queued') return '排队中，等待守护进程领取';
  return '还没有执行事件';
}

export const GoalRunView: React.FC<GoalRunViewProps> = ({ goal, onBack, backRef }) => {
  const detail = useGoalDetail(goal.goalId, true);
  const sessionId = goal.latestExecution?.sessionId ?? null;
  const workspaceId = goal.workspaceId;
  const target = React.useMemo(
    () =>
      workspaceId === null || sessionId === null
        ? null
        : ({ kind: 'session', workspaceId, sessionId } as const),
    [workspaceId, sessionId],
  );
  const conversation = useConversation(target);
  const meta = goalStatusMeta(detail.detail?.goal.status ?? goal.status);
  const lastActivityAtMs = detail.detail?.goal.lastActivityAtMs ?? goal.lastActivityAtMs;
  const executionStatus = detail.detail?.goal.latestExecution?.status ??
    goal.latestExecution?.status;
  const failure = conversation.state.failure?.message ?? null;

  // 进入会话视图时焦点落到返回按钮：键盘用户不必再从页面头部摸索。
  React.useEffect(() => {
    backRef?.current?.focus();
  }, [backRef]);

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <div className="shrink-0 h-9 px-4 flex items-center gap-2 hairline-b border-border-hairline">
        <button
          ref={backRef}
          type="button"
          onClick={onBack}
          className="shrink-0 h-6 pl-1 pr-2 inline-flex items-center gap-0.5 rounded-xs text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
        >
          <ChevronRightIcon size={12} className="rotate-180" />
          返回看板
        </button>
        <Text tone="tertiary" size="meta" className="shrink-0">
          ·
        </Text>
        <span className="min-w-0 flex items-center gap-1.5">
          <StateDot state={meta.dot} />
          <span className="min-w-0 truncate text-row text-text-primary">{goal.title}</span>
        </span>
        <span className="shrink-0 inline-flex items-center gap-1.5 ml-1">
          <Text tone="secondary" size="xs" className="shrink-0">
            {meta.label}
          </Text>
          <Text tone="tertiary" size="meta" className="shrink-0">
            ·
          </Text>
          <Text tone="tertiary" size="meta" mono className="shrink-0 tabular-nums">
            {formatDateTime(lastActivityAtMs)}
          </Text>
        </span>
      </div>

      <div className="shrink-0 px-4 pt-1.5">
        <Text tone="tertiary" size="2xs" className="block">
          后台执行由守护进程推进，此视图只读
        </Text>
      </div>

      <div className="shrink-0 px-4 pt-1">
        <GoalContractBar
          detail={detail.detail}
          objectivePreview={goal.objectivePreview}
          stepCount={goal.stepCount}
          error={detail.error}
        />
      </div>

      {failure !== null ? (
        <div className="flex-1 min-h-0 overflow-y-auto px-4 py-3">
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={failure}
            action={
              <button
                type="button"
                onClick={onBack}
                className="h-7 px-2.5 rounded-sm text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
              >
                返回看板
              </button>
            }
          />
        </div>
      ) : conversation.opening ? (
        <LoadingState label="正在读取执行逐字稿" />
      ) : conversation.state.turns.length === 0 ? (
        <EmptyState
          title={emptyStateTitle(executionStatus)}
          description="目标执行事件一旦产生，这里会按时间线呈现它的工具调用与回复。"
        />
      ) : (
        <ConversationStream
          sessionKey={`goal:${goal.goalId}`}
          turns={conversation.state.turns}
          followToken={conversation.state.turns.length}
        />
      )}
    </div>
  );
};
