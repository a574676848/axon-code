import React from 'react';
import { ChevronDownIcon } from '../../icons';
import { Dropdown } from '../../components/ui/overlays';

/*
 * 文档查看页的公共零件。
 *
 * 正文骨架、路径栏、工具按钮与缩放控件被多个渲染器共用，因此它们在这里只有一份实现：
 * 换个渲染器不该让工具栏的排版、行高或图标尺寸跟着变。
 */

/**
 * 路径标签：目录弱化、文件名保持主色，整条路径挂在提示里。
 *
 * 溢出时在**左端**固定 28px 渐隐：被裁掉的是路径开头，文件名始终看得见；
 * 判定用内容宽度超过外层可用宽度，无容差。
 */
export const PathLabel: React.FC<{ path: string }> = ({ path }) => {
  const name = path.split(/[\\/]/).pop() ?? path;
  const directory = path.slice(0, Math.max(0, path.length - name.length));
  const outer = React.useRef<HTMLSpanElement>(null);
  const inner = React.useRef<HTMLSpanElement>(null);
  const [clipped, setClipped] = React.useState(false);

  React.useLayoutEffect(() => {
    const measure = () => {
      const outerElement = outer.current;
      const innerElement = inner.current;
      if (outerElement === null || innerElement === null) return;
      setClipped(innerElement.offsetWidth > outerElement.clientWidth);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return undefined;
    const observer = new ResizeObserver(measure);
    if (outer.current !== null) observer.observe(outer.current);
    return () => observer.disconnect();
  }, [path]);

  return (
    <span
      ref={outer}
      title={path}
      data-clipped={clipped ? '' : undefined}
      className="path-label flex-1 min-w-0 mr-3 text-meta"
    >
      <span ref={inner}>
        <span className="text-text-tertiary">{directory}</span>
        <span className="text-text-primary">{name}</span>
      </span>
    </span>
  );
};

/** 工具栏上的图标按钮：28px 圆角方块套一枚 14px 图形。 */
export const ToolButton: React.FC<{
  label: string;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  children: React.ReactNode;
}> = ({ label, onClick, disabled = false, pressed, children }) => (
  <button
    type="button"
    aria-label={label}
    title={label}
    disabled={disabled}
    aria-pressed={pressed}
    onClick={onClick}
    className="shrink-0 w-7 h-7 inline-flex items-center justify-center rounded-sm text-text-secondary leading-none transition-colors duration-100 hover:bg-hover hover:text-text-primary disabled:cursor-not-allowed disabled:text-text-caption disabled:hover:bg-transparent"
  >
    {children}
  </button>
);

/** 正文里的图标按钮：24px 方块，与代码正文的复制按钮同一档。 */
export const BodyButton: React.FC<{
  label: string;
  onClick: () => void;
  disabled?: boolean;
  children: React.ReactNode;
}> = ({ label, onClick, disabled = false, children }) => (
  <button
    type="button"
    aria-label={label}
    title={label}
    disabled={disabled}
    onClick={onClick}
    className="w-6 h-6 inline-flex items-center justify-center rounded-sm text-text-secondary transition-colors duration-100 hover:bg-hover disabled:cursor-not-allowed disabled:text-text-caption disabled:hover:bg-transparent"
  >
    {children}
  </button>
);

export interface BodyShellProps {
  /** 顶部一行：这份内容的名字或规模。 */
  label: string;
  /** 顶部右侧的控件。 */
  actions?: React.ReactNode;
  /** 滚动区附加类名：底色与内边距归各渲染器自己声明。 */
  bodyClassName?: string;
  /** 正文自己是否需要滚动；内嵌文档自己管滚动时置 false。 */
  scroll?: boolean;
  children: React.ReactNode;
}

/**
 * 渲染器外壳：正文骨架的唯一实现。
 *
 * 顶部一行固定 36px（11/18 的字，与停靠栏的工具栏同高），左轴固定 16px；
 * 正文只有**一个**滚动区，因此纯文本、代码、表格、图片与 PDF 在切换时，
 * 内容的起点与「滚动的是哪一块」完全相同。
 */
export const BodyShell: React.FC<BodyShellProps> = ({
  label,
  actions,
  bodyClassName = '',
  scroll = true,
  children,
}) => (
  <div className="flex-1 min-h-0 flex flex-col">
    <div className="shrink-0 h-9 pl-4 pr-2 flex items-center justify-between gap-3">
      <span className="min-w-0 truncate font-mono text-[11px] leading-[18px] text-text-tertiary">
        {label}
      </span>
      {actions !== undefined && <span className="shrink-0 flex items-center gap-1">{actions}</span>}
    </div>
    <div
      className={`flex-1 min-h-0 ${scroll ? 'overflow-auto scroll-visible' : 'overflow-hidden'} ${bodyClassName}`}
    >
      {children}
    </div>
  </div>
);

/** 缩放档位：与既有工作台产品的下拉一致。 */
const ZOOM_PRESETS = [0.5, 0.75, 1, 1.25, 1.5, 2];

/** 缩放下限与上限，防止拖到 0 或几千倍画出看不见的东西。 */
const MIN_ZOOM = 0.1;
const MAX_ZOOM = 8;

/** 把缩放比例夹进可用区间，并按 5% 取整，避免出现 133.33333%。 */
function clampZoom(value: number): number {
  const clamped = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, value));
  return Math.round(clamped * 20) / 20;
}

/**
 * 缩放控件：缩小、比例（含适应宽度）、放大。
 *
 * 与既有工作台产品同一组动作与同一套档位；「适应宽度」由调用方算出一个比例后交回来，
 * 因此这里的职责只有「显示当前比例、把用户的意图变成一个新比例」。
 */
export const ZoomControls: React.FC<{
  zoom: number;
  onZoom: (zoom: number) => void;
  onFit: () => void;
}> = ({ zoom, onZoom, onFit }) => (
  <>
    <BodyButton label="缩小" onClick={() => onZoom(clampZoom(zoom - 0.25))}>
      <span aria-hidden="true" className="text-body leading-none">
        −
      </span>
    </BodyButton>
    <Dropdown
      align="end"
      className="shrink-0"
      trigger={({ toggle }) => (
        <button
          type="button"
          aria-label="选择缩放比例"
          title="选择缩放比例"
          onClick={toggle}
          className="h-6 min-w-[52px] px-1.5 inline-flex items-center justify-center gap-0.5 rounded-sm text-meta tabular-nums text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary"
        >
          {Math.round(zoom * 100)}%
          <ChevronDownIcon size={10} />
        </button>
      )}
    >
      <button
        type="button"
        role="menuitem"
        onClick={onFit}
        className="w-full min-h-[30px] py-1 px-2 flex items-center rounded-md text-left text-row leading-5 text-text-primary transition-colors duration-100 hover:bg-hover"
      >
        适应宽度
      </button>
      {ZOOM_PRESETS.map((preset) => (
        <button
          key={preset}
          type="button"
          role="menuitem"
          onClick={() => onZoom(preset)}
          className={`w-full min-h-[30px] py-1 px-2 flex items-center rounded-md text-left text-row leading-5 text-text-primary transition-colors duration-100 hover:bg-hover ${
            Math.abs(preset - zoom) < 0.001 ? 'bg-hover' : ''
          }`}
        >
          {Math.round(preset * 100)}%
        </button>
      ))}
    </Dropdown>
    <BodyButton label="放大" onClick={() => onZoom(clampZoom(zoom + 0.25))}>
      <span aria-hidden="true" className="text-body leading-none">
        +
      </span>
    </BodyButton>
  </>
);
