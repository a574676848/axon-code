import React from 'react';
import { CodeBlock } from './CodeBlock';
import { formatToolContent, shellCommand } from './toolPresentation';

/** 工具输入输出复用代码高亮内核，原始文本与命令的换行始终保留。 */
export const ToolContent: React.FC<{
    title: string;
    value: string;
    shell?: boolean;
    language?: string;
}> = ({ title, value, shell = false, language }) => {
    const formatted = formatToolContent(value);
    const command = shell ? shellCommand(value) : null;
    return (
        <section aria-label={title} className="py-1 min-w-0">
            <div className="text-meta text-text-tertiary mb-1">{title}</div>
            <CodeBlock code={formatted.code}
                language={formatted.language ?? language ?? (shell ? 'shell' : null)} compact />
            {command !== null && (
                <div className="mt-2">
                    <div className="text-meta text-text-tertiary mb-1">命令</div>
                    <CodeBlock code={command} language="shell" compact />
                </div>
            )}
        </section>
    );
};
