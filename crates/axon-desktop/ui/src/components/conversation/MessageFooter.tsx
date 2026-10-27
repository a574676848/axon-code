import React from 'react';
import { formatMessageTime } from '../../conversation/format';
import { MESSAGE_ACTION_SIZE, MESSAGE_FOOTER_HEIGHT, MESSAGE_TIME_CLASS } from './chatChrome';

/*
 * 消息页脚：用户气泡、助手正文与问答单元共用同一套摆放。
 *
 * 时间是这一格的事实，因此常显；操作按钮只在悬浮或键盘聚焦时出现。
 * 此前整行用 `opacity-0` 藏起来：静止时既看不到时间，又白占 24px，
 * 而且每个操作都要先猜哪里能悬浮。显示切换不占位，行高不会因此跳动。
 */

export const MessageActionButton: React.FC<{
  label: string;
  title: string;
  onClick: () => void;
  disabled?: boolean;
  /** 按下态：朗读进行中的那一枚按钮要用它表达「再点一次即停止」。 */
  pressed?: boolean;
  /** 回执态配色：成功取成功墨水档，失败取危险色，「等你处理」取琥珀墨水档，
   *  按下态取主文本色加选中底。 */
  tone?: 'default' | 'ok' | 'danger' | 'gold' | 'active';
  children: React.ReactNode;
}> = ({ label, title, onClick, disabled = false, pressed, tone = 'default', children }) => (
  <button
    type="button"
    aria-label={label}
    aria-pressed={pressed}
    title={title}
    disabled={disabled}
    onClick={onClick}
    className={`${MESSAGE_ACTION_SIZE} flex items-center justify-center rounded-sm transition-colors duration-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus hover:bg-hover hover:text-text-primary disabled:cursor-wait disabled:opacity-40 ${
      tone === 'ok'
        ? 'text-ok-ink'
        : tone === 'danger'
          ? 'text-danger'
          : tone === 'gold'
            ? 'text-gold-ink'
            : tone === 'active'
              ? 'bg-active text-text-primary'
              : 'text-text-tertiary'
    }`}
  >
    {children}
  </button>
);

export interface MessageFooterProps {
  /** 这一格的服务端记录时间；为空时不显示时间。 */
  atMs: number | null;
  /** 用户消息靠右，其余靠左。 */
  align?: 'start' | 'end';
  /** 常显的回执（例如「已导出到 …」）：不属于悬浮才出现的操作区。 */
  status?: React.ReactNode;
  /**
   * 强制显示操作区。
   *
   * 朗读进行中时用：停止入口必须看得见，不能等指针悬停才出现。
   */
  revealActions?: boolean;
  /** 操作按钮；为空时页脚只承载时间。 */
  children?: React.ReactNode;
}

export const MessageFooter: React.FC<MessageFooterProps> = ({
  atMs,
  align = 'start',
  status,
  revealActions = false,
  children,
}) => (
  <div
    className={`${MESSAGE_FOOTER_HEIGHT} flex items-center gap-0.5 ${
      align === 'end' ? 'justify-end' : ''
    }`}
  >
    {atMs !== null && <span className={MESSAGE_TIME_CLASS}>{formatMessageTime(atMs)}</span>}
    {status}
    {children !== undefined && (
      <div
        className={`${revealActions ? 'flex' : 'hidden group-hover:flex group-focus-within:flex'} items-center gap-0.5`}
      >
        {children}
      </div>
    )}
  </div>
);
