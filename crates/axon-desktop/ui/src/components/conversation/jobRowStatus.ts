import type { StateDotState } from '../ui/surfaces';

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
  if (status === 'failed') return 'error';
  // 既没在跑也没结果的中间态：排队等待与未知状态都落在这一档。
  return 'idle';
}
