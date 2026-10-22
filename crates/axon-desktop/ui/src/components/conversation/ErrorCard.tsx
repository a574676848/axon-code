import React from 'react';
import { ChevronRightIcon } from '../../icons';

/*
 * 错误卡：把一次失败说成「标题 + 结论 + 归因 + 原始回执 + 一个动作」。
 *
 * 会话事件流断裂、轮次执行失败、后端拒收走的是同一张卡：用户先看到一句人话，
 * 需要排查的人再展开逐字回执。原先这两处一处打印原始 JSON、一处只给一条红字，
 * 同一种事实两副面孔，因此收敛到这里。
 */

/** 卡片上的动作按钮（例如「重连」）。 */
export interface ErrorCardAction {
  label: string;
  onClick: () => void;
}

export const ErrorCard: React.FC<{
  title: string;
  /** 一行结论；为空时不渲染这一行。 */
  reason: string;
  /** 归因片段：Provider、HTTP 状态、错误码等；为空时不渲染这一行。 */
  meta?: string[];
  /** 处置建议；为空时不渲染这一行。 */
  hint?: string;
  /** 是否可重试：可重试时标题右侧给一枚标记。 */
  retryable?: boolean;
  /** 原始回执；有它才给展开入口。 */
  raw?: string;
  /** 卡片右下角的动作。 */
  action?: ErrorCardAction;
}> = ({ title, reason, meta = [], hint = '', retryable = false, raw = '', action }) => {
  const [open, setOpen] = React.useState(false);
  const metaParts = meta.filter((part) => part.length > 0);
  return (
    <section
      role="alert"
      aria-label={title}
      className="w-full my-1 px-3 py-2.5 rounded-md hairline border-border-muted bg-surface"
    >
      <div className="flex items-start gap-2">
        <span aria-hidden="true" className="shrink-0 mt-[6px] w-1 h-1 rounded-full bg-danger" />
        <div className="flex-1 min-w-0">
          <div className="flex items-baseline gap-2">
            <span className="text-row font-medium text-text-primary">{title}</span>
            {retryable && (
              <span className="shrink-0 px-1.5 py-[1px] rounded-xs text-code text-gold-ink bg-gold-soft">
                可重试
              </span>
            )}
          </div>
          {reason.length > 0 && (
            <p className="mt-1 mb-0 text-row-loose text-text-secondary break-words">{reason}</p>
          )}
          {hint.length > 0 && (
            <p className="mt-1 mb-0 text-row-loose text-text-tertiary">{hint}</p>
          )}
          {metaParts.length > 0 && (
            <div className="mt-1 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-code text-text-tertiary">
              {metaParts.map((part) => (
                <span key={part}>{part}</span>
              ))}
            </div>
          )}
          <div className="mt-1.5 flex items-center gap-3">
            {raw.length > 0 && (
              <button
                type="button"
                aria-expanded={open}
                onClick={() => setOpen((value) => !value)}
                className="inline-flex items-center gap-1 text-row text-text-tertiary transition-colors duration-100 hover:text-text-primary"
              >
                <ChevronRightIcon
                  size={12}
                  className={`transition-transform duration-150 ease-out ${open ? 'rotate-90' : ''}`}
                />
                {open ? '收起原始回执' : '查看原始回执'}
              </button>
            )}
            {action !== undefined && (
              <button
                type="button"
                onClick={action.onClick}
                className="h-6 px-2 rounded-xs hairline border-border-strong text-meta text-text-secondary transition-colors duration-100 hover:text-text-primary"
              >
                {action.label}
              </button>
            )}
          </div>
          {open && raw.length > 0 && (
            <pre className="mt-1 mb-0 px-2 py-1.5 rounded-sm bg-surface-sunken hairline border-border-hairline max-h-[240px] overflow-auto font-mono text-code text-text-secondary whitespace-pre-wrap break-words">
              {raw}
            </pre>
          )}
        </div>
      </div>
    </section>
  );
};
