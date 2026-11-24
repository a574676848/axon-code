/*
 * 轮后提示的展示文案。
 *
 * 运行侧按真实治理结果给出文案，其中「本轮」指的是产出它的那一轮。这里**原样透传**：
 * 客户端不改写任何字。改写成「上一轮」会让同一条提示在命令行、TUI 与桌面三处说法不同，
 * 而提示里的自述才是唯一权威口径。
 *
 * 只做一件事：没有提示、空串与纯空白一律返回 `null`，渲染一行空白等于凭空造出一条提示。
 */

/**
 * 轮后提示的展示文案。
 *
 * @param raw 网关给出的原始文案；缺省、空串与纯空白都视为「没有提示」。
 * @returns 原样透传的文案；没有提示时返回 `null`。
 */
export function reflectionNoticeText(raw: string | null | undefined): string | null {
  const text = (raw ?? '').trim();
  return text.length === 0 ? null : text;
}
