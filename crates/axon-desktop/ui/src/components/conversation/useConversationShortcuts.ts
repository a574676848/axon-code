import React from 'react';
import { escOwnedByAnotherLayer } from './conversationViewUtils';

/*
 * 会话主区域的全局快捷键。
 *
 * 两条键都挂在窗口上而不是某个元素上：本轮在跑时随手按 Esc 就该中断，
 * 右侧栏收起时也要能一键唤起。抽成 hook 是为了让 ConversationView 只留下
 * 版面与接线，不必为两组键盘逻辑再占篇幅。
 */

/**
 * Esc 中断当前轮：与 TUI 同语义，不需要先找到停止按钮。
 *
 * 空闲时不接管这一键，它仍归各浮层与草稿使用；长按 Esc 连发的 keydown
 * 只算一次中断，避免换来一串「无需中断」的回执。
 */
export function useInterruptOnEscape(running: boolean, interrupt: () => void): void {
  React.useEffect(() => {
    if (!running) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.isComposing) return;
      if (event.repeat) return;
      if (escOwnedByAnotherLayer(event.target)) return;
      event.preventDefault();
      interrupt();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [interrupt, running]);
}

/** 右侧栏的两个入口键：工作区文件与新建终端。 */
export function useRailShortcuts(openFiles: () => void, newTerminal: () => void): void {
  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      if (event.altKey && event.key.toLowerCase() === 'p') {
        event.preventDefault();
        openFiles();
        return;
      }
      if (event.altKey || event.shiftKey) return;
      if (event.key === '`' || event.code === 'Backquote') {
        event.preventDefault();
        newTerminal();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [newTerminal, openFiles]);
}
