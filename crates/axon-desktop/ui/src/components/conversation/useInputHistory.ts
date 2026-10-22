import React from 'react';

import * as ipc from '../../ipc';
import {
  historyNext,
  historyPrevious,
  IDLE_HISTORY,
  type HistoryNavigation,
} from '../../conversation/inputHistory';

/*
 * 输入历史控制器。
 *
 * 历史与 TUI 共用同一份文件（`data/runtime/draft_history.jsonl`），
 * 因此「终端里发过的消息，桌面端按上键也能取回」。这里只负责取数、
 * 记录与导航状态；能翻不能翻、翻到哪一条由 `conversation/inputHistory` 决定。
 */

export interface InputHistoryController {
  /** 上一条历史；没有可翻的记录时返回 null。 */
  previous: (draft: string) => Promise<string | null>;
  /** 下一条历史；越过后最新一条即归还草稿。 */
  next: () => string | null;
  /** 提交后记录一条；写入失败时回调错误文案。 */
  record: (text: string, onError: (message: string) => void) => void;
  /** 提交之后退出导航状态。 */
  reset: () => void;
}

export function useInputHistory(): InputHistoryController {
  const [history, setHistory] = React.useState<string[]>([]);
  const [navigation, setNavigation] = React.useState<HistoryNavigation>(IDLE_HISTORY);

  // 历史只在挂载时读一次：文件很小，读一次比每次按上键都去问一遍更省事。
  React.useEffect(() => {
    let cancelled = false;
    void ipc
      .inputHistoryList()
      .then((entries) => {
        if (!cancelled) setHistory(entries);
      })
      .catch(() => {
        // 读不到历史只是没有可翻的记录，不打扰用户。
        if (!cancelled) setHistory([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const previous = React.useCallback(
    (draft: string): Promise<string | null> => {
      // 翻到一半就按内存里的快照走：同一次导航里不该被磁盘变化打断。
      if (navigation.index !== null) {
        const step = historyPrevious(history, navigation, draft);
        return Promise.resolve(step === null ? null : step.text);
      }
      /*
       * 重新开始一次导航时先把磁盘上的历史读一遍。
       *
       * 历史与 TUI 共用同一份文件，而终端那边随时可能刚发过一条消息；只在挂载时
       * 读一次的话，那些条目要等应用重启才翻得到。
       */
      const apply = (entries: string[]) => {
        const step = historyPrevious(entries, IDLE_HISTORY, draft);
        if (step === null) return null;
        setNavigation(step.navigation);
        return step.text;
      };
      return ipc
        .inputHistoryList()
        .then((entries) => {
          setHistory(entries);
          return apply(entries);
        })
        .catch(() => apply(history));
    },
    [history, navigation],
  );

  const next = React.useCallback(() => {
    const step = historyNext(history, navigation);
    if (step === null) return null;
    setNavigation(step.navigation);
    return step.text;
  }, [history, navigation]);

  const record = React.useCallback((text: string, onError: (message: string) => void) => {
    if (text.trim().length === 0) return;
    void ipc
      .inputHistoryAppend(text)
      .then(setHistory)
      .catch((error: unknown) =>
        onError(error instanceof Error ? error.message : '历史保存失败'),
      );
  }, []);

  const reset = React.useCallback(() => setNavigation(IDLE_HISTORY), []);

  return { previous, next, record, reset };
}
