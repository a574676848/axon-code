import React from 'react';
import { Markdown } from './Markdown';

/**
 * 便签正文：读取便签与写入便签共用同一套形态。
 *
 * 便签按合同就是模型写的 Markdown 文档（标题、列表、锚点），因此按正文渲染，而不是
 * 塞进代码块：标题与列表要能扫读，正文里的路径与文件引用还要能点开。原文一字不改，
 * 只是换了呈现方式：渲染器不新增、不省略任何内容。
 *
 * 限高与代码块同一档（320px）并在内部滚动：一条便签可能上千行，展开它不该把整屏
 * 正文挤走；滚动区挂 `scroll-visible`，读的人一眼看得出下面还有。
 */
export const NotesBody: React.FC<{ text: string }> = ({ text }) => (
  <div className="my-1 max-h-80 overflow-y-auto scroll-visible pr-1">
    <Markdown source={text} />
  </div>
);
