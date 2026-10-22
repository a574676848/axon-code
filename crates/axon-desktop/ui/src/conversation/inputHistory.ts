/*
 * 输入历史导航：与 TUI 的 `history_previous` / `history_next` 同一套语义。
 *
 * 三条规则：
 * 1. 第一次按上键时先把当前草稿记下来（锚点），之后往更早的历史走；
 * 2. 走到最早一条后停住，不再往前；
 * 3. 越过最新一条时回到当初的草稿，草稿本身不会被历史覆盖掉。
 */

/** 导航状态：当前停在第几条，以及开始导航前的草稿。 */
export interface HistoryNavigation {
  /** 当前停在历史里的下标；为空表示没在导航（正在写自己的草稿）。 */
  index: number | null;
  /** 开始导航前的草稿；退出导航时原样还回去。 */
  draft: string | null;
}

/** 未开始导航的空状态。 */
export const IDLE_HISTORY: HistoryNavigation = { index: null, draft: null };

/** 上一条历史；没有历史时返回 null。 */
export function historyPrevious(
  entries: readonly string[],
  navigation: HistoryNavigation,
  draft: string,
): { navigation: HistoryNavigation; text: string } | null {
  if (entries.length === 0) return null;
  const index =
    navigation.index === null ? entries.length - 1 : Math.max(0, navigation.index - 1);
  return {
    navigation: {
      index,
      draft: navigation.index === null ? draft : navigation.draft,
    },
    text: entries[index],
  };
}

/**
 * 下一条历史。
 *
 * 已经在导航时才有意义；越过最新一条即退出导航并把草稿还回去。
 */
export function historyNext(
  entries: readonly string[],
  navigation: HistoryNavigation,
): { navigation: HistoryNavigation; text: string } | null {
  if (navigation.index === null) return null;
  if (navigation.index + 1 >= entries.length) {
    return { navigation: IDLE_HISTORY, text: navigation.draft ?? '' };
  }
  return {
    navigation: { index: navigation.index + 1, draft: navigation.draft },
    text: entries[navigation.index + 1],
  };
}

/**
 * 光标是否在草稿的第一行。
 *
 * 与 TUI 一致：只有第一行的上键、最后一行的下键才用于翻历史，
 * 多行草稿里的方向键仍然老老实实移动光标。
 */
export function caretOnFirstLine(text: string, caret: number): boolean {
  return text.lastIndexOf('\n', Math.max(0, caret - 1)) < 0;
}

/** 光标是否在草稿的最后一行。 */
export function caretOnLastLine(text: string, caret: number): boolean {
  return text.indexOf('\n', caret) < 0;
}
