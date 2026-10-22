import React from 'react';
import { compactUrlLabel, type LinkToken } from '../../conversation/linkTokens';
import { GlobeIcon } from '../../icons';
import { ComposerBadge, ComposerTile } from './ComposerTile';

/*
 * 链接卡：与附件卡同一份几何，徽标换成地球，主行是去掉查询串的站点与路径。
 *
 * 完整地址挂在卡片的原生提示上：主行只有一行宽，够读站点与路径，
 * 但要核对这一段是不是粘对了，仍然要能看到全文。
 */
export const LinkTile: React.FC<{
  link: LinkToken;
  /** 点击卡片：用系统浏览器打开。 */
  onOpen?: () => void;
  /** 省略时不渲染移除按钮。 */
  onRemove?: () => void;
}> = ({ link, onOpen, onRemove }) => (
  <ComposerTile
    title={link.url}
    badge={
      <ComposerBadge>
        <GlobeIcon size={16} className="text-text-secondary" />
      </ComposerBadge>
    }
    label={compactUrlLabel(link.url)}
    meta="网页链接"
    onOpen={onOpen}
    openLabel={`用系统浏览器打开 ${link.url}`}
    onRemove={onRemove}
    removeLabel={`移除链接 ${link.placeholder}`}
  />
);
