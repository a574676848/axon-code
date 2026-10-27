import React from 'react';
import { CodeBlock } from './CodeBlock';
import { formatToolContent, shellCommand } from './toolPresentation';

/** 工具输入输出复用代码高亮内核，标题直接融合进紧凑代码块顶栏，消除双层表头噪音。 */
export const ToolContent: React.FC<{
  title: string;
  value: string;
  shell?: boolean;
  language?: string;
}> = ({ title, value, shell = false, language }) => {
  const formatted = formatToolContent(value);
  const command = shell ? shellCommand(value) : null;
  return (
    <section aria-label={title} className="py-1 min-w-0 space-y-2">
      <CodeBlock
        title={title}
        code={formatted.code}
        language={formatted.language ?? language ?? (shell ? 'shell' : null)}
        compact
      />
      {command !== null && (
        <CodeBlock title="命令" code={command} language="shell" compact />
      )}
    </section>
  );
};
