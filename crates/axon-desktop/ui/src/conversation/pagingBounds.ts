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
 * 审计轮：派发渠道写入、已收口、但没有模型正文。
 *
 * 目标工具每改一次目标就在目标会话里留一条记录（「目标已启动：…」）。它不是对话：
 * 没有提问者，也没有回答，把它画成一条用户气泡再接一段空白，读起来就是「说了话没人
 * 答」。这类记录只属于目标看板的活动时间线，正文转录不渲染它。
 */
/** 审计轮判定所需的最小形状；索引里的渠道字段可缺省。 */
interface AuditShape {
  channel?: string;
  status: string;
  answerText?: string | null;
}

export function isAuditTurn(entry: AuditShape): boolean {
  return (
    entry.channel === 'agent' &&
    entry.status === 'understood' &&
    (entry.answerText ?? '').trim() === ''
  );
}

/**
 * 打开会话时要展开的那一轮：优先最近一条「有正文或在跑」的轮次。
 *
 * 目标与会话里还有一类已收口但没有正文的轮次（Goal 工具更新、派发记录），它们只有
 * 一句系统说明，没有过程可供展开；把最新一格让给它们，正文区会读成「这一轮什么都没
 * 发生」，顶部的上下文与首字读数也跟着整组消失。
 */
export function newestExpandable<T extends AuditShape>(entries: readonly T[]): T | null {
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index];
    if (entry === undefined || isAuditTurn(entry)) continue;
    if ((entry.answerText ?? '').trim() !== '' || entry.status !== 'understood') {
      return entry;
    }
  }
  return null;
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
