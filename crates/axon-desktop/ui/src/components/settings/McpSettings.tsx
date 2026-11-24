import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { AlertCircleIcon, CheckIcon, EditIcon, PlusIcon, TrashIcon } from '../../icons';
import type { McpServerInput, McpServerItem } from '../../types';
import * as ipc from '../../ipc';
import { Button, Input, SegmentedControl, Textarea } from '../ui/controls';
import { Dialog, Select } from '../ui/overlays';
import { Banner, Chip, FormField } from '../ui/surfaces';
import { CardDetailDialog, CardList, EntityCard } from '../ui/cards';
import { SettingsHeaderAction } from './SettingsHeaderSlot';

/*
 * MCP 服务器管理：以卡片列表呈现，支持搜索、查看详情与完整增删改。
 *
 * 两种配置方式并存：表单适合逐字段修改；完整 JSON 适合直接粘贴原始配置，
 * 因此两种方式都落到同一份 .mcp.json，不各自维护一份状态。
 *
 * 密钥边界：界面只显示环境变量名；表单输入框留空表示沿用原值，
 * 因此改别的字段不会把已配好的密钥冲掉，也不会把旧值回显到界面上。
 */

/** 传输类型候选。 */
const KIND_OPTIONS = [
  { value: 'stdio', label: 'stdio 本地进程' },
  { value: 'http', label: 'http 远端端点' },
];

/** 配置方式：表单逐字段改，完整 JSON 直接粘贴原始配置。 */
const MODE_OPTIONS = [
  { value: 'form', label: '表单配置' },
  { value: 'json', label: '完整 JSON' },
] as const;

/** 新服务器的初始值。 */
const EMPTY_SERVER: McpServerInput = {
  name: '',
  kind: 'stdio',
  command: '',
  args: [],
  url: '',
  env: {},
};

/** 把一条服务器配置渲染成完整 JSON，供 JSON 方式编辑。 */
function toEntryJson(server: McpServerItem | null): string {
  if (server === null) {
    return '{\n  "type": "stdio",\n  "command": "",\n  "args": [],\n  "env": {}\n}';
  }
  const entry: Record<string, unknown> = { type: server.kind };
  if (server.kind === 'http') {
    entry.url = server.url;
  } else {
    entry.command = server.command;
    if (server.args.length > 0) entry.args = server.args;
  }
  if (server.env_keys.length > 0) {
    const env: Record<string, string> = {};
    for (const key of server.env_keys) env[key] = '';
    entry.env = env;
  }
  return JSON.stringify(entry, null, 2);
}

/** 服务器的一句话说明，只讲形态，不重复底部的地址或命令。 */
function describe(server: McpServerItem): string {
  return server.kind === 'http'
    ? '远端 HTTP 端点，Axon 通过该地址调用工具。'
    : '本地进程，由 Axon 按需启动并通过标准输入输出通信。';
}

/** 卡片底部展示的命令行或地址，整张卡片只出现这一次。 */
function locatorOf(server: McpServerItem): string {
  return server.kind === 'http' ? server.url : [server.command, ...server.args].join(' ');
}

interface McpFormProps {
  /** 正在编辑的服务器；为空表示新增。 */
  editing: McpServerItem | null;
  /** 是否打开。 */
  open: boolean;
  /** 关闭。 */
  onClose: () => void;
  /** 保存完成。 */
  onSaved: () => void;
}

const McpForm: React.FC<McpFormProps> = ({ editing, open, onClose, onSaved }) => {
  const [draft, setDraft] = useState<McpServerInput>(EMPTY_SERVER);
  const [argsText, setArgsText] = useState('');
  const [envText, setEnvText] = useState('');
  const [rawJson, setRawJson] = useState('');
  const [mode, setMode] = useState<'form' | 'json'>('form');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setError(null);
    setBusy(false);
    setMode('form');
    setRawJson(toEntryJson(editing));
    if (editing === null) {
      setDraft(EMPTY_SERVER);
      setArgsText('');
      setEnvText('');
      return;
    }
    setDraft({
      name: editing.name,
      kind: editing.kind,
      command: editing.command,
      args: editing.args,
      url: editing.url,
      env: {},
    });
    setArgsText(editing.args.join('\n'));
    // 只回显变量名，值留空表示沿用原值。
    setEnvText(editing.env_keys.join('\n'));
  }, [open, editing]);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      if (mode === 'json') {
        await ipc.mcpUpsertRaw(draft.name.trim(), rawJson);
        onSaved();
        return;
      }
      const env: Record<string, string> = {};
      for (const line of envText.split('\n')) {
        const trimmed = line.trim();
        if (trimmed.length === 0) continue;
        const at = trimmed.indexOf('=');
        if (at < 0) {
          env[trimmed] = '';
          continue;
        }
        env[trimmed.slice(0, at).trim()] = trimmed.slice(at + 1).trim();
      }
      await ipc.mcpUpsert(
        {
          ...draft,
          args: argsText
            .split('\n')
            .map((line) => line.trim())
            .filter((line) => line.length > 0),
          env,
        },
        editing?.name,
      );
      onSaved();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '保存 MCP 服务器失败');
    } finally {
      setBusy(false);
    }
  };

  /** 把 JSON 文本重新缩进，便于核对。 */
  const formatJson = () => {
    try {
      setRawJson(JSON.stringify(JSON.parse(rawJson), null, 2));
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : 'JSON 格式不正确');
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={editing === null ? '添加 MCP 服务器' : `编辑 MCP 服务器 ${editing.name}`}
      size="md"
      primary={{
        label: '保存',
        busyLabel: '保存中',
        icon: <CheckIcon size={13} />,
        busy,
        onClick: () => void submit(),
      }}
      secondary={{
        label: '格式化',
        disabled: mode !== 'json',
        onClick: formatJson,
      }}
    >
      <FormField label="服务器名称">
        <Input
          value={draft.name}
          placeholder="唯一名称"
          onChange={(event) => setDraft({ ...draft, name: event.target.value })}
        />
      </FormField>

      <SegmentedControl
        label="配置方式"
        value={mode}
        options={MODE_OPTIONS}
        onChange={setMode}
      />

      {mode === 'json' ? (
        <FormField label="完整的 JSON 配置">
          <Textarea
            value={rawJson}
            onChange={(event) => setRawJson(event.target.value)}
            rows={12}
            spellCheck={false}
            className="font-mono"
          />
        </FormField>
      ) : (
        <>
          <FormField label="传输类型">
            <Select
              label="传输类型"
              value={draft.kind}
              options={KIND_OPTIONS}
              onChange={(value) => setDraft({ ...draft, kind: value })}
            />
          </FormField>

          {draft.kind === 'http' ? (
            <FormField label="端点地址">
              <Input
                value={draft.url}
                placeholder="http://127.0.0.1:8319"
                onChange={(event) => setDraft({ ...draft, url: event.target.value })}
              />
            </FormField>
          ) : (
            <>
              <FormField label="启动命令">
                <Input
                  value={draft.command}
                  placeholder="npx"
                  onChange={(event) => setDraft({ ...draft, command: event.target.value })}
                />
              </FormField>
              <FormField label="启动参数（每行一个）">
                <Textarea
                  value={argsText}
                  onChange={(event) => setArgsText(event.target.value)}
                  rows={3}
                  className="font-mono"
                />
              </FormField>
            </>
          )}

          <FormField label="环境变量（每行 KEY=VALUE，值留空表示沿用原值）">
            <Textarea
              value={envText}
              onChange={(event) => setEnvText(event.target.value)}
              rows={3}
              placeholder={'MORPH_API_KEY=\nCBM_CACHE_DIR=<path/to/cache>'}
              className="font-mono"
            />
          </FormField>
        </>
      )}

      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}
    </Dialog>
  );
};

export const McpSettings: React.FC = () => {
  const [servers, setServers] = useState<McpServerItem[]>([]);
  const [query, setQuery] = useState('');
  const [detail, setDetail] = useState<McpServerItem | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<McpServerItem | null>(null);
  const [busyName, setBusyName] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setServers(await ipc.mcpList());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取 MCP 配置失败');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const visible = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (keyword.length === 0) return servers;
    return servers.filter(
      (server) =>
        server.name.toLowerCase().includes(keyword) ||
        server.kind.toLowerCase().includes(keyword) ||
        server.command.toLowerCase().includes(keyword) ||
        server.url.toLowerCase().includes(keyword) ||
        server.env_keys.some((key) => key.toLowerCase().includes(keyword)),
    );
  }, [servers, query]);

  const remove = async (server: McpServerItem) => {
    setBusyName(server.name);
    try {
      await ipc.mcpRemove(server.name);
      setDetail(null);
      await load();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '移除 MCP 服务器失败');
    } finally {
      setBusyName(null);
    }
  };

  return (
    <div className="space-y-3">
      <SettingsHeaderAction>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            setEditing(null);
            setFormOpen(true);
          }}
        >
          <PlusIcon size={13} />
          添加服务器
        </Button>
      </SettingsHeaderAction>

      {error !== null && <Banner tone="danger" icon={<AlertCircleIcon size={13} />} title={error} />}

      <CardList
        query={query}
        onQueryChange={setQuery}
        placeholder="搜索服务器名称、命令或环境变量"
        summary={`共 ${visible.length} 台服务器`}
        loading={loading}
        emptyTitle={query.trim().length > 0 ? '没有匹配的服务器' : '尚未配置任何 MCP 服务器'}
        emptyDescription={
          query.trim().length > 0
            ? '换一个关键词，或清空搜索框查看全部'
            : '点右上角「添加服务器」配置一台本地进程或远端端点'
        }
      >
        {visible.map((server) => (
          <EntityCard
            key={server.name}
            title={server.name}
            badges={<Chip>{server.kind === 'http' ? 'HTTP' : 'STDIO'}</Chip>}
            description={describe(server)}
            locator={locatorOf(server)}
            meta={
              server.env_keys.length > 0
                ? `${server.env_keys.length} 个环境变量`
                : '无环境变量'
            }
            onOpen={() => setDetail(server)}
            onInspect={() => setDetail(server)}
          />
        ))}
      </CardList>

      {/* 详情：完整字段一览，含环境变量名清单（不含值） */}
      <CardDetailDialog
        open={detail !== null}
        onClose={() => setDetail(null)}
        title={detail?.name ?? '服务器详情'}
        fields={[
          { label: '传输类型', value: detail?.kind === 'http' ? 'HTTP 远端端点' : 'STDIO 本地进程' },
          detail?.kind === 'http'
            ? { label: '端点地址', value: detail?.url ?? '', mono: true }
            : { label: '启动命令', value: detail?.command ?? '', mono: true },
          {
            label: '启动参数',
            value: (detail?.args ?? []).join(' '),
            mono: true,
          },
          { label: '环境变量名（值不回显）', value: (detail?.env_keys ?? []).join('\n'), kind: 'chips' },
        ]}
        secondary={{
          label: '移除',
          icon: <TrashIcon size={13} />,
          danger: true,
          busy: busyName === detail?.name,
          onClick: () => {
            if (detail !== null) void remove(detail);
          },
        }}
        primary={{
          label: '编辑',
          icon: <EditIcon size={13} />,
          onClick: () => {
            setEditing(detail);
            setDetail(null);
            setFormOpen(true);
          },
        }}
      />

      <McpForm
        editing={editing}
        open={formOpen}
        onClose={() => setFormOpen(false)}
        onSaved={() => {
          setFormOpen(false);
          void load();
        }}
      />
    </div>
  );
};
