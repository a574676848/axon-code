import React from 'react';
import {
  CloseIcon,
  CodeIcon,
  ExitFullscreenIcon,
  FileIcon,
  FolderIcon,
  FullscreenIcon,
  PanelRightIcon,
  PlusIcon,
  SplitIcon,
  TerminalIcon,
} from '../icons';
import { EmptyState } from '../components/ui/surfaces';
import { FileTree } from '../components/conversation/fileTree';
import { FileTypeIcon } from '../components/conversation/fileIcons';
import type { ChangedFile } from '../conversation/transcript';
import { ChangesReview } from './ChangesReview';
import { DocumentPreview } from './DocumentPreview';
import { GuideBody } from './GuideBody';
import { TerminalBody } from './TerminalBody';
import { CompassIcon } from './guideArtwork';
import { addressPath, parseChangesAddress } from './registry';
import type { RailPane, RailTab } from './model';
import type { RailController } from './useRailLayout';

/*
 * 右侧栏的 tab 正文与 tab 条。
 *
 * 一格一条 tab 条：胶囊、关闭、添加控件与分栏控件从左到右排，面板控件压在
 * 最后一格的末端，因此 tab 条本身就是面板的上边，不再单独画标题行。
 *
 * tab 条固定 36px（8px 顶部内边距 + 28px 控件），与主区顶栏、文件工具栏同高，
 * 因此同一停靠栏的正文起点只有 36（无工具栏）与 72（有工具栏）两种。
 */

/** tab 图标：统一 14px，且一律吃 currentColor。 */
const TabGlyph: React.FC<{ tab: RailTab }> = ({ tab }) => {
  if (tab.kind === 'guide') return <CompassIcon size={14} className="shrink-0" />;
  if (tab.kind === 'terminal') return <TerminalIcon size={14} className="shrink-0" />;
  // 文件夹只用一种单色图形：与侧栏工作区行同一个轮廓，不在这里另换一套琥珀色。
  if (tab.kind === 'files') return <FolderIcon size={14} className="shrink-0" />;
  if (tab.kind === 'changes') return <CodeIcon size={14} className="shrink-0" />;
  const path = tab.address === null ? null : addressPath(tab.address);
  if (path === null) return <FileIcon size={14} className="shrink-0" />;
  return <FileTypeIcon path={path} size={14} className="shrink-0" />;
};

/** tab 正文：按类型选择实现。 */
export const RailTabBody: React.FC<{
  tab: RailTab;
  root: string | null;
  activePath: string | null;
  controller: RailController;
  onOpenFile: (path: string) => void;
  /** 取某一轮的改动文件；这一轮的改动不在当前投影里时返回 null。 */
  changesFor: (turnId: string) => ChangedFile[] | null;
}> = ({ tab, root, activePath, controller, onOpenFile, changesFor }) => {
  /*
   * 引导页与工作区无关：它列的是「能打开什么」，因此没有工作区时照常呈现，
   * 真正的空态落在各自的目标页里（文件页会说「先选择一个工作区」）。
   */
  if (tab.kind === 'guide') {
    return <GuideBody controller={controller} />;
  }
  /*
   * 改动对比不读工作区文件：它的正文来自这一轮的改动事实，因此没有工作区时也照常呈现，
   * 只是「用外部应用打开」这类动作没有可用的根目录而已。
   */
  if (tab.kind === 'changes') {
    const parsed = tab.address === null ? null : parseChangesAddress(tab.address);
    if (parsed === null) {
      return (
        <EmptyState
          className="flex-1"
          icon={<FileIcon size={20} />}
          title="没有可预览的内容"
          description="这个资源地址解析不出改动坐标。"
        />
      );
    }
    return (
      <ChangesReview
        root={root}
        files={changesFor(parsed.turnId)}
        index={tab.params.index ?? 0}
        onOpenFile={onOpenFile}
      />
    );
  }
  if (root === null) {
    return (
      <EmptyState
        className="flex-1"
        icon={<FolderIcon size={20} />}
        title="先选择一个工作区"
        description="选中工作区后才能在这里浏览文件与预览内容。"
      />
    );
  }
  if (tab.kind === 'terminal') {
    return (
      <TerminalBody
        root={root}
        shellPath={tab.params.shellPath}
        onRestart={() => controller.newTerminal()}
      />
    );
  }
  if (tab.kind === 'files') {
    return <FileTree root={root} activePath={activePath} onOpenFile={onOpenFile} />;
  }
  const path = tab.address === null ? null : addressPath(tab.address);
  if (path === null) {
    return (
      <EmptyState
        className="flex-1"
        icon={<FileIcon size={20} />}
        title="没有可预览的文本"
        description="这个资源地址不是工作区里的文件。"
      />
    );
  }
  return (
    <DocumentPreview
      root={root}
      path={path}
      line={tab.params.line}
      viewer={tab.viewer}
      onViewer={(viewer) => controller.selectViewer(tab.id, viewer)}
      onOpenFile={onOpenFile}
    />
  );
};

export interface RailTabStripProps {
  pane: RailPane;
  /** 是否是最后一格：面板控件只画在这里。 */
  last: boolean;
  /** 当前格数：单格给分栏，双格给并栏。 */
  paneCount: number;
  dragging: boolean;
  /** 当前宽度是否放得下两格；放不下时禁用分栏并说明原因。 */
  canSplit: boolean;
  /** 分栏不可用时的原因，同时作为原生提示。 */
  splitHint: string;
  /** 该 tab 能否关闭：独占停靠面的默认页不画关闭入口。 */
  canClose: (tabId: string) => boolean;
  onFocus: (tabId: string) => void;
  onClose: (tabId: string) => void;
  onAdd: () => void;
  onSplit: () => void;
  onDock: () => void;
  onToggleFullscreen: () => void;
  onToggleExpanded: () => void;
}

/*
 * tab 标题的裁切渐隐：不留省略号，在裁切边用遮罩收尾。
 * 溢出判定按既有工作台产品的口径（scrollWidth 比 clientWidth 多出一像素以上即算裁切），
 * 每次提交与元素尺寸变化都重判，因此拖宽面板后遮罩会自己撤掉。
 */
const ClippedTabTitle: React.FC<{ text: string }> = ({ text }) => {
  const ref = React.useRef<HTMLSpanElement>(null);
  const [clipped, setClipped] = React.useState(false);

  React.useLayoutEffect(() => {
    const element = ref.current;
    if (element === null) return undefined;
    const mark = () => setClipped(element.scrollWidth > element.clientWidth + 1);
    mark();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(mark);
    observer.observe(element);
    return () => observer.disconnect();
  }, [text]);

  return (
    <span ref={ref} className="tab-title-clip" data-clipped={clipped ? '' : undefined}>
      {text}
    </span>
  );
};

/** tab 条上的图标控件：28px 方块，与工具栏同一档。 */
const StripButton: React.FC<{
  label: string;
  title?: string;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}> = ({ label, title, disabled = false, onClick, children }) => (
  <button
    type="button"
    aria-label={label}
    title={title ?? label}
    disabled={disabled}
    onClick={onClick}
    className="w-7 h-7 shrink-0 inline-flex items-center justify-center rounded-sm text-text-tertiary transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:text-text-caption disabled:hover:bg-transparent"
  >
    {children}
  </button>
);

export const RailTabStrip: React.FC<RailTabStripProps> = ({
  pane,
  last,
  paneCount,
  dragging,
  canSplit,
  splitHint,
  canClose,
  onFocus,
  onClose,
  onAdd,
  onSplit,
  onDock,
  onToggleFullscreen,
  onToggleExpanded,
}) => {
  /*
   * 新增控件是「回到引导页」的路：这一格已经有引导页时它就没有意义，
   * 因此按既有工作台产品的口径只在没有引导页时画出来。
   */
  const showAdd = !pane.tabs.some((tab) => tab.kind === 'guide');
  const split = paneCount === 1;
  return (
    <div
      data-tauri-drag-region
      style={
        last ? { paddingRight: 'calc(4px + var(--window-controls-inset, 0px))' } : undefined
      }
      className="shrink-0 h-10 flex items-center gap-1 bg-surface-sunken hairline-b border-border-hairline select-none"
    >
      <div className="flex-1 min-w-0 flex items-center gap-1 overflow-x-auto no-scrollbar pl-2.5">
        {pane.tabs.map((tab, index) => {
          const active = tab.id === pane.activeId;
          const prevActive = index > 0 && pane.tabs[index - 1]?.id === pane.activeId;
          const closable = canClose(tab.id);
          return (
            <React.Fragment key={tab.id}>
              {/* 仅在两个相邻非激活 tab 之间绘制槽内细分隔线；浮起的激活签自带边界，不画多余竖线 */}
              {index > 0 && !active && !prevActive && (
                <span aria-hidden="true" className="shrink-0 self-center w-hairline h-3.5 bg-border" />
              )}
              <div
                data-active={active ? '' : undefined}
                /* 面板的焦点续接按这个标记找还活着的 tab 胶囊。 */
                data-rail-tab={tab.id}
                className={`tab-chip group relative shrink-0 h-7 min-w-[100px] max-w-[170px] flex items-center rounded-sm transition-colors duration-100 ${
                  active ? 'bg-surface shadow-lift hairline border-border' : 'hover:bg-hover'
                }`}
              >
                <button
                  type="button"
                  onClick={() => onFocus(tab.id)}
                  title={tab.title}
                  aria-pressed={active}
                  className={`h-full w-full pl-2.5 inline-flex items-center gap-1.5 text-row transition-colors duration-100 ${
                    closable ? 'pr-7' : 'pr-2.5'
                  } ${active ? 'text-text-primary font-medium' : 'text-text-secondary'}`}
                >
                  <TabGlyph tab={tab} />
                  <ClippedTabTitle text={tab.title} />
                </button>
                {closable && (
                  <button
                    type="button"
                    aria-label={`关闭 ${tab.title}`}
                    title="关闭"
                    onClick={() => onClose(tab.id)}
                    className={`absolute top-1 right-1 w-5 h-5 inline-flex items-center justify-center rounded-xs text-text-tertiary transition-opacity duration-100 hover:text-text-primary ${
                      active ? 'opacity-100' : 'opacity-0 group-hover:opacity-100 focus-visible:opacity-100'
                    }`}
                  >
                    <CloseIcon size={12} />
                  </button>
                )}
              </div>
            </React.Fragment>
          );
        })}
        {showAdd && (
          <button
            type="button"
            aria-label="回到引导页"
            title="回到引导页"
            onClick={onAdd}
            className="shrink-0 h-7 w-7 inline-flex items-center justify-center rounded-sm text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
          >
            <PlusIcon size={14} />
          </button>
        )}
      </div>

      {/* 末格控件簇：单格三枚 28px（92px 含间隙），分栏之后收成两枚，全屏入口隐藏。 */}
      <div className="shrink-0 flex items-center gap-1 pr-1">
        {last && split && (
          <StripButton label="分栏" title={splitHint} disabled={!canSplit} onClick={onSplit}>
            <SplitIcon size={14} />
          </StripButton>
        )}
        {last && !split && (
          <StripButton label="并掉分栏" onClick={onDock}>
            <SplitIcon size={14} />
          </StripButton>
        )}
        {/*
         * 分栏之后只留两枚（并栏 + 收起）：格子变窄时控件比内容还占地方。
         * 全屏态是例外：出口不能跟着藏起来，否则全屏里就再也退不出来了。
         */}
        {last && (!split || dragging) && (
          <StripButton
            label={dragging ? '退出全屏面板' : '全屏面板'}
            title={dragging ? '退出全屏' : '全屏'}
            onClick={onToggleFullscreen}
          >
            {dragging ? <ExitFullscreenIcon size={14} /> : <FullscreenIcon size={14} />}
          </StripButton>
        )}
        {last && (
          <StripButton label="收起面板" onClick={onToggleExpanded}>
            <PanelRightIcon size={14} />
          </StripButton>
        )}
      </div>
    </div>
  );
};

/** 分栏拖拽把手。 */
export const RailSplitter: React.FC<{ onStart: () => void }> = ({ onStart }) => (
  <div
    role="separator"
    aria-orientation="vertical"
    aria-label="调整分栏比例"
    onMouseDown={(event) => {
      event.preventDefault();
      onStart();
    }}
    className="shrink-0 w-1 cursor-col-resize touch-none bg-border-hairline hover:bg-border-strong transition-colors duration-100"
  />
);
