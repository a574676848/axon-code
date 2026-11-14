/*
 * 行内动作集合的构造。
 *
 * 从会话视图里抽出来，一是让视图只保留布局与编排，二是把「什么会让这份集合
 * 重建」收在一处：集合会作为 props 交给正文的每一轮，一旦每帧重建，每一轮的
 * props 都变，React.memo 就形同虚设，长会话的代价会重新变成全量重渲染。
 */

import React from 'react';
import { structuredInputBlock } from '../../conversation/references';
import type { AttachmentRef } from '../../conversation/transcript';
import { changesAddress, choiceAddress, fileAddress } from '../../sidebarRight/registry';
import type { ReferenceIndex } from '../../resources/referenceIndex';
import { resolveReference } from '../../resources/resolveReference';
import type { ConversationActions } from './actions';

/** 构造动作集合所需的外壳能力。 */
export interface ConversationActionsSource {
  /** 提交一条消息；结构化输入块由这里自己拼。 */
  submit: (
    text: string,
    attachments: readonly AttachmentRef[],
    structuredInput: string,
  ) => void;
  /** 按标识回放并展开一条被折叠的交互。 */
  unfoldTurn: (interactionId: string) => void;
  /** 在右侧停靠栏打开一个资源地址。 */
  openResource: (address: string, params?: { index: number }) => void;
  /** 把问答卡片叫回输入坞。 */
  openQuestion: () => void;
  /**
   * 这条会话已知的绝对路径索引。
   *
   * 正文里的路径是引用而不是路径值：它相对谁，字面上没有答案。索引由会话事件流
   * 派生，是「真的被碰过哪些文件」的唯一真值，缺省时退回「相对工作区」的既有语义。
   */
  referenceIndex?: () => ReferenceIndex;
}

/**
 * 从外壳能力构造行内动作集合。
 *
 * 能力经 ref 取当前值：它们每次渲染都可能是新引用，若当作依赖，动作集合会被每
 * 一帧重建，正文里每一轮的 props 随之变化。只有工作区路径参与依赖，因为它真的
 * 会变，而变化时正文里那些相对图片路径本该重新解析。
 */
export function useConversationActionsValue(
  source: ConversationActionsSource,
  workspaceRoot: string | null,
  laneId: string | null,
): ConversationActions {
  const sourceRef = React.useRef(source);
  sourceRef.current = source;
  /** 解析一段正文引用；索引经 ref 取当前值，只在真要打开时才查。 */
  const resolveTarget = React.useCallback(
    (path: string) => resolveReference(path, sourceRef.current.referenceIndex?.() ?? null),
    [],
  );
  return React.useMemo<ConversationActions>(
    () => ({
      retry: (text, attachments) => {
        // 附件重新走一次结构化输入块：只带图片的消息没有正文，块就是它的正文。
        const paths = attachments.map((attachment) => attachment.path);
        sourceRef.current.submit(
          text,
          [...attachments],
          structuredInputBlock(text, [], paths),
        );
      },
      openFile: (path) => {
        // 打开文件即打开对应的资源 tab：面板会自己展开，不需要先切视图。
        // 先解析引用：工作区外的产物靠这一步才能打开；对上多份文件时不替用户
        // 选，打开候选页；解析不出结论时原样交回去，行为与改动前一致。
        const resolution = resolveTarget(path);
        if (resolution.status === 'ambiguous') {
          sourceRef.current.openResource(choiceAddress(path.trim(), resolution.candidates));
          return;
        }
        sourceRef.current.openResource(fileAddress(resolution.path));
      },
      // 正文里的图片与文本共用同一条解析：候选写法与预览页不该各猜各的。
      resolvePath: (path) => resolveTarget(path).path,
      openChanges: (turnId, turnNumber, index) => {
        // 改动对比按「轮次」落成资源地址：文件下标是导航参数，因此同一个 tab 反复
        // 打开不同文件时只会切换落点，不会堆出一排标签。
        sourceRef.current.openResource(changesAddress(turnId, turnNumber), { index });
      },
      // 卡片只有一张，而且只承载当前待答的那条提问：入口因此只需把卡片叫回来。
      openQuestion: () => sourceRef.current.openQuestion(),
      // 折叠行自带标识：展开时按它重新回放那一条交互。
      unfoldTurn: (interactionId) => sourceRef.current.unfoldTurn(interactionId),
      // 正文里的相对图片路径以当前工作区为准，与输入坞的 `@` 引用同作用域。
      workspaceRoot,
      // 换会话就换车道：这个值进依赖，切换时重建一次集合是应该的。
      laneId,
    }),
    [resolveTarget, workspaceRoot, laneId],
  );
}
