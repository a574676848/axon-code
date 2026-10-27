import React from 'react';
import { CheckIcon, CopyIcon, DownloadIcon } from '../../icons';
import { exportMarkdownName } from '../../conversation/textExport';
import { stop, useSpeechState } from '../../speech';
import { Markdown } from './Markdown';
import { MessageActionButton, MessageFooter } from './MessageFooter';
import { SpeechButton } from './SpeechButton';
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
 * 朗读同样读原文：净化在语音模块里做，正文展示不受影响。
 */

export const AssistantBlock: React.FC<{
  text: string;
  streaming: boolean;
  /** 这一格第一次被看到的服务端记录时间；与用户气泡同款时间标记。 */
  atMs: number | null;
  /** 这一格在时间线里的标识；朗读状态按它回认「正在念的是这一条」。 */
  source: string;
}> = ({ text, streaming, atMs, source }) => {
  const [copied, setCopied] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState<string | null>(null);
  const [failure, setFailure] = React.useState<string | null>(null);
  const speech = useSpeechState();
  const speaking = speech.source === source;
  /*
   * 朗读中或朗读失败时把操作区摊开：停止入口与「语音运行时未就绪」都必须看得见，
   * 不能等指针悬停才出现。
   */
  const revealing = speaking;

  /*
   * 这一格被删除或整段卸载时停止朗读：声音不该跟着一条已经不存在的消息走。
   * 只有当前正在念的那一格才有权停，别的消息卸载不能打断这一条。
   */
  const speakingRef = React.useRef(speaking);
  speakingRef.current = speaking;
  React.useEffect(
    () => () => {
      if (speakingRef.current) stop();
    },
    [],
  );

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
      {/*
        流式光标由 .streaming-caret 的 ::after 画在正文最后一段的末尾：
        作为兄弟节点插在 Markdown 后面会被块级流挤到下一行，像正文断了一行。
      */}
      <div className={streaming ? 'streaming-caret' : undefined}>
        <Markdown source={text} streaming={streaming} />
      </div>
      {!streaming && text.trim().length > 0 && (
        <MessageFooter
          atMs={atMs}
          revealActions={revealing}
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
          <SpeechButton text={text} source={source} />
        </MessageFooter>
      )}
    </div>
  );
};
