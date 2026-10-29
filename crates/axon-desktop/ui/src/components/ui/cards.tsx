import React from 'react';
import { SearchIcon, SettingsIcon } from '../../icons';
import { Input } from './controls';
import { Dialog } from './overlays';
import { Chip, EmptyState, LoadingState, Text } from './surfaces';

/*
 * 卡片列表：供 MCP 与技能等「同类条目集合」复用。
 *
 * 形态对齐工作台里的条目卡：标题行放名称与类型标签，中部一行说明，
 * 底部一行次要信息与唯一入口；两列自适应网格。搜索框固定在顶部，
 * 只过滤呈现，不改动数据。
 */

interface CardListProps {
  /** 搜索关键字。 */
  query: string;
  /** 关键字变化。 */
  onQueryChange: (next: string) => void;
  /** 搜索框占位文案。 */
  placeholder: string;
  /** 概览文案，例如「共 4 台服务器」。 */
  summary: string;
  /** 首屏加载中。 */
  loading?: boolean;
  /** 过滤后为空时的标题。 */
  emptyTitle: string;
  /** 过滤后为空时的补充说明。 */
  emptyDescription?: string;
  /** 卡片集合。 */
  children: React.ReactNode;
}

export const CardList: React.FC<CardListProps> = ({
  query,
  onQueryChange,
  placeholder,
  summary,
  loading = false,
  emptyTitle,
  emptyDescription,
  children,
}) => {
  const empty = React.Children.count(children) === 0;
  return (
    <div className="@container space-y-3">
      <Input
        icon={<SearchIcon size={13} />}
        value={query}
        placeholder={placeholder}
        onChange={(event) => onQueryChange(event.target.value)}
      />
      <Text tone="secondary" size="xs" className="block">
        {summary}
      </Text>
      {loading ? (
        <LoadingState label="正在读取" />
      ) : empty ? (
        <EmptyState title={emptyTitle} description={emptyDescription} />
      ) : (
        <div className="grid grid-cols-1 @[520px]:grid-cols-2 gap-3">{children}</div>
      )}
    </div>
  );
};

export interface EntityCardStatusCue {
  /** 标签，例如「下一步」「等待」。 */
  label: string;
  /** 说明正文。 */
  text: string;
  /** 语义色调：默认中性下沉槽，warning 取柔和琥珀提示槽。 */
  tone?: 'default' | 'warning';
}

interface EntityCardProps {
  /** 卡片标题。 */
  title: string;
  /** 标题右侧唯一的类型标签。 */
  badges?: React.ReactNode;
  /** 中部说明；为空时不占行。 */
  description?: string;
  /** 命令或地址：整张卡片只出现一次，单行截断并带原生提示。 */
  locator?: string;
  /** 结构化进展提示带（目标看板用于区分「下一步」与「等待」）。 */
  statusCue?: EntityCardStatusCue | null;
  /** 是否锁定固定卡片高度（看板卡片要求尺寸完全一致）。 */
  uniformHeight?: boolean;
  /** 底部左侧的次要信息。 */
  meta?: string;
  /** 底部左侧的结构化次要信息节点（优先于纯文本 meta）。 */
  metaNode?: React.ReactNode;
  /** 点击卡片主体。 */
  onOpen?: () => void;
  /** 底部右下的查看配置入口。 */
  onInspect?: () => void;
  /** 查看配置的无障碍名称。 */
  inspectLabel?: string;
  /** 行首节点，例如状态点；不给则标题从最左开始。 */
  leading?: React.ReactNode;
  /** 定位行是否等宽，默认等宽。 */
  locatorMono?: boolean;
  /** 页脚动作图标，默认齿轮。 */
  actionIcon?: React.ReactNode;
}

/**
 * 条目卡片：一个对象只出现一次身份信息。
 *
 * 标题 14/20 500 单行截断，右上只留一个类型或状态标签；说明 12/18 固定行槽；
 * 底部左侧是有意义的次要信息，右下只有一处文字入口。
 * 卡片本身不再重复标题里的标识、命令或地址，也不在卡片内再套带边框的子盒。
 */
export const EntityCard: React.FC<EntityCardProps> = ({
  title,
  badges,
  description,
  locator,
  statusCue,
  uniformHeight = false,
  meta,
  metaNode,
  onOpen,
  onInspect,
  inspectLabel = '查看配置',
  leading,
  locatorMono = true,
  actionIcon,
}) => {
  const hasDescription = description !== undefined && description.trim().length > 0;
  return (
    <div
      className={`card-interactive flex flex-col rounded-md hairline border-border shadow-card bg-surface overflow-hidden ${
        uniformHeight ? 'h-[148px]' : ''
      }`}
    >
      <button
        type="button"
        onClick={onOpen}
        className={`flex-1 min-w-0 flex flex-col justify-between text-left px-3.5 pt-3 pb-2.5 focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus ${
          uniformHeight ? 'gap-1.5' : 'space-y-2'
        }`}
      >
        <span className="flex items-center gap-2 min-w-0">
          {leading !== undefined && (
            <span className="shrink-0 inline-flex items-center h-5">{leading}</span>
          )}
          <span className="min-w-0 flex-1 text-body font-medium leading-5 text-text-primary truncate">
            {title}
          </span>
          {badges !== undefined && <span className="shrink-0">{badges}</span>}
        </span>
        {(hasDescription || uniformHeight) && (
          <Text
            tone="secondary"
            size="xs"
            className={`block leading-[18px] break-words text-pretty ${
              uniformHeight ? 'line-clamp-2 min-h-[36px]' : 'line-clamp-3'
            }`}
          >
            {hasDescription ? description : title}
          </Text>
        )}
        {statusCue !== undefined && statusCue !== null && (
          <span
            title={`${statusCue.label} · ${statusCue.text}`}
            className="flex items-center gap-1.5 min-w-0 h-5"
          >
            <span
              className={`shrink-0 inline-flex items-center px-1.5 py-px rounded-xs text-micro font-medium leading-4 ${
                statusCue.tone === 'warning'
                  ? 'bg-gold-soft text-gold-ink'
                  : 'bg-surface-sunken text-text-secondary'
              }`}
            >
              {statusCue.label}
            </span>
            <span className="min-w-0 flex-1 text-xs leading-5 text-text-tertiary truncate">
              {statusCue.text}
            </span>
          </span>
        )}
        {locator !== undefined && locator.length > 0 && (
          <Text
            tone="tertiary"
            size="code"
            mono={locatorMono}
            truncate
            className="block"
            title={locator}
          >
            {locator}
          </Text>
        )}
      </button>
      <div className="shrink-0 h-9 flex items-center gap-2 px-3.5 bg-surface-sunken/25 hairline-t border-border-hairline">
        {metaNode !== undefined ? (
          <div className="flex-1 min-w-0">{metaNode}</div>
        ) : (
          meta !== undefined && (
            <Text
              tone="tertiary"
              size="micro"
              truncate
              className="flex-1 min-w-0 tabular-nums"
              title={meta}
            >
              {meta}
            </Text>
          )
        )}
        <button
          type="button"
          onClick={() => onInspect?.()}
          className="ml-auto shrink-0 inline-flex items-center gap-1 h-6 px-2 rounded-xs text-xs font-medium text-text-secondary transition-colors duration-100 hover:bg-hover hover:text-text-primary focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
        >
          <span>{inspectLabel}</span>
          <span className="card-action-icon text-text-tertiary">
            {actionIcon ?? <SettingsIcon size={12} />}
          </span>
        </button>
      </div>
    </div>
  );
};

/** 详情弹窗中的一行字段。 */
export interface DetailField {
  /** 字段名称。 */
  label: string;
  /** 字段值。 */
  value: string;
  /** 是否以等宽字体呈现。 */
  mono?: boolean;
  /** 值的呈现形态：文本段落，或标签集合。 */
  kind?: 'text' | 'chips';
}

interface CardDetailDialogProps {
  /** 是否打开。 */
  open: boolean;
  /** 关闭。 */
  onClose: () => void;
  /** 标题。 */
  title: string;
  /** 字段集合。 */
  fields: DetailField[];
  /** 右下角主操作。 */
  primary?: { label: string; icon?: React.ReactNode; onClick: () => void };
  /** 主操作左侧的次操作。 */
  secondary?: {
    label: string;
    icon?: React.ReactNode;
    danger?: boolean;
    busy?: boolean;
    onClick: () => void;
  };
}

/**
 * 卡片详情弹窗：把「查看配置」的统一呈现抽成组件。
 *
 * MCP 与技能等卡片集合共用同一套详情排版：字段名在上、值在下，
 * 段落值按行折行，标签值横向排布。这样新增卡片集合时无需再写一套详情视图。
 */
export const CardDetailDialog: React.FC<CardDetailDialogProps> = ({
  open,
  onClose,
  title,
  fields,
  primary,
  secondary,
}) => (
  <Dialog open={open} onClose={onClose} title={title} size="md" primary={primary} secondary={secondary}>
    {fields.map((field) => (
      <div key={field.label} className="space-y-1">
        <Text tone="secondary" size="xs" className="block">
          {field.label}
        </Text>
        {field.kind === 'chips' ? (
          <div className="flex flex-wrap gap-1.5">
            {field.value.length === 0 ? (
              <Text tone="tertiary" size="row">
                无
              </Text>
            ) : (
              field.value.split('\n').map((item) => <Chip key={item}>{item}</Chip>)
            )}
          </div>
        ) : (
          <Text
            size="row"
            mono={field.mono === true}
            className="block leading-relaxed break-all whitespace-pre-line"
          >
            {field.value.length > 0 ? field.value : '无'}
          </Text>
        )}
      </div>
    ))}
  </Dialog>
);
