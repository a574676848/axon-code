import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { rankCandidateList } from '../../conversation/composerCandidates';
import { Kbd } from './controls';
import { Text } from './surfaces';
import { useExitLifecycle } from './useExitLifecycle';

/*
 * 命令面板：`Ctrl + K` 与 `Ctrl + F` 的唯一落点。
 *
 * 桌面工具的「查找」语义不是在一屏文字里高亮，而是「找到某个东西并跳过去」，
 * 因此这里统一收纳跨视图跳转、工作区与会话、设置分区与已安装技能。
 *
 * 三条实现纪律：
 * 1. 候选匹配复用输入坞那一份有序子序列排序（`rankCandidateList`），不写第二套模糊匹配；
 * 2. 键盘独占：上下移动、Enter 执行、Esc 收起，指针与键盘共用同一个下标；
 * 3. 收起走 110ms 退场（`useExitLifecycle`），不是瞬时消失。
 */

/** 一条可执行命令。 */
export interface CommandPaletteItem {
  /** 稳定标识。 */
  id: string;
  /** 主文案，同时是模糊匹配的对象。 */
  label: string;
  /** 右侧类别标签。 */
  group: string;
  /** 右侧快捷键角标；没有就只显示类别。 */
  keys?: readonly string[];
  /** 执行。 */
  run: () => void;
}

export interface CommandPaletteProps {
  /** 是否打开。 */
  open: boolean;
  /** 收起。 */
  onClose: () => void;
  /** 全部候选；筛选与排序在面板内部完成。 */
  items: readonly CommandPaletteItem[];
}

/** 面板宽度与列表高度上限：搜索框固定，列表在内部滚动。 */
const PANEL_WIDTH = 560;
const LIST_MAX_HEIGHT = 336;
const ROW_HEIGHT = 32;

/** 匹配用的候选形态：复用 `rankCandidateList` 的 `name` 字段。 */
interface RankableItem extends CommandPaletteItem {
  name: string;
}

export const CommandPalette: React.FC<CommandPaletteProps> = ({ open, onClose, items }) => {
  const lifecycle = useExitLifecycle(open);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const rankable = useMemo<RankableItem[]>(
    () => items.map((item) => ({ ...item, name: item.label })),
    [items],
  );
  const matches = useMemo(() => rankCandidateList(rankable, query), [rankable, query]);

  /*
   * 每次打开都回到干净状态：上一次的查询词留着会让「点开面板」变成
   * 「继续上次的筛选」，与「这是一次新的跳转意图」冲突。
   */
  useEffect(() => {
    if (!open) return;
    setQuery('');
    setActive(0);
    // 焦点落到搜索框：面板打开后第一件事就是能立刻打字。
    window.requestAnimationFrame(() => inputRef.current?.focus());
  }, [open]);

  /** 命中集合变化后把下标夹回合法区间，避免高亮停在一个已经不存在的项上。 */
  useEffect(() => {
    setActive((current) => (current >= matches.length ? Math.max(0, matches.length - 1) : current));
  }, [matches.length]);

  /** 高亮行始终留在可视区内：只改容器的 scrollTop，不触发整页滚动。 */
  useEffect(() => {
    const list = listRef.current;
    if (list === null) return;
    const top = active * ROW_HEIGHT;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (top + ROW_HEIGHT > list.scrollTop + list.clientHeight) {
      list.scrollTop = top + ROW_HEIGHT - list.clientHeight;
    }
  }, [active]);

  const runItem = useCallback(
    (item: CommandPaletteItem | undefined) => {
      if (item === undefined) return;
      // 先收起再执行：执行里可能打开设置弹窗或切换视图，
      // 面板留着会盖在新内容上，读起来像点错了。
      onClose();
      item.run();
    },
    [onClose],
  );

  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (matches.length === 0) return;
      const delta = event.key === 'ArrowDown' ? 1 : -1;
      setActive((current) => (current + delta + matches.length) % matches.length);
      return;
    }
    if (event.key === 'Enter') {
      event.preventDefault();
      runItem(matches[active]);
    }
  };

  if (!lifecycle.mounted) return null;

  return createPortal(
    <div
      className={`fixed inset-0 z-[1200] flex items-start justify-center pt-[12vh] px-4 ${
        lifecycle.state === 'closed' ? 'pointer-events-none' : ''
      }`}
      onKeyDown={handleKeyDown}
    >
      {/* 遮罩只动透明度，模糊半径恒定：动画化 backdrop-filter 会让整屏逐帧重采样。 */}
      <div
        data-state={lifecycle.state}
        className="modal-static-backdrop absolute inset-0"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
        data-state={lifecycle.state}
        style={{ width: PANEL_WIDTH }}
        className="modal-spring-surface overlay-panel-in relative flex max-w-full flex-col overflow-clip rounded-lg bg-surface shadow-modal hairline border-border"
      >
        <div className="flex items-center gap-2 px-3 h-11 shrink-0 hairline-b border-border-hairline">
          <input
            ref={inputRef}
            value={query}
            onChange={(event) => {
              setQuery(event.target.value);
              setActive(0);
            }}
            placeholder="搜索页面、设置、会话与技能"
            aria-label="搜索命令"
            aria-controls="command-palette-list"
            aria-activedescendant={matches[active] === undefined ? undefined : `cmd-${matches[active].id}`}
            className="flex-1 min-w-0 h-full bg-transparent border-none outline-none text-row text-text-primary placeholder:text-text-placeholder"
          />
          <Kbd keys={['Esc']} />
        </div>

        <div
          ref={listRef}
          id="command-palette-list"
          role="listbox"
          aria-label="命令候选"
          className="scroll-visible overflow-y-auto p-1"
          style={{ maxHeight: LIST_MAX_HEIGHT }}
        >
          {matches.length === 0 ? (
            <div className="px-2.5 py-6 text-center">
              <Text tone="tertiary" size="row">
                没有命中的命令
              </Text>
            </div>
          ) : (
            matches.map((item, index) => (
              <button
                key={item.id}
                id={`cmd-${item.id}`}
                type="button"
                role="option"
                aria-selected={index === active}
                // 指针与键盘共用一个下标：只有一层高亮，不会出现「指针停一行、键盘选另一行」。
                onMouseEnter={() => setActive(index)}
                onClick={() => runItem(item)}
                className={`focus-ring-soft w-full h-8 flex items-center gap-2 px-2.5 rounded-xs text-row text-left select-none ${
                  index === active ? 'bg-row-hover text-text-primary' : 'text-text-secondary'
                }`}
              >
                <span className="flex-1 min-w-0 truncate">{item.label}</span>
                <Text tone="tertiary" size="meta" className="shrink-0">
                  {item.group}
                </Text>
                {item.keys !== undefined && item.keys.length > 0 && <Kbd keys={item.keys} />}
              </button>
            ))
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
};
