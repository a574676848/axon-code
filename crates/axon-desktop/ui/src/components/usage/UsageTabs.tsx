import React from 'react';

/*
 * 用量看板的页签条。
 *
 * 与筛选控件分工明确：页签是**导航**（换一屏内容），因此用文字加下划线指示器；
 * 周期、维度、窗口这些是**筛选**（同一屏换一个口径），用分段控件。两者外观
 * 不共享，切换页签与切换口径在视觉上就不会被读成同一件事。
 *
 * 键盘行为按 tablist 的惯例：左右方向键在页签之间移动并立即切换（不额外要求回车），
 * Home 与 End 到两端。
 */

export interface UsageTabSpec<T extends string> {
  value: T;
  label: string;
}

interface UsageTabStripProps<T extends string> {
  label: string;
  tabs: readonly UsageTabSpec<T>[];
  value: T;
  onChange: (value: T) => void;
  /** 右侧操作区：周期与刷新这些跨页签的控件停在这里。 */
  trailing?: React.ReactNode;
}

export const UsageTabStrip = <T extends string>({
  label,
  tabs,
  value,
  onChange,
  trailing,
}: UsageTabStripProps<T>): React.ReactElement => {
  const handleKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const index = tabs.findIndex((tab) => tab.value === value);
    if (index < 0) return;
    if (event.key === 'ArrowRight') {
      event.preventDefault();
      onChange(tabs[(index + 1) % tabs.length].value);
    } else if (event.key === 'ArrowLeft') {
      event.preventDefault();
      onChange(tabs[(index - 1 + tabs.length) % tabs.length].value);
    } else if (event.key === 'Home') {
      event.preventDefault();
      onChange(tabs[0].value);
    } else if (event.key === 'End') {
      event.preventDefault();
      onChange(tabs[tabs.length - 1].value);
    }
  };

  return (
    <div className="sticky top-0 z-10 bg-surface-sunken pb-2.5 hairline-b border-border-hairline">
      <div className="flex items-end justify-between gap-3 flex-wrap">
        <div role="tablist" aria-label={label} className="flex items-center gap-4" onKeyDown={handleKeyDown}>
          {tabs.map((tab) => {
            const active = tab.value === value;
            return (
              <button
                key={tab.value}
                type="button"
                role="tab"
                aria-selected={active}
                tabIndex={active ? 0 : -1}
                onClick={() => onChange(tab.value)}
                className={`relative h-7 -mb-2.5 px-0.5 text-row whitespace-nowrap transition-colors duration-100 ${
                  active
                    ? 'text-text-primary font-medium'
                    : 'text-text-secondary hover:text-text-primary'
                }`}
              >
                {tab.label}
                {/* 指示器只占一条 2px 的线：它标记当前位置，不参与排版权重。 */}
                <span
                  className={`absolute left-0 right-0 bottom-0 h-0.5 rounded-full transition-opacity duration-100 ${
                    active ? 'bg-primary opacity-100' : 'opacity-0'
                  }`}
                  aria-hidden="true"
                />
              </button>
            );
          })}
        </div>
        {trailing !== undefined && (
          <div className="flex items-center gap-2 shrink-0 pb-0.5">{trailing}</div>
        )}
      </div>
    </div>
  );
};
