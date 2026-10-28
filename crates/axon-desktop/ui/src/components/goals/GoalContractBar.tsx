import React from 'react';

import { ChecklistIcon } from '../../icons';
import type { GoalDetailView } from '../../goalTypes';
import { DisclosureBody, DisclosureRow } from '../conversation/DisclosureRow';
import { Text } from '../ui/surfaces';

/*
 * 目标契约条：目标、步骤与验收。
 *
 * 用既有可展开行承载，因此与时间线里的计划、执行、改动是同一套折叠语义：
 * 收起时只留一句摘要与步数，展开才摊开正文。截断与取消原因如实说明，
 * 不在界面上假装内容完整。
 */

export interface GoalContractBarProps {
  detail: GoalDetailView | null;
  /** 摘要兜底：详情还没回来时先用列表里的预览。 */
  objectivePreview: string;
  /** 步骤数兜底。 */
  stepCount: number;
  /** 详情读取失败的原因。 */
  error?: string | null;
}

export const GoalContractBar: React.FC<GoalContractBarProps> = ({
  detail,
  objectivePreview,
  stepCount,
  error = null,
}) => {
  const [open, setOpen] = React.useState(false);
  const preview =
    detail !== null && detail.objective.length > 0 ? detail.objective : objectivePreview;
  const steps = detail?.steps ?? [];
  const acceptance = detail?.acceptance ?? [];
  const stepsLabel = `${detail?.goal.stepCount ?? stepCount} 步`;

  return (
    <div className="min-w-0">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((current) => !current)}
        icon={<ChecklistIcon size={12} />}
        label="目标"
        title={preview}
        titleText={preview}
        meta={stepsLabel}
      />
      {open && (
        <DisclosureBody framed>
          <div className="max-h-[40vh] overflow-y-auto scroll-visible space-y-2 py-1 pr-1">
            {error !== null && (
              <Text tone="danger" size="xs" className="block">
                {error}
              </Text>
            )}
            <Text size="xs" className="block break-words whitespace-pre-wrap">
              {preview}
            </Text>
            {detail?.objectiveTruncated === true && (
              <Text tone="tertiary" size="2xs" className="block">
                目标正文超过 20000 字，此处已截断
              </Text>
            )}
            {steps.length > 0 && (
              <div className="space-y-1">
                <Text tone="secondary" size="2xs" className="block">
                  步骤
                </Text>
                {steps.map((step, index) => (
                  <Text key={`${index}-${step}`} size="xs" className="block break-words">
                    {`${index + 1}. ${step}`}
                  </Text>
                ))}
              </div>
            )}
            {acceptance.length > 0 && (
              <div className="space-y-1">
                <Text tone="secondary" size="2xs" className="block">
                  验收
                </Text>
                {acceptance.map((item, index) => (
                  <Text key={`${index}-${item}`} size="xs" className="block break-words">
                    {`- ${item}`}
                  </Text>
                ))}
              </div>
            )}
            {detail?.listsTruncated === true && (
              <Text tone="tertiary" size="2xs" className="block">
                步骤或验收超过 64 条或单条超过 2000 字，此处已截断
              </Text>
            )}
            {detail?.cancellationReason !== null && detail !== null && (
              <Text tone="danger" size="xs" className="block">
                取消原因 · {detail.cancellationReason}
              </Text>
            )}
          </div>
        </DisclosureBody>
      )}
    </div>
  );
};
