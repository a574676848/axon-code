import React from 'react';

/**
 * 输入坞的本轮显式设置：模型与推理强度都只跟随当前会话。
 *
 * 只有用户真在这里改过才随轮下发；换会话即作废（比较车道键，不做额外重置动作，
 * 因此不会多出一帧旧值）。没改过时留空，让运行侧按会话记录的模型与
 * `agent.reasoning_effort` 继续。两者都不写回全局配置：切模型与切强度都是会话内
 * 的动作，全局默认值由设置页维护。
 */
export interface ExplicitTurnSettings {
  /** 本会话显式选定的模型；没选过时为 null。 */
  explicitModel: string | null;
  /** 本会话显式选定的推理强度；没选过时为 null。 */
  explicitReasoningEffort: string | null;
  /** 改选模型：只记下本会话的显式选择，不落盘。 */
  onModelChange: (value: string) => void;
  /** 改选推理强度：只记下本会话的显式选择，不落盘。 */
  onReasoningEffortChange: (value: string) => void;
}

/** 一次显式选择的落点：车道键与两个值同生共死。 */
interface Picked {
  laneKey: string | null;
  model: string | null;
  reasoningEffort: string | null;
}

/** 取同一车道上的既有选择；换车道后整份作废，不做部分继承。 */
function pickedInLane(picked: Picked | null, laneKey: string | null): Picked | null {
  if (picked === null) return null;
  return picked.laneKey === laneKey ? picked : null;
}

export function useExplicitTurnSettings(laneKey: string | null): ExplicitTurnSettings {
  const [picked, setPicked] = React.useState<Picked | null>(null);
  const onModelChange = React.useCallback(
    (value: string) => {
      setPicked((previous) => ({
        laneKey,
        model: value,
        reasoningEffort: pickedInLane(previous, laneKey)?.reasoningEffort ?? null,
      }));
    },
    [laneKey],
  );
  const onReasoningEffortChange = React.useCallback(
    (value: string) => {
      setPicked((previous) => ({
        laneKey,
        model: pickedInLane(previous, laneKey)?.model ?? null,
        reasoningEffort: value,
      }));
    },
    [laneKey],
  );
  const current = pickedInLane(picked, laneKey);
  return {
    explicitModel: current?.model ?? null,
    explicitReasoningEffort: current?.reasoningEffort ?? null,
    onModelChange,
    onReasoningEffortChange,
  };
}
