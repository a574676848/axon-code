/*
 * 快捷键速查的唯一事实源。
 *
 * 这里只登记**代码里真的注册过**的键位：每一条都能在下面的实现处找到对应的监听，
 * 不做「文档里写了、按下去没反应」的占位。新增键位时先改这里，再改实现处。
 *
 * 实现处索引：
 * - 命令面板与浏览器动作压制：`desktopGuards.ts`
 * - 侧栏折叠、设置：`App.tsx`
 * - 新建会话：`Sidebar.tsx`
 * - 中断当前轮、工作区文件、新建终端：`conversation/useConversationShortcuts.ts`
 * - 输入坞的发送、换行、历史、撤销、候选：`conversation/composerKeys.ts`
 * - 系统级唤回：`desktop_shell.rs` 的 `GLOBAL_SHORTCUT`
 */

/** 一条快捷键。 */
export interface ShortcutEntry {
  /** 稳定标识。 */
  id: string;
  /** 这个键做什么。 */
  label: string;
  /** 键位序列；多键组合按顺序给出，界面自动用 `+` 连接。 */
  keys: readonly string[];
  /** 补充说明（同效键、生效条件等）。 */
  note?: string;
}

/** 一组快捷键。 */
export interface ShortcutGroup {
  /** 稳定标识。 */
  id: string;
  /** 分组标题。 */
  title: string;
  /** 组内条目。 */
  entries: readonly ShortcutEntry[];
}

/** 全站快捷键速查。 */
export const SHORTCUT_GROUPS: readonly ShortcutGroup[] = [
  {
    id: 'global',
    title: '全局',
    entries: [
      {
        id: 'command-palette',
        label: '打开命令面板',
        keys: ['Ctrl', 'K'],
        note: 'Ctrl + F 同效，用于跳转页面、设置分区、主题与会话',
      },
      { id: 'sidebar-toggle', label: '展开 / 收起侧栏', keys: ['Ctrl', 'B'] },
      { id: 'settings', label: '打开设置', keys: ['Ctrl', ','] },
      { id: 'new-session', label: '新建会话', keys: ['Ctrl', 'Alt', 'N'] },
      {
        id: 'wake',
        label: '唤回窗口并聚焦输入坞',
        keys: ['Ctrl', 'Alt', 'Space'],
        note: '系统级热键，窗口收进托盘时同样有效',
      },
      { id: 'open-workspace-file', label: '打开工作区文件', keys: ['Ctrl', 'Alt', 'P'] },
      { id: 'new-terminal', label: '新建终端', keys: ['Ctrl', '`'] },
    ],
  },
  {
    id: 'conversation',
    title: '对话',
    entries: [
      {
        id: 'interrupt',
        label: '中断当前一轮',
        keys: ['Esc'],
        note: '只在有轮次正在运行时接管；输入法组合中不触发',
      },
      { id: 'send', label: '发送消息', keys: ['Enter'] },
      { id: 'newline', label: '输入框内换行', keys: ['Shift', 'Enter'] },
    ],
  },
  {
    id: 'composer',
    title: '输入坞',
    entries: [
      {
        id: 'history',
        label: '翻输入历史',
        keys: ['↑', '↓'],
        note: '光标在首行按 ↑、末行按 ↓ 才翻历史，其余情况移动光标',
      },
      { id: 'undo', label: '撤销', keys: ['Ctrl', 'Z'] },
      { id: 'redo', label: '恢复', keys: ['Ctrl', 'Y'], note: 'Ctrl + Shift + Z 同效' },
      { id: 'select-all', label: '全选正文', keys: ['Ctrl', 'A'] },
      { id: 'reference', label: '引用工作区文件', keys: ['@'] },
      { id: 'slash', label: '调出指令与技能', keys: ['/'] },
      {
        id: 'candidates',
        label: '选择候选',
        keys: ['↑', '↓'],
        note: '候选展开时 ↑ ↓ 移动、Enter 或 Tab 采纳、Esc 收起',
      },
    ],
  },
  {
    id: 'lists',
    title: '列表与浮层',
    entries: [
      {
        id: 'palette-nav',
        label: '命令面板内移动与执行',
        keys: ['↑', '↓', 'Enter'],
        note: 'Esc 收起面板',
      },
      {
        id: 'memory-wheel',
        label: '切换记忆条目',
        keys: ['↑', '↓'],
        note: '滚轮同样可用；拖动或点选可直接跳转',
      },
      { id: 'close-overlay', label: '关闭最上层弹窗', keys: ['Esc'] },
    ],
  },
];
