import React from 'react';
import * as ipc from '../../ipc';
import { ImageLightbox } from './ImageLightbox';
import { imagePathCandidates } from './localImage';

/*
 * 正文里的一张本地图片。
 *
 * 与附件卡同一套取值通道（桌面窗口走 IPC 数据地址，浏览器调试桥走字节路由），
 * 因此「正文里看到的图」与「消息气泡上那张附件图」是同一份字节、同一套行为：
 * 读不出来时给一张破图占位并附上原路径，点开进同一盏灯箱。
 *
 * 待读路径按候选顺序逐个试：模型写的路径可能被引号包住、粘着中文标点或带空格，
 * 第一个能读出来的就是它。
 */

/**
 * 同一个路径只读一次：正文里同一张图被引用多次时不再重复走一次 IPC。
 *
 * 表是有界的。桌面窗口这条通道回的是 data URL，一张 1.5 MB 的截图在 JS 里就是约
 * 2 MB 的字符串；一张图读出来之后组件自己还持有地址，这里只保留最近这些条，
 * 更早的被移出后如需再显示会重新读一次，代价是一次 IPC 而不是常驻内存。
 */
const SOURCE_CACHE_LIMIT = 24;

const sources = new Map<string, Promise<string>>();

/** 取一个本地图片路径的渲染地址；失败时从缓存里撤掉，允许后续重试。 */
export function imageSource(path: string): Promise<string> {
  const cached = sources.get(path);
  if (cached !== undefined) return cached;
  const pending = ipc.attachmentSource(path).catch((error: unknown) => {
    sources.delete(path);
    throw error;
  });
  sources.set(path, pending);
  if (sources.size > SOURCE_CACHE_LIMIT) {
    const oldest = sources.keys().next();
    if (!oldest.done) sources.delete(oldest.value);
  }
  return pending;
}

/** 按候选顺序取第一张读得出来的图。 */
export function firstReadableImage(candidates: string[]): Promise<string> {
  let failure: unknown = new Error('没有可读取的图片路径');
  const attempt = (index: number): Promise<string> => {
    if (index >= candidates.length) return Promise.reject(failure);
    return imageSource(candidates[index]).catch((error: unknown) => {
      failure = error;
      return attempt(index + 1);
    });
  };
  return attempt(0);
}

/** 图片自身的展示尺寸上限；再大也只是把正文挤成一张图。 */
const MAX_HEIGHT = 420;

/**
 * 破图字形：一只画框加一道斜杠。
 *
 * 只在这里用得到，因此不放进共用的文件类型图标表（那张表按扩展名分类，服务的是
 * 文件芯片与目录树）。主形取 currentColor，跟随占位卡的文字颜色。
 */
const BrokenImageIcon: React.FC<{ size?: number }> = ({ size = 22 }) => (
  <svg
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth={1.6}
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
    <path d="M4 5.5h16v13H4z" />
    <path d="M6.5 15.5l3.4-3.9 2.4 2.6 2.1-1.9 3.1 3.4" />
    <circle cx="9.2" cy="9.4" r="1.3" />
    <path d="M3.4 20.6L20.6 3.4" />
  </svg>
);

/** 提前多远开始读图：与视口差一屏时就读，滚动到位不会再等一次 IPC。 */
const READ_AHEAD_MARGIN = '400px';

export const MarkdownImage: React.FC<{
  /** Markdown 里写的目标：本地路径或 HTTP(S) 地址。 */
  destination: string;
  /** 本地路径的候选写法；远程图片为空数组。 */
  candidates: string[];
  alt: string;
}> = ({ destination, candidates, alt }) => {
  const anchor = React.useRef<HTMLSpanElement>(null);
  const [visible, setVisible] = React.useState(false);
  const [source, setSource] = React.useState<string | null>(null);
  const [failed, setFailed] = React.useState(false);
  const [open, setOpen] = React.useState(false);

  /*
   * 进了视野才读字节。
   *
   * 长会话里图片可能几十张，挂载即全读会把每张图的 data URL 都常驻下来；
   * 这里与 `<img loading="lazy">` 同一节奏：差一屏时开始读，读不出来再退回文字。
   */
  React.useEffect(() => {
    const node = anchor.current;
    if (node === null) return undefined;
    if (typeof IntersectionObserver !== 'function') {
      setVisible(true);
      return undefined;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: READ_AHEAD_MARGIN },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  /*
   * 候选数组按内容定身份。
   *
   * 调用方在渲染里现算这个数组，每次渲染都是新引用；若把它直接当依赖，读图 effect
   * 会在「读图 到 setState 到 重渲染 到 依赖变化」之间无限重跑，每一轮都发一次 IPC 并
   * 重新分配一份图片 data URL（实测 50MB/s 涨到 4GB，渲染进程被系统直接回收）。
   * 因此依赖取它的内容摘要，数组本身经 ref 取当前值。
   */
  const candidateKey = candidates.join('\u0000');
  const candidatesRef = React.useRef(candidates);
  candidatesRef.current = candidates;

  React.useEffect(() => {
    if (!visible) return undefined;
    let cancelled = false;
    setSource(null);
    setFailed(false);
    const current = candidatesRef.current;
    // 远程图片交给 webview 自己取：这里只负责本地图片的字节通道。
    const pending =
      current.length === 0 ? Promise.resolve(destination) : firstReadableImage(current);
    void pending.then(
      (value) => {
        if (!cancelled) setSource(value);
      },
      () => {
        if (!cancelled) setFailed(true);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [candidateKey, destination, visible]);

  if (failed) {
    /*
     * 读不出来时给一张明确的破图占位，而不是把说明文字混进正文。
     *
     * 正文里本来写的就是一张图，位置突然变成一行小字，读起来像模型少输出了一段；
     * 破图占位把「这里本该有图、但它读不到」这件事说清楚，路径仍然原样写在下面，
     * 复制出去就能定位。占位不接收点击：它背后没有可打开的文件。
     */
    return (
      <span ref={anchor} className="my-3 inline-flex max-w-full flex-col items-start gap-1">
        <span
          role="img"
          aria-label={alt.length > 0 ? `图片无法显示：${alt}` : '图片无法显示'}
          className="flex h-[72px] w-[180px] items-center justify-center gap-1.5 rounded-md bg-surface-sunken hairline border-border-hairline text-text-tertiary"
        >
          <BrokenImageIcon size={16} />
          <span className="text-meta">图片无法显示</span>
        </span>
        <span className="break-all font-mono text-meta text-text-tertiary">
          {alt.length > 0 ? `${alt}：${destination}` : destination}
        </span>
      </span>
    );
  }

  if (source === null) {
    return (
      <span
        ref={anchor}
        role="status"
        className="my-3 flex h-[72px] w-[180px] items-center justify-center rounded-md bg-surface-sunken text-meta text-text-tertiary"
      >
        图片加载中
      </span>
    );
  }

  return (
    <span ref={anchor} className="contents">
      <button
        type="button"
        title="放大查看"
        aria-label={alt.length > 0 ? `放大查看 ${alt}` : '放大查看图片'}
        onClick={() => setOpen(true)}
        className="my-3 block max-w-full cursor-zoom-in rounded-md hairline border-border-hairline p-0"
      >
        <img
          src={source}
          alt={alt}
          loading="lazy"
          decoding="async"
          referrerPolicy="no-referrer"
          style={{ maxHeight: MAX_HEIGHT }}
          className="max-w-full rounded-md object-contain"
        />
      </button>
      {open && (
        <ImageLightbox
          source={source}
          alt={alt}
          onClose={() => {
            setOpen(false);
          }}
        />
      )}
    </span>
  );
};

/**
 * 正文里的图片路径芯片：点开在右侧栏查看，悬浮给缩略图。
 *
 * 缩略图只在允许挂预览、指针真正停到芯片上、并且这条路径确实读得出来时才出现；
 * 读不出来就什么都不显示，芯片仍然是一个普通的可点路径。
 */
export const ImagePathChip: React.FC<{
  path: string;
  onOpen?: (path: string) => void;
  /** 是否挂悬浮缩略图；文档预览页关掉，避免预览页里再套一层预览。 */
  thumbnail?: boolean;
  children: React.ReactNode;
}> = ({ path, onOpen, thumbnail = false, children }) => {
  const [preview, setPreview] = React.useState<string | null>(null);
  const [hover, setHover] = React.useState(false);

  React.useEffect(() => {
    if (!thumbnail || !hover || preview !== null) return undefined;
    let cancelled = false;
    void firstReadableImage(imagePathCandidates(path)).then(
      (value) => {
        if (!cancelled) setPreview(value);
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [hover, path, preview, thumbnail]);

  return (
    <span
      className="relative inline-flex max-w-full flex-col items-start"
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
    >
      <span
        role={onOpen === undefined ? undefined : 'link'}
        tabIndex={onOpen === undefined ? undefined : 0}
        title={path}
        onClick={
          onOpen === undefined
            ? undefined
            : (event) => {
                event.stopPropagation();
                onOpen(path);
              }
        }
        onKeyDown={
          onOpen === undefined
            ? undefined
            : (event) => {
                if (event.key !== 'Enter' && event.key !== ' ') return;
                event.preventDefault();
                event.stopPropagation();
                onOpen(path);
              }
        }
        className={`text-text-link font-medium${onOpen === undefined ? '' : ' cursor-pointer hover:underline'}`}
      >
        {children}
      </span>
      {hover && preview !== null && (
        <span className="absolute left-0 top-full z-40 mt-1 block w-[268px] rounded-md bg-surface p-1.5 shadow-panel hairline border-border-muted">
          <img
            src={preview}
            alt=""
            aria-hidden="true"
            className="block max-h-[200px] w-full rounded-sm object-contain"
          />
        </span>
      )}
    </span>
  );
};
