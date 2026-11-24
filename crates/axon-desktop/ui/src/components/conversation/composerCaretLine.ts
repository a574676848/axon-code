import { caretOnFirstLine, caretOnLastLine } from '../../conversation/inputHistory';

/*
 * 光标落在第几个可见行。
 *
 * textarea 里没有「行」这个 DOM 概念：软折行是排版结果，不是正文里的字符，因此
 * 「首行按上翻历史、末行按下翻历史」这套与 TUI 一致的约定不能只看换行符：一段
 * 没有换行符、却已经折成三行的草稿，光标明明在第三行，按上键也会被当成首行而
 * 直接翻历史。
 *
 * 输入坞本来就有一层与输入框同宽、同字体、同内边距的底衬（`data-composer-mirror`），
 * 这里就在它上面量：把一段零长 Range 落到光标处，取它的上边界即光标所在可见行的
 * 行首。底衬与输入框的折行规则一致，因此量出来的行就是用户看到的行。
 */

/** 输入坞底衬的选择器；由 ComposerTextArea 挂在底衬节点上。 */
export const COMPOSER_MIRROR_SELECTOR = '[data-composer-mirror]';

/** 判定容差：同一行的上边界会因为亚像素取整差不到 1px。 */
export const CARET_ROW_TOLERANCE_PX = 1;

/** 光标所在行相对首行 / 末行的位置。 */
export interface CaretPlacement {
  /** 光标在首个可见行。 */
  first: boolean;
  /** 光标在最后一个可见行。 */
  last: boolean;
}

/** 由三处行首上边界判定光标是否落在首行 / 末行。 */
export function caretPlacement(
  caretTop: number,
  headTop: number,
  tailTop: number,
): CaretPlacement {
  return {
    first: Math.abs(caretTop - headTop) <= CARET_ROW_TOLERANCE_PX,
    last: Math.abs(caretTop - tailTop) <= CARET_ROW_TOLERANCE_PX,
  };
}

/**
 * 量出光标是否落在首个 / 末个可见行。
 *
 * 量不出来时返回 null（底衬不在、光标不在两端之间可选的位置、引擎不给光标矩形）：
 * 调用方按「不能确定就不抢这一键」处理，用换行符近似兜底，方向键照常移动光标。
 *
 * 有选区时不翻历史：方向键在选区上本来是「收起选区」，此时接管会吞掉这次收起。
 */
export function caretLineBoundary(area: HTMLTextAreaElement): CaretPlacement | null {
  if (area.selectionStart !== area.selectionEnd) return { first: false, last: false };
  const mirror = area.parentElement?.querySelector(COMPOSER_MIRROR_SELECTOR) ?? null;
  if (mirror === null) return null;
  const caret = area.selectionStart;
  const headTop = caretRowTop(mirror, 0);
  const tailTop = caretRowTop(mirror, area.value.length);
  const caretTop = caretRowTop(mirror, caret);
  if (headTop === null || tailTop === null || caretTop === null) return null;
  return caretPlacement(caretTop, headTop, tailTop);
}

/**
 * 换行符近似的兜底判据。
 *
 * 只在实测不可用时使用：它把「没有换行符」当成「只有一行」，因此长草稿折行时
 * 会高估为可翻历史。实测路径正常时不会走到这里。
 */
export function caretLineBoundaryFallback(
  text: string,
  caret: number,
): CaretPlacement {
  return { first: caretOnFirstLine(text, caret), last: caretOnLastLine(text, caret) };
}

/** 底衬上某个正文字符所在可见行的行首上边界；量不出来时返回 null。 */
function caretRowTop(root: Element, offset: number): number | null {
  return caretRowBox(root, offset)?.top ?? null;
}

/**
 * 让光标落进可见区。
 *
 * 程序化落光标（翻历史、撤销、粘贴）不会让输入框自己滚过去：浏览器只在用户输入时
 * 才把光标滚进视野，`setSelectionRange` 之后光标可能停在框外，肉眼就是「光标不见了」。
 * 这里用底衬量出光标所在行，再把输入框正好滚到那一行。
 */
export function revealCaret(area: HTMLTextAreaElement): void {
  const mirror = area.parentElement?.querySelector(COMPOSER_MIRROR_SELECTOR) ?? null;
  if (mirror === null) return;
  const caret = caretRowBox(mirror, area.selectionStart);
  const head = caretRowBox(mirror, 0);
  if (caret === null || head === null) return;
  const style = getComputedStyle(area);
  const paddingTop = Number.parseFloat(style.paddingTop) || 0;
  const paddingBottom = Number.parseFloat(style.paddingBottom) || 0;
  const visible = Math.max(0, area.clientHeight - paddingTop - paddingBottom);
  const top = caret.top - head.top;
  const bottom = top + caret.height;
  if (top < area.scrollTop) {
    area.scrollTop = Math.max(0, top);
    return;
  }
  if (bottom > area.scrollTop + visible) {
    area.scrollTop = Math.max(0, bottom - visible);
  }
}

/** 底衬上某个正文字符所在可见行的盒子；量不出来时返回 null。 */
function caretRowBox(root: Element, offset: number): { top: number; height: number } | null {
  const nodes: Text[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
    nodes.push(node as Text);
  }
  if (nodes.length === 0) return null;
  let remaining = Math.max(0, offset);
  let target = nodes[nodes.length - 1];
  let within = target.data.length;
  for (const node of nodes) {
    if (remaining <= node.data.length) {
      target = node;
      within = remaining;
      break;
    }
    remaining -= node.data.length;
  }
  const range = document.createRange();
  range.setStart(target, within);
  range.collapse(true);
  const rect = range.getBoundingClientRect();
  // 空矩形说明引擎没给光标位置（例如正文末尾落在被裁掉的盒子里）：交给兜底。
  return rect.height > 0 ? { top: rect.top, height: rect.height } : null;
}
