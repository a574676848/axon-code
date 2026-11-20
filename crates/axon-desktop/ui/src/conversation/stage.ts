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

/** 首屏判定的输入。 */
export interface HeroInput {
  /** 还没有选中任何会话。 */
  noSelection: boolean;
  /** 选中的会话是否只是应用内刚建好、还没有服务端内容的那一条。 */
  pending: boolean;
  /** 当前投影里的正文轮数。 */
  turns: number;
  /** 过渡窗口还没关闭。 */
  opening: boolean;
  /** 正文还在装载。 */
  loadingHistory: boolean;
  /** 这条会话的历史索引已经交接过一次。 */
  ready: boolean;
}

/**
 * 这一屏该不该画首屏。
 *
 * 三种情形：还没选会话；选中的是应用内刚建好、还没有内容的那一条；以及**读完了仍然
 * 一条正文都没有**。最后一种此前不算首屏，投影为空时又恰好返回 `none`，于是整块正文区
 * 就是一片空白：没有字，也没有人说发生了什么。首屏至少还是一句可读的话，
 * 而「打开失败」这类结论另有正文上方的错误卡承担。
 */
export function showsHero(input: HeroInput): boolean {
  if (input.noSelection) return true;
  if (input.turns > 0) return false;
  if (input.pending) return true;
  return input.ready && !input.opening && !input.loadingHistory;
}

/**
 * 过渡面决策。
 *
 * - `hero`：没有可看的正文（还没选会话，或这一条读完了也没有正文）。过渡面留在原处
 *   替正文占位，就绪后叠化交班；它就是「这一屏本来就没有内容」的那个形态。
 * - `skeleton`：切换已经慢过阈值。骨架与正文同几何，叠化收起时版面不跳。
 * - `none`：不盖任何东西：切换快到看不出来时，上一屏继续显示到正文就绪为止。
 *
 * **只有「切换已经结束」才允许 `none`**。打开过程中返回 `none` 时，上一屏可能正是
 * 一块空画布（首屏刚点进一条有内容的会话），于是正文区会一直空白到回放落地：
 * 一篇长会话要读好几段索引，那段空白不是「快到看不出来」，而是好几秒的白屏。
 */
export function coverFace(opening: boolean, slowOpening: boolean, hero: boolean): CoverFace {
  if (hero) return 'hero';
  if (!opening) return 'none';
  return slowOpening ? 'skeleton' : 'none';
}
