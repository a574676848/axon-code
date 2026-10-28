/**
 * 桌面宿主下的浏览器特征压制。
 *
 * 界面是标准 Web 应用，但在桌面壳里它不该露出任何浏览器痕迹：
 * 右键菜单、刷新、打印、查看源码与开发者工具全部接管掉。
 *
 * 三条纪律：
 * 1. **只压浏览器动作**，不碰任何编辑类组合键（撤销、恢复、全选、复制、粘贴），
 *    否则输入坞的撤销历史与选区会被静默吃掉。
 * 2. **右键菜单不是全都关掉**：正文、代码与报错详情需要复制，因此放行
 *    `[data-allow-native-menu]` 区域内的系统菜单，其余位置交给应用内菜单或直接屏蔽。
 * 3. **Ctrl + F 不是禁用，而是转义**：桌面工具里查找的语义是「找到某个东西并跳过去」，
 *    因此它被翻译成命令面板，而不是被吞掉。
 */

/** 命令面板的打开事件名：拦截器只负责转义，谁来实现面板与它无关。 */
export const OPEN_COMMAND_PALETTE_EVENT = 'axon:open-command-palette';

/** 全局快捷键唤回窗口后的聚焦事件：宿主推送经 App 转发到这一条，由输入坞自行聚焦。 */
export const FOCUS_COMPOSER_WINDOW_EVENT = 'axon:focus-composer';

/** 宿主推送的聚焦事件名，与 Rust 侧 `FOCUS_COMPOSER_EVENT` 一致。 */
export const FOCUS_COMPOSER_EVENT = 'axon://focus-composer';

/** 是否处于开发构建：开发期保留 DevTools，生产期封掉。 */
function isDevelopment(): boolean {
  return Boolean(import.meta.env?.DEV);
}

/**
 * 安装全局压制器，返回卸载函数。
 *
 * 只挂在 window 上两条监听，不参与任何渲染路径，也不阻止冒泡，
 * 因此各组件自己的 `onContextMenu` 仍然优先（它们会先调 preventDefault 并开应用内菜单）。
 */
export function installDesktopGuards(): () => void {
  const handleContextMenu = (event: MouseEvent) => {
    const target = event.target;
    if (target instanceof Element && target.closest('[data-allow-native-menu]') !== null) return;
    event.preventDefault();
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    const modifier = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();

    // 浏览器查找条转义成命令面板：同一个手势，更合适的目的地。
    if (modifier && (key === 'f' || key === 'k')) {
      event.preventDefault();
      window.dispatchEvent(new CustomEvent(OPEN_COMMAND_PALETTE_EVENT));
      return;
    }

    // 刷新：桌面应用没有「刷新页面」这一步，F5 与 Ctrl + R 都吞掉。
    if (event.key === 'F5' || (modifier && key === 'r')) {
      event.preventDefault();
      return;
    }

    // 打印与查看源码。
    // 带 Alt 的组合（例如 Ctrl + Alt + P）属于别的用途，不在这里吞。
    if (modifier && !event.altKey && (key === 'p' || key === 'u')) {
      event.preventDefault();
      return;
    }

    // 开发者工具只在生产构建里封掉，开发期照常可用。
    if (!isDevelopment() && (event.key === 'F12' || (modifier && event.shiftKey && key === 'i'))) {
      event.preventDefault();
    }
  };

  window.addEventListener('contextmenu', handleContextMenu);
  window.addEventListener('keydown', handleKeyDown);

  return () => {
    window.removeEventListener('contextmenu', handleContextMenu);
    window.removeEventListener('keydown', handleKeyDown);
  };
}
