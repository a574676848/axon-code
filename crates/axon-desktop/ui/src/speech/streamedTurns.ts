/*
 * 已由语音流朗读过的回合。
 *
 * 语音会话开启朗读时，助手回复由宿主直传 `0x02` 音频边生成边播；
 * 这一轮收口时若再触发一次自动朗读（HTTP 合成整段），同一段话会被念两遍。
 * 因此这里记住最近的语音流回合，收口朗读前查一次。
 */

/** 最多记住的回合数：够覆盖一轮收口判定即可，不做长期缓存。 */
const CAPACITY = 16;

const spoken = new Set<string>();
const order: string[] = [];

/** 记下一个已由语音流朗读的回合。 */
export function rememberStreamedTurn(interactionId: string): void {
  if (interactionId.length === 0 || spoken.has(interactionId)) return;
  spoken.add(interactionId);
  order.push(interactionId);
  while (order.length > CAPACITY) {
    const oldest = order.shift();
    if (oldest !== undefined) spoken.delete(oldest);
  }
}

/** 这一轮是否已经由语音流朗读过。 */
export function wasStreamedTurn(interactionId: string | null | undefined): boolean {
  return interactionId !== null && interactionId !== undefined && spoken.has(interactionId);
}

/** 清空记录；会话切换时不必清，回合标识本身全局唯一。 */
export function clearStreamedTurns(): void {
  spoken.clear();
  order.length = 0;
}
