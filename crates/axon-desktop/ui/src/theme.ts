import type { ThemeId } from './types';
import { desktopPreferences } from './ipc';

/*
 * 界面主题。
 *
 * 属性挂在 documentElement 上，暖石（paper）是清空属性而不是写 "paper"：
 * 任何遗漏写入的路径都会自然回落到默认主题。
 *
 * 磁盘是事实源，localStorage 只是首帧缓存。首帧由 index.html 里的同步脚本
 * 直接读缓存上色，避免先画默认主题再跳色；启动拿到后端值后这里再校正一次。
 */

/** 缓存键：与 index.html 的内联脚本共用。 */
const STORAGE_KEY = 'axon.theme';

/**
 * 三套主题的标识与展示名。
 *
 * 设置页与命令面板共用这一份：主题名是界面文案，写两遍就会在两处慢慢分叉。
 */
export const THEME_CHOICES: readonly { id: ThemeId; label: string }[] = [
  { id: 'paper', label: '纸白' },
  { id: 'forest', label: '冷杉' },
  { id: 'obsidian', label: '黑曜石' },
];

/** 主题变更订阅者。xterm 与图形引擎不跟 CSS 走，只能靠订阅重绘。 */
const listeners = new Set<(theme: ThemeId) => void>();

/**
 * 主题瞬切锁的令牌。
 *
 * 连续切换时前一次的释放回调会把后一次的锁提前摘掉，因此用一个递增令牌配对：
 * 只有最后那一次排定的回调才有权摘锁。
 */
let transitionLockToken = 0;

/**
 * 主题瞬切锁：换色那一帧屏蔽所有 CSS 插值过渡，第二帧之后摘掉。
 *
 * 不加这道锁时，`background-color`、`border-color` 与阴影会各自按自己的
 * transition 时长爬过去，读起来就是一次颜色残影；加了之后换主题是单帧完成的。
 */
function lockThemeTransitions(): void {
  const root = document.documentElement;
  const token = (transitionLockToken += 1);
  root.classList.add('disable-transitions');
  const release = (): void => {
    // 期间又切过一次：这一次的锁已经作废，交给最后一次去摘。
    if (token !== transitionLockToken) return;
    root.classList.remove('disable-transitions');
  };
  window.requestAnimationFrame(() => {
    window.requestAnimationFrame(release);
  });
}

/** 归一化外部主题值。 */
export function normalizeThemeId(raw: unknown): ThemeId {
  if (raw === 'forest' || raw === 'obsidian') return raw;
  return 'paper';
}

/** 当前生效的主题标识。 */
export function currentTheme(): ThemeId {
  return normalizeThemeId(document.documentElement.dataset.theme);
}

/** 订阅主题变更，返回取消订阅。 */
export function onThemeChange(listener: (theme: ThemeId) => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 同步宿主原生窗口的亮暗主题（仅在 Tauri 宿主内执行）。 */
function syncNativeWindowTheme(theme: ThemeId): void {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;
  void import('@tauri-apps/api/window')
    .then(({ getCurrentWindow }) =>
      getCurrentWindow().setTheme(theme === 'obsidian' ? 'dark' : 'light'),
    )
    .catch(() => undefined);
}

/** 把主题标识落到根元素，并回写首帧缓存。 */
export function applyTheme(theme: ThemeId): void {
  const normalized = normalizeThemeId(theme);
  // 先上锁再换色：顺序反过来的话，换色那一帧已经带着过渡跑出去了。
  lockThemeTransitions();
  if (normalized === 'forest' || normalized === 'obsidian') {
    document.documentElement.dataset.theme = normalized;
  } else {
    delete document.documentElement.dataset.theme;
  }
  document.documentElement.style.colorScheme = normalized === 'obsidian' ? 'dark' : 'light';
  syncNativeWindowTheme(normalized);
  try {
    window.localStorage.setItem(STORAGE_KEY, normalized);
  } catch {
    // 缓存不可用时只影响下次首帧，当前主题照常。
  }
  listeners.forEach((listener) => listener(normalized));
}

/** 读首帧缓存；没有或取值不认识时返回 null。 */
export function cachedTheme(): ThemeId | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    return raw === 'forest' || raw === 'obsidian' || raw === 'paper' ? raw : null;
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
    const theme = normalizeThemeId(preferences.theme);
    if (cachedTheme() !== theme || currentTheme() !== theme) applyTheme(theme);
    return theme;
  } catch {
    return cachedTheme() ?? 'paper';
  }
}
