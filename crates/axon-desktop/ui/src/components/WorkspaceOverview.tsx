import React, { useCallback, useEffect, useMemo, useState } from 'react';
import * as ipc from '../ipc';
import type { WorkspaceItem } from '../types';
import { Button } from './ui/controls';
import { Banner, Chip, EmptyState, ListRow, Panel, Text } from './ui/surfaces';
import { PageShell } from './ui/shell';
import { AlertCircleIcon, FolderIcon, RefreshIcon } from '../icons';

/*
 * 工作区概览：读取真实工作区与会话统计。
 *
 * 与对话页共用同一份网关数据，但不参与订阅：它只回答「这台机器上登记了哪些工作区、
 * 各自有多少条会话」，因此每次进入都重新读一次，不维护自己的缓存副本。
 */
/** 工作区概览：读取真实工作区与会话统计。 */
export const WorkspaceOverview: React.FC = () => {
  const [workspaces, setWorkspaces] = useState<WorkspaceItem[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setWorkspaces(await ipc.fetchWorkspaces());
      setError(null);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : '读取工作区失败');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const lanes = useMemo(
    () => workspaces.flatMap((workspace) => workspace.lanes),
    [workspaces],
  );

  return (
    <PageShell
      title="工作区"
      description={`共 ${workspaces.length} 个工作区，${lanes.length} 条会话，其中置顶 ${lanes.filter((lane) => lane.pinned).length} 条，已归档 ${lanes.filter((lane) => lane.archived).length} 条`}
      actions={
        <Button variant="outline" size="md" onClick={() => void load()}>
          <RefreshIcon size={14} />
          刷新
        </Button>
      }
    >
      {error !== null && (
        <Banner
          tone="danger"
          icon={<AlertCircleIcon size={14} />}
          title={error}
          action={
            <Button variant="outline" size="sm" onClick={() => void load()}>
              重试
            </Button>
          }
        />
      )}

      {workspaces.length === 0 && error === null ? (
        <Panel clip>
          <EmptyState
            icon={<FolderIcon size={24} />}
            title="还没有工作区"
            description="用侧栏的「添加工作区」登记一个目录，这里会列出它的会话。"
          />
        </Panel>
      ) : (
        <Panel clip>
          {workspaces.map((workspace) => (
            <ListRow key={workspace.id}>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <Text size="row" strong truncate>
                    {workspace.name}
                  </Text>
                  {workspace.active && <Chip>当前</Chip>}
                </div>
                {/* 路径是机器名：等宽、11/16，长路径从左端裁掉的是目录，文件名始终看得见。 */}
                <span className="mt-0.5 block truncate font-mono text-[11px] leading-4 text-text-tertiary">
                  {workspace.path}
                </span>
              </div>
              <Text tone="tertiary" size="meta" className="shrink-0 leading-4 tabular-nums">
                {workspace.lanes.length} 条会话
              </Text>
            </ListRow>
          ))}
        </Panel>
      )}
    </PageShell>
  );
};
