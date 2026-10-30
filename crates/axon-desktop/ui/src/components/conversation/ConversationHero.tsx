import React from 'react';
import { AxonLogo } from '../../icons';
import { BRAND_SLOGAN } from '../../brand';

/*
 * 首屏。
 *
 * 品牌组（双层微立体徽标座 + 字标）与主张组（标语 + 中文使命）。
 * 标语与设置 › 关于共用同一处定义。
 */

export const ConversationHero: React.FC = () => (
  <div className="w-full max-w-chat mx-auto flex flex-col items-center gap-5 px-6 text-center select-none">
    <div className="flex items-center gap-3">
      <span className="p-1.5 rounded-[12px] bg-surface shadow-lift hairline border-border inline-flex items-center justify-center">
        <AxonLogo size={34} aria-hidden="true" className="shrink-0" />
      </span>
      <span className="text-display font-semibold tracking-[-0.025em] text-text-primary">
        Axon
      </span>
    </div>
    <div className="flex flex-col items-center gap-2">
      <p className="text-heading-1 font-semibold tracking-[-0.02em] text-text-primary">
        {BRAND_SLOGAN}
      </p>
      <p className="max-w-md text-body text-text-secondary leading-6">
        融入你的工作区与工作节奏，在每一次交互中持续深化认知的专属个人智能体。
      </p>
    </div>
  </div>
);
