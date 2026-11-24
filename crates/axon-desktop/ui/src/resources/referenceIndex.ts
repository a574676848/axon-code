/*
 * 正文引用的解析索引。
 *
 * 正文里的路径是自由文本，字面上没有写它相对谁；而「这次会话到底碰过哪些文件」
 * 只有事件流知道。索引把这份真值收成一张只读表，让打开动作能按后缀反查。
 *
 * 三条边界：
 * - 只收绝对路径。相对路径没有基准，收进来只会制造歧义。
 * - 索引只读，不判断文件是否还在。历史正文里的引用仍然是它当时指向的那一份，
 *   存在性由打开时那一次探测判定。
 * - 有条目上限。长会话不该把内存花在早已翻过去的路径上。
 */

/** 一个已知的绝对路径。 */
export interface ReferenceEntry {
  /** 规范化后的绝对路径：正斜杠、无设备前缀，可直接交给后端。 */
  absolute: string;
  /** 匹配用的原样形式：规范化之后保留大小写。 */
  normalized: string;
  /** 分桶与宽松匹配用的折叠形式：normalized 转小写。 */
  folded: string;
  /** 路径段数，用于候选排序：更浅的更可能是用户想看的那个。 */
  depth: number;
}

/** 只读索引。 */
export interface ReferenceIndex {
  /** 按引用反查，返回按「更浅优先、字典序」排好的候选。 */
  match(reference: string): ReferenceEntry[];
  /** 条目数，供诊断与测试读取。 */
  readonly size: number;
}

/** 条目上限：超出后丢最早的。 */
const MAX_ENTRIES = 4096;

/** 候选上限：有歧义时也不需要把几十条同名文件全列出来。 */
export const MAX_CANDIDATES = 8;

/**
 * 规范化一个路径：去掉 Windows 设备前缀，统一正斜杠，收敛重复分隔符。
 *
 * UNC 的前导双斜杠要保住：`//server/share` 与 `/server/share` 是两回事，
 * 一起收敛会把网络路径变成根路径。
 */
export function normalizePath(raw: string): string {
  let text = raw.trim();
  // `\\?\E:\a` 与 `\\.\E:\a` 是 Win32 设备前缀，属于实现细节，不参与比较。
  if (text.startsWith('\\\\?\\') || text.startsWith('\\\\.\\')) text = text.slice(4);
  text = text.replace(/\\/g, '/');
  const unc = text.startsWith('//');
  text = text.replace(/\/{2,}/g, '/');
  if (unc) text = `/${text}`;
  // `./docs/a.md` 与 `docs/a.md` 指同一件事，前导 `./` 不带信息。
  text = text.replace(/^(?:\.\/)+/, '');
  return text.length > 1 && text.endsWith('/') ? text.slice(0, -1) : text;
}

/** 是否已经是「不需要基准」的引用：绝对路径、UNC 或 `~` 开头。 */
export function isAbsoluteReference(raw: string): boolean {
  const text = normalizePath(raw);
  return text.startsWith('~') || text.startsWith('/') || /^[a-zA-Z]:\//.test(text);
}

/** 段数，含盘符与 UNC 的服务器段。 */
function depthOf(normalized: string): number {
  let depth = 1;
  for (const char of normalized) {
    if (char === '/') depth += 1;
  }
  return depth;
}

/** 末段名称，反查按它分桶。 */
function nameOf(normalized: string): string {
  const slash = normalized.lastIndexOf('/');
  return slash === -1 ? normalized : normalized.slice(slash + 1);
}

/** 候选序：深度小的在前，同深度按字符串序。 */
function isRankedBefore(left: ReferenceEntry, right: ReferenceEntry): boolean {
  if (left.depth !== right.depth) return left.depth < right.depth;
  return left.normalized < right.normalized;
}

/**
 * 挑出前若干个候选。
 *
 * 用一遍插入而不是整体排序：同名文件可能上千条（`index.ts`、`mod.rs` 这类），
 * 而真正要用的只有最前面几条，为它们排全表是白付的代价。比较用字符串字面序
 * 而不是 locale 序：这里只需要稳定，不需要语言习惯。
 */
function rankCandidates(hits: ReferenceEntry[]): ReferenceEntry[] {
  const top: ReferenceEntry[] = [];
  for (const entry of hits) {
    let at = top.length;
    while (at > 0) {
      const previous = top[at - 1];
      if (previous === undefined || !isRankedBefore(entry, previous)) break;
      at -= 1;
    }
    if (at >= MAX_CANDIDATES) continue;
    top.splice(at, 0, entry);
    if (top.length > MAX_CANDIDATES) top.pop();
  }
  return top;
}

/**
 * 反查：先按末段名分桶，再按「整条相等或后缀相等」筛。
 *
 * 分桶是为了让单次反查只碰同名的少数几条，而不是扫全表。
 *
 * 大小写分两步走：先按原文大小写对一遍，能对上就只认这一批（用户写的就是那个
 * 名字）；对不上再按折叠形式宽松匹配。这样 Windows 与 macOS 上「大小写随便写
 * 都能开」照旧，而 Linux 上两份仅大小写不同的文件不会被悄悄并成一条。
 */
function matchIn(byName: Map<string, ReferenceEntry[]>, reference: string): ReferenceEntry[] {
  const wanted = normalizePath(reference);
  if (wanted === '') return [];
  const folded = wanted.toLowerCase();
  const bucket = byName.get(nameOf(folded));
  if (bucket === undefined) return [];
  const looseSuffix = `/${folded}`;
  const loose = bucket.filter(
    (entry) => entry.folded === folded || entry.folded.endsWith(looseSuffix),
  );
  if (loose.length === 0) return [];
  const exactSuffix = `/${wanted}`;
  const exact = loose.filter(
    (entry) => entry.normalized === wanted || entry.normalized.endsWith(exactSuffix),
  );
  return rankCandidates(exact.length > 0 ? exact : loose);
}

/**
 * 用一批原始路径建索引。
 *
 * 输入顺序即已知顺序（越晚越新）；超出上限时丢最早的，因为它最可能已经不在
 * 用户当下的视野里。输入里的相对路径直接跳过，宁缺勿错。
 *
 * 去重只对「逐字相同」的路径生效：仅大小写不同的两条各自留一条，否则 Linux 上
 * 两份真实存在的文件会被并成一份，打开哪一份就变成运气问题了。
 */
export function buildReferenceIndex(paths: Iterable<string>): ReferenceIndex {
  const ordered: ReferenceEntry[] = [];
  const seen = new Set<string>();
  for (const raw of paths) {
    if (typeof raw !== 'string') continue;
    const absolute = normalizePath(raw);
    if (!isAbsoluteReference(absolute)) continue;
    if (seen.has(absolute)) continue;
    seen.add(absolute);
    ordered.push({
      absolute,
      normalized: absolute,
      folded: absolute.toLowerCase(),
      depth: depthOf(absolute),
    });
  }
  const kept = ordered.length > MAX_ENTRIES ? ordered.slice(ordered.length - MAX_ENTRIES) : ordered;
  const byName = new Map<string, ReferenceEntry[]>();
  for (const entry of kept) {
    const name = nameOf(entry.folded);
    const bucket = byName.get(name);
    if (bucket === undefined) byName.set(name, [entry]);
    else bucket.push(entry);
  }
  return { size: kept.length, match: (reference) => matchIn(byName, reference) };
}
