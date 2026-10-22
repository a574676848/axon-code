import * as ipc from '../../ipc';
import type { ContextMenuItem } from './ContextMenu';
import { editingMenu, READONLY_COPY_ITEM } from './editingCommands';

/*
 * 全局文本选择与右键菜单的规则层。
 *
 * 与 React 无关，因此可以被单测直接覆盖；界面只负责把事件交给它、把返回的菜单摆出来。
 *
 * 右键落在哪里，决定弹出哪一套菜单：
 * 1. 可编辑宿主（文本型 input / textarea）：与输入坞同一份编辑菜单；
 * 2. 其余位置：只读菜单，只有一项「复制」，且只在真有选区时出现。
 *    既没有选区、又不能编辑时返回 null，界面因此不会弹出一个只有禁用项的空菜单。
 *
 * 「复制」与「剪切」的文本必须在按下右键的这一刻取下来：菜单按钮会拿走焦点，
 * 等点击菜单项时再读选区只会拿到空串。
 */

/** 文本型输入控件：其余类型（勾选、文件、按钮等）没有可编辑的正文。 */
const TEXT_INPUT_TYPES = new Set([
  'text',
  'search',
  'url',
  'tel',
  'email',
  'password',
  'number',
  'date',
  'datetime-local',
  'month',
  'week',
  'time',
]);

/** 可编辑宿主：应用里只有原生输入控件承载可编辑正文。 */
export type EditableHost = HTMLInputElement | HTMLTextAreaElement;

/** 光标区间；与宿主的 `selectionStart` / `selectionEnd` 同义。 */
interface HostSelection {
  start: number;
  end: number;
}

/**
 * 从事件落点找到可编辑宿主。
 *
 * 只接受文本型输入控件：禁用与只读的输入框没有可编辑内容，按只读正文处理。
 */
export function findEditableHost(target: Element): EditableHost | null {
  const host = target.closest('input, textarea');
  if (host instanceof HTMLTextAreaElement) {
    return host.disabled || host.readOnly ? null : host;
  }
  if (host instanceof HTMLInputElement) {
    if (host.disabled || host.readOnly || !TEXT_INPUT_TYPES.has(host.type)) return null;
    return host;
  }
  return null;
}

/** 只读菜单：有选区时只给一项「复制」，没有选区时不弹菜单。 */
export function readonlySelectionMenu(): ContextMenuItem[] | null {
  const text = typeof window === 'undefined' ? '' : (window.getSelection()?.toString() ?? '');
  if (text.trim().length === 0) return null;
  return [
    {
      ...READONLY_COPY_ITEM,
      action: () => {
        // 只读菜单没有提示渠道，系统剪贴板不可用时静默失败。
        void ipc.clipboardWriteText(text).catch(() => undefined);
      },
    },
  ];
}

/** 取宿主的当前选区。 */
function readSelection(host: EditableHost): HostSelection {
  // DOM 类型把 selectionStart/End 标成可空，而文本型控件上它们始终有值：空值按 0 处理。
  return { start: host.selectionStart ?? 0, end: host.selectionEnd ?? 0 };
}

/**
 * 把新值写回宿主并派发一次输入事件。
 *
 * 走原型上的 value setter 而不是直接赋值：React 在 value 上装了追踪器，
 * 直接赋值会让它认为值没有变化，随后派发的 input 事件就不会变成 onChange。
 */
function writeHostValue(host: EditableHost, next: string, caret: number): void {
  const prototype =
    host instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(prototype, 'value')?.set?.call(host, next);
  host.dispatchEvent(new Event('input', { bubbles: true }));
  host.focus();
  host.setSelectionRange(caret, caret);
}

/**
 * 把焦点与选区还给宿主。
 *
 * 菜单按钮会拿走焦点，撤销与恢复这类由浏览器自己维护历史的命令必须落在
 * 聚焦且带选区的宿主上；区间在打开菜单时就定下来了，因此这里直接还原。
 */
function restoreHost(host: EditableHost, selection: HostSelection): void {
  host.focus();
  host.setSelectionRange(selection.start, selection.end);
}

/** 浏览器是否报告该命令当前可用；不支持时一律按不可用处理。 */
function commandEnabled(command: 'undo' | 'redo'): boolean {
  try {
    return document.queryCommandEnabled(command);
  } catch {
    return false;
  }
}

/** 执行一条由浏览器实现的编辑命令。 */
function runCommand(host: EditableHost, selection: HostSelection, command: 'undo' | 'redo'): void {
  restoreHost(host, selection);
  try {
    document.execCommand(command);
  } catch {
    // 浏览器拒绝时不另报错：菜单项在此之前已按 queryCommandEnabled 置灰。
  }
}

/** 可编辑菜单：与输入坞完全同一份条目，只是历史由浏览器自己维护。 */
export function editableHostMenu(host: EditableHost): ContextMenuItem[] {
  // 选区在打开菜单的这一刻定下来，理由见模块说明。
  const selection = readSelection(host);
  const hasSelection = selection.start !== selection.end;

  const copy = () => {
    const text = host.value.slice(selection.start, selection.end);
    if (text.length === 0) return;
    void ipc.clipboardWriteText(text).catch(() => undefined);
  };

  return editingMenu(
    {
      undo: () => runCommand(host, selection, 'undo'),
      redo: () => runCommand(host, selection, 'redo'),
      cut: () => {
        copy();
        // 剪切算一次普通输入：宿主自己维护的撤销历史因此仍然可用。
        writeHostValue(
          host,
          host.value.slice(0, selection.start) + host.value.slice(selection.end),
          selection.start,
        );
      },
      copy,
      paste: () => {
        void ipc.clipboardReadText().then(
          (clip) => {
            if (clip.length === 0) return;
            writeHostValue(
              host,
              host.value.slice(0, selection.start) + clip + host.value.slice(selection.end),
              selection.start + clip.length,
            );
          },
          () => undefined,
        );
      },
      selectAll: () => {
        host.focus();
        host.select();
      },
    },
    {
      undo: !commandEnabled('undo'),
      redo: !commandEnabled('redo'),
      cut: !hasSelection,
      copy: !hasSelection,
    },
  );
}
