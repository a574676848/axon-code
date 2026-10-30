import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircleIcon,
  BrainIcon,
  ChecklistIcon,
  ClockIcon,
} from '../icons';
import type { GatewayHealth, LaneItem, NavTab, WorkspaceItem } from '../types';
import * as ipc from '../ipc';
import type { LaneSelection } from '../conversation/useConversation';
import { isRunning, type LaneRuntimeFacts } from '../conversation/runtimeFacts';
import { Banner, Text } from './ui/surfaces';
import { NewSessionButton, NavGlide, SidebarColumn } from './ui/shell';
import { SidebarRail } from './SidebarRail';
import { SidebarBrandBar, SidebarSectionHeader } from './SidebarHeader';
import {
  ROW_ICON_COLUMN,
  ROW_STATE,
  rowTone,
} from './SidebarRows';
import { ArchiveLaneDialog, RemoveWorkspaceDialog } from './SidebarConfirmations';
import { WorkspaceTree } from './WorkspaceTree';
import type { ArchivedFilter, GroupMode, OrderMode } from './ViewOptionsMenu';
import { SettingsIcon } from '../icons';
import { UpdateHint } from './UpdateHint';

/** 侧栏默认宽度，与既有工作台产品一致。 */
const DEFAULT_WIDTH = 280;
/** 会话列表静默刷新间隔。 */
const LANE_REFRESH_MS = 15000;
/** 侧栏最小宽度，与既有工作台产品一致。 */
const MIN_WIDTH = 264;
/** 侧栏最大宽度，与既有工作台产品一致。 */
const MAX_WIDTH = 420;
/** 拖拽到此阈值以下时自动收进图标轨。 */
const COLLAPSE_THRESHOLD = 140;

const NavRow: React.FC<{
  icon: React.ReactNode;
  label: string;
  active: boolean;
  onClick: () => void;
}> = ({ icon, label, active, onClick }) => (
  <button
    type="button"
    onClick={onClick}
    aria-current={active ? 'page' : undefined}
    className={`${ROW_STATE} ${rowTone(active)} w-full h-8 px-0 text-left`}
  >
    {/* 选中指示由整列的 NavGlide 承担，行内不再画静态条。 */}
    <span className={`${ROW_ICON_COLUMN} text-text-tertiary`}>{icon}</span>
    <Text size="body" className="leading-5" truncate>
      {label}
    </Text>
  </button>
);

interface SidebarProps {
  /** 当前顶层视图。 */
  currentTab: NavTab;
  /** 切换顶层视图。 */
  onSelectTab: (tab: NavTab) => void;
  /** 侧边栏是否展开。 */
  isOpen: boolean;
  /** 折叠或展开侧边栏。 */
  onToggleSidebar: () => void;
  /** 打开设置弹窗。 */
  onOpenSettings: () => void;
  /** 网关连接状态；为空表示尚未探测完成。 */
  health?: GatewayHealth | null;
  /** 选中某条会话；主区域据此建立订阅。 */
  onSelectLane?: (selection: LaneSelection | null) => void;
  /** 外壳当前选中的会话标识；未激活会话只显示这一条。 */
  selectedLaneId?: string | null;
  /** 当前选中会话的运行时事实；归档正在运行的会话前据此二次确认。 */
  runtime?: LaneRuntimeFacts | null;
  /** 有新结果的车道：车道标识到终态；点开即清除。 */
  laneMarks?: Record<string, string>;
}

export const Sidebar: React.FC<SidebarProps> = ({
  currentTab,
  onSelectTab,
  isOpen,
  onToggleSidebar,
  onOpenSettings,
  health = null,
  onSelectLane,
  selectedLaneId = null,
  runtime = null,
  laneMarks,
}) => {
  const [width, setWidth] = useState(DEFAULT_WIDTH);
  const [resizing, setResizing] = useState(false);

  const [workspaces, setWorkspaces] = useState<WorkspaceItem[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  /** 首次读取是否已经落地：连上但还没读到数据时，骨架比「没有工作区」准确。 */
  const [loadedOnce, setLoadedOnce] = useState(false);
  const [activeWorkspaceId, setActiveWorkspaceId] = useState('');
  const [activeLaneId, setActiveLaneId] = useState('');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  /** 展示用的工作区全路径，用于树形分组的父子判定。 */
  const [fullPaths, setFullPaths] = useState<Record<string, string>>({});

  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState('');
  const [groupBy, setGroupBy] = useState<GroupMode>('workspace');
  const [orderBy, setOrderBy] = useState<OrderMode>('updated');
  const [archivedFilter, setArchivedFilter] = useState<ArchivedFilter>('hide');
  /** 等待确认的归档目标；非空时弹窗。 */
  const [pendingArchive, setPendingArchive] = useState<LaneItem | null>(null);
  const [stopping, setStopping] = useState(false);
  /** 等待确认的移除目标；只在「正在跑或正在看」的工作区上弹窗。 */
  const [pendingRemove, setPendingRemove] = useState<WorkspaceItem | null>(null);
  const [removing, setRemoving] = useState(false);

  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchToggleRef = useRef<HTMLDivElement>(null);

  /** 统一吸收后端返回的最新工作区列表。 */
  const ingest = useCallback((list: WorkspaceItem[]) => {
    setWorkspaces(list);
    setLoadedOnce(true);
    setLoadError(null);
    setFullPaths((current) => {
      const next = { ...current };
      for (const workspace of list) next[workspace.id] = workspace.path;
      return next;
    });
    setActiveWorkspaceId((current) =>
      list.some((workspace) => workspace.id === current)
        ? current
        : (list.find((workspace) => workspace.active)?.id ?? list[0]?.id ?? ''),
    );
    setExpanded((current) => {
      const next = { ...current };
      for (const workspace of list) next[workspace.id] ??= workspace.active;
      return next;
    });
  }, []);

  const refresh = useCallback(async () => {
    if (health?.connected !== true) return;
    try {
      ingest(await ipc.fetchWorkspaces());
      setLoadError(null);
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : '读取工作区失败');
    }
  }, [health?.connected, ingest]);

  useEffect(() => {
    if (health?.connected !== true) return;
    void refresh();
  }, [health?.connected, refresh]);

  /*
   * 静默轮询：会话可能由其它渠道产生（例如 TUI 新开一条），侧栏必须能自己发现。
   * 轮询只换数据、不动界面状态，失败也不弹错，连接问题由外壳的心跳统一呈现。
   */
  useEffect(() => {
    if (health?.connected !== true) return;
    const timer = window.setInterval(() => {
      void ipc
        .fetchWorkspaces()
        .then((list) => ingest(list))
        .catch(() => undefined);
    }, LANE_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [health?.connected, ingest]);

  const mutate = useCallback(
    async (action: () => Promise<WorkspaceItem[]>, fallback: string): Promise<boolean> => {
      try {
        ingest(await action());
        return true;
      } catch (error) {
        setLoadError(error instanceof Error ? error.message : fallback);
        return false;
      }
    },
    [ingest],
  );

  const createLane = useCallback(
    async (workspaceId?: string) => {
      const target = workspaceId ?? activeWorkspaceId;
      if (target.length === 0) return;
      try {
        const list = await ipc.createLane(target);
        const workspace = list.find((item) => item.id === target);
        const created = workspace?.lanes
          .filter((lane) => lane.pending)
          .sort((left, right) => right.created_at - left.created_at)[0];
        ingest(list);
        if (created !== undefined) {
          setActiveWorkspaceId(target);
          setActiveLaneId(created.id);
          onSelectLane?.({
            workspaceId: target,
            workspacePath: workspace?.path ?? '',
            laneId: created.id,
            destination: created.destination,
            title: created.title,
            channel: created.channel,
            pending: created.pending,
          });
        }
        setExpanded((current) => ({ ...current, [target]: true }));
        onSelectTab('chat');
      } catch (error) {
        setLoadError(error instanceof Error ? error.message : '新建会话失败');
      }
    },
    [activeWorkspaceId, ingest, onSelectLane, onSelectTab],
  );

  /** 添加工作区：一步直达系统原生目录选择器，选中后立即登记。 */
  const addWorkspace = useCallback(async () => {
    let picked: string | null = null;
    try {
      picked = await ipc.pickDirectory();
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : '无法打开目录选择器');
      return;
    }
    if (picked === null || picked.length === 0) return;
    try {
      const list = await ipc.addWorkspace(picked);
      ingest(list);
      const added = list.find((workspace) => workspace.path === picked);
      if (added !== undefined) {
        setActiveWorkspaceId(added.id);
        setExpanded((current) => ({ ...current, [added.id]: true }));
      }
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : '添加工作区失败');
    }
  }, [ingest]);

  /** 拖拽重排：落盘后切到手动排序，与用户看到的新顺序保持一致。 */
  const reorderLanes = useCallback(
    async (workspaceId: string, orderedIds: string[]) => {
      setOrderBy('manual');
      await mutate(() => ipc.laneReorder(workspaceId, orderedIds), '保存排序失败');
    },
    [mutate],
  );

  /** 真正落盘归档；归档掉正在看的会话后回到首屏。 */
  const applyArchive = useCallback(
    (lane: LaneItem) =>
      mutate(() => ipc.setLaneArchived(lane.id, !lane.archived), '归档操作失败').then((ok) => {
        // 内容已经归档，继续停留在它的正文上会让用户以为它还在；失败时保持原状不动。
        if (ok && !lane.archived && lane.id === selectedLaneId) onSelectLane?.(null);
      }),
    [mutate, onSelectLane, selectedLaneId],
  );

  /**
   * 移除工作区：菜单里已经是两步动作，安静的工作区不再弹确认。
   * 只有「里面还在跑」或「用户正看着它下面的会话」时先问一次：
   * 前者移除后任务仍在继续，后者移除后主区域会没有归属。
   */
  const requestRemoveWorkspace = useCallback(
    (id: string) => {
      const workspace = workspaces.find((item) => item.id === id);
      if (workspace === undefined) return;
      const viewed = workspace.lanes.some((lane) => lane.id === selectedLaneId);
      if (workspace.lanes.some((lane) => lane.running) || viewed) {
        setPendingRemove(workspace);
        return;
      }
      void mutate(() => ipc.removeWorkspace(id), '移除工作区失败');
    },
    [mutate, selectedLaneId, workspaces],
  );

  /** 确认移除：移除后把还停留在该工作区的会话视图收回首屏。 */
  const confirmRemoveWorkspace = useCallback(async () => {
    const workspace = pendingRemove;
    if (workspace === null) return;
    setRemoving(true);
    try {
      const ok = await mutate(() => ipc.removeWorkspace(workspace.id), '移除工作区失败');
      // 工作区没了，它的会话也就不再属于任何可见分组：继续停在正文上会让用户
      // 以为这条会话还在侧栏里（实测过：移除后主区域仍显示被移除工作区的会话）。
      if (ok && workspace.lanes.some((lane) => lane.id === selectedLaneId)) onSelectLane?.(null);
    } finally {
      setRemoving(false);
      setPendingRemove(null);
    }
  }, [mutate, onSelectLane, pendingRemove, selectedLaneId]);

  /**
   * 归档入口。
   *
   * 正在运行的会话先问一次：归档只是把会话移出列表，不会终止任务，
   * 用户以为「归档等于结束」时最容易丢东西，所以把正在跑的事实摆出来再确认。
   */
  const requestArchive = useCallback(
    (lane: LaneItem) => {
      const busy =
        !lane.archived &&
        runtime !== null &&
        runtime.laneId === lane.id &&
        isRunning(runtime);
      if (busy) {
        setPendingArchive(lane);
        return;
      }
      void applyArchive(lane);
    },
    [applyArchive, runtime],
  );

  /** 确认归档：先停掉本轮与派生的子任务，再归档。 */
  const confirmArchive = useCallback(async () => {
    const lane = pendingArchive;
    if (lane === null) return;
    setStopping(true);
    try {
      await runtime?.stop();
    } catch {
      // 停止失败不阻塞归档：用户确认的是归档意图，任务状态由会话自己收敛。
    } finally {
      setStopping(false);
      setPendingArchive(null);
    }
    await applyArchive(lane);
  }, [applyArchive, pendingArchive, runtime]);

  /* 拖拽调节宽度：实时跟随指针，过窄自动收进图标轨，双击恢复默认。 */
  useEffect(() => {
    if (!resizing) return;
    const handleMove = (event: MouseEvent) => {
      if (event.clientX < COLLAPSE_THRESHOLD) {
        setResizing(false);
        onToggleSidebar();
        return;
      }
      setWidth(Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, event.clientX)));
    };
    const handleUp = () => setResizing(false);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
    return () => {
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
    };
  }, [resizing, onToggleSidebar]);

  /* 快捷键：新建会话，与按钮提示保持一致。 */
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.altKey && event.key.toLowerCase() === 'n') {
        event.preventDefault();
        void createLane();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [createLane]);

  // 点击应用内其他位置即退出搜索并清空条件，与既有工作台产品的行为一致。
  useEffect(() => {
    if (!searching) return;
    const handlePointerDown = (event: MouseEvent) => {
      const target = event.target as Node | null;
      if (searchInputRef.current?.contains(target)) return;
      if (searchToggleRef.current?.contains(target)) return;
      setQuery('');
      setSearching(false);
    };
    document.addEventListener('mousedown', handlePointerDown);
    return () => document.removeEventListener('mousedown', handlePointerDown);
  }, [searching]);

  /**
   * 移除确认弹窗要摆出的事实。
   *
   * 「正在跑」取两个来源的并集：侧栏车道状态每 15 秒才刷新一次，当前会话的实时
   * 事实能补上刚刚起的那一轮，否则用户会在弹窗里看到「没有任何任务在跑」。
   */
  const removalFacts =
    pendingRemove === null
      ? null
      : {
          viewed: pendingRemove.lanes.some((lane) => lane.id === selectedLaneId),
          busy:
            pendingRemove.lanes.some((lane) => lane.running) ||
            (runtime !== null &&
              isRunning(runtime) &&
              pendingRemove.lanes.some((lane) => lane.id === runtime.laneId)),
        };

  /** 按搜索、归档筛选与排序规则加工展示数据。 */
  const presented = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    const currentLane = selectedLaneId ?? activeLaneId;
    const currentWorkspace = workspaces.find((workspace) =>
      workspace.lanes.some((lane) => lane.id === currentLane),
    )?.id ?? activeWorkspaceId;
    const lanesOf = (workspace: WorkspaceItem): LaneItem[] =>
      workspace.lanes
        .filter((lane) => {
          if (archivedFilter === 'hide' && lane.archived) return false;
          if (archivedFilter === 'only' && !lane.archived) return false;
          // 未激活会话是「即将开始的会话」，只保留当前这一条：
          // 历史遗留的空会话不该在列表里长期占位，这与既有工作台产品一致。
          if (lane.pending && lane.id !== currentLane) return false;
          if (keyword.length === 0) return true;
          return (
            lane.title.toLowerCase().includes(keyword) ||
            workspace.name.toLowerCase().includes(keyword)
          );
        })
        .sort((left, right) => {
          if (left.pinned !== right.pinned) return left.pinned ? -1 : 1;
          if (orderBy === 'updated') return right.updated_at - left.updated_at;
          // 手动排序保持后端给出的顺序，即置顶块加上用户保存的次序。
          return 0;
        });

    return workspaces.map((workspace) => ({
      ...workspace,
      active: workspace.id === currentWorkspace,
      lanes: lanesOf(workspace),
      // 树形分组需要真实路径做父子判定。
      path: fullPaths[workspace.id] ?? workspace.path,
    }));
  }, [workspaces, query, archivedFilter, orderBy, fullPaths, selectedLaneId, activeLaneId, activeWorkspaceId]);

  if (!isOpen) {
    return (
      <SidebarRail
        currentTab={currentTab}
        onToggleSidebar={onToggleSidebar}
        onNewSession={() => void createLane()}
        onOpenSearch={() => {
          onToggleSidebar();
          setSearching(true);
        }}
        onSelectWorkspace={() => {
          onToggleSidebar();
          onSelectTab('workspace');
        }}
        onSelectAutomation={() => {
          onToggleSidebar();
          onSelectTab('automation');
        }}
        onSelectGoals={() => {
          onToggleSidebar();
          onSelectTab('goals');
        }}
        onSelectMemory={() => {
          onToggleSidebar();
          onSelectTab('memory');
        }}
        onOpenSettings={onOpenSettings}
      />
    );
  }

  return (
    <SidebarColumn label="工作区与会话" width={width}>
      <SidebarBrandBar onToggleSidebar={onToggleSidebar} />

      {/* 新建会话与顶部导航 */}
      <div className="shrink-0 px-3 pb-3 pt-1 space-y-0.5">
        <NewSessionButton onClick={() => void createLane()} />
        {/*
          三行导航各自不再画静态指示条，整列共用一枚滑动指示标：
          切换视图时只有它在平移，行本身不重排。
        */}
        <div className="relative space-y-0.5">
          <NavGlide
            activeIndex={
              currentTab === 'automation' ? 0 : currentTab === 'goals' ? 1 : currentTab === 'memory' ? 2 : null
            }
            itemSize={32}
            gap={2}
          />
          <NavRow
            icon={<ClockIcon size={14} />}
            label="自动化任务"
            active={currentTab === 'automation'}
            onClick={() => onSelectTab('automation')}
          />
          <NavRow
            icon={<ChecklistIcon size={14} />}
            label="目标看板"
            active={currentTab === 'goals'}
            onClick={() => onSelectTab('goals')}
          />
          <NavRow
            icon={<BrainIcon size={14} />}
            label="记忆管理"
            active={currentTab === 'memory'}
            onClick={() => onSelectTab('memory')}
          />
        </div>
      </div>

      <SidebarSectionHeader
        groupBy={groupBy}
        orderBy={orderBy}
        archivedFilter={archivedFilter}
        searching={searching}
        query={query}
        searchInputRef={searchInputRef}
        searchToggleRef={searchToggleRef}
        onStartSearch={() => {
          setSearching(true);
          window.setTimeout(() => searchInputRef.current?.focus(), 30);
        }}
        onStopSearch={() => {
          setQuery('');
          setSearching(false);
        }}
        onQueryChange={setQuery}
        onGroupPick={setGroupBy}
        onOrderPick={setOrderBy}
        onArchivedPick={setArchivedFilter}
        onAddWorkspace={() => void addWorkspace()}
      />

      {/* 工作区与会话列表 */}
      <div className="flex-1 min-h-0 overflow-y-auto no-scrollbar px-3 pb-2 pt-1">
        {loadError !== null && health?.connected !== false ? (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={loadError}
            action={
              <button
                type="button"
                onClick={() => void refresh()}
                className="text-meta text-text-primary underline underline-offset-2"
              >
                重试
              </button>
            }
          />
        ) : (
          <WorkspaceTree
            groupBy={groupBy}
            workspaces={presented}
            activeLaneId={selectedLaneId ?? activeLaneId}
            laneMarks={laneMarks}
            expanded={expanded}
            loading={health === null || (health.connected && !loadedOnce)}
            onToggleWorkspace={(id) =>
              setExpanded((current) => ({ ...current, [id]: current[id] !== true }))
            }
            onSelectLane={(workspaceId, laneId) => {
              setActiveWorkspaceId(workspaceId);
              setActiveLaneId(laneId);
              const workspace = presented.find((item) => item.id === workspaceId);
              const lane: LaneItem | undefined = workspace?.lanes.find(
                (item) => item.id === laneId,
              );
              if (workspace !== undefined && lane !== undefined) {
                onSelectLane?.({
                  workspaceId,
                  workspacePath: workspace.path,
                  laneId,
                  destination: lane.destination,
                  title: lane.title,
                  channel: lane.channel,
                  pending: lane.pending,
                });
              }
              onSelectTab('chat');
            }}
            onTogglePin={(lane) =>
              void mutate(() => ipc.setLanePinned(lane.id, !lane.pinned), '置顶操作失败')
            }
            onToggleArchive={(lane) => requestArchive(lane)}
            onRenameLane={(laneId, title) =>
              void mutate(() => ipc.renameLane(laneId, title), '重命名失败')
            }
            onReorder={(workspaceId, orderedIds) => void reorderLanes(workspaceId, orderedIds)}
            onCreateLane={(workspaceId) => void createLane(workspaceId)}
            onRemoveWorkspace={requestRemoveWorkspace}
          />
        )}
      </div>

      {/* 底部设置入口，连接问题由统一弹窗呈现；确认有新版本时右端多一枚下载入口。 */}
      <div className="shrink-0 px-3 py-2">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={onOpenSettings}
            className={`${ROW_STATE} hover:bg-hover flex-1 min-w-0 h-8 text-left`}
          >
            <span className={`${ROW_ICON_COLUMN} text-text-tertiary`}>
              <SettingsIcon size={14} />
            </span>
            <Text size="body" className="leading-5" truncate>
              设置
            </Text>
          </button>
          <UpdateHint />
        </div>
      </div>

      {/* 宽度拖拽把手：与既有工作台产品一致，做成不可见的命中区，
          只保留列宽光标这一处可见反馈，不额外绘制把手或分隔条。 */}
      <div
        role="separator"
        aria-orientation="vertical"
        aria-label="调整侧栏宽度"
        onMouseDown={(event) => {
          event.preventDefault();
          setResizing(true);
        }}
        onDoubleClick={() => setWidth(DEFAULT_WIDTH)}
        className="absolute top-0 right-0 w-2 -mr-1 h-full cursor-col-resize touch-none z-20"
      />

      <ArchiveLaneDialog
        lane={pendingArchive}
        runtime={runtime}
        stopping={stopping}
        onClose={() => setPendingArchive(null)}
        onConfirm={() => void confirmArchive()}
      />

      <RemoveWorkspaceDialog
        workspace={pendingRemove}
        removing={removing}
        busy={removalFacts?.busy === true}
        viewed={removalFacts?.viewed === true}
        onClose={() => setPendingRemove(null)}
        onConfirm={() => void confirmRemoveWorkspace()}
      />
    </SidebarColumn>
  );
};
