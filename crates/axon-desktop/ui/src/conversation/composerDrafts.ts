/*
 * 输入坞草稿的分会话暂存。
 *
 * 还没发出去的正文、附件与链接占位属于某一条会话，不属于输入坞这个容器：换一条
 * 会话时既不该把上一条的草稿带过去（那会发错会话），也不该把它丢掉（切出去看一条
 * 消息再回来，写了一半的话不能没有）。因此草稿按会话标识暂存，切回来原样接上。
 *
 * 三条轨道各存一格：正文归编辑控制器，附件归附件控制器，链接占位归链接控制器。
 * 每个控制器只换自己那一格，切换时互不覆写，也不必知道另外两条轨道的形状。
 *
 * 暂存只在内存里：它是「这次打开应用期间写到哪儿了」，不是跨启动的草稿箱，
 * 因此不落盘，也不进状态文件。
 */

import type { AttachmentRef } from './transcript';
import type { LinkToken } from './linkTokens';

/** 还没有附件时的空值：稳定引用，切换会话不制造新数组。 */
const NO_ATTACHMENTS: AttachmentRef[] = [];
/** 还没有链接占位时的空值。 */
const NO_LINKS: LinkToken[] = [];

/**
 * 草稿桶。
 *
 * 首屏（还没有会话）也算一格：那里同样可以写一半再点进一条会话，切回首屏时
 * 那段正文要还在。空串与 null 因此共用同一个桶，避免两处各记一份。
 */
function bucketOf(key: string | null): string {
  return key ?? '';
}

const texts = new Map<string, string>();
const attachments = new Map<string, AttachmentRef[]>();
const links = new Map<string, LinkToken[]>();

/** 记下某条会话的正文；空正文即视为没有草稿。 */
export function rememberComposerText(key: string | null, text: string): void {
  const bucket = bucketOf(key);
  if (text.length === 0) {
    texts.delete(bucket);
    return;
  }
  texts.set(bucket, text);
}

/** 取回某条会话的正文；没有草稿时为空串。 */
export function composerTextFor(key: string | null): string {
  return texts.get(bucketOf(key)) ?? '';
}

/** 记下某条会话的附件；空清单一并删掉，不给内存留空壳。 */
export function rememberComposerAttachments(
  key: string | null,
  list: readonly AttachmentRef[],
): void {
  const bucket = bucketOf(key);
  if (list.length === 0) {
    attachments.delete(bucket);
    return;
  }
  attachments.set(bucket, [...list]);
}

/** 取回某条会话的附件；返回副本，调用方之后怎么改都不会改到暂存里那一份。 */
export function composerAttachmentsFor(key: string | null): AttachmentRef[] {
  const stored = attachments.get(bucketOf(key));
  return stored === undefined ? NO_ATTACHMENTS : [...stored];
}

/** 记下某条会话的链接占位；占位与正文必须一起跟随，否则提交会把占位符当成原文发出去。 */
export function rememberComposerLinks(key: string | null, list: readonly LinkToken[]): void {
  const bucket = bucketOf(key);
  if (list.length === 0) {
    links.delete(bucket);
    return;
  }
  links.set(bucket, [...list]);
}

/** 取回某条会话的链接占位；同样给副本。 */
export function composerLinksFor(key: string | null): LinkToken[] {
  const stored = links.get(bucketOf(key));
  return stored === undefined ? NO_LINKS : [...stored];
}

/** 提交之后整条会话的草稿作废：正文、附件与链接占位一起清掉。 */
export function forgetComposerDraft(key: string | null): void {
  const bucket = bucketOf(key);
  texts.delete(bucket);
  attachments.delete(bucket);
  links.delete(bucket);
}

/** 清空全部暂存；只给测试与内存复位用。 */
export function clearComposerDrafts(): void {
  texts.clear();
  attachments.clear();
  links.clear();
}

/**
 * 暂存里是否还有任何会话的草稿。
 *
 * 切档会重启应用，输入坞里没发出去的内容随之消失；确认弹窗的判据因此不能只看
 * 「当前这条会话的正文空不空」：用户在 A 写了一半，切到 B 再切档，A 的草稿一样会丢。
 */
export function hasStoredComposerDraft(): boolean {
  return texts.size > 0 || attachments.size > 0 || links.size > 0;
}
