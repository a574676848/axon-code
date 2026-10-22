//! Sub-agent 控制面运行时。
//!
//! 本模块对齐 Codex MultiAgentV2 的核心控制面思想：根会话共享一个控制面，
//! 由它统一管理 canonical path、spawn reservation、状态查询、列表与关闭。
//! 真实线程创建、fork history 与 agent 间通信由运行时适配层承载。
//! `agent_control_mailbox` 作为同一控制面的路径化 mailbox 承载。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use axon_domain::{AxonError, AxonResult};

use crate::agent_communication::AgentStatus;
use crate::agent_control_execution::AgentExecutionId;
use crate::agent_control_mailbox::AgentMailboxStore;
use crate::agent_control_reservation::{AgentControlReservations, AgentSpawnReservation};
use crate::agent_mailbox_persistence::MailboxPersistenceSlot;

/// 根代理规范路径。
pub const AGENT_PATH_ROOT: &str = "/root";
/// 默认最大子代理数量。
pub const AGENT_CONTROL_DEFAULT_MAX_AGENTS: usize = 16;
/// 默认最大同时运行的子代理 turn 数。
pub const AGENT_CONTROL_DEFAULT_MAX_ACTIVE_TURNS: usize = 16;
/// 默认最大嵌套深度。
pub const AGENT_CONTROL_DEFAULT_MAX_DEPTH: u32 = 3;
/// 自动生成昵称前缀。
pub const AGENT_CONTROL_DEFAULT_NICKNAME_PREFIX: &str = "agent";

#[path = "agent_control_reasons.rs"]
mod agent_control_reasons;
pub use agent_control_reasons::*;

/// 子代理规范路径。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentPath(String);

impl AgentPath {
    /// 返回根路径。
    #[must_use]
    pub fn root() -> Self {
        Self(AGENT_PATH_ROOT.to_owned())
    }

    /// 解析绝对路径。
    pub fn parse(value: impl AsRef<str>) -> AxonResult<Self> {
        let value = value.as_ref().trim();
        if value == AGENT_PATH_ROOT {
            return Ok(Self::root());
        }
        if !value.starts_with('/') {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        let mut parts = value.split('/').skip(1);
        if parts.next() != Some("root") {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        let tail = parts.collect::<Vec<_>>();
        if tail.is_empty() || tail.iter().any(|part| !valid_segment(part)) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        Ok(Self(format!("{AGENT_PATH_ROOT}/{}", tail.join("/"))))
    }

    /// 基于当前路径解析相对或绝对引用。
    pub fn resolve(&self, reference: impl AsRef<str>) -> AxonResult<Self> {
        let reference = reference.as_ref().trim();
        if reference.starts_with('/') {
            return Self::parse(reference);
        }
        if !valid_segment(reference) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        self.join(reference)
    }

    /// 拼接一个路径段。
    pub fn join(&self, segment: impl AsRef<str>) -> AxonResult<Self> {
        let segment = segment.as_ref().trim();
        if !valid_segment(segment) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        Ok(Self(format!("{}/{}", self.0, segment)))
    }

    /// 路径字符串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 是否根路径。
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0 == AGENT_PATH_ROOT
    }

    /// 路径深度，root 为 0。
    #[must_use]
    pub fn depth(&self) -> u32 {
        self.0.split('/').skip(2).count() as u32
    }

    /// 是否匹配前缀。
    #[must_use]
    pub fn matches_prefix(&self, prefix: &Self) -> bool {
        prefix.is_root()
            || self == prefix
            || self
                .0
                .strip_prefix(prefix.as_str())
                .is_some_and(|suffix| suffix.starts_with('/'))
    }

    /// 最末路径段。
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or("root")
    }

    /// 父路径。
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.is_root() {
            return None;
        }
        self.0
            .rsplit_once('/')
            .map(|(parent, _)| Self(parent.to_owned()))
    }
}

impl std::fmt::Display for AgentPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 控制面配置。
#[derive(Debug, Clone, Copy)]
pub struct AgentControlConfig {
    /// 最大活跃子代理数量，不包含 root。
    pub max_agents: usize,
    /// 最大同时运行的子代理 turn 数。
    pub max_active_turns: usize,
    /// 最大路径深度，root 为 0。
    pub max_depth: u32,
}

impl AgentControlConfig {}

impl Default for AgentControlConfig {
    fn default() -> Self {
        Self {
            max_agents: AGENT_CONTROL_DEFAULT_MAX_AGENTS,
            max_active_turns: AGENT_CONTROL_DEFAULT_MAX_ACTIVE_TURNS,
            max_depth: AGENT_CONTROL_DEFAULT_MAX_DEPTH,
        }
    }
}

/// Spawn 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawnRequest {
    /// 父代理路径。
    pub parent_path: AgentPath,
    /// 子任务路径段。
    pub task_name: String,
    /// 初始任务消息。
    pub message: String,
    /// 可选昵称。
    pub nickname: Option<String>,
    /// 可选角色。
    pub role: Option<String>,
}

/// 活跃代理元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlledAgent {
    /// 线程 ID。
    pub thread_id: u64,
    /// 规范路径。
    pub path: AgentPath,
    /// 可读昵称。
    pub nickname: Option<String>,
    /// 代理角色。
    pub role: Option<String>,
    /// 当前状态。
    pub status: AgentStatus,
    /// 最近任务消息。
    pub last_task_message: Option<String>,
}

/// 列表输出条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedControlledAgent {
    /// 规范路径。
    pub agent_name: String,
    /// 状态。
    pub status: AgentStatus,
    /// 最近任务消息。
    pub last_task_message: Option<String>,
}

/// Spawn 输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpawnReport {
    /// 新代理线程 ID。
    pub thread_id: u64,
    /// 规范路径。
    pub path: AgentPath,
    /// 可读昵称。
    pub nickname: Option<String>,
    /// 初始状态。
    pub status: AgentStatus,
}

/// 关闭输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCloseReport {
    /// 关闭前状态。
    pub previous_status: AgentStatus,
    /// 被关闭的代理数量，包含目标与子孙。
    pub closed_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentControlEdgeStatus {
    Open,
    Closed,
}

/// Sub-agent 控制面运行时。
#[derive(Debug, Clone)]
pub struct AgentControlRuntime {
    pub(crate) config: AgentControlConfig,
    pub(crate) next_thread_id: u64,
    pub(crate) next_execution_id: u64,
    pub(crate) agents: HashMap<AgentPath, ControlledAgent>,
    pub(crate) mailboxes: AgentMailboxStore,
    pub(crate) used_nicknames: HashSet<String>,
    /// mailbox 持久化槽位；未绑定时内核行为与 M2 一致（纯内存）。
    pub(crate) mailbox_persistence: MailboxPersistenceSlot,
    /// mailbox 作用域：同一 `AgentPath` 在不同根会话下必须隔离，也是幂等键的作用域前缀。
    pub(crate) mailbox_owner: String,
    reservations: Arc<Mutex<AgentControlReservations>>,
    pub(crate) running_turns: HashMap<AgentPath, AgentExecutionId>,
    child_index: HashMap<AgentPath, Vec<AgentPath>>,
    edges: HashMap<(AgentPath, AgentPath), AgentControlEdgeStatus>,
}

impl Default for AgentControlRuntime {
    fn default() -> Self {
        Self::new(AgentControlConfig::default())
    }
}

impl AgentControlRuntime {
    /// 创建控制面。
    #[must_use]
    pub fn new(config: AgentControlConfig) -> Self {
        Self::new_with_root_thread_id(config, 0)
    }

    /// 使用指定 root thread id 创建控制面。
    #[must_use]
    pub fn new_with_root_thread_id(config: AgentControlConfig, root_thread_id: u64) -> Self {
        let mut agents = HashMap::new();
        agents.insert(
            AgentPath::root(),
            ControlledAgent {
                thread_id: root_thread_id,
                path: AgentPath::root(),
                nickname: Some("root".to_owned()),
                role: None,
                status: AgentStatus::Running,
                last_task_message: Some("Main thread".to_owned()),
            },
        );
        Self {
            config,
            next_thread_id: root_thread_id.saturating_add(1),
            next_execution_id: 1,
            agents,
            mailboxes: AgentMailboxStore::default(),
            used_nicknames: HashSet::new(),
            mailbox_persistence: MailboxPersistenceSlot::default(),
            mailbox_owner: String::new(),
            reservations: AgentControlReservations::shared(),
            running_turns: HashMap::new(),
            child_index: HashMap::new(),
            edges: HashMap::new(),
        }
    }

    /// 返回当前控制面配置。
    #[must_use]
    pub const fn config(&self) -> AgentControlConfig {
        self.config
    }

    /// 当前 root 线程 ID。
    pub fn root_thread_id(&self) -> AxonResult<u64> {
        self.controlled_agent(&AgentPath::root())
            .map(|agent| agent.thread_id)
    }

    /// 注册一个子代理，完成 path 与昵称预留并提交元数据。
    pub fn spawn(&mut self, request: AgentSpawnRequest) -> AxonResult<AgentSpawnReport> {
        let reservation = self.reserve_spawn(&request)?;
        self.commit_spawn_reservation(reservation, request)
    }

    /// 预留 spawn 所需的 slot、path 与昵称。调用方失败时必须 drop 预留对象。
    pub fn reserve_spawn(
        &mut self,
        request: &AgentSpawnRequest,
    ) -> AxonResult<AgentSpawnReservation> {
        if request.message.trim().is_empty() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        if !self.agents.contains_key(&request.parent_path) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        let path = request.parent_path.join(&request.task_name)?;
        self.validate_spawn_path(&path)?;
        let nickname = self.reserve_nickname(request.nickname.clone())?;
        Ok(AgentSpawnReservation::reserve(
            Arc::clone(&self.reservations),
            path,
            nickname,
        ))
    }

    /// 提交 spawn 预留并注册代理。
    pub fn commit_spawn_reservation(
        &mut self,
        mut reservation: AgentSpawnReservation,
        request: AgentSpawnRequest,
    ) -> AxonResult<AgentSpawnReport> {
        if !reservation.is_active() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_RESERVATION_MISSING,
            ));
        }
        if !self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_path(reservation.path())
        {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_RESERVATION_MISSING,
            ));
        }
        if request.parent_path.join(&request.task_name)? != *reservation.path() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_RESERVATION_MISSING,
            ));
        }
        let path = reservation.path().clone();
        let nickname = reservation.nickname().map(str::to_owned);
        let thread_id = self.next_thread_id;
        self.next_thread_id = self.next_thread_id.saturating_add(1);
        let agent = ControlledAgent {
            thread_id,
            path: path.clone(),
            nickname: nickname.clone(),
            role: request.role,
            status: AgentStatus::PendingInit,
            last_task_message: Some(request.message),
        };
        self.agents.insert(path.clone(), agent);
        self.upsert_open_edge(request.parent_path, path.clone());
        self.mailboxes.ensure_agent(path.clone());
        if let Some(nickname) = nickname.as_ref() {
            self.used_nicknames.insert(nickname.clone());
        }
        {
            let mut reservations = self
                .reservations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            reservations.release(&path, nickname.as_deref());
        }
        reservation.commit();
        Ok(AgentSpawnReport {
            thread_id,
            path,
            nickname,
            status: AgentStatus::PendingInit,
        })
    }

    /// 更新状态。
    pub fn update_status(
        &mut self,
        path: &AgentPath,
        status: AgentStatus,
    ) -> AxonResult<AgentStatus> {
        let agent = self
            .agents
            .get_mut(path)
            .ok_or(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ))?;
        agent.status = status.clone();
        Ok(status)
    }

    /// 更新最近任务消息。
    pub fn update_last_task_message(
        &mut self,
        path: &AgentPath,
        message: Option<String>,
    ) -> AxonResult<()> {
        let agent = self
            .agents
            .get_mut(path)
            .ok_or(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ))?;
        agent.last_task_message = message;
        Ok(())
    }

    /// 解析代理引用。
    pub fn resolve_agent(
        &self,
        current: &AgentPath,
        reference: &str,
    ) -> AxonResult<&ControlledAgent> {
        let path = current.resolve(reference)?;
        self.agents
            .get(&path)
            .ok_or(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ))
    }

    /// 查询状态。
    #[must_use]
    pub fn status(&self, path: &AgentPath) -> AgentStatus {
        self.agents
            .get(path)
            .map_or(AgentStatus::NotFound, |agent| agent.status.clone())
    }

    /// 列出代理。
    pub fn list_agents(
        &self,
        current: &AgentPath,
        path_prefix: Option<&str>,
    ) -> AxonResult<Vec<ListedControlledAgent>> {
        let prefix = path_prefix
            .map(|value| current.resolve(value))
            .transpose()?
            .unwrap_or_else(AgentPath::root);
        let mut agents = self
            .agents
            .values()
            .filter(|agent| agent.path.matches_prefix(&prefix))
            .map(|agent| ListedControlledAgent {
                agent_name: agent.path.to_string(),
                status: agent.status.clone(),
                last_task_message: agent.last_task_message.clone(),
            })
            .collect::<Vec<_>>();
        agents.sort_by(|left, right| left.agent_name.cmp(&right.agent_name));
        Ok(agents)
    }

    /// 关闭代理及其子孙。
    pub fn close(&mut self, path: &AgentPath) -> AxonResult<AgentCloseReport> {
        if path.is_root() {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_INVALID_PATH,
            ));
        }
        let previous_status = self.status(path);
        if previous_status == AgentStatus::NotFound {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        let closing_paths = self
            .open_descendant_paths(path)
            .into_iter()
            .chain(std::iter::once(path.clone()))
            .collect::<Vec<_>>();
        let closed_count = closing_paths.len();
        for closing_path in closing_paths {
            self.close_edge_for_path(&closing_path);
            if let Some(agent) = self.agents.remove(&closing_path)
                && let Some(nickname) = agent.nickname
            {
                self.used_nicknames.remove(&nickname);
            }
            self.mailboxes.remove_agent(&closing_path);
            self.running_turns.remove(&closing_path);
        }
        Ok(AgentCloseReport {
            previous_status,
            closed_count,
        })
    }

    /// 活跃子代理数量，不包含 root。
    #[must_use]
    pub fn active_agent_count(&self) -> usize {
        self.agents
            .values()
            .filter(|agent| !agent.path.is_root())
            .count()
    }

    /// 运行时已占用或预留的子代理 slot 数。
    #[must_use]
    pub fn occupied_agent_slots(&self) -> usize {
        let reservations = self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.active_agent_count()
            .saturating_add(reservations.slots())
    }

    pub(crate) fn has_agent(&self, path: &AgentPath) -> bool {
        self.agents.contains_key(path)
    }

    pub(crate) fn controlled_agent(&self, path: &AgentPath) -> AxonResult<&ControlledAgent> {
        self.agents
            .get(path)
            .ok_or(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ))
    }

    pub(crate) fn closing_agent_edge(&self, path: &AgentPath) -> AxonResult<Option<(u64, u64)>> {
        if path.is_root() || self.status(path) == AgentStatus::NotFound {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ));
        }
        let Some(parent_path) = path.parent() else {
            return Ok(None);
        };
        let parent = self.controlled_agent(&parent_path)?;
        let child = self.controlled_agent(path)?;
        Ok(Some((parent.thread_id, child.thread_id)))
    }

    pub(crate) fn open_descendant_paths(&self, root: &AgentPath) -> Vec<AgentPath> {
        let mut descendants = Vec::new();
        let mut stack = self
            .child_index
            .get(root)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        while let Some(path) = stack.pop() {
            let Some(parent) = path.parent() else {
                continue;
            };
            if self.edge_status(&parent, &path) != Some(AgentControlEdgeStatus::Open) {
                continue;
            }
            descendants.push(path.clone());
            if let Some(children) = self.child_index.get(&path) {
                for child in children.iter().rev() {
                    stack.push(child.clone());
                }
            }
        }
        descendants
    }

    pub(crate) fn upsert_open_edge(&mut self, parent: AgentPath, child: AgentPath) {
        self.edges.insert(
            (parent.clone(), child.clone()),
            AgentControlEdgeStatus::Open,
        );
        let children = self.child_index.entry(parent).or_default();
        if !children.contains(&child) {
            children.push(child);
        }
    }

    fn close_edge_for_path(&mut self, path: &AgentPath) {
        let Some(parent) = path.parent() else {
            return;
        };
        if self.edges.contains_key(&(parent.clone(), path.clone())) {
            self.edges
                .insert((parent, path.clone()), AgentControlEdgeStatus::Closed);
        }
    }

    fn edge_status(&self, parent: &AgentPath, child: &AgentPath) -> Option<AgentControlEdgeStatus> {
        self.edges.get(&(parent.clone(), child.clone())).copied()
    }

    pub(crate) fn update_recipient_after_message(
        &mut self,
        recipient: &AgentPath,
        content: &str,
    ) -> AxonResult<AgentStatus> {
        let agent = self
            .agents
            .get_mut(recipient)
            .ok_or(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NOT_FOUND,
            ))?;
        agent.last_task_message = Some(content.to_owned());
        Ok(agent.status.clone())
    }

    fn validate_spawn_path(&self, path: &AgentPath) -> AxonResult<()> {
        let path_reserved = self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_path(path);
        if self.agents.contains_key(path) || path_reserved {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_PATH_EXISTS,
            ));
        }
        if path.depth() > self.config.max_depth {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_DEPTH_LIMIT_REACHED,
            ));
        }
        if self.occupied_agent_slots() >= self.config.max_agents {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_LIMIT_REACHED,
            ));
        }
        Ok(())
    }

    fn reserve_nickname(&mut self, requested: Option<String>) -> AxonResult<Option<String>> {
        let nickname = requested.unwrap_or_else(|| {
            format!(
                "{}-{}",
                AGENT_CONTROL_DEFAULT_NICKNAME_PREFIX, self.next_thread_id
            )
        });
        let reservations = self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.used_nicknames.contains(&nickname) || reservations.contains_nickname(&nickname) {
            return Err(AxonError::runtime_invalid_state(
                AGENT_CONTROL_REASON_NICKNAME_EXISTS,
            ));
        }
        Ok(Some(nickname))
    }
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_' || ch == '-')
}
