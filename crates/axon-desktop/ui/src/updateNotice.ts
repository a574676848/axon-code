import type { UpdateCheck } from './types';

/*
 * 一次版本检测的结论往哪里说、怎么说。
 *
 * 同一次检测有两个落点：设置 › 关于的首屏结论句，与侧栏底部的「新版本」入口。
 * 因此「什么时候算有新版本」「失败怎么讲」「点开哪个地址」都只在这里定义一次，
 * 界面不各自判断版本高低，也不各自拼一句文案。
 */

/** 关于页首屏的检测结论：只有这三种口径，外加检测中的过渡态。 */
export function updateStatusText(result: UpdateCheck | null, checking: boolean): string {
  if (checking) return '正在检测…';
  if (result === null) return '尚未检测';
  if (result.error.length > 0) return '暂时无法检测新版本';
  if (result.update_available) return `发现新版本 ${result.latest}`;
  return '已是最新版本';
}

/** 侧栏底部入口的呈现。 */
export interface UpdateHintView {
  /** 是否出现：确认有新版本，且真有可打开的地址。 */
  visible: boolean;
  /** 按钮文案。 */
  label: string;
  /** 原生提示，同时是无障碍名称。 */
  detail: string;
  /** 点击要打开的外部地址。 */
  href: string;
  /** 危险色只用于「这一次没打开成功」的重试态。 */
  tone: 'link' | 'danger';
}

/** 不出现时的值：整块不渲染，也不占位。 */
const HIDDEN: UpdateHintView = { visible: false, label: '', detail: '', href: '', tone: 'link' };

/**
 * 下载入口指向哪里：优先首个安装包资产，没有资产时退回该版本的发布页。
 *
 * 关于页的「前往下载」与侧栏入口共用这一处判断，因此两处永远指向同一个地址。
 * 没有更新、或两个地址都为空时给出空串，调用方据此不渲染入口。
 */
export function downloadHref(result: UpdateCheck | null): string {
  if (result === null || result.error.length > 0 || !result.update_available) return '';
  return result.download_url.length > 0 ? result.download_url : result.release_url;
}

/**
 * 由检测结论与「这一次打开是否失败」推出侧栏入口。
 *
 * 检测失败不进侧栏：那一行只有一个按钮的位置，讲不清一句归因；失败的详情归关于页。
 * 没有可打开的地址时同样不出现，避免摆一个点了没反应的入口。
 */
export function updateHint(result: UpdateCheck | null, openFailure: string | null): UpdateHintView {
  const href = downloadHref(result);
  // 没有更新、检测失败、或两个地址都为空：这一格不出现。
  if (result === null || href.length === 0) return HIDDEN;
  if (openFailure !== null) {
    return {
      visible: true,
      label: '下载失败',
      detail: `打开下载页失败：${openFailure}。点击重试`,
      href,
      tone: 'danger',
    };
  }
  return { visible: true, label: '新版本', detail: `新版本：${result.latest}`, href, tone: 'link' };
}
