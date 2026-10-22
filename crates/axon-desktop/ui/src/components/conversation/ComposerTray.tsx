import React from 'react';
import type { LinkToken } from '../../conversation/linkTokens';
import type { AttachmentRef } from '../../conversation/transcript';
import { AttachmentTile } from './AttachmentTile';
import { LinkTile } from './LinkTile';

/*
 * 输入坞上方的条目条：链接卡与附件卡共用一条横向滚动带。
 *
 * 链接在前、附件在后：链接卡对应正文里的占位符，紧贴输入框读起来是一条线；
 * 附件是随消息一起投递的素材。只有真有条目时才渲染这一条，不留空条。
 */
export const ComposerTray: React.FC<{
  links: readonly LinkToken[];
  attachments: readonly AttachmentRef[];
  /** 每个附件的打开动作；不可打开时返回 undefined（例如没有会话动作的文件）。 */
  attachmentOpen: (attachment: AttachmentRef) => (() => void) | undefined;
  onRemoveAttachment: (path: string) => void;
  onOpenLink: (url: string) => void;
  onRemoveLink: (placeholder: string) => void;
}> = ({
  links,
  attachments,
  attachmentOpen,
  onRemoveAttachment,
  onOpenLink,
  onRemoveLink,
}) => (
  <div className="flex items-center gap-2 min-w-0 overflow-x-auto px-4 pt-2.5 pb-1">
    {links.map((link) => (
      <LinkTile
        key={link.placeholder}
        link={link}
        onOpen={() => onOpenLink(link.url)}
        onRemove={() => onRemoveLink(link.placeholder)}
      />
    ))}
    {attachments.map((attachment) => (
      <AttachmentTile
        key={attachment.path}
        attachment={attachment}
        onOpen={attachmentOpen(attachment)}
        onRemove={() => onRemoveAttachment(attachment.path)}
      />
    ))}
  </div>
);
