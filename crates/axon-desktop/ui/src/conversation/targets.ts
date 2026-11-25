/*
 * 会话目标与对外契约。
 *
 * 从 `useConversation` 里整块移出：这些是**类型与纯函数**，没有状态也没有副作用，
 * 放在编排文件里只会把它撑过行数上限，也让「目标是什么」这件事淹没在流程里。
 * 编排文件仍然原样再导出它们，调用方的引用不必改。
 */

import type { JobRow, AttachmentRef, ConversationState } from './transcript';

/** 当前选中的车道（一份完整的会话目标，含 UI 用的展示字段）。 */
export interface SessionSelection {
  workspaceId: string;
  workspacePath: string;
  /** 会话标识：主车道即车道本身，Cron/子代理为其独立会话标识。 */
  sessionId: string;
  destination: string;
  title: string;
  channel: string;
  /** 这条会话是否还没有任何内容（刚新建、尚未产生 Interaction）；首屏据此保留。 */
  pending?: boolean;
  /** 这条会话是否只是本机草稿：服务端还没有它，首条消息提交前必须先物化。 */
  draft?: boolean;
  fixedModel?: string | null;
  fixedReasoningEffort?: string | null;
  /**
   * 进入这条会话之前所在的位置。
   *
   * 后台会话（目标执行、定时任务）不是侧栏里的常驻条目，返回必须回到「从哪儿点进来的」：
   * 从会话里的目标清单进来就回那条会话，从看板进来就回看板。存的是可序列化的目标本身，
   * 不存回调，因此它跟着选择一起持久化也不会坏。
   */
  backTo?: SessionSelection | null;
}

/** 会话订阅目标：一条连接上的一路 Interaction 订阅。 */
export interface ConversationTarget {
  readonly kind: 'interaction';
  workspaceId: string;
  interactionId: string;
  /** 消息投递目标；车道专属，提交时必填。 */
  destination: string;
  /** 本机草稿：还没有任何服务端事实，历史索引一次都不该去读。 */
  draft?: boolean;
}

/**
 * 从选中会话得到订阅目标：一律 Interaction 目标。
 *
 * 目标执行、定时任务与子代理都有入站事实（`agent-dispatch` 行），正文的事实源与侧栏会话
 * 相同，因此不存在第二种目标：历史索引、分页、展开与逐轮读数只有一份实现。
 */
export function targetOfSession(selection: SessionSelection): ConversationTarget {
  return {
    kind: 'interaction',
    workspaceId: selection.workspaceId,
    interactionId: selection.sessionId,
    destination: selection.destination,
    draft: selection.draft === true,
  };
}

/** 归一化后的目标：工作区与「是哪一种目标」都只算一次。 */
export interface NormalizedTarget {
  kind: ConversationTarget['kind'];
  workspaceId: string;
}

/** 归一化订阅目标。 */
export function normalizeTarget(target: ConversationTarget): NormalizedTarget {
  return {
    kind: target.kind,
    workspaceId: target.workspaceId,
  };
}

/**
 * 车道能力门。
 *
 * 历史索引装载、提交、重试与中断都是车道专属能力；会话目标下这些能力一律
 * 「不可用」，并且只在提交入口处判断一次，不散落到主流程里。
 */
export interface SessionCapability {
  available: boolean;
  sessionId: string | null;
  workspaceId: string | null;
  destination: string | null;
}

/** 会话编排对外暴露的能力。 */
export interface ConversationController {
  state: ConversationState;
  jobs: JobRow[];
  working: boolean;
  /** 当前待决或挂起的模型提问。 */
  activeQuestion: import('./questions').PendingQuestion | null;
  /** 提交提问回答。 */
  answerQuestion: (callId: string, answers: import('./questions').AnswerItem[]) => Promise<void>;
  /**
   * 再向权威记录要一次问答结论。
   *
   * 时间线那条问答单元的「重新打开」用它：待答位可能是空的（历史回放只够画出单元），
   * 而占座卡只由待答位驱动，空位上的一次点击不取回结论就什么也不会发生。
   */
  reconcileQuestions: () => void;
  /** 当前订阅的 Interaction 标识；会话目标恒为 null。 */
  watchedInteractionId: string | null;
  /**
   * 正在展开的折叠轮标识；没有展开动作时为空。
   *
   * 过程控件据此把箭头换成转圈：展开要先回放一窗帧，这段空档必须有可见反馈，
   * 否则点下去的一秒里那一行毫无变化，读起来像点空了。
   */
  unfoldingId: string | null;
  /**
   * 整条会话的累计读数；基线还没取回时为 null。
   *
   * 口径是各轮输入、缓存读写、输出与推理之和，以及各轮墙钟耗时之和；与上下文读数不同：
   * 那个是「最近一次请求占了多少窗口」，这个是「这条会话一共花了多少」。打开历史会话即
   * 读作该会话既有消耗，在跑的那一轮边生成边进位。
   */
  sessionCumulative: import('./sessionUsage').SessionCumulative | null;
  /** 历史索引已经装载完成的车道标识；为空表示尚未就绪或目标不是车道。 */
  sessionReadyId: string | null;
  /** 车道当前模型的权威事实；网关只读该车道最新一轮记录，解析不到时为空。 */
  sessionModel: import('./viewConfig').SessionEffectiveModel | null;
  /**
   * 会话能力事实（可写性、可中断、可答问）。
   *
   * 首页历史索引到达后才有效；在此之前为空，界面按入口声明给首帧默认值。
   */
  sessionFacts: import('./sessionFacts').SessionFacts | null;
  /**
   * 能力事实是否已经取回。
   *
   * 与 `sessionFacts` 分开：`session` 块可以是 `null`（会话行不存在），那时「取回过」
   * 与「还没取」是两种不同的结论，界面必须能分开说。
   */
  sessionFactsLoaded: boolean;
  /**
   * 最近一次记忆反思的提示文本；该会话还没有记忆动作时为空。
   *
   * 与 `sessionModel` 同一时机取回：都是「这条车道现在能对外展示的最新事实」，
   * 不随翻页回到更早的轮次。反思是轮后异步治理的产物，按「上一次记忆动作」呈现。
   */
  sessionReflectionNotice: string | null;
  /**
   * 目标已切换、正文尚未交出。
   *
   * 回放是逐条 Interaction 拉起来的，逐段画出来就是肉眼可见的抖动；正文区据此在
   * 回放期间保持过渡面，回放落地后一次交出。
   */
  opening: boolean;
  /**
   * 正文还在装载：目标刚打开，或历史回放还没把 durable 尾交完。
   *
   * 这段时间里一个请求都还没发出，工作状态行据此写「正在加载会话…」；
   * 装载完成后才进入供应商阶段文案。
   */
  loadingHistory: boolean;
  /** 本次目标是否具备车道能力（提交、重试、中断）。 */
  sessionAvailable: boolean;
  /** 尚未被服务端确认的待发送消息条数。 */
  queued: number;
  submitting: boolean;
  submitError: string | null;
  /** 作答被服务端拒绝的原因（例如提问已在别处结算）；以弹窗告知，不与 `submitError` 混用。 */
  questionError: string | null;
  /** 关闭作答失败弹窗。 */
  dismissQuestionError: () => void;
  /** 更早的历史是否还没取完；取完即隐藏入口。 */
  hasOlder: boolean;
  /** 更早那一条是否在途。 */
  loadingOlder: boolean;
  /** 取更早的一条 Interaction；它的正文在独立投影里回放后整段前置。 */
  loadOlder: () => Promise<void>;
  /** 前置计数：正文区据此补偿滚动位置，翻页后阅读位置不跳。 */
  prependToken: number;
  /** 展开一条被折叠的 Interaction：按标识重新回放后把正文换回来。 */
  unfoldTurn: (interactionId: string) => void;
  /** 收起已展开的一轮：释放过程正文，只留抬头。 */
  collapseTurn: (interactionId: string) => void;
  /**
   * 按服务端判据收口这一轮。
   *
   * 运行证据说这一轮已经没有执行者时，界面不再只给一张「重启后台服务」的卡：
   * 收口判据由服务端给出（租约过期、首字超时、本来就不在途），这里只把结论落成终态。
   * 已经收口的轮次不受影响，因此重复调用安全。
   */
  settleSilentTurn: (interactionId: string, verdict: string) => void;
  /** 提交正文到当前目标；目标不具备车道能力时明确失败，不静默丢弃。 */
  submit: (
    text: string,
    attachments?: AttachmentRef[],
    structuredInput?: string,
    /** 本会话显式选定的模型；空表示由会话历史与配置决定。 */
    model?: string | null,
    /** 本会话显式选定的推理强度；空表示由 agent.reasoning_effort 决定。 */
    reasoningEffort?: string | null,
  ) => Promise<void>;
  /**
   * 接管网关语音回合：把已经提交的这一轮接进对话流并订阅增量。
   *
   * 语音定稿由网关直接提交，界面收到 `turn` 帧时才知道这条 Interaction，
   * 因此需要一条显式的接入通道，而不是伪造一次本地提交。
   */
  attachVoiceTurn: (interactionId: string, utterance: string) => Promise<void>;
  retry: () => void;
  /** 中断正在运行的一轮。 */
  interrupt: () => Promise<void>;
  clearSubmitError: () => void;
}
