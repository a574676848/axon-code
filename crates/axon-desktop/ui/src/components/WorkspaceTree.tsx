import React, { useMemo, useState } from 'react';
import { FolderIcon, IconNewChat } from '../icons';
import type { LaneItem, WorkspaceItem } from '../types';
import type { GroupMode } from './ViewOptionsMenu';
import { EmptyState, Text } from './ui/surfaces';
import { LaneRow, ROW_TEXT_INDENT, WorkspaceRow } from './SidebarRows';

/*
 * 工作区与会话列表：同一份真实数据按三种分组方式呈现。
 * - 按工作区：工作区作为同级分组，可折叠；
 * - 按工作区树：路径互为祖先的工作区嵌套显示，子级缩进；
 * - 单列表：不显示工作区分组，全部会话平铺成一张列表。
 * 一条车道就是一个真实 Axon 会话目录，标题与时间都来自磁盘真实数据。
 *
 * 行的几何与三态在 `SidebarRows` 里只有一份实现；这里只负责分组、拖拽与分页。
 */

/** 折叠态下每个工作区最多直接展示的车道条数。 */
const COLLAPSED_LANE_LIMIT = 5;

/** 去掉末尾分隔符，便于做路径前缀比较。 */
function trimSeparators(path: string): string {
  return path.replace(/[/\\]+$/, '');
}

/** 判断 candidate 是否为 ancestor 的后代目录。 */
function isDescendant(candidate: string, ancestor: string): boolean {
  const parent = trimSeparators(ancestor);
  const child = trimSeparators(candidate);
  if (parent.length === 0 || parent === child) return false;
  return child.startsWith(`${parent}/`) || child.startsWith(`${parent}\\`);
}

/** 读取工作区期间的骨架：与真实行同轴同高，探测完成后不会整块跳动。 */
const TreeSkeleton: React.FC<{ rows?: number }> = ({ rows = 4 }) => (
  <div role="status" aria-live="polite" className="space-y-0.5 pt-1 motion-safe:animate-pulse">
    <span className="sr-only">正在读取工作区</span>
    {Array.from({ length: rows }, (_, index) => (
      <div key={index} className="flex items-center gap-2 h-8" aria-hidden="true">
        <span className="shrink-0 w-6 h-6 flex items-center justify-center">
          <span className="w-3.5 h-3.5 rounded-xs bg-hover" />
        </span>
        <span
          className="block h-2.5 rounded-xs bg-hover"
          style={{ width: `${[68, 52, 76, 60][index % 4]}%` }}
        />
      </div>
    ))}
  </div>
);

interface WorkspaceTreeProps {
  /** 分组方式。 */
  groupBy: GroupMode;
  /** 真实工作区列表；单列表模式下只取其中的车道。 */
  workspaces: WorkspaceItem[];
  /** 当前选中车道标识。 */
  activeLaneId: string;
  /** 有新结果的车道：车道标识到终态；点开即由外层清除。 */
  laneMarks?: Record<string, string>;
  /** 展开状态表。 */
  expanded: Record<string, boolean>;
  /** 网关探测尚未完成：显示骨架而不是「没有工作区」。 */
  loading?: boolean;
  /** 空态的第二句：连接中断与「确实一个都没登记」要说不同的话。 */
  emptyDescription?: string;
  /** 折叠或展开工作区。 */
  onToggleWorkspace: (id: string) => void;
  /** 选中车道。 */
  onSelectLane: (workspaceId: string, laneId: string) => void;
  /** 切换置顶。 */
  onTogglePin: (lane: LaneItem) => void;
  /** 切换归档。 */
  onToggleArchive: (lane: LaneItem) => void;
  /** 重命名车道。 */
  onRenameLane: (laneId: string, title: string) => void;
  /** 保存重排后的完整车道顺序。 */
  onReorder: (workspaceId: string, orderedIds: string[]) => void;
  /** 在工作区内新建车道。 */
  onCreateLane: (workspaceId: string) => void;
  /** 移除已登记工作区。 */
  onRemoveWorkspace: (id: string) => void;
}

export const WorkspaceTree: React.FC<WorkspaceTreeProps> = ({
  groupBy,
  workspaces,
  activeLaneId,
  laneMarks,
  expanded,
  loading = false,
  emptyDescription = '用上方的「添加工作区」登记一个目录，这里会列出它的会话。',
  onToggleWorkspace,
  onSelectLane,
  onTogglePin,
  onToggleArchive,
  onRenameLane,
  onReorder,
  onCreateLane,
  onRemoveWorkspace,
}) => {
  const [expandedAll, setExpandedAll] = useState<Record<string, boolean>>({});
  /** 正在拖拽的车道及其所属工作区。 */
  const [drag, setDrag] = useState<{ workspaceId: string; laneId: string } | null>(null);
  /** 当前落点。 */
  const [dropAt, setDropAt] = useState<{ laneId: string; half: 'before' | 'after' } | null>(null);

  /** 把拖拽结果换算成该工作区的完整车道顺序并交回上层落盘。 */
  const applyDrop = (workspace: WorkspaceItem, targetLaneId: string, half: 'before' | 'after') => {
    if (drag === null || drag.workspaceId !== workspace.id) return;
    const ids = workspace.lanes.map((lane) => lane.id);
    const from = ids.indexOf(drag.laneId);
    if (from < 0) return;
    const without = ids.filter((id) => id !== drag.laneId);
    const targetAt = without.indexOf(targetLaneId);
    if (targetAt < 0) return;
    const insertAt = half === 'before' ? targetAt : targetAt + 1;
    const next = [...without.slice(0, insertAt), drag.laneId, ...without.slice(insertAt)];
    if (next.join('\u0000') !== ids.join('\u0000')) {
      onReorder(workspace.id, next);
    }
  };

  /** 树形分组的父子关系：父级取路径上最长的已登记祖先。 */
  const tree = useMemo(() => {
    if (groupBy !== 'workspaceTree') {
      return { children: new Map<string, WorkspaceItem[]>(), roots: workspaces };
    }
    const children = new Map<string, WorkspaceItem[]>();
    const roots: WorkspaceItem[] = [];
    for (const workspace of workspaces) {
      const parent = workspaces
        .filter((candidate) => isDescendant(workspace.path, candidate.path))
        .sort((left, right) => right.path.length - left.path.length)[0];
      if (parent === undefined) {
        roots.push(workspace);
        continue;
      }
      const bucket = children.get(parent.id) ?? [];
      bucket.push(workspace);
      children.set(parent.id, bucket);
    }
    return { children, roots };
  }, [groupBy, workspaces]);

  const lanesOf = (workspace: WorkspaceItem): LaneItem[] =>
    expandedAll[workspace.id] === true
      ? workspace.lanes
      : workspace.lanes.slice(0, COLLAPSED_LANE_LIMIT);

  const renderLanes = (workspace: WorkspaceItem) => {
    const lanes = lanesOf(workspace);
    const hiddenCount = workspace.lanes.length - lanes.length;
    const showsAll = expandedAll[workspace.id] === true;
    const canPaginate = workspace.lanes.length > COLLAPSED_LANE_LIMIT;

    return (
      <div className="space-y-0.5 pb-1">
        {lanes.length === 0 ? (
          <Text tone="tertiary" size="meta" className={`block ${ROW_TEXT_INDENT} py-1`}>
            暂无会话
          </Text>
        ) : (
          lanes.map((lane) => (
            <LaneRow
              key={lane.id}
              lane={lane}
              selected={lane.id === activeLaneId}
              activity={laneMarks?.[lane.id]}
              onSelect={() => onSelectLane(workspace.id, lane.id)}
              onTogglePin={() => onTogglePin(lane)}
              onToggleArchive={() => onToggleArchive(lane)}
              onRename={(title) => onRenameLane(lane.id, title)}
              onDragStart={() => setDrag({ workspaceId: workspace.id, laneId: lane.id })}
              onDragOver={(half) => setDropAt({ laneId: lane.id, half })}
              onDrop={() => {
                applyDrop(workspace, lane.id, dropAt?.half ?? 'before');
                setDrag(null);
                setDropAt(null);
              }}
              onDragEnd={() => {
                setDrag(null);
                setDropAt(null);
              }}
              dragging={drag?.laneId === lane.id}
              dropHint={
                dropAt?.laneId === lane.id && drag !== null && drag.laneId !== lane.id
                  ? dropAt.half
                  : 'none'
              }
            />
          ))
        )}

        {canPaginate && (hiddenCount > 0 || showsAll) && (
          <button
            type="button"
            onClick={(event) => {
              event.stopPropagation();
              setExpandedAll((previous) => ({
                ...previous,
                [workspace.id]: previous[workspace.id] !== true,
              }));
            }}
            className={`w-full py-1 text-left ${ROW_TEXT_INDENT}`}
          >
            <Text tone="tertiary" size="meta" className="hover:text-text-primary">
              {showsAll ? '收起会话' : `展开其余 ${hiddenCount} 个会话`}
            </Text>
          </button>
        )}
      </div>
    );
  };

  const renderWorkspace = (workspace: WorkspaceItem, depth: number): React.ReactNode => {
    const isExpanded = expanded[workspace.id] ?? false;
    const nested = tree.children.get(workspace.id) ?? [];
    return (
      <div key={workspace.id} className="space-y-0.5">
        <WorkspaceRow
          workspace={workspace}
          depth={depth}
          expanded={isExpanded}
          onToggle={() => onToggleWorkspace(workspace.id)}
          onCreateLane={() => onCreateLane(workspace.id)}
          onRemove={() => onRemoveWorkspace(workspace.id)}
        />
        {isExpanded && (
          <>
            {renderLanes(workspace)}
            {nested.map((child) => renderWorkspace(child, depth + 1))}
          </>
        )}
      </div>
    );
  };

  if (loading) {
    return <TreeSkeleton />;
  }

  if (workspaces.length === 0) {
    return (
      <EmptyState
        icon={<FolderIcon size={20} />}
        title="未检测到可用工作区"
        description={emptyDescription}
      />
    );
  }

  // 单列表：不出现工作区分组，全部会话平铺成一张列表。
  if (groupBy === 'flat') {
    const all = workspaces.flatMap((workspace) => workspace.lanes);
    if (all.length === 0) {
      return (
        <EmptyState
          icon={<IconNewChat size={20} />}
          title="还没有会话"
          description="在工作区下新建会话，这里会按最近更新列出它们。"
        />
      );
    }
    return (
      <div className="space-y-0.5">
        {all.map((lane) => {
          const owner = workspaces.find((workspace) =>
            workspace.lanes.some((entry) => entry.id === lane.id),
          );
          return (
            <LaneRow
              key={lane.id}
              lane={lane}
              selected={lane.id === activeLaneId}
              onSelect={() => onSelectLane(owner?.id ?? '', lane.id)}
              onTogglePin={() => onTogglePin(lane)}
              onToggleArchive={() => onToggleArchive(lane)}
              onRename={(title) => onRenameLane(lane.id, title)}
              onDragStart={() => setDrag(null)}
              onDragOver={() => undefined}
              onDrop={() => undefined}
              onDragEnd={() => undefined}
              dragging={false}
              dropHint="none"
            />
          );
        })}
      </div>
    );
  }

  return <div className="space-y-0.5">{tree.roots.map((item) => renderWorkspace(item, 0))}</div>;
};
