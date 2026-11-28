import React from 'react';
import { CheckIcon, CopyIcon, DownloadIcon } from '../../icons';
import { exportMarkdownName } from '../../conversation/textExport';
import { Markdown } from './Markdown';
import { MessageActionButton, MessageFooter } from './MessageFooter';
import * as ipc from '../../ipc';

/*
 * 助手正文：通栏白板，无气泡。
 *
 * 页脚与用户消息、问答单元共用同一份 MessageFooter：时间常显，
 * 操作按钮只在悬浮或键盘聚焦时出现，图形 14px、命中区 28px。
 *
 * 复制交出的是正文原始的 Markdown 源文，而不是渲染后的可见文本：
 * 用户要的是能直接贴到别处的 Markdown，不是丢了标记的纯文本。
 *
 * 语音只剩输入坞里的 realtime 对话：正文不再挂朗读按钮，
 * 因此这里只负责复制与导出两种「把正文拿走」的动作。
 */

export const AssistantBlock: React.FC<{
  text: string;
  streaming: boolean;
  /** 这一格第一次被看到的服务端记录时间；与用户气泡同款时间标记。 */
  atMs: number | null;
  /** 这一格在时间线里的标识；页脚与导出名仍按它归属。 */
  source: string;
  /**
   * 是否渲染页脚。
   *
   * 没有记录时间的那一格（按索引立起来的历史结论）页脚只剩一条恒定高度的空横条，
   * 而操作按钮本来就悬浮才出现，因此整块不渲染。
   */
  footer?: boolean;
}> = ({ text, streaming, atMs, source: _source, footer = true }) => {
  const [copied, setCopied] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState<string | null>(null);
  const [failure, setFailure] = React.useState<string | null>(null);
  const copy = () => {
    setFailure(null);
    // 复制走原生剪贴板：网页剪贴板要求窗口处于焦点，复制会时灵时不灵。
    void ipc.clipboardWriteText(text).then(
      () => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      },
      () => setFailure('复制失败：系统剪贴板不可用'),
    );
  };

  /**
   * 导出为 Markdown 文件。
   *
   * 另存为对话框由后端调起（界面没有文件系统权限），因此这里只发一次请求；
   * 对话框在途时按钮置灰，避免连点开出第二个对话框。用户取消返回 null，不是错误。
   * 文件名取正文标题，让用户在下载目录里一眼看出这份文档讲什么。
   */
  const save = () => {
    if (saving) return;
    setSaving(true);
    setFailure(null);
    void ipc
      .exportMarkdown(exportMarkdownName('assistant', text, new Date()), text)
      .then(
        (path) => {
          if (path === null) return;
          setSaved(path);
          window.setTimeout(() => setSaved(null), 2600);
        },
        (error: unknown) => setFailure(error instanceof Error ? error.message : '导出失败'),
      )
      .finally(() => setSaving(false));
  };

  return (
    <div className="group w-full">
      {/* 流式期间的“还在生成”提示由 Markdown 内部新增块的块级淡入承担，正文不再画光标。 */}
      <Markdown source={text} streaming={streaming} />
      {footer && !streaming && text.trim().length > 0 && (
        <MessageFooter
          atMs={atMs}
          status={
            saved !== null ? (
              <span className="ml-1 min-w-0 truncate text-meta text-text-tertiary confirm-pop">
                已导出到 {saved}
              </span>
            ) : failure !== null ? (
              <span className="ml-1 text-meta text-danger">{failure}</span>
            ) : undefined
          }
        >
          <MessageActionButton label="复制 Markdown" title="复制为 Markdown" onClick={copy}>
            {copied ? (
              <CheckIcon size={14} className="text-ok-ink confirm-pop" />
            ) : (
              <CopyIcon size={14} />
            )}
          </MessageActionButton>
          <MessageActionButton
            label="下载为 Markdown 文件"
            title="下载为 Markdown 文件"
            onClick={save}
            disabled={saving}
          >
            <DownloadIcon size={14} />
          </MessageActionButton>
        </MessageFooter>
      )}
    </div>
  );
};
