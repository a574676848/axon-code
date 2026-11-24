/*
 * 输入坞的撤销与恢复历史。
 *
 * 粒度只由「一次真实编辑」决定，不做时间窗或长度启发式合并：
 * 逐字键入时浏览器每个字符派发一次变更，于是每个字符各入栈一次，撤销逐字回退；
 * 粘贴时浏览器只派发一次变更，于是整段粘贴只入栈一次，撤销整段回退；
 * 输入法组合期间不记录（拼音中间态不是用户的编辑意图），组合确认时按整个词组入栈一次。
 *
 * 三条要求因此由同一份模型自然得出，而不是三套互相打架的特例。
 */

/** 撤销栈深度上限。 */
export const COMPOSER_HISTORY_LIMIT = 200;

/** 编辑历史：past 与 future 的末尾各自是最新的一次。 */
export interface ComposerHistory {
  /** 每次编辑之前的正文，末尾是最新一次。 */
  past: string[];
  /** 被撤销掉的正文，末尾是最新一次；发生新编辑即清空。 */
  future: string[];
}

/** 空历史。 */
export const EMPTY_COMPOSER_HISTORY: ComposerHistory = { past: [], future: [] };

/**
 * 记录一次编辑。
 *
 * 压入的是编辑之前的正文，因此撤销就是「回到编辑前」；
 * 同一次新编辑会丢弃重做分支，这与所有编辑器的重做语义一致。
 */
export function recordEdit(state: ComposerHistory, before: string): ComposerHistory {
  const past = [...state.past, before];
  return {
    past:
      past.length > COMPOSER_HISTORY_LIMIT
        ? past.slice(past.length - COMPOSER_HISTORY_LIMIT)
        : past,
    future: [],
  };
}

/** 是否有可撤销的编辑。 */
export function canUndo(state: ComposerHistory): boolean {
  return state.past.length > 0;
}

/** 是否有可恢复的编辑。 */
export function canRedo(state: ComposerHistory): boolean {
  return state.future.length > 0;
}

/** 撤销一步；没有可撤销的编辑时返回 null。 */
export function undo(
  state: ComposerHistory,
  current: string,
): { history: ComposerHistory; text: string } | null {
  const text = state.past[state.past.length - 1];
  if (text === undefined) return null;
  return {
    history: { past: state.past.slice(0, -1), future: [...state.future, current] },
    text,
  };
}

/** 恢复一步；没有可恢复的编辑时返回 null。 */
export function redo(
  state: ComposerHistory,
  current: string,
): { history: ComposerHistory; text: string } | null {
  const text = state.future[state.future.length - 1];
  if (text === undefined) return null;
  return {
    history: { past: [...state.past, current], future: state.future.slice(0, -1) },
    text,
  };
}
