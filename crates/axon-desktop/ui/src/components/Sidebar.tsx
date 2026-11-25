import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertCircleIcon,
  BrainIcon,
  ChecklistIcon,
  ClockIcon,
} from '../icons';
import type { GatewayHealth, SessionItem, NavTab, WorkspaceItem } from '../types';
import type { McpHeaderReading } from '../conversation/mcpReading';
import * as ipc from '../ipc';
import type { SessionSelection } from '../conversation/useConversation';
import { isRunning, type SessionRuntimeFacts } from '../conversation/runtimeFacts';
import { Banner, Text } from './ui/surfaces';
import { NewSessionButton, NavGlide, SidebarColumn } from './ui/shell';
import { SidebarRail } from './SidebarRail';
import { useRailActivity } from './useRailActivity';
import { SidebarBrandBar, SidebarSectionHeader } from './SidebarHeader';
import {
  ROW_ICON_COLUMN,
  ROW_STATE,
  rowTone,
} from './SidebarRows';
import { ArchiveSessionDialog, RemoveWorkspaceDialog } from './SidebarConfirmations';
import { WorkspaceTree } from './WorkspaceTree';
import { buildSidebarTree } from './sidebarTree';
import type { ArchivedFilter, GroupMode, OrderMode } from './ViewOptionsMenu';
import { SidebarFooter } from './SidebarFooter';

/** 侧栏默认宽度，与既有工作台产品一致。 */
const DEFAULT_WIDTH = 280;
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
  /** 打开设置 › 连接；未传时连接胶囊的「管理连接…」退回调用 onOpenSettings。 */
  onOpenConnection?: () => void;
  /** 网关连接状态；为空表示尚未探测完成。 */
  health?: GatewayHealth | null;
  /** 选中某条会话；主区域据此建立订阅。 */
  onSelectSession?: (selection: SessionSelection | null) => void;
  /** 外壳当前选中的会话标识；未激活会话只显示这一条。 */
  selectedSessionId?: string | null;
  /** 当前选中会话的运行时事实；归档正在运行的会话前据此二次确认。 */
  runtime?: SessionRuntimeFacts | null;
  /** 有新结果的车道：车道标识到终态；点开即清除。 */
  sessionMarks?: Record<string, string>;
  /** MCP 运行态读数；由外壳持有唯一一份，侧栏只负责呈现。 */
  mcpReading?: McpHeaderReading | null;
  /** 已安装技能条数；清单未就绪时为空。 */
  skillCount?: number | null;
  /** 上次打开的工作区标识：启动时默认展开它。 */
  preferredWorkspaceId?: string;
  /** 本地状态文件记录的展开清单：启动时按它恢复展开态。 */
  expandedWorkspaceIds?: string[];
  /** 展开态变化回执：由外壳写回本地状态文件。 */
  onExpandedChange?: (ids: string[]) => void;
  /**
   * 工作区与会话清单。
   *
   * 由外壳持有：取数、15 秒静默轮询与失败结论都只有一份，侧栏只消费不重取。
   */
  workspaces?: WorkspaceItem[];
  /** 清单是否已经成功读过一次：连上但还没读到数据时给骨架而不是空态。 */
  workspacesLoaded?: boolean;
  /** 用户主动重试后的失败原因；轮询失败不进这里。 */
  workspacesError?: string | null;
  /** 单次动作（新建、置顶、归档、加工作区）返回的新清单写回外壳。 */
  onWorkspacesChange?: (list: WorkspaceItem[]) => void;
  /** 重新读取清单；返回失败原因，成功为空。 */
  onRefreshWorkspaces?: () => Promise<string | null>;
}

export const Sidebar: React.FC<SidebarProps> = ({
  currentTab,
  onSelectTab,
  isOpen,
  onToggleSidebar,
  onOpenSettings,
  onOpenConnection,
  health = null,
  onSelectSession,
  selectedSessionId = null,
  runtime = null,
  sessionMarks,
  mcpReading = null,
  skillCount = null,
  preferredWorkspaceId = '',
  expandedWorkspaceIds,
  onExpandedChange,
  workspaces = [],
  workspacesLoaded = false,
  workspacesError = null,
  onWorkspacesChange,
  onRefreshWorkspaces,
}) => {
  const [width, setWidth] = useState(DEFAULT_WIDTH);
  const [resizing, setResizing] = useState(false);

  /**
   * 单次动作（置顶、归档、新建、重命名、添加工作区）的失败原因。
   *
   * 与列表读取失败分开：动作失败时树还在，把它整块换成错误页等于因为一次置顶没写成
   * 就把用户正在看的会话列表抽走。失败结论挂成树上方的一行回执，树保持原位。
   */
  const [actionError, setActionError] = useState<string | null>(null);
  const [activeWorkspaceId, setActiveWorkspaceId] = useState('');
  const [activeSessionId, setActiveSessionId] = useState('');
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  /** 展示用的工作区全路径，用于树形分组的父子判定。 */
  const [fullPaths, setFullPaths] = useState<Record<string, string>>({});

  const [searching, setSearching] = useState(false);
  const [query, setQuery] = useState('');
  const [groupBy, setGroupBy] = useState<GroupMode>('workspace');
  const [orderBy, setOrderBy] = useState<OrderMode>('updated');
  const [archivedFilter, setArchivedFilter] = useState<ArchivedFilter>('hide');
  /** 等待确认的归档目标；非空时弹窗。 */
  const [pendingArchive, setPendingArchive] = useState<SessionItem | null>(null);
  const [stopping, setStopping] = useState(false);
  /** 等待确认的移除目标；只在「正在跑或正在看」的工作区上弹窗。 */
  const [pendingRemove, setPendingRemove] = useState<WorkspaceItem | null>(null);
  const [removing, setRemoving] = useState(false);

  const searchInputRef = useRef<HTMLInputElement>(null);
  const searchToggleRef = useRef<HTMLDivElement>(null);

  /**
   * 清单变了就补一次本地派生。
   *
   * 清单本身归外壳所有，这里只算「这份数据在侧栏里怎么呈现」：全路径表、当前工作区
   * 与展开态。轮询每 15 秒换一次数组引用，这几项各自做功能更新，用户手里的开合不被抢回。
   */
  const ingest = useCallback((list: WorkspaceItem[]) => {
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
      /*
       * 展开态跟着本地状态文件走：启动时把上次展开的工作区原样展开（并保证上次打开的
       * 那条也在展开集里）；状态文件里还没有记录时，退回「展开当前工作区」的老行为。
       * 这里只补缺省值（`??=`），用户在本次会话里手动收起的不被抢回来。
       */
      const recorded = expandedWorkspaceIds ?? [];
      for (const workspace of list) {
        next[workspace.id] ??= recorded.length > 0
          ? recorded.includes(workspace.id) || workspace.id === preferredWorkspaceId
          : workspace.active || workspace.id === preferredWorkspaceId;
      }
      return next;
    });
  }, [expandedWorkspaceIds, preferredWorkspaceId]);

  React.useEffect(() => {
    ingest(workspaces);
  }, [ingest, workspaces]);

  const mutate = useCallback(
    async (action: () => Promise<WorkspaceItem[]>, fallback: string): Promise<boolean> => {
      try {
        onWorkspacesChange?.(await action());
        setActionError(null);
        return true;
      } catch (error) {
        // 动作失败只写回执，不动列表读取的状态：树要留在原处。
        setActionError(error instanceof Error ? error.message : fallback);
        return false;
      }
    },
    [onWorkspacesChange],
  );

  const createSession = useCallback(
    async (workspaceId?: string) => {
      const target = workspaceId ?? activeWorkspaceId;
      if (target.length === 0) return;
      try {
        const list = await ipc.createSessionDraft(target);
        const workspace = list.find((item) => item.id === target);
        const created = workspace?.sessions
          .filter((session) => session.draft)
          .sort((left, right) => right.created_at - left.created_at)[0];
        onWorkspacesChange?.(list);
        if (created !== undefined) {
          setActiveWorkspaceId(target);
          setActiveSessionId(created.id);
          onSelectSession?.({
            workspaceId: target,
            workspacePath: workspace?.path ?? '',
            sessionId: created.id,
            destination: created.destination,
            title: created.title,
            channel: created.channel,
            pending: created.pending, draft: created.draft,
          });
        }
        setExpanded((current) => ({ ...current, [target]: true }));
        onSelectTab('chat');
      } catch (error) {
        setActionError(error instanceof Error ? error.message : '新建会话失败');
      }
    },
    [activeWorkspaceId, onSelectSession, onSelectTab, onWorkspacesChange],
  );

  /** 添加工作区：一步直达系统原生目录选择器，选中后立即登记。 */
  const addWorkspace = useCallback(async () => {
    let picked: string | null = null;
    try {
      picked = await ipc.pickDirectory();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : '无法打开目录选择器');
      return;
    }
    if (picked === null || picked.length === 0) return;
    try {
      const list = await ipc.addWorkspace(picked);
      onWorkspacesChange?.(list);
      setActionError(null);
      const added = list.find((workspace) => workspace.path === picked);
      if (added !== undefined) {
        setActiveWorkspaceId(added.id);
        setExpanded((current) => ({ ...current, [added.id]: true }));
      }
    } catch (error) {
      setActionError(error instanceof Error ? error.message : '添加工作区失败');
    }
  }, [onWorkspacesChange]);

  /** 拖拽重排：落盘后切到手动排序，与用户看到的新顺序保持一致。 */
  const reorderSessions = useCallback(
    async (workspaceId: string, orderedIds: string[]) => {
      setOrderBy('manual');
      await mutate(() => ipc.sessionReorder(workspaceId, orderedIds), '保存排序失败');
    },
    [mutate],
  );

  /** 真正落盘归档；归档掉正在看的会话后回到首屏。 */
  const applyArchive = useCallback(
    (session: SessionItem) =>
      mutate(() => ipc.setSessionArchived(session.id, !session.archived), '归档操作失败').then((ok) => {
        // 内容已经归档，继续停留在它的正文上会让用户以为它还在；失败时保持原状不动。
        if (ok && !session.archived && session.id === selectedSessionId) onSelectSession?.(null);
      }),
    [mutate, onSelectSession, selectedSessionId],
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
      const viewed = workspace.sessions.some((session) => session.id === selectedSessionId);
      if (workspace.sessions.some((session) => session.running) || viewed) {
        setPendingRemove(workspace);
        return;
      }
      void mutate(() => ipc.removeWorkspace(id), '移除工作区失败');
    },
    [mutate, selectedSessionId, workspaces],
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
      if (ok && workspace.sessions.some((session) => session.id === selectedSessionId)) onSelectSession?.(null);
    } finally {
      setRemoving(false);
      setPendingRemove(null);
    }
  }, [mutate, onSelectSession, pendingRemove, selectedSessionId]);

  /**
   * 归档入口。
   *
   * 正在运行的会话先问一次：归档只是把会话移出列表，不会终止任务，
   * 用户以为「归档等于结束」时最容易丢东西，所以把正在跑的事实摆出来再确认。
   */
  const requestArchive = useCallback(
    (session: SessionItem) => {
      const busy =
        !session.archived &&
        runtime !== null &&
        runtime.sessionId === session.id &&
        isRunning(runtime);
      if (busy) {
        setPendingArchive(session);
        return;
      }
      void applyArchive(session);
    },
    [applyArchive, runtime],
  );

  /** 确认归档：先停掉本轮与派生的子任务，再归档。 */
  const confirmArchive = useCallback(async () => {
    const session = pendingArchive;
    if (session === null) return;
    setStopping(true);
    try {
      await runtime?.stop();
    } catch {
      // 停止失败不阻塞归档：用户确认的是归档意图，任务状态由会话自己收敛。
    } finally {
      setStopping(false);
      setPendingArchive(null);
    }
    await applyArchive(session);
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
        void createSession();
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [createSession]);

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
          viewed: pendingRemove.sessions.some((session) => session.id === selectedSessionId),
          busy:
            pendingRemove.sessions.some((session) => session.running) ||
            (runtime !== null &&
              isRunning(runtime) &&
              pendingRemove.sessions.some((session) => session.id === runtime.sessionId)),
        };

  /** 按搜索、归档筛选与排序规则加工展示数据（纯计算见 `sidebarTree.ts`）。 */
  const presented = useMemo(
    () =>
      buildSidebarTree({
        workspaces,
        fullPaths,
        query,
        archivedFilter,
        orderBy,
        selectedSessionId,
        activeSessionId,
        activeWorkspaceId,
      }),
    [
      workspaces,
      query,
      archivedFilter,
      orderBy,
      fullPaths,
      selectedSessionId,
      activeSessionId,
      activeWorkspaceId,
    ],
  );

  /** 收轨态徽标与卡片跳转：清单、标识、实时轮次都来自外壳，这里只消费。 */
  const { activity: railActivityView, openSession: openSessionFromRail } = useRailActivity({
    workspaces,
    sessionMarks,
    runtime,
    onSelectSession,
    onSelectTab,
  });

  /*
   * 收轨态整块不渲染连接胶囊，也不给 RailButton 加状态槽：图标轨每一格都是一个
   * 导航目标，混入一枚状态入口会被读成第二个视图（与设计系统 §14.1.1「图标轨不
   * 混状态入口」同源）。收轨态下当前档案与健康态的常驻落点是设置 › 连接，展开
   * 侧栏即可看到胶囊。
   */
  if (!isOpen) {
    return (
      <SidebarRail
        currentTab={currentTab}
        activity={railActivityView}
        onToggleSidebar={onToggleSidebar}
        onNewSession={() => void createSession()}
        onOpenSearch={() => {
          onToggleSidebar();
          setSearching(true);
        }}
        onOpenSession={openSessionFromRail}
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
        <NewSessionButton onClick={() => void createSession()} />
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
        {/*
          动作失败挂在树上方一行，而不是把树换成错误页：置顶、归档失败时用户仍然要看得见
          自己正在看的那串会话。列表读取失败才走下面的整块占位。
        */}
        {actionError !== null && (
          <div className="pb-1.5">
            <Banner
              tone="danger"
              icon={<AlertCircleIcon size={14} />}
              title={actionError}
              action={
                <button
                  type="button"
                  onClick={() => setActionError(null)}
                  className="text-meta text-text-primary underline underline-offset-2"
                >
                  知道了
                </button>
              }
            />
          </div>
        )}
        {workspacesError !== null && health?.available !== false ? (
          <Banner
            tone="danger"
            icon={<AlertCircleIcon size={14} />}
            title={workspacesError}
            action={
              <button
                type="button"
                onClick={() => void onRefreshWorkspaces?.()}
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
            activeSessionId={selectedSessionId ?? activeSessionId}
            sessionMarks={sessionMarks}
            expanded={expanded}
            loading={health === null || (health.available && !workspacesLoaded)}
            /* 实时轮次事实只覆盖当前会话：主区域已经在转圈时，这一行不该还挂在灰点上。 */
            liveRunningSessionId={
              runtime !== null && isRunning(runtime) ? runtime.sessionId : null
            }
            onToggleWorkspace={(id) => {
              const next = { ...expanded, [id]: expanded[id] !== true };
              setExpanded(next);
              onExpandedChange?.(
                Object.entries(next)
                  .filter(([, open]) => open)
                  .map(([key]) => key),
              );
            }}
            onSelectSession={(workspaceId, sessionId) => {
              setActiveWorkspaceId(workspaceId);
              setActiveSessionId(sessionId);
              const workspace = presented.find((item) => item.id === workspaceId);
              const session: SessionItem | undefined = workspace?.sessions.find(
                (item) => item.id === sessionId,
              );
              if (workspace !== undefined && session !== undefined) {
                onSelectSession?.({
                  workspaceId,
                  workspacePath: workspace.path,
                  sessionId,
                  destination: session.destination,
                  title: session.title,
                  channel: session.channel,
                  pending: session.pending, draft: session.draft,
                });
              }
              onSelectTab('chat');
            }}
            onTogglePin={(session) =>
              void mutate(() => ipc.setSessionPinned(session.id, !session.pinned), '置顶操作失败')
            }
            onToggleArchive={(session) => requestArchive(session)}
            onRenameSession={(sessionId, title) =>
              void mutate(() => ipc.renameSession(sessionId, title), '重命名失败')
            }
            onReorder={(workspaceId, orderedIds) => void reorderSessions(workspaceId, orderedIds)}
            onCreateSession={(workspaceId) => void createSession(workspaceId)}
            onRemoveWorkspace={requestRemoveWorkspace}
          />
        )}
      </div>

      <SidebarFooter
        mcpReading={mcpReading}
        skillCount={skillCount}
        onOpenSettings={onOpenSettings}
        onOpenConnection={onOpenConnection ?? onOpenSettings}
      />

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

      <ArchiveSessionDialog
        session={pendingArchive}
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
