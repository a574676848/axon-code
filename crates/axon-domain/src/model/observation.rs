use serde::{Deserialize, Serialize};

/// Provider Prompt 缓存命中率 (bps)。
pub const METRIC_LLM_PROVIDER_PROMPT_CACHE_HIT_RATE: &str = "llm.provider.prompt_cache_hit_rate";
/// 终端 Transcript 渲染缓存命中率 (bps)。
pub const METRIC_TUI_TRANSCRIPT_RENDER_CACHE_HIT_RATE: &str =
    "tui.transcript.render_cache_hit_rate";
/// 应用层首字延迟 (毫秒)。
pub const METRIC_LLM_PROVIDER_TTFT_MS: &str = "llm.provider.ttft_ms";
/// 纯解码阶段生成速率 (tokens/s)。
pub const METRIC_LLM_PROVIDER_GENERATION_SPEED: &str = "llm.provider.generation_speed";
/// 网关活跃并发回合数 (count)。
pub const METRIC_GATEWAY_ACTIVE_TURN_COUNT: &str = "gateway.active_turn_count";
/// 上游实际模型标识。
pub const METRIC_LLM_PROVIDER_UPSTREAM_MODEL: &str = "llm.provider.upstream_model";
/// 模型推理强度配置。
pub const METRIC_LLM_PROVIDER_REASONING_EFFORT: &str = "llm.provider.reasoning_effort";
/// 请求路径或渠道。
pub const METRIC_LLM_PROVIDER_REQUEST_PATH: &str = "llm.provider.request_path";
/// 速率或处理模式。
pub const METRIC_LLM_PROVIDER_SPEED_MODE: &str = "llm.provider.speed_mode";

/// 计算统一网关活跃并发回合数 (SSOT §2.1: 正在进行中的 Primary Turn 与 Remote Feishu 会话总和)。
pub fn compute_gateway_active_turn_count(
    primary_active_turns: usize,
    feishu_active_chat_turns: usize,
) -> usize {
    primary_active_turns.saturating_add(feishu_active_chat_turns)
}

/// 统一应用层生成性能快照 (纯数值 POD 结构，零堆分配，Option<T> 仍满足 Copy)。
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationMetrics {
    /// 应用层首字延迟 (未测得或缺失时为 None)。
    pub ttft_ms: Option<u64>,
    /// 首字到达至生成完毕的解码耗时 (未测得首字时为 None)。
    pub decode_ms: Option<u64>,
    /// 纯解码阶段速率 (tokens/s，未测得时为 None)。
    pub generation_speed_tps: Option<f64>,
    /// 输入 Token 总量。
    pub total_input_tokens: u64,
    /// 缓存读取输入 Token 数量。
    pub cache_read_input_tokens: u64,
    /// 缓存写入创建输入 Token 数量。
    pub cache_write_input_tokens: u64,
    /// 输出 Token 数量。
    pub output_tokens: u64,
    /// 思考/推理输出 Token 数量。
    pub reasoning_output_tokens: u64,
    /// 缓存读取占完整输入的万分比 (bps, 10000 = 100%)。
    pub cache_hit_rate_basis_points: u16,
    /// 实时核算费用 (微美元)。
    pub cost_micros: Option<u64>,
}

impl GenerationMetrics {
    /// 根据缓存读取 Token 与总输入 Token 计算缓存命中率万分比 (bps)。
    pub fn calculate_cache_hit_rate_bps(cache_read_tokens: u64, total_input_tokens: u64) -> u16 {
        if total_input_tokens == 0 {
            return 0;
        }
        let hit_rate = (cache_read_tokens as u128 * 10_000) / total_input_tokens as u128;
        u16::try_from(hit_rate.min(10_000)).unwrap_or(10_000)
    }

    /// 根据输出 Token 与纯解码耗时 (ms) 计算解码速率 (tokens/s)。
    pub fn calculate_generation_speed_tps(output_tokens: u64, decode_ms: u64) -> Option<f64> {
        if decode_ms == 0 {
            None
        } else {
            Some((output_tokens as f64 * 1000.0) / decode_ms as f64)
        }
    }
}

/// Turn 各阶段耗时剖析 (POD 结构)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTimingMetrics {
    /// 模型思考与上下文装配耗时 (毫秒)。
    pub planning_ms: u64,
    /// 模型网络往返与流式生成耗时 (毫秒)。
    pub provider_ms: u64,
    /// 工具调用执行总耗时 (毫秒)。
    pub tool_execution_ms: u64,
    /// Turn 全生命周期总耗时 (毫秒)。
    pub total_turn_ms: u64,
}

/// LSP 运行状态快照 (由 Daemon 权威发布，POD 结构)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LspMetricsSnapshot {
    /// 活跃 LSP 客户端数。
    pub active_clients: usize,
    /// 队列中待处理请求数。
    pub pending_requests: usize,
    /// 当前活动诊断报错数。
    pub diagnostics_count: usize,
    /// 最近一次 LSP 请求往返耗时 (毫秒)。
    pub last_latency_ms: u64,
}

/// 网关运行指标快照 (由 Daemon 权威发布，POD 结构)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayMetrics {
    /// 当前进行中的 Primary Turn 数量。
    pub active_turn_count: usize,
}

/// 跨进程非持久观测帧 (跨进程分发，包含轻量路由句柄，核心 payload 为 POD)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeObservationFrame {
    /// 帧生成时间戳 (毫秒)。
    pub generated_at_ms: u64,
    /// 所属会话 ID。
    pub session_id: String,
    /// 所属回合/交互 ID。
    pub turn_id: String,
    /// 生成性能快照。
    pub generation: Option<GenerationMetrics>,
    /// 回合分阶段耗时。
    pub timing: Option<TurnTimingMetrics>,
    /// LSP 状态快照。
    pub lsp: Option<LspMetricsSnapshot>,
    /// 网关运行指标快照 (M1: Primary 口径)。
    pub gateway: Option<GatewayMetrics>,
}

impl RuntimeObservationFrame {
    /// 构造新的非持久观测帧。
    pub fn new(
        generated_at_ms: u64,
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Self {
        Self {
            generated_at_ms,
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            generation: None,
            timing: None,
            lsp: None,
            gateway: None,
        }
    }

    /// 设置生成指标。
    pub fn with_generation(mut self, generation: GenerationMetrics) -> Self {
        self.generation = Some(generation);
        self
    }

    /// 设置耗时指标。
    pub fn with_timing(mut self, timing: TurnTimingMetrics) -> Self {
        self.timing = Some(timing);
        self
    }

    /// 设置 LSP 指标。
    pub fn with_lsp(mut self, lsp: LspMetricsSnapshot) -> Self {
        self.lsp = Some(lsp);
        self
    }

    /// 设置网关指标。
    pub fn with_gateway(mut self, gateway: GatewayMetrics) -> Self {
        self.gateway = Some(gateway);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_metrics_is_copy_and_pod() {
        let metrics = GenerationMetrics {
            ttft_ms: Some(120),
            decode_ms: Some(800),
            generation_speed_tps: Some(50.0),
            total_input_tokens: 1000,
            cache_read_input_tokens: 600,
            cache_write_input_tokens: 0,
            output_tokens: 40,
            reasoning_output_tokens: 10,
            cache_hit_rate_basis_points: 6000,
            cost_micros: Some(500),
        };
        let copy_metrics = metrics; // Copy 验证
        assert_eq!(metrics.ttft_ms, copy_metrics.ttft_ms);
    }

    #[test]
    fn cache_hit_rate_and_speed_calculation() {
        let bps = GenerationMetrics::calculate_cache_hit_rate_bps(2000, 3300);
        assert_eq!(bps, 6060);

        let speed = GenerationMetrics::calculate_generation_speed_tps(50, 1000);
        assert_eq!(speed, Some(50.0));
        assert_eq!(
            GenerationMetrics::calculate_generation_speed_tps(50, 0),
            None
        );

        let active = compute_gateway_active_turn_count(2, 3);
        assert_eq!(active, 5);
    }

    #[test]
    fn observation_frame_serialization_roundtrip() {
        let frame = RuntimeObservationFrame::new(1700000000000, "session-1", "turn-1").with_lsp(
            LspMetricsSnapshot {
                active_clients: 2,
                pending_requests: 0,
                diagnostics_count: 5,
                last_latency_ms: 15,
            },
        );

        let json = serde_json::to_string(&frame).expect("序列化成功");
        assert!(json.contains("\"activeClients\":2"));
        assert!(json.contains("\"sessionId\":\"session-1\""));

        let deserialized: RuntimeObservationFrame =
            serde_json::from_str(&json).expect("反序列化成功");
        assert_eq!(frame, deserialized);
    }
}
