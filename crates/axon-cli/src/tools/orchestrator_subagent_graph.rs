//! 子代理工具的 graph 同步与恢复。

use axon_agent_runtime::{
    AgentControlGraphSyncRuntime, AgentControlRuntime, AgentPath, AgentStatus,
    LIST_AGENTS_TOOL_NAME, ProviderToolCall, SEND_MESSAGE_TOOL_NAME,
};

use crate::tool_orchestrator::ToolOrchestratorContext;

pub(super) fn sync_spawned_agent_graph(
    context: &ToolOrchestratorContext,
    control: &AgentControlRuntime,
    task_name: &str,
    created_at_ms: u64,
) -> axon_domain::AxonResult<()> {
    let child_path = AgentPath::parse(task_name)?;
    let parent_path = child_path
        .parent()
        .ok_or(axon_domain::AxonError::runtime_failure(
            axon_agent_runtime::AGENT_CONTROL_REASON_INVALID_PATH,
        ))?;
    block_on_graph_sync(AgentControlGraphSyncRuntime.sync_spawned_agent(
        control,
        context.agent_graph_store.as_ref(),
        &parent_path,
        &child_path,
        created_at_ms,
        &context.cwd.to_string_lossy(),
    ))
    .map(|_| ())
}

pub(super) fn restore_target_for_tool(
    context: &ToolOrchestratorContext,
    control: &mut AgentControlRuntime,
    tool_call: &ProviderToolCall,
    current_path: &AgentPath,
) -> axon_domain::AxonResult<()> {
    let Some(target) = tool_target_path(tool_call, current_path) else {
        return Ok(());
    };
    if control.status(&target) != AgentStatus::NotFound {
        return Ok(());
    }
    let root_thread_id = control.root_thread_id()?;
    let exists = block_on_graph_sync(
        context
            .agent_graph_store
            .find_thread_spawn_descendant_by_path(root_thread_id, target.as_str()),
    )?
    .is_some();
    if !exists {
        return Ok(());
    }
    restore_control_from_graph_store(context, control, root_thread_id)
}

pub(super) fn restore_visible_agents_for_tool(
    context: &ToolOrchestratorContext,
    control: &mut AgentControlRuntime,
    tool_call: &ProviderToolCall,
) -> axon_domain::AxonResult<()> {
    if !matches!(
        tool_call.dispatch_name(),
        LIST_AGENTS_TOOL_NAME | "builtin.list_agents"
    ) || control.active_agent_count() > 0
    {
        return Ok(());
    }
    let root_thread_id = control.root_thread_id()?;
    let has_open_children = !block_on_graph_sync(
        context
            .agent_graph_store
            .list_thread_spawn_children(root_thread_id),
    )?
    .is_empty();
    if !has_open_children {
        return Ok(());
    }
    restore_control_from_graph_store(context, control, root_thread_id)
}

fn restore_control_from_graph_store(
    context: &ToolOrchestratorContext,
    control: &mut AgentControlRuntime,
    root_thread_id: u64,
) -> axon_domain::AxonResult<()> {
    let (mut restored, _) =
        block_on_graph_sync(AgentControlRuntime::restore_open_tree_from_store(
            context.agent_graph_store.as_ref(),
            root_thread_id,
            control.config(),
        ))?;
    // 拓扑恢复会新建控制面实例，必须把 mailbox 持久化绑定与作用域一并带过去，
    // 否则替换实例等于丢弃跨轮信箱事实。
    restored.inherit_mailbox_persistence(control);
    // 拓扑就位后立刻补全信箱事实：先把上一进程遗留的待投递消息装回内存 mailbox，
    // 再把到期的待投递消息重新投递（合同 §5.3.1 / §5.3.2）。两步都在恢复点内完成，
    // 因此模型下一次读到子代理状态时看到的是已经补全的 mailbox。
    let now_ms = crate::tool_orchestrator::now_ms();
    restored.hydrate_mailbox(now_ms)?;
    restored.drain_mailbox_delivery(now_ms)?;
    *control = restored;
    Ok(())
}

/// 协作工具里带 `target` 字段的那些：只有 `send_message` 会寻址协作目标。
fn tool_target_path(tool_call: &ProviderToolCall, current_path: &AgentPath) -> Option<AgentPath> {
    match tool_call.dispatch_name() {
        SEND_MESSAGE_TOOL_NAME | "builtin.send_message" => {}
        _ => return None,
    }
    let arguments = tool_call.input.as_json()?;
    let value = arguments
        .get("target")
        .and_then(serde_json::Value::as_str)?;
    current_path.resolve(value).ok()
}

fn block_on_graph_sync<T>(
    future: impl std::future::Future<Output = axon_domain::AxonResult<T>>,
) -> axon_domain::AxonResult<T> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| axon_domain::AxonError::runtime_failure("subagent_graph:runtime"))?
        .block_on(future)
}
