/*
 * 目标看板的协议类型。
 *
 * 与 `types.ts` 分开放：那个文件已经贴着 700 行硬限，而目标看板需要的是一整组
 * 只读视图类型，放进主类型文件只会把它推过线。字段名与网关契约逐字对应，
 * 界面不做二次改名，避免同一份事实出现两套叫法。
 */

/** 目标状态，与领域七态一一对应。 */
export type GoalStatus =
  | 'proposed'
  | 'active'
  | 'waiting'
  | 'paused'
  | 'completed'
  | 'failed'
  | 'cancelled';

/** 一次目标执行的只读投影。 */
export interface GoalExecutionView {
  executionId: string;
  status: string;
  attempt: number;
  /** 绑定的运行时会话；未绑定时为空，此时无法进入会话视图。 */
  sessionId: string | null;
  createdAtMs: number;
  updatedAtMs: number;
}

/** 一次目标结果的只读投影。 */
export interface GoalResultView {
  resultId: string;
  succeeded: boolean;
  /** 已按 160 字符截断的摘要。 */
  summaryPreview: string;
  createdAtMs: number;
}

/** 看板卡片；详情复用同一份结构。 */
export interface GoalCardView {
  goalId: string;
  title: string;
  status: GoalStatus;
  revision: number;
  /** 已按 160 字符截断的目标预览。 */
  objectivePreview: string;
  stepCount: number;
  nextStep: string | null;
  waitingReason: string | null;
  priority: number;
  deadlineMs: number | null;
  scopeKind: 'workspace' | 'global';
  workspaceId: string | null;
  workspaceName: string | null;
  workspacePath: string | null;
  createdAtMs: number;
  updatedAtMs: number;
  lastActivityAtMs: number;
  latestExecution: GoalExecutionView | null;
  latestResult: GoalResultView | null;
}

/** 列表载荷。 */
export interface GoalListView {
  goals: GoalCardView[];
  /** 本次实际返回的目标数，恒不超过请求的 limit。 */
  returned: number;
  /** 为真表示可见目标超过本次读取上限。 */
  truncated: boolean;
}

/** 详情载荷。 */
export interface GoalDetailView {
  goal: GoalCardView;
  /** 完整目标，最多 20000 字符。 */
  objective: string;
  steps: string[];
  acceptance: string[];
  cancellationReason: string | null;
  objectiveTruncated: boolean;
  listsTruncated: boolean;
  /** 最近执行，按创建时间降序，最多 8 条。 */
  recentExecutions: GoalExecutionView[];
  latestResult: GoalResultView | null;
}
