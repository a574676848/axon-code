import React from 'react';

import type { ConversationController } from '../../conversation/targets';
import { ErrorCard } from './ErrorCard';
import { useOpenFailure } from './useOpenFailure';

/*
 * 正文上方的两张失败卡：事件流中断与「这条会话根本没读回来」。
 *
 * 两者的落点相同，因此摆在同一处；区别只在文案与动作。前者原本就在这里，后者此前
 * 只有输入坞上方一行 6 秒退场的回执，用户看到一片空白却无处置身可去。
 */

export interface ConversationFailureCardsProps {
  /** 会话控制器：失败事实与重连入口都从它取。 */
  conversation: ConversationController;
  /** 当前会话键：换会话即换一份结论。 */
  sessionKey: string | null;
}

export const ConversationFailureCards: React.FC<ConversationFailureCardsProps> = ({
  conversation,
  sessionKey,
}) => {
  const failure = conversation.state.failure;
  /** 重连即撤下那行一次性回执：结论已经由卡片承担，进度由过渡面与状态行表达。 */
  const retry = () => {
    conversation.clearSubmitError();
    void conversation.retry();
  };
  const { openFailure, retry: retryOpen } = useOpenFailure(
    {
      error: conversation.submitError,
      loadingHistory: conversation.loadingHistory,
      opening: conversation.opening,
      hasTurns: conversation.state.turns.length > 0,
      sessionKey,
    },
    retry,
  );
  return (
    <>
      {failure !== null && (
        <div className="shrink-0 px-4 pt-1">
          <ErrorCard
            title="会话事件流中断"
            reason={failure.message}
            retryable={failure.retryable}
            meta={['事件订阅已停止，界面不会继续刷新']}
            action={failure.retryable ? { label: '重连', onClick: retry } : undefined}
          />
        </div>
      )}
      {openFailure !== null && (
        <div className="shrink-0 px-4 pt-1">
          <ErrorCard
            title="没能打开这条会话"
            reason={openFailure}
            retryable
            meta={['正文一次都没有读回来']}
            action={{ label: '重连', onClick: retryOpen }}
          />
        </div>
      )}
    </>
  );
};
