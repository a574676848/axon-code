import React from 'react';
import { BrainIcon, ClockIcon, FolderIcon, IconPanelLeft, SearchIcon, SettingsIcon } from '../icons';
import { NewSessionButton, SidebarColumn } from './ui/shell';
import { ROW_SELECTED_BAR } from './SidebarRows';
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
    className={`relative w-9 h-9 flex items-center justify-center rounded-md transition-colors duration-100 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
      active
        ? 'bg-active text-text-primary'
        : 'text-text-secondary hover:bg-hover hover:text-text-primary'
    }`}
  >
    {active && <span aria-hidden="true" className={ROW_SELECTED_BAR} />}
    {children}
  </button>
);

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
  <SidebarColumn label="折叠侧边栏" variant="rail" className="justify-between">
    <div className="flex flex-col items-center gap-2">
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

    <RailButton label="设置" onClick={onOpenSettings}>
      <SettingsIcon size={14} />
    </RailButton>
  </SidebarColumn>
);
