/*
 * 正文行内标记的渲染：代码、粗体、斜体、删除线、链接、图片与文件路径。
 *
 * 从 Markdown 渲染器里拆出来，是因为这一层要同时处理三件互相牵扯的事：
 * 1) 图片优先于链接（`![a](p)` 会被链接正则命中，顺序错了图就变成链接）；
 * 2) 本地图片路径要按候选逐个试（模型写的路径常带引号、空格或全角标点）；
 * 3) 图片路径芯片要能悬浮出缩略图（这是正文里唯一按需触发一次读图的地方）。
 *
 * 这一层不认识块级结构，只吃一行文本与一个 key 前缀。
 */

import React from 'react';
import { filePathsIn } from './filePaths';
import { scanInline } from './markdownInlineScan';
import { ImagePathChip, MarkdownImage } from './MarkdownImage';
import {
  imagePathCandidates,
  imagePathScanCandidates,
  isImagePath,
  localImagePath,
  parseImageNode,
} from './localImage';

/** 正文渲染需要的外界能力；由 Markdown 组件按会话上下文组装。 */
export interface MarkdownArt {
  /** 在右侧栏打开一个文件；缺省时路径只是文本。 */
  openFile?: (path: string) => void;
  /** 当前工作区根目录，用于把相对图片路径落到工作区。 */
  workspaceRoot: string | null;
  /** 是否给图片路径芯片挂悬浮缩略图；文档预览页关掉，避免预览页自己套一层预览。 */
  hoverThumbnail?: boolean;
}

/** 只有绝对 HTTP(S) 地址才交给外部加载；其余协议一律按本地路径处理。 */
function remoteImageUrl(value: string): string | null {
  return /^https?:\/\//i.test(value) ? value : null;
}

/** 渲染一个 Markdown 图片节点。 */
function renderImage(
  destination: string,
  alt: string,
  key: string,
  art: MarkdownArt,
): React.ReactNode {
  const path = localImagePath(art.workspaceRoot, destination);
  if (path === null) {
    const remote = remoteImageUrl(destination.trim());
    if (remote === null) return alt;
    return <MarkdownImage key={key} destination={remote} candidates={[]} alt={alt} />;
  }
  return (
    <MarkdownImage
      key={key}
      destination={path}
      candidates={imagePathCandidates(path)}
      alt={alt}
    />
  );
}

/**
 * 把纯文本里的文件路径渲染成可点击的主题色标识，图片路径额外挂悬浮缩略图。
 *
 * 与工具行里点路径打开文件是同一套语义：正文提到的文件也应该能一键打开，
 * 而不是让用户自己去右侧栏翻。
 *
 * 两轮扫描：先按明确的路径形状切出路径芯片（这一轮认不出目录名带空格的路径），
 * 再在剩下的文字里补一遍图片路径。第二轮要避开第一轮已经消费的区间，
 * 否则同一段文字会被渲染两次。
 */
export function renderPlain(
  text: string,
  keyPrefix: string,
  art: MarkdownArt,
): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  const openFile = art.openFile;
  // 两轮扫描各认领自己的区间，第二轮跳过第一轮已经消费的那几段。
  const claimed: { from: number; to: number }[] = [];
  let cursor = 0;
  let index = 0;
  let segment = 0;
  for (const match of filePathsIn(text)) {
    // 每段文字各带一个序号：同一段正文里的多个片段不能共用同一批 React key。
    if (match.index > cursor) {
      nodes.push(
        ...renderImagePaths(
          text.slice(cursor, match.index),
          `${keyPrefix}:gap:${segment}`,
          art,
          claimed,
          cursor,
        ),
      );
      segment += 1;
    }
    const path = match.path;
    const key = `${keyPrefix}:path:${index}`;
    index += 1;
    /*
     * 图片路径的芯片要能悬浮出缩略图，因此不能沿用普通路径那一个 span。
     *
     * `renderImagePaths` 只补「明确路径形状」认不出来的那几段（目录名带空格的绝对路径），
     * 而像 `E:\dir\shot.png`、`docs/images/a.svg` 这类路径会先落在这一轮里；
     * 这里不换芯片，图片路径就永远拿不到缩略图。
     */
    nodes.push(
      isImagePath(path) && art.hoverThumbnail === true ? (
        <ImagePathChip key={key} path={path} onOpen={openFile} thumbnail>
          {path}
        </ImagePathChip>
      ) : (
        <span
          key={key}
          role={openFile === undefined ? undefined : 'link'}
          tabIndex={openFile === undefined ? undefined : 0}
          title={openFile === undefined ? path : `在右侧栏打开 ${path}`}
          onClick={
            openFile === undefined
              ? undefined
              : (event) => {
                  event.stopPropagation();
                  openFile(path);
                }
          }
          onKeyDown={openFile === undefined ? undefined : (event) => {
            if (event.key !== 'Enter' && event.key !== ' ') return;
            event.preventDefault();
            event.stopPropagation();
            openFile(path);
          }}
          className={`text-text-link font-medium${openFile === undefined ? '' : ' cursor-pointer hover:underline'}`}
        >
          {path}
        </span>
      ),
    );
    claimed.push({ from: match.index, to: match.index + path.length });
    cursor = match.index + path.length;
  }
  // 收尾段同样走图片路径识别：漏掉它会让正文最后的绝对图片路径失去整片与缩略图。
  if (cursor < text.length) {
    nodes.push(
      ...renderImagePaths(text.slice(cursor), `${keyPrefix}:tail`, art, claimed, cursor),
    );
  }
  return nodes;
}

/** 一个候选区段是否落在第一轮已经消费的区间里。 */
function claimedRange(
  claimed: { from: number; to: number }[],
  offset: number,
  length: number,
): boolean {
  return claimed.some((range) => offset < range.to && offset + length > range.from);
}

/**
 * 按顺序认领互不重叠的图片路径。
 *
 * 同一条路径会以多个候选出现（整片、被空格切碎的尾段、被句读粘住的写法），
 * 一次只认领一个：认领后把指针推到这段之后，与它重叠的候选全部跳过。
 * 否则同一段文字会被渲染成两个相邻的芯片。
 */
function claimCandidates(
  candidates: { path: string; index: number }[],
  claimed: { from: number; to: number }[],
  offset: number,
): { path: string; index: number }[] {
  const out: { path: string; index: number }[] = [];
  let cursor = 0;
  for (const match of candidates) {
    if (match.index < cursor) continue;
    if (claimedRange(claimed, offset + match.index, match.path.length)) continue;
    out.push(match);
    cursor = match.index + match.path.length;
  }
  return out;
}

/**
 * 裸文本里的图片路径。
 *
 * `filePathsIn` 只认「一段到底」的路径，遇到目录名里带空格的绝对路径会在空格处截断，
 * 因此这里独立扫一遍：以图片后缀为准取整片，再给出「与后续词拼接」的候选。
 * 命中区段之外的文字原样落盘，绝不改动用户看到的正文。
 */
function renderImagePaths(
  text: string,
  keyPrefix: string,
  art: MarkdownArt,
  claimed: { from: number; to: number }[] = [],
  offset = 0,
): React.ReactNode[] {
  if (art.openFile === undefined) return [text];
  const matches = claimCandidates(imagePathScanCandidates(text), claimed, offset);
  if (matches.length === 0) return [text];
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  matches.forEach((match, position) => {
    if (match.index > cursor) nodes.push(text.slice(cursor, match.index));
    nodes.push(
      <ImagePathChip
        key={`${keyPrefix}:img:${position}`}
        path={match.path}
        onOpen={art.openFile}
        thumbnail={art.hoverThumbnail === true}
      >
        {match.path}
      </ImagePathChip>,
    );
    cursor = match.index + match.path.length;
  });
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

/** 行内标记：图片、代码、粗体、斜体、删除线与链接。 */
export function renderInline(
  text: string,
  keyPrefix: string,
  art: MarkdownArt,
): React.ReactNode[] {
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  // 扫描交给专门的纯函数：它产出记号的位置与内部文本，这里只负责拼元素。
  // 强调类记号会递归调用本函数，因此扫描必须与渲染分开（见 markdownInlineScan 的说明）。
  const tokens = scanInline(text);
  let index = 0;
  for (const token of tokens) {
    if (token.index > cursor) {
      nodes.push(...renderPlain(text.slice(cursor, token.index), keyPrefix, art));
    }
    const key = `${keyPrefix}:${index}`;
    index += 1;
    const raw = token.text;
    const image = parseImageNode(raw);
    const link = image === null ? /^\[([^\]]+)\]\(([^)]+)\)$/.exec(raw) : null;
    if (image !== null) {
      nodes.push(renderImage(image.destination, image.alt, key, art));
    } else if (token.kind === 'code') {
      /*
       * 行内代码：12px 等宽、无框柔和胶囊底配深钢蓝墨色（--color-code-ink），
       * 消除密集描边造成的碎框眩光感，温润嵌入长文行间。
       */
      nodes.push(
        <code
          key={key}
          className="mx-0.5 rounded-[5px] bg-code-inline px-1.5 py-0.5 align-baseline font-mono text-code text-code-ink"
        >
          {renderPlain(token.inner, key, art)}
        </code>,
      );
    } else if (token.kind === 'strong') {
      nodes.push(
        <strong key={key} className="font-semibold text-text-primary">
          {renderInline(token.inner, key, art)}
        </strong>,
      );
    } else if (token.kind === 'strike') {
      nodes.push(
        <span key={key} className="line-through text-text-tertiary">
          {renderInline(token.inner, key, art)}
        </span>,
      );
    } else if (token.kind === 'em') {
      nodes.push(<em key={key}>{renderInline(token.inner, key, art)}</em>);
    } else if (link !== null) {
      nodes.push(
        <a
          key={key}
          href={link[2]}
          target="_blank"
          rel="noreferrer"
          className="text-text-link font-medium underline underline-offset-2"
        >
          {link[1]}
        </a>,
      );
    } else {
      nodes.push(...renderPlain(raw, key, art));
    }
    cursor = token.index + raw.length;
  }
  // 尾段是普通文本，直接落盘，不能再回调自身。
  nodes.push(...renderPlain(text.slice(cursor), `${keyPrefix}:tail`, art));
  return nodes;
}
