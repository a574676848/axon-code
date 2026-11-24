/*
 * 侧栏连接行的行尾读数（设计系统 §14.1.2）。
 *
 * 行尾只回答一件事：这一档此刻好不好用。因此延迟不是常驻读数，而是「健康时才有的
 * 那个读数」：网关答得快不代表后台在干活，后台租约失效时 Worker 已全部停下、会话
 * 不会被领取，这时再写一个 3 毫秒只会替一个不干活的进程背书。
 *
 * 规则只有两条，放在这里是为了能被单测锁住，而不是散落在组件里：
 * - 健康档（done）写延迟；取不到延迟就退回健康结论，不写 0。
 * - 其余档位（未探测、连接中、连不上、后台已停机、凭据需重输）一律写健康结论，
 *   并在悬停与无障碍名称里补上那句可执行说明。
 */

import type { ConnectionHealthView } from './connectionTypes';

/**
 * 行尾读数：健康时写延迟，其余档位写健康结论。
 *
 * 快照还没取到时返回 null，由调用方给占位读数：常驻入口不能因为一次读取没回来就消失。
 */
export function readoutText(
  health: Pick<ConnectionHealthView, 'dot' | 'label'> | null,
  latency: string | null,
): string | null {
  if (health === null) return null;
  if (health.dot !== 'done') return health.label;
  return latency === null ? health.label : latency;
}

/**
 * 整行的一句话结论：健康时补延迟，其余档位补可执行说明。
 *
 * 两者只出现一个：既写延迟又写故障会让人以为「慢」，而实际是「停了」。
 */
export function connectionSummary(
  label: string,
  health: ConnectionHealthView | null,
  latency: string | null,
): string {
  const healthLabel = health === null ? label : health.label;
  const head = healthLabel === label ? label : `${label}，${healthLabel}`;
  if (health !== null && health.dot === 'done') {
    return latency === null ? head : `${head}，延迟 ${latency}`;
  }
  const detail = health === null ? '' : health.detail;
  return detail === '' ? head : `${head}，${detail}`;
}
