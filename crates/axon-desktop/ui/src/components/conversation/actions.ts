import React from 'react';
import type { AttachmentRef } from '../../conversation/transcript';

/*
 * 时间线单元可用的行内动作。
 *
 * 时间线本身只负责呈现，重发消息与打开文件都属于会话外壳的能力，
 * 因此用上下文注入，避免把回调逐层穿过渲染组件。
 */

export interface ConversationActions {
  /**
   * 重新发送一条用户消息。
   *
   * 附件必须随消息一起重发：只带图片的消息正文是空的，丢掉附件就等于把这条消息
   * 变成了一条空消息，重发只会换来一句「消息内容不能为空」。
   */
  retry: (text: string, attachments: readonly AttachmentRef[]) => void;
  /** 在右侧栏打开工作区内的一个文件。 */
  openFile: (path: string) => void;
  /** 在右侧栏打开某一轮的改动对比，并落到该轮第 index 个文件。 */
  openChanges: (turnId: string, turnNumber: number, index: number) => void;
  /** 把收起或挂起的问答卡片叫回输入坞，并把焦点交给它。 */
  openQuestion: (callId: string) => void;
  /**
   * 展开一条被折叠的 Interaction。
   *
   * 折叠时正文已经释放，因此展开要按标识重新回放那一条，再把单元换回来。
   */
  unfoldTurn: (interactionId: string) => void;
  /**
   * 当前工作区根目录；正文里的相对图片路径以它为准。
   *
   * 与输入坞的 `@` 引用同作用域，因此正文写 `assets/shot.png` 与引用同一个文件
   * 指向同一张图。没有工作区时为空。
   */
  workspaceRoot: string | null;
  /**
   * 这一条会话是否自动朗读新回复。
   *
   * 由会话外壳算好（会话开关没有显式设置时继承全局开关），时间线只读结论：
   * 让每条消息自己去翻本地存储，会话隔离这件事就会散成多份实现。
   */
  autoReadReplies: boolean;
}

const ConversationActionsContext = React.createContext<ConversationActions | null>(null);

export const ConversationActionsProvider = ConversationActionsContext.Provider;

/** 取当前会话的动作集合；不在会话外壳内时返回空。 */
export function useConversationActions(): ConversationActions | null {
  return React.useContext(ConversationActionsContext);
}
