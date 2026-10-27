import React, { useState } from 'react';
import { AlertCircleIcon } from '../icons';
import * as ipc from '../ipc';
import type { MemoryKind, MemoryRecord, MemoryWriteInput } from '../types';
import {
  MEMORY_KINDS,
  draftEntriesFromJson,
  isWorkspaceKind,
  memoryKindLabel,
} from './memoryDraft';
import { Input, SegmentedControl, Textarea } from './ui/controls';
import { Dialog } from './ui/overlays';
import { Banner, FormField, Text } from './ui/surfaces';

const JSON_PLACEHOLDER = `[
  { "memoryKind": "knowledge", "summary": "摘要", "evidence": "证据引用", "workspacePath": "/work/repo" },
  { "memoryKind": "preference", "summary": "偏好摘要", "evidence": "证据引用" }
]`;

type EntryMode = 'form' | 'json';

const ENTRY_MODES: Array<{ value: EntryMode; label: string }> = [
  { value: 'form', label: '表单录入' },
  { value: 'json', label: 'JSON 录入' },
];

const KIND_OPTIONS = MEMORY_KINDS.map((item) => ({
  value: item.value,
  label: item.label,
}));

interface MemoryEditorProps {
  /** 编辑既有记录时传入；新建为 null。 */
  record: MemoryRecord | null;
  /** 新建时的默认类型，取自当前分栏。 */
  defaultKind: MemoryKind;
  onClose: () => void;
  onSaved: (needsRepair: boolean, count: number) => void;
}

/**
 * 记忆编辑器：与其余弹窗共用 Dialog 的几何（标题 14/20、动作 28），
 * 表单走 FormField 与 Textarea，录入方式与类型都是同一枚分段控件。
 */
export const MemoryEditor: React.FC<MemoryEditorProps> = ({
  record,
  defaultKind,
  onClose,
  onSaved,
}) => {
  const editing = record !== null;
  const [kind, setKind] = useState<MemoryKind>(record?.memoryKind ?? defaultKind);
  const [summary, setSummary] = useState(record?.memorySummary ?? '');
  const [evidence, setEvidence] = useState(record?.contentRef ?? '');
  const [workspacePath, setWorkspacePath] = useState('');
  const [mode, setMode] = useState<EntryMode>('form');
  const [jsonText, setJsonText] = useState('');
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [details, setDetails] = useState<string[]>([]);

  const needsFolder = isWorkspaceKind(kind);

  const submit = async () => {
    setError(null);
    setDetails([]);
    let entries: MemoryWriteInput[];
    if (mode === 'json') {
      const parsed = draftEntriesFromJson(jsonText);
      if (parsed.errors.length > 0) {
        setError('JSON 内容未通过校验');
        setDetails(parsed.errors);
        return;
      }
      entries = parsed.entries;
    } else {
      const problems: string[] = [];
      if (summary.trim().length === 0) problems.push('摘要不能为空');
      if (evidence.trim().length === 0) problems.push('证据引用不能为空');
      if (needsFolder && !editing && workspacePath.trim().length === 0) {
        problems.push('仓库记忆必须提供文件夹目录');
      }
      if (problems.length > 0) {
        setError('表单未填写完整');
        setDetails(problems);
        return;
      }
      entries = [
        {
          memoryKind: kind,
          summary: summary.trim(),
          evidence: evidence.trim(),
          workspacePath: needsFolder && !editing ? workspacePath.trim() : undefined,
          supersedesMemoryIds: editing && record !== null ? [record.memoryId] : undefined,
        },
      ];
    }
    setSaving(true);
    try {
      const receipt =
        entries.length === 1 ? await ipc.memoryWrite(entries[0]) : await ipc.memoryWriteBatch(entries);
      onSaved(receipt.needsRepair, receipt.records.length);
    } catch (failure) {
      setError('保存记忆失败');
      setDetails([failure instanceof Error ? failure.message : '未知错误']);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open
      onClose={onClose}
      title={editing ? '修改记忆' : '新增记忆'}
      size="lg"
      primary={{
        label: '保存',
        busyLabel: '保存中',
        busy: saving,
        onClick: () => void submit(),
      }}
      secondary={{ label: '取消', disabled: saving, onClick: onClose }}
    >
      {error !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={14} />}
          title={error}
          details={details}
        />
      )}

      {!editing && mode === 'form' ? (
        <div className="space-y-2 rounded-md bg-surface-sunken/45 hairline border-border-hairline p-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <Text tone="secondary" size="xs">
                记忆类型
              </Text>
              <SegmentedControl
                label="记忆类型"
                options={KIND_OPTIONS}
                value={kind}
                onChange={setKind}
              />
            </div>
            <SegmentedControl
              label="录入方式"
              options={ENTRY_MODES}
              value={mode}
              onChange={setMode}
            />
          </div>
          <Text tone="tertiary" size="xs" className="block">
            画像与偏好属于个人记忆；经验、知识、流程属于仓库记忆，需要文件夹目录。
          </Text>
        </div>
      ) : !editing ? (
        <div className="flex items-center justify-end">
          <SegmentedControl
            label="录入方式"
            options={ENTRY_MODES}
            value={mode}
            onChange={setMode}
          />
        </div>
      ) : null}

      {mode === 'form' ? (
        <>
          {editing && (
            <Text size="xs" tone="secondary" className="block">
              修改会写入一条新记录并标记替代关系，类型固定为「{memoryKindLabel(kind)}」；如需更换类型请新增一条。
            </Text>
          )}
          <FormField label="摘要">
            <Textarea
              value={summary}
              onChange={(event) => setSummary(event.target.value)}
              placeholder="简明提炼这条记忆的核心结论或规则"
              className="min-h-28"
              maxLength={2000}
              aria-label="记忆摘要"
            />
          </FormField>
          <FormField label="证据引用">
            <Textarea
              value={evidence}
              onChange={(event) => setEvidence(event.target.value)}
              placeholder="记录结论来源、文件路径、命令输出或上下文依据"
              className="min-h-22"
              maxLength={2000}
              aria-label="证据引用"
            />
          </FormField>
          {needsFolder && !editing && (
            <FormField label="文件夹目录（仓库记忆必填）">
              <Input
                value={workspacePath}
                onChange={(event) => setWorkspacePath(event.target.value)}
                placeholder="仓库目录的绝对路径"
                aria-label="文件夹目录"
              />
              <Text tone="secondary" size="xs" className="block">
                目录决定记忆归属的仓库；与 Agent 在同一目录写入的记忆落在同一个桶里。
              </Text>
            </FormField>
          )}
        </>
      ) : (
        <>
          <Text size="xs" tone="secondary" className="block">
            支持单条对象、对象数组或 records 包装体；字段为 memoryKind、summary、evidence、workspacePath。
            类型为经验、知识、流程时必须提供 workspacePath。
          </Text>
          <FormField label="记忆 JSON">
            <Textarea
              value={jsonText}
              onChange={(event) => setJsonText(event.target.value)}
              className="min-h-64 font-mono"
              placeholder={JSON_PLACEHOLDER}
              aria-label="记忆 JSON"
            />
          </FormField>
        </>
      )}
    </Dialog>
  );
};
