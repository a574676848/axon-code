//! 子代理控制面与 graph store 的同步适配层。
//!
//! 控制面保持同步、内存化；本模块负责把 spawn/close 的拓扑变化写入
//! `AgentGraphStore`，对齐 Codex 的 Open/Closed thread-spawn edge 写回语义。

use axon_domain::{AxonResult, ThreadSpawnEdgeStatus, ThreadSpawnNode};

use crate::agent_communication::AgentStatus;
use crate::agent_control_runtime::{
    AgentCloseReport, AgentControlRuntime, AgentPath, AgentSpawnReport, AgentSpawnRequest,
};
use crate::agent_graph_store::AgentGraphStore;

/// graph store 同步报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControlGraphSyncReport<T> {
    /// 控制面原始报告。
    pub control: T,
    /// 写入或更新的节点数量。
    pub synced_nodes: usize,
    /// 写入或更新的边数量。
    pub synced_edges: usize,
}

/// 子代理控制面 graph store 同步运行时。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentControlGraphSyncRuntime;

impl AgentControlGraphSyncRuntime {
    /// 执行 spawn，并把 root、child 与 Open edge 写入 graph store。
    pub async fn spawn_with_graph_store(
        &self,
        control: &mut AgentControlRuntime,
        store: &dyn AgentGraphStore,
        request: AgentSpawnRequest,
        created_at_ms: u64,
        cwd: &str,
    ) -> AxonResult<AgentControlGraphSyncReport<AgentSpawnReport>> {
        let parent_path = request.parent_path.clone();
        let report = control.spawn(request)?;
        let parent = control.controlled_agent(&parent_path)?;
        let child = control.controlled_agent(&report.path)?;

        ensure_parent_node(store, parent, created_at_ms, cwd).await?;
        store
            .create_thread_node(node_from_agent(
                child,
                Some(parent.thread_id),
                created_at_ms,
                cwd,
            ))
            .await?;
        store
            .upsert_thread_spawn_edge(parent.thread_id, child.thread_id, created_at_ms)
            .await?;

        Ok(AgentControlGraphSyncReport {
            control: report,
            synced_nodes: 2,
            synced_edges: 1,
        })
    }

    /// 将已完成的 spawn 控制面状态写入 graph store。
    pub async fn sync_spawned_agent(
        &self,
        control: &AgentControlRuntime,
        store: &dyn AgentGraphStore,
        parent_path: &AgentPath,
        child_path: &AgentPath,
        created_at_ms: u64,
        cwd: &str,
    ) -> AxonResult<AgentControlGraphSyncReport<()>> {
        let parent = control.controlled_agent(parent_path)?;
        let child = control.controlled_agent(child_path)?;

        ensure_parent_node(store, parent, created_at_ms, cwd).await?;
        store
            .create_thread_node(node_from_agent(
                child,
                Some(parent.thread_id),
                created_at_ms,
                cwd,
            ))
            .await?;
        store
            .upsert_thread_spawn_edge(parent.thread_id, child.thread_id, created_at_ms)
            .await?;

        Ok(AgentControlGraphSyncReport {
            control: (),
            synced_nodes: 2,
            synced_edges: 1,
        })
    }

    /// 执行 close，并把目标入边标记为 Closed。
    pub async fn close_with_graph_store(
        &self,
        control: &mut AgentControlRuntime,
        store: &dyn AgentGraphStore,
        path: &AgentPath,
        closed_at_ms: u64,
    ) -> AxonResult<AgentControlGraphSyncReport<AgentCloseReport>> {
        let closing = control.closing_agent_edge(path)?;
        let report = control.close(path)?;
        if let Some((parent_id, child_id)) = closing {
            store
                .update_edge_status(
                    parent_id,
                    child_id,
                    ThreadSpawnEdgeStatus::Closed,
                    Some(closed_at_ms),
                )
                .await?;
            store
                .update_thread_node(
                    child_id,
                    Some(closed_at_ms),
                    Some(status_snapshot(&AgentStatus::Shutdown)),
                    None,
                )
                .await?;
        }

        Ok(AgentControlGraphSyncReport {
            control: report,
            synced_nodes: usize::from(closing.is_some()),
            synced_edges: usize::from(closing.is_some()),
        })
    }

    /// 将即将关闭的目标入边写回 graph store。
    pub async fn sync_closing_agents(
        &self,
        control: &AgentControlRuntime,
        store: &dyn AgentGraphStore,
        path: &AgentPath,
        closed_at_ms: u64,
    ) -> AxonResult<AgentControlGraphSyncReport<()>> {
        let closing = control.closing_agent_edge(path)?;
        if let Some((parent_id, child_id)) = closing {
            store
                .update_edge_status(
                    parent_id,
                    child_id,
                    ThreadSpawnEdgeStatus::Closed,
                    Some(closed_at_ms),
                )
                .await?;
            store
                .update_thread_node(
                    child_id,
                    Some(closed_at_ms),
                    Some(status_snapshot(&AgentStatus::Shutdown)),
                    None,
                )
                .await?;
        }

        Ok(AgentControlGraphSyncReport {
            control: (),
            synced_nodes: usize::from(closing.is_some()),
            synced_edges: usize::from(closing.is_some()),
        })
    }

    /// 将子代理完成状态写入 graph store 节点快照，不关闭 Open edge。
    pub async fn sync_completed_agent(
        &self,
        control: &AgentControlRuntime,
        store: &dyn AgentGraphStore,
        path: &AgentPath,
        completed_at_ms: u64,
    ) -> AxonResult<AgentControlGraphSyncReport<()>> {
        let agent = control.controlled_agent(path)?;
        store
            .update_thread_node(
                agent.thread_id,
                Some(completed_at_ms),
                Some(status_snapshot(&agent.status)),
                agent.last_task_message.clone(),
            )
            .await?;

        Ok(AgentControlGraphSyncReport {
            control: (),
            synced_nodes: 1,
            synced_edges: 0,
        })
    }
}

async fn ensure_parent_node(
    store: &dyn AgentGraphStore,
    agent: &crate::agent_control_runtime::ControlledAgent,
    created_at_ms: u64,
    cwd: &str,
) -> AxonResult<()> {
    if store.get_thread_node(agent.thread_id).await?.is_none() {
        store
            .create_thread_node(node_from_agent(agent, None, created_at_ms, cwd))
            .await?;
    }
    Ok(())
}

fn node_from_agent(
    agent: &crate::agent_control_runtime::ControlledAgent,
    parent_id: Option<u64>,
    created_at_ms: u64,
    cwd: &str,
) -> ThreadSpawnNode {
    ThreadSpawnNode::new(
        agent.thread_id,
        agent.path.name().to_owned(),
        cwd.to_owned(),
        agent.path.depth(),
        parent_id,
        created_at_ms,
    )
    .with_agent_path(agent.path.to_string())
    .with_status_snapshot(Some(status_snapshot(&agent.status)))
    .with_last_task_message(agent.last_task_message.clone())
}

fn status_snapshot(status: &AgentStatus) -> String {
    match status {
        AgentStatus::PendingInit => "pending_init".to_owned(),
        AgentStatus::Running => "running".to_owned(),
        AgentStatus::Interrupted => "interrupted".to_owned(),
        AgentStatus::Shutdown => "shutdown".to_owned(),
        AgentStatus::NotFound => "not_found".to_owned(),
        AgentStatus::Completed(Some(summary)) => format!("completed:{summary}"),
        AgentStatus::Completed(None) => "completed".to_owned(),
        AgentStatus::Refused(message) => format!("refusal:{message}"),
        AgentStatus::Errored(message) => format!("errored:{message}"),
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AgentControlConfig, AgentSpawnRequest, LocalAgentGraphStore,
        agent_control_runtime::AgentControlRuntime,
    };

    use super::*;

    #[tokio::test]
    async fn graph_sync_spawn_and_close_roundtrip_controls_restore_visibility() {
        let store = LocalAgentGraphStore::new();
        let sync = AgentControlGraphSyncRuntime;
        let mut control = AgentControlRuntime::new(AgentControlConfig::default());
        let review = sync
            .spawn_with_graph_store(
                &mut control,
                &store,
                spawn_request(AgentPath::root(), "review"),
                10,
                "E:/zbg",
            )
            .await
            .unwrap()
            .control;
        sync.spawn_with_graph_store(
            &mut control,
            &store,
            spawn_request(review.path.clone(), "security"),
            11,
            "E:/zbg",
        )
        .await
        .unwrap();
        sync.spawn_with_graph_store(
            &mut control,
            &store,
            spawn_request(AgentPath::root(), "build"),
            12,
            "E:/zbg",
        )
        .await
        .unwrap();
        let build_node = store.get_thread_node(3).await.unwrap().unwrap();
        assert_eq!(build_node.agent_path.as_deref(), Some("/root/build"));
        assert_eq!(build_node.last_task_message.as_deref(), Some("处理 build"));

        let close = sync
            .close_with_graph_store(&mut control, &store, &review.path, 20)
            .await
            .unwrap();

        assert_eq!(close.control.closed_count, 2);
        assert_eq!(close.synced_edges, 1);
        let (restored, report) = AgentControlRuntime::restore_open_tree_from_store(
            &store,
            0,
            AgentControlConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(report.restored_count, 2);
        assert_eq!(
            restored
                .list_agents(&AgentPath::root(), None)
                .unwrap()
                .iter()
                .map(|agent| agent.agent_name.as_str())
                .collect::<Vec<_>>(),
            ["/root", "/root/build"]
        );
    }

    fn spawn_request(parent_path: AgentPath, task_name: &str) -> AgentSpawnRequest {
        AgentSpawnRequest {
            parent_path,
            task_name: task_name.to_owned(),
            message: format!("处理 {task_name}"),
            nickname: None,
            role: None,
        }
    }
}
