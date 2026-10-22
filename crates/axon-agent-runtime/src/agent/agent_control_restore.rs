//! 子代理控制面的持久化拓扑恢复。
//!
//! 对齐 Codex MultiAgentV2 的 thread-spawn edge 语义：恢复时只沿 Open 边
//! 重建可见 agent tree，Closed 边及其子树不自动复活。

use std::collections::VecDeque;

use axon_domain::{AxonError, AxonResult, ThreadSpawnNode};

use crate::agent_communication::AgentStatus;
use crate::agent_control_runtime::{
    AGENT_CONTROL_REASON_INVALID_PATH, AgentControlConfig, AgentControlRuntime, AgentPath,
    ControlledAgent,
};
use crate::agent_graph_store::AgentGraphStore;

/// 子代理控制面恢复报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentControlRestoreReport {
    /// 根线程 ID。
    pub root_thread_id: u64,
    /// 恢复的 agent 数量，包含 root。
    pub restored_count: usize,
    /// 被跳过的节点数量。
    pub skipped_count: usize,
}

impl AgentControlRuntime {
    /// 从持久化 graph store 恢复 root 及其 Open descendants。
    pub async fn restore_open_tree_from_store(
        store: &dyn AgentGraphStore,
        root_thread_id: u64,
        config: AgentControlConfig,
    ) -> AxonResult<(Self, AgentControlRestoreReport)> {
        let root =
            store
                .get_thread_node(root_thread_id)
                .await?
                .ok_or(AxonError::runtime_failure(
                    AGENT_CONTROL_REASON_INVALID_PATH,
                ))?;
        let mut runtime = Self::new(config);
        runtime.replace_root(root)?;

        let mut queue = VecDeque::from([(root_thread_id, AgentPath::root())]);
        let mut restored_count = 1;
        let mut skipped_count = 0;
        while let Some((parent_id, parent_path)) = queue.pop_front() {
            for child in store.list_thread_spawn_children(parent_id).await? {
                match runtime.restore_child(&parent_path, child) {
                    Ok(child_path) => {
                        restored_count += 1;
                        queue.push_back((runtime.status_thread_id(&child_path)?, child_path));
                    }
                    Err(_) => skipped_count += 1,
                }
            }
        }

        Ok((
            runtime,
            AgentControlRestoreReport {
                root_thread_id,
                restored_count,
                skipped_count,
            },
        ))
    }

    fn replace_root(&mut self, node: ThreadSpawnNode) -> AxonResult<()> {
        if !node.is_root {
            return Err(AxonError::runtime_failure(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        let root = AgentPath::root();
        if let Some(agent) = self.agents.get_mut(&root) {
            let nickname = node.nickname;
            self.used_nicknames.insert(nickname.clone());
            agent.thread_id = node.id;
            agent.nickname = Some(nickname);
            agent.status = AgentStatus::Running;
            agent.last_task_message = Some(
                node.last_task_message
                    .unwrap_or_else(|| "Restored root thread".to_owned()),
            );
        }
        self.next_thread_id = self.next_thread_id.max(node.id.saturating_add(1));
        Ok(())
    }

    fn restore_child(
        &mut self,
        parent_path: &AgentPath,
        node: ThreadSpawnNode,
    ) -> AxonResult<AgentPath> {
        let path = restore_path(parent_path, &node)?;
        self.validate_restore_path(&path)?;
        let status = restore_status(&node);
        self.used_nicknames.insert(node.nickname.clone());
        let agent = ControlledAgent {
            thread_id: node.id,
            path: path.clone(),
            nickname: Some(node.nickname),
            role: None,
            status,
            last_task_message: Some(
                node.last_task_message
                    .unwrap_or_else(|| "Restored sub-agent thread".to_owned()),
            ),
        };
        self.agents.insert(path.clone(), agent);
        self.upsert_open_edge(parent_path.clone(), path.clone());
        self.mailboxes.ensure_agent(path.clone());
        self.next_thread_id = self.next_thread_id.max(node.id.saturating_add(1));
        Ok(path)
    }

    fn status_thread_id(&self, path: &AgentPath) -> AxonResult<u64> {
        self.agents
            .get(path)
            .map(|agent| agent.thread_id)
            .ok_or(AxonError::runtime_failure(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ))
    }

    fn validate_restore_path(&self, path: &AgentPath) -> AxonResult<()> {
        if self.agents.contains_key(path) || path.depth() > self.config.max_depth {
            return Err(AxonError::runtime_failure(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        Ok(())
    }
}

fn restore_status(node: &ThreadSpawnNode) -> AgentStatus {
    match node.status_snapshot.as_deref() {
        Some("pending_init") => AgentStatus::PendingInit,
        Some("running") => AgentStatus::Running,
        Some("interrupted") => AgentStatus::Interrupted,
        Some("shutdown") => AgentStatus::Shutdown,
        Some("not_found") => AgentStatus::NotFound,
        Some("completed") => AgentStatus::Completed(None),
        Some(value) if value.starts_with("completed:") => {
            AgentStatus::Completed(Some(value.trim_start_matches("completed:").to_owned()))
        }
        Some(value) if value.starts_with("errored:") => {
            AgentStatus::Errored(value.trim_start_matches("errored:").to_owned())
        }
        _ if node.ended_at_ms.is_some() => AgentStatus::Shutdown,
        _ => AgentStatus::Running,
    }
}

fn restore_path(parent_path: &AgentPath, node: &ThreadSpawnNode) -> AxonResult<AgentPath> {
    let Some(agent_path) = node.agent_path.as_deref() else {
        return parent_path.join(&node.nickname);
    };
    let path = AgentPath::parse(agent_path)?;
    if path.parent().as_ref() != Some(parent_path) {
        return Err(AxonError::runtime_failure(
            AGENT_CONTROL_REASON_INVALID_PATH,
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use axon_domain::{ThreadSpawnEdgeStatus, ThreadSpawnNode};

    use crate::{AgentGraphStore, LocalAgentGraphStore};

    use super::*;

    #[tokio::test]
    async fn restore_open_tree_uses_canonical_path_and_skips_closed_descendants() {
        let store = LocalAgentGraphStore::new();
        let now = 10;
        for node in [
            ThreadSpawnNode::new(1, "root".to_owned(), "E:/zbg".to_owned(), 0, None, now)
                .with_agent_path("/root")
                .with_last_task_message(Some("根线程恢复".to_owned())),
            ThreadSpawnNode::new(
                2,
                "stale_review".to_owned(),
                "E:/zbg".to_owned(),
                1,
                Some(1),
                now,
            )
            .with_agent_path("/root/review")
            .with_last_task_message(Some("审查子代理恢复".to_owned())),
            ThreadSpawnNode::new(
                3,
                "security".to_owned(),
                "E:/zbg".to_owned(),
                2,
                Some(2),
                now,
            )
            .with_agent_path("/root/review/security"),
            ThreadSpawnNode::new(4, "build".to_owned(), "E:/zbg".to_owned(), 1, Some(1), now)
                .with_agent_path("/root/build")
                .with_last_task_message(Some("构建验证恢复".to_owned())),
        ] {
            store.create_thread_node(node).await.unwrap();
        }
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store.upsert_thread_spawn_edge(2, 3, now).await.unwrap();
        store.upsert_thread_spawn_edge(1, 4, now).await.unwrap();
        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
            .await
            .unwrap();

        let (runtime, report) = AgentControlRuntime::restore_open_tree_from_store(
            &store,
            1,
            AgentControlConfig::default(),
        )
        .await
        .unwrap();

        assert_eq!(report.restored_count, 2);
        let agents = runtime.list_agents(&AgentPath::root(), None).unwrap();
        assert_eq!(
            agents
                .iter()
                .map(|agent| agent.agent_name.as_str())
                .collect::<Vec<_>>(),
            ["/root", "/root/build"]
        );
        assert_eq!(
            agents
                .iter()
                .find(|agent| agent.agent_name == "/root/build")
                .and_then(|agent| agent.last_task_message.as_deref()),
            Some("构建验证恢复")
        );
        assert_eq!(
            runtime.status(&AgentPath::parse("/root/review").unwrap()),
            AgentStatus::NotFound
        );
        assert_eq!(
            runtime.status(&AgentPath::parse("/root/stale_review").unwrap()),
            AgentStatus::NotFound
        );
    }

    #[tokio::test]
    async fn restore_open_tree_preserves_completed_status_snapshot() {
        let store = LocalAgentGraphStore::new();
        let now = 20;
        for node in [
            ThreadSpawnNode::new(1, "root".to_owned(), "E:/zbg".to_owned(), 0, None, now)
                .with_agent_path("/root"),
            ThreadSpawnNode::new(2, "review".to_owned(), "E:/zbg".to_owned(), 1, Some(1), now)
                .with_agent_path("/root/review")
                .with_status_snapshot(Some("completed:审查完成".to_owned()))
                .with_last_task_message(Some("审查完成".to_owned())),
        ] {
            store.create_thread_node(node).await.unwrap();
        }
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();

        let (runtime, _) = AgentControlRuntime::restore_open_tree_from_store(
            &store,
            1,
            AgentControlConfig::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            runtime.status(&AgentPath::parse("/root/review").unwrap()),
            AgentStatus::Completed(Some("审查完成".to_owned()))
        );
    }
}
