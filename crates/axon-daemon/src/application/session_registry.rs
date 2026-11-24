//! Daemon Session Registry。

use axon_domain::DaemonSessionId;
use std::collections::BTreeSet;

/// 当前 Host 已知的 Session 索引。
#[derive(Debug, Default)]
pub struct SessionRegistry {
    sessions: BTreeSet<String>,
}

impl SessionRegistry {
    /// 注册 Session。
    pub fn insert(&mut self, session_id: DaemonSessionId) {
        self.sessions.insert(session_id.to_string());
    }
    /// 移除 Session。
    pub fn remove(&mut self, session_id: &DaemonSessionId) {
        self.sessions.remove(session_id.as_str());
    }
    /// 判断是否已注册。
    #[must_use]
    pub fn contains(&self, session_id: &DaemonSessionId) -> bool {
        self.sessions.contains(session_id.as_str())
    }
    /// 返回数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions.len()
    }
    /// 判断索引是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}
