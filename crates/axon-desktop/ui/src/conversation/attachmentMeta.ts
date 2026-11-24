/*
 * 附件卡的展示元信息：类别词与体量。
 *
 * 只做「把已知事实说成人话」这一件事：类别词由扩展名与图片标记推出，
 * 体量由后端给出的字节数推出。两者都取不到时返回空串，界面就不显示第二行，
 * 绝不猜类型、也不拿 0 冒充未知体量。
 */

/** 附件类别词。 */
export interface AttachmentMeta {
  /** 类别词，例如 `Excel`、`Markdown`、`图片`；判不出时为空串。 */
  label: string;
  /** 体量文本，例如 `5KB`；后端没给字节时为空串。 */
  size: string;
}

/** 扩展名到类别词。表里没有的类型保持空串，不硬塞「文件」这类无信息量的词。 */
const KIND_LABELS: Readonly<Record<string, string>> = {
  xlsx: 'Excel',
  xls: 'Excel',
  csv: 'CSV',
  docx: 'Word',
  doc: 'Word',
  pptx: 'PowerPoint',
  ppt: 'PowerPoint',
  pdf: 'PDF',
  md: 'Markdown',
  markdown: 'Markdown',
  txt: '文本',
  log: '日志',
  json: 'JSON',
  yaml: 'YAML',
  yml: 'YAML',
  toml: 'TOML',
  ini: 'INI',
  zip: '压缩包',
  '7z': '压缩包',
  rar: '压缩包',
  tar: '压缩包',
  gz: '压缩包',
};

/** 体量单位换算：1024 进制，KB 起保留一位小数以内。 */
const BYTES_PER_KB = 1024;
const BYTES_PER_MB = 1024 * 1024;

/** 取扩展名（小写，不含点）。 */
function extensionOf(name: string): string {
  const index = name.lastIndexOf('.');
  return index <= 0 ? '' : name.slice(index + 1).toLowerCase();
}

/** 体量文本：小于 1KB 给整数字节，否则给 KB／MB。 */
export function formatAttachmentSize(bytes: number | null): string {
  if (bytes === null || bytes < 0) return '';
  if (bytes < BYTES_PER_KB) return `${bytes}B`;
  if (bytes < BYTES_PER_MB) return `${Math.round(bytes / BYTES_PER_KB)}KB`;
  return `${(bytes / BYTES_PER_MB).toFixed(1)}MB`;
}

/** 组装附件卡的元信息。 */
export function attachmentMeta(input: {
  name: string;
  image: boolean;
  bytes: number | null;
}): AttachmentMeta {
  const label = input.image ? '图片' : (KIND_LABELS[extensionOf(input.name)] ?? '');
  return { label, size: formatAttachmentSize(input.bytes) };
}

/** 元信息成行：类别与体量都缺时返回空串，界面整行不渲染。 */
export function attachmentMetaText(meta: AttachmentMeta): string {
  return [meta.label, meta.size].filter((part) => part.length > 0).join(' · ');
}
