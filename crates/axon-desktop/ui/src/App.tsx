import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CronJob, GatewayHealth, NavTab, SettingsSection, WorkspaceItem } from './types';
import * as ipc from './ipc';
import { listenLaneActivity, listenOpenLane } from './laneEvents';
import { useWindowTitle } from './useWindowTitle';
import { Sidebar } from './components/Sidebar';
import { BasicSettingsPanel, SETTINGS_SECTION_LABELS } from './components/BasicSettingsPanel';
import { GatewayUnconnected, GatewayUnavailableState } from './components/GatewayUnconnected';
import { ConversationView } from './components/conversation/ConversationView';
import { ErrorBoundary } from './components/conversation/ErrorBoundary';
import type { LaneSelection } from './conversation/useConversation';
import { clearedAtSettle, markForNotice } from './conversation/laneMarks';
import { sameRuntimeFacts, type LaneRuntimeFacts } from './conversation/runtimeFacts';
import { LoadingState } from './components/ui/surfaces';
import { WindowChrome } from './components/ui/WindowChrome';
import { CommandPalette, type CommandPaletteItem } from './components/ui/CommandPalette';
import { AutomationPage } from './components/AutomationPage';
import { MemoryPage } from './components/MemoryPage';
import { WorkspaceOverview } from './components/WorkspaceOverview';
import {
  FOCUS_COMPOSER_EVENT,
  FOCUS_COMPOSER_WINDOW_EVENT,
  OPEN_COMMAND_PALETTE_EVENT,
  installDesktopGuards,
} from './desktopGuards';
import { applyTheme, currentTheme, THEME_CHOICES } from './theme';
import { notifyDesktopReady, useWindowDynamicSurfaces } from './windowSurfaces';

/*
 * 应用外壳：左侧工作区与会话，右侧主区域，设置以浮层弹窗叠加。
 *
 * 壳层持有三件跨视图的状态：当前顶层视图、当前会话目标、右侧栏可见性。
 * 业务数据仍由各视图自行从后端读取，壳层不做缓存也不维护第二份事实。
 */

/** 网关心跳间隔。 */
const GATEWAY_HEARTBEAT_MS = 5000;
/**
 * 会话列表的静默刷新间隔。
 *
 * 会话可能由其它渠道产生（例如 TUI 新开一条），侧栏必须能自己发现；
 * 刷新全程不显示加载态、失败也不打扰用户，由下一次心跳统一报连接问题。
 */
const LANE_REFRESH_MS = 15000;
/** 侧栏折叠组合键。 */
const SIDEBAR_TOGGLE_KEY = 'b';
/** 设置弹窗组合键。 */
const SETTINGS_KEY = ',';
/**
 * 窗口窄于此值时侧栏自动收成 56px 图标轨。
 *
 * 桌面端没有响应式断点可用（整窗就是一屏），因此窗口宽度是唯一的外形事实：
 * 低于这条线时中间内容列已经放不下 280 + 320 两根栏，收起侧栏比压扁正文更可读。
 */
const AUTO_COLLAPSE_WIDTH = 1024;

export const App: React.FC = () => {
  const [currentTab, setCurrentTab] = useState<NavTab>('chat');
  /**
   * 用户手动开合的偏好；为空表示「还没表过态」，此时按窗口宽度自动决定。
   *
   * 偏好只活在本会话里：用户把窄窗里的侧栏手动展开之后，它会一直生效，
   * 窗口回宽也不会被自动值顶掉；反之亦然。
   */
  const [sidebarPreference, setSidebarPreference] = useState<boolean | null>(null);
  /** 窗口是否窄到该自动收起侧栏。 */
  const [autoCollapsed, setAutoCollapsed] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [health, setHealth] = useState<GatewayHealth | null>(null);
  const [connectionDismissed, setConnectionDismissed] = useState(false);
  /**
   * 触发「服务未启用」判定的那次调用失败原因。
   *
   * 探测结论说明「怎么了」，这一行说明「哪一次请求、什么原因」，统一弹窗与主区域
   * 的常驻状态都直接展示它，用户不必去翻各视图各自的报错。
   */
  const [lastFailure, setLastFailure] = useState<string | null>(null);
  const connectionRef = useRef<boolean | null>(null);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>('general');
  // 启动一律落在首屏：会话目标是本次使用的临时上下文，
  // 自动恢复上次的会话会把「打开应用」变成「接着上次干活」，与首屏语义冲突。
  const [selection, setSelection] = useState<LaneSelection | null>(null);
  const [configRevision, setConfigRevision] = useState(0);
  const [workspaces, setWorkspaces] = useState<WorkspaceItem[]>([]);
  /** 当前会话的运行时事实；归档前需要知道它是不是还在跑。 */
  const [laneRuntime, setLaneRuntime] = useState<LaneRuntimeFacts | null>(null);
  /**
   * 有新结果的会话：车道标识到终态。
   *
   * 应用在前台时它是系统通知的替代品，看过即清；收进托盘时两者同时出现。
   * 只存最近一次终态，不落盘：它是「这次打开应用期间发生了什么」的提示，
   * 跨启动保留会让旧标识永远挂在那里。
   */
  const [laneMarks, setLaneMarks] = useState<Record<string, string>>({});
  /** 命令面板是否打开。 */
  const [paletteOpen, setPaletteOpen] = useState(false);
  /** 订阅回调里要读当前选中会话，用 ref 避免每次选择变化都重订阅。 */
  const selectionRef = useRef<LaneSelection | null>(null);
  selectionRef.current = selection;
  /** 订阅回调里要读最新车道清单，理由同上。 */
  const workspacesRef = useRef<WorkspaceItem[]>([]);
  workspacesRef.current = workspaces;

  /** 选中会话即视为已查看：清掉它的状态标识。 */
  const selectLane = useCallback((next: LaneSelection | null) => {
    setSelection(next);
    if (next === null) return;
    setLaneMarks((current) => {
      if (current[next.laneId] === undefined) return current;
      const next_marks = { ...current };
      delete next_marks[next.laneId];
      return next_marks;
    });
  }, []);

  /*
   * 窗口宽度是这一层唯一的外形事实：窄窗自动收成 56px 图标轨，
   * 让中间内容列拿回整条宽度（页面内容自己滚动，不产生窗口级滚动条）。
   * 用 matchMedia 而不是监听 scroll：断点只在跨越时触发一次，也不参与滚动路径。
   */
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return undefined;
    const query = window.matchMedia(`(max-width: ${AUTO_COLLAPSE_WIDTH - 1}px)`);
    const apply = () => setAutoCollapsed(query.matches);
    apply();
    /*
     * 两套订阅口径：新引擎走 addEventListener，旧 WebKit（macOS 10.15 的 WKWebView）
     * 只有已废弃的 addListener。三平台都在支持范围内，因此两条路都要留。
     */
    const legacy = query as MediaQueryList & {
      addListener?: (listener: () => void) => void;
      removeListener?: (listener: () => void) => void;
    };
    if (typeof query.addEventListener === 'function') {
      query.addEventListener('change', apply);
      return () => query.removeEventListener('change', apply);
    }
    legacy.addListener?.(apply);
    return () => legacy.removeListener?.(apply);
  }, []);

  /** 实际开合：用户表过态就听用户的，没表过态才按窗口宽度自动决定。 */
  const isSidebarOpen = sidebarPreference ?? !autoCollapsed;
  const toggleSidebar = useCallback(
    () => setSidebarPreference(!isSidebarOpen),
    [isSidebarOpen],
  );

  /*
   * 通知点击回到对应会话。
   *
   * 系统通知与窗口之间没有直接回调：宿主在窗口重新获得焦点时把车道标识交下来，
   * 会话选择由界面完成（它持有真实工作区与会话清单）。标识不在清单里时什么也不做，
   * 绝不凭空造一条会话出来。
   */
  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void listenOpenLane((laneId) => {
      for (const workspace of workspacesRef.current) {
        const lane = workspace.lanes.find((item) => item.id === laneId);
        if (lane === undefined) continue;
        selectLane({
          workspaceId: workspace.id,
          workspacePath: workspace.path,
          laneId: lane.id,
          destination: lane.destination,
          title: lane.title,
          channel: lane.channel,
          pending: lane.pending,
        });
        return;
      }
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [selectLane]);

  // 轮次结束的推送：标记「有新结果」的会话。
  useEffect(() => {
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void listenLaneActivity((activity) => {
      /*
       * 待答与新结果是两回事，不能共用「看过即清」：判定见 conversation/laneMarks，
       * 那里同时是它唯一的测试入口。
       */
      const mark = markForNotice(activity, selectionRef.current?.laneId ?? null);
      if (mark === null) return;
      setLaneMarks((marks) => ({ ...marks, [mark.laneId]: mark.status }));
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else dispose = unlisten;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, []);

  /** 上一次探测是否仍在途：单次探测可长达数秒，心跳不能叠加第二次。 */
  const probingRef = useRef(false);

  /** 启动即探测网关；未连接时展示可执行的修复引导而不是空侧栏。 */
  const probeGateway = useCallback(async () => {
    probingRef.current = true;
    try {
      const result = await ipc.gatewayHealth();
      if (result.connected || connectionRef.current === true) setConnectionDismissed(false);
      // 连上了就没有「最近一次失败」可讲：留着它只会让下一次断连的弹窗显示旧原因。
      if (result.connected) setLastFailure(null);
      connectionRef.current = result.connected;
      setHealth(result);
    } catch {
      // 探测本身失败（例如桌面宿主不可用）时保持上一次结论，由引导页的重试接管。
    } finally {
      probingRef.current = false;
    }
  }, []);

  useEffect(() => {
    void probeGateway();
  }, [probeGateway]);

  // 心跳：连接正常时也持续复探，服务中途挂掉能立刻反映到界面与状态入口。
  // 单次探测最长可达数秒（探活预算加一次重试），因此在途时跳过这一拍，
  // 既不让探测互相叠加，也避免先发的旧结论覆盖后发的新结论。
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (probingRef.current) return;
      void probeGateway();
    }, GATEWAY_HEARTBEAT_MS);
    return () => window.clearInterval(timer);
  }, [probeGateway]);

  /*
   * 网络请求拦截器：任何一个后端调用报「服务未启用」，都立即复探并弹出统一弹窗。
   *
   * 不依赖各视图各自解释同一个断连原因，也不必等下一次心跳：服务刚被停掉时，
   * 用户看到的第一个失败就已经把结论带到这里。用户此前的手动关闭会被重新打开：
   * 服务又断了一次是新的信息，不该被上一次的「知道了」永久压住。
   */
  useEffect(
    () =>
      ipc.onBackendUnavailable((detail) => {
        setLastFailure(detail);
        setConnectionDismissed(false);
        void probeGateway();
      }),
    [probeGateway],
  );

  /** 工作区列表供主区域取当前工作区；侧栏自身仍独立持有一份用于渲染。 */
  const refreshWorkspaces = useCallback(async () => {
    if (connectionRef.current !== true) return;
    try {
      setWorkspaces(await ipc.fetchWorkspaces());
    } catch {
      // 连接问题由统一弹窗呈现。
    }
  }, []);

  useEffect(() => {
    void refreshWorkspaces();
  }, [refreshWorkspaces, selection?.laneId, health?.connected]);

  // 静默轮询：只更新数据，不动界面状态，因此不会打断正在输入或翻历史的用户。
  useEffect(() => {
    const timer = window.setInterval(() => void refreshWorkspaces(), LANE_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [refreshWorkspaces]);

  /** 主区域当前工作区：优先跟随选中的会话，否则取运行态标记的当前工作区。 */
  const activeWorkspace = useMemo(() => {
    if (selection !== null) {
      const matched = workspaces.find((item) => item.id === selection.workspaceId);
      if (matched !== undefined) return matched;
      // Cron 等独立会话的目录可以不在侧栏注册，仍须使用会话的真实工作区。
      if (selection.workspacePath) return {
        id: selection.workspaceId,
        name: selection.workspacePath.split(/[\\/]/).filter(Boolean).pop() ?? selection.workspacePath,
        path: selection.workspacePath,
        active: false,
        removable: false,
        lanes: [],
      } satisfies WorkspaceItem;
      return null;
    }
    return workspaces.find((item) => item.active) ?? workspaces[0] ?? null;
  }, [selection, workspaces]);

  const selectedLane = selection === null ? null :
    activeWorkspace?.lanes.find((lane) => lane.id === selection.laneId);
  const hasPendingQuestion = Boolean(
    (selectedLane?.pending_question_count ?? 0) > 0 ||
    (selection && laneMarks[selection.laneId]?.startsWith('question_')),
  );
  const selectedRunning = Boolean(
    selectedLane?.running ||
    (selection !== null && laneRuntime?.laneId === selection.laneId && laneRuntime.turnRunning),
  );
  const baseTitle = selectedLane?.title || selection?.title;
  // 待答与运行状态用文字前缀表达：窗口标题栏里不放彩色表情符号，与侧栏的状态语言同源。
  const windowTitle = selection === null
    ? undefined
    : hasPendingQuestion
      ? `待答 · ${baseTitle}`
      : selectedRunning
        ? `运行中 · ${baseTitle}`
        : baseTitle;

  useWindowTitle(windowTitle);

  /*
   * 桌面外壳三件装配（互不阻塞，各自回收自己的监听）：
   * 1. 浏览器特征压制：右键菜单、刷新、打印、查看源码与开发者工具；
   * 2. 窗口动态表面：焦点与最大化只在侧栏容器上落属性，不动根节点；
   * 3. 首帧就绪：渲染完成才告诉宿主显示窗口，白底闪烁与窗口瞬移都不会出现。
   */
  useEffect(() => installDesktopGuards(), []);
  // 侧栏在展开与图标轨之间切换时会整棵换掉，容器换了要重新落一次状态。
  useWindowDynamicSurfaces(`${isSidebarOpen}:${currentTab}`);
  useEffect(() => {
    void notifyDesktopReady();
  }, []);

  /*
   * 全局快捷键（Ctrl + Alt + Space）唤回窗口后的落点：回到对话页，并把焦点交给输入坞。
   * 宿主只负责显示窗口与推送，具体落点由界面决定（它才知道当前视图与输入坞在哪）。
   */
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void import('@tauri-apps/api/event')
      .then(({ listen }) => listen(FOCUS_COMPOSER_EVENT, () => {
        setCurrentTab('chat');
        window.dispatchEvent(new CustomEvent(FOCUS_COMPOSER_WINDOW_EVENT));
      }))
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** 命令面板的打开手势来自全局压制器（Ctrl + K 与 Ctrl + F 都翻译到同一条事件）。 */
  useEffect(() => {
    const openPalette = () => setPaletteOpen(true);
    window.addEventListener(OPEN_COMMAND_PALETTE_EVENT, openPalette);
    return () => window.removeEventListener(OPEN_COMMAND_PALETTE_EVENT, openPalette);
  }, []);

  /*
   * 待答标识的清除：这条会话已经没有在等回答的提问了。
   *
   * 标识说明的是「有事等你做」而不是「有新结果没看」，因此它不随开关会话消失，
   * 只随提问结算消失；结算事实来自会话视图，因此前台一问完标识就落。
   */
  useEffect(() => {
    const laneId = clearedAtSettle(
      laneMarks,
      laneRuntime?.laneId ?? null,
      laneRuntime?.hasPendingQuestion ?? true,
    );
    if (laneId === null) return;
    setLaneMarks((marks) => {
      const next = { ...marks };
      delete next[laneId];
      return next;
    });
  }, [laneMarks, laneRuntime]);

  /*
   * 登记当前会话：系统通知的标题与点击跳转都按它归属，会话一换就必须跟着写。
   * 标题取车道清单里的最新值（重命名后仍正确），清单里还没有这条会话时回落到
   * 选中项自带的名字；没有选中会话时显式清空，免得通知挂到上一条会话上。
   */
  useEffect(() => {
    const laneId = selection?.laneId ?? '';
    const title = selectedLane?.title ?? selection?.title ?? '';
    void ipc.conversationSetActiveLane(laneId, title).catch(() => undefined);
  }, [selection, selectedLane?.title]);

  /** 打开设置并定位到指定分区。 */
  const openSettingsAt = useCallback((target: SettingsSection) => {
    setSettingsSection(target);
    setIsSettingsOpen(true);
  }, []);

  /**
   * 命令面板的全部候选。
   *
   * 每一项都落到真实存在的动作上：跳转改顶层视图、设置分区开弹窗、主题走同一份
   * 持久化接口、工作区与会话直接切订阅目标。没有占位项，也没有「未来会有」的入口。
   */
  const paletteItems = useMemo<CommandPaletteItem[]>(() => {
    const navigation: CommandPaletteItem[] = [
      { id: 'nav-chat', label: '前往对话', group: '跳转', run: () => setCurrentTab('chat') },
      {
        id: 'nav-workspace',
        label: '前往工作区',
        group: '跳转',
        run: () => setCurrentTab('workspace'),
      },
      {
        id: 'nav-automation',
        label: '前往自动化任务',
        group: '跳转',
        run: () => setCurrentTab('automation'),
      },
      {
        id: 'nav-memory',
        label: '前往记忆管理',
        group: '跳转',
        run: () => setCurrentTab('memory'),
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
          setCurrentTab('chat');
        },
      })),
    );

    return [...navigation, ...settings, ...themes, ...lanes];
  }, [openSettingsAt, selectLane, workspaces]);


  /**
   * 接收会话视图上报的运行态事实。
   *
   * 值没变就保留原引用：上报方每轮都会新建一个事实对象，照单全收地换引用会让
   * App 每轮重渲染，而重渲染又让上报方的回调换引用、再次上报，形成自激循环。
   * 这里按值比较后返回原引用，React 会直接跳过这次更新，循环因此断开。
   */
  const applyRuntimeFacts = useCallback((facts: LaneRuntimeFacts) => {
    setLaneRuntime((previous) => (sameRuntimeFacts(previous, facts) ? previous : facts));
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const modifier = event.ctrlKey || event.metaKey;
      if (!modifier) return;
      if (event.key.toLowerCase() === SIDEBAR_TOGGLE_KEY) {
        event.preventDefault();
        toggleSidebar();
      } else if (event.key === SETTINGS_KEY) {
        event.preventDefault();
        setIsSettingsOpen(true);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [toggleSidebar]);

  return (
    // 外层是沉底托盘（bg-sidebar），主工作区是嵌在托盘里的圆角舞台（Inset Sheet）。
    // 托盘同时承载系统材质：材质可用时它取半透色，失焦或最大化立刻退回实色。
    <div className="app-shell-tray h-full w-full flex overflow-clip">
      <Sidebar
        currentTab={currentTab}
        onSelectTab={setCurrentTab}
        isOpen={isSidebarOpen}
        onToggleSidebar={toggleSidebar}
        onOpenSettings={() => setIsSettingsOpen(true)}
        health={health}
        onSelectLane={selectLane}
        selectedLaneId={selection?.laneId ?? null}
        laneMarks={laneMarks}
        runtime={laneRuntime}
      />

      {/*
        主舞台。窗口三键画在这块舞台的顶栏里，而不是托盘外框上：按钮与标题栏同属
        一个表面，才读得成「一体的窗口」，否则它们像贴在边框外的附件。
        舞台自己只保留一圈发丝描边，不再叠投影：外框已有系统窗口阴影，
        两道阴影并排会把窗口读成框套框。
      */}
      <div className="relative flex-1 min-w-0 my-1.5 mr-1.5 ml-0 flex flex-col rounded-[12px] bg-canvas hairline border-border-hairline overflow-clip">
        <WindowChrome />
        {/*
          静态微晶纹理：一张 64x64 的 Base64 PNG 平铺，单次采样、零实时滤镜。
          它盖在画布之上、正文之下感觉不到存在，但能抹掉大面积纯色与毛玻璃背景下的
          色彩断层，读起来像阳极氧化铝表面而不是一块纯色塑料。
        */}
        <span aria-hidden="true" className="noise-underlay absolute inset-0" />
        {/*
          视图切换只动 opacity 与 transform：`key` 换掉整块内容，入场动画由
          `.view-swap-in` 承担，因此切页没有白屏，也不会有上一屏残影。
        */}
        <div key={currentTab} className="view-swap-in flex-1 min-h-0 min-w-0 flex">
        {health === null ? (
          <main className="flex-1 min-w-0 h-full flex flex-col bg-canvas">
            <div data-tauri-drag-region className="h-10 shrink-0" />
            <div className="flex-1 flex items-center justify-center">
              <LoadingState label="正在连接 Axon 后台服务" />
            </div>
          </main>
        ) : !health.connected ? (
          <main className="flex-1 h-full flex flex-col overflow-clip bg-canvas">
            <div data-tauri-drag-region className="h-10 shrink-0" />
            {/* 弹窗可以被关掉，主区域不能因此变成一片空白：这里给出同样的结论与重试入口。 */}
            <div className="flex-1 min-h-0 flex flex-col">
              <GatewayUnavailableState
                health={health}
                detail={lastFailure}
                onRetry={async () => {
                  await probeGateway();
                  if (connectionRef.current === true) await refreshWorkspaces();
                }}
                onOpenSettings={() => openSettingsAt('remote')}
              />
            </div>
          </main>
        ) : currentTab === 'workspace' ? (
          <WorkspaceOverview />
        ) : currentTab === 'automation' ? (
          <AutomationPage onOpenSession={(job: CronJob) => {
            if (!job.workspaceId || !job.sessionId) return;
            selectLane({
              workspaceId: job.workspaceId,
              workspacePath: job.workspacePath ?? '',
              laneId: job.sessionId,
              destination: job.destination ?? '',
              title: job.name,
              channel: 'cron',
              fixedModel: job.model,
              fixedReasoningEffort: job.reasoningEffort,
            });
            setCurrentTab('chat');
          }} />
        ) : currentTab === 'memory' ? (
          <MemoryPage connected={health.connected} />
        ) : (
          <ErrorBoundary>
            <ConversationView
              selection={selection}
              workspace={activeWorkspace}
              configRevision={configRevision}
              onSelectLane={selectLane}
              onRuntimeFacts={applyRuntimeFacts}
            />
          </ErrorBoundary>
        )}
        </div>
      </div>

      {health !== null && !health.connected && (
        <GatewayUnconnected
          health={health}
          detail={lastFailure}
          open={!connectionDismissed && !isSettingsOpen}
          onClose={() => setConnectionDismissed(true)}
          onRetry={async () => {
            await probeGateway();
            if (connectionRef.current === true) await refreshWorkspaces();
          }}
          onOpenSettings={() => {
            setConnectionDismissed(true);
            openSettingsAt('remote');
          }}
        />
      )}

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        items={paletteItems}
      />

      <BasicSettingsPanel
        open={isSettingsOpen}
        onClose={() => {
          setIsSettingsOpen(false);
          setConfigRevision((value) => value + 1);
          void probeGateway();
        }}
        initialSection={settingsSection}
      />
    </div>
  );
};
