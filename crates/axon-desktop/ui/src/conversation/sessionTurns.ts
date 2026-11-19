/*
 * 轮次头订阅的帧处理：只做「要不要补读索引」这一件事。
 *
 * 从会话编排里抽出来的原因有两个：这条判断同时有「建立回执」与「事件帧」两个入口，
 * 写在编排里会把一次简单的策略拆散；编排文件本身也还在长，规则必须有自己的住处。
 */

import type { ConversationStreamEvent } from './wire';
import { baselineNeedsRefresh } from './wire';

/** 轮次头订阅的标识前缀；与正文订阅（`interaction:`）必须分开。 */
export const SESSION_TURNS_PREFIX = 'session-turns:';

/**
 * 处理一条可能属于轮次头的帧；返回 `true` 表示它已经被轮次头消费。
 *
 * - `subscribed`：服务端带了对齐锚点。锚点指向投影里没有的轮次，说明索引读完、
 *   订阅建立之间又来了一轮，这一轮不会再作为 `interaction.created` 播报，只能补读。
 * - `interaction.created` / `interaction.settled`：轮次清单变了，唤一次索引重读。
 */
export function handleTurnsFrame(
  event: ConversationStreamEvent,
  knownTurnIds: readonly string[],
  requestRefresh: () => void,
): boolean {
  if (!event.subscribeId.startsWith(SESSION_TURNS_PREFIX)) return false;
  if (event.kind === 'subscribed') {
    if (baselineNeedsRefresh(event.baseline, knownTurnIds)) requestRefresh();
    return true;
  }
  if (
    event.kind === 'frame' &&
    (event.eventType === 'interaction.created' || event.eventType === 'interaction.settled')
  ) {
    requestRefresh();
  }
  return true;
}
