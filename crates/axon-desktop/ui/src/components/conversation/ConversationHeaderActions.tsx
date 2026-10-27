import React from 'react';
import { IconButton } from '../ui/controls';
import { PanelRightIcon } from '../../icons';
import { RuntimeActions } from './RuntimeActions';
import { VoiceLaneToggle } from './VoiceLaneToggle';
import type { JobRow, SubagentRow } from '../../conversation/transcript';

/*
 * 指标条右侧的操作区。
 *
 * 三件东西按「会话自己的偏好、随会话走的过程事实、窗口的停靠面」排开：
 * 朗读开关常驻，运行时事实只在真的存在子代理或后台任务时出现，
 * 右侧栏入口只在收起态出现，展开后收起动作归面板自己的 tab 条末端。
 */

export const ConversationHeaderActions: React.FC<{
  subagents: SubagentRow[];
  jobs: JobRow[];
  /** 当前会话车道；停止作业时作为归属参数。 */
  laneId: string | null;
  /** 当前会话的偏好键；为空表示还没有选中会话。 */
  laneKey: string | null;
  onOpenSubagent: (agentId: string) => void;
  railExpanded: boolean;
  onToggleRail: () => void;
}> = ({ subagents, jobs, laneId, laneKey, onOpenSubagent, railExpanded, onToggleRail }) => (
  <>
    <VoiceLaneToggle laneKey={laneKey} />
    <RuntimeActions
      subagents={subagents}
      jobs={jobs}
      laneId={laneId}
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
