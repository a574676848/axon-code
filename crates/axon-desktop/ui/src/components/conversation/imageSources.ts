/*
 * 图片的字节通道：一次读取、按持有者计数回收。
 *
 * 地址过去是 data URL。一张 8 MiB 的截图在 JS 堆里是一条约 10.7 MiB 的字符串，
 * 解码后的位图还要另算一份，两者在整条会话里同时常驻；同一张图被引用几次就多出
 * 几份字符串。这里换成 Blob 地址：字符串换成由浏览器托管的压缩字节，解码位图由
 * img 自己管理，持有者归零即撤销地址。
 *
 * 这里刻意不留常驻缓存：地址只要活得比「当前真的显示着的那些图」更久，就等于把
 * 已经看不见的图钉在内存里，而这正是要解决的那件事。滚回来再看一次的代价是一次
 * IPC，换来的是内存随可见内容走，而不是随浏览历史走。
 */

import * as ipc from '../../ipc';

/** 一张图的使用权；用完必须调用 release。 */
export interface ImageHandle {
  /** 可直接交给 img.src 的地址。 */
  url: string;
  /** 归还使用权；最后一个持有者归还时撤销地址。 */
  release: () => void;
}

/** 一个路径当前有多少个持有者。 */
interface Entry {
  url: string;
  holders: number;
}

/** 已经读出地址、仍被持有的图。 */
const ready = new Map<string, Entry>();
/** 正在读取的图；同一路径并发请求只读一次。 */
const pending = new Map<string, Promise<string>>();

/** 把一次读取的结果转成可渲染的地址。 */
async function readDisplayUrl(path: string): Promise<string> {
  const source = await ipc.attachmentSource(path);
  /*
   * 浏览器调试桥给的是网关的字节路由，本身就是一条可复用的地址：直接交给 img，
   * 生命周期归浏览器管（它没有对应的撤销动作）。
   */
  if (!source.startsWith('data:')) return source;
  /*
   * 桌面窗口给的是 data URL。转成 Blob 地址再交给 img：8 MiB 的截图在堆里是一条
   * 约 10.7 MiB 的字符串，转成 Blob 之后只剩压缩字节那一份。
   *
   * 这里自己做解码而不用 fetch：解析不依赖网络层对 data 协议的支持，改 CSP 也
   * 不会把图片一起拦掉。
   */
  const comma = source.indexOf(',');
  if (comma < 0) throw new Error('图片地址格式不正确');
  const mime = source.slice(5, comma).split(';')[0] || 'application/octet-stream';
  const binary = atob(source.slice(comma + 1));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return URL.createObjectURL(new Blob([bytes], { type: mime }));
}

/** 给一个已经就绪的地址加一份持有。 */
function claim(path: string, url: string): ImageHandle {
  const entry = ready.get(path);
  if (entry === undefined) {
    ready.set(path, { url, holders: 1 });
  } else {
    entry.holders += 1;
  }
  let released = false;
  return {
    url,
    release: () => {
      // 归还必须幂等：effect 的清理函数与一次显式释放可能各跑一次。
      if (released) return;
      released = true;
      const current = ready.get(path);
      if (current === undefined) return;
      current.holders -= 1;
      if (current.holders > 0) return;
      ready.delete(path);
      // 只有 Blob 地址需要撤销；调试桥那条字节路由没有生命周期可管。
      if (current.url.startsWith('blob:')) URL.revokeObjectURL(current.url);
    },
  };
}

/** 取得一个路径的图片使用权；读不出来时抛错。 */
export async function acquireImageUrl(path: string): Promise<ImageHandle> {
  const existing = ready.get(path);
  if (existing !== undefined) return claim(path, existing.url);
  let reading = pending.get(path);
  if (reading === undefined) {
    reading = readDisplayUrl(path);
    pending.set(path, reading);
  }
  try {
    return claim(path, await reading);
  } finally {
    pending.delete(path);
  }
}

/**
 * 按候选顺序取第一张读得出来的图。
 *
 * 模型给出的路径未必干净：被引号包住、粘着中文标点、带空格或全角冒号。候选之间
 * 不做存在性判断，逐个试到第一个能读出来的为止，失败原因留最后一次。
 */
export async function acquireFirstImage(paths: readonly string[]): Promise<ImageHandle> {
  let failure: unknown = new Error('没有可读取的图片路径');
  for (const path of paths) {
    try {
      return await acquireImageUrl(path);
    } catch (error) {
      failure = error;
    }
  }
  throw failure;
}
