//! Runtime event 进入 TUI reducer 前的桥接边界。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use crate::r#loop::agent_tui::{
    AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiSubmit,
    assistant_stream_registry::AssistantStreamRegistry,
    live_status::LiveRuntimeStatus,
    provider_activity::ProviderActivityState,
    run_output::DaemonSubscriptionPhase,
    streaming::{chunking::AdaptiveChunkingPolicy, controller::PlanStreamController},
};
use crate::surface::animation::transcript_motion_tokens;
use crate::surface::cell::RenderCellRecord;
use crate::surface::event_envelope::RuntimeEventId;
use crate::surface::telemetry::{TuiTelemetry, TuiTraceSpanKind};
use crate::surface::watchers::{
    RuntimeWatcherEvent, RuntimeWatcherRegistry, RuntimeWatcherSource, RuntimeWatcherStatus,
};
use axon_agent_runtime::{EventLedgerDecision, TurnExecutionLedger};

use super::{
    mcp_login_loader::AgentTuiMcpAuthRequest,
    session_generation::SessionGenerationLedger,
    state::{AgentRuntimeStatus, subagent_activity::SubagentActivityState},
};

/// Runtime bridge 处理后的 reducer 输入。
#[derive(Debug)]
pub(in crate::r#loop::agent_tui) struct RuntimeBridgeSnapshot {
    pub(in crate::r#loop::agent_tui) payload: AgentTuiRuntimePayload,
    pub(in crate::r#loop::agent_tui) event_id: RuntimeEventId,
}

/// Runtime 事件、流式控制器和运行态临时状态。
#[derive(Debug, Clone)]
pub(in crate::r#loop::agent_tui) struct RuntimeBridge {
    pub(in crate::r#loop::agent_tui) turn_ledger: TurnExecutionLedger<()>,
    pub(in crate::r#loop::agent_tui) runtime_status: AgentRuntimeStatus,
    pub(in crate::r#loop::agent_tui) terminal_title_revision: u64,
    pub(in crate::r#loop::agent_tui) runtime_tmp_dir: PathBuf,
    pub(in crate::r#loop::agent_tui) spinner_frame: usize,
    pub(in crate::r#loop::agent_tui) next_spinner_tick: Instant,
    pub(in crate::r#loop::agent_tui) live_status: LiveRuntimeStatus,
    pub(in crate::r#loop::agent_tui) provider_activity: ProviderActivityState,
    pub(in crate::r#loop::agent_tui) subscription_phase: Option<DaemonSubscriptionPhase>,
    pub(in crate::r#loop::agent_tui) generation_performance: GenerationPerformanceState,
    /// 会话级生成指标台账：header 的首字与速率由它按整个会话重算。
    pub(in crate::r#loop::agent_tui) session_generation: SessionGenerationLedger,
    pub(in crate::r#loop::agent_tui) turn_work_activity: bool,
    pub(in crate::r#loop::agent_tui) assistant_streams: AssistantStreamRegistry,
    pub(in crate::r#loop::agent_tui) active_reasoning_parts: BTreeMap<String, RuntimePartState>,
    pub(in crate::r#loop::agent_tui) active_reasoning_started_at: BTreeMap<String, Instant>,
    pub(in crate::r#loop::agent_tui) plan_stream_controller: Option<PlanStreamController>,
    pub(in crate::r#loop::agent_tui) stream_chunking_policy: AdaptiveChunkingPolicy,
    pub(in crate::r#loop::agent_tui) live_lifecycle_line: Option<RenderCellRecord>,
    pub(in crate::r#loop::agent_tui) live_plan_line: Option<RenderCellRecord>,
    pub(in crate::r#loop::agent_tui) live_review_line: Option<RenderCellRecord>,
    pub(in crate::r#loop::agent_tui) live_hook_line: Option<RenderCellRecord>,
    pub(in crate::r#loop::agent_tui) telemetry: TuiTelemetry,
    pub(in crate::r#loop::agent_tui) interrupt_requested: bool,
    pub(in crate::r#loop::agent_tui) should_exit: bool,
    pub(in crate::r#loop::agent_tui) runtime_submission_enabled: bool,
    pub(in crate::r#loop::agent_tui) pending_runtime_submit: Option<AgentTuiSubmit>,
    pub(in crate::r#loop::agent_tui) catalog_reload_requested: bool,
    pub(in crate::r#loop::agent_tui) catalog_reload_force_mcp_refresh: bool,
    pub(in crate::r#loop::agent_tui) mcp_auth_request: Option<AgentTuiMcpAuthRequest>,
    pub(in crate::r#loop::agent_tui) mcp_auth_notice_shown: bool,
    pub(in crate::r#loop::agent_tui) subagent_activity: SubagentActivityState,
    pub(in crate::r#loop::agent_tui) watchers: RuntimeWatcherRegistry,
    pub(in crate::r#loop::agent_tui) watcher_states: Vec<RuntimeWatcherState>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct GenerationPerformanceState {
    pub(in crate::r#loop::agent_tui) last: Option<GenerationPerformanceSnapshot>,
    pub(in crate::r#loop::agent_tui) last_observation_at_ms: u64,
    pub(in crate::r#loop::agent_tui) turn_terminal: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct GenerationPerformanceSnapshot {
    pub(in crate::r#loop::agent_tui) ttft_total_ms: u64,
    pub(in crate::r#loop::agent_tui) ttft_samples: u64,
    pub(in crate::r#loop::agent_tui) decode_total_ms: u64,
    pub(in crate::r#loop::agent_tui) generation_tokens: u64,
}

impl GenerationPerformanceSnapshot {
    pub(in crate::r#loop::agent_tui) fn first_token_avg_ms(self) -> Option<u64> {
        (self.ttft_samples > 0).then(|| self.ttft_total_ms / self.ttft_samples)
    }

    pub(in crate::r#loop::agent_tui) fn generation_tps_x10(self) -> Option<u64> {
        (self.decode_total_ms > 0).then(|| {
            let tokens = u128::from(self.generation_tokens);
            let millis = u128::from(self.decode_total_ms);
            u64::try_from(tokens.saturating_mul(10_000).saturating_add(millis / 2) / millis)
                .unwrap_or(u64::MAX)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct RuntimeWatcherState {
    pub(in crate::r#loop::agent_tui) source: RuntimeWatcherSource,
    pub(in crate::r#loop::agent_tui) status: RuntimeWatcherStatus,
    pub(in crate::r#loop::agent_tui) summary: String,
}

impl RuntimeBridge {
    pub(in crate::r#loop::agent_tui) fn new(
        runtime_tmp_dir: PathBuf,
        plan_stream_controller: Option<PlanStreamController>,
    ) -> Self {
        Self {
            turn_ledger: TurnExecutionLedger::default(),
            runtime_status: AgentRuntimeStatus::Ready,
            terminal_title_revision: 0,
            runtime_tmp_dir,
            spinner_frame: 0,
            next_spinner_tick: Instant::now() + transcript_motion_tokens().spinner,
            live_status: LiveRuntimeStatus::default(),
            provider_activity: ProviderActivityState::default(),
            subscription_phase: None,
            generation_performance: GenerationPerformanceState::default(),
            session_generation: SessionGenerationLedger::default(),
            turn_work_activity: false,
            assistant_streams: AssistantStreamRegistry::default(),
            active_reasoning_parts: BTreeMap::new(),
            active_reasoning_started_at: BTreeMap::new(),
            plan_stream_controller,
            stream_chunking_policy: AdaptiveChunkingPolicy::default(),
            live_lifecycle_line: None,
            live_plan_line: None,
            live_review_line: None,
            live_hook_line: None,
            telemetry: TuiTelemetry::new(),
            interrupt_requested: false,
            should_exit: false,
            runtime_submission_enabled: true,
            pending_runtime_submit: None,
            catalog_reload_requested: false,
            catalog_reload_force_mcp_refresh: false,
            mcp_auth_request: None,
            mcp_auth_notice_shown: false,
            subagent_activity: SubagentActivityState::default(),
            watchers: RuntimeWatcherRegistry::new(Instant::now()),
            watcher_states: Vec::new(),
        }
    }

    pub(in crate::r#loop::agent_tui) fn accept_event(
        &mut self,
        event: &AgentTuiRuntimeEvent,
    ) -> bool {
        accept_runtime_event(&mut self.turn_ledger, event)
    }

    pub(in crate::r#loop::agent_tui) fn apply_accepted(
        &mut self,
        event: AgentTuiRuntimeEvent,
    ) -> RuntimeBridgeSnapshot {
        self.telemetry.record_trace_span(
            TuiTraceSpanKind::RuntimeEvent,
            &["event_id", "payload_kind", "source"],
        );
        RuntimeBridgeSnapshot {
            event_id: event.event_id,
            payload: event.payload,
        }
    }

    pub(in crate::r#loop::agent_tui) fn reset_turn_ledger(&mut self) {
        self.turn_ledger = TurnExecutionLedger::default();
        self.subscription_phase = None;
    }

    pub(in crate::r#loop::agent_tui) fn record_watcher_event(
        &mut self,
        event: &RuntimeWatcherEvent,
    ) {
        let summary = watcher_summary(event);
        if let Some(state) = self
            .watcher_states
            .iter_mut()
            .find(|state| state.source == event.source)
        {
            state.status = event.status;
            state.summary = summary;
            return;
        }
        self.watcher_states.push(RuntimeWatcherState {
            source: event.source,
            status: event.status,
            summary,
        });
    }
}

fn accept_runtime_event(
    ledger: &mut TurnExecutionLedger<()>,
    event: &AgentTuiRuntimeEvent,
) -> bool {
    let decision = match &event.payload {
        AgentTuiRuntimePayload::WireEvent { event, .. } => ledger.accept_event_identity(
            event.event_id.0.clone(),
            wire_stream_key(&event.session.session_id, event.session.run_id.as_deref()),
        ),
        _ => ledger.accept_event(
            event.event_id.get().to_string(),
            format!("runtime:{:?}", event.source),
            event.event_id.get(),
        ),
    };
    matches!(decision, EventLedgerDecision::Accept)
}

pub(in crate::r#loop::agent_tui) fn wire_stream_key(
    session_id: &str,
    run_id: Option<&str>,
) -> String {
    format!("wire:{session_id}:{}", run_id.unwrap_or("default"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::r#loop::agent_tui) struct RuntimePartAnchor {
    part_id: String,
    cell_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::r#loop::agent_tui) enum RuntimePartState {
    Pending,
    Anchored(RuntimePartAnchor),
}

impl RuntimePartState {
    pub(in crate::r#loop::agent_tui) fn anchor(&self) -> Option<&RuntimePartAnchor> {
        match self {
            Self::Pending => None,
            Self::Anchored(anchor) => Some(anchor),
        }
    }
}

impl RuntimePartAnchor {
    pub(in crate::r#loop::agent_tui) fn new(part_id: String, cell_index: usize) -> Self {
        Self {
            part_id,
            cell_index,
        }
    }

    pub(in crate::r#loop::agent_tui) fn part_id(&self) -> &str {
        &self.part_id
    }

    pub(in crate::r#loop::agent_tui) const fn cell_index(&self) -> usize {
        self.cell_index
    }
}

fn watcher_summary(event: &RuntimeWatcherEvent) -> String {
    match &event.payload {
        crate::surface::watchers::RuntimeWatcherPayload::Refresh(payload) => {
            format!("{} refresh requested", payload.source.as_code())
        }
    }
}
