import React from 'react';
import { ContextMenu, useContextMenu } from './ContextMenu';
import { editableHostMenu, findEditableHost, readonlySelectionMenu } from './textSelection';

/*
 * 全局文本菜单宿主：整个应用只挂一次，挂上之后所有页面（含后续新增的）自动获得
 * 统一的右键行为，不需要各自处理 onContextMenu。
 *
 * 分工与既有实现的关系：
 * - 这个宿主监听的是 **document** 上的右键事件，因此它是兜底的那一层；
 * - 组件若需要自己的菜单（例如输入坞的编辑器菜单，它带真实的撤销历史），
 *   在自己的 onContextMenu 里调用 useContextMenu().open 即可：那一步会
 *   stopPropagation，事件到不了这里，两套菜单不会同时出现。
 *
 * 系统自带的 webview 菜单在这里统一压掉：这是桌面应用不是网页，
 * 「刷新 / 另存为 / 打印 / 检查」既用不到，也会让人误以为界面是浏览器页面。
 */

export const TextSelectionMenuHost: React.FC = () => {
  const menu = useContextMenu();

  React.useEffect(() => {
    const handleContextMenu = (event: MouseEvent) => {
      const target = event.target;
      if (!(target instanceof Element)) return;
      // 先压掉系统菜单：即使这里没有可给的条目，也不允许 webview 自己的菜单冒出来。
      event.preventDefault();
      const editable = findEditableHost(target);
      const items = editable === null ? readonlySelectionMenu() : editableHostMenu(editable);
      if (items === null) return;
      menu.openAt(event.clientX, event.clientY, items);
    };
    document.addEventListener('contextmenu', handleContextMenu);
    return () => document.removeEventListener('contextmenu', handleContextMenu);
  }, [menu]);

  if (menu.menu === null) return null;
  return (
    <ContextMenu
      items={menu.menu.items}
      position={menu.menu.position}
      onClose={menu.close}
    />
  );
};
