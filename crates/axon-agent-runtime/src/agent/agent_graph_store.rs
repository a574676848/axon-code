//! Agent Graph Store - 多智能体并行执行层的持久化拓扑层。
//!
//! 参考 codex-rs `agent-graph-store` 模块设计，实现线程 spawn 拓扑的存储抽象。
//! 支持父子线程关系查询（直接子节点、后裔节点）以及边的状态管理。

use async_trait::async_trait;
use axon_domain::{AxonResult, ThreadSpawnEdge, ThreadSpawnEdgeStatus, ThreadSpawnNode};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

const AGENT_GRAPH_REASON_DUPLICATE_AGENT_PATH: &str = "agent_graph:duplicate_agent_path";

/// AgentGraphStore 存储抽象 trait。
///
/// 定义线程 spawn 拓扑的持久化操作：
/// - 边的增删改查
/// - 节点的生命周期管理
/// - 拓扑查询（子节点、后裔节点）
#[async_trait]
pub trait AgentGraphStore: Send + Sync {
    /// Upsert 一条 thread spawn 边。
    async fn upsert_thread_spawn_edge(
        &self,
        parent_id: u64,
        child_id: u64,
        created_at_ms: u64,
    ) -> AxonResult<()>;

    /// 更新边的状态。
    async fn update_edge_status(
        &self,
        parent_id: u64,
        child_id: u64,
        status: ThreadSpawnEdgeStatus,
        closed_at_ms: Option<u64>,
    ) -> AxonResult<()>;

    /// 列出某线程的直接子节点，按 thread id 稳定排序。
    async fn list_thread_spawn_children(&self, parent_id: u64) -> AxonResult<Vec<ThreadSpawnNode>>;

    /// 按边状态列出某线程的直接子节点，按 thread id 稳定排序。
    async fn list_thread_spawn_children_with_status(
        &self,
        parent_id: u64,
        status: ThreadSpawnEdgeStatus,
    ) -> AxonResult<Vec<ThreadSpawnNode>>;

    /// 按规范代理路径查找某线程的直接子节点。
    async fn find_thread_spawn_child_by_path(
        &self,
        parent_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>>;

    /// 列出某线程的所有后裔节点（递归），按 BFS 层级、同层 thread id 稳定排序。
    async fn list_thread_spawn_descendants(
        &self,
        parent_id: u64,
    ) -> AxonResult<Vec<ThreadSpawnNode>>;

    /// 按规范代理路径查找某线程的 open 子树后裔节点。
    async fn find_thread_spawn_descendant_by_path(
        &self,
        root_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>>;

    /// 按规范代理路径查找任意已持久化节点，不受边状态限制。
    async fn find_thread_spawn_node_by_path(
        &self,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>>;

    /// 获取某线程的直接父节点。
    async fn get_thread_parent(&self, child_id: u64) -> AxonResult<Option<ThreadSpawnNode>>;

    /// 获取节点。
    async fn get_thread_node(&self, thread_id: u64) -> AxonResult<Option<ThreadSpawnNode>>;

    /// 创建线程节点。
    async fn create_thread_node(&self, node: ThreadSpawnNode) -> AxonResult<()>;

    /// 更新线程节点（主要用于标记结束时间与状态快照）。
    async fn update_thread_node(
        &self,
        thread_id: u64,
        ended_at_ms: Option<u64>,
        status_snapshot: Option<String>,
        last_task_message: Option<String>,
    ) -> AxonResult<()>;

    /// 列出所有活跃的（未关闭的）边。
    async fn list_open_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>>;

    /// 列出所有活跃的线程节点。
    async fn list_open_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>>;

    /// 列出所有边，供持久化快照使用。
    async fn list_all_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>>;

    /// 列出所有线程节点，供持久化快照使用。
    async fn list_all_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>>;
}

/// 内存版 AgentGraphStore 实现（无持久化）。
///
/// 适用于单进程场景或测试环境。生产环境可实现 SQLite/StateRuntime 后端。
#[derive(Debug, Clone)]
pub struct LocalAgentGraphStore {
    edges: Arc<RwLock<HashMap<(u64, u64), ThreadSpawnEdge>>>,
    nodes: Arc<RwLock<HashMap<u64, ThreadSpawnNode>>>,
    child_index: Arc<RwLock<HashMap<u64, Vec<u64>>>>,
}

impl Default for LocalAgentGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalAgentGraphStore {
    /// 创建新实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            edges: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            child_index: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 获取读锁。
    async fn read_edges(&self) -> RwLockReadGuard<'_, HashMap<(u64, u64), ThreadSpawnEdge>> {
        self.edges.read().await
    }

    /// 获取写锁。
    async fn write_edges(&self) -> RwLockWriteGuard<'_, HashMap<(u64, u64), ThreadSpawnEdge>> {
        self.edges.write().await
    }

    /// 获取读锁（节点）。
    async fn read_nodes(&self) -> RwLockReadGuard<'_, HashMap<u64, ThreadSpawnNode>> {
        self.nodes.read().await
    }

    /// 获取写锁（节点）。
    async fn write_nodes(&self) -> RwLockWriteGuard<'_, HashMap<u64, ThreadSpawnNode>> {
        self.nodes.write().await
    }

    /// 获取写锁（子节点索引）。
    async fn write_child_index(&self) -> RwLockWriteGuard<'_, HashMap<u64, Vec<u64>>> {
        self.child_index.write().await
    }

    /// 按边状态收集所有后裔节点 ID（递归）。
    fn collect_descendants_with_status(
        edges: &HashMap<(u64, u64), ThreadSpawnEdge>,
        child_index: &HashMap<u64, Vec<u64>>,
        node_id: u64,
        status: ThreadSpawnEdgeStatus,
    ) -> Vec<u64> {
        let mut descendants = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::from([node_id]);

        while let Some(current) = queue.pop_front() {
            if let Some(children) = child_index.get(&current) {
                let mut ordered_children = children.clone();
                ordered_children.sort_unstable();
                for child_id in ordered_children {
                    let edge_is_expected_status = edges
                        .get(&(current, child_id))
                        .is_some_and(|edge| edge.status == status);
                    if !edge_is_expected_status || !visited.insert(child_id) {
                        continue;
                    }
                    descendants.push(child_id);
                    queue.push_back(child_id);
                }
            }
        }
        descendants
    }

    fn one_node_by_path<I>(
        nodes: &HashMap<u64, ThreadSpawnNode>,
        ids: I,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>>
    where
        I: IntoIterator<Item = u64>,
    {
        let mut matches = ids
            .into_iter()
            .filter_map(|id| nodes.get(&id))
            .filter(|node| node.agent_path.as_deref() == Some(agent_path))
            .take(2)
            .cloned()
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(axon_domain::AxonError::runtime_invalid_state(
                AGENT_GRAPH_REASON_DUPLICATE_AGENT_PATH,
            ));
        }
        Ok(matches.pop())
    }
}

#[async_trait]
impl AgentGraphStore for LocalAgentGraphStore {
    async fn upsert_thread_spawn_edge(
        &self,
        parent_id: u64,
        child_id: u64,
        created_at_ms: u64,
    ) -> AxonResult<()> {
        let edge = ThreadSpawnEdge::new(parent_id, child_id, created_at_ms);
        let mut edges = self.write_edges().await;
        let is_new_edge = edges.insert((parent_id, child_id), edge).is_none();

        let mut child_idx = self.write_child_index().await;
        let children = child_idx.entry(parent_id).or_insert_with(Vec::new);
        if is_new_edge && !children.contains(&child_id) {
            children.push(child_id);
        }

        Ok(())
    }

    async fn update_edge_status(
        &self,
        parent_id: u64,
        child_id: u64,
        status: ThreadSpawnEdgeStatus,
        closed_at_ms: Option<u64>,
    ) -> AxonResult<()> {
        let mut edges = self.write_edges().await;
        if let Some(edge) = edges.get_mut(&(parent_id, child_id)) {
            if status == ThreadSpawnEdgeStatus::Closed {
                edge.close(closed_at_ms.unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64
                }));
            } else {
                edge.status = status;
                edge.closed_at_ms = None;
            }
        }
        Ok(())
    }

    async fn list_thread_spawn_children(&self, parent_id: u64) -> AxonResult<Vec<ThreadSpawnNode>> {
        self.list_thread_spawn_children_with_status(parent_id, ThreadSpawnEdgeStatus::Open)
            .await
    }

    async fn list_thread_spawn_children_with_status(
        &self,
        parent_id: u64,
        status: ThreadSpawnEdgeStatus,
    ) -> AxonResult<Vec<ThreadSpawnNode>> {
        let edges = self.read_edges().await;
        let nodes = self.read_nodes().await;
        let child_index = self.child_index.read().await;

        let mut child_ids = child_index.get(&parent_id).cloned().unwrap_or_default();
        child_ids.sort_unstable();
        let children: Vec<ThreadSpawnNode> = child_ids
            .iter()
            .filter(|&&id| {
                edges
                    .get(&(parent_id, id))
                    .is_some_and(|edge| edge.status == status)
            })
            .filter_map(|&id| nodes.get(&id).cloned())
            .collect();
        Ok(children)
    }

    async fn find_thread_spawn_child_by_path(
        &self,
        parent_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        let edges = self.read_edges().await;
        let nodes = self.read_nodes().await;
        let child_index = self.child_index.read().await;
        let child_ids = child_index
            .get(&parent_id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|child_id| {
                edges
                    .get(&(parent_id, *child_id))
                    .is_some_and(|edge| edge.status == ThreadSpawnEdgeStatus::Open)
            });
        Self::one_node_by_path(&nodes, child_ids, agent_path)
    }

    async fn list_thread_spawn_descendants(
        &self,
        parent_id: u64,
    ) -> AxonResult<Vec<ThreadSpawnNode>> {
        let edges = self.read_edges().await;
        let nodes = self.read_nodes().await;
        let child_idx = self.child_index.read().await;

        let descendant_ids = Self::collect_descendants_with_status(
            &edges,
            &child_idx,
            parent_id,
            ThreadSpawnEdgeStatus::Open,
        );
        let descendants: Vec<ThreadSpawnNode> = descendant_ids
            .iter()
            .filter_map(|&id| nodes.get(&id).cloned())
            .collect();
        Ok(descendants)
    }

    async fn find_thread_spawn_descendant_by_path(
        &self,
        root_id: u64,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        let edges = self.read_edges().await;
        let nodes = self.read_nodes().await;
        let child_idx = self.child_index.read().await;
        let descendant_ids = Self::collect_descendants_with_status(
            &edges,
            &child_idx,
            root_id,
            ThreadSpawnEdgeStatus::Open,
        );
        Self::one_node_by_path(&nodes, descendant_ids, agent_path)
    }

    async fn find_thread_spawn_node_by_path(
        &self,
        agent_path: &str,
    ) -> AxonResult<Option<ThreadSpawnNode>> {
        let nodes = self.read_nodes().await;
        Self::one_node_by_path(&nodes, nodes.keys().copied(), agent_path)
    }

    async fn get_thread_parent(&self, child_id: u64) -> AxonResult<Option<ThreadSpawnNode>> {
        let nodes = self.read_nodes().await;
        let parent_id = nodes.get(&child_id).and_then(|n| n.parent_id);

        if let Some(pid) = parent_id {
            Ok(nodes.get(&pid).cloned())
        } else {
            Ok(None)
        }
    }

    async fn get_thread_node(&self, thread_id: u64) -> AxonResult<Option<ThreadSpawnNode>> {
        let nodes = self.read_nodes().await;
        Ok(nodes.get(&thread_id).cloned())
    }

    async fn create_thread_node(&self, node: ThreadSpawnNode) -> AxonResult<()> {
        let mut nodes = self.write_nodes().await;
        nodes.insert(node.id, node);
        Ok(())
    }

    async fn update_thread_node(
        &self,
        thread_id: u64,
        ended_at_ms: Option<u64>,
        status_snapshot: Option<String>,
        last_task_message: Option<String>,
    ) -> AxonResult<()> {
        let mut nodes = self.write_nodes().await;
        if let Some(node) = nodes.get_mut(&thread_id) {
            if let Some(ts) = ended_at_ms {
                node.end(ts);
            }
            if status_snapshot.is_some() {
                node.status_snapshot = status_snapshot;
            }
            if last_task_message.is_some() {
                node.last_task_message = last_task_message;
            }
        }
        Ok(())
    }

    async fn list_open_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>> {
        let edges = self.read_edges().await;
        let open: Vec<ThreadSpawnEdge> = edges
            .values()
            .filter(|e| e.status.is_open())
            .cloned()
            .collect();
        Ok(open)
    }

    async fn list_open_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>> {
        let nodes = self.read_nodes().await;
        let open: Vec<ThreadSpawnNode> = nodes
            .values()
            .filter(|n| n.ended_at_ms.is_none())
            .cloned()
            .collect();
        Ok(open)
    }

    async fn list_all_edges(&self) -> AxonResult<Vec<ThreadSpawnEdge>> {
        let edges = self.read_edges().await;
        Ok(edges.values().cloned().collect())
    }

    async fn list_all_threads(&self) -> AxonResult<Vec<ThreadSpawnNode>> {
        let nodes = self.read_nodes().await;
        Ok(nodes.values().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[tokio::test]
    async fn test_upsert_edge_and_list_children() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);

        store.create_thread_node(root.clone()).await.unwrap();
        store.create_thread_node(child.clone()).await.unwrap();
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();

        let children = store.list_thread_spawn_children(1).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, 2);
    }

    #[tokio::test]
    async fn test_list_descendants() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child1 = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);
        let child2 = ThreadSpawnNode::new(3, "child-1".into(), "/tmp".into(), 1, Some(1), now);
        let grandchild =
            ThreadSpawnNode::new(4, "grandchild".into(), "/tmp".into(), 2, Some(2), now);

        for node in [root, child1, child2, grandchild] {
            store.create_thread_node(node).await.unwrap();
        }
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store.upsert_thread_spawn_edge(1, 3, now).await.unwrap();
        store.upsert_thread_spawn_edge(2, 4, now).await.unwrap();

        let descendants = store.list_thread_spawn_descendants(1).await.unwrap();
        assert_eq!(descendants.len(), 3);
        let ids: Vec<u64> = descendants.iter().map(|n| n.id).collect();
        assert!(ids.contains(&2));
        assert!(ids.contains(&3));
        assert!(ids.contains(&4));
    }

    #[tokio::test]
    async fn children_and_descendants_use_stable_thread_id_order() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let nodes = [
            ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now),
            ThreadSpawnNode::new(4, "child-4".into(), "/tmp".into(), 1, Some(1), now),
            ThreadSpawnNode::new(2, "child-2".into(), "/tmp".into(), 1, Some(1), now),
            ThreadSpawnNode::new(3, "child-3".into(), "/tmp".into(), 1, Some(1), now),
            ThreadSpawnNode::new(7, "grandchild-7".into(), "/tmp".into(), 2, Some(2), now),
            ThreadSpawnNode::new(5, "grandchild-5".into(), "/tmp".into(), 2, Some(2), now),
            ThreadSpawnNode::new(6, "grandchild-6".into(), "/tmp".into(), 2, Some(3), now),
        ];
        for node in nodes {
            store.create_thread_node(node).await.unwrap();
        }
        for (parent_id, child_id) in [(1, 4), (1, 2), (1, 3), (2, 7), (2, 5), (3, 6)] {
            store
                .upsert_thread_spawn_edge(parent_id, child_id, now)
                .await
                .unwrap();
        }

        let children = store.list_thread_spawn_children(1).await.unwrap();
        let descendants = store.list_thread_spawn_descendants(1).await.unwrap();

        assert_eq!(
            children.iter().map(|node| node.id).collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(
            descendants.iter().map(|node| node.id).collect::<Vec<_>>(),
            [2, 3, 4, 5, 7, 6]
        );
    }

    #[tokio::test]
    async fn test_close_edge() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);

        store.create_thread_node(root).await.unwrap();
        store.create_thread_node(child).await.unwrap();
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();

        let open_before = store.list_open_edges().await.unwrap();
        assert_eq!(open_before.len(), 1);

        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(timestamp()))
            .await
            .unwrap();

        let open_after = store.list_open_edges().await.unwrap();
        assert!(open_after.is_empty());
    }

    #[tokio::test]
    async fn closed_edge_is_excluded_from_default_children_and_descendants() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);

        store.create_thread_node(root).await.unwrap();
        store.create_thread_node(child).await.unwrap();
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
            .await
            .unwrap();

        let children = store.list_thread_spawn_children(1).await.unwrap();
        let descendants = store.list_thread_spawn_descendants(1).await.unwrap();
        let closed_children = store
            .list_thread_spawn_children_with_status(1, ThreadSpawnEdgeStatus::Closed)
            .await
            .unwrap();

        assert!(children.is_empty());
        assert!(descendants.is_empty());
        assert_eq!(closed_children.len(), 1);
        assert_eq!(closed_children[0].id, 2);
    }

    #[tokio::test]
    async fn closed_intermediate_edge_blocks_open_grandchild_traversal() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);
        let grandchild =
            ThreadSpawnNode::new(3, "grandchild".into(), "/tmp".into(), 2, Some(2), now);

        for node in [root, child, grandchild] {
            store.create_thread_node(node).await.unwrap();
        }
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store.upsert_thread_spawn_edge(2, 3, now).await.unwrap();
        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
            .await
            .unwrap();

        let root_descendants = store.list_thread_spawn_descendants(1).await.unwrap();
        let child_descendants = store.list_thread_spawn_descendants(2).await.unwrap();

        assert!(root_descendants.is_empty());
        assert_eq!(child_descendants.len(), 1);
        assert_eq!(child_descendants[0].id, 3);
    }

    #[tokio::test]
    async fn upsert_same_edge_reopens_without_duplicate_children() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now);
        let child = ThreadSpawnNode::new(2, "child-0".into(), "/tmp".into(), 1, Some(1), now);

        store.create_thread_node(root).await.unwrap();
        store.create_thread_node(child).await.unwrap();
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
            .await
            .unwrap();
        store.upsert_thread_spawn_edge(1, 2, now + 2).await.unwrap();

        let children = store.list_thread_spawn_children(1).await.unwrap();
        let descendants = store.list_thread_spawn_descendants(1).await.unwrap();

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, 2);
        assert_eq!(descendants.len(), 1);
        assert_eq!(descendants[0].id, 2);
    }

    #[tokio::test]
    async fn canonical_path_lookup_respects_direct_child_and_open_subtree() {
        let store = LocalAgentGraphStore::new();
        let now = timestamp();

        let root = ThreadSpawnNode::new(1, "root".into(), "/tmp".into(), 0, None, now)
            .with_agent_path("/root");
        let review = ThreadSpawnNode::new(2, "review".into(), "/tmp".into(), 1, Some(1), now)
            .with_agent_path("/root/review");
        let security = ThreadSpawnNode::new(3, "security".into(), "/tmp".into(), 2, Some(2), now)
            .with_agent_path("/root/review/security");
        let duplicate = ThreadSpawnNode::new(4, "duplicate".into(), "/tmp".into(), 1, Some(1), now)
            .with_agent_path("/root/review");

        for node in [root, review, security, duplicate] {
            store.create_thread_node(node).await.unwrap();
        }
        store.upsert_thread_spawn_edge(1, 2, now).await.unwrap();
        store.upsert_thread_spawn_edge(2, 3, now).await.unwrap();

        let direct = store
            .find_thread_spawn_child_by_path(1, "/root/review")
            .await
            .unwrap();
        let descendant = store
            .find_thread_spawn_descendant_by_path(1, "/root/review/security")
            .await
            .unwrap();

        assert_eq!(direct.map(|node| node.id), Some(2));
        assert_eq!(descendant.map(|node| node.id), Some(3));

        store
            .update_edge_status(1, 2, ThreadSpawnEdgeStatus::Closed, Some(now + 1))
            .await
            .unwrap();
        assert!(
            store
                .find_thread_spawn_child_by_path(1, "/root/review")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .find_thread_spawn_descendant_by_path(1, "/root/review/security")
                .await
                .unwrap()
                .is_none()
        );

        store.upsert_thread_spawn_edge(1, 2, now + 2).await.unwrap();
        store.upsert_thread_spawn_edge(1, 4, now + 3).await.unwrap();
        assert!(matches!(
            store
                .find_thread_spawn_child_by_path(1, "/root/review")
                .await,
            Err(axon_domain::AxonError::Runtime(
                axon_domain::RuntimeError::InvalidState(reason),
            )) if reason == AGENT_GRAPH_REASON_DUPLICATE_AGENT_PATH
        ));
    }
}
