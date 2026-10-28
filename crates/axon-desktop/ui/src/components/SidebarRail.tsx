import React from 'react';
import { BrainIcon, ClockIcon, FolderIcon, IconPanelLeft, SearchIcon, SettingsIcon } from '../icons';
import { NavGlide, NewSessionButton, SidebarColumn } from './ui/shell';
import type { NavTab } from '../types';

/*
 * 图标轨：侧边栏折叠后的 56px 形态。
 * 每个动作都是 36x36 的方形控件，靠无障碍名称提供原生提示。
 *
 * 三种状态与展开态同一套语言：hover 一档浅底，当前页是更深一档的底加左侧 2px 主色条。
 * 当前页因此不再只靠 `aria-current` 表达，视力用户与读屏器读到的是同一个事实。
 */

/** 图标轨上的一个动作。 */
const RailButton: React.FC<{
  label: string;
  active?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}> = ({ label, active = false, onClick, children }) => (
  <button
    type="button"
    aria-label={label}
    title={label}
    aria-current={active ? 'page' : undefined}
    onClick={onClick}
    className={`btn-tactile relative w-9 h-9 flex items-center justify-center rounded-md focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
      active
        ? 'bg-surface shadow-lift hairline border-border text-text-primary'
        : 'text-text-secondary hover:bg-hover hover:text-text-primary'
    }`}
  >
    {children}
  </button>
);

/** 图标轨上各动作的序号：滑动指示标按它在整列里的位置平移。 */
const RAIL_ORDER = ['expand', 'new', 'search', 'workspace', 'automation', 'memory'] as const;
/** 单格边长与格间距，与上面的类名一致。 */
const RAIL_CELL = 36;
const RAIL_GAP = 8;
/** 按钮在 56px 栏里居中，因此左缘缩进 10px。 */
const RAIL_INSET = 10;

interface SidebarRailProps {
  /** 当前顶层视图；导航按钮据此给出选中态。 */
  currentTab: NavTab;
  /** 展开侧边栏。 */
  onToggleSidebar: () => void;
  /** 新建会话。 */
  onNewSession: () => void;
  /** 展开侧边栏并聚焦搜索。 */
  onOpenSearch: () => void;
  /** 展开侧边栏并切到工作区视图。 */
  onSelectWorkspace: () => void;
  /** 展开侧边栏并切到自动化任务。 */
  onSelectAutomation: () => void;
  /** 展开侧边栏并切到记忆管理。 */
  onSelectMemory: () => void;
  /** 打开设置弹窗。 */
  onOpenSettings: () => void;
}

export const SidebarRail: React.FC<SidebarRailProps> = ({
  currentTab,
  onToggleSidebar,
  onNewSession,
  onOpenSearch,
  onSelectWorkspace,
  onSelectAutomation,
  onSelectMemory,
  onOpenSettings,
}) => (
  <SidebarColumn
    label="折叠侧边栏"
    variant="rail"
    className="justify-between"
  >
    <div
      data-tauri-drag-region
      style={{ paddingTop: 'var(--macos-traffic-inset, 0px)' }}
      className="flex flex-col items-center"
    >
      {/*
        整列只有一枚滑动指示标：切页时只有它在平移，图标本身不重排。
        它从这里开始摆放（内边距在外层），因此位移基准与第一格严格对齐。
      */}
      <div className="relative flex flex-col items-center gap-2">
        <NavGlide
          activeIndex={
            currentTab === 'workspace'
              ? RAIL_ORDER.indexOf('workspace')
              : currentTab === 'automation'
                ? RAIL_ORDER.indexOf('automation')
                : currentTab === 'memory'
                  ? RAIL_ORDER.indexOf('memory')
                  : null
          }
          itemSize={RAIL_CELL}
          gap={RAIL_GAP}
          inset={RAIL_INSET}
        />
        <RailButton label="展开侧边栏" onClick={onToggleSidebar}>
          <IconPanelLeft size={14} />
        </RailButton>
        <NewSessionButton compact onClick={onNewSession} />
        <RailButton label="搜索会话" onClick={onOpenSearch}>
          <SearchIcon size={14} />
        </RailButton>
        <RailButton label="工作区" active={currentTab === 'workspace'} onClick={onSelectWorkspace}>
          <FolderIcon size={14} />
        </RailButton>
        <RailButton label="自动化任务" active={currentTab === 'automation'} onClick={onSelectAutomation}>
          <ClockIcon size={14} />
        </RailButton>
        <RailButton label="记忆管理" active={currentTab === 'memory'} onClick={onSelectMemory}>
          <BrainIcon size={14} />
        </RailButton>
      </div>
    </div>

    <RailButton label="设置" onClick={onOpenSettings}>
      <SettingsIcon size={14} />
    </RailButton>
  </SidebarColumn>
);
