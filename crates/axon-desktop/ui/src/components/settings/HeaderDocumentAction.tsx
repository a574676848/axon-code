import React, { useCallback, useEffect, useState } from 'react';
import { AlertCircleIcon, FolderIcon, SettingsIcon } from '../../icons';
import type { DocumentItem, SettingsSection } from '../../types';
import * as ipc from '../../ipc';
import { Button } from '../ui/controls';
import { Text } from '../ui/surfaces';

/*
 * 分区级「打开」动作：排在分区标题行右端，只保留一个入口，目标随当前分区切换。
 * 配置类分区打开 config.toml，MCP 打开 .mcp.json，技能打开技能目录，
 * 人设与准则打开各自源文件。
 *
 * 打开入口只属于**配置真正落地的分区**：系统设置、关于与用量统计没有对应的
 * 配置文件，这些分区不渲染任何按钮，而不是退回去开 config.toml 凑一个入口。
 * 目标不存在时同样不渲染，打开中禁用，失败时在按钮左侧就地报错。
 */

/** 分区到打开目标的映射；没有可打开目标的分区取 null。 */
const SECTION_TARGET: Record<SettingsSection, string | null> = {
  general: 'config',
  models: 'config',
  balancer: 'config',
  remote: 'config',
  memory: 'config',
  mcp: 'mcp',
  skills: 'skills',
  soul: 'soul',
  agents: 'agents',
  usage: null,
  pricing: null,
  system: null,
  about: null,
};

interface HeaderDocumentActionProps {
  /** 当前分区。 */
  section: SettingsSection;
}

export const HeaderDocumentAction: React.FC<HeaderDocumentActionProps> = ({ section }) => {
  const targetId = SECTION_TARGET[section];
  const [entry, setEntry] = useState<DocumentItem | undefined>(undefined);
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (targetId === null) {
      setEntry(undefined);
      return;
    }
    try {
      const catalog = await ipc.documentCatalog();
      setEntry(catalog.find((item) => item.id === targetId));
    } catch {
      setEntry(undefined);
    }
  }, [targetId]);

  useEffect(() => {
    setError(null);
    void load();
  }, [load]);

  if (targetId === null || entry === undefined || !entry.exists) return null;

  const open = async () => {
    setOpening(true);
    setError(null);
    try {
      await ipc.openDocument(targetId);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '打开失败');
    } finally {
      setOpening(false);
    }
  };

  return (
    <div className="flex items-center gap-2">
      {error !== null && (
        <span className="flex items-center gap-1" role="alert">
          <AlertCircleIcon size={13} className="text-danger" />
          <Text tone="danger" size="xs" className="max-w-[180px] truncate" title={error}>
            {error}
          </Text>
        </span>
      )}
      <Button
        variant="outline"
        size="sm"
        disabled={opening}
        title={entry.path}
        onClick={() => void open()}
      >
        {entry.kind === 'dir' ? <FolderIcon size={13} /> : <SettingsIcon size={13} />}
        {entry.label}
      </Button>
    </div>
  );
};
