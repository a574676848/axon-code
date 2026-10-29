import React from 'react';
import { Z_MODAL } from './chatChrome';
import { acquireImageUrl, type ImageHandle } from './imageSources';

/*
 * 图片放大查看：点击遮罩或按 Esc 关闭。
 *
 * 两种入口共用这一份实现：
 *   - 附件卡与消息气泡给出路径，字节在这里按地址取（桌面窗口走 IPC 数据地址，
 *     调试桥走字节路由），因此这里只持有拿到的地址，不把 base64 读进内存；
 *   - 正文内的图片已经把地址取好了（同一份缓存），直接传 source，不再读第二遍。
 */

export const ImageLightbox: React.FC<{
  /** 已经取好的图片地址；与 path 二选一，同时给出时以 source 为准。 */
  source?: string;
  /** 图片来源路径；给出时由灯箱自己取字节。 */
  path?: string;
  alt?: string;
  onClose: () => void;
}> = ({ source, path, alt = '预览', onClose }) => {
  const [fetched, setFetched] = React.useState<string | null>(null);
  React.useEffect(() => {
    if (source !== undefined || path === undefined) return undefined;
    let disposed = false;
    let handle: ImageHandle | null = null;
    void acquireImageUrl(path).then(
      (acquired) => {
        if (disposed) {
          acquired.release();
          return;
        }
        handle = acquired;
        setFetched(acquired.url);
      },
      () => undefined,
    );
    return () => {
      disposed = true;
      setFetched(null);
      handle?.release();
    };
  }, [source, path]);
  const resolved = source ?? fetched;
  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [onClose]);
  return (
    <div
      role="dialog"
      aria-label="图片预览"
      onClick={onClose}
      className={`fixed inset-0 ${Z_MODAL} flex items-center justify-center bg-scrim p-8`}
    >
      {resolved !== null && (
        <img
          src={resolved}
          alt={alt}
          className="max-w-full max-h-full object-contain"
          onClick={(event) => event.stopPropagation()}
        />
      )}
    </div>
  );
};
