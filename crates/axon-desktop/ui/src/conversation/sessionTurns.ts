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

/** 轮次头帧里的轮次标识；载荷不可解析时返回空串。 */
export function turnHeadInteractionId(payloadJson: string): string {
  try {
    const value = JSON.parse(payloadJson) as { interactionId?: unknown };
    return typeof value.interactionId === 'string' ? value.interactionId : '';
  } catch {
    return '';
  }
}

/** 轮次头消费之后的副作用出口。 */
export interface TurnsFrameHooks {
  /** 补读一次索引：出现本视图还不认识的轮次时调用。 */
  requestRefresh: () => void;
  /** 轮次头订阅断开：由调用方决定重试或把事实亮出来。 */
  onFailure: () => void;
}

/**
 * 处理一条可能属于轮次头的帧；返回 `true` 表示它已经被轮次头消费。
 *
 * - `subscribed`：服务端带了对齐锚点。锚点指向投影里没有的轮次，说明索引读完、
 *   订阅建立之间又来了一轮，这一轮不会再作为 `interaction.created` 播报，只能补读。
 * - `interaction.created` / `interaction.settled`：先看载荷里的轮次标识。已经认识的
 *   一轮不重读整页索引：它的正文由正文订阅推进，收口也由该轮自己的终态帧完成；
 *   只有不认识的轮次（外部发起）才值得读一次索引。
 * - `failure`：订阅断开，交给调用方重试或如实报出。
 */
export function handleTurnsFrame(
  event: ConversationStreamEvent,
  knownTurnIds: readonly string[],
  hooks: TurnsFrameHooks,
): boolean {
  if (!event.subscribeId.startsWith(SESSION_TURNS_PREFIX)) return false;
  if (event.kind === 'subscribed') {
    if (baselineNeedsRefresh(event.baseline, knownTurnIds)) hooks.requestRefresh();
    return true;
  }
  if (event.kind === 'failure') {
    hooks.onFailure();
    return true;
  }
  if (
    event.kind === 'frame' &&
    (event.eventType === 'interaction.created' || event.eventType === 'interaction.settled')
  ) {
    const interactionId = turnHeadInteractionId(event.payloadJson);
    if (interactionId.length > 0 && knownTurnIds.includes(interactionId)) return true;
    hooks.requestRefresh();
  }
  return true;
}
