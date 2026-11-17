import React from 'react';
import type { ComposerPick } from '../../conversation/composerPick';

/**
 * 输入坞的本轮显式设置：模型与推理强度都只跟随当前会话。
 *
 * 只有用户真在这里改过才随轮下发；换会话即作废（比较车道键，不做额外重置动作，
 * 因此不会多出一帧旧值）。没改过时留空，让运行侧按会话记录的模型与
 * `agent.reasoning_effort` 继续。两者都不写回全局配置：切模型与切强度都是会话内
 * 的动作，全局默认值由设置页维护。
 *
 * 新开会话是唯一的例外：那时还没有会话记录可继承，`inherited` 给出上一次在输入坞
 * 选过的模型名称与强度，界面显示与随轮下发的值都取它。历史会话不传 `inherited`，
 * 它自己的权威模型来自该车道最新一轮的运行记录。
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

/** 新会话可继承的选择，以及改选之后要记下来的两个回调。 */
export interface ExplicitTurnSettingsOptions {
  /** 新会话可继承的选择；历史会话与只读会话留空。 */
  inherited?: ComposerPick | null;
  /** 用户改选模型之后记下这一选择（跨会话缓存）。 */
  onRememberModel?: (value: string) => void;
  /** 用户改选推理强度之后记下这一选择（跨会话缓存）。 */
  onRememberReasoningEffort?: (value: string) => void;
}

/** 一次显式选择的落点：车道键与两个值同生共死。 */
interface Picked {
  sessionKey: string | null;
  model: string | null;
  reasoningEffort: string | null;
}

/** 取同一车道上的既有选择；换车道后整份作废，不做部分继承。 */
function pickedInSession(picked: Picked | null, sessionKey: string | null): Picked | null {
  if (picked === null) return null;
  return picked.sessionKey === sessionKey ? picked : null;
}

export function useExplicitTurnSettings(
  sessionKey: string | null,
  options: ExplicitTurnSettingsOptions = {},
): ExplicitTurnSettings {
  const { inherited = null, onRememberModel, onRememberReasoningEffort } = options;
  const [picked, setPicked] = React.useState<Picked | null>(null);
  const onModelChange = React.useCallback(
    (value: string) => {
      setPicked((previous) => ({
        sessionKey,
        model: value,
        reasoningEffort: pickedInSession(previous, sessionKey)?.reasoningEffort ?? null,
      }));
      onRememberModel?.(value);
    },
    [sessionKey, onRememberModel],
  );
  const onReasoningEffortChange = React.useCallback(
    (value: string) => {
      setPicked((previous) => ({
        sessionKey,
        model: pickedInSession(previous, sessionKey)?.model ?? null,
        reasoningEffort: value,
      }));
      onRememberReasoningEffort?.(value);
    },
    [sessionKey, onRememberReasoningEffort],
  );
  const current = pickedInSession(picked, sessionKey);
  /*
   * 新会话的继承值只在该会话第一次出现时落一次。
   *
   * 继承条件（首屏、刚建好还没有内容）在第一条消息发出后就不再成立，而这条会话
   * 自己的权威模型要等订阅把最新一轮记录带回来才到。若只按调用方给的继承值渲染，
   * 输入坞会在这段空档里退回配置默认模型，读起来就是「刚发出去模型就变了」。
   * 因此把继承值记成本会话的选择：此后即使不再给继承值，显示的与下发的仍是同一个。
   * 两个值都为空时不落：配置与缓存都还没读回来，落了就把这条路堵死。
   */
  if (
    inherited !== null &&
    (inherited.model !== null || inherited.reasoningEffort !== null) &&
    current === null
  ) {
    setPicked({
      sessionKey,
      model: inherited.model,
      reasoningEffort: inherited.reasoningEffort,
    });
  }
  return {
    explicitModel: current?.model ?? inherited?.model ?? null,
    explicitReasoningEffort: current?.reasoningEffort ?? inherited?.reasoningEffort ?? null,
    onModelChange,
    onReasoningEffortChange,
  };
}
