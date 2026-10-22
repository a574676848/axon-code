import React from 'react';
import { classifyDroppedItems } from '../../conversation/dropFiles';
import {
  base64FromText,
  exportMarkdownName,
  isLargeText,
} from '../../conversation/textExport';
import type { AttachmentRef } from '../../conversation/transcript';
import * as ipc from '../../ipc';
import { NOTICE_FAILURE_MS, NOTICE_MS } from './composerNotice';

/*
 * 输入坞的附件：粘贴落盘、拖放落盘、系统选择器登记，以及这三条路径的提示。
 *
 * 三条路径最终都收敛成同一件事：磁盘上的真实文件加绝对路径。后端把附件投影为
 * Provider turn 的输入素材（图片走 vision，文件走可读附件），因此这里不改写内容，
 * 也不替用户猜他打算怎么用这个文件。
 */

/** 提示的语气：过程说明用 info，失败用 danger。 */
export type ComposerNoticeTone = 'info' | 'danger';

export interface ComposerNotice {
  tone: ComposerNoticeTone;
  text: string;
}

export interface ComposerAttachmentsController {
  attachments: AttachmentRef[];
  /** 当前提示；为空时不渲染。 */
  notice: ComposerNotice | null;
  /** 拖拽正悬停在输入区上方，用于给落点加视觉反馈。 */
  dragging: boolean;
  notify: (text: string, tone?: ComposerNoticeTone) => void;
  /** 粘贴接管：剪贴板里有文件就落文件，是大段文本就落 Markdown；返回是否已接管。 */
  handlePaste: (event: React.ClipboardEvent<HTMLTextAreaElement>) => boolean;
  /** 系统文件选择器登记附件。 */
  pick: () => void;
  remove: (path: string) => void;
  /** 提交之后清空。 */
  clear: () => void;
  /** 把一段大文本落成 Markdown 附件。 */
  stageLargeText: (text: string) => void;
  dropHandlers: {
    onDragOver: (event: React.DragEvent) => void;
    onDragLeave: (event: React.DragEvent) => void;
    onDrop: (event: React.DragEvent) => void;
  };
}

/** 附件控制器：三条落地路径共用一个提示位，停留时长见 composerNotice。 */
export function useComposerAttachments(): ComposerAttachmentsController {
  const [attachments, setAttachments] = React.useState<AttachmentRef[]>([]);
  const [notice, setNotice] = React.useState<ComposerNotice | null>(null);
  const [dragging, setDragging] = React.useState(false);
  const timer = React.useRef<number | null>(null);

  const notify = React.useCallback((text: string, tone: ComposerNoticeTone = 'info') => {
    setNotice({ tone, text });
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setNotice(null), tone === 'danger' ? NOTICE_FAILURE_MS : NOTICE_MS);
  }, []);

  React.useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  /** 读一个文件并落地；读不到时给出带文件名的原因，不吞掉失败。 */
  const stageFile = React.useCallback(
    (file: File): Promise<AttachmentRef> =>
      new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onerror = () => reject(new Error(`读取 ${file.name} 失败`));
        reader.onload = () => {
          const result = String(reader.result ?? '');
          const payload = result.includes(',') ? result.slice(result.indexOf(',') + 1) : result;
          void ipc
            .attachmentStage(file.name.length > 0 ? file.name : 'attachment', payload)
            .then(resolve, reject);
        };
        reader.readAsDataURL(file);
      }),
    [],
  );

  const addFiles = React.useCallback(
    (files: File[]) => {
      if (files.length === 0) return;
      void Promise.all(files.map(stageFile)).then(
        (staged) => {
          setAttachments((current) => [...current, ...staged]);
          notify(`已添加 ${staged.length} 个附件`);
        },
        (error: unknown) =>
          notify(error instanceof Error ? error.message : '附件落地失败', 'danger'),
      );
    },
    [notify, stageFile],
  );

  const stageLargeText = React.useCallback(
    (text: string) => {
      // 文件名取正文标题：用户在附件条与磁盘上都一眼看出这份文档讲什么。
      const name = exportMarkdownName('pasted', text, new Date());
      void ipc.attachmentStage(name, base64FromText(text)).then(
        (attachment) => {
          setAttachments((current) => [...current, attachment]);
          notify(`已把 ${text.length} 字符的文本存为 ${name}，随消息作为附件发送`);
        },
        (error: unknown) =>
          notify(error instanceof Error ? error.message : '大段文本落地失败', 'danger'),
      );
    },
    [notify],
  );

  const pick = React.useCallback(() => {
    void ipc.attachmentPick().then(
      (staged) => setAttachments((current) => [...current, ...staged]),
      (error: unknown) =>
        notify(error instanceof Error ? error.message : '选择附件失败', 'danger'),
    );
  }, [notify]);

  const remove = React.useCallback((path: string) => {
    setAttachments((current) => current.filter((item) => item.path !== path));
  }, []);

  const clear = React.useCallback(() => setAttachments([]), []);

  const handlePaste = React.useCallback(
    (event: React.ClipboardEvent<HTMLTextAreaElement>): boolean => {
      const files = Array.from(event.clipboardData?.files ?? []);
      if (files.length > 0) {
        event.preventDefault();
        addFiles(files);
        return true;
      }
      const text = event.clipboardData?.getData('text') ?? '';
      if (isLargeText(text)) {
        event.preventDefault();
        stageLargeText(text);
        return true;
      }
      return false;
    },
    [addFiles, stageLargeText],
  );

  const dropHandlers = React.useMemo(
    () => ({
      onDragOver: (event: React.DragEvent) => {
        // 不阻止默认行为，浏览器就不把这里当可落点，drop 根本不会派发。
        event.preventDefault();
        setDragging(true);
      },
      onDragLeave: (event: React.DragEvent) => {
        // 在子元素之间移动同样会派发 dragleave，进到子元素里不算离开落点。
        const next = event.relatedTarget;
        if (next instanceof Node && event.currentTarget.contains(next)) return;
        setDragging(false);
      },
      onDrop: (event: React.DragEvent) => {
        event.preventDefault();
        setDragging(false);
        const { files, directories } = classifyDroppedItems(event.dataTransfer);
        if (directories.length > 0) {
          notify(`暂不支持拖入文件夹：${directories.join('、')}`, 'danger');
        }
        addFiles(files);
      },
    }),
    [addFiles, notify],
  );

  return {
    attachments,
    notice,
    dragging,
    notify,
    handlePaste,
    pick,
    remove,
    clear,
    stageLargeText,
    dropHandlers,
  };
}
