//! 子代理 spawn 预留状态。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::agent_control_runtime::AgentPath;

/// Spawn 预留共享状态。
#[derive(Debug, Default)]
pub(crate) struct AgentControlReservations {
    paths: HashSet<AgentPath>,
    nicknames: HashSet<String>,
    slots: usize,
}

impl AgentControlReservations {
    /// 创建共享预留状态。
    #[must_use]
    pub(crate) fn shared() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    /// 判断路径是否已预留。
    #[must_use]
    pub(crate) fn contains_path(&self, path: &AgentPath) -> bool {
        self.paths.contains(path)
    }

    /// 判断昵称是否已预留。
    #[must_use]
    pub(crate) fn contains_nickname(&self, nickname: &str) -> bool {
        self.nicknames.contains(nickname)
    }

    /// 当前已预留 slot 数。
    #[must_use]
    pub(crate) const fn slots(&self) -> usize {
        self.slots
    }

    fn reserve(&mut self, path: AgentPath, nickname: Option<&str>) {
        self.paths.insert(path);
        if let Some(nickname) = nickname {
            self.nicknames.insert(nickname.to_owned());
        }
        self.slots = self.slots.saturating_add(1);
    }

    pub(crate) fn release(&mut self, path: &AgentPath, nickname: Option<&str>) {
        self.paths.remove(path);
        if let Some(nickname) = nickname {
            self.nicknames.remove(nickname);
        }
        self.slots = self.slots.saturating_sub(1);
    }
}

/// Spawn 预留。Drop 时自动释放 path、nickname 与 slot。
#[derive(Debug)]
pub struct AgentSpawnReservation {
    path: AgentPath,
    nickname: Option<String>,
    reservations: Arc<Mutex<AgentControlReservations>>,
    slot_reserved: bool,
    committed: bool,
}

impl AgentSpawnReservation {
    /// 新建并注册一个预留。
    pub(crate) fn reserve(
        reservations: Arc<Mutex<AgentControlReservations>>,
        path: AgentPath,
        nickname: Option<String>,
    ) -> Self {
        reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reserve(path.clone(), nickname.as_deref());
        Self {
            path,
            nickname,
            reservations,
            slot_reserved: true,
            committed: false,
        }
    }

    /// 被预留的路径。
    #[must_use]
    pub(crate) const fn path(&self) -> &AgentPath {
        &self.path
    }

    /// 被预留的昵称。
    #[must_use]
    pub(crate) fn nickname(&self) -> Option<&str> {
        self.nickname.as_deref()
    }

    /// 标记预留已提交。
    pub(crate) fn commit(&mut self) {
        self.committed = true;
        self.slot_reserved = false;
    }

    /// 预留是否仍有效。
    #[must_use]
    pub(crate) const fn is_active(&self) -> bool {
        self.slot_reserved && !self.committed
    }
}

impl Drop for AgentSpawnReservation {
    fn drop(&mut self) {
        if !self.is_active() {
            return;
        }
        self.reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release(&self.path, self.nickname.as_deref());
    }
}
