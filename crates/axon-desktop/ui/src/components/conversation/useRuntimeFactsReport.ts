/*
 * 会话运行态事实的上报。
 *
 * 外壳（侧栏、窗口标题、通知）只关心四件事：这条会话在不在跑、还有几个子代理与后台
 * 作业、是不是在等用户回答，以及「停止」这个动作本身。四处事实都从同一份会话投影算出，
 * 因此收在一处上报，不让每个消费方各算一遍。
 *
 * 与渲染解耦：本模块只做取数与上报，不产出任何节点。`stop` 经 ref 取当前控制器：
 * 回调每轮换引用会让上报 effect 自激，那会把一次渲染放大成一串。
 */

import React from 'react';

import {
  activeJobCount,
  isActiveSubagent,
  type SessionRuntimeFacts,
} from '../../conversation/runtimeFacts';
import type { ConversationController, SessionSelection } from '../../conversation/useConversation';
import type { SubagentRow } from '../../conversation/transcript';
import { useLiveJobIds } from './useLiveJobIds';
import { useSessionGoals } from './useSessionGoals';

export interface RuntimeFactsOptions {
  conversation: ConversationController;
  selection: SessionSelection | null;
  /** 选中另一条会话（子代理入口用它跳到它的运行轨迹）。 */
  onSelectSession: (next: SessionSelection) => void;
  /** 上报入口；没有消费方时整段不做。 */
  onRuntimeFacts?: (facts: SessionRuntimeFacts) => void;
}

export interface RuntimeFacts {
  /** 打开某个子代理的运行轨迹（把它的会话当成一条会话选中）。 */
  openSubagentSession: (row: SubagentRow) => void;
  /** 本会话创建的目标清单。 */
  sessionGoals: ReturnType<typeof useSessionGoals>;
}

export function useRuntimeFactsReport({
  conversation,
  selection,
  onSelectSession,
  onRuntimeFacts,
}: RuntimeFactsOptions): RuntimeFacts {
  /*
   * 子代理与主会话同一条路径：直接把它当成一条会话选中，历史索引、分页、过程折叠与
   * 输入坞都复用同一套实现。差别只在输入坞的控制（模型与思考强度只读），不再另立视图。
   */
  const openSubagentSession = React.useCallback(
    (row: SubagentRow) => {
      const sessionId = (row.sessionId ?? '').trim();
      if (sessionId.length === 0 || selection === null) return;
      onSelectSession({
        ...selection,
        sessionId,
        destination: '',
        title: row.label,
        channel: 'agent',
        backTo: selection,
        fixedModel: null,
        fixedReasoningEffort: null,
      });
    },
    [onSelectSession, selection],
  );

  /** 停止本轮：控制器经 ref 取，避免回调每轮换引用而让上报 effect 自激。 */
  const interruptRef = React.useRef(conversation.interrupt);
  interruptRef.current = conversation.interrupt;
  const stopRunning = React.useCallback(async () => {
    await interruptRef.current();
  }, []);

  const activeSubagents = conversation.state.subagents.filter((row) =>
    isActiveSubagent(row.status),
  ).length;
  const selectedSessionId = selection?.sessionId ?? null;
  const liveJobIds = useLiveJobIds(selectedSessionId);
  const activeJobs = activeJobCount(conversation.jobs, liveJobIds);
  /** 本会话创建的目标清单；点其中的一行进入它的执行会话。 */
  const sessionGoals = useSessionGoals(selectedSessionId, selection?.workspaceId ?? null);

  React.useEffect(() => {
    onRuntimeFacts?.({
      sessionId: selectedSessionId,
      turnRunning: conversation.working,
      activeSubagents,
      activeJobs,
      // 待答标识的清除条件：这条会话已经没有在等用户回答的提问了。
      hasPendingQuestion: conversation.activeQuestion !== null,
      stop: stopRunning,
    });
  }, [
    activeJobs,
    activeSubagents,
    conversation.activeQuestion,
    conversation.working,
    onRuntimeFacts,
    selectedSessionId,
    stopRunning,
  ]);

  return { openSubagentSession, sessionGoals };
}
