import React from 'react';

import * as ipc from '../../ipc';
import type { FileEntryItem } from '../../types';
import {
  activeReferenceToken,
  directoryArgument,
  matchReferenceCandidates,
  referenceInsertion,
  type ReferenceToken,
} from '../../conversation/references';

/*
 * `@` 文件引用的补全控制器。
 *
 * 只扫描 token 指向的那一层目录（接口本身就是工作区作用域的相对路径索引），
 * 候选与采纳规则都来自 `conversation/references`，这里只负责取数与缓存。
 */

/** 目录缓存时长：会话里新出现的文件不至于永远看不见。 */
const DIRECTORY_CACHE_TTL_MS = 5_000;

interface ReferenceCompletionInput {
  /** 当前工作区根目录；为空时不做任何索引。 */
  workspaceRoot: string | null;
  /** 输入框正文。 */
  text: string;
  /** 光标位置。 */
  caret: number;
}

export interface ReferenceCompletion {
  /** 已采纳但还没提交的引用（相对工作区根目录）。 */
  references: string[];
  /** 当前命中的候选项；为空表示不渲染浮层。 */
  options: FileEntryItem[];
  /** 活跃 token 的目录部分，用于候选项展示。 */
  directory: string;
  /** 当前选中项。 */
  index: number;
  setIndex: (index: number) => void;
  /** 采纳某一项；返回写回正文的文本与新的光标位置，无法采纳时为 null。 */
  accept: (index: number, text: string) => { text: string; caret: number } | null;
  /** 关闭浮层（Esc）。 */
  dismiss: () => void;
  /** 提交之后清空引用与缓存。 */
  reset: () => void;
}

export function useReferenceCompletion({
  workspaceRoot,
  text,
  caret,
}: ReferenceCompletionInput): ReferenceCompletion {
  const [references, setReferences] = React.useState<string[]>([]);
  const [token, setToken] = React.useState<ReferenceToken | null>(null);
  const [options, setOptions] = React.useState<FileEntryItem[]>([]);
  const [index, setIndex] = React.useState(0);
  /** 目录到条目与读取时间的缓存。 */
  const cacheRef = React.useRef(new Map<string, { entries: FileEntryItem[]; atMs: number }>());

  React.useEffect(() => {
    if (workspaceRoot === null) {
      setToken(null);
      setOptions([]);
      return undefined;
    }
    const active = activeReferenceToken(text, caret);
    if (active === null) {
      setToken(null);
      setOptions([]);
      return undefined;
    }
    setToken(active);
    const directory = directoryArgument(active.directory);
    const cached = cacheRef.current.get(directory);
    if (cached !== undefined && Date.now() - cached.atMs < DIRECTORY_CACHE_TTL_MS) {
      setOptions(matchReferenceCandidates(cached.entries, active.prefix));
      setIndex(0);
      return undefined;
    }
    let cancelled = false;
    void ipc
      .listWorkspaceEntries(workspaceRoot, directory)
      .then((entries) => {
        cacheRef.current.set(directory, { entries, atMs: Date.now() });
        if (cancelled) return;
        setOptions(matchReferenceCandidates(entries, active.prefix));
        setIndex(0);
      })
      .catch(() => {
        // 目录读不到（越界、已删除、无权限）时只是没有候选，不打扰用户。
        if (!cancelled) setOptions([]);
      });
    return () => {
      cancelled = true;
    };
    // caret 必须在依赖里：光标移动本身不改变正文，但会改变活跃 token。
  }, [text, caret, workspaceRoot]);

  const accept = React.useCallback(
    (position: number, draft: string) => {
      const entry = options[position];
      if (entry === undefined || token === null) return null;
      const insertion = referenceInsertion(entry);
      const next = `${draft.slice(0, token.start)}@${token.directory}${insertion}${draft.slice(token.end)}`;
      if (entry.kind !== 'dir') {
        setReferences((current) =>
          current.includes(entry.path) ? current : [...current, entry.path],
        );
      }
      return { text: next, caret: token.start + 1 + token.directory.length + insertion.length };
    },
    [options, token],
  );

  const dismiss = React.useCallback(() => {
    setToken(null);
    setOptions([]);
  }, []);

  const reset = React.useCallback(() => {
    setReferences([]);
    setToken(null);
    setOptions([]);
    cacheRef.current.clear();
  }, []);

  return {
    references,
    options,
    directory: token?.directory ?? '',
    index,
    setIndex,
    accept,
    dismiss,
    reset,
  };
}
