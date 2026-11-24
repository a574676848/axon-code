/*
 * 口播稿净化。
 *
 * 正文是给眼睛看的 Markdown：围栏、星号与整条网址直接送去合成既难听也没用。
 * 这里把它收敛成一段能听的文字，规则只服务于朗读，不改正文：复制、导出与
 * 界面渲染读到的仍然是消息原文。
 *
 * 五条处理：代码不出声、标记不读出、绝对路径只念文件名、网址只念域名、
 * 令牌与邮箱换成类别词。合成侧的入参上限是 4000 字符，截断在客户端完成。
 */

/** 围栏代码块：整段不出声；未闭合的围栏一直吃到结尾。 */
const FENCED_CODE_BLOCK = /```[\s\S]*?(?:```|$)/g;

/** 行内代码：只摘掉反引号，里面的词照常念。 */
const INLINE_CODE = /`([^`\n]*)`/g;

/** Markdown 图片：留替代文字，图片地址不出声。 */
const IMAGE_LINK = /!\[([^\]]*)\]\([^)]*\)/g;

/** Markdown 链接：只念可见文字，地址不出声。 */
const INLINE_LINK = /\[([^\]]*)\]\([^)]*\)/g;

/** HTML 标签：正文里的行内标签不参与朗读。 */
const HTML_TAG = /<\/?[A-Za-z][^>\n]{0,64}>/g;

/** 常见令牌前缀；整串换成类别词，不让凭据进扬声器。 */
const TOKEN =
  /(?:\bsk-[A-Za-z0-9_-]{8,}|\bgh[pousr]_[A-Za-z0-9]{8,}|\bxox[baprs]-[A-Za-z0-9-]{8,}|\bAKIA[0-9A-Z]{16}\b|\beyJ[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,}\.[A-Za-z0-9_-]{6,})/g;

/** Bearer 头里的凭据。 */
const BEARER_TOKEN = /\bBearer\s+[A-Za-z0-9._~+/=-]{8,}/gi;

/** 邮箱。 */
const EMAIL = /[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g;

/** http 与 https 地址。 */
const URL = /\bhttps?:\/\/[^\s<>()[\]（）【】「」，。；、]+/gi;

/** Windows 绝对路径：盘符开头或 UNC 开头。 */
const WINDOWS_PATH = /(?:[A-Za-z]:[\\/]|\\\\)[^\s<>()[\]（）【】「」，。；、'"`]+/g;

/** POSIX 绝对路径：至少两级，避免把算式里的斜杠当成路径。 */
const POSIX_PATH =
  /(?:^|[\s(（"'`])\/(?:[^\s/<>()[\]（）【】「」，。；、'"`]+\/)+[^\s/<>()[\]（）【】「」，。；、'"`]+/g;

/** 行首的 Markdown 块级标记。 */
function stripBlockMarkers(line: string): string {
  return line
    .replace(/^\s{0,3}#{1,6}\s*/, '')
    .replace(/^\s{0,3}>\s?/, '')
    .replace(/^\s*\|?\s*:?-{2,}:?\s*(?:\|\s*:?-{2,}:?\s*)+\|?\s*$/, '')
    .replace(/^\s*[-*+]\s+/, '')
    .replace(/^\s*\d+[.)]\s+/, '')
    .replace(/^\s*(?:[-*_]\s*){3,}$/, '')
    .replace(/\|/g, ' ');
}

/** 取路径末段：口播只念文件名，不逐层念目录。 */
function fileNameOf(path: string): string {
  const segments = path.split(/[\\/]/).filter((segment) => segment.length > 0);
  return segments[segments.length - 1] ?? path;
}

/** 取域名：口播只念站点，不逐段念路径与查询串。 */
function domainOf(url: string): string {
  const withoutScheme = url.replace(/^https?:\/\//i, '');
  const host = withoutScheme.split(/[/?#]/)[0] ?? '';
  return host.replace(/^www\./i, '').replace(/:\d+$/, '');
}

/** POSIX 路径匹配带了前导空白或括号，替换时原样还回去。 */
function replacePosixPath(match: string): string {
  const lead = /^[\s(（"'`]/.test(match) ? match[0] : '';
  const path = lead.length > 0 ? match.slice(1) : match;
  return `${lead}${fileNameOf(path)}`;
}

/** 把消息原文收敛成口播稿；没有可读文字时返回空串。 */
export function speakableScript(source: string): string {
  if (source.trim().length === 0) return '';
  let text = source.replace(FENCED_CODE_BLOCK, ' ');
  text = text.replace(IMAGE_LINK, '$1');
  text = text.replace(INLINE_LINK, '$1');
  text = text.replace(INLINE_CODE, '$1');
  text = text.replace(HTML_TAG, ' ');
  text = text.replace(BEARER_TOKEN, '令牌');
  text = text.replace(TOKEN, '令牌');
  text = text.replace(EMAIL, '邮箱');
  text = text.replace(URL, (match) => domainOf(match));
  text = text.replace(WINDOWS_PATH, (match) => fileNameOf(match));
  text = text.replace(POSIX_PATH, (match) => replacePosixPath(match));
  text = text.split('\n').map(stripBlockMarkers).join('\n');
  text = text
    .replace(/\*\*([^*\n]+)\*\*/g, '$1')
    .replace(/__([^_\n]+)__/g, '$1')
    .replace(/~~([^~\n]+)~~/g, '$1')
    .replace(/\*([^*\n]+)\*/g, '$1')
    .replace(/(^|[\s(（])_([^_\n]+)_(?=[\s)）.,，。!?！？]|$)/g, '$1$2')
    .replace(/\*+/g, '')
    .replace(/~~+/g, '');
  return text.replace(/\s+/g, ' ').trim();
}
