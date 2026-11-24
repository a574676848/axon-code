import React from 'react';
import { useExpiringNotice } from './composerNotice';

/*
 * 轮后提示的停留时长：够读完一句结论，又不在时间线尾部长期占位。
 *
 * 它是「上一次收口之后发生了什么」的一次性注记，不是需要一直挂着的状态；把 30 秒
 * 用完就退场，尾巴让给下一轮正文。
 */
export const REFLECTION_NOTICE_MS = 30_000;

/**
 * 让轮后提示亮一段就自己退场。
 *
 * 计时身份是「这次出现」：换会话，或提示内容变了，都重新亮起并从零计时；同一段提示
 * 到点退场之后，普通重渲染不会把它叫回来（依赖没变，本地状态也不会自己长回来）。
 */
export function useReflectionNotice(
  notice: string | null,
  sessionKey: string | null,
): string | null {
  const [visible, setVisible] = React.useState<string | null>(notice);
  React.useEffect(() => {
    setVisible(notice);
  }, [notice, sessionKey]);
  useExpiringNotice(visible, () => setVisible(null), REFLECTION_NOTICE_MS, sessionKey);
  return visible;
}
