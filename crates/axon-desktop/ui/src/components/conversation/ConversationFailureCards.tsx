import React from 'react';

import * as ipc from '../../ipc';
import type { ConversationController } from '../../conversation/targets';
import type { LiveEvidence } from '../../conversation/liveEvidence';
import { ErrorCard } from './ErrorCard';
import { NoticeBlock } from './Transcript';
import { useOpenFailure } from './useOpenFailure';

/*
 * 正文上方的几张提示：事件流中断、这条会话根本没读回来、运行证据过期与残缺历史。
 *
 * 四者落点相同，因此摆在同一处；区别只在文案与动作。失败两张原本就在这里，
 * 后两张是「服务端给了但界面还没如实说出来」的两条事实：
 * 运行证据过期意味着不该继续显示在跑，残缺回放意味着不该把手上这段当完整历史。
 */

export interface ConversationFailureCardsProps {
  /** 会话控制器：失败事实与重连入口都从它取。 */
  conversation: ConversationController;
  /** 当前会话键：换会话即换一份结论。 */
  sessionKey: string | null;
  /** 运行态证据结论：过期时给一张带可执行建议的卡；未知或成立时不占位。 */
  evidence?: LiveEvidence;
}

export const ConversationFailureCards: React.FC<ConversationFailureCardsProps> = ({
  conversation,
  sessionKey,
  evidence,
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
  /** 后台服务可能已经停机：重启是这一档唯一能自己完成的动作。 */
  const restartService = React.useCallback(() => {
    void ipc.gatewayStartService('restart').catch(() => undefined);
  }, []);
  const replayNotice = conversation.state.replayNotice;
  const expired = evidence !== undefined && evidence.state === 'expired' ? evidence : null;
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
      {expired !== null && (
        <div className="shrink-0 px-4 pt-1">
          <ErrorCard
            title={expired.title ?? '运行证据已过期'}
            reason={expired.reason ?? ''}
            hint={expired.advice ?? ''}
            meta={expired.meta}
            action={
              expired.action === null
                ? undefined
                : { label: expired.action, onClick: restartService }
            }
          />
        </div>
      )}
      {replayNotice !== null && (
        <div className="shrink-0 px-4 pt-1">
          <NoticeBlock
            cell={{
              id: 'replay-truncated',
              role: 'notice',
              title: '历史待补齐',
              summary: replayNotice,
              tone: 'info',
            }}
          />
        </div>
      )}
    </>
  );
};
