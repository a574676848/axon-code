import type { WorkspaceItem } from '../types';

/** 收轨徽标与卡片条目的形态：待答走线性图标，其余走状态点。 */
export type RailActivityCue = 'question' | 'ongoing' | 'done' | 'error' | 'warning';

/** 卡片里的一条会话：状态点加跳转所需的最小事实。 */
export interface RailActivitySession {
  sessionId: string;
  workspaceId: string;
  title: string;
  cue: RailActivityCue;
}

/** 收轨态搜索格聚合出的会话活动。 */
export interface RailActivity {
  /** 徽标形态：待答 > 在跑 > 未查看终态里优先级最高的一档；都没有为 null。 */
  cue: RailActivityCue | null;
  /** 卡片条目：待答、在跑、未查看终态依次排列。 */
  sessions: RailActivitySession[];
  /** 待答的会话数，写进无障碍名称。 */
  questionCount: number;
  /** 在跑的会话数，写进无障碍名称。 */
  runningCount: number;
  /** 未查看终态的会话数，写进无障碍名称。 */
  unseenCount: number;
}

/** 终态字符串到状态点的映射，与 sessionStatusState 的终态分支同一口径。 */
function settledState(status: string): 'done' | 'error' | 'warning' {
  if (status === 'failed') return 'error';
  if (status === 'interrupted') return 'warning';
  return 'done';
}

/** 一档终态在徽标上的优先级，数字越大越先被看见。 */
const BADGE_PRIORITY: Record<'done' | 'error' | 'warning', number> = {
  done: 1,
  warning: 2,
  error: 3,
};

/** 待答的字符串前缀，与 conversation/sessionMarks 的 QUESTION_MARK_PREFIX 同一口径。 */
const QUESTION_PREFIX = 'question_';

/**
 * 把工作区清单与未查看标识投影成收轨态搜索格的活动事实。
 *
 * 纯函数：清单、标识与实时轮次都从外壳传来，这里只做投影，不重取也不写入。
 * 三档与展开态会话行的状态槽同一口径：待答（`pending_question_count > 0` 或
 * `question_` 前缀）最高，其次在跑，最后未查看终态。待答是「有事等你做」，
 * 看过也不撤，因此不与在跑合并；一条会话只归一档，不重复出现在卡片里。
 */
export function railActivity(
  workspaces: readonly WorkspaceItem[],
  sessionMarks: Readonly<Record<string, string>> | undefined,
  liveRunningSessionId: string | null = null,
): RailActivity {
  const questions: RailActivitySession[] = [];
  const running: RailActivitySession[] = [];
  const settled: RailActivitySession[] = [];
  let questionCount = 0;
  let runningCount = 0;
  let unseenCount = 0;
  let top: 'done' | 'error' | 'warning' | null = null;

  for (const workspace of workspaces) {
    for (const session of workspace.sessions) {
      const live = session.id === liveRunningSessionId;
      const status = sessionMarks?.[session.id];
      const pendingQuestion =
        (session.pending_question_count ?? 0) > 0 || status?.startsWith(QUESTION_PREFIX) === true;

      if (pendingQuestion) {
        questionCount += 1;
        questions.push({
          sessionId: session.id,
          workspaceId: workspace.id,
          title: session.title,
          cue: 'question',
        });
        continue;
      }
      if (session.running || live) {
        runningCount += 1;
        running.push({
          sessionId: session.id,
          workspaceId: workspace.id,
          title: session.title,
          cue: 'ongoing',
        });
        continue;
      }

      if (status === undefined) continue;
      const state = settledState(status);
      unseenCount += 1;
      settled.push({
        sessionId: session.id,
        workspaceId: workspace.id,
        title: session.title,
        cue: state,
      });
      if (top === null || BADGE_PRIORITY[state] > BADGE_PRIORITY[top]) top = state;
    }
  }

  return {
    cue: questionCount > 0 ? 'question' : runningCount > 0 ? 'ongoing' : top,
    sessions: [...questions, ...running, ...settled],
    questionCount,
    runningCount,
    unseenCount,
  };
}
