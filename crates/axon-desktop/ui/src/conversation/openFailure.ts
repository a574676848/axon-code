/*
 * 打开会话失败的判定。
 *
 * 打开一条会话要读两段：历史索引与逐条回放。两段都可能失败，失败结论在控制器里
 * 与「提交失败」共用同一个字段（`submitError`），而那个字段只是一行到点退场的回执：
 * 6 秒之后屏幕上只剩下一片空白，用户不知道发生了什么，也没有地方可以点。
 *
 * 这里把「这一屏根本没读回来」从那个字段里认出来，交给正文上方那张错误卡承担；
 * 卡不自己消失，重试入口就在卡上。判据只有一条：**一条正文都没有，且没有任何装载
 * 在途**。提交失败不在其中，因为那时本地轮次已经立起来了。
 */

/** 判定一次打开失败的输入。 */
export interface OpenFailureInput {
  /** 控制器当前的失败回执。 */
  error: string | null;
  /** 正文还在装载（打开会话、历史回放未落地）。 */
  loadingHistory: boolean;
  /** 过渡窗口还没关闭。 */
  opening: boolean;
  /** 当前投影里是否已经有正文。 */
  hasTurns: boolean;
}

/**
 * 下一次该不该把这张卡亮出来。
 *
 * - 已经有正文：这一屏读回来了，卡随之撤下。
 * - 装载还在途：保持上一份结论，不在中途把卡收掉再长回来。
 * - 有一个失败回执：认它，并且记住：回执 6 秒后会被清掉，卡不跟着消失。
 */
export function nextOpenFailure(
  previous: string | null,
  input: OpenFailureInput,
): string | null {
  if (input.hasTurns) return null;
  if (input.loadingHistory || input.opening) return previous;
  return input.error ?? previous;
}
