import React from 'react';

/**
 * 自己接管 Esc 的浮层与字段。
 *
 * 弹窗、菜单与下拉都用 Esc 关闭自己；单行输入框（会话重命名、侧栏搜索）用 Esc
 * 取消编辑。它们打开或聚焦时这一键属于它们，不属于中断本轮。
 * 输入坞本身是多行输入，因此不受这条规则影响：在那里按 Esc 正是要中断。
 */
export function escOwnedByAnotherLayer(target: EventTarget | null): boolean {
  if (document.querySelector('[role="dialog"], [role="menu"], [role="listbox"]') !== null) {
    return true;
  }
  return target instanceof HTMLElement && target.closest('input') !== null;
}

/**
 * 过渡窗口里保持上一屏的取值。
 *
 * 正文、指标条、运行时入口与输入坞的状态行都从会话投影派生，而投影在切换目标的那一帧
 * 就作废了：不冻住这些派生值，界面会先把它们塌成空再长回来，读起来就是一闪一闪。
 * 交接之后逐帧跟上，因此调用方只会看到一次替换。
 */
export function useSettledValue<T>(value: T, settling: boolean): T {
  const held = React.useRef(value);
  React.useEffect(() => {
    if (!settling) held.current = value;
  });
  return settling ? held.current : value;
}
