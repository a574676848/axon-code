/**
 * 右侧栏布局模型。
 *
 * 每个会话一份停靠面：若干 tab 分在最多两格里，一格一个选中项。所有动作都是
 * 纯函数（旧布局进、新布局出），持久化只发生在动作之后，因此「界面上看到的」
 * 与「存下来的」永远来自同一份计算结果，不存在就地编辑出的第二份事实。
 *
 * 布局整体按会话持久化：停靠面内容（tab 集合、分栏比例、序号）与开合、全屏形态
 * 都随会话存下来，重启后回到离开时的样子；从未打开过的会话是折叠的默认形态。
 *
 * 开合时机与 既有工作台产品 的右侧栏同一套规则：默认折叠；只有用户主动展开、或打开一个
 * 资源时才展开；打开内容不先塞默认页；关掉整列最后一个 tab 即收起并退出全屏；
 * 独占停靠面的默认页不可关闭（关它等于关整列，这一手势归收起按钮）。
 */

import type { ResourceParams } from './registry';

/** 一格里最多保留的 tab 数，防止长会话把布局撑成不可用的长条。 */
const MAX_TABS_PER_PANE = 12;

/**
 * 停靠面的默认页：首次展开与分栏时播种的那一页。
 *
 * 引导页是这一列的立足点：它同时是一扇门（列出可以打开的东西）与一个不可关闭的默认页，
 * 与既有工作台产品的引导页同一条规则（见 `canCloseTab`）。
 */
export const DEFAULT_PAGE = { kind: 'guide', title: '开始' } as const;

/** 单格 tab 身份与导航。 */
export interface RailTab {
  /** 本次打开的实例身份；同一个资源重复打开时复用。 */
  id: string;
  /** tab 类型。 */
  kind: string;
  /** 资源地址；页类型为 null。 */
  address: string | null;
  /** 打开时固定的标题。 */
  title: string;
  /** 导航参数。 */
  params: ResourceParams;
  /** 该实例的导航次数，用于让正文知道「又导航到了同一个地址」。 */
  revision: number;
  /**
   * 用户为这个 tab 选定的呈现方式；未选过时为空，正文取第一个候选。
   *
   * 它是**这个 tab 的事实**而不是组件的临时状态：切 tab、收起面板、重启之后
   * 都要回到用户选过的那一种看法，因此跟着布局一起持久化。
   */
  viewer?: string;
}

/** 一格。 */
export interface RailPane {
  id: string;
  tabs: RailTab[];
  activeId: string | null;
}

/** 新建一个默认页 tab。 */
function defaultTab(id: string): RailTab {
  return {
    id,
    kind: DEFAULT_PAGE.kind,
    address: null,
    title: DEFAULT_PAGE.title,
    params: {},
    revision: 0,
  };
}

/** 一个会话的停靠面。 */
export interface RailLayout {
  panes: RailPane[];
  /** 面板是否展开。 */
  expanded: boolean;
  /** 是否全屏覆盖。 */
  fullscreen: boolean;
  /** 分栏比例，0.5 为均分。 */
  splitRatio: number;
  /** 已铸造的实例序号，保证 id 在回放时稳定。 */
  sequence: number;
}

/** 打开请求。 */
export interface OpenRequest {
  kind: string;
  address: string | null;
  title: string;
  params?: ResourceParams;
  /** 指定落到哪一格；缺省落到当前活跃格。 */
  paneId?: string;
  /** 已展示同一 (kind, address) 时是否聚焦而不是新增。 */
  revealIfOpened?: boolean;
  /** 该类型能否多开；页类型专用（资源类型天然按地址去重）。 */
  multiple?: boolean;
}

/** 空停靠面：收起且为空，首次展开时才播种。 */
export const EMPTY_LAYOUT: RailLayout = {
  panes: [],
  expanded: false,
  fullscreen: false,
  splitRatio: 0.5,
  sequence: 0,
};

/** 活跃格：优先最后一格。 */
export function activePane(layout: RailLayout): RailPane | null {
  return layout.panes.length === 0 ? null : (layout.panes[layout.panes.length - 1] ?? null);
}

/** 当前活跃 tab。 */
export function activeTab(layout: RailLayout): RailTab | null {
  const pane = activePane(layout);
  if (pane === null) return null;
  return pane.tabs.find((tab) => tab.id === pane.activeId) ?? null;
}

/** 找到承载某个 tab 的格。 */
function paneOf(layout: RailLayout, tabId: string): RailPane | null {
  return layout.panes.find((pane) => pane.tabs.some((tab) => tab.id === tabId)) ?? null;
}

/** 替换某一格。 */
function withPane(layout: RailLayout, pane: RailPane): RailLayout {
  return {
    ...layout,
    panes: layout.panes.map((current) => (current.id === pane.id ? pane : current)),
  };
}

/** 播种默认页：展开且为空的停靠面填入它，收起时留空等下一次展开。 */
function seed(layout: RailLayout): RailLayout {
  if (layout.panes.length > 0) return layout;
  const sequence = layout.sequence + 1;
  const tab = defaultTab(`tab-${sequence}`);
  return {
    ...layout,
    sequence,
    panes: [{ id: 'pane-1', tabs: [tab], activeId: tab.id }],
  };
}

/** 确保停靠面有一格可落：打开内容不先塞默认页，空列先得到一格空面板。 */
function seedPane(layout: RailLayout): RailLayout {
  if (layout.panes.length > 0) return layout;
  return { ...layout, panes: [{ id: 'pane-1', tabs: [], activeId: null }] };
}

/**
 * 打开一个 tab。
 *
 * 新展开的面板只显示它打开的东西：默认页只在「展开后仍然为空」时播种，
 * 因此从会话里点开一个文件得到的是那一个文件，而不是默认页加文件两个 tab。
 */
export function open(layout: RailLayout, request: OpenRequest): RailLayout {
  const base = seedPane(layout);
  const target = request.paneId ?? activePane(base)?.id ?? 'pane-1';
  const pane = base.panes.find((current) => current.id === target) ?? base.panes[0];
  if (pane === undefined) return base;

  if (request.address !== null && request.revealIfOpened !== false) {
    const existing = base.panes
      .flatMap((current) => current.tabs)
      .find((tab) => tab.kind === request.kind && tab.address === request.address);
    if (existing !== undefined) {
      const owner = paneOf(base, existing.id);
      if (owner !== null) {
        return {
          ...withPane(base, {
            ...owner,
            tabs: owner.tabs.map((tab) =>
              tab.id === existing.id
                ? { ...tab, params: request.params ?? {}, revision: tab.revision + 1 }
                : tab,
            ),
            activeId: existing.id,
          }),
          expanded: true,
        };
      }
    }
  }

  // 页类型没有资源身份，去重口径是「同一格里的同一 kind」；能多开的类型（终端）不吃这一条。
  if (request.address === null && request.multiple !== true) {
    const existing = pane.tabs.find((tab) => tab.kind === request.kind);
    if (existing !== undefined) {
      return { ...withPane(base, { ...pane, activeId: existing.id }), expanded: true };
    }
  }

  const sequence = base.sequence + 1;
  const tab: RailTab = {
    id: `tab-${sequence}`,
    kind: request.kind,
    address: request.address,
    title: request.title,
    params: request.params ?? {},
    revision: 0,
  };
  const tabs = [...pane.tabs, tab].slice(-MAX_TABS_PER_PANE);
  return {
    ...withPane(base, { ...pane, tabs, activeId: tab.id }),
    sequence,
    expanded: true,
  };
}

/**
 * 一个 tab 能不能关。
 *
 * 停靠面只剩这一个 tab 且它是默认页时不能关：默认页是这一列的立足点，关掉它
 * 等于关掉整列，因此这一手势归收起按钮，控件上也不画关闭入口（既有工作台产品 的引导页
 * 同一条规则）。资源 tab 独占时照常可关，关掉即收起整列。
 */
export function canCloseTab(layout: RailLayout, tabId: string): boolean {
  const pane = paneOf(layout, tabId);
  const tab = pane?.tabs.find((current) => current.id === tabId);
  if (pane === null || tab === undefined) return false;
  const sole = pane.tabs.length === 1 && layout.panes.length === 1;
  return !(sole && tab.kind === DEFAULT_PAGE.kind);
}

/**
 * 用一个新的页替换掉当前活跃格的活跃 tab。
 *
 * 引导页是一扇门：点它的入口就是「这一页让位给那个类型」。替换必须在同一步里完成：
 * 先关后开会让中间那一帧变成空格子，面板宽度与滚动位置都会跟着抖一下。
 * 没有活跃 tab 时退化成一次普通打开，不留下空格。
 */
export function replace(layout: RailLayout, request: OpenRequest): RailLayout {
  const pane = activePane(layout);
  const index = pane === null || pane.activeId === null
    ? -1
    : pane.tabs.findIndex((tab) => tab.id === pane.activeId);
  if (pane === null || index < 0) return open(layout, request);
  const sequence = layout.sequence + 1;
  const tab: RailTab = {
    id: `tab-${sequence}`,
    kind: request.kind,
    address: request.address,
    title: request.title,
    params: request.params ?? {},
    revision: 0,
  };
  const tabs = [...pane.tabs.slice(0, index), tab, ...pane.tabs.slice(index + 1)];
  return {
    ...withPane(layout, { ...pane, tabs, activeId: tab.id }),
    sequence,
    expanded: true,
  };
}

/**
 * 关闭一个 tab；关掉某格最后一个 tab 时并掉该格，整列空了就收起。
 *
 * 关掉整列最后一个 tab 就是收起整列，因此同时退出全屏：否则下一次展开会把整个
 * 窗口让给刚播种的默认页。收起之后这一列保持空，播种等下一次展开。
 */
export function close(layout: RailLayout, tabId: string): RailLayout {
  if (!canCloseTab(layout, tabId)) return layout;
  const pane = paneOf(layout, tabId);
  if (pane === null) return layout;
  const tabs = pane.tabs.filter((tab) => tab.id !== tabId);
  if (tabs.length === 0) {
    const panes = layout.panes.filter((current) => current.id !== pane.id);
    if (panes.length === 0) return { ...layout, panes, expanded: false, fullscreen: false };
    return { ...layout, panes };
  }
  const activeId =
    pane.activeId === tabId
      ? (tabs[Math.max(0, pane.tabs.findIndex((tab) => tab.id === tabId) - 1)]?.id ?? tabs[0]!.id)
      : pane.activeId;
  return withPane(layout, { ...pane, tabs, activeId });
}

/** 聚焦一个 tab。 */
export function focus(layout: RailLayout, tabId: string): RailLayout {
  const pane = paneOf(layout, tabId);
  if (pane === null || pane.activeId === tabId) return layout;
  return withPane(layout, { ...pane, activeId: tabId });
}

/**
 * 记下一个 tab 选定的呈现方式。
 *
 * 与打开、聚焦一样是纯函数：界面只提交「用户选了哪一种看法」，
 * 由这一份状态负责把它落到布局上并随会话持久化。
 */
export function setViewer(layout: RailLayout, tabId: string, viewer: string): RailLayout {
  const pane = paneOf(layout, tabId);
  if (pane === null) return layout;
  const current = pane.tabs.find((tab) => tab.id === tabId);
  if (current === undefined || current.viewer === viewer) return layout;
  return withPane(layout, {
    ...pane,
    tabs: pane.tabs.map((tab) => (tab.id === tabId ? { ...tab, viewer } : tab)),
  });
}

/** 展开或收起整列。 */
export function setExpanded(layout: RailLayout, expanded: boolean): RailLayout {
  if (layout.expanded === expanded) return layout;
  const next = { ...layout, expanded };
  // 首次展开才播种默认页：收起状态下的空布局一直是空。
  return expanded ? seed(next) : next;
}

/** 切换全屏。 */
export function setFullscreen(layout: RailLayout, fullscreen: boolean): RailLayout {
  if (layout.fullscreen === fullscreen) return layout;
  return { ...layout, fullscreen };
}

/**
 * 切换呈现形态：贴靠与全屏互切。
 *
 * 自动全屏生效时（帧宽不足）「退出全屏」无处可退：这一手势在 既有工作台产品 口径下改为
 * 收起整列并回到贴靠形态，否则按钮点了没有任何可见变化。
 */
export function togglePresentation(layout: RailLayout, autoFullscreen: boolean): RailLayout {
  const fullscreen = autoFullscreen || layout.fullscreen;
  if (fullscreen && autoFullscreen) return setFullscreen(setExpanded(layout, false), false);
  return setFullscreen(layout, !fullscreen);
}

/** 分栏：把当前活跃格复制成第二格，并给新格一个默认页。 */
export function split(layout: RailLayout): RailLayout {
  const base = seed(layout);
  if (base.panes.length >= 2) return base;
  const sequence = base.sequence + 1;
  const tab = defaultTab(`tab-${sequence}`);
  return {
    ...base,
    sequence,
    // 新格不留空：空格会让用户以为面板坏了，默认页给出下一步可做的事。
    panes: [...base.panes, { id: 'pane-2', tabs: [tab], activeId: tab.id }],
    expanded: true,
  };
}

/** 并掉第二格，收回到单格。 */
export function dock(layout: RailLayout): RailLayout {
  if (layout.panes.length < 2) return layout;
  const [first, second] = layout.panes;
  if (first === undefined || second === undefined) return layout;
  const tabs = [...first.tabs, ...second.tabs].slice(-MAX_TABS_PER_PANE);
  return {
    ...layout,
    panes: [{ ...first, tabs, activeId: second.activeId ?? first.activeId }],
  };
}

/** 拖动分栏比例。 */
export function setSplitRatio(layout: RailLayout, ratio: number): RailLayout {
  const clamped = Math.max(0.2, Math.min(0.8, ratio));
  if (layout.splitRatio === clamped) return layout;
  return { ...layout, splitRatio: clamped };
}

/** 把布局压成可持久化的形状。 */
export function serialize(layout: RailLayout): string {
  return JSON.stringify(layout);
}

/**
 * 从持久化数据恢复布局；数据不合法时返回空布局。
 *
 * 开合与全屏形态随会话一起恢复，因此重启后回到离开时的样子；没有存过布局的
 * 会话拿到空布局，也就是折叠的默认形态。
 */
export function deserialize(raw: string | null): RailLayout {
  if (raw === null) return EMPTY_LAYOUT;
  try {
    const parsed = JSON.parse(raw) as Partial<RailLayout>;
    const panes = (parsed.panes ?? []).map((pane, index) => ({
      id: typeof pane.id === 'string' ? pane.id : `pane-${index + 1}`,
      tabs: (pane.tabs ?? []).map((tab, tabIndex) => ({
        id: typeof tab.id === 'string' ? tab.id : `tab-${index}-${tabIndex}`,
        kind: typeof tab.kind === 'string' ? tab.kind : DEFAULT_PAGE.kind,
        address: typeof tab.address === 'string' ? tab.address : null,
        title: typeof tab.title === 'string' ? tab.title : '会话资源',
        params: typeof tab.params === 'object' && tab.params !== null ? tab.params : {},
        revision: typeof tab.revision === 'number' ? tab.revision : 0,
        ...(typeof tab.viewer === 'string' ? { viewer: tab.viewer } : {}),
      })),
      activeId: typeof pane.activeId === 'string' ? pane.activeId : null,
    }));
    return {
      panes,
      expanded: parsed.expanded === true,
      fullscreen: parsed.fullscreen === true,
      splitRatio:
        typeof parsed.splitRatio === 'number' && parsed.splitRatio >= 0.2 && parsed.splitRatio <= 0.8
          ? parsed.splitRatio
          : 0.5,
      sequence: typeof parsed.sequence === 'number' ? parsed.sequence : 0,
    };
  } catch {
    return EMPTY_LAYOUT;
  }
}
