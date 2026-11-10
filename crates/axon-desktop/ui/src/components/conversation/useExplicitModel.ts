import React from 'react';

/**
 * 输入坞的模型选择：显式选择只跟随当前会话。
 *
 * 只有用户真在这里选过才随轮下发；换会话即作废。没选过时必须留空，让运行侧按
 * 会话记录的模型继续，而不是被全局默认值覆盖。
 */
export interface ExplicitModelSelection {
  /** 本会话显式选定的模型；提交时按原样交给后端。 */
  explicitModelRef: React.RefObject<string | null>;
  /** 改选模型：记下这一轮的显式选择，并交给调用方写回配置。 */
  onModelChange: (value: string) => void;
}

export function useExplicitModel(
  laneKey: string | null,
  persistModel: (value: string) => void,
): ExplicitModelSelection {
  const explicitModelRef = React.useRef<string | null>(null);
  // 写回动作每次渲染都可能换引用：放进 ref，避免把回调做成会过期的闭包。
  const persist = React.useRef(persistModel);
  persist.current = persistModel;
  React.useEffect(() => {
    explicitModelRef.current = null;
  }, [laneKey]);
  const onModelChange = React.useCallback((value: string) => {
    explicitModelRef.current = value;
    persist.current(value);
  }, []);
  return { explicitModelRef, onModelChange };
}
