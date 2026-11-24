import React from 'react';
import * as pdfjs from 'pdfjs-dist';
import type { PDFDocumentProxy } from 'pdfjs-dist';
import workerUrl from 'pdfjs-dist/build/pdf.worker.min.mjs?url';
import { bytesOf, type ViewerProps } from './contract';
import { BodyShell, ZoomControls } from './parts';

/*
 * PDF 看法：把每一页画进自己的 canvas。
 *
 * 与既有工作台产品同一条路径：客户端解析（pdfjs），不依赖宿主装了什么阅读器。
 * Worker 由打包器单独出包，因此解析不占界面线程；一份大文档滚动时页面照常响应。
 *
 * 页面按需绘制：先按每页的原始尺寸摆好占位（滚动条一开始就是对的），
 * 滚到附近才真的画，因此打开一份 300 页的文档不会一次画 300 张画布。
 */

// Worker 必须先于任何 getDocument 指定：不指定时 pdfjs 会退回主线程解析，
// 大文档一打开界面就整块卡住。
pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

/** 缩放档位起点。 */
const INITIAL_ZOOM = 1;

/** 一次最多画多少页：再多的部分不提供渲染，避免把面板拖垮。 */
const MAX_PAGES = 500;

/** 提前多少像素开始绘制：略大于一屏，滚动时不至于看见空白。进入与离开共用同一档。 */
const PRELOAD_MARGIN = '600px';

/**
 * 把画布还回去。
 *
 * 画布位图不在 JS 堆里：只把状态翻成「未绘制」不会释放任何内存，必须把宽度与高度
 * 归零，浏览器才会立刻丢掉这一页的位图后端。实测一页 A4 在 2x 屏上约 8 MiB，
 * 一份三百页的文档一路滚过去，只画不还就是 1.2 GiB，足够把渲染进程吃到被回收。
 */
function releaseCanvas(canvas: HTMLCanvasElement | null): void {
  if (canvas === null) return;
  canvas.width = 0;
  canvas.height = 0;
}

/** 把引擎抛出的东西压成一行可读说明；没有说明时给一句兜底。 */
function describeFailure(failure: unknown): string {
  const message =
    failure instanceof Error ? failure.message : typeof failure === 'string' ? failure : '';
  const trimmed = message.trim();
  return trimmed.length === 0 ? '文件可能已损坏或不是 PDF' : trimmed.slice(0, 160);
}

/** 一页：占位尺寸先摆好，进入视野才画。 */
const PdfPage: React.FC<{
  doc: PDFDocumentProxy;
  /** 页码，从 1 开始。 */
  number: number;
  /** 原始尺寸（缩放 1 时）。 */
  size: { width: number; height: number };
  scale: number;
}> = ({ doc, number, size, scale }) => {
  const host = React.useRef<HTMLDivElement>(null);
  const canvas = React.useRef<HTMLCanvasElement>(null);
  const [painted, setPainted] = React.useState(false);

  React.useEffect(() => {
    const element = host.current;
    if (element === null) return undefined;
    if (typeof IntersectionObserver === 'undefined') {
      // 没有观察器时退化成「进页面就画」：宁可多占一页，也不要永远停在一块空白。
      setPainted(true);
      return undefined;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        const entry = entries[entries.length - 1];
        if (entry === undefined) return;
        /*
         * 进出都响应：看着的页才配占一块位图。
         *
         * 只按「进入」置位、从不复位，等于把读过一遍的每一页都钉在内存里；
         * 面板开着滚一遍长文档就会一路堆到看门狗的高水位。
         */
        setPainted(entry.isIntersecting);
        if (!entry.isIntersecting) releaseCanvas(canvas.current);
      },
      { rootMargin: PRELOAD_MARGIN },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  React.useEffect(() => {
    if (!painted) return undefined;
    let cancelled = false;
    let task: { cancel: () => void } | null = null;
    void doc.getPage(number).then((page) => {
      if (cancelled) return;
      const target = canvas.current;
      if (target === null) return;
      // 按设备像素比绘制，高 DPI 屏幕上文字才不糊。
      const ratio = window.devicePixelRatio || 1;
      const viewport = page.getViewport({ scale: scale * ratio });
      target.width = Math.floor(viewport.width);
      target.height = Math.floor(viewport.height);
      const context = target.getContext('2d');
      if (context === null) return;
      const render = page.render({ canvas: target, canvasContext: context, viewport });
      task = render;
      void render.promise.catch(() => undefined);
    });
    return () => {
      cancelled = true;
      task?.cancel();
    };
  }, [doc, number, scale, painted]);

  return (
    <div
      ref={host}
      data-pdf-page={number}
      className="shrink-0 bg-surface shadow-menu rounded-sm overflow-hidden"
      style={{ width: Math.round(size.width * scale), height: Math.round(size.height * scale) }}
    >
      <canvas ref={canvas} className="block w-full h-full" />
    </div>
  );
};

export const PdfBody: React.FC<ViewerProps> = ({ content }) => {
  const [doc, setDoc] = React.useState<PDFDocumentProxy | null>(null);
  const [sizes, setSizes] = React.useState<Array<{ width: number; height: number }>>([]);
  const [scale, setScale] = React.useState(INITIAL_ZOOM);
  const [error, setError] = React.useState<string | null>(null);
  const frame = React.useRef<HTMLDivElement>(null);

  const base64 = content.kind === 'bytes' ? content.base64 : '';
  const mime = content.kind === 'bytes' ? content.mime : '';

  React.useEffect(() => {
    if (base64.length === 0) return undefined;
    let cancelled = false;
    setDoc(null);
    setSizes([]);
    setError(null);
    const task = pdfjs.getDocument({ data: bytesOf({ kind: 'bytes', mime, base64, truncated: false }) });
    void task.promise
      .then(async (loaded) => {
        if (cancelled) {
          // 卸载时的清理已经销毁了加载任务，这里什么都不用做。
          return;
        }
        setDoc(loaded);
        // 先把每页的原始尺寸取齐：占位尺寸定下来，页面陆续画出来时滚动条不会跳。
        const collected: Array<{ width: number; height: number }> = [];
        const total = Math.min(loaded.numPages, MAX_PAGES);
        for (let number = 1; number <= total; number += 1) {
          const page = await loaded.getPage(number);
          const viewport = page.getViewport({ scale: 1 });
          collected.push({ width: viewport.width, height: viewport.height });
          if (cancelled) return;
        }
        setSizes(collected);
      })
      .catch((failure: unknown) => {
        if (cancelled) return;
        // 加密文档与损坏文档要分开说：前者用户有办法，后者没有。
        const name = (failure as { name?: string } | null)?.name;
        setError(
          name === 'PasswordException'
            ? '此 PDF 需要密码，暂不支持预览'
            // 其余失败把引擎自己的说明带出来：它通常已经说清了是缺字体、缺对象还是流被截断，
            // 比一句「无法显示」有用得多（既有工作台产品的同一条文案也是这么写的）。
            : `无法显示 PDF：${describeFailure(failure)}`,
        );
      });
    return () => {
      cancelled = true;
      void task.destroy();
    };
  }, [base64, mime]);

  /** 适应宽度：按可用宽度与第一页的宽度算出比例。 */
  const fit = React.useCallback(() => {
    const available = frame.current?.clientWidth ?? 0;
    const first = sizes[0];
    if (available <= 0 || first === undefined || first.width <= 0) return;
    // 留出页面两侧各 16px 的边距，否则「适应宽度」会把页面顶到贴边。
    setScale(Math.max(0.1, Math.min(8, (available - 32) / first.width)));
  }, [sizes]);

  return (
    <BodyShell
      label={doc === null ? 'PDF' : `${doc.numPages} 页`}
      actions={<ZoomControls zoom={scale} onZoom={setScale} onFit={fit} />}
      bodyClassName="bg-surface-sunken"
    >
      <div
        ref={frame}
        data-pdf-preview
        className="min-h-full px-4 py-4 flex flex-col items-center gap-4"
      >
        {error !== null ? (
          <p role="alert" className="px-2 text-center text-row text-danger">
            {error}
          </p>
        ) : doc === null ? (
          <p className="px-2 text-row text-text-secondary">文档渲染中...</p>
        ) : (
          <>
            {sizes.map((size, index) => (
              <PdfPage key={index} doc={doc} number={index + 1} size={size} scale={scale} />
            ))}
            {doc.numPages > MAX_PAGES && (
              <p className="px-2 text-meta text-text-tertiary">
                文档共 {doc.numPages} 页，只渲染前 {MAX_PAGES} 页。
              </p>
            )}
          </>
        )}
      </div>
      {content.kind === 'bytes' && content.truncated && (
        <div className="px-4 py-1.5 text-meta text-text-tertiary">
          文件超过 12 MiB，已截断，文档可能不完整。
        </div>
      )}
    </BodyShell>
  );
};
