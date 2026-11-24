//! 会话级生成指标台账。
//!
//! 首字与速率按**整个会话**重算，而不是只看最近一轮：
//! - 首字：各轮 TTFT 的算术平均。在跑轮的首字帧一到即入账，因此不必等它收口。
//! - 速率：Σ(输出 + 推理 token) / Σ(解码窗口)，含思考输出，按窗口加权。
//!
//! 台账按轮身份（turn id）记账而不是累加计数：同一轮的观测帧会重复到达
//! （首字帧与收尾帧都带 ttft），按轮覆盖才不会把同一轮算两次。
//! 没有任何样本时返回 None，绝不返回 0 冒充「测得 0」。

use std::collections::BTreeMap;

use axon_domain::GenerationMetrics;

/// 单轮样本。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct SessionGenerationTurn {
    /// 该轮首字耗时；未测到为 None。
    first_token_ms: Option<u64>,
    /// 该轮输出与推理 token 之和。
    output_tokens: u64,
    /// 该轮解码窗口；尚未收口为 0。
    decode_ms: u64,
}

/// 会话级生成指标。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct SessionGenerationMetrics {
    /// 各轮首字耗时算术平均。
    pub(in crate::r#loop::agent_tui) first_token_avg_ms: Option<u64>,
    /// 会话级速率，单位 0.1 tok/s。
    pub(in crate::r#loop::agent_tui) generation_tps_x10: Option<u64>,
}

/// 按轮记账的会话级生成指标台账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct SessionGenerationLedger {
    turns: BTreeMap<String, SessionGenerationTurn>,
}

impl SessionGenerationLedger {
    /// 消费一帧观测：只写该帧真正带来的事实，缺项保持原值。
    pub(in crate::r#loop::agent_tui) fn observe(
        &mut self,
        turn_id: &str,
        metrics: GenerationMetrics,
    ) {
        let turn_id = turn_id.trim();
        if turn_id.is_empty() {
            return;
        }
        let entry = self.turns.entry(turn_id.to_owned()).or_default();
        if let Some(first_token_ms) = metrics.ttft_ms {
            entry.first_token_ms = Some(first_token_ms);
        }
        let output_tokens = metrics
            .output_tokens
            .saturating_add(metrics.reasoning_output_tokens);
        let decode_ms = metrics.decode_ms.unwrap_or_default();
        if output_tokens > 0 {
            entry.output_tokens = output_tokens;
        }
        if decode_ms > 0 {
            entry.decode_ms = decode_ms;
        }
    }

    /// 补记一轮已完成轮的样本（恢复回放用）。
    pub(in crate::r#loop::agent_tui) fn seed_turn(
        &mut self,
        turn_id: &str,
        first_token_ms: Option<u64>,
        output_tokens: u64,
        decode_ms: u64,
    ) {
        let turn_id = turn_id.trim();
        if turn_id.is_empty() {
            return;
        }
        let entry = self.turns.entry(turn_id.to_owned()).or_default();
        if first_token_ms.is_some() {
            entry.first_token_ms = first_token_ms;
        }
        if output_tokens > 0 {
            entry.output_tokens = output_tokens;
        }
        if decode_ms > 0 {
            entry.decode_ms = decode_ms;
        }
    }

    /// 汇总会话级指标。
    pub(in crate::r#loop::agent_tui) fn metrics(&self) -> SessionGenerationMetrics {
        let mut first_token_total = 0_u64;
        let mut first_token_samples = 0_u64;
        let mut output_tokens = 0_u64;
        let mut decode_total_ms = 0_u64;
        for turn in self.turns.values() {
            if let Some(first_token_ms) = turn.first_token_ms {
                first_token_total = first_token_total.saturating_add(first_token_ms);
                first_token_samples = first_token_samples.saturating_add(1);
            }
            if turn.decode_ms > 0 {
                output_tokens = output_tokens.saturating_add(turn.output_tokens);
                decode_total_ms = decode_total_ms.saturating_add(turn.decode_ms);
            }
        }
        SessionGenerationMetrics {
            first_token_avg_ms: (first_token_samples > 0)
                .then(|| first_token_total / first_token_samples),
            generation_tps_x10: generation_tps_x10(output_tokens, decode_total_ms),
        }
    }
}

/// 与单轮同款：token 数乘 10000 后按毫秒均分，四舍五入。
fn generation_tps_x10(output_tokens: u64, decode_total_ms: u64) -> Option<u64> {
    if decode_total_ms == 0 || output_tokens == 0 {
        return None;
    }
    let tokens = u128::from(output_tokens);
    let millis = u128::from(decode_total_ms);
    u64::try_from(tokens.saturating_mul(10_000).saturating_add(millis / 2) / millis).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(
        ttft_ms: Option<u64>,
        decode_ms: Option<u64>,
        output: u64,
        reasoning: u64,
    ) -> GenerationMetrics {
        GenerationMetrics {
            ttft_ms,
            decode_ms,
            output_tokens: output,
            reasoning_output_tokens: reasoning,
            ..GenerationMetrics::default()
        }
    }

    #[test]
    fn session_metrics_average_ttft_and_weight_rate_by_decode_window() {
        let mut ledger = SessionGenerationLedger::default();
        ledger.observe("turn-1", metrics(Some(2_000), Some(8_000), 100, 20));
        ledger.observe("turn-2", metrics(Some(6_000), Some(2_000), 300, 80));

        let session = ledger.metrics();

        assert_eq!(session.first_token_avg_ms, Some(4_000));
        // (100 + 20) + (300 + 80) = 500 token；窗口 8s + 2s = 10s
        assert_eq!(session.generation_tps_x10, Some(500));
    }

    #[test]
    fn repeated_frames_for_one_turn_do_not_double_count() {
        let mut ledger = SessionGenerationLedger::default();
        // 首字帧与收尾帧都带 ttft：同一轮只算一个样本。
        ledger.observe("turn-1", metrics(Some(3_000), None, 0, 0));
        ledger.observe("turn-1", metrics(Some(3_000), Some(5_000), 250, 0));

        let session = ledger.metrics();

        assert_eq!(session.first_token_avg_ms, Some(3_000));
        assert_eq!(session.generation_tps_x10, Some(500));
    }

    #[test]
    fn running_turn_contributes_ttft_before_it_settles() {
        let mut ledger = SessionGenerationLedger::default();
        ledger.observe("turn-1", metrics(Some(4_000), Some(10_000), 200, 0));
        ledger.observe("turn-2", metrics(Some(2_000), None, 0, 0));

        let session = ledger.metrics();

        assert_eq!(
            session.first_token_avg_ms,
            Some(3_000),
            "在跑轮的首字即时参与"
        );
        assert_eq!(
            session.generation_tps_x10,
            Some(200),
            "在跑轮还没有解码窗口"
        );
    }

    #[test]
    fn no_samples_reports_none_instead_of_zero() {
        let ledger = SessionGenerationLedger::default();
        assert_eq!(ledger.metrics(), SessionGenerationMetrics::default());
        assert_eq!(ledger.metrics().first_token_avg_ms, None);
        assert_eq!(ledger.metrics().generation_tps_x10, None);
    }

    #[test]
    fn replayed_turn_seeds_the_same_ledger() {
        let mut ledger = SessionGenerationLedger::default();
        ledger.seed_turn("turn-1", Some(1_250), 400, 4_000);

        let session = ledger.metrics();

        assert_eq!(session.first_token_avg_ms, Some(1_250));
        assert_eq!(session.generation_tps_x10, Some(1_000));
    }
}
