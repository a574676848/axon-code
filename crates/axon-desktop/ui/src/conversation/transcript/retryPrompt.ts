/*
 * 失败轮次的重试入口要拿回「这一轮用户到底要求了什么」。
 *
 * 轮次投影里用户消息可能被运行中追加、也可能被折叠窗口裁掉，因此从**第一个**用户
 * 单元取：它是发起这一轮的那条消息，运行中补发的消息排在它之后。用户消息已被裁掉、
 * 或既没有正文也没有附件时没有可重发的内容，返回 `null`，界面据此不提供重试。
 */

import type { AttachmentRef } from '../transcript';
import type { TranscriptCell } from './state';

/** 一轮可重发的原始请求。 */
export interface RetryPrompt {
  text: string;
  attachments: readonly AttachmentRef[];
}

/** 取这一轮发起时的用户请求；取不到返回 `null`。 */
export function retryPromptOf(cells: readonly TranscriptCell[]): RetryPrompt | null {
  for (const cell of cells) {
    if (cell.role !== 'user') continue;
    const text = cell.text.trim();
    if (text.length === 0 && cell.attachments.length === 0) return null;
    return { text, attachments: cell.attachments };
  }
  return null;
}
