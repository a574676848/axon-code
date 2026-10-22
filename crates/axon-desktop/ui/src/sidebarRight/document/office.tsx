import React from 'react';
import { FileIcon } from '../../icons';
import { EmptyState } from '../../components/ui/surfaces';
import { OpenInAppAction } from '../OpenInAppAction';
import type { ViewerProps } from './contract';

/*
 * Office 看法：如实说明这台机器上没有可用的转换服务。
 *
 * `.docx` / `.pptx` 要显示出来必须先转成 PDF 或 HTML，而那需要宿主上装一套转换服务。
 * 本工作台没有内置它，因此这里给出的是**既有工作台产品在服务未启用时的同一句话**，
 * 并把「用系统里的应用打开」摆在同一屏：看不了就说看不了，同时给一条真的走得通的路，
 * 而不是画一个转不出来的进度条。
 *
 * 空态与「暂时无法预览」共用同一份组件与同一套几何（图标 24、标题 14、说明 12）。
 */
export const OfficeBody: React.FC<ViewerProps> = ({ root, path }) => (
  <div className="flex-1 min-h-0 flex items-center justify-center">
    <EmptyState
      icon={<FileIcon size={24} />}
      title="Office 预览不可用"
      description="转换服务未随本工作台提供，请用系统里的应用打开。"
      action={<OpenInAppAction root={root} path={path} prominent />}
    />
  </div>
);
