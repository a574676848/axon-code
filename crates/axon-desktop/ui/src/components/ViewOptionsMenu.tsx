import React from 'react';
import {
  SlidersIcon,
  ViewFilterAllIcon,
  ViewFilterHiddenIcon,
  ViewFilterOnlyIcon,
  ViewGroupFlatIcon,
  ViewGroupTreeIcon,
  ViewGroupWorkspaceIcon,
  ViewOrderManualIcon,
  ViewOrderUpdatedIcon,
} from '../icons';
import { IconButton } from './ui/controls';
import { Dropdown, MenuItem, MenuLabel, MenuSeparator } from './ui/overlays';

/*
 * 视图选项菜单：分组方式、排序方式、筛选会话三组显式单选。
 * 三组各带一个分组标题，组间以分隔线断开，每个条目都带图标，
 * 当前生效项在条目右侧显示对勾。
 */

/** 分组方式。 */
export type GroupMode = 'workspace' | 'workspaceTree' | 'flat';

/** 排序方式。 */
export type OrderMode = 'manual' | 'updated';

/** 归档筛选。 */
export type ArchivedFilter = 'hide' | 'show' | 'only';

/** 侧栏标题文案随分组方式变化。 */
export function groupModeTitle(mode: GroupMode): string {
  return mode === 'flat' ? '会话' : '工作区';
}

interface ViewOptionsMenuProps {
  /** 当前分组方式。 */
  groupBy: GroupMode;
  /** 当前排序方式。 */
  orderBy: OrderMode;
  /** 当前归档筛选。 */
  archivedFilter: ArchivedFilter;
  /** 分组方式变化。 */
  onGroupPick: (mode: GroupMode) => void;
  /** 排序方式变化。 */
  onOrderPick: (mode: OrderMode) => void;
  /** 归档筛选变化。 */
  onArchivedPick: (filter: ArchivedFilter) => void;
}

export const ViewOptionsMenu: React.FC<ViewOptionsMenuProps> = ({
  groupBy,
  orderBy,
  archivedFilter,
  onGroupPick,
  onOrderPick,
  onArchivedPick,
}) => (
  <Dropdown
    align="end"
    menuClassName="min-w-[220px]"
    trigger={({ toggle, open }) => (
      <IconButton
        label="视图选项"
        className={open ? 'bg-hover text-text-primary' : ''}
        onClick={toggle}
      >
        <SlidersIcon size={14} />
      </IconButton>
    )}
  >
    <MenuLabel>分组方式</MenuLabel>
    <MenuItem
      selected={groupBy === 'workspace'}
      icon={<ViewGroupWorkspaceIcon size={14} />}
      onSelect={() => onGroupPick('workspace')}
    >
      按工作区
    </MenuItem>
    <MenuItem
      selected={groupBy === 'workspaceTree'}
      icon={<ViewGroupTreeIcon size={14} />}
      onSelect={() => onGroupPick('workspaceTree')}
    >
      按工作区树
    </MenuItem>
    <MenuItem
      selected={groupBy === 'flat'}
      icon={<ViewGroupFlatIcon size={14} />}
      onSelect={() => onGroupPick('flat')}
    >
      单列表
    </MenuItem>

    <MenuSeparator />
    <MenuLabel>排序方式</MenuLabel>
    <MenuItem
      selected={orderBy === 'manual'}
      icon={<ViewOrderManualIcon size={14} />}
      onSelect={() => onOrderPick('manual')}
    >
      手动排序
    </MenuItem>
    <MenuItem
      selected={orderBy === 'updated'}
      icon={<ViewOrderUpdatedIcon size={14} />}
      onSelect={() => onOrderPick('updated')}
    >
      最近更新
    </MenuItem>

    <MenuSeparator />
    <MenuLabel>筛选会话</MenuLabel>
    <MenuItem
      selected={archivedFilter === 'hide'}
      icon={<ViewFilterHiddenIcon size={14} />}
      onSelect={() => onArchivedPick('hide')}
    >
      隐藏已归档
    </MenuItem>
    <MenuItem
      selected={archivedFilter === 'show'}
      icon={<ViewFilterAllIcon size={14} />}
      onSelect={() => onArchivedPick('show')}
    >
      全部对话（显示已归档）
    </MenuItem>
    <MenuItem
      selected={archivedFilter === 'only'}
      icon={<ViewFilterOnlyIcon size={14} />}
      onSelect={() => onArchivedPick('only')}
    >
      仅显示已归档
    </MenuItem>
  </Dropdown>
);
