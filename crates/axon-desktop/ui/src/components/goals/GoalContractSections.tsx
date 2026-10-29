import React from 'react';

import { AgentIcon, AlertCircleIcon, CheckIcon, ListIcon } from '../../icons';
import { StateDot, Text } from '../ui/surfaces';

/*
 * 目标契约正文：供执行会话顶部的可展开概览面板与目标详情弹窗共用。
 *
 * 遵循「点睛而非涂满」的阅读优先规范：
 * 1. 卡片表面统一采用安静的中性浅底（bg-surface-sunken/45），不铺大面积彩色底板、不套多层白盒；
 * 2. 颜色仅落在标题行首的 13px 微图标与状态圆点上作语义区分（蓝：目标说明，琥珀：执行步骤，翠绿：验收标准）；
 * 3. 所有标题与正文全部保持高对比碳素墨色（--color-text-prose / --color-text-secondary），零视觉噪音。
 */

export interface GoalContractSectionsProps {
  /** 目标完整说明或预览。 */
  objective: string;
  /** 步骤列表。 */
  steps: readonly string[];
  /** 验收标准列表。 */
  acceptance: readonly string[];
  /** 目标是否已完成。 */
  completed?: boolean;
  /** 布局变体：会话舞台用三列等高网格，详情弹窗用两列等高网格。 */
  variant?: 'columns' | 'dialog';
  /** 目标正文是否被服务端按上限截断。 */
  objectiveTruncated?: boolean;
  /** 步骤或验收列表是否被服务端按上限截断。 */
  listsTruncated?: boolean;
  /** 取消原因。 */
  cancellationReason?: string | null;
  /** 最近一次结果摘要。 */
  latestResultSummary?: string | null;
  /** 最近一次结果是否成功。 */
  latestResultSucceeded?: boolean | null;
  /** 详情正在读取中（首次尚未拿到步骤与验收）。 */
  loading?: boolean;
  /** 详情读取失败原因。 */
  error?: string | null;
}

export const GoalContractSections: React.FC<GoalContractSectionsProps> = ({
  objective,
  steps,
  acceptance,
  variant = 'columns',
  objectiveTruncated = false,
  listsTruncated = false,
  cancellationReason = null,
  latestResultSummary = null,
  latestResultSucceeded = null,
  loading = false,
  error = null,
}) => {
  const hasCancelReason =
    cancellationReason !== null && cancellationReason.trim().length > 0;
  const hasLatestResult =
    latestResultSummary !== null && latestResultSummary.trim().length > 0;

  const objectiveCard = (
    <div className="h-full min-h-[132px] rounded-md bg-surface-sunken/45 hairline border-border-hairline p-3.5 flex flex-col justify-between gap-2.5">
      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <span className="h-5 inline-flex items-center gap-1.5">
            <AgentIcon size={13} className="shrink-0 text-text-link" />
            <Text size="xs" strong>
              目标说明
            </Text>
          </span>
          {objectiveTruncated && (
            <Text tone="caption" size="micro">
              仅展示前 20,000 字
            </Text>
          )}
        </div>
        <p className="text-row-loose text-text-prose whitespace-pre-wrap break-words text-pretty">
          {objective.trim().length > 0 ? objective : '暂无目标说明'}
        </p>
      </div>

      {(hasCancelReason || hasLatestResult) && (
        <div className="pt-2.5 hairline-t border-border-hairline">
          {hasCancelReason ? (
            <div className="flex items-start gap-1.5 text-xs leading-5 text-danger break-words text-pretty">
              <span className="shrink-0 h-5 flex items-center justify-center">
                <StateDot state="error" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="font-medium">取消原因 · </span>
                {cancellationReason}
              </span>
            </div>
          ) : (
            <div className="flex items-start gap-1.5 text-xs leading-5 text-text-secondary break-words text-pretty">
              <span className="shrink-0 h-5 flex items-center justify-center">
                <StateDot
                  state={
                    latestResultSucceeded === false
                      ? 'warning'
                      : latestResultSucceeded === true
                        ? 'done'
                        : 'idle'
                  }
                />
              </span>
              <span className="min-w-0 flex-1">
                <span className="font-medium text-text-primary">最近结果 · </span>
                {latestResultSummary}
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  );

  const stepsCard = (
    <div className="h-full min-h-[132px] rounded-md bg-surface-sunken/45 hairline border-border-hairline p-3.5 flex flex-col gap-2.5">
      <div className="flex items-center justify-between gap-2">
        <span className="h-5 inline-flex items-center gap-1.5">
          <ListIcon size={13} className="shrink-0 text-gold-ink" />
          <Text size="xs" strong>
            执行步骤
          </Text>
        </span>
        <Text tone="tertiary" size="micro" mono className="tabular-nums">
          {steps.length}
        </Text>
      </div>
      {steps.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <Text tone="caption" size="xs">
            {loading ? '正在读取步骤...' : '暂无预设步骤'}
          </Text>
        </div>
      ) : (
        <ol className="space-y-2">
          {steps.map((step, index) => (
            <li key={`${index}-${step}`} className="flex items-start gap-2.5">
              <span
                aria-hidden="true"
                className="shrink-0 w-5 h-5 rounded-xs bg-surface hairline border-border-hairline inline-flex items-center justify-center font-mono text-micro tabular-nums text-gold-ink"
              >
                {index + 1}
              </span>
              <span className="min-w-0 flex-1 text-row leading-5 text-text-prose break-words text-pretty">
                {step}
              </span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );

  const acceptanceCard = (
    <div className="h-full min-h-[132px] rounded-md bg-surface-sunken/45 hairline border-border-hairline p-3.5 flex flex-col gap-2.5">
      <div className="flex items-center justify-between gap-2">
        <span className="h-5 inline-flex items-center gap-1.5">
          <CheckIcon size={13} className="shrink-0 text-ok-ink" />
          <Text size="xs" strong>
            验收标准
          </Text>
        </span>
        <Text tone="tertiary" size="micro" mono className="tabular-nums">
          {acceptance.length}
        </Text>
      </div>
      {acceptance.length === 0 ? (
        <div className="flex-1 flex items-center justify-center">
          <Text tone="caption" size="xs">
            {loading ? '正在读取验收标准...' : '暂无验收细则'}
          </Text>
        </div>
      ) : (
        <ul className="space-y-2">
          {acceptance.map((item, index) => (
            <li key={`${index}-${item}`} className="flex items-start gap-2.5">
              <span
                aria-hidden="true"
                className="shrink-0 w-5 h-5 rounded-xs bg-surface hairline border-border-hairline inline-flex items-center justify-center text-ok-ink"
              >
                <CheckIcon size={12} />
              </span>
              <span className="min-w-0 flex-1 text-row leading-5 text-text-prose break-words text-pretty">
                {item}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );

  return (
    <div className="space-y-3">
      {error !== null && (
        <div className="flex items-start gap-2 rounded-xs bg-danger-soft hairline border-danger/25 px-3 py-2">
          <AlertCircleIcon size={14} className="shrink-0 mt-0.5 text-danger" />
          <Text tone="danger" size="xs" className="block break-words">
            {error}
          </Text>
        </div>
      )}

      {variant === 'columns' ? (
        <div className="grid grid-cols-1 lg:grid-cols-3 gap-3 items-stretch">
          {objectiveCard}
          {stepsCard}
          {acceptanceCard}
        </div>
      ) : (
        <div className="space-y-3">
          {objectiveCard}
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 items-stretch">
            {stepsCard}
            {acceptanceCard}
          </div>
        </div>
      )}

      {listsTruncated && (
        <Text tone="caption" size="xs" className="block">
          列表较长，仅展示前 64 项
        </Text>
      )}
    </div>
  );
};
