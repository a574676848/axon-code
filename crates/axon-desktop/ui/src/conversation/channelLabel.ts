/*
 * 轮次来源标签。
 *
 * 同一段会话可能由桌面、TUI 与 CLI 先后续聊，来源是**逐轮**的事实：会话级统一值会在
 * 跨端续聊时撒谎。这里只做展示映射，未知渠道原样给出，不猜成某一端。
 */

/** 渠道的内部标识到界面标签。 */
const CHANNEL_LABELS: Record<string, string> = {
  cli: 'CLI',
  desktop: '桌面',
  tui: 'TUI',
  cron: '定时',
  goal: '目标执行',
  internal: '内部',
};

/** 一轮的来源标签；没有渠道事实时返回空串。 */
export function turnChannelLabel(channel: string | undefined | null): string {
  const value = (channel ?? '').trim();
  if (value.length === 0) return '';
  return CHANNEL_LABELS[value] ?? value;
}
