import React from 'react';

/*
 * 流式光标：2px 竖条，1 秒一次的柔和呼吸。
 * 减弱动效时全局降级规则会把动画压到瞬时，光标自然静止。
 */

interface StreamingCursorProps {
  /** 是否显示。 */
  active?: boolean;
}

export const StreamingCursor: React.FC<StreamingCursorProps> = ({ active = true }) => {
  if (!active) return null;

  return (
    <span
      aria-hidden="true"
      className="inline-block align-text-bottom w-[2px] h-[1.1em] ml-0.5 bg-text-primary animate-[cursor-blink_1s_ease-in-out_infinite]"
    />
  );
};
