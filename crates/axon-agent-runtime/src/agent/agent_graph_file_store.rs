//! 文件持久化 Agent Graph Store。
//!
//! 该实现用于 CLI/TUI 本地运行态，把 thread spawn 拓扑写入一个 JSON 文件，
//! 让 root session 重启后仍可恢复 open 子代理树。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use axon_domain::{AxonError, AxonResult, ThreadSpawnEdge, ThreadSpawnEdgeStatus, ThreadSpawnNode};
use serde::{Deserialize, Serialize};

use crate::{AgentGraphStore, LocalAgentGraphStore};

const AGENT_GRAPH_FILE_REASON_IO: &str = "agent_graph_file:io";

/// 文件持久化 AgentGraphStore。
#[derive(Debug, Clone)]
pub struct FileAgentGraphStore {
    path: PathBuf,
    inner: LocalAgentGraphStore,
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl FileAgentGraphStore {
    /// 打开或创建一个文件持久化 graph store。
    pub fn open(path: impl AsRef<Path>) -> AxonResult<Self> {
        let path = path.as_ref().to_path_buf();
        let store = Self {
            path,
            inner: LocalAgentGraphStore::new(),
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        };
        store.load_from_disk()?;
        Ok(store)
    }

    fn load_from_disk(&self) -> AxonResult<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))?;
        if content.trim().is_empty() {
            return Ok(());
        }
        let snapshot: AgentGraphFileSnapshot = serde_json::from_str(&content)
            .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))?;
        self.load_snapshot(snapshot)
    }

    fn load_snapshot(&self, snapshot: AgentGraphFileSnapshot) -> AxonResult<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))?
            .block_on(async {
                for node in snapshot.nodes {
                    self.inner.create_thread_node(node.into_domain()).await?;
                }
                for edge in snapshot.edges {
                    self.inner
                        .upsert_thread_spawn_edge(edge.parent_id, edge.child_id, edge.created_at_ms)
                        .await?;
                    if edge.status() == ThreadSpawnEdgeStatus::Closed {
                        self.inner
                            .update_edge_status(
                                edge.parent_id,
                                edge.child_id,
                                ThreadSpawnEdgeStatus::Closed,
                                edge.closed_at_ms,
                            )
                            .await?;
                    }
                }
                Ok(())
            })
    }

    async fn persist(&self) -> AxonResult<()> {
        let snapshot = AgentGraphFileSnapshot {
            nodes: self
                .inner
                .list_all_threads()
                .await?
                .into_iter()
                .map(ThreadSpawnNodeRecord::from)
                .collect(),
            edges: self
                .inner
                .list_all_edges()
                .await?
                .into_iter()
                .map(ThreadSpawnEdgeRecord::from)
                .collect(),
        };
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))?;
        }
        let content = serde_json::to_string_pretty(&snapshot)
            .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))?;
        fs::write(&self.path, content)
            .map_err(|_| AxonError::runtime_failure(AGENT_GRAPH_FILE_REASON_IO))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentGraphFileSnapshot {
    nodes: Vec<ThreadSpawnNodeRecord>,
    edges: Vec<ThreadSpawnEdgeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadSpawnNodeRecord {
    id: u64,
    nickname: String,
    agent_path: Option<String>,
    cwd: String,
    depth: u32,
    parent_id: Option<u64>,
    is_root: bool,
    created_at_ms: u64,
    last_task_message: Option<String>,
    status_snapshot: Option<String>,
    ended_at_ms: Option<u64>,
}

impl ThreadSpawnNodeRecord {
    fn into_domain(self) -> ThreadSpawnNode {
        let mut node = ThreadSpawnNode::new(
            self.id,
            self.nickname,
            self.cwd,
            self.depth,
            self.parent_id,
            self.created_at_ms,
        );
        node.agent_path = self.agent_path;
        node.last_task_message = self.last_task_message;
        node.status_snapshot = self.status_snapshot;
        node.ended_at_ms = self.ended_at_ms;
        node.is_root = self.is_root;
        node
    }
}

impl From<ThreadSpawnNode> for ThreadSpawnNodeRecord {
    fn from(node: ThreadSpawnNode) -> Self {
        Self {
            id: node.id,
            nickname: node.nickname,
            agent_path: node.agent_path,
            cwd: node.cwd,
            depth: node.depth,
            parent_id: node.parent_id,
            is_root: node.is_root,
            created_at_ms: node.created_at_ms,
            last_task_message: node.last_task_message,
            status_snapshot: node.status_snapshot,
            ended_at_ms: node.ended_at_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadSpawnEdgeRecord {
    parent_id: u64,
    child_id: u64,
    status: String,
    created_at_ms: u64,
    closed_at_ms: Option<u64>,
}

impl From<ThreadSpawnEdge> for ThreadSpawnEdgeRecord {
    fn from(edge: ThreadSpawnEdge) -> Self {
        Self {
            parent_id: edge.parent_id,
            child_id: edge.child_id,
            status: edge_status_name(edge.status).to_owned(),
            created_at_ms: edge.created_at_ms,
            closed_at_ms: edge.closed_at_ms,
        }
    }
}

impl ThreadSpawnEdgeRecord {
    fn status(&self) -> ThreadSpawnEdgeStatus {
        if self.status == "closed" {
            ThreadSpawnEdgeStatus::Closed
        } else {
            ThreadSpawnEdgeStatus::Open
        }
    }
}

fn edge_status_name(status: ThreadSpawnEdgeStatus) -> &'static str {
    match status {
        ThreadSpawnEdgeStatus::Open => "open",
        ThreadSpawnEdgeStatus::Closed => "closed",
    }
}

#[async_trait]
impl AgentGraphStore for FileAgentGraphStore {
    async fn upsert_thread_spawn_edge(
        &self,
        parent_id: u64,
        child_id: u64,
        created_at_ms: u64,
    ) -> AxonResult<()> {
        let _guard = self.write_lock.lock().await;
        self.inner
            .upsert_thread_spawn_edge(parent_id, child_id, created_at_ms)
            .await?;
        self.persist().await
    }

    async fn update_edge_status(
        &self,
        parent_id: u64,
        child_id: u64,
        status: ThreadSpawnEdgeStatus,
        closed_at_ms: Option<u64>,
    ) -> AxonResult<()> {
        let _guard = self.write_lock.lock().await;
        self.inner
            .update_edge_status(parent_id, child_id, status, closed_at_ms)
            .await?;
        self.persist().await
    }

    async fn list_thread_spawn_children(&self, parent_id: u64) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.inner.list_thread_spawn_children(parent_id).await
    }

    async fn list_thread_spawn_children_with_status(
        &self,
        parent_id: u64,
        status: ThreadSpawnEdgeStatus,
    ) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.inner
            .list_thread_spawn_children_with_status(parent_id, status)
            .await
    }

    async fn find_thread_spawn_child_by_path(
        &self,
        parent_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        self.inner
            .find_thread_spawn_child_by_path(parent_id, agent_path)
            .await
    }

    async fn list_thread_spawn_descendants(
        &self,
        parent_id: u64,
    ) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.inner.list_thread_spawn_descendants(parent_id).await
    }

    async fn find_thread_spawn_descendant_by_path(
        &self,
        root_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        self.inner
            .find_thread_spawn_descendant_by_path(root_id, agent_path)
            .await
    }

    async fn find_thread_spawn_node_by_path(
        &self,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        self.inner.find_thread_spawn_node_by_path(agent_path).await
    }

    async fn get_thread_parent(&self, child_id: u64) -> AxonResult<Option<ThreadSpawnNode>> {
        self.inner.get_thread_parent(child_id).await
    }

    async fn get_thread_node(&self, thread_id: u64) -> AxonResult<Option<ThreadSpawnNode>> {
        self.inner.get_thread_node(thread_id).await
    }

    async fn create_thread_node(&self, node: ThreadSpawnNode) -> AxonResult<()> {
        let _guard = self.write_lock.lock().await;
        self.inner.create_thread_node(node).await?;
        self.persist().await
    }

    async fn update_thread_node(
        &self,
        thread_id: u64,
        ended_at_ms: Option<u64>,
        status_snapshot: Option<String>,
        last_task_message: Option<String>,
    ) -> AxonResult<()> {
        let _guard = self.write_lock.lock().await;
        self.inner
            .update_thread_node(thread_id, ended_at_ms, status_snapshot, last_task_message)
            .await?;
        self.persist().await
    }

    async fn list_open_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>> {
        self.inner.list_open_edges().await
    }

    async fn list_open_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.inner.list_open_threads().await
    }

    async fn list_all_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>> {
        self.inner.list_all_edges().await
    }

    async fn list_all_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.inner.list_all_threads().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph_path() -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!(
                "axon-subagent-graph-{}-{nonce}",
                std::process::id()
            ))
            .join("subagent-graph.json")
    }

    #[test]
    fn file_graph_store_roundtrips_open_tree_after_reopen() {
        let path = graph_path();
        let now = 100;
        let store = FileAgentGraphStore::open(&path).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            for node in [
                ThreadSpawnNode::new(1, "root".into(), "E:/zbg".into(), 0, None, now)
                    .with_agent_path("/root"),
                ThreadSpawnNode::new(2, "review".into(), "E:/zbg".into(), 1, Some(1), now)
                    .with_agent_path("/root/review")
                    .with_last_task_message(Some("审查当前改动".to_owned())),
                ThreadSpawnNode::new(3, "security".into(), "E:/zbg".into(), 2, Some(2), now)
                    .with_agent_path("/root/review/security"),
                ThreadSpawnNode::new(4, "build".into(), "E:/zbg".into(), 1, Some(1), now)
                    .with_agent_path("/root/build"),
            ] {
                store.create_thread_node(node).await.unwrap();
            }
            store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
            store.upsert_thread_spawn_edge(2, 3, now).await.unwrap();
            store.upsert_thread_spawn_edge(1, 4, now).await.unwrap();
            store
                .update_edge_status(1, 4, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
                .await
                .unwrap();
        });

        let reopened = FileAgentGraphStore::open(&path).unwrap();
        let (runtime, report) = rt
            .block_on(crate::AgentControlRuntime::restore_open_tree_from_store(
                &reopened,
                1,
                crate::AgentControlConfig::default(),
            ))
            .unwrap();

        assert_eq!(report.restored_count, 3);
        let agents = runtime
            .list_agents(&crate::AgentPath::root(), None)
            .unwrap();
        assert_eq!(
            agents
                .iter()
                .map(|agent| agent.agent_name.as_str())
                .collect::<Vec<_>>(),
            ["/root", "/root/review", "/root/review/security"]
        );
        assert_eq!(
            runtime.status(&crate::AgentPath::parse("/root/build").unwrap()),
            crate::AgentStatus::NotFound
        );
        let _ = fs::remove_dir_all(path.parent().unwrap());
    }
}
