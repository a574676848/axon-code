/**
 * 启动时的「上次看到哪」恢复。
 *
 * 事实全部来自本地状态文件（`desktop-state.v2.json` 的 `client.lastWorkspaceDir` /
 * `lastSessionId` / `expandedWorkspaceIds`），不向服务端推导：服务端的会话清单只按
 * 最近活动排序，它不知道用户上次点开的是哪一条。
 *
 * 纯函数，便于单测锁定四件事：路径归一（Windows 反斜杠、`\\?\` 前缀、大小写）、
 * 工作区按目录匹配、会话按标识命中、以及都缺失时按最近活动兜底。
 */
import type { SessionItem, WorkspaceItem } from './types';

/**
 * 路径归一：只用于比较，不改写任何落盘路径。
 *
 * 侧栏展示的路径可能带 Windows 扩展前缀（`\\?\`）或正反斜杠混用，两个平台都要能
 * 比出同一条工作区。
 */
export function normalizePathForCompare(value: string): string {
  return value
    .trim()
    .replace(/^\\\\\?\\/, '')
    .replace(/^\/\/\?\//, '')
    .replace(/\\/g, '/')
    .replace(/\/+$/, '')
    .toLowerCase();
}

/** 按上次打开的目录找那条工作区；找不到返回 `null`。 */
export function workspaceOfDir(
  workspaces: WorkspaceItem[],
  directory: string | null | undefined,
): WorkspaceItem | null {
  const target = normalizePathForCompare(directory ?? '');
  if (target === '') return null;
  return workspaces.find((workspace) => normalizePathForCompare(workspace.path) === target) ?? null;
}

/** 一条工作区里最近活动的会话（置顶与归档不影响这里：只回答「最后一条」）。 */
function mostRecentSession(workspace: WorkspaceItem): SessionItem | null {
  const live = workspace.sessions.filter((session) => !session.archived);
  const pool = live.length > 0 ? live : workspace.sessions;
  if (pool.length === 0) return null;
  return pool.reduce((newest, session) => (session.updated_at > newest.updated_at ? session : newest));
}

/**
 * 决定启动时展开哪个工作区、打开哪条会话。
 *
 * 优先级：上次打开的工作区 > 上次会话所在工作区 > 最近有活动的工作区 > 当前工作区 > 首个。
 * 会话：上次打开的会话 > 该工作区最近活动的会话。两者都没有时返回 `null`，
 * 界面停在首屏而不是硬造一条会话。
 */
export function resolveRestoreTarget(
  workspaces: WorkspaceItem[],
  last: { workspaceDir?: string | null; sessionId?: string | null },
): { workspace: WorkspaceItem; session: SessionItem } | null {
  if (workspaces.length === 0) return null;
  const sessionId = (last.sessionId ?? '').trim();
  const owning = sessionId === ''
    ? null
    : workspaces.find((workspace) => workspace.sessions.some((session) => session.id === sessionId)) ?? null;
  const recent = [...workspaces]
    .filter((workspace) => mostRecentSession(workspace) !== null)
    .sort((left, right) => {
      const leftAt = mostRecentSession(left)?.updated_at ?? 0;
      const rightAt = mostRecentSession(right)?.updated_at ?? 0;
      return rightAt - leftAt;
    })[0] ?? null;
  const workspace =
    workspaceOfDir(workspaces, last.workspaceDir) ??
    owning ??
    recent ??
    workspaces.find((item) => item.active) ??
    workspaces[0];
  const byId = sessionId === ''
    ? null
    : workspace.sessions.find((session) => session.id === sessionId) ?? null;
  const session = byId ?? mostRecentSession(workspace);
  if (session === null) return null;
  return { workspace, session };
}
