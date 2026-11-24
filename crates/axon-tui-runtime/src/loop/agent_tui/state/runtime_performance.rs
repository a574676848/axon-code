//! 跨进程非持久运行时指标观测帧的投影与收敛 (M0: Daemon 权威采样 + 客户端只读消费)。

use crate::r#loop::agent_tui::AgentTuiState;
use axon_domain::RuntimeObservationFrame;

impl AgentTuiState {
    /// 消费一条来自 Daemon 的非持久运行时指标观测帧。
    pub(in crate::r#loop::agent_tui) fn apply_observation(
        &mut self,
        frame: &RuntimeObservationFrame,
    ) {
        // 1. LSP 权威快照：Daemon 进程池状态更新
        if let Some(lsp) = frame.lsp {
            self.header.lsp_active_count = lsp.active_clients;
        }

        // 2. Gateway 运行指标：Daemon 权威活跃主回合数更新 (Primary 口径)
        if let Some(gateway) = frame.gateway {
            self.header.gateway_active_turn_count = gateway.active_turn_count;
        }

        // 3. 生成性能快照：落地 SSOT §3.3 权威收敛规则
        if let Some(generation) = frame.generation {
            // 会话级台账按轮记账：同一轮的重复帧幂等覆盖，不重复计入均值。
            self.bridge
                .session_generation
                .observe(&frame.turn_id, generation);
            let perf_state = &mut self.bridge.generation_performance;
            // 终态防线与迟到帧丢弃：
            // - 若当前回合已终态（Terminal），丢弃瞬态 Observation 帧，防止 Wire 事实被脏写；
            // - 若帧生成时间早于最近已消费帧（乱序迟到），同样丢弃。
            if !perf_state.turn_terminal
                && frame.generated_at_ms >= perf_state.last_observation_at_ms
            {
                perf_state.last_observation_at_ms = frame.generated_at_ms;
                if let Some(ttft) = generation.ttft_ms {
                    let mut snapshot = perf_state.last.unwrap_or_default();
                    snapshot.ttft_total_ms = ttft;
                    snapshot.ttft_samples = 1;
                    // 该轮速率与头部同口径：分子含思考输出。帧里的 token 是本轮实测累计值，
                    // 每收口一条 provider 流刷新一次，因此这条读数会随本轮推进。
                    snapshot.decode_total_ms = generation.decode_ms.unwrap_or(0);
                    snapshot.generation_tokens = generation
                        .output_tokens
                        .saturating_add(generation.reasoning_output_tokens);
                    perf_state.last = Some(snapshot);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::agent_tui::state_tests;
    use axon_domain::{GenerationMetrics, LspMetricsSnapshot};

    fn state() -> AgentTuiState {
        AgentTuiState::new_for_test(&state_tests::launch_request(None))
    }

    #[test]
    fn observation_updates_lsp_count_and_ttft() {
        let mut state = state();
        assert_eq!(state.lsp_active_count(), 0);

        let frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1")
            .with_lsp(LspMetricsSnapshot {
                active_clients: 2,
                pending_requests: 0,
                diagnostics_count: 0,
                last_latency_ms: 0,
            })
            .with_generation(GenerationMetrics {
                ttft_ms: Some(1_500),
                decode_ms: Some(800),
                generation_speed_tps: Some(50.0),
                output_tokens: 40,
                ..Default::default()
            });

        state.apply_observation(&frame);

        assert_eq!(state.lsp_active_count(), 2);
        let perf = state
            .bridge
            .generation_performance
            .last
            .expect("应记录生成性能");
        assert_eq!(perf.first_token_avg_ms(), Some(1_500));
        assert_eq!(perf.generation_tokens, 40);
        assert_eq!(perf.decode_total_ms, 800);
    }

    #[test]
    fn per_turn_rate_counts_reasoning_output_and_advances_on_live_frames() {
        let mut state = state();

        // 首字帧：还没有解码窗口，速率为空。
        state.apply_observation(
            &RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_generation(
                GenerationMetrics {
                    ttft_ms: Some(9_690),
                    ..Default::default()
                },
            ),
        );
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .generation_tps_x10(),
            None
        );

        // 收口一条流后的实时帧：69 输出 + 1_525 推理 = 1_594 token / 25_307ms = 63.0 tok/s。
        state.apply_observation(
            &RuntimeObservationFrame::new(2_000, "session-1", "turn-1").with_generation(
                GenerationMetrics {
                    ttft_ms: Some(9_690),
                    decode_ms: Some(25_307),
                    output_tokens: 69,
                    reasoning_output_tokens: 1_525,
                    ..Default::default()
                },
            ),
        );

        let snapshot = state.bridge.generation_performance.last.unwrap();
        assert_eq!(snapshot.generation_tokens, 1_594);
        assert_eq!(snapshot.generation_tps_x10(), Some(630));
    }

    #[test]
    fn terminal_turn_rejects_late_observation_frame() {
        let mut state = state();
        state.bridge.generation_performance.turn_terminal = true;

        let frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_generation(
            GenerationMetrics {
                ttft_ms: Some(2_000),
                ..Default::default()
            },
        );

        state.apply_observation(&frame);

        assert_eq!(
            state.bridge.generation_performance.last, None,
            "终态回合应丢弃瞬态观测帧"
        );
    }

    #[test]
    fn out_of_order_observation_frame_is_discarded() {
        let mut state = state();

        let frame1 = RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_generation(
            GenerationMetrics {
                ttft_ms: Some(1_200),
                ..Default::default()
            },
        );
        state.apply_observation(&frame1);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_200)
        );

        // 迟到帧：generated_at_ms = 800 < 1000
        let frame2 = RuntimeObservationFrame::new(800, "session-1", "turn-1").with_generation(
            GenerationMetrics {
                ttft_ms: Some(3_000),
                ..Default::default()
            },
        );
        state.apply_observation(&frame2);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_200),
            "迟到帧不得脏写已更新的指标"
        );
    }

    #[test]
    fn lsp_updates_even_when_turn_is_terminal() {
        let mut state = state();
        state.bridge.generation_performance.turn_terminal = true;

        let frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_lsp(
            LspMetricsSnapshot {
                active_clients: 1,
                pending_requests: 0,
                diagnostics_count: 0,
                last_latency_ms: 0,
            },
        );

        state.apply_observation(&frame);
        assert_eq!(
            state.lsp_active_count(),
            1,
            "进程级 LSP 状态不受回合终态限制"
        );
    }

    #[test]
    fn finished_turn_locks_against_late_observation_frames_end_to_end() {
        let mut state = state();
        state.start_runtime_submit(
            &crate::r#loop::agent_tui::state::runtime_flow_test_support::test_submission(
                "观测帧终态防线",
            ),
        );

        let live_frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1")
            .with_generation(GenerationMetrics {
                ttft_ms: Some(1_500),
                ..Default::default()
            });
        state.apply_observation(&live_frame);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_500)
        );

        // Wire 终态收口（finish_submit）后，迟到的瞬态帧必须被 Terminal 防线拒绝。
        state.finish_submit(Ok(
            crate::r#loop::agent_tui::run_output::AgentTuiRunOutput::empty(),
        ));
        assert!(state.bridge.generation_performance.turn_terminal);

        let late_frame = RuntimeObservationFrame::new(2_000, "session-1", "turn-1")
            .with_generation(GenerationMetrics {
                ttft_ms: Some(9_999),
                ..Default::default()
            });
        state.apply_observation(&late_frame);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_500),
            "终态后迟到帧不得脏写已锁定的指标"
        );
    }

    #[test]
    fn data_source_defense_verifies_lsp_from_daemon_observation_not_in_process_static() {
        let mut state = state();
        // 初始为 0
        assert_eq!(state.header.lsp_active_count, 0);

        // 经由 Observation 帧注入数值，验证只读缓存被正确设置
        let frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_lsp(
            LspMetricsSnapshot {
                active_clients: 3,
                pending_requests: 0,
                diagnostics_count: 0,
                last_latency_ms: 0,
            },
        );
        state.apply_observation(&frame);
        assert_eq!(state.header.lsp_active_count, 3);
    }

    #[test]
    fn gateway_active_turn_count_reaches_tui_state_from_observation() {
        let mut state = state();
        assert_eq!(state.header.gateway_active_turn_count, 0);

        let frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1").with_gateway(
            axon_domain::GatewayMetrics {
                active_turn_count: 5,
            },
        );
        state.apply_observation(&frame);
        assert_eq!(state.header.gateway_active_turn_count, 5);
    }

    #[test]
    fn user_interrupt_sets_turn_terminal_and_blocks_late_observation_frames() {
        let mut state = state();
        state.start_runtime_submit(
            &crate::r#loop::agent_tui::state::runtime_flow_test_support::test_submission(
                "中断终态防线",
            ),
        );

        let live_frame = RuntimeObservationFrame::new(1_000, "session-1", "turn-1")
            .with_generation(GenerationMetrics {
                ttft_ms: Some(1_200),
                ..Default::default()
            });
        state.apply_observation(&live_frame);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_200)
        );

        // 用户主动中断当前任务
        state.request_interrupt_current_task();
        assert!(
            state.bridge.generation_performance.turn_terminal,
            "用户主动中断必须置位 turn_terminal 防线"
        );

        // 迟到帧到来，必须被拒绝
        let late_frame = RuntimeObservationFrame::new(2_000, "session-1", "turn-1")
            .with_generation(GenerationMetrics {
                ttft_ms: Some(9_999),
                ..Default::default()
            });
        state.apply_observation(&late_frame);
        assert_eq!(
            state
                .bridge
                .generation_performance
                .last
                .unwrap()
                .first_token_avg_ms(),
            Some(1_200),
            "用户中断后迟到帧不得脏写已锁定的指标"
        );
    }
}
