import React from 'react';

import { ChevronRightIcon } from '../../icons';
import { Text } from '../ui/surfaces';

/*
 * 会话来源条：从目标看板这类页面进入的后台会话，在指标条下给一条返回入口。
 *
 * 几何与观感与子代理会话的「返回会话」逐项一致：36px 高、发丝下边、12px 箭头
 * 反转成返回方向、`·` 分隔后跟标题。这里只多一件事：返回的目标是**页面**，
 * 因此文案直接写明回哪一页。
 */

export interface SessionOriginBarProps {
  /** 返回文案，例如「返回看板」。 */
  backLabel: string;
  /** 当前会话的标题。 */
  title: string;
  /** 本会话实际使用的模型；为空时不占这一格。 */
  model?: string | null;
  onBack: () => void;
}

export const SessionOriginBar: React.FC<SessionOriginBarProps> = ({
  backLabel,
  title,
  model = null,
  onBack,
}) => (
  <div className="shrink-0 h-9 px-4 flex items-center gap-2 hairline-b border-border-hairline">
    <button
      type="button"
      onClick={onBack}
      className="shrink-0 h-6 pl-1 pr-2 inline-flex items-center gap-0.5 rounded-xs text-meta text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
    >
      <ChevronRightIcon size={12} className="rotate-180" />
      {backLabel}
    </button>
    <Text tone="tertiary" size="meta" className="shrink-0">
      ·
    </Text>
    <Text size="row" truncate className="min-w-0">
      {title}
    </Text>
    {model !== null && model.length > 0 && (
      <>
        <Text tone="tertiary" size="meta" className="shrink-0">
          ·
        </Text>
        <Text tone="tertiary" size="xs" mono truncate className="shrink-0">
          {model}
        </Text>
      </>
    )}
  </div>
);
