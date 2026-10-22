import { useCallback, useEffect, useMemo, useRef } from 'react';
import * as ipc from '../ipc';
import type { AnswerItem, PendingQuestion } from './questions';
import { shouldReattachLane } from './questions';
import type { ConversationAction, ConversationState } from './transcript/state';

export interface UseQuestionSyncReturn {
  /** 当前车道最新需要关注的提问（pending / extended / suspended）。 */
  activeQuestion: PendingQuestion | null;
  /** 提交问题的作答。 */
  answerQuestion: (callId: string, answers: AnswerItem[]) => Promise<void>;
}

/** 问答对账与作答的装配参数。 */
export interface QuestionSyncOptions {
  /** 当前车道；为空时什么也不做。 */
  laneId: string | null;
  /** 对话投影。 */
  state: ConversationState;
  /** 投影入口。 */
  dispatch: React.Dispatch<ConversationAction>;
  /** 作答被服务端拒绝时的提示入口。 */
  onError?: (err: string) => void;
  /**
   * 作答之后的续跑不在本视图的订阅里时，用它把车道重新贴一次。
   *
   * 答案直接回灌给活等待者时同一条订阅还在看着那一轮，界面自己会长出后续；
   * 答案走消息通道时续跑的那一轮由守护进程开启，界面必须重贴车道才能看到。
   */
  onContinuation?: () => void;
  /**
   * 车道重贴的请求序号。
   *
   * 重贴会把整个投影连同权威台账一起重置，因此每贴一次都要重新对账一次；
   * 否则回放出来的挂起帧会把已经答完的提问重新摆成待答。
   */
  reattachToken?: number;
}

export function useQuestionSync({
  laneId,
  state,
  dispatch,
  onError,
  onContinuation,
  reattachToken = 0,
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

  /*
   * UQ-107: 问答状态对账。
   *
   * 两条触发路径：
   * - **进入车道**先校准一次（`requireActive` 为 false）：本地待答位可能是从历史帧
   *   重建出来的，而迟到作答只发布结算事件、没有可依附的回合，重建结果会比服务端
   *   事实旧；不校准就会把已经结算的提问继续摆在待答位上。
   * - **窗口重新聚焦**再校准一次，且只在确实存在待答项时请求（`requireActive` 为 true）。
   *
   * 对账走 Rust 侧 IPC 命令：网关协议在这里不是可直接消费的形状（字段名不同、
   * 也缺 partId），由后端适配完再交上来；每次请求都带 limit，网关不会因此被拉爆。
   *
   * **失败一律不动本地状态**：对账只是一次校准，拿不到权威结论时本地事实仍然
   * 是用户看到的那一份，把它清掉等于凭空丢掉一个待答提示。
   */
  const reconcile = useCallback(
    async (requireActive: boolean) => {
      if (!laneId) return;
      if (requireActive && activeQuestionRef.current === null) return;
      try {
        const questions = await ipc.conversationLaneQuestions(laneId);
        dispatch({ type: 'reconcile-questions', questions, authoritative: true });
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        console.warn('[axon] 问答状态对账失败，保留本地状态', msg);
      }
    },
    [laneId, dispatch],
  );

  const reconcileRef = useRef(reconcile);
  reconcileRef.current = reconcile;

  const answerQuestion = useCallback(
    async (callId: string, answers: AnswerItem[]) => {
      if (!laneId) return;
      try {
        const receipt = await ipc.conversationAnswerQuestion(laneId, callId, answers);
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
        if (shouldReattachLane(receipt?.deliveryMode)) {
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
    [laneId, state.pendingQuestions, dispatch, onError, onContinuation],
  );

  /*
   * UQ-107: 问答状态对账见上：这里只挂载两条触发时机。
   * 重贴车道同样要重新对账一次：重贴把台账一起重置了。
   */

  useEffect(() => {
    void reconcile(false);
  }, [reconcile, reattachToken]);

  useEffect(() => {
    if (!laneId) return;

    const onFocus = () => {
      void reconcile(true);
    };

    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [laneId, reconcile]);

  return {
    activeQuestion,
    answerQuestion,
  };
}
