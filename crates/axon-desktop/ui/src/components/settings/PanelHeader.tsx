import React from 'react';
import { Text } from '../ui/surfaces';

/*
 * 面板标题行：标题 14/20 500、一句说明 12/16 secondary、右侧动作区。
 *
 * 用量图表面板、请求日志表与定价规则表此前各写了一套标题层，
 * 三处的字号、间距与动作对齐并不一致。收敛到这里之后，
 * 面板头只有一个来源，右侧动作始终与内容右轴同轴。
 */

export interface PanelHeaderProps {
  /** 面板名称。 */
  title: string;
  /** 一句说明：这块面板读的是什么。 */
  description?: string;
  /** 右侧动作区。 */
  action?: React.ReactNode;
  /** 附加类名，仅用于布局。 */
  className?: string;
}

export const PanelHeader: React.FC<PanelHeaderProps> = ({
  title,
  description,
  action,
  className = '',
}) => (
  <div
    className={`shrink-0 px-3.5 py-2.5 flex items-start justify-between gap-3 hairline-b border-border-hairline ${className}`}
  >
    <div className="min-w-0 space-y-0.5">
      <Text size="sm" strong className="block">
        {title}
      </Text>
      {description !== undefined && (
        <Text tone="secondary" size="xs" className="block">
          {description}
        </Text>
      )}
    </div>
    {action !== undefined && (
      <div className="shrink-0 flex items-center gap-2">{action}</div>
    )}
  </div>
);
