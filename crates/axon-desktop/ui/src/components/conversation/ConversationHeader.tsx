import React from 'react';
import {
  headerClusters,
  currentContextTokens,
  type HeaderClusterKind,
  type HeaderMetricsInput,
  type HeaderSegment,
  type HeaderSegmentTone,
} from '../../conversation/metrics';
import { contextUsageModel } from '../../conversation/contextUsage';
import {
  AgentIcon,
  ChecklistIcon,
  ClockIcon,
  CodeIcon,
  FolderIcon,
  GaugeIcon,
  ListIcon,
  RingIcon,
} from '../../icons';
import { ContextUsagePopover } from './ContextUsagePopover';

/*
 * 主区域顶部指标条。
 *
 * 三簇按语义排开：身份（在哪、用什么模型）、预算（上下文与计划）、性能（首字、速率、耗时）。
 * 此前是十段用「·」连成一条，装不下就从中间硬裁，读起来像一串调试输出。
 *
 * 装不下时整簇收起：断点由宽到窄依次让预算与性能退场，身份簇始终在。
 * 行高恒 36px、字号恒 13/20，因此隐藏与出现都不会让下方正文跳位。
 *
 * 指标文本是可读的事实（模型、耗时、命中率都是用户会想抄走的东西），
 * 因此这一行允许选中：双击选词、拖动选择、右键复制、Ctrl+C 都照常。
 */

/** 段色调到文本色的映射；文字一律取墨水档。 */
const TONE_CLASS: Record<HeaderSegmentTone, string> = {
  default: 'text-text-secondary',
  warning: 'text-gold-ink',
  critical: 'text-danger',
};

/**
 * 簇的可见断点。
 *
 * 以**主区域宽度**为基准而不是视口：右侧停靠栏展开后，视口没变但正文列只剩 504px，
 * 按视口判定会把三簇全画出来再从中间硬裁。容器查询让整簇按真实可用宽度退场。
 */
const CLUSTER_VISIBILITY: Record<HeaderClusterKind, string> = {
  identity: 'flex',
  budget: 'hidden @min-[560px]:flex',
  performance: 'hidden @min-[700px]:flex',
};

/** 上下文占用百分比；无窗口定义时返回 null。 */
function contextPercentValue(metrics: HeaderMetricsInput): number | null {
  if (metrics.contextWindow === null || metrics.contextWindow <= 0 || metrics.usage === null) {
    return null;
  }
  const used = currentContextTokens(metrics.usage);
  if (used === 0) return null;
  return Math.min(100, Math.floor((used * 100) / metrics.contextWindow));
}

/**
 * 指标图标：线性图形，14px。
 *
 * 不再叠加 `opacity-70`：透明度乘到继承色上会把同一行的图标与文字拆成两个灰度，
 * 图标色由父级的文本色统一决定。
 */
function metricIcon(segment: HeaderSegment, contextPercent: number | null): React.ReactNode {
  const size = 14;
  switch (segment.icon) {
    case 'spark':
      return <AgentIcon size={size} className="shrink-0" />;
    case 'folder':
      return <FolderIcon size={size} className="shrink-0" />;
    case 'ring':
      return <RingIcon size={size} className="shrink-0" ratio={(contextPercent ?? 0) / 100} />;
    case 'clock':
      return <ClockIcon size={size} className="shrink-0" />;
    case 'gauge':
      return <GaugeIcon size={size} className="shrink-0" />;
    case 'list':
      return <ListIcon size={size} className="shrink-0" />;
    case 'code':
      return <CodeIcon size={size} className="shrink-0" />;
    default:
      return null;
  }
}

export interface ConversationHeaderProps extends HeaderMetricsInput {
  /** 指标条右侧的操作区。 */
  trailing?: React.ReactNode;
  /** 当前会话是否存在待答复提问。 */
  hasPendingQuestion?: boolean;
}

export const ConversationHeader: React.FC<ConversationHeaderProps> = ({
  trailing,
  hasPendingQuestion = false,
  ...metrics
}) => {
  const clusters = headerClusters(metrics);
  /** 上下文段带弹窗：其余段是纯读数，悬停不展开任何东西。 */
  const contextUsage = contextUsageModel(metrics.usage, metrics.contextWindow);
  const contextPercent = contextPercentValue(metrics);
  return (
    <header
      data-tauri-drag-region
      className="@container shrink-0 h-9 px-4 flex items-center gap-3 hairline-b border-border-hairline bg-canvas select-text"
    >
      <div className="flex-1 min-w-0 flex items-center gap-3 overflow-hidden whitespace-nowrap">
        {hasPendingQuestion && (
          <span
            className="shrink-0 inline-flex items-center gap-1 rounded-xs px-1.5 text-xs text-gold-ink"
            title="当前会话存在待答复提问"
          >
            <ChecklistIcon size={14} className="shrink-0" aria-hidden="true" />
            待答复
          </span>
        )}
        {clusters.map((cluster, position) => (
          <div
            key={cluster.kind}
            data-header-cluster={cluster.kind}
            className={`${CLUSTER_VISIBILITY[cluster.kind]} min-w-0 items-center gap-1.5 ${
              position > 0 ? 'pl-3 hairline-l border-border-hairline' : ''
            }`}
          >
            {cluster.segments.map((segment) => {
              // 上下文段挂了指标浮层：它的原生 title 会与浮层同时冒出来，
              // 因此这一段不设 title；其余段保留，读数被截断时它仍是全文入口。
              const withPopover = segment.kind === 'context' && contextUsage !== null;
              const reading = (
                <span
                  className={`min-w-0 ${
                    segment.kind === 'cwd' ? '' : 'shrink-0'
                  } inline-flex items-center gap-1 text-row tabular-nums ${
                    segment.kind === 'cwd' ? 'max-w-[260px]' : ''
                  } ${
                    segment.kind === 'status'
                      ? 'text-text-primary font-medium'
                      : TONE_CLASS[segment.tone]
                  }`}
                  title={withPopover ? undefined : segment.text}
                >
                  {metricIcon(segment, contextPercent)}
                  <span className="truncate">{segment.text}</span>
                </span>
              );
              return withPopover ? (
                <ContextUsagePopover key={segment.kind} model={contextUsage}>
                  {reading}
                </ContextUsagePopover>
              ) : (
                <React.Fragment key={segment.kind}>{reading}</React.Fragment>
              );
            })}
          </div>
        ))}
      </div>
      {trailing !== undefined && <div className="shrink-0 flex items-center gap-0.5">{trailing}</div>}
    </header>
  );
};
