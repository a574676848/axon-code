/*
 * 运行中这一轮实际使用的模型路由。
 *
 * provider 状态事件的第一段就是 `供应商/模型 id`，但它只在流式阶段存在：等工具、等作业
 * 时 activity 会被清空，状态行于是掉回配置里的默认模型。同一轮里模型不可能中途换人，
 * 因此这里把这一轮最近一次见到的路由记住，activity 清空时继续用它。
 *
 * 记忆按轮次标识隔离：下一轮若换了模型，第一次状态事件会覆盖它，不会沿用上一轮。
 */

import React from 'react';

/** 从 provider 状态文案里取出模型路由；不是 `供应商/模型` 形状时返回 null。 */
export function routeFromActivity(activity: string | null): string | null {
  if (activity === null) return null;
  const head = activity.trim().split(/\s+/)[0] ?? '';
  return head.includes('/') ? head : null;
}

/**
 * 这一轮该显示的模型路由。
 *
 * 优先级：本轮最近一次状态事件里的路由，其次配置投影（`供应商/模型 id` 或展示名）。
 */
export function useTurnModelRoute(
  turnId: string | null,
  activity: string | null,
  fallback: string,
): string {
  const seen = React.useRef<{ turnId: string | null; route: string }>({
    turnId: null,
    route: '',
  });

  const candidate = routeFromActivity(activity);
  if (turnId !== null && candidate !== null) {
    seen.current = { turnId, route: candidate };
  }
  if (turnId !== null && seen.current.turnId === turnId) return seen.current.route;
  return fallback;
}
