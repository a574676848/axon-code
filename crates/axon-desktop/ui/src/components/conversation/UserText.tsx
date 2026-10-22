import React from 'react';

import { useSkillNames } from '../../conversation/skillCatalog';
import { splitMessageSegments } from '../../conversation/textSegments';
import { LinkChip } from './LinkChip';

/*
 * 用户消息正文：技能指令照旧点亮，网页地址收敛成一枚可打开的链接芯片。
 *
 * 切分与输入框底衬共用 textSegments：先认链接、再认技能，因此地址里的斜杠
 * 不会被拆成技能指令。消息原文不参与渲染之外的任何改写，复制与重新发送都取原文。
 */
export const UserText: React.FC<{ text: string }> = ({ text }) => {
  const names = useSkillNames();
  const segments = React.useMemo(() => splitMessageSegments(text, names), [text, names]);
  return (
    <>
      {segments.map((segment, index) =>
        segment.kind === 'link' && segment.url !== undefined ? (
          <LinkChip key={index} url={segment.url} />
        ) : segment.kind === 'skill' ? (
          <span key={index} className="text-skill-ink">
            {segment.text}
          </span>
        ) : (
          <React.Fragment key={index}>{segment.text}</React.Fragment>
        ),
      )}
    </>
  );
};
