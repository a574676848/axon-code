import React from 'react';

/**
 * 输入坞的模型选择：显式选择只跟随当前会话。
 *
 * 只有用户真在这里选过才随轮下发；换会话即作废（比较车道键，不做额外重置动作，
 * 因此不会多出一帧旧值）。没选过时留空，让运行侧按会话记录的模型继续。
 */
export interface ExplicitModelSelection {
  /** 本会话显式选定的模型；没选过时为 null。 */
  explicitModel: string | null;
  /** 改选模型：记下这一轮的显式选择，并交给调用方写回配置。 */
  onModelChange: (value: string) => void;
}

export function useExplicitModel(
  laneKey: string | null,
  persistModel: (value: string) => void,
): ExplicitModelSelection {
  const [picked, setPicked] = React.useState<{ laneKey: string | null; model: string } | null>(
    null,
  );
  // 写回动作每次渲染都可能换引用：放进 ref，避免把回调做成会过期的闭包。
  const persist = React.useRef(persistModel);
  persist.current = persistModel;
  const onModelChange = React.useCallback(
    (value: string) => {
      setPicked({ laneKey, model: value });
      persist.current(value);
    },
    [laneKey],
  );
  const explicitModel = picked !== null && picked.laneKey === laneKey ? picked.model : null;
  return { explicitModel, onModelChange };
}
