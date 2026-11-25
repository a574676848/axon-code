import type { JobRow } from './transcript';

/*
 * 后台作业行的收口事实点名。
 *
 * 收口后的真实耗时与终态只存在于后端事件日志里，界面因此按需点名一次；这一层只回答
 * 「该点哪些作业」。判定从组件里抽出来单放，是因为它踩过三个坑，每个都只能靠单测钉住：
 * 名册仍持有的作业不能问（它在跑，耗时由界面按秒走）、已经问过的不能重复问
 * （每拍重发就是性能问题）、同一作业标识在进程重启后可能同时对应两代行，只认最后一行。
 */

/** 一次点名最多带上的作业标识数：与网关的 `JOB_FACT_MAX_IDS` 同口径。 */
export const SETTLED_FACT_MAX_IDS = 64;

export interface SettlementAskInput {
  /** 会话台账里的作业行。 */
  jobs: readonly JobRow[];
  /** 后端此刻仍持有的作业标识。 */
  live: ReadonlySet<string>;
  /** 已经点过名的作业标识。 */
  asked: ReadonlySet<string>;
  /** 本次最多点几条；缺省取网关上限。 */
  limit?: number;
}

export interface SettlementAsk {
  /** 要去问的作业标识（去重，已按上限截断）。 */
  ids: string[];
  /** 这些标识对应的行：回执按行落账，不按标识落账。 */
  rows: JobRow[];
}

/**
 * 该点名哪些作业的收口事实。
 *
 * 三条排除：行里没有作业标识（前台命令与交互式会话）、名册仍持有它、已经有实测耗时
 * （历史清单自带）。同一个标识只认最后一行；超出上限的留给下一拍，不标记为已问过。
 * 从**新到旧**挑：上限只够发一批时，用户刚看完的那几条必须先拿到读数。
 */
export function settlementAskOf(input: SettlementAskInput): SettlementAsk {
  const limit = Math.max(1, input.limit ?? SETTLED_FACT_MAX_IDS);
  // 作业标识是进程内自增数，重启后会从 1 重新计数：同一个标识可能同时对应上一代的
  // 死行与这一代的活行，只有最后出现的那一行才是它此刻的身份。
  const lastRowByJobId = new Map<string, string>();
  for (const job of input.jobs) {
    if (job.jobId !== null) lastRowByJobId.set(job.jobId, job.id);
  }
  const ids: string[] = [];
  const rows: JobRow[] = [];
  for (let index = input.jobs.length - 1; index >= 0; index -= 1) {
    const job = input.jobs[index];
    if (job.jobId === null || job.durationMs !== null) continue;
    if (input.live.has(job.jobId)) continue;
    if (lastRowByJobId.get(job.jobId) !== job.id) continue;
    if (input.asked.has(job.jobId) || ids.includes(job.jobId)) continue;
    if (ids.length >= limit) break;
    ids.push(job.jobId);
    rows.push(job);
  }
  return { ids, rows };
}
