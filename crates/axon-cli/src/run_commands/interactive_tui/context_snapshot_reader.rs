//! 会话级上下文快照读取：TUI 在自己的进程里直接读运行态存储。
//!
//! 读端口是 `axon_session` 的 `ContextSnapshotStore`，服务端写入点只有事件追加的唯一投影
//! 入口，因此这里不会出现第二条写路径，也不存在两处各自维护读数的问题。
//!
//! 打开数据库只做一次：TUI 每帧都可能来读，按帧重跑初始化会白白付迁移与连接开销。

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};
use axon_tui_runtime::AgentTuiContextSnapshot;

/// 会话级上下文快照读取器。
pub(super) struct ContextSnapshotReader {
    config_path: PathBuf,
    store: Mutex<Option<SqliteRuntimeStore>>,
}

impl ContextSnapshotReader {
    /// 按配置路径创建读取器；数据库延迟打开。
    pub(super) fn new(config_path: &Path) -> Self {
        Self {
            config_path: config_path.to_path_buf(),
            store: Mutex::new(None),
        }
    }

    /// 读一次会话级快照。
    ///
    /// 没有会话标识、会话标识非法或还没有任何读数时返回 `None`：界面据此显示未知，
    /// 而不是拿 `0` 或上一次读数冒充。存储读失败只留告警，不阻断 TUI 主循环。
    pub(super) fn read(&self, session_id: Option<&str>) -> Option<AgentTuiContextSnapshot> {
        let session = DaemonSessionId::new(session_id?.to_owned()).ok()?;
        let store = self.store()?;
        match store.context_snapshot(&session) {
            Ok(Some(snapshot)) => Some(AgentTuiContextSnapshot::new(
                snapshot.current_context_tokens,
                snapshot.context_window,
                snapshot.rollover_before_tokens,
                snapshot.rollover_after_tokens,
                snapshot.updated_at_ms,
                snapshot.source,
            )),
            Ok(None) => None,
            Err(error) => {
                axon_logger::warning(&format!(
                    "[tui-context] session={} 读取上下文快照失败：{error}",
                    session.as_str()
                ));
                None
            }
        }
    }

    fn store(&self) -> Option<SqliteRuntimeStore> {
        let mut slot = self
            .store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = slot.as_ref() {
            return Some(store.clone());
        }
        let data_root = match crate::runtime_paths::runtime_axon_data_path_static(&self.config_path)
        {
            Ok(root) => root,
            Err(error) => {
                axon_logger::warning(&format!(
                    "[tui-context] Axon 运行态数据目录解析失败：{error}"
                ));
                return None;
            }
        };
        let store = SqliteRuntimeStore::from_root(data_root);
        if let Err(error) = store.initialize() {
            axon_logger::warning(&format!("[tui-context] 运行态库打开失败：{error}"));
            return None;
        }
        *slot = Some(store.clone());
        Some(store)
    }
}
