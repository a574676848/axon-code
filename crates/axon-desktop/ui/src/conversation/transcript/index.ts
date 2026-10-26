/*
 * 对话投影模块的对外入口。
 *
 * 投影按职责拆成高内聚的子模块，调用方只认这一份入口：状态模型、轮次与单元的原语、
 * 文件改动的归并与派生、子代理投影、帧路由与派生读取。拆分的边界即依赖方向，
 * 因此不存在互相 import 的环。
 */

export {
  EMPTY_USAGE,
  INITIAL_CONVERSATION,
  LOCAL_STEER_PREFIX,
  cacheHitRateBasisPoints,
  completeInputTokens,
  compositionTokens,
  currentContextTokens,
  totalTokens,
  type AttachmentRef,
  type ConnectionState,
  type ConversationAction,
  type ConversationState,
  type JobRow,
  type PlanProjection,
  type SubagentEvent,
  type SubagentRow,
  type TokenUsage,
  type TranscriptCell,
  type TurnState,
} from './state';

export {
  CHANGE_KIND_LABEL,
  changeTotals,
  changedFilesOf,
  countDiffLines,
  countLines,
  lineCountsOf,
  toolChangeSummary,
  type ChangedFile,
  type FileChangeKind,
  type FileChangePart,
  type TurnChanges,
} from './changes';

export { conversationReducer } from './reducer';

/*
 * 逐帧归约对外可见：翻页时更早的一条 Interaction 要先在独立投影里回放完，
 * 再把算好的轮次整段前置，因此调用方需要能对一份临时状态跑同一套帧归约。
 */
export { applyFrames } from './frames';

/*
 * 折叠判据与摘要对外可见：控制器据此决定哪一轮折成一行，并在派发前把摘要算好
 * （折叠之后就没有可依据的单元了）。
 */
export {
  FOLD_CELL_THRESHOLD,
  FOLD_TEXT_THRESHOLD,
  MATERIALIZED_TURN_WINDOW,
  RECENT_EXPANDED_TURNS,
  foldedSummary,
  isFoldCandidate,
  shouldEvictFromWindow,
  shouldFold,
  turnTextWeight,
} from './fold';

export {
  attachmentsFromPaths,
  compactDuration,
  deriveJobs,
  pendingSteerCount,
  previewCommand,
  workingStatusPrimary,
} from './derive';
