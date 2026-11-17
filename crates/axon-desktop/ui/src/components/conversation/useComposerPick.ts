import React from 'react';
import * as ipc from '../../ipc';
import {
  EMPTY_COMPOSER_PICK,
  inheritedComposerPick,
  type ComposerPick,
} from '../../conversation/composerPick';

/*
 * 输入坞显式选择的跨会话缓存。
 *
 * 读一次、写一次：挂载时读客户端状态文件里的那一份，用户改选时写回。它不碰
 * `config.toml`，也不参与任何轮次的提交事实：提交仍由调用方按显式值下发，
 * 这里只负责让下一次新开会话知道上一次选的是什么。
 */

/** 输入坞选择缓存对外给出的能力。 */
export interface ComposerPickController {
  /** 新会话可继承的选择；模型名称已不在配置里时整份为空。 */
  inherited: ComposerPick;
  /** 记下用户改选的模型名称。 */
  rememberModel: (value: string) => void;
  /** 记下用户改选的思考强度。 */
  rememberReasoningEffort: (value: string) => void;
}

export function useComposerPick(modelNames: readonly string[]): ComposerPickController {
  const [cache, setCache] = React.useState<ComposerPick>(EMPTY_COMPOSER_PICK);

  React.useEffect(() => {
    let cancelled = false;
    void ipc
      .desktopPreferences()
      .then((preferences) => {
        if (cancelled) return;
        setCache({
          model: preferences.composerModel ?? null,
          reasoningEffort: preferences.composerReasoningEffort ?? null,
        });
      })
      .catch(() => {
        // 读不到就当作没有缓存：新会话仍按配置里的默认模型与强度走。
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const rememberModel = React.useCallback((value: string) => {
    setCache((previous) => ({ ...previous, model: value }));
    void ipc.desktopSetComposerModel(value).catch(() => undefined);
  }, []);

  const rememberReasoningEffort = React.useCallback((value: string) => {
    setCache((previous) => ({ ...previous, reasoningEffort: value }));
    void ipc.desktopSetComposerReasoningEffort(value).catch(() => undefined);
  }, []);

  const inherited = React.useMemo(
    () => inheritedComposerPick(modelNames, cache),
    [modelNames, cache],
  );

  return { inherited, rememberModel, rememberReasoningEffort };
}
