import React from 'react';

/*
 * 收口行：一条发丝线加一行元信息。
 *
 * 轮次收尾分割线与轮后提示（记忆反思）共用这一份实现。两者说的是同一件事：这一段
 * 时间线到此为止，下面那一行属于这一次收口。字号、色阶与留白必须一致，各写一遍
 * 迟早会分叉。
 *
 * 分隔不用贯穿横线：上方留白加一条 0.5px 发丝线收口即可，读数与下一段正文之间仍是留白。
 */

export interface DividerMetaRowProps {
  /** 原生提示：读数较长时悬停给全文。 */
  title?: string;
  /** 这一行会在同一次会话里被替换：交给读屏器按礼貌播报，静态历史提示传空。 */
  ariaLive?: 'polite';
  children: React.ReactNode;
}

export const DividerMetaRow: React.FC<DividerMetaRowProps> = ({ title, ariaLive, children }) => (
  <div className="w-full pt-2">
    <div className="h-hairline bg-border-hairline" role="separator" />
    <div
      className="mt-2 text-meta leading-5 tabular-nums text-text-secondary"
      title={title}
      aria-live={ariaLive}
    >
      {children}
    </div>
  </div>
);
