/*
 * 网关可用性与服务操作的契约类型。
 *
 * 这两组结构只描述「问网关要状态」与「对后台服务动手」的回执，与工作台其余领域类型没有
 * 交叉引用；它们的事实源在网关而不是工作台，因此单独成文件。types.ts 只回导出，
 * 既有 `import type { GatewayHealth } from './types'` 全部照旧。
 */

/** 网关可用性。 */
export interface GatewayHealth {
  connected: boolean;
  /** 界面是否应处在已连接形态：单轮失败按瞬时处理，连续多轮失败才切换。 */
  available: boolean;
  /** 已连续失败多少轮；成功一轮即归零。 */
  consecutive_failures: number;
  listening: boolean;
  /** 网关是否有 HTTP 响应（即便状态码不是 2xx）。 */
  responding: boolean;
  target: string;
  reason: string;
  /** 修复动作标识，由后端按故障类型给出。 */
  actions: string[];
}

/** 服务操作结果。 */
export interface ServiceAction {
  success: boolean;
  message: string;
  output: string;
}
