/*
 * 会话目标与对外契约。
 *
 * 从 `useConversation` 里整块移出：这些是**类型与纯函数**，没有状态也没有副作用，
 * 放在编排文件里只会把它撑过行数上限，也让「目标是什么」这件事淹没在流程里。
 * 编排文件仍然原样再导出它们，调用方的引用不必改。
 */

import type { JobRow, AttachmentRef, ConversationState } from './transcript';

/** 当前选中的车道（一份完整的会话目标，含 UI 用的展示字段）。 */
export interface LaneSelection {
  workspaceId: string;
  workspacePath: string;
  laneId: string;
  destination: string;
  title: string;
  channel: string;
  /** 这条会话是否还没有任何内容（刚新建、尚未产生 Interaction）；首屏据此保留。 */
  pending?: boolean;
  fixedModel?: string | null;
  fixedReasoningEffort?: string | null;
  /** Cron 或子代理的独立会话标识；存在时按 session 目标订阅。 */
  sessionId?: string;
}

/** 会话订阅目标：一条连接上的一路订阅。 */
export type ConversationTarget =
  | {
      kind: 'interaction';
      workspaceId: string;
      interactionId: string;
      /** 消息投递目标；车道专属，提交时必填。 */
      destination: string;
    }
  | { kind: 'session'; workspaceId: string; sessionId: string };

/** 从车道选择得到订阅目标。 */
export function targetOfLane(selection: LaneSelection): ConversationTarget {
  if (selection.sessionId !== undefined && selection.sessionId.length > 0) {
    return { kind: 'session', workspaceId: selection.workspaceId, sessionId: selection.sessionId };
  }
  return {
    kind: 'interaction',
    workspaceId: selection.workspaceId,
    interactionId: selection.laneId,
    destination: selection.destination,
  };
}

/** 归一化后的目标：工作区与「是哪一种目标」都只算一次。 */
export interface NormalizedTarget {
  kind: ConversationTarget['kind'];
  workspaceId: string;
}

/**
 * 目标对应的订阅标识；必须与 Rust 侧 `SubscribeTarget::subscribe_id` 逐字符一致。
 *
 * 车道目标用**交互标识**（`interactionId`）：网关车道 API 的 `laneId` 与 Interaction 的
 * `interactionId` 是两个不同的串，订阅与事件回显用的都是后者。曾经这里误用
 * `LaneSelection.laneId`，结果是帧全部到达、又被过滤器按「标识不匹配」整批丢掉，
 * 界面永远停在「生成回复中」。
 */
export function subscriptionIdOf(target: ConversationTarget): string {
  return target.kind === 'session'
    ? `session:${target.sessionId}`
    : `interaction:${target.interactionId}`;
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
export interface LaneCapability {
  available: boolean;
  laneId: string | null;
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
  /** 当前订阅的 Interaction 标识；会话目标恒为 null。 */
  watchedInteractionId: string | null;
  /** 历史索引已经装载完成的车道标识；为空表示尚未就绪或目标不是车道。 */
  laneReadyId: string | null;
  /**
   * 目标已切换、正文尚未交出。
   *
   * 回放是逐条 Interaction 拉起来的，逐段画出来就是肉眼可见的抖动；正文区据此在
   * 回放期间保持过渡面，回放落地后一次交出。
   */
  opening: boolean;
  /** 本次目标是否具备车道能力（提交、重试、中断）。 */
  laneAvailable: boolean;
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
  /** 提交正文到当前目标；目标不具备车道能力时明确失败，不静默丢弃。 */
  submit: (text: string, attachments?: AttachmentRef[], structuredInput?: string) => Promise<void>;
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
