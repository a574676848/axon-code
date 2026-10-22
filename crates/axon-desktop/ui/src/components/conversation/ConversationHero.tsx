import React from 'react';

/*
 * 首屏。
 *
 * 只两组：品牌组（标识 + 字标）与主张组（标语 + 中文使命）。
 * 此前是四行等距、字标与标语同号，读起来没有主次；英文副标是纯翻译、
 * 对比最低、还常常折出孤行，因此整行删除。
 *
 * 组内间距 10px（品牌）与 8px（主张），组间 24px。
 */

export const ConversationHero: React.FC = () => (
  <div className="w-full max-w-chat mx-auto flex flex-col items-center gap-6 px-6 text-center">
    <div className="flex items-center gap-2.5">
      <img src="axon-logo.svg" alt="" aria-hidden="true" className="w-9 h-9 rounded-sm" />
      <span className="text-display font-medium text-text-primary">Axon</span>
    </div>
    <div className="flex flex-col items-center gap-2">
      <p className="text-heading-1 font-medium tracking-tight text-text-primary">知你所想，办你所托</p>
      <p className="max-w-lg text-body text-text-secondary">
        融入你的工作区与工作节奏，在每一次交互中持续深化认知的专属个人智能体。
      </p>
    </div>
  </div>
);
