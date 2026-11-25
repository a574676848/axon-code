import React from 'react';
import { AlertCircleIcon, BrainIcon, ChecklistIcon, ClockIcon, FolderIcon, IconPanelLeft, SearchIcon, SettingsIcon } from '../icons';
import { NavGlide, NewSessionButton, SidebarColumn } from './ui/shell';
import { Dropdown, MenuItem, MenuLabel, MenuSeparator } from './ui/overlays';
import { StateDot } from './ui/surfaces';
import type { RailActivity, RailActivityCue } from './railActivity';
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
  /** 右上角的活动徽标；只是状态的视觉提示，结论写在无障碍名称里。 */
  badge?: React.ReactNode;
  /** 徽标状态的键：换档时换键，让入场动画重新播一次。 */
  badgeKey?: string;
  children: React.ReactNode;
}> = ({ label, active = false, onClick, badge, badgeKey, children }) => (
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
    {badge !== undefined && (
      <span
        key={badgeKey}
        className="rail-activity-badge pointer-events-none absolute -right-0.5 -top-0.5 flex items-center justify-center"
        aria-hidden="true"
      >
        {badge}
      </span>
    )}
  </button>
);

/** 徽标的无障碍名称：颜色不单独承载状态，中文结论写进按钮名称。 */
function activityLabel(activity: RailActivity): string {
  const parts = ['搜索会话'];
  if (activity.questionCount > 0) parts.push(`${activity.questionCount} 个会话待答复`);
  if (activity.runningCount > 0) parts.push(`${activity.runningCount} 个会话进行中`);
  if (activity.unseenCount > 0) parts.push(`${activity.unseenCount} 个会话有新结果`);
  return parts.join('，');
}

/** 徽标与卡片条目共用一份标识：待答是琥珀线性图标，其余是状态点。 */
const ActivityMark: React.FC<{ cue: RailActivityCue }> = ({ cue }) =>
  cue === 'question' ? (
    <AlertCircleIcon size={14} className="text-gold-ink" />
  ) : (
    <StateDot state={cue} />
  );

/** 收轨态的搜索格：有会话活动时弹出动态卡片，否则维持原来的展开搜索行为。 */
const SearchRailControl: React.FC<{
  activity: RailActivity;
  onOpenSearch: () => void;
  onOpenSession: (workspaceId: string, sessionId: string) => void;
}> = ({ activity, onOpenSearch, onOpenSession }) => {
  const cue = activity.cue;
  if (cue === null) {
    return (
      <RailButton label="搜索会话" onClick={onOpenSearch}>
        <SearchIcon size={14} />
      </RailButton>
    );
  }

  return (
    <Dropdown
      placement="right"
      closeOnSelect
      menuClassName="min-w-[220px] max-w-[300px]"
      trigger={({ open, toggle }) => (
        <RailButton
          label={activityLabel(activity)}
          active={open}
          onClick={toggle}
          badgeKey={cue}
          badge={<ActivityMark cue={cue} />}
        >
          <SearchIcon size={14} />
        </RailButton>
      )}
    >
      <MenuLabel>会话动态</MenuLabel>
      {activity.sessions.map((item) => (
        <MenuItem
          key={`${item.workspaceId}:${item.sessionId}`}
          icon={<ActivityMark cue={item.cue} />}
          onSelect={() => onOpenSession(item.workspaceId, item.sessionId)}
        >
          {item.title}
        </MenuItem>
      ))}
      <MenuSeparator />
      <MenuItem icon={<SearchIcon size={14} />} onSelect={onOpenSearch}>
        搜索会话
      </MenuItem>
    </Dropdown>
  );
};

/** 图标轨上各动作的序号：滑动指示标按它在整列里的位置平移。 */
const RAIL_ORDER = ['expand', 'new', 'search', 'workspace', 'automation', 'goals', 'memory'] as const;
/** 单格边长与格间距，与上面的类名一致。 */
const RAIL_CELL = 36;
const RAIL_GAP = 8;
/** 按钮在 56px 栏里居中，因此左缘缩进 10px。 */
const RAIL_INSET = 10;

interface SidebarRailProps {
  /** 当前顶层视图；导航按钮据此给出选中态。 */
  currentTab: NavTab;
  /** 收轨态搜索格的会话活动聚合：在跑与未查看终态。 */
  activity: RailActivity;
  /** 展开侧边栏。 */
  onToggleSidebar: () => void;
  /** 新建会话。 */
  onNewSession: () => void;
  /** 展开侧边栏并聚焦搜索。 */
  onOpenSearch: () => void;
  /** 从活动卡片进入某条会话：折叠态不展开侧栏，直接切到该会话。 */
  onOpenSession: (workspaceId: string, sessionId: string) => void;
  /** 展开侧边栏并切到工作区视图。 */
  onSelectWorkspace: () => void;
  /** 展开侧边栏并切到自动化任务。 */
  onSelectAutomation: () => void;
  /** 展开侧边栏并切到目标看板。 */
  onSelectGoals: () => void;
  /** 展开侧边栏并切到记忆管理。 */
  onSelectMemory: () => void;
  /** 打开设置弹窗。 */
  onOpenSettings: () => void;
}

export const SidebarRail: React.FC<SidebarRailProps> = ({
  currentTab,
  activity,
  onToggleSidebar,
  onNewSession,
  onOpenSearch,
  onOpenSession,
  onSelectWorkspace,
  onSelectAutomation,
  onSelectGoals,
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
                : currentTab === 'goals'
                  ? RAIL_ORDER.indexOf('goals')
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
        <SearchRailControl
          activity={activity}
          onOpenSearch={onOpenSearch}
          onOpenSession={onOpenSession}
        />
        <RailButton label="工作区" active={currentTab === 'workspace'} onClick={onSelectWorkspace}>
          <FolderIcon size={14} />
        </RailButton>
        <RailButton label="自动化任务" active={currentTab === 'automation'} onClick={onSelectAutomation}>
          <ClockIcon size={14} />
        </RailButton>
        <RailButton label="目标看板" active={currentTab === 'goals'} onClick={onSelectGoals}>
          <ChecklistIcon size={14} />
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
