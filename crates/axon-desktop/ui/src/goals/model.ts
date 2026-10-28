/*
 * 目标看板的纯函数层：状态映射、栏目归组、筛选、计数与卡片上限。
 *
 * 这一层不碰 React、不碰 IPC：页面级状态由 CDP 真机验收，仓库不引入 DOM 测试框架，
 * 因此凡是能算成纯函数的判定都收在这里，由 `npm test` 逐条锁死。
 */

import type { GoalCardView, GoalStatus } from '../goalTypes';

/** 栏目标识：看板从左到右的四栏。 */
export type GoalColumnId = 'proposed' | 'running' | 'paused' | 'settled';

/** 一个目标状态在界面上的三件事：文案、状态点、归入哪一栏。 */
export interface GoalStatusMeta {
  label: string;
  /** 与 `StateDot` 的取值一一对应。 */
  dot: 'done' | 'warning' | 'error' | 'idle' | 'ongoing';
  column: GoalColumnId;
}

const STATUS_META: Record<GoalStatus, GoalStatusMeta> = {
  proposed: { label: '待开始', dot: 'idle', column: 'proposed' },
  active: { label: '进行中', dot: 'ongoing', column: 'running' },
  waiting: { label: '等待中', dot: 'warning', column: 'running' },
  paused: { label: '已暂停', dot: 'warning', column: 'paused' },
  completed: { label: '已完成', dot: 'done', column: 'settled' },
  failed: { label: '已失败', dot: 'error', column: 'settled' },
  cancelled: { label: '已取消', dot: 'idle', column: 'settled' },
};

/** 状态映射；未知取值按「待开始」处理，界面永远不显示后端枚举。 */
export const goalStatusMeta = (status: GoalStatus): GoalStatusMeta =>
  STATUS_META[status] ?? STATUS_META.proposed;

/**
 * 执行状态的中文文案。
 *
 * 执行状态只出现在详情弹窗与卡片读数里，取值与网关投影一致；界面不直接显示
 * `running`、`uncertain` 这类后端枚举。
 */
const EXECUTION_LABELS: Record<string, string> = {
  queued: '排队中',
  running: '进行中',
  waiting: '等待中',
  uncertain: '结果不确定',
  completed: '已完成',
  failed: '已失败',
  cancelled: '已取消',
};

/** 执行状态文案；未知取值原样回落，不伪装成某个已知状态。 */
export const executionStatusLabel = (status: string): string =>
  EXECUTION_LABELS[status] ?? status;

/** 四栏的固定顺序与标题。 */
export const GOAL_COLUMNS: readonly { id: GoalColumnId; label: string }[] = [
  { id: 'proposed', label: '待开始' },
  { id: 'running', label: '进行中' },
  { id: 'paused', label: '已暂停' },
  { id: 'settled', label: '已结束' },
];

/** 单栏默认渲染上限。 */
export const COLUMN_CARD_LIMIT = 20;
/** 初始挂载上限：四栏各 20 张，合计 80 张。 */
export const INITIAL_CARD_LIMIT = COLUMN_CARD_LIMIT * GOAL_COLUMNS.length;

/** 工具栏的状态分段。 */
export type GoalFilter = 'all' | 'open' | 'settled';

/** 是否已收口（进入终态）。 */
export const isSettled = (status: GoalStatus): boolean =>
  goalStatusMeta(status).column === 'settled';

/** 目标是否已经绑定一个可订阅的执行会话。 */
export const hasSessionBinding = (goal: GoalCardView): boolean =>
  goal.latestExecution !== null &&
  goal.latestExecution.sessionId !== null &&
  goal.workspaceId !== null;

/**
 * 卡片页脚的动作文案。
 *
 * 有执行记录且能定位工作区才是「进入会话」；缺任何一半都退回详情，
 * 否则点下去只会拿到一条订阅失败横幅。
 */
export const cardActionLabel = (goal: GoalCardView): string =>
  hasSessionBinding(goal) ? '进入会话' : '查看详情';

/** 卡片定位行文案：优先等待原因，其次下一步；都没有就是空串。 */
export function goalLocatorText(goal: GoalCardView): string {
  if (goal.waitingReason !== null && goal.waitingReason.length > 0) {
    return `等待 · ${goal.waitingReason}`;
  }
  if (goal.nextStep !== null && goal.nextStep.length > 0) {
    return `下一步 · ${goal.nextStep}`;
  }
  return '';
}

/** 搜索命中：标题、目标预览、工作区名、下一步与等待原因都在检索范围内。 */
export function matchesQuery(goal: GoalCardView, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (needle.length === 0) return true;
  return [
    goal.title,
    goal.objectivePreview,
    goal.workspaceName ?? '',
    goal.workspacePath ?? '',
    goal.nextStep ?? '',
    goal.waitingReason ?? '',
  ].some((field) => field.toLowerCase().includes(needle));
}

/** 状态分段命中。 */
export function matchesFilter(goal: GoalCardView, filter: GoalFilter): boolean {
  if (filter === 'open') return !isSettled(goal.status);
  if (filter === 'settled') return isSettled(goal.status);
  return true;
}

/** 先按分段过滤，再按搜索串过滤；顺序固定，因此计数与卡片永远同源。 */
export function selectGoals(
  goals: readonly GoalCardView[],
  query: string,
  filter: GoalFilter,
): GoalCardView[] {
  return goals.filter(
    (goal) => matchesFilter(goal, filter) && matchesQuery(goal, query),
  );
}

/** 一段状态计数。 */
export interface GoalCounts {
  /** 本次读取到的目标数：口径固定取网关回执里的 `returned`。 */
  total: number;
  open: number;
  settled: number;
}

/**
 * 当前这次读取的分段计数。
 *
 * `total` 取网关回执的 `returned`（本次实际读到的目标数），不是过滤后的可见条数：
 * 计数说明的是「这次读到了什么」，筛选只改变看板里摆哪些卡。
 */
export function countGoals(
  goals: readonly GoalCardView[],
  returned: number,
): GoalCounts {
  const settled = goals.filter((goal) => isSettled(goal.status)).length;
  return { total: returned, open: goals.length - settled, settled };
}

/** 一栏的渲染切片。 */
export interface GoalColumnSlice {
  id: GoalColumnId;
  label: string;
  /** 该栏在过滤后的目标数。 */
  total: number;
  /** 本次实际渲染的卡片。 */
  visible: GoalCardView[];
  /** 收起时被折叠掉的张数；为 0 表示无需「显示其余」。 */
  hidden: number;
}

/**
 * 按四栏归组并应用单栏上限。
 *
 * `expanded` 里的栏目不再限制张数。四栏各 20 张即为初始挂载上限 80 张，
 * 展开后仍受数据侧 256 条上限约束，因此不会出现一次挂载几百张卡的情况。
 */
export function boardColumns(
  goals: readonly GoalCardView[],
  expanded: readonly GoalColumnId[],
  limit: number = COLUMN_CARD_LIMIT,
): GoalColumnSlice[] {
  return GOAL_COLUMNS.map((column) => {
    const members = goals.filter(
      (goal) => goalStatusMeta(goal.status).column === column.id,
    );
    const cap = expanded.includes(column.id)
      ? members.length
      : Math.min(limit, members.length);
    return {
      id: column.id,
      label: column.label,
      total: members.length,
      visible: members.slice(0, cap),
      hidden: members.length - cap,
    };
  });
}

/** 是否还有未收口的目标；为真才继续 4 秒轮询。 */
export const hasUnsettled = (goals: readonly GoalCardView[]): boolean =>
  goals.some((goal) => !isSettled(goal.status));

/**
 * 计数旁的口径说明。
 *
 * 服务端按更新时间升序出页，所以读满上限时拿到的是「最早的那一批」，不是「最近的」。
 * 这里只如实说「仅显示 N 个」，不给一个方向性错误的话。
 */
export function countSummary(
  counts: GoalCounts,
  truncated: boolean,
  limit: number,
): string {
  const base = `共 ${counts.total} 个目标 · 未收口 ${counts.open} · 已收口 ${counts.settled}`;
  if (!truncated) return base;
  return `${base} · 已达上限，仅显示 ${limit} 个`;
}
