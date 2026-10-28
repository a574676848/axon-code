import { useMemo } from 'react';
import type React from 'react';

/*
 * 输入坞的键盘约定（纯函数加一层薄接线）。
 *
 * 这一层承担三件事，全部与渲染无关：
 * 1. 撤销与恢复由编辑控制器接管（逐字、粘贴整段、中文按词）；
 * 2. 候选浮层打开时方向键、Enter、Tab、Esc 优先服务于选择；
 * 3. 光标在首行 / 末行时方向键翻输入历史，Enter 提交、Shift + Enter 换行。
 *
 * 拆成独立模块的原因是行数预算：输入坞本身已经接近 700 行上限，
 * 而这一段逻辑与它的依赖都只通过下面这份上下文往来，搬走不需要改语义。
 */

/** 候选控制器的最小面：`@` 文件引用与 `/` 指令两份实现都满足它。 */
export interface CandidateController {
  /** 采纳第 index 项，返回替换后的正文与光标；无法采纳时返回 null。 */
  accept: (index: number, text: string) => { text: string; caret: number } | null;
  /** 移动高亮。 */
  setIndex: (next: number) => void;
  /** 收起（只作用于当前这条 token）。 */
  dismiss: () => void;
}

/** 当前打开的候选组。 */
export interface OpenCandidates {
  /** 是否 `@` 文件引用组（另一组是 `/` 指令）。 */
  isReference: boolean;
  /** 候选条数。 */
  count: number;
  /** 当前高亮下标。 */
  index: number;
}

/** 键盘处理需要的全部依赖。 */
export interface ComposerKeyContext {
  /** 当前正文。 */
  text: string;
  /** 当前光标位置。 */
  caret: number;
  /** 当前打开的候选组；未打开为 null。 */
  openCandidates: OpenCandidates | null;
  /** 移动候选高亮。 */
  setCandidateIndex: (next: number) => void;
  /** 采纳候选。 */
  acceptCandidate: (index: number) => void;
  /** 收起候选（只作用于当前这条 token）。 */
  dismissCandidates: () => void;
  /** 撤销。 */
  undo: () => void;
  /** 恢复。 */
  redo: () => void;
  /** 全选。 */
  selectAll: () => void;
  /** 用新正文替换，并让调用方安排光标落点。 */
  commitText: (next: string) => void;
  /** 请求把光标落到某处（下一次渲染后生效）。 */
  requestCaret: (position: number) => void;
  /** 取上一条输入历史。 */
  previousHistory: (text: string) => Promise<string | null>;
  /** 取下一条输入历史。 */
  nextHistory: () => string | null;
  /** 提交正文。 */
  submit: () => void;
}

/** 生成输入框的 keydown 处理器。 */
export function createComposerKeyHandler(
  context: ComposerKeyContext,
): (event: React.KeyboardEvent<HTMLTextAreaElement>) => void {
  const {
    text,
    caret,
    openCandidates,
    setCandidateIndex,
    acceptCandidate,
    dismissCandidates,
    undo,
    redo,
    selectAll,
    commitText,
    requestCaret,
    previousHistory,
    nextHistory,
    submit,
  } = context;

  return (event) => {
    // 输入法组合期间的按键属于输入法，别抢。
    if (event.nativeEvent.isComposing) return;

    /*
     * 撤销与恢复由这里接管：语义见 composerHistory（逐字、粘贴整段、中文按词）。
     * 复制、剪切与粘贴交给浏览器原生行为，它会照常派发一次变更，
     * 因此撤销粒度自动正确，也不必自己实现剪贴板往返。
     */
    if (event.ctrlKey || event.metaKey) {
      const key = event.key.toLowerCase();
      if (key === 'z' && !event.shiftKey) {
        event.preventDefault();
        undo();
        return;
      }
      if (key === 'y' || (key === 'z' && event.shiftKey)) {
        event.preventDefault();
        redo();
        return;
      }
      if (key === 'a') {
        event.preventDefault();
        selectAll();
        return;
      }
    }

    /*
     * 候选项浮层打开时，键盘优先服务于选择，与 TUI 的 popup 一致：
     * 上下移动、Enter 或 Tab 采纳、Esc 与 Shift + Tab 收起（收起只作用于这一条 token，
     * 正文一变候选照常回来，因此它不会变成「这个功能以后都不弹了」）。
     */
    if (openCandidates !== null) {
      const { count, index: active } = openCandidates;
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const delta = event.key === 'ArrowDown' ? 1 : -1;
        setCandidateIndex((active + delta + count) % count);
        return;
      }
      if (event.key === 'Enter' || (event.key === 'Tab' && !event.shiftKey)) {
        event.preventDefault();
        acceptCandidate(active);
        return;
      }
      if (event.key === 'Escape' || (event.key === 'Tab' && event.shiftKey)) {
        event.preventDefault();
        dismissCandidates();
        return;
      }
    }

    /*
     * 历史输入：与 TUI 同口径，只有光标在第一行按上、最后一行按下才翻历史，
     * 多行草稿里的方向键仍然用于移动光标。
     */
    if (
      (event.key === 'ArrowUp' || event.key === 'ArrowDown') &&
      !event.shiftKey &&
      !event.altKey &&
      !event.ctrlKey &&
      !event.metaKey
    ) {
      const firstLine = !text.slice(0, caret).includes('\n');
      const lastLine = !text.slice(caret).includes('\n');
      if (event.key === 'ArrowUp' && firstLine) {
        // 上键在首行固定用于翻历史（与 TUI 一致）；没历史时只是什么也不发生。
        event.preventDefault();
        void previousHistory(text).then((recalled) => {
          if (recalled === null) return;
          commitText(recalled);
          requestCaret(recalled.length);
        });
        return;
      }
      if (event.key === 'ArrowDown' && lastLine) {
        const recalled = nextHistory();
        if (recalled !== null) {
          event.preventDefault();
          commitText(recalled);
          requestCaret(recalled.length);
          return;
        }
      }
    }

    if (event.key !== 'Enter' || event.shiftKey) return;
    event.preventDefault();
    submit();
  };
}

/** 接线需要的控制器与动作。 */
export interface ComposerKeyWiring {
  /** 当前正文。 */
  text: string;
  /** 当前光标位置。 */
  caret: number;
  /** `@` 文件引用候选组。 */
  references: CandidateController;
  /** `/` 指令候选组。 */
  slash: CandidateController;
  /** 当前打开的是哪一组、以及它的条目数与高亮下标。 */
  openCandidates: OpenCandidates | null;
  /** 撤销。 */
  undo: () => void;
  /** 恢复。 */
  redo: () => void;
  /** 全选。 */
  selectAll: () => void;
  /** 用新正文替换。 */
  commitText: (next: string) => void;
  /** 请求光标落点。 */
  requestCaret: (position: number) => void;
  /** 取上一条历史。 */
  previousHistory: (text: string) => Promise<string | null>;
  /** 取下一条历史。 */
  nextHistory: () => string | null;
  /** 提交正文。 */
  submit: () => void;
}

/**
 * 输入坞的键盘接线：采纳候选、移动高亮与全部按键分支都收在这一处。
 *
 * 组件因此只需要声明「这一轮有哪些依赖」，不必把六七段分支写进 JSX 上面那一段。
 */
export function useComposerKeyHandler(
  wiring: ComposerKeyWiring,
): (event: React.KeyboardEvent<HTMLTextAreaElement>) => void {
  const {
    text,
    caret,
    references,
    slash,
    openCandidates,
    undo,
    redo,
    selectAll,
    commitText,
    requestCaret,
    previousHistory,
    nextHistory,
    submit,
  } = wiring;
  const isReference = openCandidates?.isReference === true;

  /** 采纳一个候选：目录继续下钻，文件或指令结束这一次候选。 */
  const acceptCandidate = useMemo(
    () => (index: number) => {
      const step = (isReference ? references : slash).accept(index, text);
      if (step === null) return;
      commitText(step.text);
      requestCaret(step.caret);
    },
    [commitText, isReference, references, requestCaret, slash, text],
  );

  return useMemo(
    () =>
      createComposerKeyHandler({
        text,
        caret,
        openCandidates,
        setCandidateIndex: isReference ? references.setIndex : slash.setIndex,
        acceptCandidate,
        dismissCandidates: isReference ? references.dismiss : slash.dismiss,
        undo,
        redo,
        selectAll,
        commitText,
        requestCaret,
        previousHistory,
        nextHistory,
        submit,
      }),
    [
      acceptCandidate,
      caret,
      commitText,
      isReference,
      nextHistory,
      openCandidates,
      previousHistory,
      references,
      redo,
      requestCaret,
      selectAll,
      slash,
      submit,
      text,
      undo,
    ],
  );
}

