import React from 'react';
import { AxonLogo } from '../../icons';

/*
 * 首屏。
 *
 * 只两组：品牌组（标识 + 字标）与主张组（标语 + 中文使命）。
 * 标识直接走 AxonLogo 矢量组件，随三套主题自动切换金属蚀刻色板。
 */

export const ConversationHero: React.FC = () => (
  <div className="w-full max-w-chat mx-auto flex flex-col items-center gap-6 px-6 text-center select-none">
    <div className="flex items-center gap-3">
      <AxonLogo size={38} aria-hidden="true" className="shrink-0 drop-shadow-xs" />
      <span className="text-display font-semibold tracking-[-0.025em] text-text-primary">
        Axon
      </span>
    </div>
    <div className="flex flex-col items-center gap-2">
      <p className="text-heading-1 font-semibold tracking-[-0.02em] text-text-primary">
        知你所想，办你所托
      </p>
      <p className="max-w-lg text-body text-text-secondary">
        融入你的工作区与工作节奏，在每一次交互中持续深化认知的专属个人智能体。
      </p>
    </div>
  </div>
);
