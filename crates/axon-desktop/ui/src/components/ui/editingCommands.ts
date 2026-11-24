import type { ContextMenuItem } from './ContextMenu';

/*
 * 编辑命令的文案、快捷键与分组：整个应用只有这一处定义。
 *
 * 输入坞自己的编辑器菜单与全局可编辑宿主的菜单都经由 editingMenu 生成，
 * 因此两处的条目、措辞、顺序与分组线必然一致；任何一处都不允许再写一次字面量。
 */

/** 编辑命令标识。 */
export type EditingCommand = 'undo' | 'redo' | 'cut' | 'copy' | 'paste' | 'selectAll';

/** 每条命令的菜单文案与右侧快捷键提示。 */
const EDITING_LABEL: Record<EditingCommand, { label: string; shortcut: string }> = {
  undo: { label: '撤销', shortcut: 'Ctrl+Z' },
  redo: { label: '恢复', shortcut: 'Ctrl+Y' },
  cut: { label: '剪切', shortcut: 'Ctrl+X' },
  copy: { label: '复制', shortcut: 'Ctrl+C' },
  paste: { label: '粘贴', shortcut: 'Ctrl+V' },
  selectAll: { label: '全选', shortcut: 'Ctrl+A' },
};

/**
 * 条目顺序与分组：撤销恢复一组，剪切复制粘贴一组，全选收尾。
 * 这是可编辑区域右键菜单的唯一形态，改这里就是改全站。
 */
const EDITING_MENU_SHAPE: Array<{ command: EditingCommand; separatorBefore: boolean }> = [
  { command: 'undo', separatorBefore: false },
  { command: 'redo', separatorBefore: false },
  { command: 'cut', separatorBefore: true },
  { command: 'copy', separatorBefore: false },
  { command: 'paste', separatorBefore: false },
  { command: 'selectAll', separatorBefore: true },
];

/** 只读区域的复制项：只读菜单有且只有这一项。 */
export const READONLY_COPY_ITEM = { label: '复制', shortcut: 'Ctrl+C' } as const;

/**
 * 组装可编辑区域的右键菜单。
 *
 * 调用方只交出「每条命令做什么」与「哪几条当前不可用」，
 * 展示形态由这里统一决定，因此不存在两套编辑菜单。
 */
export function editingMenu(
  actions: Record<EditingCommand, () => void>,
  disabled: Partial<Record<EditingCommand, boolean>> = {},
): ContextMenuItem[] {
  return EDITING_MENU_SHAPE.map(({ command, separatorBefore }) => ({
    label: EDITING_LABEL[command].label,
    shortcut: EDITING_LABEL[command].shortcut,
    separatorBefore,
    disabled: disabled[command] ?? false,
    action: actions[command],
  }));
}
