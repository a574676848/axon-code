/**
 * 桌面宿主下发的会话事件：新结果提示与「回到某条会话」。
 *
 * 这两条事件只由桌面宿主推：浏览器调试桥没有窗口状态，也就没有「前台用标识、
 * 托盘才通知」的区分，因此不在桥里复刻，一律返回空订阅。
 *
 * 通知点击没有直接回调：宿主在窗口重新获得焦点时下发目标车道，界面据此切会话。
 */

import { isDesktopHost } from './ipc';

/** 会话有新结果的推送事件名，与 Rust 侧 `LANE_ACTIVITY_EVENT` 一致。 */
const LANE_ACTIVITY_EVENT = 'axon://lane-activity';

/** 打开某条会话的推送事件名，与 Rust 侧 `LANE_OPEN_EVENT` 一致。 */
const LANE_OPEN_EVENT = 'axon://open-lane';

/** 一条「会话有新结果」的提示。 */
export interface LaneActivity {
  /** 轮次标识，用于去重。 */
  id: string;
  /** 终态：completed / failed / interrupted，以及提问的 question_pending 等。 */
  status: string;
  /** 提示标题。 */
  title: string;
  /** 推送时应用是否在前台；待答标识与前台的「有新结果」标识据此分流。 */
  foreground: boolean;
}

/** 桌面宿主专属的事件订阅；非桌面宿主返回空订阅。 */
async function listenDesktopEvent<T>(
  eventName: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  if (!isDesktopHost()) return () => undefined;
  const { listen } = await import('@tauri-apps/api/event');
  return listen<T>(eventName, (event) => handler(event.payload));
}

/** 订阅「会话有新结果」。 */
export async function listenLaneActivity(
  handler: (activity: LaneActivity) => void,
): Promise<() => void> {
  return listenDesktopEvent<LaneActivity>(LANE_ACTIVITY_EVENT, handler);
}

/** 订阅「回到某条会话」。 */
export async function listenOpenLane(handler: (laneId: string) => void): Promise<() => void> {
  return listenDesktopEvent<{ laneId: string }>(LANE_OPEN_EVENT, (payload) =>
    handler(payload.laneId),
  );
}
