/*
 * 输入坞候选菜单的纯规则：`/` 指令 token 的判定与候选排序。
 *
 * 两者都只依赖输入本身（正文加光标、候选加查询词），因此不放进组件：
 * 纯函数可以被单测钉死，组件只剩摆位与取数。
 *
 * 与 TUI 的差别只有一处：终端只认正文开头那一条斜杠，桌面端按词的边界认，
 * 因此「先写一句话，再补一条指令」在桌面端同样能弹出候选。
 */

/** 正文里一个活跃的 `/` 指令 token。 */
export interface SlashToken {
  /** `/` 在正文中的下标。 */
  start: number;
  /** 光标位置，也就是 token 的结束下标。 */
  end: number;
  /** `/` 与光标之间的文本，也就是这一次的查询词。 */
  query: string;
}

/** 参与排序的候选：`name` 是唯一匹配键。 */
export interface RankableCandidate {
  name: string;
}

/** 词字符：紧跟其后的斜杠属于普通文本，不是指令。 */
const WORD_CHAR = /[\p{L}\p{N}_]/u;
const WHITESPACE = /\s/u;

/**
 * 从光标位置向前找活跃的 `/` 指令。
 *
 * 一条斜杠只在这一类边界上生效：正文开头、空白之后、或标点之后。紧跟词字符的斜杠
 * 属于普通文本（`看/看`、路径 `a/b` 都不该弹出菜单）；另外放行 URL 里的两种斜杠：
 * 协议分隔符（`https:/…`）与 `//` 的第二条。
 *
 * 扫描遇到空白即停止：光标所在的那一段里没有可用的斜杠就没有菜单。因此参数里再写一条
 * 斜杠（`/model/gpt`）仍然认开头那条，而 `/model gpt` 光标停在末尾时已经进了参数段，
 * 候选自然收起。从光标向前找的是**最近**的那一条。
 */
export function activeSlashToken(text: string, caret: number): SlashToken | null {
  const end = Math.max(0, Math.min(caret, text.length));
  for (let index = end - 1; index >= 0; index -= 1) {
    const char = text.charAt(index);
    if (WHITESPACE.test(char)) return null;
    if (char !== '/') continue;
    if (!slashBoundary(text, index)) continue;
    return { start: index, end, query: text.slice(index + 1, end) };
  }
  return null;
}

/** 某一条斜杠是否落在词的边界上；判定依据见 activeSlashToken 的说明。 */
function slashBoundary(text: string, index: number): boolean {
  if (index === 0) return true;
  const previous = text.charAt(index - 1);
  if (WHITESPACE.test(previous)) return true;
  if (WORD_CHAR.test(previous)) return false;
  if (previous === '/') return false;
  return !(previous === ':' && index >= 2 && !WHITESPACE.test(text.charAt(index - 2)));
}

/** 命中窗口：查询词作为有序子序列落在候选名里的那一段。 */
interface MatchWindow {
  start: number;
  length: number;
}

/**
 * 按查询词排序候选。
 *
 * 查询词必须是候选名的**有序子序列**（大小写无关），因此 `/bws` 能命中
 * `browser-skill`；排序依次看：前缀命中优先、命中窗口更紧凑优先、窗口更靠前优先，
 * 最后保持候选原本的顺序。这样「按前缀筛」是它的一个子集，字母打全时结果不变，
 * 而少打几个字也能找到东西。
 *
 * 查询词为空（只敲了一个 `/`）时原序返回，菜单因此先摊开全量候选。
 */
export function rankCandidateList<T extends RankableCandidate>(
  items: readonly T[],
  rawQuery: string,
): T[] {
  const query = rawQuery.trim().toLowerCase();
  if (query.length === 0) return [...items];
  const ranked: { item: T; index: number; prefix: boolean; window: MatchWindow }[] = [];
  items.forEach((item, index) => {
    const name = item.name.toLowerCase();
    const window = matchWindow(name, query);
    if (window === null) return;
    ranked.push({ item, index, prefix: name.startsWith(query), window });
  });
  ranked.sort(
    (left, right) =>
      Number(right.prefix) - Number(left.prefix) ||
      left.window.length - right.window.length ||
      left.window.start - right.window.start ||
      left.index - right.index,
  );
  return ranked.map((entry) => entry.item);
}

/**
 * 查询词在候选名里的最短命中窗口。
 *
 * 逐个起点贪心向前吃字符，取其中最短的一段；候选名与查询词都很短，
 * 因此不做动态规划的优化。没有任何起点能吃完查询词时返回 null。
 */
function matchWindow(name: string, query: string): MatchWindow | null {
  let best: MatchWindow | null = null;
  for (let start = 0; start < name.length; start += 1) {
    if (name.charAt(start) !== query.charAt(0)) continue;
    let cursor = start + 1;
    let matched = true;
    for (let step = 1; step < query.length; step += 1) {
      const found = name.indexOf(query.charAt(step), cursor);
      if (found < 0) {
        matched = false;
        break;
      }
      cursor = found + 1;
    }
    if (!matched) continue;
    const length = cursor - start;
    if (best === null || length < best.length) best = { start, length };
  }
  return best;
}
