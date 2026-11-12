/*
 * 命令面板候选的构造。
 *
 * 从 `App` 里整块移出：这是一张「有什么可跳」的清单，不含任何状态，也不参与渲染，
 * 放在组合根里只会把壳层撑长。每一项都必须落到真实动作上，因此依赖全部由参数注入。
 */

import * as ipc from './ipc';
import { SETTINGS_SECTION_LABELS } from './components/BasicSettingsPanel';
import type { CommandPaletteItem } from './components/ui/CommandPalette';
import type { NavTab, SettingsSection, WorkspaceItem } from './types';
import type { LaneSelection } from './conversation/useConversation';
import { applyTheme, currentTheme, THEME_CHOICES } from './theme';

/** 一条可切换的连接档案；只用到标识与显示名。 */
export interface PaletteProfile {
  id: string;
  label: string;
}

/** 构造命令面板所需的全部外部动作。 */
export interface PaletteSources {
  workspaces: WorkspaceItem[];
  profiles: PaletteProfile[];
  navigate: (tab: NavTab) => void;
  openSettingsAt: (section: SettingsSection) => void;
  switchProfile: (profileId: string) => void;
  selectLane: (selection: LaneSelection) => void;
}

/** 组装命令面板候选：跳转、连接、设置、主题与会话。 */
export function buildPaletteItems(sources: PaletteSources): CommandPaletteItem[] {
  const { workspaces, profiles, navigate, openSettingsAt, switchProfile, selectLane } = sources;

  const navigation: CommandPaletteItem[] = [
    { id: 'nav-chat', label: '前往对话', group: '跳转', run: () => navigate('chat') },
    { id: 'nav-workspace', label: '前往工作区', group: '跳转', run: () => navigate('workspace') },
    {
      id: 'nav-automation',
      label: '前往自动化任务',
      group: '跳转',
      run: () => navigate('automation'),
    },
    { id: 'nav-goals', label: '前往目标看板', group: '跳转', run: () => navigate('goals') },
    { id: 'nav-memory', label: '前往记忆管理', group: '跳转', run: () => navigate('memory') },
  ];

  /*
   * 连接档案：非生效档案各给一条「切换到 <档案名>」，另给一条「管理连接…」。
   * 切档会重启应用，因此这里只发一次请求，重试与错误由连接分区呈现。
   */
  const connections: CommandPaletteItem[] = [
    ...profiles
      .filter((profile) => !(profile as PaletteProfile & { active?: boolean }).active)
      .map((profile) => ({
        id: `switch-${profile.id}`,
        label: `切换到 ${profile.label}`,
        group: '连接',
        run: () => switchProfile(profile.id),
      })),
    {
      id: 'manage-connection',
      label: '管理连接…',
      group: '连接',
      run: () => openSettingsAt('connection'),
    },
  ];

  const settings: CommandPaletteItem[] = SETTINGS_SECTION_LABELS.map((section) => ({
    id: `settings-${section.id}`,
    label: `设置：${section.label}`,
    group: '设置',
    run: () => openSettingsAt(section.id),
  }));

  const themes: CommandPaletteItem[] = THEME_CHOICES.map((choice) => ({
    id: `theme-${choice.id}`,
    label: `主题：${choice.label}`,
    group: '外观',
    run: () => {
      // 与设置页同一条路径：先乐观上色，落盘回执再校正一次，失败回退到当前主题。
      applyTheme(choice.id);
      void ipc
        .desktopSetTheme(choice.id)
        .then((saved) => applyTheme(saved.theme))
        .catch(() => applyTheme(currentTheme()));
    },
  }));

  const lanes: CommandPaletteItem[] = workspaces.flatMap((workspace) =>
    workspace.lanes.map((lane) => ({
      id: `lane-${lane.id}`,
      label: `${lane.title || '未命名会话'}`,
      group: workspace.name || '会话',
      run: () => {
        selectLane({
          workspaceId: workspace.id,
          workspacePath: workspace.path,
          laneId: lane.id,
          destination: lane.destination,
          title: lane.title,
          channel: lane.channel,
          pending: lane.pending,
        });
        navigate('chat');
      },
    })),
  );

  return [...navigation, ...connections, ...settings, ...themes, ...lanes];
}
