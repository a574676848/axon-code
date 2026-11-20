import React from 'react';
import { ChecklistIcon, ChevronDownIcon } from '../../icons';
import { Dropdown } from '../ui/overlays';
import { SectionHeader } from './JobPanel';
import { StateDot, type StateDotState } from '../ui/surfaces';
import type { GoalCardView, GoalStatus } from '../../goalTypes';

/*
 * 会话 header 里的目标清单。
 *
 * 与子代理、后台任务同一处：它们都是「这条会话产生的过程事实」，只有真的存在时才
 * 长出控件。目标由本会话创建，卡片给出状态与下一步，点一行就进入它的执行会话。
 */

/** 状态到状态点的映射，与看板的语义一致。 */
function statusDot(status: GoalStatus): StateDotState {
  switch (status) {
    case 'active':
      return 'ongoing';
    case 'completed':
      return 'done';
    case 'failed':
      return 'error';
    case 'cancelled':
      return 'warning';
    default:
      return 'idle';
  }
}

/** 状态文案。 */
function statusLabel(status: GoalStatus): string {
  switch (status) {
    case 'proposed':
      return '待开始';
    case 'active':
      return '进行中';
    case 'waiting':
      return '等待中';
    case 'paused':
      return '已暂停';
    case 'completed':
      return '已完成';
    case 'failed':
      return '已失败';
    case 'cancelled':
      return '已取消';
  }
}

/** 相对时间：只粗分到分钟与小时，这一行不是精确读数。 */
function relativeTime(ms: number, now: number): string {
  const delta = Math.max(0, now - ms);
  if (delta < 60_000) return '刚刚';
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)} 分钟前`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)} 小时前`;
  return `${Math.floor(delta / 86_400_000)} 天前`;
}

/** 目标一行：状态点 + 标题 + 状态与下一步。 */
const GoalLine: React.FC<{ goal: GoalCardView; now: number; onOpen: () => void }> = ({
  goal,
  now,
  onOpen,
}) => (
  <button
    type="button"
    onClick={onOpen}
    className="w-full text-left px-2 py-1.5 rounded-xs transition-colors duration-100 hover:bg-hover focus-visible:outline focus-visible:outline-2 focus-visible:outline-focus"
  >
    <span className="flex items-center gap-2">
      <StateDot state={statusDot(goal.status)} />
      <span className="min-w-0 flex-1 truncate text-row text-text-primary">{goal.title}</span>
      <span className="shrink-0 text-meta text-text-tertiary">
        {relativeTime(goal.lastActivityAtMs, now)}
      </span>
    </span>
    <span className="mt-0.5 block pl-[calc(8px+0.5rem)] text-meta text-text-tertiary truncate">
      {statusLabel(goal.status)}
      {goal.nextStep === null || goal.nextStep === '' ? '' : ` · ${goal.nextStep}`}
    </span>
  </button>
);

export const GoalActions: React.FC<{
  /** 本会话创建的目标；为空时整枚收起。 */
  goals: GoalCardView[];
  /** 点一行进入它的执行会话。 */
  onOpenGoal: (goal: GoalCardView) => void;
}> = ({ goals, onOpenGoal }) => {
  const now = Date.now();
  if (goals.length === 0) return null;
  const active = goals.filter((goal) => goal.status === 'active' || goal.status === 'waiting');
  return (
    <Dropdown
      align="end"
      panelRole="menu"
      menuClassName="w-[320px] max-h-[420px] overflow-y-auto"
      trigger={({ open, toggle }) => (
        <button
          type="button"
          onClick={toggle}
          aria-haspopup="menu"
          aria-expanded={open}
          title={active.length > 0 ? `目标：${active.length} 个进行中` : '目标'}
          className={`h-7 px-2 inline-flex items-center gap-1.5 rounded-xs text-meta transition-colors duration-100 ${
            active.length > 0
              ? 'bg-ok-soft text-ok-ink font-medium hover:bg-active'
              : 'text-text-tertiary hover:bg-hover hover:text-text-primary'
          }`}
        >
          <ChecklistIcon size={14} className="shrink-0" />
          <span>目标</span>
          <span>{active.length > 0 ? `${active.length}/${goals.length}` : goals.length}</span>
          <ChevronDownIcon
            size={12}
            className={`shrink-0 transition-transform duration-150 ease-out ${open ? 'rotate-180' : ''}`}
          />
        </button>
      )}
    >
      <SectionHeader first label={`目标 ${goals.length}`} />
      {goals.map((goal) => (
        <GoalLine
          key={goal.goalId}
          goal={goal}
          now={now}
          onOpen={() => onOpenGoal(goal)}
        />
      ))}
    </Dropdown>
  );
};
