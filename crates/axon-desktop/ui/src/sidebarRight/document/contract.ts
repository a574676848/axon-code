/*
 * 渲染器与所有者之间的契约。
 *
 * 所有者只管「读什么、什么时候读、工具栏长什么样」，渲染器只管「把这些内容画出来」。
 * 两者之间只有这一个类型，因此新增一种看法不必回头改所有者。
 */

/** 渲染器拿到的内容。 */
export type ViewerContent =
  /** 文本内容：按文本读来的，可能因超出上限被截断。 */
  | { kind: 'text'; text: string; truncated: boolean }
  /** 原始字节：图片、PDF、HTML 必须拿到未解码的字节。 */
  | { kind: 'bytes'; mime: string; base64: string; truncated: boolean };

/** 渲染器的入参。 */
export interface ViewerProps {
  /** 工作区根目录绝对路径。 */
  root: string;
  /** 工作区内的相对路径。 */
  path: string;
  /** 这份文件的内容。 */
  content: ViewerContent;
  /** 是否自动换行；不支持换行的渲染器忽略它。 */
  wrap: boolean;
  /** 需要定位到的行号（从 1 开始）；只有文本类渲染器用得上。 */
  line?: number;
}

/** 把字节内容拼成浏览器能直接吃的 data 地址。 */
export function dataUrlOf(content: ViewerContent): string {
  return content.kind === 'bytes' ? `data:${content.mime};base64,${content.base64}` : '';
}

/** 把 base64 还原成字节；渲染二进制内容的渲染器用它建 Blob。 */
export function bytesOf(content: ViewerContent): Uint8Array {
  if (content.kind !== 'bytes') return new Uint8Array();
  const binary = window.atob(content.base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}
