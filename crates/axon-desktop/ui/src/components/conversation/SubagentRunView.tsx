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
  /** 模型展示名（配置里的别名）；缺省回落到运行侧记录的模型标识。 */
  modelLabel?: string | null;
  /** 该子代理所属工作区与车道；定位会话与投递消息都要。 */
  workspaceId: string | null;
  sessionId: string | null;
  /** 返回主会话。 */
  onBack: () => void;
}

export const SubagentRunView: React.FC<SubagentRunViewProps> = ({
  row,
  workspaceId,
  sessionId,
  modelLabel = null,
  onBack,
}) => {
  const [locatedSessionId, setLocatedSessionId] = React.useState<string | null>(null);
  const [title, setTitle] = React.useState<string | null>(null);
  const [model, setModel] = React.useState<string | null>(null);
  const [locateError, setLocateError] = React.useState<string | null>(null);
  const [sending, setSending] = React.useState(false);
  const [sendError, setSendError] = React.useState<string | null>(null);

  // 会话定位：子代理会话挂在车道的运行会话之下，用 slug 与界面上的 agent 路径对齐。
  // 名册已经把会话标识补给行时直接用，不再为此重拉一次名册。
  const knownSessionId = (row.sessionId ?? '').trim();
  React.useEffect(() => {
    if (knownSessionId.length > 0) {
      setLocatedSessionId(knownSessionId);
      // 名册行本身带着模型；标题没有更细的来源时退回 row.label，与转录行一致。
      setModel(row.model ?? null);
      setLocateError(null);
      return undefined;
    }
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
        if (wanted.length === 0) {
          setLocateError('这次委派没有记录代理路径，无法定位它的会话');
          return;
        }
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
  }, [knownSessionId, sessionId, row.agentId, row.model]);

  /*
   * 会话加载与主会话完全同一条路径：把子代理会话标识当作「这条会话」交给控制器，
   * 由它走同一套历史索引与尾部订阅。三个入口（主、cron、子代理）唯一的差别是输入坞的
   * 控制，正文取数不该有第二套实现——有第二套就会像修前那样，一轮 Interaction 都探
   * 不出来时整块正文空白。
   */
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

  /*
   * 正文既不该静默空白，也不该把「还没定位到」说成「没有内容」：
   * 目标建好、装载结束、一帧未渲染时才归因到「服务端没有给出帧」。
   */
  const blankAfterLoad =
    target !== null &&
    !conversation.loadingHistory &&
    conversation.state.turns.length === 0 &&
    conversation.state.failure === null;
  const failure =
    locateError ??
    conversation.state.failure?.message ??
    (blankAfterLoad ? '这条子代理的正文没有读到：服务端没有返回任何帧' : null);

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
        {/* 与主会话同一套输入坞：提示语、模型展示与键盘说明都读同一份实现。 */}
        <InputDock
          workspaceRoot={null}
          enabled={locatedSessionId !== null}
          busy={sending}
          configLocked
          model={modelLabel ?? model ?? ''}
          onSubmit={(text) => send(text)}
          error={sendError}
          working={null}
          onInterrupt={() => undefined}
        />
      </div>
    </div>
  );
};
