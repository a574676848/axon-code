/*
 * 索引轮的三形态骨架。
 *
 * 索引页给的是「这一轮问了什么、最后答了什么」两件事，正文帧给的是完整过程。两处不能
 * 互相冒充：帧到了就用帧（`projectFrames`），帧还没到才用这里的骨架。因此骨架只剩两件
 * 事要做：用本地标识立起用户那一格（真帧到达时会被认领替换，不会出现两条气泡），以及
 * 在没有帧的情况下按索引的终态给一句结论。
 *
 * 尾窗先行时还要用它补开场输入：尾窗只覆盖这一轮的最后一段，超大轮次的开场那句在很
 * 前面，缺了它正文会先少一格气泡。
 */

import type { TurnMetricsReading } from '../types';
import { completedFrame, interactionStillRunning, statusFrame, wireFrame } from './historyProjection';
import type { InteractionFrames } from './interactionReplay';
import type { ConversationStreamEvent } from './wire';

/** 索引页里一条 Interaction 的界面相关字段。 */
export interface HistoryEntry {
  interactionId: string;
  receivedAtMs: number;
  status: string;
  content?: string;
  answerText?: string | null;
  /** 老网关才有：没有指标事件时的墙钟用时。 */
  elapsedMs?: number | null;
  /** 这一轮的读数；事实源是该轮自己的 `turn_metrics` 事件。 */
  metrics?: TurnMetricsReading | null;
  /** 这一轮用过的工具名；索引轮据此立起动作摘要。 */
  toolNames?: readonly string[];
  attachments?: readonly string[];
  channel?: string;
  client?: string;
}

/**
 * 索引给出的三形态骨架帧：用户说了什么、最后答了什么。
 *
 * 用户单元用本地标识落位，真正的 durable 用户帧到达时会「认领」它并换成服务端标识，
 * 因此三形态与补帧后的正文之间不会出现两条气泡。
 */
export function provisionalFrames(entry: HistoryEntry): ConversationStreamEvent[] {
  const frames: ConversationStreamEvent[] = [
    wireFrame(entry.interactionId, entry.receivedAtMs, 'userMessage', {
      partId: `local-user:${entry.interactionId}`,
      messageId: `${entry.interactionId}:user`,
      content: entry.content ?? '',
    }),
  ];
  const answer = (entry.answerText ?? '').trim();
  /*
   * 还在跑的一轮没有终态帧：索引里的 answerText 只是此刻的片段，把它当结论收口，
   * 就等于用半截正文宣布这一轮结束。
   */
  if (interactionStillRunning(entry.status)) return frames;
  if (entry.status === 'interrupted') {
    frames.push(statusFrame(entry.interactionId, 1_000, 'interrupted'));
  } else if (entry.status === 'rejected' || entry.status === 'failed') {
    // 失败与中断是两种结论：都必须在骨架上落地，否则会读成「已完成但没有回答」。
    // 网关的失败终态词是 `rejected`；老记录若写 `failed`，同样按失败收口。
    frames.push(statusFrame(entry.interactionId, 1_000, 'rejected'));
  } else if (answer.length > 0) {
    frames.push(completedFrame(entry.interactionId, answer));
  }
  return frames;
}

/**
 * 尾窗先出屏时，用索引里的开场输入补上第一格。
 *
 * 尾窗自己带着开场输入（窄轮次）时不补，否则正文里会出现两格气泡。
 */
export function withOpeningInput(
  entry: HistoryEntry,
  read: InteractionFrames,
): readonly ConversationStreamEvent[] {
  const hasInput = read.frames.some(
    (frame) => frame.kind === 'frame' && frame.payloadJson.includes('"userMessage"'),
  );
  if (hasInput) return read.frames;
  return [...provisionalFrames(entry).slice(0, 1), ...read.frames];
}
