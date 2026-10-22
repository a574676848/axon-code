import React from 'react';

import { CloseIcon } from '../../icons';

/*
 * 输入坞里的条目卡外壳：类型徽标 + 主行 + 次行 + 移除。
 *
 * 附件与链接共用这一份几何（224 乘 52、12px 圆角、0.5px 发丝描边、底取卡片白），
 * 因此输入区的卡片永远只有一种尺寸与一种悬浮反馈，两侧只提供自己的徽标与两行文字。
 * 同一张卡也出现在用户消息气泡上方，那里没有移除按钮。
 */

/** 徽标：32 乘 32，图片放进真实缩略图，其余放类型图形。 */
export const ComposerBadge: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <span
    aria-hidden="true"
    className="shrink-0 w-8 h-8 flex items-center justify-center rounded-sm bg-surface-sunken overflow-hidden"
  >
    {children}
  </span>
);

export interface ComposerTileProps {
  /** 原生提示：附件给绝对路径，链接给完整地址。 */
  title: string;
  /** 左侧徽标。 */
  badge: React.ReactNode;
  /** 主行文字。 */
  label: string;
  /** 次行文字；为空时不占位。 */
  meta?: string;
  /** 打开动作；省略时主行只是文字。 */
  onOpen?: () => void;
  /** 打开动作的可访问名称；缺省时用主行文字。 */
  openLabel?: string;
  /** 移除动作；省略时不渲染移除按钮（消息气泡里的只读展示）。 */
  onRemove?: () => void;
  /** 移除动作的可访问名称；缺省时用主行文字。 */
  removeLabel?: string;
}

export const ComposerTile: React.FC<ComposerTileProps> = ({
  title,
  badge,
  label,
  meta,
  onOpen,
  openLabel,
  onRemove,
  removeLabel,
}) => {
  const body = (
    <>
      {badge}
      <span className="flex-1 min-w-0 flex flex-col text-left">
        <span className="truncate text-row text-text-primary">{label}</span>
        {meta !== undefined && meta.length > 0 && (
          <span className="truncate text-code text-text-tertiary">{meta}</span>
        )}
      </span>
    </>
  );
  return (
    <span
      title={title}
      className="shrink-0 h-[52px] w-[224px] pl-2 pr-1 flex items-center gap-2.5 rounded-md hairline border-border-hairline bg-surface"
    >
      {onOpen === undefined ? (
        <span className="flex-1 min-w-0 flex items-center gap-2.5">{body}</span>
      ) : (
        <button
          type="button"
          aria-label={openLabel ?? `打开 ${label}`}
          onClick={onOpen}
          className="flex-1 min-w-0 flex items-center gap-2.5 rounded-sm transition-colors duration-100 hover:bg-hover"
        >
          {body}
        </button>
      )}
      {onRemove !== undefined && (
        <button
          type="button"
          aria-label={removeLabel ?? `移除 ${label}`}
          title="移除"
          onClick={onRemove}
          className="shrink-0 w-5 h-5 flex items-center justify-center rounded-xs text-text-tertiary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
        >
          <CloseIcon size={12} />
        </button>
      )}
    </span>
  );
};
