import React from 'react';
import { failureHeadline, type FailureReport } from '../../conversation/failureReport';
import { useConversationActions } from './actions';
import { ErrorCard } from './ErrorCard';

/*
 * 轮次失败卡：把 run_failed 回执说成「标题 + 结论 + 归因 + 原始回执」。
 *
 * 真机上这里原先直接打印一整段 run_failed JSON：用户既看不出哪一句话是结论，
 * 也无法判断该不该重试。现在结论与归因摆在明面上，原始回执收进展开区。
 * 展示由共用 ErrorCard 承担，这里只负责把失败事实映射成它的字段。
 *
 * **有动作才谈处置**：只有在会话动作上下文里才给「可重试」标记与换渠道建议。
 * 只读视图（目标执行会话）与子代理视图都没有重试入口，在那里写「可重试」
 * 等于指一个不存在的按钮。
 */
export const FailureCard: React.FC<{ report: FailureReport; fallbackText: string }> = ({
  report,
  fallbackText,
}) => {
  const actionable = useConversationActions() !== null;
  return (
    <ErrorCard
      title={failureHeadline(report)}
      reason={report.reason}
      retryable={actionable && report.retryable}
      hint={actionable ? channelHint(report) : ''}
      raw={report.raw.length > 0 ? report.raw : fallbackText}
      meta={[
        report.provider.length > 0 || report.model.length > 0
          ? `${report.provider}${report.provider.length > 0 && report.model.length > 0 ? ' / ' : ''}${report.model}`
          : '',
        report.httpStatus === null ? '' : `HTTP ${report.httpStatus}`,
        report.code,
        report.attemptCount === null || report.attemptCount <= 1
          ? ''
          : `已尝试 ${report.attemptCount} 次`,
        report.providerEventType,
        report.upstreamRequestId.length > 0 ? `请求 ID ${report.upstreamRequestId}` : '',
      ]}
    />
  );
};

/**
 * 渠道提示：传输类失败只是「这条渠道这次没成」，用户能做的动作是换渠道或换模型。
 *
 * 只对可重试的传输失败给出，避免把参数类、额度类失败的处置方式也说成换渠道。
 */
export function channelHint(report: FailureReport): string {
  if (report.category.toLowerCase() !== 'provider_transport' || !report.retryable) return '';
  return '该渠道本次请求未成功，可切换模型或渠道后重试。';
}
