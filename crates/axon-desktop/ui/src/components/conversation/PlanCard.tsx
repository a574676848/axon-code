import React from 'react';
import { PLAN_LABEL, planHeading } from '../../conversation/transcript';
import { ChecklistIcon } from '../../icons';
import { StateDot, Text, type StateDotState } from '../ui/surfaces';
import { DisclosureRow, DISCLOSURE_BODY_INDENT } from './DisclosureRow';

/*
 * 计划卡：标题行给计数摘要，点击展开或收起步骤。
 *
 * 时间线单元与输入坞上方的固定计划坞共用这一份实现：两处形态完全一致，
 * 差别只有首次渲染是展开还是收起，因此用 prop 参数化而不是各写一遍。
 *
 * 步骤状态点的语义与参考实现的任务清单同表：完成、进行中、失败各一色，
 * 待处理与已跳过都是静息态，共用灰点。
 */

/** 步骤状态归类，取值与后端 planUpdate 的状态字符串一致。 */
export type PlanStepKind = 'completed' | 'running' | 'failed' | 'skipped' | 'pending';

function planStepKind(status: string): PlanStepKind {
  switch (status) {
    case 'completed':
    case 'complete':
      return 'completed';
    case 'running':
    case 'in_progress':
    case 'active':
      return 'running';
    case 'failed':
    case 'error':
      return 'failed';
    case 'skipped':
      return 'skipped';
    default:
      return 'pending';
  }
}

const PLAN_STEP_LABEL: Record<PlanStepKind, string> = {
  completed: '已完成',
  running: '进行中',
  failed: '失败',
  skipped: '已跳过',
  pending: '待处理',
};

/** 步骤状态点到状态语义的映射。 */
function planStepState(kind: PlanStepKind): StateDotState {
  if (kind === 'completed') return 'done';
  if (kind === 'running') return 'ongoing';
  if (kind === 'failed') return 'error';
  return 'idle';
}

/** 计数摘要：只列非零项，用中点连接。 */
function planSummary(steps: readonly { status: string }[]): string {
  const tally: Record<PlanStepKind, number> = {
    completed: 0,
    running: 0,
    failed: 0,
    skipped: 0,
    pending: 0,
  };
  for (const step of steps) tally[planStepKind(step.status)] += 1;
  return (['completed', 'running', 'pending'] as PlanStepKind[])
    .filter((kind) => tally[kind] > 0)
    .map((kind) => `${tally[kind]} ${PLAN_STEP_LABEL[kind]}`)
    .join(' · ');
}

export interface PlanCardProps {
  /** 计划标题；为空或与固定标签「计划」同文时不重复显示。 */
  title: string;
  /** 步骤明细，顺序与后端下发一致。 */
  steps: readonly { step: string; status: string }[];
  /** 首次渲染是否展开；此后由用户点击控制。 */
  defaultOpen: boolean;
}

/** 计划卡：时间线单元与固定计划坞共用的一处实现。 */
export const PlanCard: React.FC<PlanCardProps> = ({ title, steps, defaultOpen }) => {
  const [open, setOpen] = React.useState(defaultOpen);
  const summary = React.useMemo(() => planSummary(steps), [steps]);
  const heading = planHeading(title);
  const accessibleTitle = [heading ?? PLAN_LABEL, summary]
    .filter((part) => part.length > 0)
    .join(' ');
  return (
    <div className="w-full rounded-md hairline border-border-hairline bg-surface px-1 py-0.5">
      <DisclosureRow
        open={open}
        onToggle={() => setOpen((value) => !value)}
        icon={<ChecklistIcon size={14} />}
        label={PLAN_LABEL}
        title={heading}
        titleText={accessibleTitle}
        meta={summary.length > 0 ? summary : undefined}
      />
      {open && (
        <ul className={`${DISCLOSURE_BODY_INDENT} mr-1 mb-1 border-l-[2px] border-border-hairline pl-3 space-y-1`}>
          {steps.map((step, index) => {
            const kind = planStepKind(step.status);
            return (
              <li key={`${index}:${step.step}`} className="flex items-start gap-2">
                {/* 标记槽与正文首行等高并居中，圆点与转圈尺寸不同也不会错位。 */}
                <span className="shrink-0 w-4 h-5 flex items-center justify-center">
                  <StateDot state={planStepState(kind)} />
                </span>
                <Text
                  size="meta"
                  tone={kind === 'completed' ? 'tertiary' : 'secondary'}
                  className="break-words"
                >
                  {step.step}
                </Text>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
};
