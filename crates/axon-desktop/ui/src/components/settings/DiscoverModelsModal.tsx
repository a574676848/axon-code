import React, { useEffect, useMemo, useState } from 'react';
import { CheckIcon, CloseIcon, SearchIcon } from '../../icons';
import type { DiscoveredModel } from '../../types';
import { Button, Checkbox, Input } from '../ui/controls';
import { Dialog } from '../ui/overlays';
import { EmptyState, Text } from '../ui/surfaces';

/*
 * 上游候选模型选择器。
 * 探测结果只是候选：勾选并确认后才会进入草稿，绝不直接写配置。
 * 已经配置过的模型默认不勾选，避免覆盖用户手工校正过的容量参数。
 */

interface DiscoverModelsModalProps {
  /** 是否打开。 */
  open: boolean;
  /** 探测到的候选模型。 */
  candidates: DiscoveredModel[];
  /** 已配置的模型标识。 */
  knownIds: Set<string>;
  /** 实际请求的端点，用于向用户说明结果来自哪里。 */
  endpoint: string;
  /** 关闭。 */
  onClose: () => void;
  /** 采用所选候选。 */
  onAdopt: (models: DiscoveredModel[]) => void;
}

export const DiscoverModelsModal: React.FC<DiscoverModelsModalProps> = ({
  open,
  candidates,
  knownIds,
  endpoint,
  onClose,
  onAdopt,
}) => {
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [query, setQuery] = useState('');

  /*
   * 只在弹窗打开或候选换了一批时重置勾选。
   *
   * 不能依赖 `candidates` 与 `knownIds` 的引用：父组件每次渲染都会新建它们
   * （`new Set(models.map(...))`、`answer?.models ?? []`），那样这个 effect 每次
   * 渲染都会重跑，把用户刚勾上的候选立刻清掉。
   */
  const batchKey = candidates.map((model) => model.id).join('\n');
  const latest = React.useRef({ candidates, knownIds });
  latest.current = { candidates, knownIds };

  useEffect(() => {
    if (!open) return;
    setQuery('');
    const { candidates: batch, knownIds: known } = latest.current;
    setPicked(new Set(batch.filter((model) => !known.has(model.id)).map((model) => model.id)));
  }, [open, batchKey]);

  const visible = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (keyword.length === 0) return candidates;
    return candidates.filter(
      (model) =>
        model.id.toLowerCase().includes(keyword) ||
        (model.name ?? '').toLowerCase().includes(keyword),
    );
  }, [candidates, query]);

  const allPicked = visible.length > 0 && visible.every((model) => picked.has(model.id));

  const toggle = (id: string) => {
    setPicked((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="获取模型列表"
      size="lg"
      secondary={{ label: '取消', icon: <CloseIcon size={13} />, onClick: onClose }}
      primary={{
        label: picked.size > 0 ? `采用所选（${picked.size}）` : '采用所选',
        icon: <CheckIcon size={13} />,
        disabled: picked.size === 0,
        onClick: () => onAdopt(candidates.filter((model) => picked.has(model.id))),
      }}
    >
      <div className="w-full flex flex-col min-h-0">
        <div className="shrink-0 space-y-2">
          <Text tone="secondary" size="xs" className="block leading-relaxed">
            已从 {endpoint} 读取到 {candidates.length} 个候选模型。确认后仅加入草稿，仍需保存才会写入配置。
          </Text>
          <div className="flex items-center gap-2">
            <Input
              icon={<SearchIcon size={13} />}
              value={query}
              placeholder="搜索模型标识"
              onChange={(event) => setQuery(event.target.value)}
              className="flex-1"
            />
            <Button
              variant="outline"
              size="sm"
              disabled={visible.length === 0}
              onClick={() =>
                setPicked(allPicked ? new Set() : new Set(visible.map((model) => model.id)))
              }
            >
              {allPicked ? '取消全选' : '全选'}
            </Button>
          </div>
        </div>

        {/*
          候选列表自带高度上限并自己滚动：候选可能有上百条，交给外层对话框滚动会
          把搜索框一起滚走，而 `flex-1 min-h-0` 在「父级高度由内容决定」时又是失效的。
        */}
        <div className="min-h-0 max-h-[52vh] overflow-y-auto px-1">
          {visible.length === 0 ? (
            <EmptyState title="没有匹配的候选模型" description="换一个关键词再试" />
          ) : (
            <div className="space-y-0.5">
              {visible.map((model) => (
                <div
                  key={model.id}
                  className="flex items-center gap-2 h-8 px-2 rounded-sm hover:bg-hover transition-colors duration-100"
                >
                  <Checkbox
                    label={model.id}
                    checked={picked.has(model.id)}
                    onChange={() => toggle(model.id)}
                  >
                    <Text mono truncate title={model.name ?? model.id}>
                      {model.id}
                    </Text>
                  </Checkbox>
                  {knownIds.has(model.id) && (
                    <Text tone="tertiary" size="xs" className="shrink-0">
                      已配置
                    </Text>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </Dialog>
  );
};
