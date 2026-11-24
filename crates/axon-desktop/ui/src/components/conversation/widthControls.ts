/** 本地存储保存的宽度偏好键名 (px)。 */
export const WIDTH_PREF_KEY = 'axon.conversation.contentWidth';
/** 对话列宽度的理想下限；防止内容折行破碎。 */
export const CONTENT_MIN = 640;
/** 窄窗下的绝对下限：再窄也留得住一列可读正文。 */
export const CONTENT_FLOOR = 320;
/** 默认对话列基准宽度 (px)。 */
export const DEFAULT_WIDTH = 840;
/** 两侧预留给拖拽手柄及外侧安全区的间距 (px)。 */
export const CONTENT_EDGE_BUDGET = 176;
/** 低于这个容器宽度不再显示两侧手柄：余量放不下手柄，拖了也看不出变化。 */
export const HANDLE_MIN_CONTAINER_WIDTH = 680;

/** 读取本地持久化的有效宽度偏好，不存在或损坏时返回 null。 */
export function readWidthPreference(storage?: Storage): number | null {
  try {
    const store = storage ?? (typeof localStorage !== 'undefined' ? localStorage : null);
    if (!store) return null;
    const raw = store.getItem(WIDTH_PREF_KEY);
    if (raw === null) return null;
    const value = Number(raw);
    return Number.isFinite(value) && value > 0 ? value : null;
  } catch {
    return null;
  }
}

/**
 * 求解当前容器下的列宽下界。
 *
 * 固定 640 在窄窗下会硬裁：容器只剩 700px 时，列宽仍被撑到 640，两侧手柄与安全区
 * 被挤到容器外。这里改成「跟着可用宽度走」：常规仍是 CONTENT_MIN，
 * 空间不够时降到可用宽度，但不低于 CONTENT_FLOOR。
 */
export function contentLowerBound(containerWidth: number): number {
  const available = Math.max(0, containerWidth - CONTENT_EDGE_BUDGET);
  return Math.max(CONTENT_FLOOR, Math.min(CONTENT_MIN, available));
}

/**
 * 求解给定量测容器下实际应用的对话列宽度。
 *
 * `minWidth` 由调用方给出（见 contentLowerBound）；缺省保持 640 的历史口径，
 * 因此纯函数在外面被单独使用时行为不变。
 */
export function resolveContentWidth(
  containerWidth: number,
  preference: number | null,
  minWidth: number = CONTENT_MIN,
): number {
  const max = Math.max(minWidth, containerWidth - CONTENT_EDGE_BUDGET);
  if (preference !== null) {
    return Math.min(Math.max(preference, minWidth), max);
  }
  return Math.min(DEFAULT_WIDTH, max);
}

/** 计算手柄拖拽引起的宽度对称变化量。 */
export function calculateDraggedWidth(
  baseWidth: number,
  deltaX: number,
  side: 'left' | 'right',
): number {
  const outward = side === 'right' ? deltaX : -deltaX;
  return baseWidth + outward * 2;
}
