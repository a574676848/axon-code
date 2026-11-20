/*
 * 从页面进入会话的公共构造。
 *
 * 目标看板与自动化页都要把「某条后台执行会话」交给主舞台：两条入口共用同一套
 * `SessionSelection` 口径与同一段只读判定，避免各写一份后逐渐漂移。
 */

import type { SessionSelection } from './conversation/targets';
import type { SessionFacts } from './conversation/sessionFacts';

/**
 * 后台驱动会话的只读说明：输入坞禁用时显示在占位位置。
 *
 * 文案与 `docs/dev/ui-constraints.md` §4.2 的目标看板那一条同源：先说明「这里谁来干」，
 * 再说「这里为什么不能输入」。取消链路属于主会话的工具调用，不写进这个占位位：
 * 那是一句操作说明，而这一行只回答「这个框为什么是灰的」。
 */
export const READ_ONLY_PLACEHOLDER = '目标由后台执行，这里只读';

/** 能力事实还没到达时的占位说明：此刻不能确认这条会话可写。 */
export const CAPABILITY_PENDING_PLACEHOLDER = '正在读取会话…';

/** 能力事实到达、但会话行不存在时的占位说明。 */
export const UNREGISTERED_SESSION_PLACEHOLDER = '会话还没有登记';

/** 写入口的结论：可写性只由网关能力事实回答。 */
export interface SessionWriteGate {
  /** 能力事实是否已到达；首屏与草稿没有服务端事实，天然就绪。 */
  ready: boolean;
  /** 是否只读。 */
  readOnly: boolean;
  /** 是否允许作答；后台链不注入提问端口，因此只有主会话可答。 */
  answerable: boolean;
  /** 当前轮次是否允许中断；能力事实缺席时按不可中断处理。 */
  canInterrupt: boolean;
  /** 能力事实已到达但会话行不存在：这条会话还没有登记。 */
  unregistered: boolean;
}

/** 按能力事实算出写入口的结论，界面只读它。 */
export function sessionWriteGate(
  selection: Pick<SessionSelection, 'draft'> | null,
  facts: SessionFacts | null,
  loaded: boolean,
): SessionWriteGate {
  const fresh = selection === null || selection.draft === true;
  const ready = fresh || facts !== null;
  return {
    ready,
    readOnly: facts !== null && facts.writable === false,
    answerable: ready && facts?.canAnswerQuestions !== false,
    canInterrupt: facts?.canInterrupt === true,
    unregistered: !fresh && loaded && facts === null,
  };
}

/** 目标执行会话的选择。 */
export function goalSessionSelection(
  target: {
    workspaceId: string;
    workspacePath: string;
    sessionId: string;
    title: string;
  },
  /** 从哪条会话点进来的；给出时返回键回到那条会话，而不是看板。 */
  backTo: SessionSelection | null = null,
): SessionSelection {
  return {
    workspaceId: target.workspaceId,
    workspacePath: target.workspacePath,
    sessionId: target.sessionId,
    destination: '',
    title: target.title,
    channel: 'goal',
    backTo: backTo === null ? null : { ...backTo, backTo: null },
  };
}

/** 会话顶栏的来源返回条：只有从页面进入的后台会话才有。 */
export interface SessionOriginBarState {
  /** 返回文案，例如「返回看板」。 */
  backLabel: string;
  /** 会话标题，用于确认自己看的是哪一件事。 */
  title: string;
  onBack: () => void;
}

/** 按会话来源给出返回条；普通车道会话返回 null，不占高度。 */
export function originBarFor(
  selection: SessionSelection | null,
  navigate: (tab: 'goals' | 'automation') => void,
  openSession: (next: SessionSelection) => void,
): SessionOriginBarState | null {
  if (selection === null) return null;
  // 从别的会话点进来的后台会话：返回那一条会话，而不是它所属的页面。
  const backTo = selection.backTo ?? null;
  if (backTo !== null) {
    return { backLabel: '返回会话', title: selection.title, onBack: () => openSession(backTo) };
  }
  if (selection.channel === 'goal') {
    return { backLabel: '返回看板', title: selection.title, onBack: () => navigate('goals') };
  }
  if (selection.channel === 'cron') {
    return { backLabel: '返回自动化', title: selection.title, onBack: () => navigate('automation') };
  }
  return null;
}
