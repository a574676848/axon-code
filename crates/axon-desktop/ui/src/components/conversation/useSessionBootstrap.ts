/*
 * 会话引导：首屏直接输入与语音起手时的「先建会话、再提交」。
 *
 * 这一段的时序比看上去紧：新建会话会切换当前目标并触发一次投影重置，消息若在重置
 * 之前发出，这一轮的本地消息与流式内容都会被重置吃掉。提交因此必须等新目标的历史
 * 索引装载完成（`sessionReadyId` 与当前目标对齐）之后才发生。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import * as ipc from '../../ipc';
import type { AttachmentRef } from '../../conversation/transcript';
import type { ConversationController, SessionSelection } from '../../conversation/targets';
import type { WorkspaceItem } from '../../types';

/** 会话引导的依赖面。 */
export interface SessionBootstrapOptions {
  /** 当前选中的会话；为空表示还在首屏。 */
  selection: SessionSelection | null;
  /** 当前工作区：新建会话要用它的标识与路径。 */
  workspace: WorkspaceItem | null;
  /** 会话控制器：只取提交入口与目标就绪标记。 */
  conversation: ConversationController;
  /** 新建会话后通知外层切换目标。 */
  onSelectSession: (selection: SessionSelection) => void;
}

/** 会话引导对外给出的事实。 */
export interface SessionBootstrap {
  /** 新建会话是否在途。 */
  creating: boolean;
  /** 新建会话的失败原因。 */
  createError: string | null;
  /** 清掉失败原因：回执到点退场时调用。 */
  clearCreateError: () => void;
  /** 首屏直接提交：先建会话，等目标就绪后再发送。 */
  submitFromHero: (
    text: string,
    attachments: AttachmentRef[],
    structuredInput: string,
    /** 新会话沿用的模型名称；空表示交给配置默认值。 */
    model?: string | null,
    /** 新会话沿用的思考强度；空表示交给 `agent.reasoning_effort`。 */
    reasoningEffort?: string | null,
  ) => void;
  /** 首屏起语音时先建会话，返回这一轮可用的工作区、目的地与会话标识。 */
  ensureVoiceSession: () => Promise<{
    workspaceId: string;
    replyDestination: string;
    sessionId: string;
  } | null>;
}

export function useSessionBootstrap(options: SessionBootstrapOptions): SessionBootstrap {
  const { selection, workspace, conversation, onSelectSession } = options;
  const [creating, setCreating] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);
  /** 首屏直接输入时暂存的正文与附件；等新会话的历史索引装载完成后再发出。 */
  const pendingHeroText = useRef<{
    text: string;
    attachments: AttachmentRef[];
    /** 提交时附在正文之后的结构化输入块（`@` 文件引用）；为空表示没有引用。 */
    structuredInput: string;
    /** 新会话沿用的模型名称；空表示交给配置默认值。 */
    model: string | null;
    /** 新会话沿用的思考强度；空表示交给 `agent.reasoning_effort`。 */
    reasoningEffort: string | null;
  } | null>(null);

  /** 新建一条会话并把它设为当前目标。 */
  const createSession = useCallback(async (): Promise<SessionSelection | null> => {
    if (workspace === null) {
      setCreateError('请先在左侧添加一个工作区');
      return null;
    }
    setCreating(true);
    setCreateError(null);
    try {
      // 物化：服务端在此刻生成会话标识，本条草稿同时被替换成真实会话。
      const creation = await ipc.createSession(workspace.id);
      const target =
        creation.sessions.find((item) => item.id === workspace.id) ?? creation.sessions[0];
      const created = target?.sessions.find((session) => session.id === creation.sessionId);
      if (created === undefined) {
        setCreateError('新建会话失败：后端未返回新车道');
        return null;
      }
      const next: SessionSelection = {
        workspaceId: workspace.id,
        workspacePath: workspace.path,
        sessionId: created.id,
        destination: created.destination,
        title: created.title,
        channel: created.channel,
        // 刚建好的会话还没有内容：主区域继续留在首屏，直到第一条消息发出。
        pending: created.pending,
        draft: false,
      };
      onSelectSession(next);
      return next;
    } catch (error) {
      setCreateError(error instanceof Error ? error.message : '新建会话失败');
      return null;
    } finally {
      setCreating(false);
    }
  }, [onSelectSession, workspace]);

  /**
   * 首屏直接起语音时先建一条会话，返回这一轮可用的工作区、目的地与会话标识。
   *
   * 语音定稿由网关直接提交，界面没有第二次机会去补建车道；因此入口必须在
   * 采集开始之前就拿到一个真实目的地与真实的会话标识，否则用户说完话只会被告知
   * 「缺少工作区」或「尚未就绪」。
   */
  const ensureVoiceSession = useCallback(async () => {
    const created = await createSession();
    if (created === null) return null;
    return {
      workspaceId: created.workspaceId,
      replyDestination: created.destination,
      sessionId: created.sessionId,
    };
  }, [createSession]);

  /** 首屏直接提交：先建会话，等这一轮目标就绪后再发送。 */
  const submitFromHero = useCallback(
    (
      text: string,
      attachments: AttachmentRef[],
      structuredInput: string,
      model: string | null = null,
      reasoningEffort: string | null = null,
    ) => {
      pendingHeroText.current = { text, attachments, structuredInput, model, reasoningEffort };
      void createSession().then((target) => {
        if (target === null) pendingHeroText.current = null;
      });
    },
    [createSession],
  );

  // 目标就绪才发送：新建会话会切换当前车道并触发一次投影重置，
  // 若在重置之前把消息发出去，这一轮的本地消息与流式内容都会被重置吃掉。
  // 模型与强度随这一轮带过去：新会话还没有自己的记录，这一轮就是它的第一个事实。
  useEffect(() => {
    const pending = pendingHeroText.current;
    if (pending === null || selection === null) return;
    if (conversation.sessionReadyId !== selection.sessionId) return;
    pendingHeroText.current = null;
    void conversation.submit(
      pending.text,
      pending.attachments,
      pending.structuredInput,
      pending.model,
      pending.reasoningEffort,
    );
  }, [conversation, selection]);

  const clearCreateError = useCallback(() => setCreateError(null), []);

  return { creating, createError, clearCreateError, submitFromHero, ensureVoiceSession };
}
