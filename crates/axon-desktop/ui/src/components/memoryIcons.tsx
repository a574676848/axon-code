import React from 'react';

/**
 * 记忆管理页专用图标。
 *
 * 全局 `icons.tsx` 已到 700 行硬限，因此按域就近放置（与 `conversation/fileIcons.tsx`
 * 的做法一致）：只承载记忆生命周期里独有的图形，通用图标仍然走 `../icons`。
 */

/** 取消归档：与 `ArchiveIcon` 同一只收纳箱，箱内改为向上取出的箭头。 */
export const ArchiveRestoreIcon: React.FC<{ size?: number; className?: string }> = ({
  size = 16,
  className = '',
}) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 16 16"
    fill="none"
    strokeWidth="1"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
    className={className}
  >
    <path
      d="M13.5 2.5H2.5C1.94772 2.5 1.5 2.94772 1.5 3.5V4.5C1.5 5.05228 1.94772 5.5 2.5 5.5H13.5C14.0523 5.5 14.5 5.05228 14.5 4.5V3.5C14.5 2.94772 14.0523 2.5 13.5 2.5Z"
      stroke="currentColor"
    />
    <path
      d="M2.5 5.5V13.5C2.5 13.7652 2.60536 14.0196 2.79289 14.2071C2.98043 14.3946 3.23478 14.5 3.5 14.5H12.5C12.7652 14.5 13.0196 14.3946 13.2071 14.2071C13.3946 14.0196 13.5 13.7652 13.5 13.5V5.5"
      stroke="currentColor"
    />
    <path d="M8 13.5V9M6 10.8L8 8.8L10 10.8" stroke="currentColor" />
  </svg>
);
