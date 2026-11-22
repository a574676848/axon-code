/*
 * durable 游标的解析。
 *
 * 游标是 `{generation, sequence}` 的 JSON 文本（网关与桌面端共用同一份编解码基准）。
 * 序号是唯一可靠的「读到哪一帧」判据：历史分页按它决定补前段还是接尾，续读按它去重。
 *
 * 单独成篇的原因：纯投影层（`transcript/**`）与续读判定都要用它，而历史回放那一篇
 * 带着 IPC 依赖，投影层不该被动地拉进一条网络链路。
 */

/** 从游标里取序号；不是合法游标时返回 null。 */
export function cursorSequence(cursor: string | null): number | null {
  if (cursor === null) return null;
  try {
    const parsed: unknown = JSON.parse(cursor);
    const sequence = (parsed as { sequence?: unknown }).sequence;
    return typeof sequence === 'number' && Number.isFinite(sequence) ? sequence : null;
  } catch {
    return null;
  }
}
