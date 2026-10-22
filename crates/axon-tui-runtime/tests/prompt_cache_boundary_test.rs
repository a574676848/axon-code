//! TUI prompt cache 边界集成验证。

use axon_agent_runtime::{
    AgentContextRuntime, AgentContextSnapshotInput, AgentModelMessage, AgentModelPayload,
    AgentModelPayloadInput, AgentModelPayloadRuntime, AgentPaneProjection,
    AgentPaneProjectionInput, AgentPromptContext, PromptCacheAnchor, PromptCacheAnchorInput,
    PromptCacheAnchorRuntime, PromptCacheDynamicContextRef,
};
use axon_domain::{
    CellClosureState, CellId, ModelProfileId, OutputDrainState, PaneId, PaneSecurityBadge,
    PaneSecurityBadgeLevel, PermissionSnapshotId, RuntimeCellSnapshot, RuntimeCellState,
    TokenBudget,
};

const TEST_CWD: &str = "E:\\zbg\\axon-code";
const TEST_PROVIDER_POLICY_HASH: &str = "provider-policy-v1";
const TEST_MODEL_PROFILE_HASH: &str = "model-profile-v1";
const TEST_SOURCE_REF: &str = "tui-boundary://provider-visible-payload";
const TEST_CONTEXT_LIMIT: u64 = 2_048;
const TEST_RESERVED_TOOLS: u64 = 256;
const TEST_HASH_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const TEST_HASH_PRIME: u64 = 0x0100_0000_01b3;

#[test]
fn tui_render_state_does_not_change_prompt_cache_anchor() {
    let baseline = prompt_cache_probe("解释当前 TUI 渲染边界", TuiRenderFacts::baseline());
    let changed_ui = prompt_cache_probe(
        "解释当前 TUI 渲染边界",
        TuiRenderFacts {
            dock_open: true,
            viewport_scroll: 6_543,
            selected_cell: Some(42_424),
            animation_tick: 987_654_321,
            cell_id: 88_888,
            cell_revision: 777_777,
        },
    );

    assert_eq!(baseline.payload.messages, changed_ui.payload.messages);
    assert_eq!(baseline.anchor.cache_key, changed_ui.anchor.cache_key);
    assert_eq!(
        baseline.anchor.stable_prefix_hash,
        changed_ui.anchor.stable_prefix_hash
    );
    assert!(!baseline.anchor.dynamic_context_in_cache_key);
}

#[test]
fn draft_text_changes_prompt_cache_anchor_without_ui_state_leaking() {
    let baseline = prompt_cache_probe("解释当前 TUI 渲染边界", TuiRenderFacts::baseline());
    let edited_draft = prompt_cache_probe(
        "解释当前 TUI 渲染边界，并列出验证步骤",
        TuiRenderFacts {
            dock_open: true,
            viewport_scroll: 6_543,
            selected_cell: Some(42_424),
            animation_tick: 987_654_321,
            cell_id: 88_888,
            cell_revision: 777_777,
        },
    );

    assert_ne!(baseline.anchor.cache_key, edited_draft.anchor.cache_key);
    assert_ne!(
        provider_visible_message_fingerprint(&baseline.payload.messages),
        provider_visible_message_fingerprint(&edited_draft.payload.messages)
    );
    assert!(
        edited_draft
            .payload
            .messages
            .iter()
            .all(|message| !contains_tui_render_fact(message, &edited_draft.ui_facts))
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TuiRenderFacts {
    dock_open: bool,
    viewport_scroll: u16,
    selected_cell: Option<u64>,
    animation_tick: u64,
    cell_id: u64,
    cell_revision: u64,
}

impl TuiRenderFacts {
    const fn baseline() -> Self {
        Self {
            dock_open: false,
            viewport_scroll: 0,
            selected_cell: None,
            animation_tick: 0,
            cell_id: 1,
            cell_revision: 1,
        }
    }
}

struct PromptCacheProbe {
    payload: AgentModelPayload,
    anchor: PromptCacheAnchor,
    ui_facts: TuiRenderFacts,
}

fn prompt_cache_probe(user_prompt: &str, ui_facts: TuiRenderFacts) -> PromptCacheProbe {
    let payload = AgentModelPayloadRuntime
        .build(&AgentModelPayloadInput {
            user_prompt: user_prompt.to_owned(),
            stable_prefix: AgentModelPayloadRuntime.compile_stable_prefix(),
            stable_context_messages: Vec::new(),
            dynamic_context_messages: Vec::new(),
            history_messages: Vec::new(),
            prompt_context: AgentPromptContext::default(),
            runtime_constraints: Some("execution_mode=direct".to_owned()),
            pane_projection: pane_projection(),
            context_snapshot: context_snapshot(),
        })
        .unwrap();
    let anchor = prompt_cache_anchor(&payload);
    PromptCacheProbe {
        payload,
        anchor,
        ui_facts,
    }
}

fn pane_projection() -> AgentPaneProjection {
    AgentPaneProjection::from_input(AgentPaneProjectionInput {
        pane_id: PaneId(1),
        cwd: TEST_CWD.to_owned(),
        shell_kind: "powershell".to_owned(),
        git_state: None,
        recent_command: None,
        permission_snapshot_id: PermissionSnapshotId(1),
        backend_badge: PaneSecurityBadge {
            level: PaneSecurityBadgeLevel::Normal,
            label: "local".to_owned(),
            reasons: Vec::new(),
        },
        runtime_cell: Some(runtime_cell_snapshot()),
        response: None,
        apply_execution: None,
    })
}

fn context_snapshot() -> axon_agent_runtime::AgentContextSnapshot {
    AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(1),
        cwd: TEST_CWD.to_owned(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(TEST_CONTEXT_LIMIT).unwrap(),
        reserved_for_tools: TokenBudget::new(TEST_RESERVED_TOOLS).unwrap(),
    })
}

fn runtime_cell_snapshot() -> RuntimeCellSnapshot {
    RuntimeCellSnapshot {
        id: CellId(1),
        pane_id: PaneId(1),
        cwd: TEST_CWD.to_owned(),
        model_profile_id: ModelProfileId(1),
        permission_snapshot_id: PermissionSnapshotId(1),
        state: RuntimeCellState::Running,
        output_drain: OutputDrainState::Drained,
        closure: CellClosureState::Open,
        event_count: 1,
    }
}

fn prompt_cache_anchor(payload: &AgentModelPayload) -> PromptCacheAnchor {
    let system_prompt = payload
        .messages
        .iter()
        .find(|message| message.role == "system")
        .map_or("", |message| message.content.as_str());
    let message_fingerprint = provider_visible_message_fingerprint(&payload.messages);

    PromptCacheAnchorRuntime
        .plan(PromptCacheAnchorInput {
            system_prompt_hash: stable_test_hash(system_prompt),
            developer_policy_hash: stable_test_hash(TEST_PROVIDER_POLICY_HASH),
            tool_schema_hashes: payload
                .model_tools
                .iter()
                .map(|tool| tool.schema_hash.clone())
                .collect(),
            soul_hash: stable_test_hash(""),
            user_agents_hash: stable_test_hash(""),
            personal_memory_hash: stable_test_hash(""),
            config_hash: stable_test_hash(&message_fingerprint),
            model_profile_hash: stable_test_hash(TEST_MODEL_PROFILE_HASH),
            dynamic_context: vec![PromptCacheDynamicContextRef {
                source_ref: TEST_SOURCE_REF.to_owned(),
                summary_hash: stable_test_hash(&message_fingerprint),
                token_estimate: token_estimate(&message_fingerprint),
            }],
            previous_anchor: None,
        })
        .unwrap()
}

fn provider_visible_message_fingerprint(messages: &[AgentModelMessage]) -> String {
    messages
        .iter()
        .map(|message| format!("{}:{}", message.role, stable_test_hash(&message.content)))
        .collect::<Vec<_>>()
        .join("|")
}

fn stable_test_hash(value: &str) -> String {
    format!(
        "{:016x}",
        value.bytes().fold(TEST_HASH_OFFSET_BASIS, |hash, byte| {
            hash.wrapping_mul(TEST_HASH_PRIME) ^ u64::from(byte)
        })
    )
}

fn token_estimate(value: &str) -> u64 {
    u64::try_from(value.chars().count() / 4).unwrap_or(u64::MAX)
}

fn contains_tui_render_fact(message: &AgentModelMessage, facts: &TuiRenderFacts) -> bool {
    [
        format!("dock_open={}", facts.dock_open),
        format!("viewport_scroll={}", facts.viewport_scroll),
        format!("animation_tick={}", facts.animation_tick),
        format!("cell_id={}", facts.cell_id),
        format!("cell_revision={}", facts.cell_revision),
    ]
    .iter()
    .any(|value| message.content.contains(value))
        || facts
            .selected_cell
            .is_some_and(|cell| message.content.contains(&format!("selected_cell={cell}")))
}
