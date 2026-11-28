import React from 'react';
import { IconButton } from '../ui/controls';
import { PanelRightIcon } from '../../icons';
import { RuntimeActions } from './RuntimeActions';
import { GoalActions } from './GoalActions';
import { VoiceSessionToggle } from './VoiceSessionToggle';
import type { JobRow, SubagentRow } from '../../conversation/transcript';
import type { GoalCardView } from '../../goalTypes';

/*
 * 指标条右侧的操作区。
 *
 * 三件东西按「会话自己的偏好、随会话走的过程事实、窗口的停靠面」排开：
 * 朗读开关常驻（语音入口隐藏时整枚收起），运行时事实只在真的存在子代理或后台
 * 任务时出现，右侧栏入口只在收起态出现，展开后收起动作归面板自己的 tab 条末端。
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
  /** 当前会话的偏好键；为空表示还没有选中会话。 */
  sessionKey: string | null;
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
  sessionKey,
  onOpenSubagent,
  railExpanded,
  onToggleRail,
}) => (
  <>
    <VoiceSessionToggle sessionKey={sessionKey} />
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
