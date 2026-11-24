import React from 'react';
import { RailSplitter, RailTabBody, RailTabStrip } from './RailSurface';
import { activePane, canCloseTab, DEFAULT_PAGE, type RailLayout } from './model';
import { fileAddress } from './registry';
import type { RailController } from './useRailLayout';
import type { ChangedFile } from '../conversation/transcript';

/*
 * 右侧栏：按会话持有的停靠面。
 *
 * 两种呈现形态共用同一棵内容树：`push` 贴靠右列，`fullscreen` 覆盖窗口。
 * 宽度、分栏比例、tab 集合与展开态都来自控制器这一份状态，
 * 组件本身不持有任何布局事实。
 *
 * 收起不卸载内容：占位宽度归零，面板本体贴右缘整体右移一栏宽滑出去，
 * 滑完之后由外层把可见性翻掉。因此文件树的展开态与预览的滚动位置都留着，
 * 开合是同一个手势的两个方向，两种形态下都是「从那条边滑出、滑回」。
 */

/** 栏宽持久化键。 */
const WIDTH_KEY = 'axon.rail.width.v1';

/*
 * 栏宽与既有工作台产品同一套列几何：首次展开取帧宽的比例，拖拽后被夹取在
 * 「下限」与「帧宽上限」之间，并且任何时候都不把中间内容列压到最小宽度以下。
 * 帧宽窄到一定程度时，展开的面板自动全屏，不挤中间列。
 */

/** 栏宽下限，与既有工作台产品的右栏夹取下限一致。 */
const MIN_WIDTH = 300;

/** 栏宽上限：帧宽的比例。 */
const MAX_WIDTH_RATIO = 0.7;

/** 首次展开的栏宽：帧宽的比例。 */
const DEFAULT_WIDTH_RATIO = 0.45;

/** 中间内容列的最小宽度；面板不得把它压到这条线以下。 */
const CENTER_MIN = 400;

/** 帧宽低于此值时，展开的面板自动全屏。 */
const AUTO_FULLSCREEN_WIDTH = 768;

/*
 * 分栏可容判定：一格的硬需求是「一个最小宽度的 tab 胶囊 + 格内边距 +
 * 这一格自己的固定控件 + 分栏条」，两格相加才是这条栏能分栏的宽度。
 *
 * 三枚 28px 控件加两处 4px 间隙正好 92px；tab 胶囊最小 100px；格内边距 12px。
 * 末格控件只在最后一格出现，但两格都要按同一份硬需求算：分栏之后两个格子
 * 会平分这条栏，任何一格低于这条线都读不出内容，点了也只能看见两条细缝。
 */

/** 一个 tab 胶囊的最小可用宽度。 */
const CHIP_MIN_WIDTH = 100;

/** 一格的左右内边距与元素间隙。 */
const PANE_INSET = 12;

/** 分栏条宽度（两个格子之间的拖拽把手）。 */
const SPLITTER_WIDTH = 4;

/** 末格固定控件：分栏、全屏、收起三枚 28px 按钮加它们之间的 4px 间隙。 */
const PANE_CONTROLS_WIDTH = 3 * 28 + 2 * 4;

/** 一格的硬需求。 */
const PANE_MIN_WIDTH = CHIP_MIN_WIDTH + PANE_INSET + PANE_CONTROLS_WIDTH + SPLITTER_WIDTH;

/** 两格阈值：低于它时分栏入口禁用，并把需要多宽写进提示。 */
export const SPLIT_MIN_WIDTH = PANE_MIN_WIDTH * 2;

/** 读回用户拖过的栏宽；没拖过返回 null，由比例决定。 */
function loadWidth(): number | null {
  try {
    const raw = window.localStorage.getItem(WIDTH_KEY);
    if (raw === null) return null;
    const parsed = Number.parseInt(raw, 10);
    return Number.isFinite(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

/** 量出面板所在那一行的宽度：宽度规则全部以它为基准。 */
function useFrameWidth(ref: React.RefObject<HTMLElement | null>): number {
  const [frame, setFrame] = React.useState(() => window.innerWidth);
  React.useEffect(() => {
    const host = ref.current?.parentElement;
    if (host === null || host === undefined) return undefined;
    const measure = () => setFrame(host.clientWidth);
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(host);
    return () => observer.disconnect();
  }, [ref]);
  return frame;
}

/** 把请求宽度夹进这一帧允许的范围。 */
function clampForFrame(requested: number, frame: number): number {
  const available = Math.max(0, frame - CENTER_MIN);
  const upper = Math.min(frame * MAX_WIDTH_RATIO, available);
  return Math.round(Math.min(upper, Math.max(MIN_WIDTH, requested)));
}

/*
 * 面板外壳的两种形态共用同一个节点：切形态只换类名，内容树整体留在原处，
 * 否则手动切一次全屏也会把 tab 的展开态与预览的滚动位置弄丢。
 */
/** 两种形态共有的外壳。 */
const PANEL_SHELL = 'flex flex-col bg-surface';

/** 贴靠形态：贴右缘的实体列。 */
const PANEL_DOCKED = 'absolute inset-y-0 right-0 hairline-l border-border-hairline';

/** 全屏形态：覆盖窗口，不占正文宽度。 */
const PANEL_FULLSCREEN = 'fixed inset-0 z-[900]';

/*
 * 开合位移：只动 transform（Tailwind 的 translate 属性），时长与缓动取既有工作台产品
 * 右栏的同一组值。宽度不做过渡：正文列是瞬间让出与收回的，动的是内容。
 */
const PANEL_SLIDE = 'transition-transform duration-300 ease-in-out';

/*
 * 收起态：右移一栏宽，正好落在窗口右缘之外；同时断开指针，拖拽把手会探出窗口右缘
 * 几个像素，收起时不该还能按到。
 */
const PANEL_OFF_EDGE = 'translate-x-full pointer-events-none';

/*
 * 焦点续接只认一件事：刚才获得焦点的节点随这次提交从文档里消失，并且这个节点属于面板
 * （或就是展开入口）。于是点展开入口（收起态才画，点完自己就没了）把焦点交给面板，
 * 关掉焦点所在的 tab 时交给还活着的格；而面板自己的收起按钮、正文里点开文件的入口
 * 都还在文档里，一律不抢焦点。
 */

/** 与浏览器 Tab 序列同口径的可聚焦元素。 */
const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** 容器里第一个排得下的可聚焦元素。 */
function firstFocusable(root: HTMLElement | null): HTMLElement | null {
  if (root === null) return null;
  for (const element of root.querySelectorAll<HTMLElement>(FOCUSABLE)) {
    if (element.getClientRects().length > 0) return element;
  }
  return null;
}

/** 焦点落点：活跃格的活跃 tab；正文里没有可聚焦元素时退到它的胶囊。 */
function focusActiveTab(host: HTMLElement, layout: RailLayout): void {
  const pane = activePane(layout);
  if (pane === null) return;
  const scope = host.querySelector<HTMLElement>(`[data-rail-pane="${CSS.escape(pane.id)}"]`);
  const body = scope?.querySelector<HTMLElement>('[data-rail-body]') ?? null;
  const chip =
    pane.activeId === null
      ? null
      : (scope?.querySelector<HTMLElement>(`[data-rail-tab="${CSS.escape(pane.activeId)}"]`) ?? null);
  const target = firstFocusable(body) ?? chip?.querySelector<HTMLElement>('button') ?? null;
  target?.focus({ preventScroll: true });
}

export interface SidebarRightProps {
  controller: RailController;
  /** 工作区根目录；为空时文件类 tab 给出可执行的空态。 */
  workspaceRoot: string | null;
  /** 取某一轮的改动文件；改动对比 tab 的正文读这一份。 */
  changesFor: (turnId: string) => ChangedFile[] | null;
}

export const SidebarRight: React.FC<SidebarRightProps> = ({ controller, workspaceRoot, changesFor }) => {
  const { layout } = controller;
  const [resizingWidth, setResizingWidth] = React.useState(false);
  const [resizingSplit, setResizingSplit] = React.useState(false);
  const splitRef = React.useRef<HTMLDivElement>(null);
  /** 占位与定位的宿主：栏宽在这里，因此量帧宽要量它的父元素，也就是面板所在的那一行。 */
  const hostRef = React.useRef<HTMLDivElement>(null);
  const frame = useFrameWidth(hostRef);
  const [requestedWidth, setRequestedWidth] = React.useState<number | null>(() => loadWidth());
  // 拖过就按拖出来的值，没拖过按帧宽比例；两者都夹在同一套范围里。
  const width = clampForFrame(requestedWidth ?? frame * DEFAULT_WIDTH_RATIO, frame);
  // 帧宽不够时展开的面板自动全屏，这与用户手动选的全屏是两件事。
  const autoFullscreen = frame < AUTO_FULLSCREEN_WIDTH;
  const expanded = layout.expanded;
  // 收起一律走贴靠形态：手动全屏后收起时 fullscreen 这一位仍然保留（下次展开回到全屏），
  // 但收起态不能是全屏，否则会留下一层盖住整窗的浮层。
  const fullscreen = expanded && (layout.fullscreen || autoFullscreen);

  React.useEffect(() => {
    if (requestedWidth === null) return;
    try {
      window.localStorage.setItem(WIDTH_KEY, String(requestedWidth));
    } catch {
      // 存储不可用不影响使用。
    }
  }, [requestedWidth]);

  /*
   * 展开态但没有 tab：补上默认页，避免出现一块什么都没有的面板。
   * 走布局效应：展开这一提交就要有内容，同一提交里的焦点续接才接得上。
   */
  React.useLayoutEffect(() => {
    if (layout.expanded && layout.panes.length === 0) controller.openPage(DEFAULT_PAGE.kind);
  }, [controller, layout.expanded, layout.panes.length]);

  /** 最后一次获得焦点的节点，以及它当时是否属于面板（节点被摘掉之后就问不出来了）。 */
  const lastFocused = React.useRef<{ element: HTMLElement; ours: boolean } | null>(null);

  React.useEffect(() => {
    const capture = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof HTMLElement)) return;
      const host = hostRef.current;
      lastFocused.current = {
        element: target,
        ours:
          (host !== null && host.contains(target)) ||
          target.closest('[data-rail-expand]') !== null,
      };
    };
    document.addEventListener('focusin', capture);
    return () => document.removeEventListener('focusin', capture);
  }, []);

  // 每次提交判一次：只有「焦点节点消失」这一种情况才动手，其余一律不抢焦点。
  React.useLayoutEffect(() => {
    const previous = lastFocused.current;
    if (previous === null || previous.element.isConnected) return;
    lastFocused.current = null;
    if (!previous.ours) return;
    // 焦点已经落到别处：那是用户自己的选择，不动。
    const active = document.activeElement;
    if (active !== null && active !== document.body) return;
    const host = hostRef.current;
    if (!expanded) {
      // 整列已经收起：内部没有可落点，这一手势的回头路是主区右上角的展开入口。
      document.querySelector<HTMLElement>('[data-rail-expand]')?.focus({ preventScroll: true });
      return;
    }
    if (host !== null) focusActiveTab(host, layout);
  });

  React.useEffect(() => {
    if (!resizingWidth) return undefined;
    const handleMove = (event: MouseEvent) => {
      setRequestedWidth(clampForFrame(frame - event.clientX, frame));
    };
    const handleUp = () => setResizingWidth(false);
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
  }, [frame, resizingWidth]);

  React.useEffect(() => {
    if (!resizingSplit) return undefined;
    const handleMove = (event: MouseEvent) => {
      const box = splitRef.current?.getBoundingClientRect();
      if (box === undefined || box.width <= 0) return;
      controller.resize((event.clientX - box.left) / box.width);
    };
    const handleUp = () => setResizingSplit(false);
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
  }, [controller, resizingSplit]);

  const openFile = React.useCallback(
    (path: string) => controller.openResource(fileAddress(path), { line: undefined }),
    [controller],
  );

  const panes = layout.panes;
  // 两格都要放得下才有意义：窄栏里分栏只会得到两条读不了的细缝。
  const canSplit = width >= SPLIT_MIN_WIDTH;
  const splitHint = canSplit
    ? '分栏'
    : `分栏需要面板至少 ${SPLIT_MIN_WIDTH}px（当前 ${Math.round(width)}px），先把它拖宽`;
  const strip = (index: number) => {
    const pane = panes[index];
    if (pane === undefined) return null;
    return (
      <RailTabStrip
        pane={pane}
        last={index === panes.length - 1}
        paneCount={panes.length}
        dragging={fullscreen}
        canSplit={canSplit}
        splitHint={splitHint}
        canClose={(tabId) => canCloseTab(layout, tabId)}
        onFocus={controller.focusTab}
        onClose={controller.closeTab}
        onAdd={() => controller.openPage(DEFAULT_PAGE.kind, { revealIfOpened: false })}
        onSplit={controller.splitPane}
        onDock={controller.dockPane}
        onToggleFullscreen={() => controller.toggleFullscreen(autoFullscreen)}
        onToggleExpanded={controller.toggleExpanded}
      />
    );
  };

  const surface = (
    <div ref={splitRef} className="flex-1 min-h-0 flex">
      {panes.map((pane, index) => (
        <React.Fragment key={pane.id}>
          {index > 0 && <RailSplitter onStart={() => setResizingSplit(true)} />}
          <div
            data-rail-pane={pane.id}
            className="min-w-0 flex flex-col bg-surface"
            style={
              panes.length === 1
                ? { flex: '1 1 0%' }
                : index === 0
                  ? { flex: `0 0 ${(layout.splitRatio * 100).toFixed(2)}%` }
                  : { flex: '1 1 0%' }
            }
          >
            {strip(index)}
            <div data-rail-body className="flex-1 min-h-0 flex flex-col">
              {pane.tabs
                .filter((tab) => tab.id === pane.activeId)
                .map((tab) => (
                  <RailTabBody
                    key={`${tab.id}:${tab.revision}`}
                    tab={tab}
                    root={workspaceRoot}
                    activePath={controller.activePath}
                    controller={controller}
                    onOpenFile={openFile}
                    changesFor={changesFor}
                  />
                ))}
            </div>
          </div>
        </React.Fragment>
      ))}
    </div>
  );

  /*
   * 占位与面板分成两层：外层只在正文里占出栏宽（收起归零）并给面板定位，
   * 面板本体保持真实栏宽贴右缘。宽度归零因此不挪动面板的屏幕位置，
   * 收起时看得见的那一段就只有位移本身。
   *
   * 外层顺手承担可见性：visible 到 hidden 在时长走完之后才翻（滑出全程看得见），
   * hidden 到 visible 立刻翻（滑入不迟一帧），与既有工作台产品右栏同一套口径。
   * 减少动效时全站把时长归零，翻面变成瞬时，收起位置照样成立。
   */
  return (
    <div
      ref={hostRef}
      style={{ width: fullscreen || !expanded ? 0 : width }}
      className={`relative shrink-0 h-full transition-[visibility] duration-300 ${
        expanded ? 'visible' : 'invisible'
      }${fullscreen ? '' : ' z-10'}`}
    >
      <aside
        aria-label="会话资源面板"
        // inert 写在 aria-hidden 前面：它先把焦点从即将隐藏的内容里摘出来，
        // 随后落地的 aria-hidden 才不会被浏览器以「后代仍持有焦点」为由拦下。
        inert={!expanded}
        aria-hidden={!expanded || undefined}
        style={fullscreen ? undefined : { width }}
        className={`${PANEL_SHELL} ${
          fullscreen
            ? PANEL_FULLSCREEN
            : `${PANEL_DOCKED} ${PANEL_SLIDE} ${expanded ? 'translate-x-0' : PANEL_OFF_EDGE}`
        }`}
      >
        {surface}
        {!fullscreen && (
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label="调整面板宽度"
            onMouseDown={(event) => {
              event.preventDefault();
              setResizingWidth(true);
            }}
            onDoubleClick={() => setRequestedWidth(null)}
            className="absolute top-0 left-0 w-2 -ml-1 h-full cursor-col-resize touch-none z-20"
          />
        )}
      </aside>
    </div>
  );
};
