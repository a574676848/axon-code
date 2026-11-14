import React from 'react';
import { FileIcon } from '../icons';

/*
 * 引用有歧义时的候选页。
 *
 * 正文里写的是 `docs/a.md`，会话里却有两份同名文件：这时不替用户选，把候选
 * 摆在文件打开的地方，点哪一份就开哪一份。
 *
 * 只列绝对路径。相对形式正是歧义的来源，再摆一次只是把同一个问题推回给用户。
 */

export interface ChoiceBodyProps {
  /** 正文里那一段原文，用来解释「在给什么选」。 */
  reference: string;
  /** 候选绝对路径，顺序即优先级。 */
  candidates: readonly string[];
  /** 选中一份：调用方按既有入口打开它。 */
  onOpen: (path: string) => void;
}

export const ChoiceBody: React.FC<ChoiceBodyProps> = ({ reference, candidates, onOpen }) => (
  <div className="flex-1 min-h-0 overflow-y-auto scroll-visible px-4 py-3">
    <div className="flex items-center gap-1.5">
      <FileIcon size={14} className="shrink-0 text-text-tertiary" />
      <span className="min-w-0 truncate font-mono text-meta text-text-tertiary" title={reference}>
        {reference}
      </span>
    </div>
    <p className="mt-2 text-meta text-text-caption">
      这条路径对上了 {candidates.length} 份文件，选一份打开。
    </p>
    <ul className="mt-2 flex flex-col gap-0.5">
      {candidates.map((path) => (
        <li key={path}>
          <button
            type="button"
            onClick={() => onOpen(path)}
            title={path}
            className="w-full min-h-[30px] px-2 py-1 flex items-center gap-1.5 rounded-md text-left text-row leading-5 text-text-primary transition-colors duration-100 hover:bg-hover"
          >
            <FileIcon size={14} className="shrink-0 text-text-tertiary" />
            <span className="flex-1 min-w-0 truncate font-mono text-meta">{path}</span>
          </button>
        </li>
      ))}
    </ul>
  </div>
);
