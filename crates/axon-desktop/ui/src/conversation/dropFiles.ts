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
