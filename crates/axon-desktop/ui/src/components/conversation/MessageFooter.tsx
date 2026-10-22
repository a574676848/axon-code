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
  /** 回执态配色：成功取成功墨水档，失败取危险色，「等你处理」取琥珀墨水档。 */
  tone?: 'default' | 'ok' | 'danger' | 'gold';
  children: React.ReactNode;
}> = ({ label, title, onClick, disabled = false, tone = 'default', children }) => (
  <button
    type="button"
    aria-label={label}
    title={title}
    disabled={disabled}
    onClick={onClick}
    className={`${MESSAGE_ACTION_SIZE} flex items-center justify-center rounded-sm transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-wait disabled:opacity-40 ${
      tone === 'ok'
        ? 'text-ok-ink'
        : tone === 'danger'
          ? 'text-danger'
          : tone === 'gold'
            ? 'text-gold-ink'
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
  /** 操作按钮；为空时页脚只承载时间。 */
  children?: React.ReactNode;
}

export const MessageFooter: React.FC<MessageFooterProps> = ({
  atMs,
  align = 'start',
  status,
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
      <div className="hidden group-hover:flex group-focus-within:flex items-center gap-0.5">
        {children}
      </div>
    )}
  </div>
);
