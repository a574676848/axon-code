import React from 'react';
import {
  activeTab,
  close,
  deserialize,
  dock,
  focus,
  open,
  replace,
  serialize,
  setExpanded,
  setSplitRatio,
  setViewer,
  split,
  togglePresentation,
  EMPTY_LAYOUT,
  type RailLayout,
} from './model';
import { addressPath, claim, pageType, type ResourceParams } from './registry';

/*
 * 右侧栏控制器。
 *
 * 布局按会话持久化：切会话就读回那个会话的停靠面，刷新或重启后仍在原处，
 * 开合与全屏形态也是这份布局的一部分；没存过布局的会话从折叠起步。
 *
 * 返回值整体 memo 化：动作都是稳定的 useCallback，但对象字面量每次渲染都是新的，
 * 直接当成 effect 依赖会让订阅每个渲染都重来一遍。
 */

/** 持久化键前缀；按会话 id 分开存。 */
const STORAGE_PREFIX = 'axon.sidebar-right.v1.';

function storageKey(laneId: string | null): string {
  return `${STORAGE_PREFIX}${laneId ?? 'none'}`;
}

/** 打开一个页类型时的落位选项。 */
export interface OpenPageOptions {
  /** 导航参数（终端用它带 shell）。 */
  params?: ResourceParams;
  /** 已展示同一页类型时是否聚焦而不是新增；终端这类可多开的类型不受影响。 */
  revealIfOpened?: boolean;
  /** 用新页替换当前活跃 tab（引导页入口就是「这一页让位给那个类型」）。 */
  replaceActive?: boolean;
}

export interface RailController {
  layout: RailLayout;
  /** 打开一个资源地址；没人认领时忽略。 */
  openResource: (address: string, params?: ResourceParams) => void;
  /** 打开一个页类型。 */
  openPage: (kind: string, options?: OpenPageOptions) => void;
  /** 新开一个终端；不给 shell 时由后端按配置顺序取第一个可用的。 */
  newTerminal: (shellPath?: string) => void;
  closeTab: (tabId: string) => void;
  focusTab: (tabId: string) => void;
  /** 记下一个 tab 选定的呈现方式；跟 tab 一起持久化。 */
  selectViewer: (tabId: string, viewer: string) => void;
  toggleExpanded: () => void;
  /** 切换贴靠与全屏；自动全屏生效时这一手势改为收起整列。 */
  toggleFullscreen: (autoFullscreen: boolean) => void;
  splitPane: () => void;
  dockPane: () => void;
  resize: (ratio: number) => void;
  /** 当前预览的文件相对路径；文件页据此高亮。 */
  activePath: string | null;
}

export function useRailLayout(laneId: string | null): RailController {
  const key = storageKey(laneId);
  const [layout, setLayout] = React.useState<RailLayout>(() =>
    deserialize(window.localStorage.getItem(key)),
  );
  const [adoptedKey, setAdoptedKey] = React.useState(key);

  /*
   * 切会话：换一整套布局，而不是在新会话上继续用上一个会话的 tab。
   *
   * 与目标切换同一次提交完成。晚一帧会先把上一个会话的停靠面画出去，其中就包括指标条
   * 右侧那个展开入口：它一出现就消失，读起来和指标条的塌陷是同一种闪动。
   */
  if (adoptedKey !== key) {
    setAdoptedKey(key);
    setLayout(deserialize(window.localStorage.getItem(key)));
  }

  React.useEffect(() => {
    try {
      window.localStorage.setItem(storageKey(laneId), serialize(layout));
    } catch {
      // 存储不可用时布局仍在内存里可用，不打扰用户。
    }
  }, [laneId, layout]);

  const openResource = React.useCallback((address: string, params?: ResourceParams) => {
    const type = claim(address);
    if (type === null) return;
    setLayout((current) =>
      open(current, {
        kind: type.kind,
        address,
        title: type.title(address),
        params: params ?? {},
      }),
    );
  }, []);

  const openPage = React.useCallback((kind: string, options?: OpenPageOptions) => {
    const type = pageType(kind);
    // 未注册的页类型是接线错误：静默忽略而不是开一个渲染不出东西的 tab。
    if (type === null) return;
    setLayout((current) => {
      const request = {
        kind,
        address: null,
        title: type.title(null),
        params: options?.params ?? {},
        multiple: type.multiple === true,
        revealIfOpened: options?.revealIfOpened,
      };
      return options?.replaceActive === true ? replace(current, request) : open(current, request);
    });
  }, []);

  const newTerminal = React.useCallback((shellPath?: string) => {
    openPage('terminal', shellPath === undefined ? undefined : { params: { shellPath } });
  }, [openPage]);

  const current = activeTab(layout);
  const activePath = current?.address == null ? null : addressPath(current.address);

  return React.useMemo(
    () => ({
      layout,
      openResource,
      openPage,
      newTerminal,
      closeTab: (tabId: string) => setLayout((value) => close(value, tabId)),
      focusTab: (tabId: string) => setLayout((value) => focus(value, tabId)),
      selectViewer: (tabId: string, viewer: string) =>
        setLayout((value) => setViewer(value, tabId, viewer)),
      toggleExpanded: () => setLayout((value) => setExpanded(value, !value.expanded)),
      toggleFullscreen: (autoFullscreen: boolean) =>
        setLayout((value) => togglePresentation(value, autoFullscreen)),
      splitPane: () => setLayout((value) => split(value)),
      dockPane: () => setLayout((value) => dock(value)),
      resize: (ratio: number) => setLayout((value) => setSplitRatio(value, ratio)),
      activePath,
    }),
    [activePath, layout, newTerminal, openPage, openResource],
  );
}

/** 空布局导出给测试与初始态复用。 */
export { EMPTY_LAYOUT };
