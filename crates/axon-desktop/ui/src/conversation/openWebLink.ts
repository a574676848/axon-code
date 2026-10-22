import * as ipc from '../ipc';

/*
 * 用系统浏览器打开一个网页地址。
 *
 * 打开失败时把地址复制到剪贴板再回话：桌面端没有浏览器地址栏这一层，
 * 只报一句「打不开」等于把地址丢了。返回值是可读的失败原因，成功时为 null。
 */
export async function openWebLink(url: string): Promise<string | null> {
  try {
    await ipc.openExternalUrl(url);
    return null;
  } catch (failure) {
    const reason = failure instanceof Error ? failure.message : `无法打开 ${url}`;
    const copied = await ipc.clipboardWriteText(url).then(
      () => true,
      () => false,
    );
    return copied ? `${reason}；地址已复制，可粘贴到浏览器打开` : reason;
  }
}
