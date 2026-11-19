/*
 * 子代理名册的类型。
 *
 * 从 `types.ts` 拆出：那个文件已经到了行数硬限，而这两个接口是一组自洽的只读视图
 * （名册行与清单），与其余会话类型没有互相引用。`types.ts` 仍原样再导出，
 * 既有引用不必改。
 */

/** 车道下的一条子代理会话。 */
export interface SessionSubagentSummary {
  /** 子代理会话标识；没有运行态会话行时为空，原因写在 `unavailableReason`。 */
  sessionId: string | null;
  /** 带前缀的稳定代理标识（例如 `subagent:/root/review`）：唯一匹配键。 */
  agentId: string;
  /** `sessionId` 为空时的原因码；非空时界面按它给出可读说明。 */
  unavailableReason?: string | null;
  title?: string | null;
  model?: string;
  provider?: string;
  updatedAtMs?: number;
  /** 线程真实状态：`active` 才是此刻仍在运行。 */
  status?: string;
  turnCount?: number;
  /** 末轮状态：结束/中断/失败由它决定。 */
  lastTurnStatus?: string | null;
  lastTurnAtMs?: number | null;
}

/** 一条车道的子代理会话清单。 */
export interface SessionSubagents {
  sessionId: string;
  runSessionId?: string | null;
  subagents: SessionSubagentSummary[];
}
