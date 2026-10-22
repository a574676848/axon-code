use crate::tool_orchestrator::{ToolExecutionRequest, ToolKind, ToolRuntimeEvent, classify_tool};
use axon_agent_runtime::ProviderToolCall;
use axon_domain::{
    CellId, ExecutionRequestId, SessionId, ToolCallId, ToolConcurrencyPolicy,
    ToolExecutionPermission,
};
use std::time::Instant;

#[test]
fn classify_tool_accepts_only_canonical_names() {
    assert_eq!(classify_tool("builtin.lsp"), ToolKind::Unsupported);
    assert_eq!(classify_tool("builtin.update_plan"), ToolKind::UpdatePlan);
    assert_eq!(classify_tool("builtin_lsp"), ToolKind::Unsupported);
    assert_eq!(classify_tool("builtin.spawn_agent"), ToolKind::Subagent);
}

fn provider_tool_call(name: &str) -> ProviderToolCall {
    ProviderToolCall::new(name, name, "{}")
}

#[test]
fn parallel_tool_batch_progress_tracks_total_and_completed_items() {
    let request = ToolExecutionRequest {
        tool_call: provider_tool_call("builtin.glob"),
        tool_call_id: ToolCallId(3),
        part_id: "tool-part:test:parallel:1".to_owned(),
        execution_request_id: ExecutionRequestId(3),
        kind: ToolKind::Glob,
        concurrency_policy: ToolConcurrencyPolicy::ParallelSafe,
        execution_permission: ToolExecutionPermission::Allowed,
    };

    let ToolRuntimeEvent::Progress(progress) =
        super::parallel_tool_batch_progress(&request, 1, 3, Instant::now())
    else {
        panic!("并行批次进度必须使用 Progress 事件");
    };

    assert_eq!(progress.phase, "batch_running");
    assert_eq!(progress.items_completed, Some(2));
    assert_eq!(progress.items_total, Some(3));
    assert_eq!(progress.current_item.as_deref(), Some("builtin.glob"));
    assert_eq!(progress.message.as_deref(), Some("并行工具批次执行中"));
}

#[test]
fn declared_concurrency_policy_controls_parallel_batches() {
    let mut execution_requests = vec![
        ToolExecutionRequest {
            tool_call: provider_tool_call("builtin.edit"),
            tool_call_id: ToolCallId(1),
            part_id: "tool-part:test:parallel-blocked:1".to_owned(),
            execution_request_id: ExecutionRequestId(1),
            kind: ToolKind::Edit,
            concurrency_policy: ToolConcurrencyPolicy::WorkspaceCoordinated,
            execution_permission: ToolExecutionPermission::Allowed,
        },
        ToolExecutionRequest {
            tool_call: provider_tool_call("builtin.search"),
            tool_call_id: ToolCallId(2),
            part_id: "tool-part:test:parallel-blocked:2".to_owned(),
            execution_request_id: ExecutionRequestId(2),
            kind: ToolKind::Search,
            concurrency_policy: ToolConcurrencyPolicy::ParallelSafe,
            execution_permission: ToolExecutionPermission::Allowed,
        },
    ];

    // 需要审批的同段工具不再把整批拖成串行：审批决定由协调线程在 worker 执行前给出。
    assert!(super::tool_calls_can_run_in_parallel(&execution_requests));

    execution_requests[0].concurrency_policy = ToolConcurrencyPolicy::Exclusive;
    assert!(!super::tool_calls_can_run_in_parallel(&execution_requests));
}

/// 核心收益：作业工具是 `ParallelSafe`，含作业的批次不再被强制串行。
///
/// 批次经真实注册表与调度器构造，因此并发策略来自工具定义本身：
/// 若作业工具未注册（或注册成 `Exclusive`），本用例会因回落到 `Exclusive` 而失败。
#[test]
fn job_tools_keep_their_batch_parallel_while_session_tools_do_not() {
    let registry = registry_with_builtin_tools();

    let job_batch = scheduled_requests(
        &registry,
        &[
            ("builtin.job_output", r#"{"job_id":"1"}"#),
            ("builtin.read_file", r#"{"path":"Cargo.toml"}"#),
        ],
    );
    assert!(
        super::tool_calls_can_run_in_parallel(&job_batch),
        "读取后台作业必须与其他只读工具并行：{:?}",
        job_batch
            .iter()
            .map(|request| request.concurrency_policy)
            .collect::<Vec<_>>()
    );

    // P6：把「可并行」从布尔结论强化为**可计数的并行度**：
    // 批次内每一个请求都必须是 `ParallelSafe`，否则策略允许的实际并行上限
    // 小于批次大小，「收益」就不再等于整批并行。
    let parallel_safe = job_batch
        .iter()
        .filter(|request| request.concurrency_policy == ToolConcurrencyPolicy::ParallelSafe)
        .count();
    assert_eq!(
        parallel_safe,
        job_batch.len(),
        "作业批次的并行度必须等于批次大小，实际 {parallel_safe}/{}",
        job_batch.len()
    );

    // 反例守护：交互会话仍要求独占。若这条也并行，说明作业通道的收益
    // 并不是「独立出来」带来的，而是整批串行被整体放宽了。
    let session_batch = scheduled_requests(
        &registry,
        &[
            ("builtin.write_stdin", r#"{"session_id":"sess-1"}"#),
            ("builtin.read_file", r#"{"path":"Cargo.toml"}"#),
        ],
    );
    assert!(
        !super::tool_calls_can_run_in_parallel(&session_batch),
        "write_stdin 必须保持 Exclusive，否则同一批次会与交互会话竞争"
    );
}

fn plain_request(
    request_id: u64,
    name: &str,
    concurrency_policy: ToolConcurrencyPolicy,
) -> ToolExecutionRequest {
    ToolExecutionRequest {
        tool_call: provider_tool_call(name),
        tool_call_id: ToolCallId(request_id),
        part_id: format!("tool-part:test:segment:{request_id}"),
        execution_request_id: ExecutionRequestId(request_id),
        kind: classify_tool(name),
        concurrency_policy,
        execution_permission: ToolExecutionPermission::Allowed,
    }
}

/// 并发子段切分：`Exclusive` 请求自成一段，其余请求合并成可并发的一段。
#[test]
fn exclusive_requests_split_the_batch_into_sequential_segments() {
    let requests = vec![
        plain_request(1, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(2, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
        plain_request(3, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(4, "builtin.glob", ToolConcurrencyPolicy::ParallelSafe),
    ];

    assert_eq!(
        super::parallel_tool_batch_ranges(&requests),
        vec![0..1, 1..2, 2..4]
    );

    // 段必须完整覆盖批次且顺序不重叠：任何请求都不能被跳过，也不能被重排。
    for requests in [
        Vec::new(),
        vec![plain_request(
            1,
            "builtin.glob",
            ToolConcurrencyPolicy::ParallelSafe,
        )],
        vec![
            plain_request(1, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
            plain_request(2, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
        ],
    ] {
        let ranges = super::parallel_tool_batch_ranges(&requests);
        let mut cursor = 0usize;
        for range in &ranges {
            assert_eq!(range.start, cursor, "段之间不得留下空洞或重叠");
            cursor = range.end;
        }
        assert_eq!(cursor, requests.len(), "段必须覆盖到批次末尾");
    }
}

fn registry_with_builtin_tools() -> axon_agent_runtime::ToolRegistry {
    let mut registry = axon_agent_runtime::ToolRegistry::new();
    axon_agent_runtime::AgentToolRegistryRuntime::register_builtin_tools(&mut registry)
        .expect("内置工具应注册成功");
    registry
}

fn scheduled_requests(
    registry: &axon_agent_runtime::ToolRegistry,
    calls: &[(&str, &str)],
) -> Vec<ToolExecutionRequest> {
    let tool_calls = calls
        .iter()
        .enumerate()
        .map(|(index, (name, input))| ProviderToolCall::new(format!("call-{index}"), *name, *input))
        .collect::<Vec<_>>();

    crate::tool_orchestrator::schedule_tool_execution(
        registry,
        &tool_calls,
        1,
        std::path::Path::new("."),
        SessionId(1),
        CellId(1),
        1,
    )
    .expect("批次应调度成功")
    .requests
}
