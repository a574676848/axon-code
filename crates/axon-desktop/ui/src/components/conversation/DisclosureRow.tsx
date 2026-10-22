import React from 'react';
import { ChevronRightIcon, SpinnerIcon } from '../../icons';

/*
 * 可展开行的唯一实现。
 *
 * 此前时间线、计划卡、改动卡与作业列表各写一套：行高 24 / 40 / 44 / 40，
 * 箭头 11 / 12 / 13，还有「悬浮反向旋转」这种只在某一处成立的约定。
 * 全站因此只有一条规则：
 *   - 行高 28px，左侧固定 56px 标签列（思考 / 执行 / 命令），正文 13/20；
 *   - 箭头 12px，收起朝右、展开朝下，只有一个旋转方向；
 *   - 行尾指标 12px 等宽右对齐；运行中用转圈替换箭头。
 * 不再是「行内任意可点」的第二套折叠语义：整行就是一个按钮。
 */

/**
 * 标签列：按内容展开，不设固定宽度，也不截断。
 *
 * 过程指令不只有两个字：终端执行、作业输出、更新计划都是四个字。
 * 此前用一个 28px 的固定列，长指令会被截成「终...」「作...」；这里改成
 * 内容宽度加不换行，正文缩进只锚定标签起点，因此长标签只影响右侧标题，
 * 不会把展开正文推离指令左轴。
 */
const LABEL_COLUMN = 'shrink-0 whitespace-nowrap';
/**
 * 展开后正文的缩进。
 *
 * 正文与「指令」标签的左轴对齐：外 2px + 箭头 12 + 间距 6 + 图标 14 + 间距 6 = 40px。
 * 引导线放在 34px（图标右缘），线宽 2px，再留 4px，正文正好从 40px 开始。
 * 标签按内容展开，宽度不参与这个计算，因此「终端执行」这类四字指令不会把正文推走。
 */
export const DISCLOSURE_BODY_INDENT = 'ml-[34px]';

export const DISCLOSURE_ROW_HEIGHT = 'h-7';

export interface DisclosureRowProps {
  /**
   * 展开态。给出即渲染箭头；不给出表示这一行只有动作、没有展开（例如改动卡头部）。
   */
  open?: boolean;
  /** 整行点击。 */
  onToggle?: () => void;
  /** 行首图形；给出即占据标签列的第一个位置。 */
  icon?: React.ReactNode;
  /** 固定标签列文本，例如「思考」「执行」。 */
  label?: string;
  /** 内容主体。 */
  title?: React.ReactNode;
  /** 行尾读数，例如 exit 0 · 1.22s。 */
  meta?: React.ReactNode;
  /** 运行中：转圈替换箭头。 */
  active?: boolean;
  /** 失败或非零退出：整行取危险色。 */
  danger?: boolean;
  /** 追加在行尾的控件（不参与整行点击）。 */
  trailing?: React.ReactNode;
  /** 行背景等额外样式。 */
  className?: string;
  /** 行的无障碍名称；标题被截断时它仍是全文入口。 */
  titleText?: string;
  /** 导航行所在的菜单语义：整行按钮直接充当 menuitem。 */
  rowRole?: 'menuitem';
}

export const DisclosureRow: React.FC<DisclosureRowProps> = ({
  open,
  onToggle,
  icon,
  label,
  title,
  meta,
  active = false,
  danger = false,
  trailing,
  className = '',
  titleText,
  rowRole,
}) => {
  /*
   * 两种形态：给 open 时是「可展开行」，只给 onToggle 时是「有动作但没有展开态」
   * （例如改动卡头部，点一下是去右侧栏，不是在本行展开）。
   */
  const interactive = onToggle !== undefined;
  /*
   * 圆点只分隔「指令」与「内容」。
   * 思考行展开后行头不再重复正文，标题为空；此时保留圆点会留下一个悬空的「·」。
   */
  const hasTitle =
    title !== undefined &&
    title !== null &&
    !(typeof title === 'string' && title.trim().length === 0);
  const content = (
    <>
      {open !== undefined && (
        <span className="shrink-0 text-text-tertiary transition-colors duration-100 group-hover:text-text-primary">
          {active ? (
            <SpinnerIcon size={12} className="animate-spin" />
          ) : (
            <ChevronRightIcon
              size={12}
              className={`transition-transform duration-150 ease-out ${open ? 'rotate-90' : ''}`}
            />
          )}
        </span>
      )}
      {icon !== undefined && (
        <span
          className={`shrink-0 inline-flex items-center transition-colors duration-100 ${
            danger ? 'text-danger' : 'text-text-tertiary group-hover:text-text-primary'
          }`}
        >
          {icon}
        </span>
      )}
      {label !== undefined && label.length > 0 && (
        <span
          className={`${LABEL_COLUMN} text-row ${danger ? 'text-danger' : 'text-text-primary'}`}
        >
          {label}
        </span>
      )}
      {label !== undefined && label.length > 0 && hasTitle && (
        <span aria-hidden="true" className="shrink-0 text-row text-text-caption">
          ·
        </span>
      )}
      <span
        className={`min-w-0 flex-1 truncate text-row ${
          danger ? 'text-danger' : 'text-text-primary'
        }`}
      >
        {title}
      </span>
      {meta !== undefined && (
        <span
          className={`shrink-0 tabular-nums text-meta ${
            danger ? 'text-danger' : 'text-text-tertiary'
          }`}
        >
          {meta}
        </span>
      )}
    </>
  );

  return (
    <div className={`w-full ${className}`}>
      <div className="flex items-center gap-1.5">
        {interactive ? (
          <button
            type="button"
            role={rowRole}
            onClick={onToggle}
            aria-expanded={open}
            title={titleText}
            className={`group min-w-0 flex-1 ${DISCLOSURE_ROW_HEIGHT} flex items-center gap-1.5 rounded-sm pl-0.5 pr-1 text-left transition-colors duration-100 cursor-pointer`}
          >
            {content}
          </button>
        ) : (
          <div
            title={titleText}
            className={`min-w-0 flex-1 ${DISCLOSURE_ROW_HEIGHT} flex items-center gap-1.5 rounded-sm pl-0.5 pr-1`}
          >
            {content}
          </div>
        )}
        {trailing}
      </div>
    </div>
  );
};

/** 展开后的正文：统一缩进，与行头内容列对齐。 */
export const DisclosureBody: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div className={`${DISCLOSURE_BODY_INDENT} mb-1 border-l-[2px] border-border-hairline pl-1`}>
    {children}
  </div>
);

/**
 * 增删行数指标：时间线工具行与改动卡共用。
 *
 * 11px 等宽、右对齐，新增取成功墨水档（标记档对白底不够 AA），删除取危险色。
 */
export const DiffStat: React.FC<{ added: number; removed: number; className?: string }> = ({
  added,
  removed,
  className = '',
}) => (
  <span className={`inline-flex items-center gap-1.5 font-mono text-xs tabular-nums ${className}`}>
    <span className="text-ok-ink">+{added}</span>
    <span className="text-danger">-{removed}</span>
  </span>
);
