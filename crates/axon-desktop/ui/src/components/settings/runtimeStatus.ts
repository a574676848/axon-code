import type { DaemonHealth } from '../../types';
import type { StateDotState } from '../ui/surfaces';

/*
 * 运行状态面板的纯视图模型：把后端给出的结论标识与后台健康快照翻成界面读数。
 *
 * 与 React 无关，因此边界（认不出的结论、快照读不到、租约失效）可以直接单测。
 * 这几个分支平时看不见，出问题时恰恰是唯一能说明情况的东西，不能靠人眼看一遍
 * 就算验证过。
 */

/** Daemon 结论的展示形态。 */
export interface DaemonStateView {
  /** 结论短语。 */
  label: string;
  /** 状态点取值。 */
  dot: StateDotState;
  /** 是否需要人处理：非正常结论据此加重显示。 */
  attention: boolean;
}

const DAEMON_STATE_VIEW: Record<string, DaemonStateView> = {
  running: { label: '运行中', dot: 'done', attention: false },
  workers_stopped: { label: '后台已停机', dot: 'warning', attention: true },
  degraded: { label: '运行受限', dot: 'warning', attention: true },
  stopped: { label: '未运行', dot: 'error', attention: true },
};

/**
 * 把结论标识翻成界面形态。
 *
 * 认不出的标识不猜语义：原样显示并置灰。宁可让人看到一个新词去查，
 * 也不能把未知状态画成「运行中」。
 */
export function daemonStateView(state: string | null | undefined): DaemonStateView {
  if (state === null || state === undefined || state.length === 0) {
    return { label: '正在查询', dot: 'idle', attention: false };
  }
  return DAEMON_STATE_VIEW[state] ?? { label: state, dot: 'idle', attention: false };
}

/** 后台读数行的形态。 */
export interface DaemonHealthView {
  /** 一行读数。 */
  text: string;
  /** 是否为需要人处理的读数。 */
  warning: boolean;
}

/** 租约剩余时长的读法：续租窗口是秒级，超过一分半才换成分钟。 */
function readLease(millis: number): string {
  const seconds = Math.ceil(millis / 1000);
  return seconds < 90 ? `${seconds} 秒` : `${Math.round(seconds / 60)} 分钟`;
}

/**
 * 把租约与排队读数拼成一行。
 *
 * 三种情况分开说：读不到快照（说明原因）、租约失效（点明后果与恢复动作）、
 * 正常续租（把排队读数摊开）。租约失效不是「读数异常」，而是后台真的停了：
 * 网关照样应答，会话却再也不会被领取。
 */
export function daemonHealthView(
  health: DaemonHealth | null | undefined,
  error: string,
): DaemonHealthView {
  if (health === null || health === undefined) {
    const reason = error.trim();
    return {
      text:
        reason.length > 0
          ? `后台读数读不到：${reason}`
          : '后台读数读不到，无法确认 Worker 是否在推进',
      warning: false,
    };
  }
  const queue = `待领取 ${health.interactions_pending} · 处理中 ${health.interactions_processing} · 待投递 ${health.deliveries_pending}`;
  if (health.lease_remaining_ms <= 0) {
    return {
      text: `后台租约已失效，Worker 已停机（${queue}），点右侧重启可恢复`,
      warning: true,
    };
  }
  return { text: `后台租约剩余 ${readLease(health.lease_remaining_ms)} · ${queue}`, warning: false };
}
