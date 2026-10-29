import React from 'react';

import { ChecklistIcon } from '../../icons';
import type { GoalDetailView } from '../../goalTypes';
import { goalContractSummary } from '../../goals/model';
import { DisclosureRow } from '../conversation/DisclosureRow';
import { GoalContractSections } from './GoalContractSections';

/*
 * 目标概览面板：执行会话顶部全宽可展开的目标契约面板。
 *
 * 收起时给出一行目标摘要与步骤/验收计数；展开时切换为三列等宽等高卡片网格
 * （目标说明 / 执行步骤 / 验收标准）。当会话尚无执行记录时默认展开，
 * 让进入页面的第一眼即饱满可读。
 */

export interface GoalContractBarProps {
  detail: GoalDetailView | null;
  /** 摘要兜底：详情还没回来时先用列表里的预览。 */
  objectivePreview: string;
  /** 步骤数兜底。 */
  stepCount: number;
  /** 是否默认展开（无执行记录时为 true）。 */
  defaultOpen?: boolean;
  /** 详情读取失败的原因。 */
  error?: string | null;
}

export const GoalContractBar: React.FC<GoalContractBarProps> = ({
  detail,
  objectivePreview,
  stepCount,
  defaultOpen = true,
  error = null,
}) => {
  const [open, setOpen] = React.useState(defaultOpen);
  const preview =
    detail !== null && detail.objective.length > 0 ? detail.objective : objectivePreview;
  const steps = detail?.steps ?? [];
  const acceptance = detail?.acceptance ?? [];
  const resolvedStepCount = detail?.goal.stepCount ?? stepCount;
  const summaryLabel = goalContractSummary(resolvedStepCount, acceptance.length);
  const completed = detail?.goal.status === 'completed';

  return (
    <section
      aria-label="目标概览"
      className="w-full rounded-md hairline border-border shadow-card bg-surface overflow-hidden"
    >
      <div className="px-2.5 py-1">
        <DisclosureRow
          open={open}
          onToggle={() => setOpen((current) => !current)}
          icon={<ChecklistIcon size={14} />}
          label={open ? '目标与验收标准' : '目标'}
          title={open ? undefined : preview}
          titleText={preview}
          meta={summaryLabel}
        />
      </div>
      {open && (
        <div className="disclosure-enter hairline-t border-border-hairline p-3.5 max-h-[44vh] overflow-y-auto scroll-visible">
          <GoalContractSections
            variant="columns"
            completed={completed}
            objective={preview}
            steps={steps}
            acceptance={acceptance}
            objectiveTruncated={detail?.objectiveTruncated === true}
            listsTruncated={detail?.listsTruncated === true}
            cancellationReason={detail?.cancellationReason ?? null}
            latestResultSummary={detail?.latestResult?.summaryPreview ?? null}
            latestResultSucceeded={detail?.latestResult?.succeeded ?? null}
            error={error}
          />
        </div>
      )}
    </section>
  );
};
