import React from 'react';

import { SHORTCUT_GROUPS, type ShortcutEntry } from '../../shortcuts';
import { Input, Kbd } from '../ui/controls';
import { Dialog } from '../ui/overlays';
import { Text } from '../ui/surfaces';

/*
 * 快捷键速查弹窗。
 *
 * 与设置里其它分区的关系：这一屏只回答「有哪些键可按」，不做键位自定义
 * （自定义需要一套冲突检测与持久化协议，本工作台尚未提供，界面不伪装成可编辑）。
 *
 * 三条实现纪律：
 * 1. 数据只有一份（`src/shortcuts.ts`），弹窗只负责筛选与排版；
 * 2. 搜索同时命中动作名、键位与说明，输入 `ctrl` 或 `esc` 都能直接筛出来；
 * 3. **层级是两级的**：组标题吸顶并带条目数，组内行缩进在一根发丝引导线之后，
 *    动作名与说明在左、键帽在右。分组不是装饰：它决定「这个键属于哪一层」
 *    （外壳、对话、输入坞、浮层），读者按层找比按字母找快。
 */

/** 一条快捷键的检索文本。 */
function searchText(entry: ShortcutEntry): string {
  return [entry.label, entry.keys.join(' '), entry.note ?? ''].join(' ').toLowerCase();
}

export interface ShortcutsDialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭。 */
  onClose: () => void;
}

export const ShortcutsDialog: React.FC<ShortcutsDialogProps> = ({ open, onClose }) => {
  const [query, setQuery] = React.useState('');

  // 每次打开都回到干净状态：上一次的筛选词留着会让「查看」变成「继续上次的筛选」。
  React.useEffect(() => {
    if (open) setQuery('');
  }, [open]);

  const keyword = query.trim().toLowerCase();
  const groups = React.useMemo(
    () =>
      SHORTCUT_GROUPS.map((group) => ({
        ...group,
        entries:
          keyword.length === 0
            ? group.entries
            : group.entries.filter((entry) => searchText(entry).includes(keyword)),
      })).filter((group) => group.entries.length > 0),
    [keyword],
  );

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="快捷键"
      size="lg"
      primary={{ label: '知道了', onClick: onClose }}
    >
      <Input
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        placeholder="搜索动作或键位"
        aria-label="搜索快捷键"
        onClear={() => setQuery('')}
      />

      {groups.length === 0 ? (
        <div className="py-8 text-center">
          <Text tone="tertiary" size="row">
            没有命中的快捷键
          </Text>
        </div>
      ) : (
        <div className="space-y-1">
          {groups.map((group) => (
            <section key={group.id} aria-label={group.title}>
              {/* 组标题吸顶：长列表滚到中段时仍然知道自己在哪一层。 */}
              <div className="sticky top-0 z-10 flex items-baseline gap-2 bg-surface pb-1 pt-2">
                <Text size="row" strong className="shrink-0">
                  {group.title}
                </Text>
                <Text tone="tertiary" size="micro" className="shrink-0 tabular-nums">
                  {group.entries.length} 条
                </Text>
                <span
                  aria-hidden="true"
                  className="mb-1 flex-1 h-hairline bg-border-hairline self-center"
                />
              </div>
              {/* 组内行缩进在一根发丝引导线之后：层级靠缩进与对齐表达，不靠底色。 */}
              <div className="ml-1 pl-3 border-l border-border-hairline">
                {group.entries.map((entry) => (
                  <div key={entry.id} className="flex items-start gap-3 py-1.5">
                    <div className="min-w-0 flex-1">
                      <Text size="row" className="block">
                        {entry.label}
                      </Text>
                      {entry.note !== undefined && (
                        <Text tone="tertiary" size="meta" className="mt-0.5 block">
                          {entry.note}
                        </Text>
                      )}
                    </div>
                    <span className="shrink-0 pt-px">
                      <Kbd keys={entry.keys} />
                    </span>
                  </div>
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
    </Dialog>
  );
};
