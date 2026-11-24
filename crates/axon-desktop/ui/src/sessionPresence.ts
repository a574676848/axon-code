/*
 * 选中会话的存在性：删除与归档都要把主区带回首屏。
 *
 * 侧栏自己的归档动作会立刻交出空选择；这个判据兜住其余通道（命令行、TUI、另一个窗口、
 * 网关侧清理）。判据只看「同一条会话的形态起了变化」：打开一条本来就在归档夹里的会话
 * 属于浏览，不该被弹回首屏，因此只在未归档翻成已归档、或整条从清单里消失时成立。
 */

/** 上一次观察到的选中会话形态。 */
export interface SessionPresence {
  /** 会话标识。 */
  sessionId: string;
  /** 当时是否已归档。 */
  archived: boolean;
}

/**
 * 选中会话该不该退回首屏。
 *
 * `listing` 为 `null` 表示这一条已经不在清单里（被删除，或被清单过滤掉）。还没观察过
 * 这一条（`seen` 为空或指向别的会话）时一律不回落：目标看板、子代理、定时任务的会话
 * 本来就不进侧栏清单，界面不能因为它们「查无此条」就把人弹走。
 */
export function shouldLeaveSession(
  seen: SessionPresence | null,
  sessionId: string,
  listing: { archived: boolean } | null,
): boolean {
  if (seen === null || seen.sessionId !== sessionId) return false;
  if (listing === null) return true;
  return !seen.archived && listing.archived;
}
