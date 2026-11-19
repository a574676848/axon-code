import React from 'react';
import { AgentIcon, ChevronRightIcon } from '../../icons';
import * as ipc from '../../ipc';
import { useConversation } from '../../conversation/useConversation';
import { canonicalAgentId } from '../../conversation/subagentRoster';
import type { SubagentRow } from '../../conversation/transcript';
import { ConversationStream } from './ConversationStream';
import { InputDock } from './InputDock';
import { Text } from '../ui/surfaces';

/*
 * 子代理会话视图。
 *
 * 子代理是**独立会话**，但与主会话走同一条渲染管线与同一套取数：同一个控制器、
 * 同一份历史索引、同一个归约器。两条订阅并存于同一条连接上，因此在这里来回切换
 * 不会掐断主会话的流，也不会重建连接。
 *
 * 输入条直接复用主输入坞的紧凑变体：字号、发送按钮、键盘与历史都是同一份实现，
 * 这里只关掉模型选择与附件（那两项由委派时的意图决定）。
 */

/** 没有运行态会话行时的可读结论：原因码由网关给出，这里只翻译成一句话。 */
function unavailableText(reason: string | null | undefined): string {
  if (reason === 'no-runtime-session') return '这次执行早于子代理会话化，没有可回看的逐字稿';
  return '这条子代理还没有可回看的会话';
}

export interface SubagentRunViewProps {
  row: SubagentRow;
  /** 该子代理所属工作区与车道；定位会话与投递消息都要。 */
  workspaceId: string | null;
  sessionId: string | null;
  /** 返回主会话。 */
  onBack: () => void;
}

export const SubagentRunView: React.FC<SubagentRunViewProps> = ({ row, workspaceId, sessionId, onBack }) => {
  const [locatedSessionId, setLocatedSessionId] = React.useState<string | null>(null);
  const [title, setTitle] = React.useState<string | null>(null);
  const [model, setModel] = React.useState<string | null>(null);
  const [locateError, setLocateError] = React.useState<string | null>(null);
  const [sending, setSending] = React.useState(false);
  const [sendError, setSendError] = React.useState<string | null>(null);

  // 会话定位：子代理会话挂在车道的运行会话之下，用 slug 与界面上的 agent 路径对齐。
  React.useEffect(() => {
    if (sessionId === null) {
      setLocateError('这条会话没有绑定车道，无法定位子代理会话');
      return undefined;
    }
    let cancelled = false;
    void ipc
      .conversationSessionSubagents(sessionId)
      .then((list) => {
        if (cancelled) return;
        /*
         * 匹配键是规范化的代理标识：网关给的是库里的相对路径，转录行给的是绝对路径，
         * 两者只在去掉前导斜杠后同源。
         */
        const wanted = canonicalAgentId(row.agentId);
        const match = list.subagents.find((item) => canonicalAgentId(item.agentId) === wanted);
        if (match === undefined) {
          setLocateError('还没有读到这个子代理的会话记录');
          return;
        }
        if (match.sessionId === null || match.sessionId === '') {
          setLocateError(unavailableText(match.unavailableReason));
          return;
        }
        setLocatedSessionId(match.sessionId);
        setTitle(match.title ?? null);
        setModel(match.model ?? row.model);
      })
      .catch((failure: unknown) => {
        if (!cancelled) {
          setLocateError(failure instanceof Error ? failure.message : '读取子代理会话失败');
        }
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, row.agentId, row.model]);

  const target = React.useMemo(
    () =>
      workspaceId === null || locatedSessionId === null
        ? null
        : ({
            kind: 'interaction',
            workspaceId,
            interactionId: locatedSessionId,
            // 子代理轮次不支持用户提交：投递走专用通道，因此这里不提供回复目标。
            destination: '',
          } as const),
    [workspaceId, locatedSessionId],
  );
  const conversation = useConversation(target);

  /** 投递一条消息：输入坞只把正文交出来，附件与模型选择在这个变体里是关闭的。 */
  const send = (text: string) => {
    if (text.trim().length === 0 || sending || locatedSessionId === null) return;
    setSending(true);
    setSendError(null);
    void ipc
      .conversationSendSubagentMessage(locatedSessionId, row.agentId, text)
      .catch((failure: unknown) => {
        setSendError(
          failure instanceof Error
            ? failure.message
            : typeof failure === 'string' && failure.length > 0
              ? failure
              : '消息投递失败',
        );
      })
      .finally(() => setSending(false));
  };

  const failure = locateError ?? conversation.state.failure?.message ?? null;

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <div className="shrink-0 h-9 px-4 flex items-center gap-2 hairline-b border-border-hairline">
        <button
          type="button"
          onClick={onBack}
          className="shrink-0 h-6 pl-1 pr-2 inline-flex items-center gap-0.5 rounded-xs text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
        >
          <ChevronRightIcon size={12} className="rotate-180" />
          返回会话
        </button>
        <Text tone="tertiary" size="meta" className="shrink-0">
          ·
        </Text>
        <span className="shrink-0 inline-flex items-center gap-1.5 min-w-0">
          <AgentIcon size={14} className="shrink-0 text-text-tertiary" />
          <span className="min-w-0 truncate text-row text-text-primary">
            {title !== null && title.length > 0 ? title : row.label}
          </span>
        </span>
        {model !== null && model.length > 0 && (
          <>
            <Text tone="tertiary" size="meta" className="shrink-0">
              ·
            </Text>
            <Text tone="tertiary" size="xs" mono truncate className="shrink-0">
              {model}
            </Text>
          </>
        )}
      </div>

      {failure !== null ? (
        <div className="flex-1 min-h-0 flex items-center justify-center px-6">
          <Text tone="secondary" size="meta">
            {failure}
          </Text>
        </div>
      ) : (
        <ConversationStream
          sessionKey={`subagent:${row.agentId}`}
          turns={conversation.state.turns}
          followToken={conversation.state.turns.length}
          subagentSession={{ interrupted: row.status === 'interrupted' }}
          hasOlder={conversation.hasOlder}
          loadingOlder={conversation.loadingOlder}
          onLoadOlder={() => void conversation.loadOlder()}
          prependToken={conversation.prependToken}
        />
      )}

      <div className="shrink-0 relative z-40 bg-canvas">
        <InputDock
          compact
          workspaceRoot={null}
          enabled={locatedSessionId !== null}
          busy={sending}
          placeholder={`把下一步交给 ${row.label}`}
          hint={
            conversation.working
              ? 'Enter 发给这个子代理 · Shift + Enter 换行 · 它的模型与思考强度由委派时决定'
              : 'Enter 发给这个子代理 · Shift + Enter 换行 · 界面显示它已收口，发送将由服务端裁决'
          }
          onSubmit={(text) => send(text)}
          error={sendError}
          working={null}
          onInterrupt={() => undefined}
        />
      </div>
    </div>
  );
};
