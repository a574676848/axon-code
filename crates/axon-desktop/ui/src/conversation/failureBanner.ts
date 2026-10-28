/*
 * 轮次失败横幅：主会话、子代理与只读会话的结论不是同一句话。
 *
 * 主会话的失败可以由用户「重新发送这条消息」解决；子代理车道不是用户消息驱动的车道，
 * 它没跑完通常是父轮失败的连坐结果（父轮一停，运行时连同它的子代理一起收口）。
 * 只读会话（目标执行会话）没有输入坞，因此既不能「重新发送」也不能「回到上层会话」，
 * 它只说清后台已停止推进。
 */

/** 横幅所属的车道类型。 */
export type FailureBannerLane = 'primary' | 'subagent' | 'readonly';

/** 决定横幅文案的输入。 */
export interface FailureBannerInput {
  lane: FailureBannerLane;
  /** 这一轮的终态：只有这两种终态才需要给结论提示。 */
  status: 'failed' | 'rejected';
  /** 子代理车道：该子代理是否被中断（父轮连坐或用户中止）；其余车道忽略。 */
  interrupted?: boolean;
}

/** 横幅文案。 */
export interface FailureBanner {
  text: string;
}

const PRIMARY_REJECTED = '本轮被后端拒绝，请检查模型与凭据配置后重试。';
const PRIMARY_FAILED = '本轮执行失败，可重新发送这条消息。';
const SUBAGENT_INTERRUPTED = '父轮失败导致本子代理被中断，请在上层会话重发。';
const SUBAGENT_FAILED = '本子代理执行失败，请在上层会话查看原因后重发。';
const SUBAGENT_REJECTED = '本子代理请求被后端拒绝，请在上层会话检查模型与凭据配置。';
const READONLY_REJECTED = '本轮被后端拒绝，后台已停止推进，此视图只读。';
const READONLY_FAILED = '本轮执行失败，后台已停止推进，此视图只读。';

/** 按车道与终态给出结论文案。 */
export function failureBanner(input: FailureBannerInput): FailureBanner {
  if (input.lane === 'readonly') {
    return {
      text: input.status === 'rejected' ? READONLY_REJECTED : READONLY_FAILED,
    };
  }
  if (input.lane === 'subagent') {
    if (input.status === 'rejected') {
      return { text: SUBAGENT_REJECTED };
    }
    return { text: input.interrupted === true ? SUBAGENT_INTERRUPTED : SUBAGENT_FAILED };
  }
  return {
    text: input.status === 'rejected' ? PRIMARY_REJECTED : PRIMARY_FAILED,
  };
}
