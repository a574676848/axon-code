/*
 * 输入坞的问答占座卡状态。
 *
 * 卡片什么时候占位、草稿存在哪、外部入口怎么把它叫回来，都与输入与提交无关，
 * 因此单独收在这里：输入坞只按 `visible`、`draft`、`setDraft` 三件事取用。
 *
 * 草稿按提问标识持有：卡片收起再打开时已选项还在；问题一旦结算，草稿随之清空，
 * 否则它会随会话时长一直堆着。
 */

import React from 'react';

import type { PendingQuestion } from '../../conversation/questions';
import type { AnswerDraftMap } from '../../conversation/questionDraft';

/** 还没有草稿时的空值。 */
const EMPTY_DRAFT: AnswerDraftMap = {};

export interface QuestionCardState {
  /** 卡片是否该占住输入坞：有待答问题、没被收起，且处于等待或延长状态。 */
  visible: boolean;
  /** 当前问题的草稿。 */
  draft: AnswerDraftMap;
  /** 写入当前问题的草稿。 */
  setDraft: (next: AnswerDraftMap) => void;
  /** 用户把卡片收起来。 */
  dismiss: () => void;
}

export function useQuestionCard(
  activeQuestion: PendingQuestion | null,
  reopenToken: number,
): QuestionCardState {
  /** 占座卡是否被用户显式收起。 */
  const [dismissed, setDismissed] = React.useState(false);
  /** 卡片是否由外部入口叫回主展示位；它同时让挂起的提问也能作答。 */
  const [forced, setForced] = React.useState(false);
  /** 上一次看到的提问标识，用于在出现新提问时自动重新弹出占座卡。 */
  const lastCallId = React.useRef<string | null>(null);
  /** 作答草稿：按提问标识持有，卡片收起再打开时已选项仍在。 */
  const [drafts, setDrafts] = React.useState<Record<string, AnswerDraftMap>>({});

  React.useEffect(() => {
    if (activeQuestion !== null && activeQuestion.callId !== lastCallId.current) {
      lastCallId.current = activeQuestion.callId;
      setDismissed(false);
      setForced(false);
    }
  }, [activeQuestion]);

  React.useEffect(() => {
    if (reopenToken === 0) return;
    setDismissed(false);
    setForced(true);
  }, [reopenToken]);

  React.useEffect(() => {
    if (activeQuestion !== null) return;
    setForced(false);
    setDrafts((current) => (Object.keys(current).length === 0 ? current : {}));
  }, [activeQuestion]);

  const callId = activeQuestion?.callId ?? null;
  const visible =
    activeQuestion !== null &&
    !dismissed &&
    (forced || activeQuestion.status === 'pending' || activeQuestion.status === 'extended');

  const setDraft = React.useCallback(
    (next: AnswerDraftMap) => {
      if (callId === null) return;
      setDrafts((current) => ({ ...current, [callId]: next }));
    },
    [callId],
  );

  const dismiss = React.useCallback(() => setDismissed(true), []);

  return {
    visible,
    draft: callId === null ? EMPTY_DRAFT : (drafts[callId] ?? EMPTY_DRAFT),
    setDraft,
    dismiss,
  };
}
