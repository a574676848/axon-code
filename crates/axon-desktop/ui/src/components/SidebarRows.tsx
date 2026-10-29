import React, { useState } from 'react';
import {
  AlertCircleIcon,
  ArchiveIcon,
  ChevronRightIcon,
  EditIcon,
  FolderIcon,
  GlobeIcon,
  IconNewChat,
  MoreIcon,
  PinFillIcon,
  PinIcon,
  SpinnerIcon,
  TrashIcon,
} from '../icons';
import type { LaneItem, WorkspaceItem } from '../types';
import { IconButton } from './ui/controls';
import { Dropdown, MenuItem, MenuSeparator } from './ui/overlays';
import { Chip, RowActions, StateDot, Text } from './ui/surfaces';
import type { StateDotState } from './ui/surfaces';
import { HoverCard, HoverCardLine, HoverCardStatus, HoverCardTitle } from './ui/HoverCard';
import { MARQUEE_TITLE_CLASS, useRowTitleMarquee } from './ui/useTitleMarquee';

/*
 * 侧栏的行层：栅格、三态与两种行（工作区、会话）。
 *
 * 这是侧栏唯一的一套行几何。品牌行、顶部导航、区块头、工作区行、会话行与页脚
 * 都从这里取值，因此任何一行的文字都落在同一条竖线上，行内图标也只有一个尺寸档。
 */

// ------------------------------------------------------------------ 栅格与行状态

/*
 * 侧栏的单一栅格：外边距 12（容器给）+ 图标列 24 + 文字列间隙 8。
 */
export const ROW_ICON_COLUMN = 'shrink-0 w-6 flex items-center justify-center';

/** 文字列起点（相对行的左缘）：图标列 24 + 间隙 8。 */
export const ROW_TEXT_INDENT = 'pl-8';

/*
 * 行状态只有三档，全站一套：静止、hover、selected。
 * selected 在沉底托盘上浮起一张白签（bg-surface + shadow-lift + 微描边），
 * 并由左侧 2px 主色条给出位置，视觉层级干净明确。
 */
export const ROW_STATE =
  'group relative flex items-center gap-2 rounded-sm transition-colors duration-100';
export const ROW_SELECTED = 'bg-surface shadow-lift hairline border-border font-medium';
export const ROW_SELECTED_BAR =
  'pointer-events-none absolute left-0 top-2 bottom-2 w-0.5 rounded-full bg-primary';

/**
 * 底色档位：选中不再叠加 hover，否则悬浮时选中行会比静止时更浅。
 * @param selected 是否处于选中态。
 * @returns 该行应带的底色类名。
 */
export function rowTone(selected: boolean): string {
  return selected ? ROW_SELECTED : 'hover:bg-hover';
}

/** 状态槽：恒定 16px，无论有没有状态点，标题都不会横跳。 */
const STATUS_SLOT = 'shrink-0 w-4 flex items-center justify-center';

/**
 * 会话行尾槽的固定宽度。
 *
 * 三枚 28px 图标按钮加两处 2px 间隙正好 88px：槽宽恒定，因此 hover 时操作区
 * 替换时间不会改变标题的可用宽度，跑马灯也不会按旧宽度判定溢出。
 */
const LANE_TRAILING_WIDTH = 88;

/** 工作区行尾槽：两枚 28px 按钮加 4px 间隙。 */
const WORKSPACE_TRAILING_WIDTH = 60;

/** 树形分组下每一层的缩进像素。 */
const TREE_INDENT = 14;

/**
 * 相对时间：中文界面取最短但仍读得通的写法，数字与汉字之间留一个空格。
 * 会话行只有 16px 的竖向余量，因此不再写「分钟前」这种完整句子。
 */
export function formatRelativeTime(timestamp: number): string {
  if (timestamp === 0) return '';
  const diff = Date.now() - timestamp;
  if (diff < 120_000) return '刚刚';
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)} 分钟`;
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)} 小时`;
  return `${Math.floor(diff / 86_400_000)} 天`;
}

/**
 * 会话行的前置状态：优先级与参考实现一致。
 *
 * 在跑优先于未查看的终态（跑起来之后旧标识不该抢位置），两者都没有就不占位；
 * 完成是绿点，异常是红点，被中断与主动停止同属「按人的意思结束」因此是琥珀点。
 * 标识在用户点开这条会话后由外层清除。
 */
export function laneStatusState(
  lane: LaneItem,
  activity: string | undefined,
): StateDotState | null {
  if (lane.running) return 'ongoing';
  if (activity === 'failed') return 'error';
  if (activity === 'interrupted') return 'warning';
  if (activity !== undefined) return 'done';
  return null;
}

// ------------------------------------------------------------------ 会话行

export interface LaneRowProps {
  lane: LaneItem;
  selected: boolean;
  /** 该会话有新结果时的终态；为空表示没有待查看的标识。 */
  activity?: string;
  onSelect: () => void;
  onTogglePin: () => void;
  onToggleArchive: () => void;
  onRename: (title: string) => void;
  /** 拖拽开始。 */
  onDragStart: () => void;
  /** 拖拽经过，half 表示落点在该行的上半还是下半。 */
  onDragOver: (half: 'before' | 'after') => void;
  /** 放下。 */
  onDrop: () => void;
  /** 拖拽结束。 */
  onDragEnd: () => void;
  /** 是否正在被拖拽。 */
  dragging: boolean;
  /** 落点指示：无、上方、下方。 */
  dropHint: 'none' | 'before' | 'after';
}

/** 一条会话：状态槽、标题、行尾槽（时间与图钉，悬浮换成操作区）。 */
export const LaneRow: React.FC<LaneRowProps> = ({
  lane,
  selected,
  activity,
  onSelect,
  onTogglePin,
  onToggleArchive,
  onRename,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  dragging,
  dropHint,
}) => {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(lane.title);
  const [menuOpen, setMenuOpen] = useState(false);
  const { titleRef, handlers: marqueeHandlers } = useRowTitleMarquee();
  /*
   * 侦测会话标题自动生成或外部重命名：首次挂载时不播动画，
   * 运行中后端提炼出新标题时触发一次柔和上浮叠化（title-morph-in）。
   */
  const initialTitleRef = React.useRef(lane.title);
  const titleChanged = lane.title !== initialTitleRef.current;
  /** 尚未产生任何交互的会话：按既有工作台产品的语义，它是一条安静的占位行。 */
  const blank = lane.pending;
  /** 前置状态槽里要画的状态；为空表示这一行不占状态位。 */
  const status = blank ? null : laneStatusState(lane, activity);
  const hasPendingQuestion = Boolean(
    (lane.pending_question_count ?? 0) > 0 || activity?.startsWith('question_'),
  );

  const commit = () => {
    const trimmed = draft.trim();
    if (trimmed.length > 0 && trimmed !== lane.title) onRename(trimmed);
    setEditing(false);
  };

  return (
    <HoverCard
      disabled={menuOpen || editing}
      content={
        <>
          <HoverCardTitle>{lane.title}</HoverCardTitle>
          <HoverCardLine>
            {!blank && <>{formatRelativeTime(lane.updated_at)}{' · '}</>}
            共 {lane.interaction_count} 条会话
          </HoverCardLine>
          <HoverCardStatus
            icon={
              lane.running ? (
                <SpinnerIcon size={14} className="state-spinner" />
              ) : (
                <GlobeIcon size={14} />
              )
            }
          >
            {lane.running ? '进行中' : `渠道 ${lane.channel}`}
            {lane.archived ? ' · 已归档' : ''}
            {lane.pinned ? ' · 已置顶' : ''}
          </HoverCardStatus>
        </>
      }
    >
      <div
        draggable={!editing && !blank}
        {...marqueeHandlers}
        onDragStart={onDragStart}
        onDragOver={(event) => {
          event.preventDefault();
          const rect = event.currentTarget.getBoundingClientRect();
          onDragOver(event.clientY - rect.top < rect.height / 2 ? 'before' : 'after');
        }}
        onDrop={(event) => {
          event.preventDefault();
          onDrop();
        }}
        onDragEnd={onDragEnd}
        onClick={() => {
          if (!editing) onSelect();
        }}
        title={`渠道 ${lane.channel} · 目标 ${lane.destination || '未指定'} · 共 ${lane.interaction_count} 条会话`}
        className={`${ROW_STATE} ${rowTone(selected)} h-9 pr-1 cursor-pointer select-none ${
          dragging ? 'opacity-40' : ''
        }`}
      >
        {selected && (
          <span
            aria-hidden="true"
            className={`${ROW_SELECTED_BAR} ${lane.running ? 'dock-ambient-glow' : ''}`}
          />
        )}
        {dropHint !== 'none' && (
          <span
            aria-hidden="true"
            className={`absolute left-1 right-1 h-0.5 rounded-full bg-focus ${
              dropHint === 'before' ? '-top-px' : '-bottom-px'
            }`}
          />
        )}

        {/* 状态槽：待答用线性图标，在跑是转圈，有未查看的结果是终态点；槽宽恒定。 */}
        <span className={ROW_ICON_COLUMN}>
          {hasPendingQuestion ? (
            <span
              role="img"
              aria-label="有待答复提问"
              title="有待答复提问"
              className={`${STATUS_SLOT} text-gold-ink confirm-pop`}
            >
              <AlertCircleIcon size={14} />
            </span>
          ) : status !== null ? (
            <span className={`${STATUS_SLOT} ${status !== 'ongoing' ? 'confirm-pop' : ''}`}>
              <StateDot state={status} />
            </span>
          ) : null}
        </span>

        {editing ? (
          <input
            autoFocus
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onBlur={commit}
            onClick={(event) => event.stopPropagation()}
            onKeyDown={(event) => {
              if (event.key === 'Enter') commit();
              if (event.key === 'Escape') setEditing(false);
            }}
            className="flex-1 min-w-0 h-7 px-1.5 rounded-xs bg-surface hairline border-focus text-row text-text-primary outline-none"
          />
        ) : (
          <Text
            key={titleChanged ? lane.title : 'initial'}
            ref={titleRef}
            size="row"
            truncate
            className={`flex-1 min-w-0 ${MARQUEE_TITLE_CLASS} ${titleChanged ? 'title-morph-in' : ''}`}
          >
            {lane.title}
          </Text>
        )}

        {!editing && !blank && (
          <div
            className="shrink-0 flex items-center justify-end whitespace-nowrap"
            style={{ width: LANE_TRAILING_WIDTH }}
          >
            {/*
             * 静止态显示时间与图钉，hover 时整块换成操作区：两态同宽，
             * 因此标题宽度不随指针进出变化（这正是跑马灯误判溢出的来源）。
             */}
            <span
              className={`inline-flex items-center gap-1.5 ${menuOpen ? 'hidden' : 'group-hover:hidden'}`}
            >
              <span className="text-[11px] leading-4 tabular-nums text-text-tertiary">
                {formatRelativeTime(lane.updated_at)}
              </span>
              {/* 置顶标记与时间同一格：图标本身不发声，名称由这一层给。 */}
              {lane.pinned && (
                <span
                  role="img"
                  aria-label="已置顶"
                  title="已置顶"
                  className="shrink-0 inline-flex text-text-tertiary"
                >
                  <PinFillIcon size={14} />
                </span>
              )}
            </span>
            <RowActions active={menuOpen}>
              <Dropdown
                align="end"
                menuClassName="min-w-[160px]"
                onOpenChange={setMenuOpen}
                trigger={({ toggle }) => (
                  <IconButton
                    label="会话操作"
                    onClick={(event) => {
                      event.stopPropagation();
                      toggle();
                    }}
                  >
                    <MoreIcon size={14} />
                  </IconButton>
                )}
              >
                <MenuItem
                  icon={lane.pinned ? <PinFillIcon size={14} /> : <PinIcon size={14} />}
                  onSelect={onTogglePin}
                >
                  {lane.pinned ? '取消置顶' : '置顶会话'}
                </MenuItem>
                <MenuItem
                  icon={<EditIcon size={14} />}
                  onSelect={() => {
                    setDraft(lane.title);
                    setEditing(true);
                  }}
                >
                  重命名
                </MenuItem>
                <MenuSeparator />
                <MenuItem icon={<ArchiveIcon size={14} />} onSelect={onToggleArchive}>
                  {lane.archived ? '取消归档' : '归档会话'}
                </MenuItem>
              </Dropdown>
              {/* 悬浮即出现的快捷操作：归档与置顶，与既有工作台产品一致 */}
              <IconButton
                label={lane.archived ? '取消归档' : '归档会话'}
                onClick={(event) => {
                  event.stopPropagation();
                  onToggleArchive();
                }}
              >
                <ArchiveIcon size={14} />
              </IconButton>
              <IconButton
                label={lane.pinned ? '取消置顶' : '置顶会话'}
                onClick={(event) => {
                  event.stopPropagation();
                  onTogglePin();
                }}
              >
                {lane.pinned ? <PinFillIcon size={14} /> : <PinIcon size={14} />}
              </IconButton>
            </RowActions>
          </div>
        )}
      </div>
    </HoverCard>
  );
};

// ------------------------------------------------------------------ 工作区行

export interface WorkspaceRowProps {
  workspace: WorkspaceItem;
  /** 树形分组的缩进深度。 */
  depth: number;
  expanded: boolean;
  onToggle: () => void;
  onCreateLane: () => void;
  onRemove: () => void;
}

/** 一个工作区分组行：展开箭头与文件夹共用一个 24px 图标列。 */
export const WorkspaceRow: React.FC<WorkspaceRowProps> = ({
  workspace,
  depth,
  expanded,
  onToggle,
  onCreateLane,
  onRemove,
}) => {
  const [menuOpen, setMenuOpen] = useState(false);

  return (
    <HoverCard
      disabled={menuOpen}
      content={
        <>
          <HoverCardTitle>{workspace.name}</HoverCardTitle>
          <HoverCardLine mono>{workspace.path}</HoverCardLine>
          <HoverCardStatus>
            共 {workspace.lanes.length} 条会话
            {workspace.active ? ' · 当前工作区' : ''}
          </HoverCardStatus>
        </>
      }
    >
      <div
        onClick={onToggle}
        style={{ paddingLeft: depth * TREE_INDENT }}
        className={`${ROW_STATE} hover:bg-hover h-8 pr-1 cursor-pointer select-none`}
      >
        {/*
         * 图标列只有一份图形：收起时是单色文件夹，指针移入或已展开时换成 chevron。
         * chevron 只做旋转，不换成实心三角，因此展开与收起的形状是同一条线。
         */}
        <span className={ROW_ICON_COLUMN}>
          <span
            className={`items-center justify-center text-text-tertiary ${
              expanded ? 'flex' : 'hidden group-hover:flex'
            }`}
          >
            <ChevronRightIcon
              size={12}
              className={`transition-transform duration-150 ${expanded ? 'rotate-90' : ''}`}
            />
          </span>
          <span
            className={`items-center justify-center text-text-secondary ${
              expanded ? 'hidden' : 'flex group-hover:hidden'
            }`}
          >
            <FolderIcon size={14} />
          </span>
        </span>
        <Text size="row" strong truncate title={workspace.path} className="flex-1 min-w-0">
          {workspace.name}
        </Text>

        <div
          className="shrink-0 flex items-center justify-end whitespace-nowrap"
          style={{ width: WORKSPACE_TRAILING_WIDTH }}
        >
          <span className={`inline-flex items-center ${menuOpen ? 'hidden' : 'group-hover:hidden'}`}>
            {workspace.active ? (
              <Text tone="tertiary" size="meta">
                当前
              </Text>
            ) : (
              <Chip>{workspace.lanes.length}</Chip>
            )}
          </span>
          <RowActions active={menuOpen}>
            <Dropdown
              align="end"
              menuClassName="min-w-[180px]"
              onOpenChange={setMenuOpen}
              trigger={({ toggle }) => (
                <IconButton
                  label="工作区操作"
                  onClick={(event) => {
                    event.stopPropagation();
                    toggle();
                  }}
                >
                  <MoreIcon size={14} />
                </IconButton>
              )}
            >
              <MenuItem onSelect={onCreateLane}>新建会话</MenuItem>
              {workspace.removable && (
                <>
                  <MenuSeparator />
                  <MenuItem danger icon={<TrashIcon size={14} />} onSelect={onRemove}>
                    移除工作区
                  </MenuItem>
                </>
              )}
            </Dropdown>
            {/* 悬浮即出现的新建会话入口，与既有工作台产品一致 */}
            <IconButton
              label="新建会话"
              onClick={(event) => {
                event.stopPropagation();
                onCreateLane();
              }}
            >
              <IconNewChat size={14} />
            </IconButton>
          </RowActions>
        </div>
      </div>
    </HoverCard>
  );
};
