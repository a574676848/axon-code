import React from 'react';

import { ComposerCandidateMenu } from './ComposerCandidates';
import type { ReferenceCompletion } from './useReferenceCompletion';
import type { SlashCompletion } from './useSlashCompletion';

/*
 * 输入坞上方的候选浮层：文件引用与斜杠指令各一组，同一时刻至多出现一组。
 *
 * 两组候选的输入判据互斥（光标之前都不能有空白），因此这里不做切换，只把各自
 * 的列表摆到同一个锚点上。浮层贴着卡片上沿展开，展开与收起都不改卡片版面。
 */
export const ComposerCandidateLayer: React.FC<{
  /** 候选菜单的锚点：输入坞卡片，菜单贴在它的上沿。 */
  anchor: HTMLElement | null;
  slashListId: string;
  referenceListId: string;
  slash: SlashCompletion;
  references: ReferenceCompletion;
  onAcceptSlash: (index: number) => void;
  onAcceptReference: (index: number) => void;
}> = ({
  anchor,
  slashListId,
  referenceListId,
  slash,
  references,
  onAcceptSlash,
  onAcceptReference,
}) => (
  <>
    {slash.open && (
      <ComposerCandidateMenu
        id={slashListId}
        anchor={anchor}
        label="斜杠指令"
        items={slash.options.map((skill) => ({
          id: skill.id,
          label: `/${skill.name}`,
          hint: skill.description,
        }))}
        activeIndex={slash.index}
        onActivate={slash.setIndex}
        onAccept={onAcceptSlash}
        onDismiss={slash.dismiss}
      />
    )}
    {references.options.length > 0 && (
      <ComposerCandidateMenu
        id={referenceListId}
        anchor={anchor}
        label="文件引用"
        items={references.options.map((entry) => ({
          id: entry.path,
          label: `${references.directory}${entry.name}`,
          hint: entry.kind === 'dir' ? '目录' : '文件',
        }))}
        activeIndex={references.index}
        onActivate={references.setIndex}
        onAccept={onAcceptReference}
        onDismiss={references.dismiss}
      />
    )}
  </>
);
