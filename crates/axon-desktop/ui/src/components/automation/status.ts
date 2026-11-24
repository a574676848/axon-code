import type { CronJob } from "../../types";
import type { StateDotState } from "../ui/surfaces";

/*
 * 任务状态的展示口径：中文说法与状态点。
 * 列表与详情共用同一份映射，界面不直接输出后端枚举（active / paused / completed / removed）。
 */

const STATUS_TEXT: Record<CronJob["status"], string> = {
  active: "运行中",
  paused: "已暂停",
  completed: "已完成",
  removed: "已移除",
};

/** 任务状态的中文说法。 */
export const statusText = (status: CronJob["status"]): string => STATUS_TEXT[status];

/**
 * 行首状态点：正在执行是转圈，上次失败是红点，暂停是琥珀，
 * 已启用与已完成是绿点（同样的健康终态色），其余（从未执行、已移除）是灰点。
 */
export function jobState(job: CronJob): StateDotState {
  if (job.running === true) return "ongoing";
  if (job.lastRunSuccess === false) return "error";
  if (job.status === "paused") return "warning";
  if (job.status === "active" || job.status === "completed") return "done";
  return "idle";
}
