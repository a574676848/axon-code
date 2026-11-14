/*
 * 正文引用的解析。
 *
 * 一段正文里的路径不是「路径值」而是「引用」：它相对谁，字面上没有答案。
 * 这里按确定性顺序给结论，并把「拿不准」如实标出来，绝不静默取一个近似解。
 *
 * 顺序（前一步有结论就不再往下走）：
 * 1. 绝对路径、UNC、`~`：原样交给后端，它才是路径规则的权威。
 * 2. 会话索引里唯一命中：换成那条绝对路径。索引来自「真的被碰过的文件」，
 *    这份证据强于任何默认基准。
 * 3. 多命中：不猜。退回既有语义，同时把候选带出去，供上层以后做选择。
 * 4. 没命中：退回既有语义（相对工作区），行为与改动前逐字一致。
 */

import { MAX_CANDIDATES, isAbsoluteReference, type ReferenceIndex } from './referenceIndex';

/** 引用的解析结论。 */
export interface ReferenceResolution {
  /**
   * `direct` 原样打开，`resolved` 换成绝对路径，
   * `ambiguous` 与 `workspace` 沿用原文，交给既有语义处理。
   */
  status: 'direct' | 'resolved' | 'ambiguous' | 'workspace';
  /** 应当交给打开动作的路径；除 `resolved` 外都是引用原文。 */
  path: string;
  /** 歧义时的候选绝对路径；其余情况为空。 */
  candidates: readonly string[];
}

/**
 * 解析一段正文引用。
 *
 * @param reference 正文里识别出的那段路径文本，未经任何加工。
 * @param index 这条会话的已知路径索引；没有会话上下文时传 null。
 */
export function resolveReference(
  reference: string,
  index: ReferenceIndex | null,
): ReferenceResolution {
  const text = reference.trim();
  if (text === '') return { status: 'workspace', path: reference, candidates: [] };
  if (isAbsoluteReference(text)) return { status: 'direct', path: text, candidates: [] };
  const candidates = index === null ? [] : index.match(text);
  const only = candidates[0];
  if (candidates.length === 1 && only !== undefined) {
    return { status: 'resolved', path: only.absolute, candidates: [] };
  }
  if (candidates.length > 1) {
    return {
      status: 'ambiguous',
      path: text,
      candidates: candidates.slice(0, MAX_CANDIDATES).map((entry) => entry.absolute),
    };
  }
  return { status: 'workspace', path: text, candidates: [] };
}
