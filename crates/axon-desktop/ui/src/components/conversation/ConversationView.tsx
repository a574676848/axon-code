import React from 'react';
import * as ipc from '../../ipc';
import type { CronJob, WorkspaceItem } from '../../types';
import { targetOfSession, useConversation, type SessionSelection } from '../../conversation/useConversation';
import { sessionGenerationMetrics } from '../../conversation/sessionGeneration';
import { reflectionNoticeText } from '../../conversation/reflectionNotice';
import { useConversationObservations } from '../../conversation/useConversationObservations';
import { coverFace, showsHero, type StageFrame } from '../../conversation/stage';
import { useSessionAutoRead, useStopSpeechOnSessionChange } from '../../speech';
import { ConversationHeader } from './ConversationHeader';
import { SessionOriginBar } from './SessionOriginBar';
import { CAPABILITY_PENDING_PLACEHOLDER, READ_ONLY_PLACEHOLDER, UNREGISTERED_SESSION_PLACEHOLDER, sessionWriteGate, type SessionOriginBarState } from '../../sessionOpening';
import { ConversationHeaderActions } from './ConversationHeaderActions';
import { useSessionGoals } from './useSessionGoals';
import { useLiveJobIds } from './useLiveJobIds';
import { goalSessionSelection } from '../../sessionOpening';
import { useAutoReadReply } from './useAutoReadReply';
import { ConversationStream } from './ConversationStream';
import { ConversationCover } from './ConversationCover';
import { ConversationFailureCards } from './ConversationFailureCards';
import { InputDock } from './InputDock';
import { PlanDock } from './PlanDock';
import { SidebarRight } from '../../sidebarRight/SidebarRight';
import { useRailLayout } from '../../sidebarRight/useRailLayout';
import { ConversationActionsProvider, UnfoldingProvider } from './actions';
import { useConversationActionsValue } from './useConversationActionsValue';
import { sessionReferenceIndex } from './referenceAnchors';
import type { ConversationMcp } from '../../conversation/useConversationMcp';
import { changedFilesOf, workingLineOf } from '../../conversation/transcript';
import {
  isActiveSubagent,
  activeJobCount,
  type SessionRuntimeFacts,
} from '../../conversation/runtimeFacts';
import {
  resolveConfig,
  resolveEffectiveModel,
  type ResolvedConfig,
} from '../../conversation/viewConfig';
import { ConversationWidthControls } from './ConversationWidthControls';
import { useConversationWidth } from './useConversationWidth';
import { useExpiringNotice } from './composerNotice';
import { useSettledValue } from './conversationViewUtils';
import { useComposerPick } from './useComposerPick';
import { useReflectionNotice } from './useReflectionNotice';
import { useExplicitTurnSettings } from './useExplicitTurnSettings';
import { useOpeningStage } from './useOpeningStage';
import { useInterruptOnEscape, useRailShortcuts } from './useConversationShortcuts';
import { useSessionBootstrap } from './useSessionBootstrap';

/*
 * 主区域：首屏、对话过程与右侧栏的组合根。
 *
 * 这一层同时承担「会话目标」的补全职责：首屏直接输入时先建会话再提交，
 * 因此用户不必先点一次新会话才能开始。
 */

/** 后端配置里与指标条相关的键。 */
const CRON_DESTINATION_PREFIX = 'cron-';
/** 配置还没读回来时的模型目录：稳定的空清单，避免每帧重算继承值。 */
const NO_COMPOSER_MODELS: string[] = [];

/** 后台驱动的独立会话在输入坞里的禁用说明。 */
export interface ConversationViewProps {
  /** 当前选中的会话；为空时展示首屏空态。 */
  selection: SessionSelection | null;
  /** 当前工作区；用于占位提示与新建会话。 */
  workspace: WorkspaceItem | null;
  /** 配置版本号；设置保存后递增以触发复读。 */
  configRevision: number;
  /** 通知外层选中了新的会话。 */
  onSelectSession: (selection: SessionSelection) => void;
  /** 上报当前会话的运行时事实；外层据此判断归档是否需要二次确认。 */
  onRuntimeFacts?: (facts: SessionRuntimeFacts) => void;
  /** 会话来源返回条；从页面进入的后台会话才有。 */
  originBar?: SessionOriginBarState | null;
  /**
   * MCP 运行态。
   *
   * 读数已经搬去侧栏底栏的连接行，这里只留输入坞的 `/mcp` 需要的那一份执行入口；
   * 实例由外壳持有，因此重载一次之后侧栏读数与输入坞回执读的是同一份结论。
   */
  mcp: ConversationMcp;
}

export const ConversationView: React.FC<ConversationViewProps> = ({
  selection,
  workspace,
  configRevision,
  onSelectSession,
  onRuntimeFacts,
  originBar = null,
  mcp,
}) => {
  // 车道选择归一化成订阅目标：控制器本身与子代理会话视图共用同一个 hook。
  const target = React.useMemo(
    () => (selection === null ? null : targetOfSession(selection)),
    [selection],
  );
  /**
   * 车道快照里的轮数。
   *
   * 外部发起的新一轮（后台作业跑完的唤醒轮、定时投递的一轮）不在本视图的订阅里，
   * 唯一能说明「这条车道又多了一轮」的外部事实就是快照上的总数；车道不在侧栏
   * 登记时（定时任务这类独立会话）没有这份事实，传空。
   */
  const snapshotSessionId = selection?.sessionId ?? null;
  const sessionFacts = React.useMemo(
    () => {
      if (snapshotSessionId === null) return null;
      const session = workspace?.sessions.find((item) => item.id === snapshotSessionId);
      return session === undefined
        ? null
        : { sessionId: session.id, interactionCount: session.interaction_count };
    },
    [snapshotSessionId, workspace],
  );
  const conversation = useConversation(target, sessionFacts);
  // 右侧栏按会话持有停靠面：开关、tab 与分栏都在这一份状态里。
  const rail = useRailLayout(selection?.sessionId ?? null);
  /*
   * 语音播报按会话生效：偏好键由工作区与会话拼成，正文舞台与指标条里的
   * 开关读的都是这一个键，因此两处不会各说一套。
   */
  const sessionKey = selection === null ? null : `${selection.workspaceId}:${selection.sessionId}`;
  const voiceAutoRead = useSessionAutoRead(sessionKey);
  useStopSpeechOnSessionChange(sessionKey);
  // 一轮收口时朗读这一轮的回复：流式与收口补齐两种落地方式都覆盖。
  useAutoReadReply(sessionKey, conversation.working, conversation.state.turns, voiceAutoRead);
  const [config, setConfig] = React.useState<ResolvedConfig | null>(null);
  const [cronConfig, setCronConfig] = React.useState<CronJob | null>(null);
  const cronJobId = selection?.destination.startsWith(CRON_DESTINATION_PREFIX)
    ? selection.destination.slice(CRON_DESTINATION_PREFIX.length)
    : null;
  const fixedModel = (cronConfig?.jobId === cronJobId ? cronConfig?.model : null) ?? selection?.fixedModel;
  const fixedReasoningEffort = (cronConfig?.jobId === cronJobId ? cronConfig?.reasoningEffort : null) ?? selection?.fixedReasoningEffort;
  /**
   * 还没有正文的一屏：首屏本身，或应用内刚建好、还没有任何内容的那条会话。
   *
   * 这一屏还没有自己的运行记录，输入坞因此沿用上一次选过的模型与思考强度；
   * 历史会话不走这条路，它的权威模型来自该车道最新一轮的记录。
   */
  const heroSession =
    selection === null ||
    (selection.pending === true && conversation.state.turns.length === 0);
  const composerPick = useComposerPick(config?.models ?? NO_COMPOSER_MODELS);
  const { explicitModel, explicitReasoningEffort, onModelChange, onReasoningEffortChange } =
    useExplicitTurnSettings(sessionKey, {
      inherited: heroSession ? composerPick.inherited : null,
      /* 显式选择跟着配置走：停用供应商或删掉模型之后这一格要退回配置默认值。 */
      selectableModels: config?.models ?? null,
      onRememberModel: composerPick.rememberModel,
      onRememberReasoningEffort: composerPick.rememberReasoningEffort,
    });
  // 本会话真实跑过的模型（来自终态帧，回放历史时同样可得）：优先于配置默认值。
  /*
   * 指标条与输入坞共用的生效模型：展示名与最大上下文必须同源，否则会出现
   * 「模型是会话的、窗口却是 default_model 的」这种对不上的读数。
   */
  const effectiveModel = resolveEffectiveModel(
    config,
    fixedModel ?? null,
    explicitModel,
    conversation.sessionModel,
  );
  /*
   * 会话级读数：上下文快照与运行态证据都只经网关读，头部与换窗进度行读同一份快照。
   * 快照通常不带窗口字段，分母回落到当前生效模型的配置事实，因此这一组读数必须排在
   * `effectiveModel` 之后：回退值要是这一轮真正在跑的模型，不是配置默认模型。
   */
  const { contextSnapshot, evidence } = useConversationObservations(
    selection?.sessionId ?? null,
    conversation.working,
    conversation.state.turns.length,
    effectiveModel.contextWindow,
  );
  /**
   * 最近一次记忆反思提示：随首页历史带回，换目标即清空，翻页不回退。空白一律当没有。
   * 文案原样透传：运行侧的自述才是唯一权威口径，客户端不改写「本轮/上一轮」。
   */
  const memoryReflection = reflectionNoticeText(conversation.sessionReflectionNotice);
  const gate = sessionWriteGate(selection, conversation.sessionFacts, conversation.sessionFactsLoaded);
  /** 递增即要求定位到最新消息：发送消息后必须看到自己刚发的那条。 */
  const [followToken, setFollowToken] = React.useState(0);
  /** 正在查看的子代理；非空时主区域让给它的运行轨迹。 */
  /*
   * 打开的子代理：状态存整行，不只存标识。
   *
   * 名册补出来的行只存在于运行态名册里、不在转录投影里，只按标识回查会查不到，
   * 表现成「点了没反应」。同时保留标识，用于在转录投影刷新后取更新的那一行。
   */
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

  /*
   * 会话引导：首屏直接输入与语音起手都先建会话再提交，时序收在
   * `useSessionBootstrap` 里，本层只消费它的事实。
   */
  const { creating, createError, clearCreateError, submitFromHero, ensureVoiceSession } =
    useSessionBootstrap({ selection, workspace, conversation, onSelectSession });

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
   * 工作状态行在整轮运行期间都在：耗时与中断提示是这一行的职责。正文装载期间同样要它
   * 在场：那一句「正在加载会话…」是这段时间唯一的可见进展。文案只认 provider 阶段事实。
   */
  const workingLine =
    selection !== null && (conversation.working || conversation.loadingHistory)
      ? workingLineOf(runningTurn, conversation.queued, conversation.loadingHistory)
      : null;

  const { settling, slowOpening } = useOpeningStage(conversation.opening, sessionKey);

  /*
   * 轮后提示与正文同一时机交接。
   *
   * 它长在时间线尾部，比正文先换一行就会挂到上一屏的正文下面，那一瞬间读到的是
   * 另一条会话的记忆动作。
   */
  const settledReflection = useSettledValue(memoryReflection, settling);

  /** 亮 30 秒就退场：它是轮后注记，不是要一直挂在时间线尾部的状态。 */
  const visibleReflection = useReflectionNotice(settledReflection, sessionKey);

  /** 中断当前这一轮；Esc 与停止按钮共用同一个入口。 */
  const interruptTurn = conversation.interrupt;
  const turnRunning = conversation.working;

  /* Esc 统一中断：本轮在跑且服务端说可中断时才接管；空闲与装载中仍归浮层与草稿。 */
  useInterruptOnEscape(turnRunning && gate.canInterrupt && !conversation.loadingHistory, () => void interruptTurn());

  /**
   * 工作区根目录只取路径字符串。
   *
   * 工作区对象每换一次引用都让整条时间线重渲染是不划算的；只有路径真的变了，
   * 正文里那些相对图片路径才需要重新解析。
   */
  const workspacePath = workspace?.path ?? null;

  /** 会话控制器与右侧栏都只经 ref 取当前值：动作集合因此不会每帧重建。 */
  const conversationRef = React.useRef(conversation);
  conversationRef.current = conversation;
  const railRef = React.useRef(rail);
  railRef.current = rail;
  const actions = useConversationActionsValue(
    {
      submit: (text, attachments, structuredInput) => {
        void conversationRef.current.submit(
          text,
          [...attachments],
          structuredInput,
          explicitModel,
          explicitReasoningEffort,
        );
      },
      unfoldTurn: (interactionId) => conversationRef.current.unfoldTurn(interactionId),
    collapseTurn: (interactionId) => conversationRef.current.collapseTurn(interactionId),
      openResource: (address, params) => railRef.current.openResource(address, params),
      openQuestion: () => setQuestionReopenToken((value) => value + 1),
      // 正文里的相对路径要靠会话已知路径才能落到真路径上；真值在点击时从当前
      // 投影拉取，渲染期不遍历任何单元，动作集合也不会因此重建。
      referenceIndex: () => sessionReferenceIndex(conversationRef.current.state.turns),
    },
    workspacePath,
    selection?.sessionId ?? null,
  );

  /*
   * 右侧栏的两个入口键：工作区文件与新建终端。
   *
   * 沿用既有工作台产品的键位（工作区文件在桌面宿主里带 Alt，避免与浏览器/系统的打印键撞车；
   * 新建终端用 Ctrl+`，与主流终端产品一致）。绑定挂在窗口上：面板收起时也要能一键唤起。
   */
  useRailShortcuts(
    () => rail.openPage('files'),
    () => rail.newTerminal(),
  );

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

  // MCP 运行态与技能条数不在这里取：它们已经搬到侧栏底栏的连接行，读数由外壳持有。
  const liveHeaderMetrics = {
    working: conversation.working,
    animationTick: conversation.state.turns.length,
    model: effectiveModel.label,
    reasoningEffort: fixedReasoningEffort ?? config?.reasoningEffort ?? null,
    /* 首屏还没有会话：预算与会话读数一律不占位，只留运行态与辅助读数。 */
    inSession: selection !== null,
    /** 上下文读数取会话级快照：占用与窗口分母同源，未知就显示未知。 */
    contextSnapshot,
    /* 累计用量与累计耗时是整条会话的事实，由网关基线加本地增量合成，不随翻页或换轮退回。 */
    sessionCumulative: conversation.sessionCumulative,
    firstTokenAvgMs: sessionGeneration.firstTokenAvgMs,
    generationTpsX10: sessionGeneration.generationTpsX10,
    plan: conversation.state.plan,
    lspActiveCount: conversation.state.lspActiveCount,
  };

  /*
   * 交接之前，顶部这一行也保持上一屏。
   *
   * 指标条的各段里有六段（上下文、首字、速率、耗时、计划、LSP）以及右侧的后台任务与
   * 子代理入口，都是从投影里派生的；投影一空它们就整排消失再整排长回来，比正文的空档
   * 更显眼。输入坞的状态行同理：它一变高，正文视口也跟着抖一下。
   */
  const headerMetrics = useSettledValue(liveHeaderMetrics, settling);
  const runtimeFacts = useSettledValue(
    { subagents: conversation.state.subagents, jobs: conversation.jobs },
    settling,
  );
  const settledWorkingLine = useSettledValue(workingLine, settling);
  /*
   * 装载行不走交接冻结。
   *
   * 冻结是为了不让投影塌陷带着输入坞抖一下；但「正在加载会话…」说的正是正文还没到
   * 这件事，而装载窗口与交接窗口几乎完全重合，冻住上一屏的值等于把这句话永远压掉。
   * 证据过期时一律不显示「在跑」：此刻没有能指回带租约行的证据。
   */
  const dockWorking = evidence.showRunning
    ? conversation.loadingHistory ? workingLine : settledWorkingLine
    : null;
  /*
   * 计划坞同样读交接后的值：它长期占位在输入坞上方，切换会话时若直接读实时投影，
   * 会在新一屏的投影落地前先闪出上一条会话的计划。
   */
  const dockPlan = useSettledValue(conversation.state.plan, settling);

  const dockError = conversation.submitError ?? createError;
  /*
   * 输入坞上方那一行是「刚才这一次操作为什么没成」的回执，不是常驻状态：
   * 没送达的正文自己带未送达标记与重新发送入口，订阅与重连失败在正文里也有失败卡，
   * 因此这两个来源都到点自己退场，不长期占着输入框上方的位置。
   */
  useExpiringNotice(conversation.submitError, conversation.clearSubmitError);
  useExpiringNotice(createError, clearCreateError);

  /*
   * 子代理与主会话同一条路径：直接把它当成一条会话选中，历史索引、分页、过程折叠与
   * 输入坞都复用同一套实现。差别只在输入坞的控制（模型与思考强度只读），不再另立视图。
   */
  const openSubagentSession = React.useCallback(
    (row: import('../../conversation/transcript').SubagentRow) => {
      const sessionId = (row.sessionId ?? '').trim();
      if (sessionId.length === 0 || selection === null) return;
      onSelectSession({
        ...selection,
        sessionId,
        destination: '',
        title: row.label,
        channel: 'agent',
        backTo: selection,
        fixedModel: null,
        fixedReasoningEffort: null,
      });
    },
    [onSelectSession, selection],
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
  const selectedSessionId = selection?.sessionId ?? null;
  const liveJobIds = useLiveJobIds(selectedSessionId);
  const activeJobs = activeJobCount(conversation.jobs, liveJobIds);
  /** 本会话创建的目标清单；点其中的一行进入它的执行会话。 */
  const sessionGoals = useSessionGoals(selection?.sessionId ?? null, selection?.workspaceId ?? null);


  React.useEffect(() => {
    onRuntimeFacts?.({
      sessionId: selection?.sessionId ?? null,
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
    selection?.sessionId,
    stopRunning,
  ]);

  /*
   * 正文舞台：切换会话时正文层显示哪一屏。
   *
   * `settling` 期间保持上一屏：目标投影这时候要么还是空的，要么正一段段长出来，
   * 两者都不该上屏。取值按提交记录（过渡窗口内不刷新），因此切换那一帧取到的就是
   * 切换前正在看的那一屏。
   */
  const shown = useSettledValue<StageFrame>(
    { key: sessionKey, turns: conversation.state.turns },
    settling,
  );

  /**
   * 首屏条件：还没有选中会话，或者这一屏没有正文可看（判据见 `stage.ts` 的 `showsHero`）。
   *
   * 新建会话与「读完了仍然没有正文」都必须回落首屏，否则主区域只剩一片空白；
   * 输入坞已经指向当前会话，第一条消息发出去之后正文自然接管。
   */
  const heroVisible = showsHero({
    noSelection: selection === null,
    pending: selection?.pending === true,
    turns: conversation.state.turns.length,
    opening: conversation.opening,
    loadingHistory: conversation.loadingHistory,
    ready: conversation.sessionReadyId !== null,
  });
  const cover = coverFace(settling, slowOpening, heroVisible);
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
                sessionId={selection?.sessionId ?? null}
                working={conversation.working}
                sessionKey={sessionKey}
                onOpenSubagent={openSubagentSession}
                goals={sessionGoals}
                onOpenGoal={(goal) => {
                  onSelectSession(
                    goalSessionSelection(
                      {
                        workspaceId: goal.workspaceId ?? selection?.workspaceId ?? '',
                        workspacePath: goal.workspacePath ?? '',
                        sessionId: goal.latestExecution?.sessionId ?? '',
                        title: goal.title,
                      },
                      selection,
                    ),
                  );
                }}
                railExpanded={rail.layout.expanded}
                onToggleRail={rail.toggleExpanded}
              />
            }
          />
          {originBar !== null && <SessionOriginBar {...originBar} />}

          <ConversationFailureCards conversation={conversation} sessionKey={sessionKey} evidence={evidence} />

          <div className="flex-1 min-h-0 flex flex-col relative">
                        <>
                <div className="relative flex-1 min-h-0 flex flex-col" ref={contentStageRef}>
                  {/* 层内不重挂载：换会话只换内容，滚动容器与落点都留在原地。 */}
                  <div
                    data-conversation-stage="content"
                    className="flex-1 min-h-0 flex flex-col"
                    inert={cover !== 'none'}
                    aria-hidden={cover !== 'none'}
                  >
                    <UnfoldingProvider value={conversation.unfoldingId}>
                      <ConversationStream
                        sessionKey={shown.key}
                        turns={shown.turns}
                        followToken={followToken}
                        hasOlder={conversation.hasOlder}
                        loadingOlder={conversation.loadingOlder}
                        onLoadOlder={() => void conversation.loadOlder()}
                        prependToken={conversation.prependToken}
                        reflectionNotice={visibleReflection}
                        readOnly={gate.readOnly}
                      />
                    </UnfoldingProvider>
                  </div>

                  {/* 手柄只长在正文舞台的有效高度里，避免坞两侧留白变成隐形热区。 */}
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

            {/*
              输入坞是浮在画布上的一张卡，不再用一条全宽发丝线把它和正文切开：
              线横贯整宽时读起来像「另一块页面」，而卡片自己已经用描边与阴影分家。
              上方 24px 的静态渐隐只负责让滚动内容在贴到输入坞前先淡出（不监听滚动）。
              查看子代理轨迹时输入坞隐藏，避免让人以为能在这里继续对话。
            */}
            <div
              className={`relative z-40 shrink-0 bg-canvas`}
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
                sessionId={selection?.sessionId ?? ''}
                voiceTarget={{
                  ensureSession: ensureVoiceSession,
                }}
                enabled={gate.ready && !gate.readOnly && (workspace !== null || selection !== null)}
                readOnly={gate.readOnly}
                disabledPlaceholder={
                  gate.unregistered
                    ? UNREGISTERED_SESSION_PLACEHOLDER
                    : !gate.ready
                      ? CAPABILITY_PENDING_PLACEHOLDER
                      : gate.readOnly
                        ? READ_ONLY_PLACEHOLDER
                        : undefined
                }
                busy={conversation.submitting || creating}
                hintVisible={hintVisible}
                model={effectiveModel.label}
                models={config?.models ?? []}
                configLocked={
                  selection?.channel === 'cron' ||
                  cronJobId !== null ||
                  (conversation.sessionFacts?.kind ?? 'primary') !== 'primary'
                }
                reasoningEffort={
                  fixedReasoningEffort ?? explicitReasoningEffort ?? config?.reasoningEffort ?? null
                }
                onModelChange={onModelChange}
                onReasoningEffortChange={onReasoningEffortChange}
                onSubmit={
                  /*
                   * 首屏与草稿走同一条路：都没有真实会话可以写。
                   *
                   * 草稿的目标是服务端还没见过的一条占位，直接提交只会换来「会话不存在」；
                   * 因此先在提交前把它物化成真实会话（服务端生成标识），目标就绪后再发出这一轮。
                   */
                  selection === null || selection.draft === true
                    ? (text, attachments, structuredInput) => {
                        setFollowToken((value) => value + 1);
                        submitFromHero(
                          text,
                          attachments,
                          structuredInput,
                          explicitModel,
                          explicitReasoningEffort,
                        );
                      }
                    : (text, attachments, structuredInput) => {
                        setFollowToken((value) => value + 1);
                        void conversation.submit(
                          text,
                          attachments,
                          structuredInput,
                          explicitModel,
                          explicitReasoningEffort,
                        );
                      }
                }
                onRunCommand={mcp.run}
                error={dockError}
                working={
                  dockWorking === null
                    ? null
                    : { ...dockWorking, interruptible: dockWorking.interruptible && gate.canInterrupt }
                }
                onInterrupt={() => void conversation.interrupt()}
                sessionKey={sessionKey}
                activeQuestion={gate.answerable ? conversation.activeQuestion : null}
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
  </ConversationActionsProvider>
);
};
