/*
 * 目标看板的 IPC 封装。
 *
 * 与 `ipc.ts` 分开放：那个文件已接近 700 行硬限，而看板只需要两条只读命令。
 * 两条命令都只做转发，事实全部来自网关的只读 goals 面。
 */

import { call } from './backend';
import type { GoalDetailView, GoalListView } from './goalTypes';

/** 看板固定读取的条数上限，与网关契约一致。 */
export const GOAL_LIST_LIMIT = 256;

/**
 * 读取可见目标。
 *
 * `workspaceId` 为空表示「全部作用域」，与网关缺省语义一致。
 */
export const goalList = (
  limit: number = GOAL_LIST_LIMIT,
  workspaceId: string | null = null,
): Promise<GoalListView> =>
  call<GoalListView>(
    'goal_list',
    workspaceId === null || workspaceId.length === 0
      ? { limit }
      : { limit, workspaceId },
  );

/** 读取单个目标的详情。 */
export const goalGet = (goalId: string): Promise<GoalDetailView> =>
  call<GoalDetailView>('goal_get', { goalId });
