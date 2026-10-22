import React from 'react';

import type { SkillItem } from '../../types';
import { useSkillCatalog } from '../../conversation/skillCatalog';
import {
  activeSlashToken,
  rankCandidateList,
  type SlashToken,
} from '../../conversation/composerCandidates';

/*
 * `/` 指令补全控制器（真实来源是已安装技能，不是前端硬编码清单）。
 *
 * 只负责取数、缓存与采纳写回：token 判定与候选排序都在
 * conversation/composerCandidates 里，技能清单本身在 conversation/skillCatalog
 * （全应用共用一次读取，输入框的正文高亮读的也是它），组件只把结果摆到输入坞上方。
 *
 * 收起是「这一条不再弹」，不是「以后都不弹」：Esc 记住当前 token 的身份，
 * 光标不动、正文不变时菜单不再回来；正文一变（多打一个字）或这一条 token
 * 消失，记忆立即作废，菜单照常出现。
 */

/** 判定一条 token 是否就是被收起的那一条。 */
function tokenKey(token: SlashToken): string {
  return `${token.start}:${token.end}:${token.query}`;
}

interface SlashCompletionInput {
  /** 输入框正文。 */
  text: string;
  /** 光标位置。 */
  caret: number;
}

export interface SlashCompletion {
  /** 浮层是否该出现：有活跃 token、没被收起、且确实有候选。 */
  open: boolean;
  /** 当前候选；为空表示没有可展示的东西。 */
  options: SkillItem[];
  /** 当前选中项。 */
  index: number;
  setIndex: (index: number) => void;
  /** 采纳某一项；返回写回正文的文本与新的光标位置，无法采纳时为 null。 */
  accept: (index: number, text: string) => { text: string; caret: number } | null;
  /** 收起浮层（Esc）。 */
  dismiss: () => void;
  /** 提交之后清掉收起记忆。 */
  reset: () => void;
}

export function useSlashCompletion({ text, caret }: SlashCompletionInput): SlashCompletion {
  const skills = useSkillCatalog();
  const [index, setIndex] = React.useState(0);
  /** 被 Esc 收起的那一条 token；身份变了就自动失效。 */
  const [dismissed, setDismissed] = React.useState<SlashToken | null>(null);

  const token = React.useMemo(() => activeSlashToken(text, caret), [text, caret]);
  const key = token === null ? null : tokenKey(token);

  // 这一条 token 消失（打成空格、删掉斜杠）：收起的记忆随之作废，下次照常弹。
  React.useEffect(() => {
    if (token === null) setDismissed(null);
  }, [token]);

  // 查询词变了就把选中项收回第一条：列表整体换过，留着旧下标会停在无关的行上。
  React.useEffect(() => {
    setIndex(0);
  }, [key]);

  const options = React.useMemo(
    () => (skills === null || token === null ? [] : rankCandidateList(skills, token.query)),
    [skills, token],
  );

  /** 选中的行：两次渲染之间列表变短时当场夹回末行，不会指到不存在的行上。 */
  const activeIndex = options.length === 0 ? 0 : Math.min(index, options.length - 1);

  const accept = React.useCallback(
    (position: number, draft: string) => {
      const skill = options[position];
      if (skill === undefined || token === null) return null;
      // 指令后面补一个空格：这一次指令到此结束，接着写参数。
      const insertion = `/${skill.name} `;
      return {
        text: `${draft.slice(0, token.start)}${insertion}${draft.slice(token.end)}`,
        caret: token.start + insertion.length,
      };
    },
    [options, token],
  );

  const dismiss = React.useCallback(() => {
    if (token !== null) setDismissed(token);
  }, [token]);

  const reset = React.useCallback(() => {
    setDismissed(null);
    setIndex(0);
  }, []);

  return {
    open: options.length > 0 && (dismissed === null || tokenKey(dismissed) !== key),
    options,
    index: activeIndex,
    setIndex,
    accept,
    dismiss,
    reset,
  };
}
