/**
 * 会话状态标识的规则。
 *
 * 标识只有两种来历，规则也因此只有两条，但两者极易被混为一谈：
 *
 * - **待答**说明「有事等你做」。窗口在前台也要挂上，而且只由提问结算撤下，
 *   因为它要的是用户的一个动作，不是一次浏览；
 * - **新结果**说明「有东西可以看」。用户正看着这条会话时结果已经在屏幕上，
 *   挂标识只会变成噪音，因此前台不挂、看过即清。
 *
 * 规则从界面里移出来单放，是因为它决定了会话列表行与窗口标题上的前缀图标
 * 什么时候出现、什么时候消失，而这正是最容易各写一半的地方。
 */

/** 宿主推来的一条会话动向；字段取自 `LaneActivity`。 */
export interface LaneNotice {
  /** 终态：completed / failed / interrupted，以及提问的 question_pending 等。 */
  status: string;
  /** 推送时应用是否在前台。 */
  foreground: boolean;
}

/** 待答状态的标识前缀。 */
export const QUESTION_MARK_PREFIX = 'question_';

/** 一条推送要挂上的标识；不挂时返回 null。 */
export function markForNotice(
  notice: LaneNotice,
  viewingLaneId: string | null,
): { laneId: string; status: string } | null {
  if (viewingLaneId === null) return null;
  if (notice.foreground && !notice.status.startsWith(QUESTION_MARK_PREFIX)) return null;
  return { laneId: viewingLaneId, status: notice.status };
}

/**
 * 该撤掉待答标识的车道；不需要撤时返回 null。
 *
 * 待答标识不随「看过即清」消失，只随提问结算消失：`hasPendingQuestion` 为真
 * （还在等回答）或未知时一律保留，免得标识比事实先走一步。
 */
export function clearedAtSettle(
  marks: Readonly<Record<string, string>>,
  laneId: string | null,
  hasPendingQuestion: boolean,
): string | null {
  if (laneId === null || hasPendingQuestion) return null;
  return marks[laneId]?.startsWith(QUESTION_MARK_PREFIX) === true ? laneId : null;
}
