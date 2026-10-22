import React from 'react';
import { Z_MENU } from './chatChrome';

/*
 * 浮层菜单的唯一几何。
 *
 * 对话域里的候选菜单与模型选择此前是四套数值（34px/40px 行、px-2/px-4、
 * 6px/4px 间距、8/12 圆角），同一屏里并排出现时一眼能看出不是一套东西。
 * 这里定死：卡片 8px 圆角、内边距 4px、行 32px、正文 13/20、与锚点 4px。
 */

/** 菜单与锚点之间的间距。 */
export const MENU_ANCHOR_GAP = 4;
/** 菜单内边距。 */
export const MENU_PADDING = 'p-1';
/** 行高下限：单行 32px。 */
export const MENU_ROW_HEIGHT = 'min-h-8';
/** 卡片几何：白底、发丝描边、8px 圆角、菜单阴影。 */
export const MENU_PANEL_CLASS = `flex flex-col overflow-hidden rounded-sm bg-surface shadow-menu hairline border-border-hairline ${MENU_PADDING}`;
/** 浮层定位的公共部分：portal 加 fixed，统一压到菜单档。 */
export const MENU_SURFACE_CLASS = `fixed ${Z_MENU} shadow-menu rounded-sm bg-surface hairline border-border-hairline overflow-hidden`;

export interface MenuRowProps {
  /** 主文本：指令名、路径或选项名。 */
  label: React.ReactNode;
  /** 右侧弱化说明；给出即右对齐并参与截断。 */
  hint?: React.ReactNode;
  /** 右侧取值（例如模型选择器里的当前值）。 */
  value?: React.ReactNode;
  /** 尾部图形，例如进入子菜单。 */
  trailing?: React.ReactNode;
  /** 当前高亮（键盘或指针）。 */
  active?: boolean;
  /** 已选中（例如当前模型）。 */
  selected?: boolean;
  onClick?: () => void;
  onMouseDown?: (event: React.MouseEvent<HTMLButtonElement>) => void;
  onMouseMove?: () => void;
  /** 列表语义：候选菜单用 option，子菜单入口用 menuitem。 */
  role?: 'option' | 'menuitem';
  id?: string;
  ariaSelected?: boolean;
  title?: string;
}

export const MenuRow: React.FC<MenuRowProps> = ({
  label,
  hint,
  value,
  trailing,
  active = false,
  selected = false,
  onClick,
  onMouseDown,
  onMouseMove,
  role,
  id,
  ariaSelected,
  title,
}) => {
  /*
   * 只有存在右侧内容时才给标签设上限。
   * 纯选项列表没有 hint/value，标签必须吃掉整行；此前一律按 45% 截断，
   * 菜单又固定 300px，结果长模型名被截断，右侧却空出一大片。
   */
  const hasSide = hint !== undefined || value !== undefined;
  return (
    <button
      type="button"
      role={role}
      id={id}
      title={title}
      aria-selected={ariaSelected}
      onClick={onClick}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      className={`flex w-full items-center gap-2 rounded-xs px-2.5 text-left text-row ${MENU_ROW_HEIGHT} transition-colors duration-100 ${
        active ? 'bg-hover' : 'hover:bg-hover'
      } ${selected ? 'text-text-primary' : ''}`}
    >
      <span
        className={`${hasSide ? 'shrink-0 max-w-[140px]' : 'min-w-0 flex-1'} truncate text-text-primary`}
      >
        {label}
      </span>
      {hint !== undefined && (
        <span className="min-w-0 flex-1 truncate text-right text-meta text-text-tertiary">{hint}</span>
      )}
      {value !== undefined && (
        <span className="shrink-0 max-w-[160px] truncate text-row text-text-secondary">{value}</span>
      )}
      {trailing}
    </button>
  );
};
