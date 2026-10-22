/*
 * 会话级生成指标。
 *
 * 与 TUI 恒等：两条口径都按整个会话累计，而不是只看最近一轮。
 * - 首字：各轮 TTFT 的算术平均；在跑轮的首字帧一到就参与，不必等收口。
 * - 速率：Σ(输出 + 推理 token) / Σ(解码窗口)，含思考输出，按窗口加权。
 *   分子取 Provider 实测的本轮输出（观测帧与收尾负载带下来），没有该读数时才退回
 *   自己累计的用量：用量帧是每条流的累计快照且会重复投递，直接累加会把分子翻倍。
 * 没有任何样本时不返回 0：缺观测就是不显示，不填占位值。
 */

import type { TurnState } from './transcript/state';

/** 会话级生成指标。 */
export interface SessionGenerationMetrics {
  /** 各轮首字耗时算术平均；无样本时为 null。 */
  firstTokenAvgMs: number | null;
  /** 会话级速率，单位 0.1 tok/s；无样本时为 null。 */
  generationTpsX10: number | null;
  /** 参与平均的首字样本数。 */
  ttftSamples: number;
  /** 参与加权的轮数（有解码窗口的轮）。 */
  decodeSamples: number;
}

const EMPTY_METRICS: SessionGenerationMetrics = {
  firstTokenAvgMs: null,
  generationTpsX10: null,
  ttftSamples: 0,
  decodeSamples: 0,
};

/** 一轮对会话级口径贡献的 token：输出加推理，优先用 Provider 实测值。 */
function turnOutputTokens(turn: TurnState): number {
  const measured =
    (turn.generationOutputTokens ?? 0) + (turn.generationReasoningTokens ?? 0);
  if (measured > 0) return measured;
  return turn.usage.outputTokens + turn.usage.reasoningOutputTokens;
}

/** 汇总整个会话的生成指标。 */
export function sessionGenerationMetrics(turns: TurnState[]): SessionGenerationMetrics {
  let ttftTotal = 0;
  let ttftSamples = 0;
  let tokenTotal = 0;
  let decodeTotal = 0;
  let decodeSamples = 0;
  for (const turn of turns) {
    if (turn.firstTokenAvgMs !== null) {
      ttftTotal += turn.firstTokenAvgMs;
      ttftSamples += 1;
    }
    // 解码窗口为 0 的轮（没测到、或尚未收口的在跑轮）不参与速率，也不拉高分母。
    if (turn.decodeMs !== null && turn.decodeMs > 0) {
      decodeTotal += turn.decodeMs;
      tokenTotal += turnOutputTokens(turn);
      decodeSamples += 1;
    }
  }
  if (ttftSamples === 0 && decodeSamples === 0) return EMPTY_METRICS;
  return {
    firstTokenAvgMs: ttftSamples === 0 ? null : Math.round(ttftTotal / ttftSamples),
    generationTpsX10:
      decodeTotal > 0 && tokenTotal > 0
        ? Math.floor((tokenTotal * 10_000 + Math.floor(decodeTotal / 2)) / decodeTotal)
        : null,
    ttftSamples,
    decodeSamples,
  };
}
