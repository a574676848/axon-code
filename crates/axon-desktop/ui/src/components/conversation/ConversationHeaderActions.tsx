import React from 'react';
import { IconButton } from '../ui/controls';
import { PanelRightIcon } from '../../icons';
import { RuntimeActions } from './RuntimeActions';
import { GoalActions } from './GoalActions';
import type { JobRow, SubagentRow } from '../../conversation/transcript';
import type { GoalCardView } from '../../goalTypes';

/*
 * 指标条右侧的操作区。
 *
 * 三件东西按「随会话走的过程事实、会话目标、窗口的停靠面」排开：
 * 运行时事实只在真的存在子代理或后台任务时出现，右侧栏入口只在收起态出现，
 * 展开后收起动作归面板自己的 tab 条末端。
 *
 * 旧的「会话级自动朗读开关」已随文本朗读能力一并移除：语音只剩输入坞里的
 * realtime 对话一路，会话级再挂一个朗读开关等于给同一条能力开第二个口子。
 */

export const ConversationHeaderActions: React.FC<{
  subagents: SubagentRow[];
  jobs: JobRow[];
  /** 本会话创建的目标；为空时目标入口整枚收起。 */
  goals: GoalCardView[];
  /** 点目标行进入它的执行会话。 */
  onOpenGoal: (goal: GoalCardView) => void;
  /** 当前会话车道；停止作业时作为归属参数。 */
  sessionId: string | null;
  /** 本视图是否有正在跑的轮次：子代理名册只在这段窗口里按间隔对账。 */
  working: boolean;
  onOpenSubagent: (row: import('../../conversation/transcript').SubagentRow) => void;
  railExpanded: boolean;
  onToggleRail: () => void;
}> = ({
  subagents,
  jobs,
  goals,
  onOpenGoal,
  sessionId,
  working,
  onOpenSubagent,
  railExpanded,
  onToggleRail,
}) => (
  <>
    <GoalActions goals={goals} onOpenGoal={onOpenGoal} />
    <RuntimeActions
      subagents={subagents}
      jobs={jobs}
      sessionId={sessionId}
      working={working}
      onOpenSubagent={onOpenSubagent}
    />
    {/*
     * 入口只画在收起态：展开后收起动作归面板自己的 tab 条末端，
     * 同一件事不在两处各摆一个开关，与既有工作台产品一致。
     * 收起态才存在的东西点完就没了，因此要留一个标记给面板的焦点续接。
     */}
    {!railExpanded && (
      <IconButton label="展开资源面板" data-rail-expand onClick={onToggleRail}>
        <PanelRightIcon size={16} />
      </IconButton>
    )}
  </>
);
