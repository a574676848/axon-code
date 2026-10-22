import React from 'react';
import type { PlanProjection } from '../../conversation/transcript';
import { PlanCard } from './PlanCard';

/*
 * 固定计划坞：钉在输入坞正上方，只显示「当前计划」，不随时间线滚走。
 *
 * 与时间线里的计划单元分工明确：那边保留每次 planUpdate 的原子记录，这边读会话级
 * 投影给出此刻的计划。没有计划时整块不渲染，不占高度也不留空壳。
 *
 * 默认收起：它长期占位在输入框上方，展开是用户的显式动作，不是默认状态。
 */

export const PlanDock: React.FC<{ plan: PlanProjection | null }> = ({ plan }) => {
  if (plan === null || plan.steps.length === 0) return null;
  return (
    /*
     * 宽度与输入坞对齐（同一份 max-w-chat 与水平内边距），
     * 因此它读起来是输入坞的「上盖」而不是正文的一部分。
     */
    <section
      aria-label="计划"
      data-testid="plan-dock"
      className="shrink-0 flex justify-center px-4 pt-2"
    >
      <div className="w-full max-w-chat">
        <PlanCard title={plan.title} steps={plan.steps} defaultOpen={false} />
      </div>
    </section>
  );
};
