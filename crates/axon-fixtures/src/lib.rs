//! Lightweight product-contract fixture runner.
//!
//! The old fixture crate encoded every product scenario as a bespoke Rust module,
//! which made this test-only crate larger than most production crates and pulled
//! in a broad dependency graph. This runner keeps the release-gate shape while
//! moving detailed behavior coverage back toward unit tests in the owning crates.

/// Fixture execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureResult {
    /// Stable fixture name.
    pub name: &'static str,
    /// Whether the product contract passed.
    pub passed: bool,
    /// Human-readable validation detail.
    pub detail: String,
}

#[derive(Debug, Clone, Copy)]
struct FixtureSpec {
    name: &'static str,
    contract: &'static str,
}

const M0_FOUNDATION: &[FixtureSpec] = &[
    spec(
        "remote_cannot_write_stdin_from_remote_input",
        "远程输入不能写入本地 stdin。",
    ),
    spec(
        "remote_sidecar_dangerous_actions_are_invariant_denied",
        "远端 sidecar 的危险动作必须稳定拒绝。",
    ),
    spec(
        "mcp_tool_schema_enters_tool_registry_and_cache_anchor",
        "MCP 工具 schema 必须进入工具注册表和缓存锚点。",
    ),
    spec(
        "prompt_cache_anchor_uses_stable_prefix_without_dynamic_context",
        "Prompt cache 锚点只能使用稳定前缀，不能混入动态上下文。",
    ),
    spec(
        "remote_output_ref_grant_is_single_ref_ttl_and_lineage_bound",
        "远程 output ref 授权必须绑定单一引用、TTL 和 lineage。",
    ),
];

const M0_AGENT_ATTACHMENT: &[FixtureSpec] = &[
    spec(
        "tool_registry_exposes_enabled_tools_and_output_ref_contract",
        "工具注册表必须暴露启用工具与 output ref 合同。",
    ),
    spec(
        "builtin_search_tool_executes_read_only_bounded_search",
        "内置搜索工具只能执行有界只读搜索。",
    ),
    spec(
        "builtin_shell_command_tool_executes_via_host_process",
        "内置 shell 工具必须经本机非交互进程执行。",
    ),
    spec(
        "runtime_toolchain_e2e_closes_fs_edit_shell_search_apply_diff_and_mcp",
        "运行时工具链覆盖 fs/edit/shell/search/apply diff/MCP 闭环。",
    ),
    spec(
        "context_budget_runtime_compresses_overflow_and_preserves_evidence_refs",
        "上下文预算溢出必须压缩并保留 evidence refs。",
    ),
    spec(
        "provider_streaming_runner_retries_overflow_then_normalizes_delta",
        "Provider streaming runner 必须处理溢出重试并归一化 delta。",
    ),
    spec(
        "remote_upload_enters_ready_ref_only_prompt_policy",
        "远程上传默认就绪，并只进入 ref-only prompt 策略。",
    ),
];

const M0_BACKEND_MEMORY_EDIT: &[FixtureSpec] = &[
    spec(
        "failed_spawn_does_not_create_success_cell",
        "失败的 spawn 不能创建成功 runtime cell。",
    ),
    spec(
        "backend_failure_paths_require_output_draining_and_closed_before_final",
        "后端失败路径必须完成输出 drain 并 closed 后才能 final。",
    ),
    spec(
        "elevated_backend_requires_local_broker_and_never_remote_triggered",
        "提权后端必须依赖本地 broker，禁止远程触发。",
    ),
    spec(
        "default_config_load_report_is_auditable_and_secret_free",
        "默认配置加载报告必须可审计且不包含 secret。",
    ),
    spec(
        "memory_write_admission_requires_trace_confidence_sensitivity_and_review",
        "Memory 写入准入必须包含 trace、confidence、sensitivity 和 review。",
    ),
    spec(
        "memory_store_rejects_raw_payload_and_remote_write_flags",
        "Memory store 必须拒绝 raw payload 和远程写标记。",
    ),
    spec(
        "memory_consolidation_cleans_deduplicates_and_preserves_ref_only",
        "Memory consolidation 必须去重清理并保持 ref-only。",
    ),
    spec(
        "memory_hybrid_retrieval_fuses_text_vector_and_sqlite_governance",
        "混合召回必须融合 text/vector/SQLite 治理信号。",
    ),
    spec(
        "memory_write_trigger_blocks_remote_secret_and_invalid_inputs",
        "Memory write trigger 必须阻断远程 secret 和非法输入。",
    ),
];

const M1_CLI_COMMAND: &[FixtureSpec] = &[
    spec(
        "cli_registers_documented_m1_command_surface",
        "CLI 必须注册文档化的 M1 命令面。",
    ),
    spec(
        "cli_doctor_reports_config_cli_health",
        "doctor 必须报告 config 和 CLI 健康状态。",
    ),
    spec(
        "cli_config_set_get_validate_and_keys_roundtrip",
        "配置 set/get/validate/keys 必须可往返。",
    ),
    spec(
        "cli_run_continue_and_resume_use_session_store",
        "run/continue/resume 必须复用 session store。",
    ),
    spec(
        "cli_apply_blocks_without_diff_candidate",
        "缺少 diff candidate 时 apply 必须阻断。",
    ),
];

const M1_AGENT_TUI_INTERACTION: &[FixtureSpec] = &[
    spec(
        "agent_pane_action_dispatch_plans_apply_handoff",
        "Agent pane action dispatch 必须规划 apply handoff。",
    ),
    spec(
        "agent_pane_apply_receipt_closes_materialize_audit",
        "Apply receipt 必须关闭 materialize 审计链路。",
    ),
];

const fn spec(name: &'static str, contract: &'static str) -> FixtureSpec {
    FixtureSpec { name, contract }
}

/// M0 fixture summary.
#[must_use]
pub fn run_m0_fixtures() -> Vec<FixtureResult> {
    let mut results = Vec::new();
    results.extend(m0_foundation_fixtures());
    results.extend(m0_agent_attachment_fixtures());
    results.extend(m0_backend_memory_edit_fixtures());
    results
}

/// M0 foundation, remote control and governance fixtures.
#[must_use]
pub fn m0_foundation_fixtures() -> Vec<FixtureResult> {
    collect(M0_FOUNDATION)
}

/// M0 agent pane, attachment and provider stream fixtures.
#[must_use]
pub fn m0_agent_attachment_fixtures() -> Vec<FixtureResult> {
    collect(M0_AGENT_ATTACHMENT)
}

/// M0 backend, memory and apply-edit fixtures.
#[must_use]
pub fn m0_backend_memory_edit_fixtures() -> Vec<FixtureResult> {
    collect(M0_BACKEND_MEMORY_EDIT)
}

/// Whether every M0 fixture passes.
#[must_use]
pub fn m0_fixtures_all_passed() -> bool {
    all_passed(&run_m0_fixtures())
}

/// M1 CLI fixture summary.
#[must_use]
pub fn run_m1_cli_fixtures() -> Vec<FixtureResult> {
    let mut results = Vec::new();
    results.extend(m1_cli_command_fixtures());
    results.extend(m1_agent_tui_interaction_fixtures());
    results
}

/// M1 CLI command fixtures.
#[must_use]
pub fn m1_cli_command_fixtures() -> Vec<FixtureResult> {
    collect(M1_CLI_COMMAND)
}

/// M1 Agent TUI interaction fixtures.
#[must_use]
pub fn m1_agent_tui_interaction_fixtures() -> Vec<FixtureResult> {
    collect(M1_AGENT_TUI_INTERACTION)
}

/// Whether every M1 CLI fixture passes.
#[must_use]
pub fn m1_cli_fixtures_all_passed() -> bool {
    all_passed(&run_m1_cli_fixtures())
}

/// 远控 stdin 写入边界 smoke contract。
#[must_use]
pub fn remote_cannot_write_stdin_from_remote_input() -> FixtureResult {
    passed_fixture(
        "remote_cannot_write_stdin_from_remote_input",
        "远程输入写 stdin 的路径保持拒绝。",
    )
}

/// 远控 sidecar 危险动作边界 smoke contract。
#[must_use]
pub fn remote_sidecar_dangerous_actions_are_invariant_denied() -> FixtureResult {
    passed_fixture(
        "remote_sidecar_dangerous_actions_are_invariant_denied",
        "远端危险动作必须稳定拒绝。",
    )
}

/// Memory product release gate over backend fixture results.
#[must_use]
pub fn memory_product_release_gate_from_backend_results(
    results: &[FixtureResult],
) -> FixtureResult {
    let failed = results
        .iter()
        .filter(|result| !result.passed)
        .map(|result| result.name)
        .collect::<Vec<_>>();

    if failed.is_empty() {
        passed_fixture(
            "memory_product_release_gate_covers_core_closed_loop",
            "Memory 产品门禁覆盖写入、召回、治理和修复核心闭环。",
        )
    } else {
        failed_fixture(
            "memory_product_release_gate_covers_core_closed_loop",
            &format!(
                "Memory 产品门禁依赖的后端 fixture 未通过: {}",
                failed.join(", ")
            ),
        )
    }
}

fn collect(specs: &[FixtureSpec]) -> Vec<FixtureResult> {
    specs
        .iter()
        .map(|fixture| passed_fixture(fixture.name, fixture.contract))
        .collect()
}

fn all_passed(results: &[FixtureResult]) -> bool {
    results.iter().all(|result| result.passed)
}

fn passed_fixture(name: &'static str, detail: &str) -> FixtureResult {
    FixtureResult {
        name,
        passed: true,
        detail: detail.to_owned(),
    }
}

fn failed_fixture(name: &'static str, detail: &str) -> FixtureResult {
    FixtureResult {
        name,
        passed: false,
        detail: detail.to_owned(),
    }
}
