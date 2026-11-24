import React from 'react';
import { dataUrlOf, type ViewerProps } from './contract';
import { BodyShell, ZoomControls } from './parts';

/*
 * 图片与 HTML 两种看法。
 *
 * 两者都要**原始字节**：按文本读会把非 UTF-8 的部分替换成占位符，画出来的图就是坏的。
 * 图片走 data 地址（后端已经给出 base64），HTML 走 Blob 地址：前者只被读一次，
 * 后者需要一个真实 URL 才能让 iframe 按照文档的方式解析。
 *
 * 两种看法共用同一份正文骨架：顶部 36px 一行，下面各自的内容面。
 */

/** 缩放比例：初始按 100%，用户改过之后就一直用他改的那一档。 */
const INITIAL_ZOOM = 1;

/** 图片：居中摆放，可缩放，图片本体之外留出与文件树一致的边距。 */
export const ImageBody: React.FC<ViewerProps> = ({ path, content }) => {
  const [zoom, setZoom] = React.useState(INITIAL_ZOOM);
  const frame = React.useRef<HTMLDivElement>(null);
  const [natural, setNatural] = React.useState<{ width: number; height: number } | null>(null);

  /** 适应宽度：按可用宽度与图片原始宽度算出一个比例。 */
  const fit = React.useCallback(() => {
    // 量的是内容框：容器两侧各有 16px 内边距，不扣掉会让「适应宽度」正好横向溢出一屏。
    const available = (frame.current?.clientWidth ?? 0) - 32;
    if (available <= 0 || natural === null || natural.width <= 0) return;
    setZoom(Math.max(0.1, Math.min(8, available / natural.width)));
  }, [natural]);

  if (content.kind !== 'bytes') return null;
  const name = path.split(/[\\/]/).pop() ?? path;

  return (
    <BodyShell
      label={content.mime}
      actions={<ZoomControls zoom={zoom} onZoom={setZoom} onFit={fit} />}
      bodyClassName="bg-surface-sunken"
    >
      <div ref={frame} data-image-preview className="p-4 flex items-start justify-center">
        <img
          src={dataUrlOf(content)}
          alt={name}
          draggable={false}
          onLoad={(event) =>
            setNatural({
              width: event.currentTarget.naturalWidth,
              height: event.currentTarget.naturalHeight,
            })
          }
          style={{ width: natural === null ? undefined : natural.width * zoom }}
          className="max-w-none shrink-0 rounded-sm bg-surface shadow-menu"
        />
      </div>
      {content.truncated && (
        <div className="px-4 py-1.5 text-meta text-text-tertiary">
          文件超过 12 MiB，已截断，图片可能不完整。
        </div>
      )}
    </BodyShell>
  );
};

/** HTML：交给浏览器按文档解析，因此脚本、样式与排版都是它本来的样子。 */
export const HtmlBody: React.FC<ViewerProps> = ({ content }) => {
  const [url, setUrl] = React.useState<string | null>(null);

  React.useEffect(() => {
    if (content.kind !== 'bytes') return undefined;
    // Blob 地址按内容身份建一次，卸载时立刻回收：不留一个指向已释放内存的地址。
    const binary = window.atob(content.base64);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    const objectUrl = URL.createObjectURL(new Blob([bytes], { type: content.mime }));
    setUrl(objectUrl);
    return () => {
      URL.revokeObjectURL(objectUrl);
      setUrl(null);
    };
  }, [content]);

  return (
    <BodyShell label="HTML 文档预览" scroll={false} bodyClassName="bg-surface">
      {url !== null && (
        /*
         * 沙箱只放开脚本：文档自己的 JS 照常跑，但它是**独立来源**，
         * 拿不到工作台的存储与身份，也打不开顶层窗口。
         * 这也意味着文档里的相对资源（同目录的图片、样式）不会加载：
         * 预览的是这份文件本身，不是它所在的目录。
         */
        <iframe
          src={url}
          title="HTML 文档预览"
          sandbox="allow-scripts"
          className="w-full h-full border-0 bg-surface"
        />
      )}
    </BodyShell>
  );
};
