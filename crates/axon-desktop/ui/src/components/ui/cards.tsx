import React from 'react';
import { SearchIcon, SettingsIcon } from '../../icons';
import { Button, Input } from './controls';
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

interface EntityCardProps {
  /** 卡片标题。 */
  title: string;
  /** 标题右侧唯一的类型标签。 */
  badges?: React.ReactNode;
  /** 中部说明。 */
  description: string;
  /** 命令或地址：整张卡片只出现一次，单行截断并带原生提示。 */
  locator?: string;
  /** 底部左侧的次要信息。 */
  meta?: string;
  /** 点击卡片主体。 */
  onOpen?: () => void;
  /** 底部右下的查看配置入口。 */
  onInspect?: () => void;
  /** 查看配置的无障碍名称。 */
  inspectLabel?: string;
}

/**
 * 条目卡片：一个对象只出现一次身份信息。
 *
 * 标题 14/20 500 单行截断，右上只留一个类型标签；说明 12/18 最多三行；
 * 底部左侧是有意义的次要信息，右下只有一处「查看配置」文字入口。
 * 卡片本身不再重复标题里的标识、命令或地址。
 */
export const EntityCard: React.FC<EntityCardProps> = ({
  title,
  badges,
  description,
  locator,
  meta,
  onOpen,
  onInspect,
  inspectLabel = '查看配置',
}) => (
  <div className="flex flex-col rounded-md hairline border-border shadow-card bg-surface overflow-hidden transition-colors duration-150 hover:border-border-strong">
    <button
      type="button"
      onClick={onOpen}
      className="flex-1 min-w-0 text-left p-3.5 space-y-2 hover:bg-hover"
    >
      <span className="flex items-start gap-2 min-w-0">
        <Text strong truncate className="min-w-0 flex-1">
          {title}
        </Text>
        {badges}
      </span>
      <Text
        tone="secondary"
        size="xs"
        className="leading-[18px] line-clamp-3 break-words"
      >
        {description}
      </Text>
      {locator !== undefined && locator.length > 0 && (
        <Text
          tone="tertiary"
          size="code"
          mono
          truncate
          className="block"
          title={locator}
        >
          {locator}
        </Text>
      )}
    </button>
    <div className="shrink-0 flex items-center gap-2 px-3.5 py-2 hairline-t border-border-hairline">
      {meta !== undefined && (
        <Text tone="tertiary" size="2xs" truncate className="flex-1 min-w-0" title={meta}>
          {meta}
        </Text>
      )}
      <Button variant="ghost" size="sm" className="ml-auto" onClick={() => onInspect?.()}>
        <SettingsIcon size={13} />
        {inspectLabel}
      </Button>
    </div>
  </div>
);

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
