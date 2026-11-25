import { useCallback, useEffect, useMemo, useRef } from 'react';
import * as ipc from '../ipc';
import type { AnswerItem, PendingQuestion } from './questions';
import { shouldReattachSession } from './questions';
import type { ConversationAction, ConversationState } from './transcript/state';

export interface UseQuestionSyncReturn {
  /** 当前车道最新需要关注的提问（pending / extended / suspended）。 */
  activeQuestion: PendingQuestion | null;
  /** 提交问题的作答。 */
  answerQuestion: (callId: string, answers: AnswerItem[]) => Promise<void>;
  /**
   * 再向权威记录要一次结论。
   *
   * 时间线那条问答单元的「重新打开」用它：待答位此刻可能是空的（回放见到的提问只
   * 够画出单元），空位上的卡片不会因为一次点击就存在，必须把权威结论取回来。
   */
  reconcileQuestions: () => void;
}

/** 问答对账与作答的装配参数。 */
export interface QuestionSyncOptions {
  /** 当前车道；为空时什么也不做。 */
  sessionId: string | null;
  /**
   * 会话投影的代次。
   *
   * 换会话与重试重开都会清空投影（`reset`），而清空之前取回的问答结论会被同一次
   * 清空原样丢掉。对账因此挂在代次上：清空一次就重取一次。只依赖 `sessionId` 时，
   * 首次进入那条会话的对账抢在清空之前返回，待答位就一直是空的，也没有任何一处
   * 会再触发一次（窗口聚焦那一路要求本地已经有待答项）。
   */
  projectionGeneration: number;
  /** 对话投影。 */
  state: ConversationState;
  /** 投影入口。 */
  dispatch: React.Dispatch<ConversationAction>;
  /** 作答被服务端拒绝时的提示入口。 */
  onError?: (err: string) => void;
  /**
   * 作答之后的续跑不在本视图的订阅里时，让它把新增的那一轮接进来。
   *
   * 答案直接回灌给活等待者时同一条订阅还在看着那一轮，界面自己会长出后续；
   * 答案走消息通道时续跑的那一轮由守护进程开启，界面必须按增量接管才看得到。
   */
  onContinuation?: () => void;
}

export function useQuestionSync({
  sessionId,
  projectionGeneration,
  state,
  dispatch,
  onError,
  onContinuation,
}: QuestionSyncOptions): UseQuestionSyncReturn {
  // 提取当前活跃提问：优先 pending/extended，其次 suspended
  const activeQuestion = useMemo<PendingQuestion | null>(() => {
    const questions = Object.values(state.pendingQuestions);
    const active = questions.find(
      (q) => q.status === 'pending' || q.status === 'extended',
    );
    if (active) return active;
    return questions.find((q) => q.status === 'suspended') ?? null;
  }, [state.pendingQuestions]);

  const activeQuestionRef = useRef(activeQuestion);
  activeQuestionRef.current = activeQuestion;

  /**
   * 当前车道标识。
   *
   * 投影自身不带车道标识，一次迟到的对账结论只能在这里核对归属：它回来时用户可能
   * 已经换到另一条会话，把它写进新会话的待答位就是凭空长出一条并不存在的提问。
   */
  const sessionIdRef = useRef(sessionId);
  sessionIdRef.current = sessionId;

  /*
   * UQ-107: 问答状态对账。
   *
   * 三条触发路径：
   * - **投影清空之后**先校准一次（`requireActive` 为 false）：换会话、重试重开都走
   *   `reset`，代次随之前进，对账跟着重取一次。这是补位的主路径。
   * - **窗口重新聚焦**再校准一次，且只在确实存在待答项时请求（`requireActive` 为 true）。
   * - **时间线上的回入口**显式请求一次（见返回的 `reconcileQuestions`）。
   *
   * 对账走 Rust 侧 IPC 命令：网关协议在这里不是可直接消费的形状（字段名不同、
   * 也缺 partId），由后端适配完再交上来；每次请求都带 limit，网关不会因此被拉爆。
   *
   * **失败一律不动本地状态**：对账只是一次校准，拿不到权威结论时本地事实仍然
   * 是用户看到的那一份，把它清掉等于凭空丢掉一个待答提示。
   */
  const reconcile = useCallback(
    async (requireActive: boolean) => {
      if (!sessionId) return;
      if (requireActive && activeQuestionRef.current === null) return;
      try {
        const questions = await ipc.conversationSessionQuestions(sessionId);
        // 迟到的结论不写进别的车道：它在回来之前目标可能已经换过。
        if (sessionIdRef.current !== sessionId) return;
        dispatch({ type: 'reconcile-questions', questions, authoritative: true });
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        console.warn('[axon] 问答状态对账失败，保留本地状态', msg);
      }
    },
    [sessionId, dispatch],
  );

  const reconcileRef = useRef(reconcile);
  reconcileRef.current = reconcile;

  const answerQuestion = useCallback(
    async (callId: string, answers: AnswerItem[]) => {
      if (!sessionId) return;
      try {
        const receipt = await ipc.conversationAnswerQuestion(sessionId, callId, answers);
        const existing = state.pendingQuestions[callId];
        if (existing) {
          dispatch({
            type: 'reconcile-questions',
            questions: [
              {
                ...existing,
                status: 'settled',
                answers,
              },
            ],
          });
        }
        // 走消息通道的作答会换来新的一轮：那一条 Interaction 界面此前不认识，
        // 不重贴车道就会一直停在原地，看起来像「答了但没继续」。
        if (shouldReattachSession(receipt?.deliveryMode)) {
          onContinuation?.();
        }
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        // 提交被拒多半是这一条已经在别处结算（迟到作答、渠道作答）：先把服务端事实
        // 取回来，再把原因交给上层弹窗，界面因此不会留着一张已经过期的卡片。
        void reconcileRef.current(false);
        if (onError) onError(msg);
      }
    },
    [sessionId, state.pendingQuestions, dispatch, onError, onContinuation],
  );

  /*
   * UQ-107: 问答状态对账见上：这里只挂载触发时机。投影每清空一次（代次前进）对账
   * 一次，窗口重新聚焦再对账一次（且只在确实存在待答项时请求）。
   */

  useEffect(() => {
    void reconcile(false);
  }, [projectionGeneration, reconcile]);

  useEffect(() => {
    if (!sessionId) return;

    const onFocus = () => {
      void reconcile(true);
    };

    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [sessionId, reconcile]);

  /** 时间线上的回入口：显式向权威记录要一次结论，空待答位因此能被叫回来。 */
  const reconcileQuestions = useCallback(() => {
    void reconcile(false);
  }, [reconcile]);

  return {
    activeQuestion,
    answerQuestion,
    reconcileQuestions,
  };
}
