/*
 * 会话锚点：这条会话真实碰过的文件。
 *
 * 正文里的相对路径只有配上这份锚点才能落到唯一一条绝对路径上，因此锚点必须是
 * 证据而不是猜测：只取工具参数里的路径、改动事件里的路径与用户附件，
 * 三者都由后端判定过。
 */

import type { TurnState } from '../../conversation/transcript/state';
import { buildReferenceIndex, type ReferenceIndex } from '../../resources/referenceIndex';
import { extractFilePath } from './toolPresentation';

/** 参数字符串的扫描上限：超过这个长度的参数必然是大段正文，路径另有来源。 */
const MAX_ARGUMENT_SCAN_CHARS = 8 * 1024;

/** 按 turns 数组建索引：数组引用不变就复用上一次的结论。 */
const indexByTurns = new WeakMap<readonly TurnState[], ReferenceIndex>();

/** 收集这条会话里可以被引用的绝对路径，顺序即已知顺序。 */
function collectAnchors(turns: readonly TurnState[]): string[] {
  const paths: string[] = [];
  for (const turn of turns) {
    for (const cell of turn.cells) {
      if (cell.role === 'tool') {
        if (cell.editPath !== null) paths.push(cell.editPath);
        // 参数摘要可能装着整段正文：超过上限就不再解析，路径早已由改动事件给出。
        if (cell.arguments.length <= MAX_ARGUMENT_SCAN_CHARS) {
          const declared = extractFilePath(cell.arguments);
          if (declared !== null) paths.push(declared);
        }
        continue;
      }
      if (cell.role === 'fileChange') {
        paths.push(cell.path);
        continue;
      }
      if (cell.role === 'user') {
        for (const attachment of cell.attachments) paths.push(attachment.path);
      }
    }
  }
  return paths;
}

/**
 * 这条会话的引用索引。
 *
 * `turns` 是不可变数组：引用一变就说明投影换了一份新的，此时才重建。点击是低频
 * 动作，重建成本落在点击那一帧，渲染路径上没有任何遍历。
 */
export function sessionReferenceIndex(turns: readonly TurnState[]): ReferenceIndex {
  const cached = indexByTurns.get(turns);
  if (cached !== undefined) return cached;
  const built = buildReferenceIndex(collectAnchors(turns));
  indexByTurns.set(turns, built);
  return built;
}
