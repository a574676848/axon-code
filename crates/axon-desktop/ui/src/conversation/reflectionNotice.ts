/*
 * 轮后提示的展示文案。
 *
 * 后端按「产出它的那一轮」写「本轮…」，而这一行落在时间线尾部：新一轮在下面跑时，
 * 同一个「本轮」会被读成正在跑的这轮。展示时统一改写成「上一轮」，一句话说清它说的是谁。
 *
 * 只认「记忆反思：本轮」这一个开头：别的文案、以及提示里其余的字都不动。
 */

const NOTICE_PREFIX = '记忆反思：';
const NOTICE_CURRENT_TURN = '本轮';
const NOTICE_PREVIOUS_TURN = '上一轮';
const NOTICE_HEAD = `${NOTICE_PREFIX}${NOTICE_CURRENT_TURN}`;

/**
 * 轮后提示的展示文案。
 *
 * 没有提示、空串与纯空白一律返回 `null`：网关偶尔给出空串，渲染一行空白等于凭空
 * 造出一条提示。
 */
export function reflectionNoticeText(raw: string | null | undefined): string | null {
  const text = (raw ?? '').trim();
  if (text.length === 0) return null;
  if (!text.startsWith(NOTICE_HEAD)) return text;
  return `${NOTICE_PREFIX}${NOTICE_PREVIOUS_TURN}${text.slice(NOTICE_HEAD.length)}`;
}
