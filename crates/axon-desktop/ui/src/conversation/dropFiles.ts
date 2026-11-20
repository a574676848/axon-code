/*
 * 拖入项的判别：把可读文件与被拒目录分开。
 *
 * 文件夹在浏览器里同样是一个 File 对象，但读它一定失败。若不在落盘之前分开，
 * 用户看到的只会是一句「读取 xxx 失败」，看不出真正原因是拖了文件夹。
 */

/** 一次拖放的判别结果。 */
export interface DroppedItems {
  /** 可以落盘的文件。 */
  files: File[];
  /** 被拒的目录名。 */
  directories: string[];
}

/**
 * 单个附件允许的体量上限。
 *
 * 与正文里图片读取的 8 MiB 同一口径：落盘要把整份文件读成 base64，超过这个体量的
 * 文件既拖不动也读不完，界面该给的是一句结论而不是一段卡死的等待。
 */
export const MAX_ATTACHMENT_BYTES = 8 * 1024 * 1024;

/** 超限时的说法；未超限返回 null。 */
export function attachmentSizeError(name: string, size: number): string | null {
  if (size <= MAX_ATTACHMENT_BYTES) return null;
  const limitMb = Math.round(MAX_ATTACHMENT_BYTES / (1024 * 1024));
  return `超过 ${limitMb} MB 不支持：${name.length > 0 ? name : '未命名文件'}`;
}

/**
 * 按体量把待落盘的文件分成两组。
 *
 * 判别与落盘分开：超限的文件要在读之前就被挑出来，既不占内存，也能单独给一句回执。
 */
export function partitionBySize(files: readonly File[]): { accepted: File[]; rejected: File[] } {
  const accepted: File[] = [];
  const rejected: File[] = [];
  for (const file of files) {
    if (attachmentSizeError(file.name, file.size) === null) accepted.push(file);
    else rejected.push(file);
  }
  return { accepted, rejected };
}

/** 判定单个拖入项是不是目录；拿不到目录信息时按文件处理。 */
function isDirectory(item: DataTransferItem): boolean {
  const entry = (
    item as DataTransferItem & { webkitGetAsEntry?: () => { isDirectory: boolean } | null }
  ).webkitGetAsEntry?.();
  return entry?.isDirectory === true;
}

/**
 * 判别一次拖放。
 *
 * 优先按 items 逐项判别（只有它带目录信息），items 缺失时退回 files。
 */
export function classifyDroppedItems(drop: {
  items?: ArrayLike<DataTransferItem> | null;
  files?: ArrayLike<File> | null;
}): DroppedItems {
  const files: File[] = [];
  const directories: string[] = [];
  const items = drop.items;
  if (items !== undefined && items !== null && items.length > 0) {
    for (const item of Array.from(items)) {
      if (item.kind !== 'file') continue;
      const file = item.getAsFile();
      if (file === null) continue;
      if (isDirectory(item)) {
        directories.push(file.name.length > 0 ? file.name : '未命名文件夹');
        continue;
      }
      files.push(file);
    }
    if (files.length > 0 || directories.length > 0) return { files, directories };
  }
  return { files: Array.from(drop.files ?? []), directories };
}
