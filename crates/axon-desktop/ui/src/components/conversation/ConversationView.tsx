import React from 'react';
import * as ipc from '../../ipc';
import type { CronJob, WorkspaceItem } from '../../types';
import { targetOfLane, useConversation, type LaneSelection } from '../../conversation/useConversation';
import { sessionGenerationMetrics } from '../../conversation/sessionGeneration';
import { coverFace, type StageFrame } from '../../conversation/stage';
import { useLaneAutoRead, useStopSpeechOnLaneChange } from '../../speech';
import { Dialog } from '../ui/overlays';
import { ConversationHeader } from './ConversationHeader';
import { ConversationHeaderActions } from './ConversationHeaderActions';
import { useAutoReadReply } from './useAutoReadReply';
import { ConversationStream } from './ConversationStream';
import { ConversationCover } from './ConversationCover';
import { ErrorCard } from './ErrorCard';
import { InputDock } from './InputDock';
import { PlanDock } from './PlanDock';
import { SidebarRight } from '../../sidebarRight/SidebarRight';
import { changesAddress, fileAddress } from '../../sidebarRight/registry';
import { useRailLayout } from '../../sidebarRight/useRailLayout';
import { ConversationActionsProvider, type ConversationActions } from './actions';
import { structuredInputBlock } from '../../conversation/references';
import { useConversationMcp } from '../../conversation/useConversationMcp';
import { useTurnModelRoute } from '../../conversation/useTurnModelRoute';
import {
  changedFilesOf,
  workingStatusPrimary,
  type AttachmentRef,
} from '../../conversation/transcript';
import {
  isActiveJob,
  isActiveSubagent,
  type LaneRuntimeFacts,
} from '../../conversation/runtimeFacts';
import { SubagentRunView } from './SubagentRunView';
import {
  KEY_DEFAULT_MODEL,
  KEY_REASONING_EFFORT,
  resolveConfig,
  type ResolvedConfig,
} from '../../conversation/viewConfig';
import { ConversationWidthControls } from './ConversationWidthControls';
import { useConversationWidth } from './useConversationWidth';
import { useExpiringNotice } from './composerNotice';
import { escOwnedByAnotherLayer, useSettledValue } from './conversationViewUtils';

/*
 * 主区域：首屏、对话过程与右侧栏的组合根。
 *
 * 这一层同时承担「会话目标」的补全职责：首屏直接输入时先建会话再提交，
 * 因此用户不必先点一次新会话才能开始。
 */

/** 后端配置里与指标条相关的键。 */
const CRON_DESTINATION_PREFIX = 'cron-';
/** 打开慢过这个时长才亮过渡面：快到看不出来时上一屏继续显示，不做多余的闪动。 */
const OPENING_SURFACE_DELAY_MS = 140;
/** 过渡面亮起后至少停留的时长：才亮就撤会读成一次闪动。 */
const OPENING_SURFACE_MIN_MS = 160;

export interface ConversationViewProps {
  /** 当前选中的会话；为空时展示首屏空态。 */
  selection: LaneSelection | null;
  /** 当前工作区；用于占位提示与新建会话。 */
  workspace: WorkspaceItem | null;
  /** 配置版本号；设置保存后递增以触发复读。 */
  configRevision: number;
  /** 通知外层选中了新的会话。 */
  onSelectLane: (selection: LaneSelection) => void;
  /** 上报当前会话的运行时事实；外层据此判断归档是否需要二次确认。 */
  onRuntimeFacts?: (facts: LaneRuntimeFacts) => void;
}

export const ConversationView: React.FC<ConversationViewProps> = ({
  selection,
  workspace,
  configRevision,
  onSelectLane,
  onRuntimeFacts,
}) => {
  // 车道选择归一化成订阅目标：控制器本身与子代理会话视图共用同一个 hook。
  const target = React.useMemo(
    () => (selection === null ? null : targetOfLane(selection)),
    [selection],
  );
  const conversation = useConversation(target);
  // 右侧栏按会话持有停靠面：开关、tab 与分栏都在这一份状态里。
  const rail = useRailLayout(selection?.laneId ?? null);
  /*
   * 语音播报按会话生效：偏好键由工作区与会话拼成，正文舞台与指标条里的
   * 开关读的都是这一个键，因此两处不会各说一套。
   */
  const laneKey = selection === null ? null : `${selection.workspaceId}:${selection.laneId}`;
  const voiceAutoRead = useLaneAutoRead(laneKey);
  useStopSpeechOnLaneChange(laneKey);
  // 一轮收口时朗读这一轮的回复：流式与收口补齐两种落地方式都覆盖。
  useAutoReadReply(laneKey, conversation.working, conversation.state.turns, voiceAutoRead);
  const [config, setConfig] = React.useState<ResolvedConfig | null>(null);
  const [cronConfig, setCronConfig] = React.useState<CronJob | null>(null);
  const cronJobId = selection?.destination.startsWith(CRON_DESTINATION_PREFIX)
    ? selection.destination.slice(CRON_DESTINATION_PREFIX.length)
    : null;
  const fixedModel = (cronConfig?.jobId === cronJobId ? cronConfig?.model : null) ?? selection?.fixedModel;
  const fixedReasoningEffort = (cronConfig?.jobId === cronJobId ? cronConfig?.reasoningEffort : null) ?? selection?.fixedReasoningEffort;
  const [creating, setCreating] = React.useState(false);
  const [createError, setCreateError] = React.useState<string | null>(null);
  const [configError, setConfigError] = React.useState<string | null>(null);
  const [savingConfig, setSavingConfig] = React.useState(false);
  const configWritePending = React.useRef(false);
  /** 递增即要求定位到最新消息：发送消息后必须看到自己刚发的那条。 */
  const [followToken, setFollowToken] = React.useState(0);
  /** 正在查看的子代理；非空时主区域让给它的运行轨迹。 */
  const [openSubagentId, setOpenSubagentId] = React.useState<string | null>(null);
  /** 首屏直接输入时暂存的正文与附件；等新会话的历史索引装载完成后再发出。 */
  const pendingHeroText = React.useRef<{
    text: string;
    attachments: AttachmentRef[];
    /** 提交时附在正文之后的结构化输入块（`@` 文件引用）；为空表示没有引用。 */
    structuredInput: string;
  } | null>(null);
  /** 重新打开卡片的请求序号：时间线上的入口每点一次递增一次。 */
  const [questionReopenToken, setQuestionReopenToken] = React.useState(0);
  /** 正文舞台容器：供宽度拖拽手柄量测宽度并注入 CSS 变量。 */
  const contentStageRef = React.useRef<HTMLDivElement | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    void ipc
      .readConfig()
      .then((value) => {
        if (!cancelled) setConfig(resolveConfig(value, fixedModel));
      })
      .catch(() => {
        // 配置读不到只影响指标条上的模型与思考段，不影响会话本身。
      });
    return () => {
      cancelled = true;
    };
  }, [configRevision, fixedModel]);

  React.useEffect(() => {
    let cancelled = false;
    setCronConfig(null);
    if (cronJobId !== null) {
      void ipc.cronGet(cronJobId).then((job) => {
        if (!cancelled) setCronConfig(job);
      }).catch(() => {
        // 已移除的任务仍可查看历史，会话配置由后端继续锁定。
      });
    }
    return () => { cancelled = true; };
  }, [cronJobId]);

  /** 写回一个配置项并复读，供模型与推理等级切换共用。 */
  const applyConfig = React.useCallback(async (values: Record<string, string>) => {
    if (configWritePending.current) return;
    configWritePending.current = true;
    setSavingConfig(true);
    setConfigError(null);
    try {
      await ipc.applyConfigValues(values);
      setConfig(resolveConfig(await ipc.readConfig()));
    } catch (error) {
      setConfigError(error instanceof Error ? error.message : '保存配置失败');
    } finally {
      configWritePending.current = false;
      setSavingConfig(false);
    }
  }, []);

  /** 新建一条会话并把它设为当前目标。 */
  const createSession = React.useCallback(async (): Promise<LaneSelection | null> => {
    if (workspace === null) {
      setCreateError('请先在左侧添加一个工作区');
      return null;
    }
    setCreating(true);
    setCreateError(null);
    try {
      const list = await ipc.createLane(workspace.id);
      const target = list.find((item) => item.id === workspace.id) ?? list[0];
      const pending = (target?.lanes ?? []).filter((lane) => lane.pending);
      const created = pending.sort((left, right) => right.created_at - left.created_at)[0];
      if (created === undefined) {
        setCreateError('新建会话失败：后端未返回新车道');
        return null;
      }
      const next: LaneSelection = {
        workspaceId: workspace.id,
        workspacePath: workspace.path,
        laneId: created.id,
        destination: created.destination,
        title: created.title,
        channel: created.channel,
        // 刚建好的会话还没有内容：主区域继续留在首屏，直到第一条消息发出。
        pending: true,
      };
      onSelectLane(next);
      return next;
    } catch (error) {
      setCreateError(error instanceof Error ? error.message : '新建会话失败');
      return null;
    } finally {
      setCreating(false);
    }
  }, [onSelectLane, workspace]);

  /** 首屏直接提交：先建会话，等这一轮目标就绪后再发送。 */
  const submitFromHero = React.useCallback(
    (text: string, attachments: AttachmentRef[], structuredInput: string) => {
      pendingHeroText.current = { text, attachments, structuredInput };
      void createSession().then((target) => {
        if (target === null) pendingHeroText.current = null;
      });
    },
    [createSession],
  );

  // 目标就绪才发送：新建会话会切换当前车道并触发一次投影重置，
  // 若在重置之前把消息发出去，这一轮的本地消息与流式内容都会被重置吃掉。
  React.useEffect(() => {
    const pending = pendingHeroText.current;
    if (pending === null || selection === null) return;
    if (conversation.laneReadyId !== selection.laneId) return;
    pendingHeroText.current = null;
    void conversation.submit(pending.text, pending.attachments, pending.structuredInput);
  }, [conversation, selection]);

  const lastTurn = conversation.state.turns[conversation.state.turns.length - 1] ?? null;
  /**
   * 工作状态行的主语：最后一条真正在跑的轮次。
   *
   * 不取「数组最后一条」：一条发送失败的本地轮次也会排在末尾，用它会让状态行
   * 显示错误的阶段与耗时。
   */
  const runningTurn =
    [...conversation.state.turns]
      .reverse()
      .find((turn) => turn.status === 'pending' || turn.status === 'streaming') ?? null;
  /*
   * 工作状态行在整轮运行期间都在：耗时与中断提示是这一行的职责，
   * 助手正文开始流式输出并不改变「这一轮还在跑」这个事实。
   */
  const workingRoute = useTurnModelRoute(
    runningTurn?.id ?? null,
    runningTurn?.activity ?? null,
    config?.route ?? config?.model ?? '',
  );
  const workingLine =
    conversation.working && selection !== null
      ? {
          primary: workingStatusPrimary(runningTurn?.activity ?? null, workingRoute),
          elapsedMs: runningTurn?.elapsedMs ?? null,
          queued: conversation.queued,
        }
      : null;

  /*
   * 过渡相位：回放还没落地，或过渡面还没停够时长。
   *
   * 后半句是必需的：慢了才亮、亮了就撤，会让过渡面自己变成一次闪动。停够时长再交出
   * 正文，回放的快慢就只体现在它停留的长短上。
   */
  const [slowOpening, setSlowOpening] = React.useState(false);
  const [coverHeld, setCoverHeld] = React.useState(false);
  React.useEffect(() => {
    if (!conversation.opening) {
      setSlowOpening(false);
      setCoverHeld(false);
      return undefined;
    }
    // 慢过阈值才亮过渡面；亮起后至少停留一个下限，不做「亮了就撤」。
    const show = window.setTimeout(() => setSlowOpening(true), OPENING_SURFACE_DELAY_MS);
    const hold = window.setTimeout(
      () => setCoverHeld(true),
      OPENING_SURFACE_DELAY_MS + OPENING_SURFACE_MIN_MS,
    );
    return () => {
      window.clearTimeout(show);
      window.clearTimeout(hold);
    };
  }, [conversation.opening]);
  const settling = conversation.opening || (slowOpening && !coverHeld);

  /** 中断当前这一轮；Esc 与停止按钮共用同一个入口。 */
  const interruptTurn = conversation.interrupt;
  const turnRunning = conversation.working;

  /*
   * Esc 统一中断：与 TUI 同语义。本轮跑着的时候，Esc 就是中断当前请求，
   * 不需要先找到停止按钮。空闲时不接管这一键，它仍归各浮层与草稿使用。
   */
  React.useEffect(() => {
    if (!turnRunning) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing) return;
      // 长按 Esc 会连发 keydown；每一次都发中断请求只会得到一串「无需中断」。
      if (event.repeat) return;
      if (escOwnedByAnotherLayer(event.target)) return;
      event.preventDefault();
      void interruptTurn();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [interruptTurn, turnRunning]);

  /** 行内动作：重发消息、打开文件、打开某一轮改动对比；控制器经 ref 取当前值。 */
  const conversationRef = React.useRef(conversation);
  conversationRef.current = conversation;
  const actions = React.useMemo<ConversationActions>(
    () => ({
      retry: (text: string, attachments: readonly AttachmentRef[]) => {
        // 附件重新走一次结构化输入块：只带图片的消息没有正文，块就是它的正文。
        const paths = attachments.map((attachment) => attachment.path);
        void conversationRef.current.submit(
          text,
          [...attachments],
          structuredInputBlock(text, [], paths),
        );
      },
      openFile: (path: string) => {
        // 打开文件即打开对应的资源 tab：面板会自己展开，不需要先切视图。
        rail.openResource(fileAddress(path));
      },
      openChanges: (turnId: string, turnNumber: number, index: number) => {
        // 改动对比按「轮次」落成资源地址：文件下标是导航参数，因此同一个 tab 反复
        // 打开不同文件时只会切换落点，不会堆出一排标签。
        rail.openResource(changesAddress(turnId, turnNumber), { index });
      },
      // 卡片只有一张，而且只承载当前待答的那条提问：入口因此只需把卡片叫回来。
      openQuestion: () => setQuestionReopenToken((value) => value + 1),
      // 折叠行自带标识：展开时按它重新回放那一条 Interaction。
      unfoldTurn: (interactionId: string) => conversationRef.current.unfoldTurn(interactionId),
      // 正文里的相对图片路径以当前工作区为准，与输入坞的 `@` 引用同作用域。
      workspaceRoot: workspace?.path ?? null,
    }),
    [rail, workspace],
  );

  /*
   * 右侧栏的两个入口键：工作区文件与新建终端。
   *
   * 沿用既有工作台产品的键位（工作区文件在桌面宿主里带 Alt，避免与浏览器/系统的打印键撞车；
   * 新建终端用 Ctrl+`，与主流终端产品一致）。绑定挂在窗口上：面板收起时也要能一键唤起。
   */
  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      if (event.altKey && event.key.toLowerCase() === 'p') {
        event.preventDefault();
        rail.openPage('files');
        return;
      }
      if (event.altKey || event.shiftKey) return;
      if (event.key === '`' || event.code === 'Backquote') {
        event.preventDefault();
        rail.newTerminal();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [rail]);

  /**
   * 某一轮的改动文件。
   *
   * 事实源是对话投影，因此这里只做一次查找：改动不走工作区文件读取，
   * 文件此后又被改过也不会篡改这一轮已经发生的事实。
   */
  const changesFor = React.useCallback(
    (turnId: string) => {
      const turn = conversation.state.turns.find((item) => item.id === turnId);
      return turn === undefined ? null : changedFilesOf(turn.changes);
    },
    [conversation.state.turns],
  );

  // 首字与速率按整个会话重算：单看最近一轮会把早期慢轮次抹掉，
  // 在跑轮的首字帧一到就参与平均，不必等它收口。
  const sessionGeneration = React.useMemo(
    () => sessionGenerationMetrics(conversation.state.turns),
    [conversation.state.turns],
  );

  // MCP 运行态：指标条读数与输入坞的 `/mcp` 同源（见 useConversationMcp）。
  const mcp = useConversationMcp(workspace?.path ?? null);

  const liveHeaderMetrics = {
    working: conversation.working,
    animationTick: conversation.state.turns.length,
    model: fixedModel ?? config?.model ?? '',
    cwd: workspace?.path ?? '',
    reasoningEffort: fixedReasoningEffort ?? config?.reasoningEffort ?? null,
    usage: lastTurn?.usage ?? null,
    contextWindow: config?.contextWindow ?? null,
    firstTokenAvgMs: sessionGeneration.firstTokenAvgMs,
    generationTpsX10: sessionGeneration.generationTpsX10,
    elapsedMs: lastTurn?.elapsedMs ?? null,
    plan: conversation.state.plan,
    lspActiveCount: conversation.state.lspActiveCount,
    mcp: mcp.reading,
  };

  /*
   * 交接之前，顶部这一行也保持上一屏。
   *
   * 指标条的十段里有六段（上下文、首字、速率、耗时、计划、LSP）以及右侧的后台任务与
   * 子代理入口，都是从投影里派生的；投影一空它们就整排消失再整排长回来，比正文的空档
   * 更显眼。输入坞的状态行同理：它一变高，正文视口也跟着抖一下。
   */
  const headerMetrics = useSettledValue(liveHeaderMetrics, settling);
  const runtimeFacts = useSettledValue(
    { subagents: conversation.state.subagents, jobs: conversation.jobs },
    settling,
  );
  const dockWorking = useSettledValue(workingLine, settling);
  /*
   * 计划坞同样读交接后的值：它长期占位在输入坞上方，切换会话时若直接读实时投影，
   * 会在新一屏的投影落地前先闪出上一条会话的计划。
   */
  const dockPlan = useSettledValue(conversation.state.plan, settling);

  const failure = conversation.state.failure;
  const dockError = conversation.submitError ?? createError;
  /*
   * 输入坞上方那一行是「刚才这一次操作为什么没成」的回执，不是常驻状态：
   * 没送达的正文自己带未送达标记与重新发送入口，订阅与重连失败在正文里也有失败卡，
   * 因此这两个来源都到点自己退场，不长期占着输入框上方的位置。
   */
  useExpiringNotice(conversation.submitError, conversation.clearSubmitError);
  useExpiringNotice(createError, () => setCreateError(null));

  const openSubagent = React.useMemo(
    () =>
      openSubagentId === null
        ? null
        : (conversation.state.subagents.find((row) => row.agentId === openSubagentId) ?? null),
    [conversation.state.subagents, openSubagentId],
  );

  /** 停止本轮：控制器经 ref 取，避免回调每轮换引用而让上报 effect 自激。 */
  const interruptRef = React.useRef(conversation.interrupt);
  interruptRef.current = conversation.interrupt;
  const stopRunning = React.useCallback(async () => {
    await interruptRef.current();
  }, []);

  const activeSubagents = conversation.state.subagents.filter((row) =>
    isActiveSubagent(row.status),
  ).length;
  const activeJobs = conversation.jobs.filter((job) => isActiveJob(job.status)).length;

  React.useEffect(() => {
    onRuntimeFacts?.({
      laneId: selection?.laneId ?? null,
      turnRunning: conversation.working,
      activeSubagents,
      activeJobs,
      // 待答标识的清除条件：这条会话已经没有在等用户回答的提问了。
      hasPendingQuestion: conversation.activeQuestion !== null,
      stop: stopRunning,
    });
  }, [
    activeJobs,
    activeSubagents,
    conversation.activeQuestion,
    conversation.working,
    onRuntimeFacts,
    selection?.laneId,
    stopRunning,
  ]);

  /**
   * 首屏条件：还没有选中会话，或者选中的是一条刚建好、还没有任何内容的会话。
   *
   * 新建会话不该把主区域变成一片空白：那里仍应是「标志 + 标语」的首屏，
   * 而输入坞已经指向这条新会话，第一条消息发出去之后正文自然接管。
   */
  const blankSession =
    selection !== null && selection.pending === true && conversation.state.turns.length === 0;

  /*
   * 正文舞台：切换会话时正文层显示哪一屏。
   *
   * `settling` 期间保持上一屏：目标投影这时候要么还是空的，要么正一段段长出来，
   * 两者都不该上屏。取值按提交记录（过渡窗口内不刷新），因此切换那一帧取到的就是
   * 切换前正在看的那一屏。
   */
  const shown = useSettledValue<StageFrame>(
    { key: laneKey, turns: conversation.state.turns },
    settling,
  );

  /** 还没有正文可看的两种情形：没选会话，或这条会话刚建好还没有内容。 */
  const heroVisible = selection === null || blankSession;
  const cover = coverFace(settling, shown.turns.length > 0, slowOpening, heroVisible);
  /*
   * 过渡面淡出期间保持上一份内容。
   *
   * 内容与透明度同时变，叠化会读成一次塌陷（先塌成空画布再长出正文），因此内容
   * 只在出场时更新，收起时冻结。
   */
  const [coverContent, setCoverContent] = React.useState<'hero' | 'skeleton'>('hero');
  if (cover !== 'none' && cover !== coverContent) setCoverContent(cover);
  /*
   * 列宽在视图层解析：与 cover 是 hero 还是 none 无关，首屏、正文、输入坞、
   * 计划坞与两侧手柄因此始终读同一个 `--axon-chat-user-width`。
   */
  const chatWidth = useConversationWidth(contentStageRef, cover === 'none');
  /** 键盘提示只在列宽够宽时出现：窄到 472px 时那一行会先折行再被裁。 */
  const hintVisible = chatWidth.width >= 720;

  return (
    <ConversationActionsProvider value={actions}>
      {/*
        外壳层一律用 overflow-clip 而不是 overflow-hidden：防止焦点滚动推偏顶栏。
      */}
      <div className="flex-1 min-w-0 flex h-full overflow-clip">
        <main className="flex-1 min-w-0 flex flex-col h-full overflow-clip bg-canvas">
          <ConversationHeader
            {...headerMetrics}
            hasPendingQuestion={Boolean(conversation.activeQuestion)}
            avoidWindowControls={!rail.layout.expanded}
            trailing={
              <ConversationHeaderActions
                subagents={runtimeFacts.subagents}
                jobs={runtimeFacts.jobs}
                laneId={selection?.laneId ?? null}
                laneKey={laneKey}
                onOpenSubagent={setOpenSubagentId}
                railExpanded={rail.layout.expanded}
                onToggleRail={rail.toggleExpanded}
              />
            }
          />

          {failure !== null && (
            <div className="shrink-0 px-4 pt-1">
              <ErrorCard
                title="会话事件流中断"
                reason={failure.message}
                retryable={failure.retryable}
                meta={['事件订阅已停止，界面不会继续刷新']}
                action={
                  failure.retryable
                    ? { label: '重连', onClick: () => void conversation.retry() }
                    : undefined
                }
              />
            </div>
          )}

          <div className="flex-1 min-h-0 flex flex-col relative">
            {openSubagent !== null ? (
              <SubagentRunView
                row={openSubagent}
                workspaceId={selection?.workspaceId ?? null}
                laneId={selection?.laneId ?? null}
                onBack={() => setOpenSubagentId(null)}
              />
            ) : (
              /*
               * 正文舞台：正文层常驻，过渡面浮在整列之上。
               *
               * 投影在切换目标的那一帧就作废，正文又要等历史索引与逐条回放，直接按
               * 投影渲染会先空一屏再一段段长出来。这里让正文层在回放期间继续显示上一屏
               * 或首屏，回放落地后一次交出，中间态不出现在屏幕上。
               */
              <>
                <div className="relative flex-1 min-h-0 flex flex-col" ref={contentStageRef}>
                  {/* 层内不重挂载：换会话只换内容，滚动容器与落点都留在原地。 */}
                  <div
                    data-conversation-stage="content"
                    className="flex-1 min-h-0 flex flex-col"
                    inert={cover !== 'none'}
                    aria-hidden={cover !== 'none'}
                  >
                    <ConversationStream
                      sessionKey={shown.key}
                      turns={shown.turns}
                      followToken={followToken}
                      hasOlder={conversation.hasOlder}
                      loadingOlder={conversation.loadingOlder}
                      onLoadOlder={() => void conversation.loadOlder()}
                      prependToken={conversation.prependToken}
                    />
                  </div>

                  {/*
                    手柄只长在正文舞台的有效高度里：跨到输入坞上方时，
                    坞两侧的留白会变成一块 col-resize 的隐形热区。
                  */}
                  {cover === 'none' && chatWidth.handlesVisible && (
                    <ConversationWidthControls
                      onStart={chatWidth.startDrag}
                      onDrag={chatWidth.drag}
                      onCommit={chatWidth.commit}
                      onReset={chatWidth.reset}
                    />
                  )}
                </div>

                {/*
                  过渡面与首屏共用一层：面本身只做透明度叠化，位移留给正文的入场，
                  两层不会同时动。常驻挂载，交接时才有一段 150ms 的淡出可看；
                  none 态整层透明且不接收指针，不影响下方的输入坞。
                  挂在「正文 + 输入坞」整列上而不是正文舞台内，
                  首屏的重心因此不随输入坞长高而上跳。
                */}
                <ConversationCover cover={cover} content={coverContent} />
              </>
            )}

            {/*
              输入坞是浮在画布上的一张卡，不再用一条全宽发丝线把它和正文切开：
              线横贯整宽时读起来像「另一块页面」，而卡片自己已经用描边与阴影分家。
              上方 24px 的静态渐隐只负责让滚动内容在贴到输入坞前先淡出（不监听滚动）。
              查看子代理轨迹时输入坞隐藏，避免让人以为能在这里继续对话。
            */}
            <div
              className={`relative z-40 shrink-0 bg-canvas ${
                openSubagent === null ? '' : 'hidden'
              }`}
            >
              <div
                aria-hidden="true"
                className="pointer-events-none absolute inset-x-0 -top-6 h-6"
                style={{
                  backgroundImage: 'linear-gradient(to top, var(--color-canvas), transparent)',
                }}
              />
              {/*
                固定计划坞：会话级「当前计划」，与时间线里的计划单元分工不同。
                钉在输入坞正上方，随会话重挂载，展开态不跨会话沿用。
              */}
              <PlanDock key={shown.key} plan={dockPlan} />
              <InputDock
                workspaceRoot={workspace?.path ?? null}
                workspaceId={selection?.workspaceId ?? workspace?.id ?? null}
                replyDestination={selection?.destination ?? ''}
                enabled={workspace !== null || selection !== null}
                busy={conversation.submitting || creating}
                hintVisible={hintVisible}
                model={fixedModel ?? config?.model ?? ''}
                models={config?.models ?? []}
                configBusy={savingConfig}
                configLocked={selection?.channel === 'cron' || cronJobId !== null}
                reasoningEffort={fixedReasoningEffort ?? config?.reasoningEffort ?? null}
                onModelChange={(value) => void applyConfig({ [KEY_DEFAULT_MODEL]: value })}
                onReasoningEffortChange={(value) =>
                  void applyConfig({ [KEY_REASONING_EFFORT]: value })
                }
                onSubmit={
                  selection === null
                    ? (text, attachments, structuredInput) => {
                        setFollowToken((value) => value + 1);
                        submitFromHero(text, attachments, structuredInput);
                      }
                    : (text, attachments, structuredInput) => {
                        setFollowToken((value) => value + 1);
                        void conversation.submit(text, attachments, structuredInput);
                      }
                }
                onRunCommand={mcp.run}
                error={dockError}
                working={dockWorking}
                onInterrupt={() => void conversation.interrupt()}
                activeQuestion={conversation.activeQuestion}
                onAnswerQuestion={conversation.answerQuestion}
                reopenToken={questionReopenToken}
                questionError={conversation.questionError}
                onDismissQuestionError={conversation.dismissQuestionError}
              />
            </div>
          </div>
        </main>

        {/*
          收起不卸载：面板自己贴右缘滑出去、宽度归零不占正文宽，
          因此这里不按展开态裁剪，文件树的展开态与预览的滚动位置才留得住。
        */}
        <SidebarRight
          controller={rail}
          workspaceRoot={workspace?.path ?? null}
          changesFor={changesFor}
        />
      </div>
     <Dialog
       open={configError !== null}
       onClose={() => setConfigError(null)}
       title="保存配置失败"
       size="sm"
       primary={{ label: '知道了', onClick: () => setConfigError(null) }}
     >
       <p className="text-sm text-danger whitespace-pre-wrap break-words">{configError}</p>
     </Dialog>
   </ConversationActionsProvider>
 );
};
