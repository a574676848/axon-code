/*
 * 车道接管判定：外部发起的新一轮要能被已经打开的会话视图接住。
 *
 * 会话事件流按 Interaction 订阅，因此**不属于本视图提交的那一轮**（后台作业跑完
 * 由 daemon 唤醒出来的新一轮、定时任务投递的一轮）不会被自动取到：正文停在上一轮，
 * 状态位与标题却已经按车道的真实状态在走，读起来就是「活已经结束，界面还在跑」。
 *
 * 这里只做一件事：把「车位快照里有多少轮」与「本视图已经接管到哪一轮」对齐，
 * 并在真的多出新一轮时要求重新接管一次。规则全部收在这个纯函数里，界面与
 * 控制器只消费结论，因此可以被单测逐条钉住。
 */

/** 接管游标：一条车道已经接管到第几轮。 */
export interface LaneSyncCursor {
  /** 游标所属的车道；为空表示还没接管过任何车道。 */
  laneId: string | null;
  /** 已经接管到的 Interaction 总数。 */
  interactionCount: number;
}

/** 空游标：还没有接管过任何车道。 */
export const EMPTY_LANE_SYNC_CURSOR: LaneSyncCursor = { laneId: null, interactionCount: 0 };

/** 一次对齐的结论：游标推进到哪，以及是否要重新接管。 */
export interface LaneSyncStep {
  /** 推进之后的游标。 */
  cursor: LaneSyncCursor;
  /** 是否需要重新接管一次（取最新一页并订阅那一轮）。 */
  reattach: boolean;
}

/**
 * 按车道快照推进接管游标。
 *
 * 三条规则，各自对应一种真实情形：
 *
 * - **车道换了**：游标清零重记，接管由目标切换本身的首次加载完成，这里不重复触发；
 * - **轮数没有增长**：没有新的一轮，不动；
 * - **轮数增长且本视图没有在跑的轮次**：这一轮不是本视图提交的，必须重新接管。
 *   反过来，本视图正在跑的时候增长出来的一轮就是自己刚提交的那条，已经订阅在
 *   手上，重新接管只会把正在流式写入的这一轮再回放一遍。
 *
 * 增长而本视图在跑时仍然推进游标：否则自己这一轮收口之后会被当成「外部新轮」
 * 再接管一次，白白回放一遍刚写完的正文。
 */
export function advanceLaneSync(
  cursor: LaneSyncCursor,
  laneId: string,
  interactionCount: number,
  working: boolean,
): LaneSyncStep {
  const trimmed = laneId.trim();
  // 车道标识为空或计数不成立时不推进：这两者是「还不知道」而不是「没有变化」。
  if (trimmed.length === 0 || !Number.isFinite(interactionCount) || interactionCount < 0) {
    return { cursor, reattach: false };
  }
  if (cursor.laneId !== trimmed) {
    return { cursor: { laneId: trimmed, interactionCount }, reattach: false };
  }
  if (interactionCount <= cursor.interactionCount) {
    return { cursor, reattach: false };
  }
  return {
    cursor: { laneId: trimmed, interactionCount },
    reattach: !working,
  };
}
