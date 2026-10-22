/*
 * 正文舞台：正文区这一刻显示哪一屏、过渡面画什么。
 *
 * 打开一条会话要经过两段异步：读历史索引、逐条订阅回放正文。这段窗口里没有可读的
 * 正文，若直接按投影渲染，屏幕要经历「上一屏 → 空 → 一段段长出来」三步，读起来就是
 * 切换时那一下闪烁。因此正文区在回放期间继续显示上一屏（`StageFrame`），过渡面按
 * 下面的规则出场，回放落地后一次交出。
 *
 * 这一层只做纯决策，节点与动画留在组件里。
 */

import type { TurnState } from './transcript';

/** 已经交出的一屏正文。 */
export interface StageFrame {
  /** 这一屏属于哪条会话；为空表示还没有交出过任何一屏。 */
  key: string | null;
  turns: TurnState[];
}

/** 过渡面这一刻画什么。 */
export type CoverFace = 'hero' | 'skeleton' | 'none';

/**
 * 过渡面决策。
 *
 * - `hero`：没有可看的正文（还没选会话、新建的空会话），或正在打开的这条会话此前
 *   没有正文（从首屏点进一条会话）。此时过渡面留在原处替正文占位，就绪后叠化交班。
 * - `skeleton`：切换发生在两屏正文之间，且打开已经慢过阈值。骨架与正文同几何，
 *   叠化收起时版面不跳。
 * - `none`：不盖任何东西。两屏正文之间切换且快到看不出来时走这一条：上一屏继续
 *   显示到正文就绪为止，不做多余的闪动。
 */
export function coverFace(
  opening: boolean,
  heldHasTurns: boolean,
  slowOpening: boolean,
  hero: boolean,
): CoverFace {
  if (opening) {
    if (!heldHasTurns) return 'hero';
    return slowOpening ? 'skeleton' : 'none';
  }
  return hero ? 'hero' : 'none';
}
