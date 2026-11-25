/*
 * 后台作业相关的 IPC：停止、活作业名册（含收口事实）与输出增量。
 *
 * 三件事回答同一张作业面板：还能不能停、还有谁在跑、它跑了多久、它输出了什么。
 * 它们共用一份事实源（网关的作业注册表与输出归档），因此单独成篇，
 * `ipc.ts` 只把它整体转出去，调用方仍然只从 `ipc` 取后端能力。
 */

import { call } from './backend';
import type { JobStopReceipt, SessionJobsRoster } from './types';

/** 用户手动停止某条车道上的后台作业。 */
export const conversationStopJob = (sessionId: string, jobId: string): Promise<JobStopReceipt> =>
  call('conversation_stop_job', { sessionId, jobId });

/**
 * 读取一条车道上仍在运行的后台作业名册。
 *
 * `settledJobIds` 是界面手上有行、名册却已不再持有的那些作业：同一次往返顺带取回
 * 它们的收口事实（后端实测耗时与终态），因此「跑了多久」不必再另建一条通道。
 */
export const conversationSessionJobs = (
  sessionId: string,
  settledJobIds: readonly string[] = [],
): Promise<SessionJobsRoster> =>
  call('conversation_session_jobs', { sessionId, settled: [...settledJobIds] });

/** 后台作业输出的一页：网关按字节游标增量给出。 */
export interface JobOutputStream {
  sessionId: string;
  jobId: string;
  /** 本次读取的起始字节偏移（已在归档头部之后）。 */
  offset: number;
  /** 下一次读取的字节偏移。 */
  nextOffset: number;
  text: string;
  /** 是否已到归档末尾；作业在跑时同样可能为真。 */
  eof: boolean;
  running: boolean;
  status: string | null;
  exitCode: number | null;
  elapsedMs: number | null;
  fullOutputAvailable: boolean;
  /** 读不到的原因；为空表示这一页就是真实输出。 */
  unavailable: string | null;
}

/** 读取一次后台作业的输出增量。 */
export const conversationJobOutput = (
  sessionId: string,
  jobId: string,
  offset: number,
): Promise<JobOutputStream> => call('conversation_job_output', { sessionId, jobId, offset });
