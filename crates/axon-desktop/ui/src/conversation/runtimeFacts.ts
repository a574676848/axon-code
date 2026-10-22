/**
 * 当前选中会话的运行时事实。
 *
 * 归档是破坏性动作：会话正在跑的时候归档，用户很可能没有意识到任务会被留下。
 * 会话的运行状态只存在于会话视图内部，因此由它把这份最小事实上报给外壳，
 * 侧栏据此决定是否需要二次确认，以及确认后该停止什么。
 */
export interface LaneRuntimeFacts {
  /** 事实所属的车道；没有选中会话时为 null。 */
  laneId: string | null;
  /** 主轮是否仍在推进。 */
  turnRunning: boolean;
  /** 进行中的子代理数量。 */
  activeSubagents: number;
  /** 进行中的后台任务数量。 */
  activeJobs: number;
  /** 当前会话是否还有在等用户回答的提问；待答标识的清除以它为准。 */
  hasPendingQuestion: boolean;
  /** 停止本轮及其派生的子任务；归档前调用。 */
  stop: () => Promise<void>;
}

/** 是否真的有什么在跑。 */
export function isRunning(facts: LaneRuntimeFacts | null): boolean {
  if (facts === null) return false;
  return facts.turnRunning || facts.activeSubagents > 0 || facts.activeJobs > 0;
}

/**
 * 两份运行态事实是否等价。
 *
 * `stop` 回调不参与比较：它每轮渲染都会换一次引用，而它换引用不代表事实变了。
 * 上报方每次都会新建一个事实对象，若照单全收地换引用，外壳就会每轮重渲染，
 * 而重渲染又让上报方的回调再次换引用并再上报一次，形成自激循环（实测能把
 * 渲染进程一路推到内存耗尽）。
 */
export function sameRuntimeFacts(
  previous: LaneRuntimeFacts | null,
  next: LaneRuntimeFacts | null,
): boolean {
  if (previous === null || next === null) return previous === next;
  return (
    previous.laneId === next.laneId &&
    previous.turnRunning === next.turnRunning &&
    previous.activeSubagents === next.activeSubagents &&
    previous.activeJobs === next.activeJobs &&
    previous.hasPendingQuestion === next.hasPendingQuestion
  );
}

/** 进行中的作业状态；其余状态一律视为已结束。 */
const ACTIVE_JOB_STATUS = new Set(['running', 'starting', 'stopping', 'pending']);

/** 作业是否仍在进行。 */
export function isActiveJob(status: string): boolean {
  return ACTIVE_JOB_STATUS.has(status);
}

/** 子代理是否仍在进行。 */
export function isActiveSubagent(status: string): boolean {
  return status === 'starting' || status === 'running' || status === 'waiting';
}
