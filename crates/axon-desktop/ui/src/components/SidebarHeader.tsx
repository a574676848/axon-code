import React from 'react';
import {
  AxonLogo,
  FolderIcon,
  IconPanelLeft,
  ListIcon,
  SearchIcon,
  ViewGroupTreeIcon,
  WorkspaceAddIcon,
} from '../icons';
import { IconButton, Input } from './ui/controls';
import { Text } from './ui/surfaces';
import { ROW_ICON_COLUMN } from './SidebarRows';
import { ViewOptionsMenu, groupModeTitle } from './ViewOptionsMenu';
import type { ArchivedFilter, GroupMode, OrderMode } from './ViewOptionsMenu';

/*
 * 侧边栏头部组件：40px 沉浸式品牌拖拽行与工作区分组搜索条。
 *
 * 从 Sidebar.tsx 拆出以保持单一职责并严控单文件行数上限。
 */

export interface SidebarBrandBarProps {
  onToggleSidebar: () => void;
}

/** 顶部品牌行：40px 高，与主区指标条处于同一水平轴线；macOS 下自动避让左上角交通灯。 */
export const SidebarBrandBar: React.FC<SidebarBrandBarProps> = ({ onToggleSidebar }) => (
  <div
    data-tauri-drag-region
    style={{ paddingLeft: 'calc(12px + var(--macos-traffic-inset, 0px))' }}
    className="h-10 pt-1.5 shrink-0 pr-3 flex items-center justify-between select-none"
  >
    <div className="flex items-center gap-2 min-w-0">
      <span className={ROW_ICON_COLUMN}>
        <AxonLogo size={20} aria-hidden="true" className="rounded-xs" />
      </span>
      <span className="text-[14px] leading-5 font-semibold tracking-[-0.015em] text-text-primary truncate">
        Axon
      </span>
    </div>
    <IconButton label="收起侧边栏" onClick={onToggleSidebar}>
      <IconPanelLeft size={14} />
    </IconButton>
  </div>
);

export interface SidebarSectionHeaderProps {
  groupBy: GroupMode;
  orderBy: OrderMode;
  archivedFilter: ArchivedFilter;
  searching: boolean;
  query: string;
  searchInputRef: React.RefObject<HTMLInputElement | null>;
  searchToggleRef: React.RefObject<HTMLDivElement | null>;
  onStartSearch: () => void;
  onStopSearch: () => void;
  onQueryChange: (value: string) => void;
  onGroupPick: (mode: GroupMode) => void;
  onOrderPick: (mode: OrderMode) => void;
  onArchivedPick: (filter: ArchivedFilter) => void;
  onAddWorkspace: () => void;
}

/** 工作区分组头与原位展开搜索框。 */
export const SidebarSectionHeader: React.FC<SidebarSectionHeaderProps> = ({
  groupBy,
  orderBy,
  archivedFilter,
  searching,
  query,
  searchInputRef,
  searchToggleRef,
  onStartSearch,
  onStopSearch,
  onQueryChange,
  onGroupPick,
  onOrderPick,
  onArchivedPick,
  onAddWorkspace,
}) => (
  <div className="shrink-0 px-3">
    <div className="relative h-8">
      <div className={`h-8 flex items-center justify-between ${searching ? 'invisible' : ''}`}>
        <div className="flex items-center gap-2 min-w-0">
          <span className={`${ROW_ICON_COLUMN} text-text-tertiary`}>
            {groupBy === 'flat' ? (
              <ListIcon size={14} />
            ) : groupBy === 'workspaceTree' ? (
              <ViewGroupTreeIcon size={14} />
            ) : (
              <FolderIcon size={14} />
            )}
          </span>
          <Text size="meta" strong tone="tertiary" truncate className="leading-4">
            {groupModeTitle(groupBy)}
          </Text>
        </div>
        <div className="flex items-center gap-0.5" ref={searchToggleRef}>
          <IconButton label="搜索会话" onClick={onStartSearch}>
            <SearchIcon size={14} />
          </IconButton>

          <ViewOptionsMenu
            groupBy={groupBy}
            orderBy={orderBy}
            archivedFilter={archivedFilter}
            onGroupPick={onGroupPick}
            onOrderPick={onOrderPick}
            onArchivedPick={onArchivedPick}
          />

          <IconButton label="添加工作区" onClick={onAddWorkspace}>
            <WorkspaceAddIcon size={14} />
          </IconButton>
        </div>
      </div>

      {searching && (
        <div className="absolute inset-0 flex items-center">
          <Input
            ref={searchInputRef}
            icon={<SearchIcon size={14} />}
            value={query}
            placeholder="搜索会话名称"
            onChange={(event) => onQueryChange(event.target.value)}
            onClear={() => onQueryChange('')}
            onKeyDown={(event) => {
              if (event.key === 'Escape') {
                onStopSearch();
              }
            }}
          />
        </div>
      )}
    </div>
  </div>
);
