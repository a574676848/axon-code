import React from 'react';

import {
  expandLinks,
  liveLinks,
  tokenizePastedText,
  type LinkToken,
} from '../../conversation/linkTokens';
import type { CaretRange } from './useComposerEditing';

/*
 * 输入坞的链接占位：粘贴时把地址换成一个短占位符，提交时再展开回原文。
 *
 * 占位清单只增不减：从正文删掉的占位仍留在清单里，撤销才能把同一个占位连同它的
 * 地址一起带回来，序号也不会让给下一条链接。提交或清空草稿时整份复位。
 */

/** 粘贴投影：替换后的正文与光标落点。 */
export interface LinkPasteStep {
  text: string;
  caret: number;
}

export interface ComposerLinksController {
  /** 草稿里仍然看得见的链接，按正文位置排序。 */
  links: LinkToken[];
  /** 全部登记过的占位：底衬按它认链接，含已从正文删掉、撤销还能带回来的那些。 */
  tokens: LinkToken[];
  /** 粘贴接管：正文里有地址时写入占位并返回 true。 */
  paste: (pasted: string, range: CaretRange) => boolean;
  /** 右键菜单的粘贴投影：只算结果，写入交给编辑控制器。 */
  projectPaste: (pasted: string, range: CaretRange) => LinkPasteStep | null;
  /** 删掉一个链接在正文里的全部占位。 */
  remove: (placeholder: string) => void;
  /** 提交前展开。 */
  expand: (text: string) => string;
  /** 提交或清空草稿之后复位。 */
  reset: () => void;
}

export interface ComposerLinksOptions {
  /** 当前正文。 */
  text: string;
  /** 写入正文（走编辑控制器，撤销粒度与一次粘贴对齐）。 */
  commit: (next: string) => void;
  /** 程序化写入之后把光标落到某处。 */
  requestCaret: (position: number) => void;
}

export function useComposerLinks({
  text,
  commit,
  requestCaret,
}: ComposerLinksOptions): ComposerLinksController {
  const [tokens, setTokens] = React.useState<LinkToken[]>([]);
  /*
   * 正文与清单各留一份即时值：右键菜单的粘贴要先读剪贴板再写入，
   * 这段异步窗口里草稿可能已经变了，闭包里的渲染快照会把正文插到旧位置上。
   */
  const textRef = React.useRef(text);
  textRef.current = text;
  const tokensRef = React.useRef(tokens);
  tokensRef.current = tokens;

  /** 算一次粘贴的结果并登记新占位；没有地址时返回 null。 */
  const plan = React.useCallback((pasted: string, range: CaretRange): LinkPasteStep | null => {
    const draft = textRef.current;
    const step = tokenizePastedText(pasted, draft, tokensRef.current);
    if (step === null) return null;
    setTokens(step.tokens);
    return {
      text: draft.slice(0, range.start) + step.text + draft.slice(range.end),
      caret: range.start + step.text.length,
    };
  }, []);

  const paste = React.useCallback(
    (pasted: string, range: CaretRange): boolean => {
      const step = plan(pasted, range);
      if (step === null) return false;
      commit(step.text);
      requestCaret(step.caret);
      return true;
    },
    [commit, plan, requestCaret],
  );

  const projectPaste = React.useCallback(
    (pasted: string, range: CaretRange): LinkPasteStep | null => plan(pasted, range),
    [plan],
  );

  const remove = React.useCallback(
    (placeholder: string) => {
      const draft = textRef.current;
      const next = draft.split(placeholder).join('');
      if (next !== draft) commit(next);
    },
    [commit],
  );

  const expand = React.useCallback(
    (draft: string): string => expandLinks(draft, tokensRef.current),
    [],
  );

  const reset = React.useCallback(() => {
    tokensRef.current = [];
    setTokens([]);
  }, []);

  return {
    links: liveLinks(text, tokens),
    tokens,
    paste,
    projectPaste,
    remove,
    expand,
    reset,
  };
}
