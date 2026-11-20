/*
 * 会话级计划快照的协议形状。
 *
 * 网关在会话索引里随首页下发最近一次 `update_plan` 的投影；界面把它当作「打开这一刻
 * 的当前计划」，只在视图里还没有计划时采纳（见归约器的 `plan-seed`）。单独成文件而不是
 * 堆进 `types.ts`：那边是各条协议的集合，计划快照有自己的读侧约定与取舍。
 */

/** 会话级计划快照：网关索引里带回来的「当前那一份计划」。 */
export interface SessionPlanSnapshot {
  /** transcript part ID，计划单元的稳定身份。 */
  partId: string;
  /** 计划标题。 */
  title: string;
  /** 步骤明细（网关按上限截断）。 */
  steps: { step: string; status: string }[];
  /** 发出这次更新的那一轮；会话级帧没有归属时为空。 */
  interactionId?: string | null;
  /** 这次更新的时刻（Unix 毫秒）。 */
  updatedAtMs: number;
}
