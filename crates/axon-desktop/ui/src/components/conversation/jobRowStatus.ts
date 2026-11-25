import type { StateDotState } from '../ui/surfaces';
import { isActiveJob } from '../../conversation/runtimeFacts';

/*
 * 作业行的状态点口径。
 *
 * 转录里的作业状态是启动那一刻的历史快照，作业结束后这一行不会自己更新，
 * 活作业名册才是「后端此刻是否还持有」的权威（见 `conversation/runtimeFacts`）。
 * 状态点若照状态词直译，收口之后的作业会永远挂着那一枚转圈。
 */

/** 作业行此刻的活动事实，与行内文案共用同一份判断。 */
export interface JobRowActivity {
  /** 确实在跑：后端仍持有、状态为进行中、且本轮没有被用户停下。 */
  running: boolean;
  /** 本轮已按人的意思停下，状态词可能还没翻过来。 */
  stopped: boolean;
}

/**
 * 作业行该画的状态点。
 *
 * 转圈只属于确实在跑的作业：状态词仍写着 running、后端却已不再持有（历史行）
 * 或本轮已被用户停下时，继续转圈是在骗人，按终态画。
 * @param status - 转录里记录的状态词。
 * @param activity - 这一行此刻是否真的在跑、是否已被停下。
 * @returns `StateDot` 的状态取值。
 */
export function jobDotState(status: string, activity: JobRowActivity): StateDotState {
  if (status !== 'running') return settledDotState(status);
  if (activity.running) return 'ongoing';
  // 已按人的意思停下：与 killed 同档，共用警示色。
  if (activity.stopped) return 'warning';
  // 名册不再持有：没有结果证据，画静息灰点，不假装还在跑。
  return 'idle';
}

/** 非进行中的状态词到状态点。 */
function settledDotState(status: string): StateDotState {
  // 正在停止与被停同属「按人的意思收口」，与参考实现共用警示色。
  if (status === 'stopping' || status === 'killed') return 'warning';
  if (status === 'completed') return 'done';
  // 失败、超时、失去响应与被拒绝执行都是「没有正常收口」：与 failed 同档，共用错误色。
  if (ABNORMAL_TERMINAL.has(status)) return 'error';
  // 既没在跑也没结果的中间态：排队等待与未知状态都落在这一档。
  return 'idle';
}

/**
 * 「作业结束但没有正常收口」的终态集合。
 *
 * 与终端卡画「未正常退出」用的是同一份判据：超时、失去响应、被拒绝执行都是失败的一种，
 * 界面不得因为它们不叫 `failed` 就画成灰点，也不得把英文枚举直接铺上行内。
 */
export const ABNORMAL_TERMINAL: ReadonlySet<string> = new Set([
  'failed',
  'killed',
  'timeout',
  'unresponsive',
  'refusal',
]);

/**
 * 作业状态词的中文文案。
 *
 * 状态文案只显示中文（界面约束 §5）：网关的六个终态词必须逐个别名，不能落到
 * `return status` 的分支上，否则行里会直接出现 `timeout` 这类英文枚举。
 */
export function jobStatusLabel(status: string): string {
  if (status === 'running') return '运行中';
  if (status === 'stopping') return '正在停止';
  if (status === 'completed') return '已完成';
  if (status === 'killed') return '已取消';
  if (status === 'failed') return '已失败';
  if (status === 'timeout') return '已超时';
  if (status === 'unresponsive') return '已失去响应';
  if (status === 'refusal') return '已拒绝执行';
  return status;
}

/** 作业行行尾的结论：文案、是否取危险色、是否只是「后端已不再持有」。 */
export interface JobRowConclusion {
  /** 行尾文案。 */
  text: string;
  /** 是否取危险色。 */
  danger: boolean;
  /** 行里仍写着在跑、后端却已不再持有：文案已经如实写「已结束」。 */
  stale: boolean;
}

/** 判定输入：状态与退出码由调用方按收口事实折算成「这一行真实的结局」。 */
export interface JobRowConclusionInput {
  /** 折算后的状态词。 */
  status: string;
  /** 折算后的退出码；没有证据时为 null。 */
  exitCode: number | null;
  /** 后端是否仍持有它。 */
  live: boolean;
  /** 用户手动停止的终态原因；未停止为 null。 */
  stoppedReason: string | null;
}

/**
 * 作业行行尾该写什么。
 *
 * 优先级：用户停止的原因最具体；其次是**非零**退出码（干净退出不画退出码，只留状态词）；
 * 再次是「后端已不再持有」的如实收口；最后才是状态词。危险色与文案同源：取危险色的
 * 三种情况（被停、非零退出、异常终态）都不允许只靠颜色，文案本身已经把结论说清。
 */
export function jobRowConclusion(input: JobRowConclusionInput): JobRowConclusion {
  const stale = !input.live && isActiveJob(input.status);
  const failed = input.exitCode !== null && input.exitCode !== 0;
  const text =
    input.stoppedReason ??
    (failed
      ? `exit ${input.exitCode}`
      : stale
        ? '已结束'
        : jobStatusLabel(input.status));
  const danger =
    input.stoppedReason !== null || failed || ABNORMAL_TERMINAL.has(input.status);
  return { text, danger, stale };
}
