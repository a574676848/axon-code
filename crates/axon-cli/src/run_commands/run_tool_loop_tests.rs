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

/// 并发子段切分：隔离调用（独占、整工作区写、读盘但无路径声明）各自成段。
#[test]
fn isolated_requests_split_the_batch_into_sequential_segments() {
    let requests = vec![
        plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(2, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
        plain_request(
            3,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        // 写调用自成隔离段。
        plain_request(
            4,
            "builtin.edit",
            ToolConcurrencyPolicy::WorkspaceCoordinated,
        ),
    ];

    assert_eq!(segments(&requests), vec![0..1, 1..2, 2..3, 3..4]);

    // 段必须完整覆盖批次且顺序不重叠：任何请求都不能被跳过，也不能被重排。
    for requests in [
        Vec::new(),
        vec![plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        )],
        vec![
            plain_request(1, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
            plain_request(2, "builtin.write_stdin", ToolConcurrencyPolicy::Exclusive),
        ],
    ] {
        let ranges = segments(&requests);
        let mut cursor = 0usize;
        for range in &ranges {
            assert_eq!(range.start, cursor, "段之间不得留下空洞或重叠");
            cursor = range.end;
        }
        assert_eq!(cursor, requests.len(), "段必须覆盖到批次末尾");
    }
}

/// 同批两个只读调用应合并成一段并发，而不是被拆开。
#[test]
fn sibling_reads_share_one_parallel_segment() {
    let requests = vec![
        plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(
            2,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(3, "builtin.job_output", ToolConcurrencyPolicy::ParallelSafe),
    ];
    assert_eq!(segments(&requests), vec![0..3]);
}

/// 按执行域与读写交叉切段：独占请求自成一段，其余按路径冲突断开。
fn segments(requests: &[ToolExecutionRequest]) -> Vec<std::ops::Range<usize>> {
    let cwd = std::path::Path::new(".");
    let scopes = requests
        .iter()
        .map(|request| super::tool_access_scope(cwd, request))
        .collect::<Vec<_>>();
    super::tool_batch_ranges(&scopes)
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

fn fake_receipt(name: &str) -> super::RunToolReceipt {
    super::RunToolReceipt {
        tool_call_id: 0,
        part_id: name.to_owned(),
        provider_call_id: name.to_owned(),
        tool_name: name.to_owned(),
        status: "completed".to_owned(),
        summary: name.to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    }
}

fn timed_receipt(name: &str) -> super::TimedToolReceipt {
    super::TimedToolReceipt {
        receipt: fake_receipt(name),
        duration_ms: Some(1),
    }
}

/// `S-6`：整批已取消时，回执数量必须等于调用数量，且每条都带配对 id。
#[test]
fn cancelled_batch_returns_one_paired_receipt_per_call() {
    let requests = vec![
        plain_request(1, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(2, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(3, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
    ];
    let receipts = super::cancelled_receipts(&requests, None);
    assert_eq!(receipts.len(), requests.len());
    for (receipt, request) in receipts.iter().zip(&requests) {
        assert_eq!(receipt.tool_call_id, request.tool_call_id.0);
        assert_eq!(receipt.part_id, request.part_id);
        assert!(receipt.summary.contains("未启动"));
    }
}

/// `S-2`：整工作区写（写入集不完整）必须自成一段。
#[test]
fn workspace_write_is_isolated_into_its_own_segment() {
    let requests = vec![
        plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(
            2,
            "builtin.shell_command",
            ToolConcurrencyPolicy::WorkspaceCoordinated,
        ),
        plain_request(
            3,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
    ];
    assert_eq!(segments(&requests), vec![0..1, 1..2, 2..3]);
}

/// `S-4`：完成顺序 C、A、B，提交顺序必须是 A、B、C。
#[test]
fn ordered_commit_follows_model_order_not_completion_order() {
    let requests = vec![
        plain_request(1, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(2, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(3, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
    ];
    let mut slots = vec![None, None, None];
    let mut receipts = vec![None, None, None];
    let mut committed = 0usize;

    slots[2] = Some(timed_receipt("c"));
    super::commit_ready_receipts(
        &mut slots,
        &mut receipts,
        &mut committed,
        &requests,
        None,
        &mut |_| {},
    );
    assert_eq!(committed, 0, "前序未就绪时不得提交后序结果");

    slots[0] = Some(timed_receipt("a"));
    super::commit_ready_receipts(
        &mut slots,
        &mut receipts,
        &mut committed,
        &requests,
        None,
        &mut |_| {},
    );
    assert_eq!(committed, 1, "游标只前进到连续就绪处");

    slots[1] = Some(timed_receipt("b"));
    super::commit_ready_receipts(
        &mut slots,
        &mut receipts,
        &mut committed,
        &requests,
        None,
        &mut |_| {},
    );
    assert_eq!(committed, 3);

    let order = receipts
        .into_iter()
        .map(|receipt| receipt.expect("每个槽位都应有回执").provider_call_id)
        .collect::<Vec<_>>();
    assert_eq!(order, vec!["a", "b", "c"]);
}

/// `S-6`：取消后未启动的调用必须有合成回执；游标之后已到达的真实回执不得被覆盖。
#[test]
fn cancel_fills_unstarted_calls_with_paired_synthetic_receipts() {
    let requests = vec![
        plain_request(1, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(2, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
        plain_request(3, "builtin.read_file", ToolConcurrencyPolicy::ParallelSafe),
    ];
    let mut slots = vec![None, None, None];
    // 第 3 个调用的真实回执已到达，但游标还停在第 2 个。
    slots[2] = Some(timed_receipt("real-c"));
    let mut receipts = vec![None, None, None];
    receipts[0] = Some(fake_receipt("a"));

    super::fill_unstarted_receipts(&mut slots, &mut receipts, 1, &requests, None);

    assert!(receipts.iter().all(Option::is_some), "不得留下孤立的 call");
    let receipt = receipts[1].as_ref().expect("未启动的调用必须有合成回执");
    assert!(receipt.summary.contains("未启动"));
    assert_eq!(receipt.part_id, requests[1].part_id);
    assert_eq!(receipt.tool_call_id, requests[1].tool_call_id.0);
    let real = receipts[2].as_ref().expect("真实回执必须保留");
    assert_eq!(
        real.provider_call_id, "real-c",
        "不得用合成回执覆盖真实结果"
    );
}

/// G3：真实池调度下，完成序 2→1→0，提交序必须是 0→1→2。
#[test]
fn parallel_pool_commits_in_model_order_under_reordering() {
    let requests = vec![
        plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(
            2,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(
            3,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
    ];
    let mut commit_log = Vec::new();
    let receipts = super::execute_parallel_for_tests(&requests, &[80, 10, 40], &mut commit_log);
    assert_eq!(commit_log, vec![0, 1, 2], "提交必须按模型序，不按完成序");
    assert_eq!(receipts.len(), 3);
    for (receipt, request) in receipts.iter().zip(&requests) {
        assert_eq!(receipt.part_id, request.part_id);
    }
}

/// G2：写调用把批次切成前后屏障段，读调用不能被夹在写中间并发。
#[test]
fn write_call_barriers_the_batch_around_it() {
    let requests = vec![
        plain_request(
            1,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
        plain_request(
            2,
            "builtin.edit",
            ToolConcurrencyPolicy::WorkspaceCoordinated,
        ),
        plain_request(
            3,
            "builtin.memory_search",
            ToolConcurrencyPolicy::ParallelSafe,
        ),
    ];
    assert_eq!(segments(&requests), vec![0..1, 1..2, 2..3]);
}
