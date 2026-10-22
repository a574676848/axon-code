import React from 'react';
import { compactUrlLabel } from '../../conversation/linkTokens';
import { openWebLink } from '../../conversation/openWebLink';
import { GlobeIcon } from '../../icons';

/*
 * 用户消息里的链接芯片：站点与路径代替整段地址。
 *
 * 消息原文不变：复制消息、重新发送与模型拿到的都是那段 URL，这里只是显示层的收敛。
 * 完整地址挂在原生提示上，因为芯片只够读站点与路径；打不开时把失败原因换到提示里。
 */
export const LinkChip: React.FC<{ url: string }> = ({ url }) => {
  const [failure, setFailure] = React.useState<string | null>(null);
  return (
    <button
      type="button"
      title={failure ?? url}
      aria-label={`打开链接 ${compactUrlLabel(url)}`}
      onClick={() => {
        void openWebLink(url).then((message) => setFailure(message));
      }}
      className="mx-0.5 inline-flex max-w-[240px] items-center gap-1 rounded-sm bg-surface px-1.5 py-px align-middle text-code text-text-link hairline border-border transition-colors duration-100 hover:bg-hover"
    >
      <GlobeIcon size={12} className="shrink-0" />
      <span className="truncate">{compactUrlLabel(url)}</span>
    </button>
  );
};
