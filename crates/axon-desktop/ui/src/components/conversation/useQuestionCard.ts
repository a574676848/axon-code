/*
 * 输入坞的问答占座卡状态。
 *
 * 卡片什么时候占位、草稿存在哪、外部入口怎么把它叫回来，都与输入与提交无关，
 * 因此单独收在这里：输入坞只按 `visible`、`draft`、`setDraft` 三件事取用。
 *
 * 占位只有一条总规则：**这条提问还没结算，卡片就占住输入坞**。进入会话、新提问
 * 到达、以及时间线上的回入口都走同一条路，卡片因此不会被留在一个需要用户去别处
 * 点一下才回来的状态。挂起的提问同样占位：它还可以回答，占住位置比藏起来更诚实。
 *
 * 唯一的例外是「这一次进来」里的收起：用户当场点了收起，就让出输入坞去打字，
 * 卡片不再自己弹回来；换一条会话再回来时收起失效，还没答完的提问重新占位。
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
  /** 卡片是否该占住输入坞：有待答问题，且这一次进来还没有被收起过。 */
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
  laneKey: string | null = null,
): QuestionCardState {
  /** 用户收起的那一条提问；换一条提问或换一条会话即失效。 */
  const [dismissedCallId, setDismissedCallId] = React.useState<string | null>(null);
  /** 作答草稿：按提问标识持有，卡片收起再打开时已选项仍在。 */
  const [drafts, setDrafts] = React.useState<Record<string, AnswerDraftMap>>({});
  /** 上一次所在的会话：用来区分「进入另一条会话」与「同一条会话里的重渲染」。 */
  const laneRef = React.useRef(laneKey);

  /*
   * 进入另一条会话就是重新唤醒：收起只对「这一次进来」有效。
   * 同一条会话里的重渲染不经过这里，因此用户正在打字时卡片不会被顶回来。
   */
  React.useEffect(() => {
    if (laneRef.current === laneKey) return;
    laneRef.current = laneKey;
    setDismissedCallId(null);
  }, [laneKey]);

  /* 时间线上的「重新打开」按次递增，等于一次显式唤醒：它同样解除收起。 */
  React.useEffect(() => {
    if (reopenToken === 0) return;
    setDismissedCallId(null);
  }, [reopenToken]);

  React.useEffect(() => {
    if (activeQuestion !== null) return;
    setDrafts((current) => (Object.keys(current).length === 0 ? current : {}));
  }, [activeQuestion]);

  const callId = activeQuestion?.callId ?? null;
  const visible =
    activeQuestion !== null &&
    activeQuestion.status !== 'settled' &&
    activeQuestion.callId !== dismissedCallId;

  const setDraft = React.useCallback(
    (next: AnswerDraftMap) => {
      if (callId === null) return;
      setDrafts((current) => ({ ...current, [callId]: next }));
    },
    [callId],
  );

  const dismiss = React.useCallback(() => {
    if (callId === null) return;
    setDismissedCallId(callId);
  }, [callId]);

  return {
    visible,
    draft: callId === null ? EMPTY_DRAFT : (drafts[callId] ?? EMPTY_DRAFT),
    setDraft,
    dismiss,
  };
}
