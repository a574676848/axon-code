/*
 * 增量接管的时间下界。
 *
 * 增量只按「本视图还不认识」去重，但补回的老轮同样不认识：向前翻过页之后，最新一页窗口里
 * 会同时含已被替换成历史的老轮，把它们当新轮追加会让旧对话出现在对话末尾。判定因此需要
 * 一个时间下界：只有不早于已加载最新时刻的轮次才算新轮。这里把它做成不依赖 React 的纯函数，
 * 让门禁能直接盯住「老轮不会变成最新一格」。
 */

/** 参与时间比较的最小形状。 */
interface ReceivedAt {
  receivedAtMs: number;
}

/** 一页里最新的接收时刻；空页没有下界。 */
export function newestReceivedAtMs(entries: readonly ReceivedAt[]): number | null {
  let newest: number | null = null;
  for (const entry of entries) {
    if (newest === null || entry.receivedAtMs > newest) newest = entry.receivedAtMs;
  }
  return newest;
}

/**
 * 从一页索引里挑出真正的新轮：本视图不认识，且不早于已加载的最新时代。
 *
 * 同刻的轮次照常收下：接收时刻只到毫秒，同一毫秒内落两条是可能的，按 `>=` 才不会漏。
 */
export function turnsNewerThanFloor<T extends ReceivedAt>(
  entries: readonly T[],
  known: ReadonlySet<string>,
  idOf: (entry: T) => string,
  floor: number | null,
): T[] {
  return entries.filter(
    (entry) => !known.has(idOf(entry)) && (floor === null || entry.receivedAtMs >= floor),
  );
}
