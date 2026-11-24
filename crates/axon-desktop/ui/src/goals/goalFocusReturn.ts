/*
 * 目标卡与执行会话之间的焦点交接。
 *
 * 看板 → 执行会话 → 回到看板，这是同一个动作的来回。整页重新挂载之后浏览器只会把
 * 焦点丢到 body，键盘用户要从头 Tab 一遍才能回到刚才那张卡，而那张卡正是他要接着
 * 处理的东西。因此进入会话时把来源卡记下来，看板重新挂载后按它复原一次。
 *
 * 只记一枚标识：焦点只有一个去处，记一串既没有意义，也会在多次往返后错位。
 */

/** 待复原的来源目标；为空表示这一次不是从卡片进来的。 */
let pending: string | null = null;

/** 进入执行会话之前记下来源卡。 */
export function rememberGoalFocus(goalId: string): void {
  pending = goalId.length > 0 ? goalId : null;
}

/** 取走待复原的目标标识；取一次即清空。 */
export function takeGoalFocus(): string | null {
  const value = pending;
  pending = null;
  return value;
}

/** 清空待复原项；只给测试与复位用。 */
export function clearGoalFocus(): void {
  pending = null;
}
