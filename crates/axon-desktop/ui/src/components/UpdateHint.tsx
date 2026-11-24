import React, { useCallback, useState } from 'react';
import { DownloadIcon } from '../icons';
import * as ipc from '../ipc';
import { useDesktopUpdate } from '../desktopUpdate';
import { updateHint } from '../updateNotice';
import { Button } from './ui/controls';

/*
 * 侧栏底部的「新版本」入口。
 *
 * 它不发起检测：结论来自桌面端共用的那一份更新事实（启动自动检测一次，设置 › 关于
 * 读的也是它），因此两处不会各查一遍，也不会互相打脸。
 *
 * 打开失败就地转成重试态：侧栏这一行只有一枚按钮的位置，写不下一段归因，因此原因挂在
 * 原生提示上，文案与颜色只说明「这一次没打开」，再点即重试。
 */
export const UpdateHint: React.FC = () => {
  const { result } = useDesktopUpdate();
  const [openFailure, setOpenFailure] = useState<string | null>(null);
  const hint = updateHint(result, openFailure);

  const open = useCallback(() => {
    void ipc
      .openExternalUrl(hint.href)
      .then(() => setOpenFailure(null))
      .catch((failure: unknown) =>
        setOpenFailure(failure instanceof Error ? failure.message : '无法打开下载页'),
      );
  }, [hint.href]);

  if (!hint.visible) return null;
  return (
    <Button
      variant={hint.tone === 'danger' ? 'danger' : 'link'}
      size="md"
      aria-label={hint.detail}
      title={hint.detail}
      onClick={open}
      className="shrink-0"
    >
      <DownloadIcon size={13} />
      {hint.label}
    </Button>
  );
};
