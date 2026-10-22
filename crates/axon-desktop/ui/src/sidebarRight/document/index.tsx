import React from 'react';
import { LoadingState } from '../../components/ui/surfaces';
import { CodeBody } from './code';
import { ImageBody, HtmlBody } from './media';
import { OfficeBody } from './office';
import { TableBody } from './table';
import { MarkdownBody, PlainBody } from './text';
import type { ViewerId } from './viewers';
import type { ViewerProps } from './contract';

/*
 * 渲染器表：身份到实现的唯一映射。
 *
 * PDF 用动态引入：pdfjs 加它的 Worker 是这一层里最重的一块，而大多数会话里
 * 一份 PDF 都不会打开，因此它不该出现在首屏包里。
 */
const PdfBody = React.lazy(() =>
  import('./pdf').then((module) => ({ default: module.PdfBody })),
);

/** 每种看法的实现。 */
export const RENDERERS: Record<ViewerId, React.FC<ViewerProps>> = {
  plain: PlainBody,
  markdown: MarkdownBody,
  html: HtmlBody,
  image: ImageBody,
  pdf: PdfBody,
  office: OfficeBody,
  table: TableBody,
  code: CodeBody,
};

/** 渲染一份内容；动态引入还没到位时给出同一套加载态。 */
export const ViewerOutlet: React.FC<{ viewer: ViewerId } & ViewerProps> = ({
  viewer,
  ...props
}) => {
  const Renderer = RENDERERS[viewer];
  return (
    <React.Suspense
      fallback={
        <div className="flex-1 min-h-0 flex items-center justify-center">
          <LoadingState label="文档渲染中" />
        </div>
      }
    >
      <Renderer {...props} />
    </React.Suspense>
  );
};
