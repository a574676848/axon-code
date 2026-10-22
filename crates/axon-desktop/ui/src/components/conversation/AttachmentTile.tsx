import React from 'react';
import { attachmentMeta, attachmentMetaText } from '../../conversation/attachmentMeta';
import type { AttachmentRef } from '../../conversation/transcript';
import * as ipc from '../../ipc';
import { FileTypeIcon } from './fileIcons';
import { ComposerBadge, ComposerTile } from './ComposerTile';

/*
 * 附件的条目卡：类型徽标 + 文件名 + 类别与体量。
 *
 * 粘贴、拖放、系统选择器与大段文本落盘四条路径最终都是磁盘上的真实文件，
 * 因此卡片只呈现文件本身的事实：类型徽标（图片给缩略图）、文件名、类别与体量。
 * 不显示正文片段，也不把「粘贴来的」这类来源信息混进文件名之外的地方。
 *
 * 卡片外壳与链接卡共用 ComposerTile：移除是卡片自己的动作，因此按钮常显而不是
 * 悬浮才现形，输入区的附件随时可能被撤掉。
 */

export const AttachmentTile: React.FC<{
  attachment: AttachmentRef;
  /** 点击卡片的动作：图片给放大查看，文件给在右侧栏打开。 */
  onOpen?: () => void;
  /** 省略时不渲染移除按钮（例如消息气泡里的只读展示）。 */
  onRemove?: () => void;
}> = ({ attachment, onOpen, onRemove }) => {
  const metaText = attachmentMetaText(attachmentMeta(attachment));
  return (
    <ComposerTile
      title={attachment.path}
      badge={<AttachmentBadge attachment={attachment} />}
      label={attachment.name}
      meta={metaText}
      onOpen={onOpen}
      openLabel={`打开附件 ${attachment.name}`}
      onRemove={onRemove}
      removeLabel={`移除附件 ${attachment.name}`}
    />
  );
};

/** 类型徽标：图片给真实缩略图，其余走共用文件图标表。 */
const AttachmentBadge: React.FC<{ attachment: AttachmentRef }> = ({ attachment }) => {
  const [thumbnail, setThumbnail] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (!attachment.image) {
      setThumbnail(null);
      return;
    }
    let cancelled = false;
    void ipc
      .attachmentSource(attachment.path)
      .then((value) => {
        if (!cancelled) setThumbnail(value);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [attachment.image, attachment.path]);

  return (
    <ComposerBadge>
      {thumbnail !== null ? (
        <img src={thumbnail} alt="" className="w-full h-full object-cover" />
      ) : (
        <FileTypeIcon path={attachment.name} size={16} />
      )}
    </ComposerBadge>
  );
};
