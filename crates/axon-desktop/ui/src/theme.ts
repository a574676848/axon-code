import type { ThemeId } from './types';
import { desktopPreferences } from './ipc';

/*
 * 界面主题。
 *
 * 属性挂在 documentElement 上，纸白是清空属性而不是写 "paper"：
 * 任何遗漏写入的路径都会自然回落到默认主题。
 *
 * 磁盘是事实源，localStorage 只是首帧缓存。首帧由 index.html 里的同步脚本
 * 直接读缓存上色，避免先画默认主题再跳色；启动拿到后端值后这里再校正一次。
 */

/** 缓存键：与 index.html 的内联脚本共用。 */
const STORAGE_KEY = 'axon.theme';

/** 主题变更订阅者。xterm 与图形引擎不跟 CSS 走，只能靠订阅重绘。 */
const listeners = new Set<(theme: ThemeId) => void>();

/** 订阅主题变更，返回取消订阅。 */
export function onThemeChange(listener: (theme: ThemeId) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 把主题标识落到根元素，并回写首帧缓存。 */
export function applyTheme(theme: ThemeId): void {
  if (theme === 'forest') {
    document.documentElement.dataset.theme = 'forest';
  } else {
    delete document.documentElement.dataset.theme;
  }
  try {
    window.localStorage.setItem(STORAGE_KEY, theme);
  } catch {
    // 缓存不可用时只影响下次首帧，当前主题照常。
  }
  listeners.forEach((listener) => listener(theme));
}

/** 读首帧缓存；没有或取值不认识时返回 null。 */
export function cachedTheme(): ThemeId | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw === 'forest' || raw === 'paper' ? raw : null;
  } catch {
    return null;
  }
}

/**
 * 启动后与磁盘校正一次。
 *
 * 后端不可达时保持缓存值，界面不会因为一次读失败就跳回默认主题。
 */
export async function syncThemeFromBackend(): Promise<ThemeId> {
  try {
    const preferences = await desktopPreferences();
    const theme: ThemeId = preferences.theme === 'forest' ? 'forest' : 'paper';
    if (cachedTheme() !== theme) applyTheme(theme);
    return theme;
  } catch {
    return cachedTheme() ?? 'paper';
  }
}
