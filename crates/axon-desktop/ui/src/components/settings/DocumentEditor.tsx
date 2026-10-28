import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, CheckIcon } from '../../icons';
import * as ipc from '../../ipc';
import { Button, Textarea } from '../ui/controls';
import { Banner, Panel, Text } from '../ui/surfaces';
import { SettingsHeaderAction, SettingsHeaderStatus } from './SettingsHeaderSlot';

/*
 * 人设与行为准则：直接编辑源文件并原子保存。
 * 两个文档共用同一套读写交互，只按标识区分，避免重复实现。
 *
 * 保存是这一屏唯一的主动作，放在分区标题行右端；已保存状态是说明，
 * 放在说明行右端。两者不再挤成正文里的一枚黑色药丸。
 */

interface DocumentEditorProps {
  /** 文档标识：soul 或 agents。 */
  documentId: string;
  /** 编辑区占位文案。 */
  placeholder: string;
}

export const DocumentEditor: React.FC<DocumentEditorProps> = ({
  documentId,
  placeholder,
}) => {
  const [content, setContent] = useState('');
  const [saved, setSaved] = useState('');
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const document = await ipc.documentRead(documentId);
      setContent(document.content);
      setSaved(document.content);
      setPath(document.path);
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取文档失败');
    }
  }, [documentId]);

  useEffect(() => {
    void load();
  }, [load]);

  const dirty = content !== saved;
  /** 行数：空内容按一行计。 */
  const lineCount = content.length === 0 ? 1 : content.split('\n').length;

  const save = async () => {
    setBusy(true);
    try {
      await ipc.documentWrite(documentId, content);
      setSaved(content);
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '写入文档失败');
    } finally {
      setBusy(false);
    }
  };

  return (
    /*
     * 整屏铺满：这一屏只有一件东西要写，编辑框因此吃满余下的高度。
     * 根节点是 `flex-1 min-h-0` 的纵向 flex（父级本身就是 flex 容器），
     * 面板与文本框各自再 `flex-1 min-h-0`；父级没有 flex 上下文时退回自然高度。
     */
    <div className="flex flex-1 min-h-0 flex-col gap-3">
      <SettingsHeaderAction>
        <Button
          variant="primary"
          size="sm"
          disabled={busy || !dirty}
          onClick={() => void save()}
        >
          <CheckIcon size={13} />
          {busy ? '写入中' : '保存修改'}
        </Button>
      </SettingsHeaderAction>

      {/*
        只在有未保存内容时给一句提示。保存成功不再回一句「已保存」：
        按钮自身会因为 dirty 变空而禁用，那已经是「没有待保存」的证据，
        再挂一个状态标签反而在标题行右下角多出一块要读的字。
      */}
      <SettingsHeaderStatus>
        {dirty ? (
          <Text tone="tertiary" size="xs">
            有未保存的修改
          </Text>
        ) : null}
      </SettingsHeaderStatus>

      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}

      <Panel className="p-2 flex-1 min-h-0 flex flex-col">
        <Textarea
          value={content}
          rows={18}
          spellCheck={false}
          placeholder={placeholder}
          onChange={(event) => setContent(event.target.value)}
          className="flex-1 min-h-[320px] h-full resize-none scroll-visible"
        />
      </Panel>

      {/* 计数与源文件单独一行，不覆盖正文 */}
      <div className="flex items-center justify-between gap-3">
        <Text tone="tertiary" size="2xs" className="shrink-0 tabular-nums">
          {content.length} 字符 · {lineCount} 行
        </Text>
        <Text
          tone="tertiary"
          size="2xs"
          mono
          truncate
          className="min-w-0 flex-1 text-right"
          title={path}
        >
          {path}
        </Text>
      </div>
    </div>
  );
};
