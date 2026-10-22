import React from 'react';
import {
  canRedo,
  canUndo,
  EMPTY_COMPOSER_HISTORY,
  recordEdit,
  redo,
  undo,
  type ComposerHistory,
} from '../../conversation/composerHistory';
import { isLargeText } from '../../conversation/textExport';
import { editingMenu } from '../ui/editingCommands';
import type { ContextMenuItem } from '../ui/ContextMenu';
import * as ipc from '../../ipc';

/*
 * 输入框的编辑能力：正文、撤销历史，以及撤销、恢复、剪切、复制、粘贴、全选六个动作。
 *
 * 快捷键与右键菜单共用这一份实现，因此两处的启用状态与效果必然一致；
 * 菜单条目的措辞、顺序与分组来自 editingCommands，与全局可编辑宿主读的是同一份定义，
 * 因此输入坞的菜单和设置里某个输入框的菜单完全一样（选区在打开菜单的那一刻取下来，
 * 见 buildMenu 的说明）。
 *
 * 历史粒度由「一次真实变更」决定，语义与依据见 composerHistory 的模块说明。
 */

/** 光标区间。 */
export interface CaretRange {
  start: number;
  end: number;
}

export interface ComposerEditingOptions {
  /** 输入框节点：选区、光标与全选都要落到真实节点上。 */
  areaRef: React.RefObject<HTMLTextAreaElement | null>;
  /** 大段文本的落盘动作，由附件控制器实现。 */
  onLargeText: (text: string) => void;
  /** 提示（例如剪贴板读不到）。 */
  onNotice: (text: string) => void;
  /** 程序化写入正文之后要求把光标落到某处。 */
  onCaretRequest: (position: number) => void;
  /**
   * 粘贴投影：把剪贴板文本换成另一种写法（粘贴链接时落成占位符）。
   *
   * 返回空表示按原样插入；返回结果由本控制器一次写入，撤销粒度仍然是一次粘贴。
   */
  preparePaste?: (text: string, range: CaretRange) => { text: string; caret: number } | null;
}

export interface ComposerEditing {
  text: string;
  /** 记下一次编辑并写入新正文。 */
  commit: (next: string) => void;
  /** 丢弃草稿与历史（提交之后）。 */
  reset: () => void;
  onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => void;
  onCompositionStart: () => void;
  onCompositionEnd: (event: React.CompositionEvent<HTMLTextAreaElement>) => void;
  undo: () => void;
  redo: () => void;
  selectAll: () => void;
  /** 组装右键菜单项；调用方负责把它摆到光标处。 */
  buildMenu: () => ContextMenuItem[];
}

export function useComposerEditing({
  areaRef,
  onLargeText,
  onNotice,
  onCaretRequest,
  preparePaste,
}: ComposerEditingOptions): ComposerEditing {
  const [text, setText] = React.useState('');
  /**
   * 撤销与恢复历史。
   *
   * 只放 ref、不放 state：历史不参与渲染（菜单的禁用态在打开菜单时才现算），
   * 而放进 state 就会被闭包捕获成一份过期快照，同一任务里连着两次撤销会重复消费
   * 同一层历史。正文本身也是同一个道理，见 textRef。
   */
  const editsRef = React.useRef<ComposerHistory>(EMPTY_COMPOSER_HISTORY);
  /**
   * 正文的即时值。
   *
   * 同一帧里可能连着发生多次编辑，每次都按 state 读「编辑前文本」会读到批处理前的
   * 旧值，撤销栈因此少记一层。所有写入都同步更新这里，读也一律读这里。
   */
  const textRef = React.useRef('');
  /** 输入法组合开始时的正文；组合期间不记录，确认时按整个词组记一次。 */
  const compositionBase = React.useRef<string | null>(null);

  const commit = React.useCallback((next: string) => {
    const before = textRef.current;
    if (next === before) return;
    textRef.current = next;
    editsRef.current = recordEdit(editsRef.current, before);
    setText(next);
  }, []);

  const reset = React.useCallback(() => {
    textRef.current = '';
    editsRef.current = EMPTY_COMPOSER_HISTORY;
    setText('');
  }, []);

  /**
   * 正文变更。
   *
   * 浏览器对「逐字键入」与「一次粘贴」的派发次数本来就不同：前者每个字符一次，
   * 后者整段一次。因此这里不做任何启发式合并，来一次记一次，
   * 「单字符逐字撤销」与「粘贴整段撤销」就同时成立。
   */
  const onChange = React.useCallback(
    (event: React.ChangeEvent<HTMLTextAreaElement>) => {
      const next = event.target.value;
      if (compositionBase.current !== null) {
        // 组合中间态不是用户的编辑意图：只更新画面，确认时再记一次。
        textRef.current = next;
        setText(next);
        return;
      }
      commit(next);
    },
    [commit],
  );

  const onCompositionStart = React.useCallback(() => {
    compositionBase.current = textRef.current;
  }, []);

  const onCompositionEnd = React.useCallback(
    (event: React.CompositionEvent<HTMLTextAreaElement>) => {
      const base = compositionBase.current;
      compositionBase.current = null;
      if (base === null) return;
      const next = event.currentTarget.value;
      if (next === base) return;
      // 整个词组记成一次编辑：撤销时整词回退，而不是回退到拼音中间态。
      textRef.current = next;
      editsRef.current = recordEdit(editsRef.current, base);
      setText(next);
    },
    [],
  );

  /** 取当前选区；拿不到节点时给出一个空区间。 */
  const currentRange = React.useCallback((): CaretRange => {
    const area = areaRef.current;
    if (area === null) return { start: 0, end: 0 };
    return { start: area.selectionStart, end: area.selectionEnd };
  }, [areaRef]);

  const undoStep = React.useCallback(() => {
    const step = undo(editsRef.current, textRef.current);
    if (step === null) return;
    textRef.current = step.text;
    editsRef.current = step.history;
    setText(step.text);
    onCaretRequest(step.text.length);
  }, [onCaretRequest]);

  const redoStep = React.useCallback(() => {
    const step = redo(editsRef.current, textRef.current);
    if (step === null) return;
    textRef.current = step.text;
    editsRef.current = step.history;
    setText(step.text);
    onCaretRequest(step.text.length);
  }, [onCaretRequest]);

  const selectAll = React.useCallback(() => {
    const area = areaRef.current;
    if (area === null) return;
    area.focus();
    area.setSelectionRange(0, area.value.length);
  }, [areaRef]);

  /** 复制一个区间；写入走原生剪贴板，因此不依赖窗口焦点。 */
  const copyRange = React.useCallback(
    (range: CaretRange) => {
      if (range.start === range.end) return;
      const selected = textRef.current.slice(range.start, range.end);
      void ipc.clipboardWriteText(selected).catch(() => onNotice('复制失败：系统剪贴板不可用'));
    },
    [onNotice],
  );

  /** 剪切：抄走再删掉，两者算同一次编辑，撤销一次就回到剪切之前。 */
  const cutRange = React.useCallback(
    (range: CaretRange) => {
      if (range.start === range.end) return;
      const value = textRef.current;
      copyRange(range);
      commit(value.slice(0, range.start) + value.slice(range.end));
      onCaretRequest(range.start);
    },
    [commit, copyRange, onCaretRequest],
  );

  /** 粘贴：大段文本落成文件，其余插到光标处。 */
  const pasteRange = React.useCallback(
    (range: CaretRange) => {
      void ipc.clipboardReadText().then(
        (clip) => {
          if (clip.length === 0) {
            onNotice('剪贴板里没有可粘贴的文本');
            return;
          }
          if (isLargeText(clip)) {
            onLargeText(clip);
            return;
          }
          // 链接占位要先登记再写入：投影自己带光标落点，写完这一次就结束。
          const projected = preparePaste?.(clip, range) ?? null;
          if (projected !== null) {
            commit(projected.text);
            onCaretRequest(projected.caret);
            return;
          }
          const value = textRef.current;
          commit(value.slice(0, range.start) + clip + value.slice(range.end));
          onCaretRequest(range.start + clip.length);
        },
        () => onNotice('读取剪贴板失败，请改用 Ctrl+V 粘贴'),
      );
    },
    [commit, onCaretRequest, onLargeText, onNotice, preparePaste],
  );

  /**
   * 组装右键菜单项。
   *
   * 选区在这里就定下来：菜单按钮会拿走焦点，等点击时再读输入框的选区不可靠。
   */
  const buildMenu = React.useCallback((): ContextMenuItem[] => {
    const range = currentRange();
    const hasSelection = range.start !== range.end;
    return editingMenu(
      {
        undo: undoStep,
        redo: redoStep,
        cut: () => cutRange(range),
        copy: () => copyRange(range),
        paste: () => pasteRange(range),
        selectAll,
      },
      {
        undo: !canUndo(editsRef.current),
        redo: !canRedo(editsRef.current),
        cut: !hasSelection,
        copy: !hasSelection,
      },
    );
  }, [copyRange, currentRange, cutRange, pasteRange, redoStep, selectAll, undoStep]);

  return {
    text,
    commit,
    reset,
    onChange,
    onCompositionStart,
    onCompositionEnd,
    undo: undoStep,
    redo: redoStep,
    selectAll,
    buildMenu,
  };
}
