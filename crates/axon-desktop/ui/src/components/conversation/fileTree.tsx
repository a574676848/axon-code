import React from 'react';
import * as ipc from '../../ipc';
import type { FileEntryItem } from '../../types';
import { ChevronRightIcon } from '../../icons';
import { FileTypeIcon, FolderTypeIcon } from './fileIcons';
import { Text } from '../ui/surfaces';

/*
 * 工作区目录树：右侧栏「文件」页的正文。
 *
 * 只读，且后端已把范围钳死在工作区根目录内：任何越界路径都会被拒绝，
 * 这里不做二次拼路径，避免出现两套边界判断。子目录展开时才装载。
 */

/** 目录树中的一个节点。 */
const TreeRow: React.FC<{
  entry: FileEntryItem;
  depth: number;
  expanded: Record<string, boolean>;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  activePath: string | null;
}> = ({ entry, depth, expanded, onToggle, onOpen, activePath }) => {
  const isDir = entry.kind === 'dir';
  const open = expanded[entry.path] === true;
  const active = !isDir && entry.path === activePath;
  return (
    <button
      type="button"
      onClick={() => (isDir ? onToggle(entry.path) : onOpen(entry.path))}
      title={entry.path}
      style={{ paddingLeft: 8 + depth * 12 }}
      className={`w-full h-7 pr-2 flex items-center gap-1.5 text-left rounded-sm transition-colors duration-100 hover:bg-hover ${
        active ? 'bg-hover' : ''
      }`}
    >
      {isDir ? (
        <>
          <span
            className={`shrink-0 text-text-tertiary transition-transform duration-150 ${
              open ? 'rotate-90' : ''
            }`}
          >
            <ChevronRightIcon size={12} />
          </span>
          <FolderTypeIcon size={14} className="shrink-0" />
        </>
      ) : (
        <>
          <span className="shrink-0 w-[12px]" aria-hidden="true" />
          <FileTypeIcon path={entry.path} size={14} className="shrink-0" />
        </>
      )}
      <span className="flex-1 min-w-0 truncate text-row-loose text-text-primary">{entry.name}</span>
    </button>
  );
};

/** 目录子节点：展开时才装载。 */
const TreeChildren: React.FC<{
  root: string;
  path: string;
  depth: number;
  expanded: Record<string, boolean>;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  activePath: string | null;
}> = ({ root, path, depth, expanded, onToggle, onOpen, activePath }) => {
  const [entries, setEntries] = React.useState<FileEntryItem[] | null>(null);
  const [error, setError] = React.useState<string | null>(null);

  React.useEffect(() => {
    let cancelled = false;
    setEntries(null);
    setError(null);
    void ipc
      .listWorkspaceEntries(root, path)
      .then((value) => {
        if (!cancelled) setEntries(value);
      })
      .catch((failure: unknown) => {
        if (!cancelled) setError(failure instanceof Error ? failure.message : '读取目录失败');
      });
    return () => {
      cancelled = true;
    };
  }, [root, path]);

  if (error !== null) {
    return (
      <div style={{ paddingLeft: 8 + depth * 12 }} className="py-1">
        <Text tone="danger" size="xs">
          {error}
        </Text>
      </div>
    );
  }
  if (entries === null) {
    return (
      <div style={{ paddingLeft: 8 + depth * 12 }} className="py-1">
        <Text tone="tertiary" size="xs">
          读取中
        </Text>
      </div>
    );
  }
  if (entries.length === 0) {
    return (
      <div style={{ paddingLeft: 8 + depth * 12 }} className="py-1">
        <Text tone="tertiary" size="xs">
          空目录
        </Text>
      </div>
    );
  }
  return (
    <>
      {entries.map((entry) => (
        <React.Fragment key={entry.path}>
          <TreeRow
            entry={entry}
            depth={depth}
            expanded={expanded}
            onToggle={onToggle}
            onOpen={onOpen}
            activePath={activePath}
          />
          {entry.kind === 'dir' && expanded[entry.path] === true && (
            <TreeChildren
              root={root}
              path={entry.path}
              depth={depth + 1}
              expanded={expanded}
              onToggle={onToggle}
              onOpen={onOpen}
              activePath={activePath}
            />
          )}
        </React.Fragment>
      ))}
    </>
  );
};

export interface FileTreeProps {
  /** 工作区根目录绝对路径。 */
  root: string;
  /** 当前已打开的文件相对路径，用于高亮所在行。 */
  activePath: string | null;
  /** 打开一个文件。 */
  onOpenFile: (path: string) => void;
}

export const FileTree: React.FC<FileTreeProps> = ({ root, activePath, onOpenFile }) => {
  const [expanded, setExpanded] = React.useState<Record<string, boolean>>({});

  const toggle = React.useCallback((path: string) => {
    setExpanded((current) => ({ ...current, [path]: current[path] !== true }));
  }, []);

  return (
    <div className="min-h-0 flex-1 flex flex-col">
      <div className="shrink-0 h-9 px-3 flex items-center gap-2">
        <Text tone="tertiary" size="xs" className="flex-1 min-w-0 truncate" mono>
          {root}
        </Text>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto scroll-visible pb-2">
        <TreeChildren
          root={root}
          path=""
          depth={0}
          expanded={expanded}
          onToggle={toggle}
          onOpen={onOpenFile}
          activePath={activePath}
        />
      </div>
    </div>
  );
};
