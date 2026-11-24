//! Agent TUI 后台 catalog 加载器。

use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::{AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState};

const CATALOG_LOADER_THREAD_START_FAILED: &str = "catalog loader 线程启动失败";
const CATALOG_LOADER_THREAD_PANICKED: &str = "catalog loader 后台加载异常退出";
const CATALOG_LOADER_CHANNEL_DISCONNECTED: &str = "catalog loader 后台通道已断开";

/// Agent TUI 后台 catalog 加载器。
pub struct AgentTuiCatalogLoader {
    rx: Receiver<AgentTuiCatalogLoadSnapshot>,
}

impl AgentTuiCatalogLoader {
    /// 在线程中加载 catalog 快照。
    #[must_use]
    pub fn spawn(
        name: impl Into<String>,
        load: impl FnOnce() -> AgentTuiCatalogLoadSnapshot + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let thread_name = name.into();
        let tx_spawn = tx.clone();
        let spawn_result = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let snapshot = std::panic::catch_unwind(std::panic::AssertUnwindSafe(load))
                    .unwrap_or_else(|_| {
                        failed_catalog_load_snapshot(CATALOG_LOADER_THREAD_PANICKED)
                    });
                let _ = tx_spawn.try_send(snapshot);
            });
        if spawn_result.is_err() {
            let _ = tx.try_send(failed_catalog_load_snapshot(
                CATALOG_LOADER_THREAD_START_FAILED,
            ));
        }
        Self { rx }
    }

    pub(in crate::r#loop::agent_tui) fn poll(&mut self) -> Option<AgentTuiCatalogLoadSnapshot> {
        match self.rx.try_recv() {
            Ok(snapshot) => Some(snapshot),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(failed_catalog_load_snapshot(
                CATALOG_LOADER_CHANNEL_DISCONNECTED,
            )),
        }
    }
}

fn failed_catalog_load_snapshot(message: &str) -> AgentTuiCatalogLoadSnapshot {
    AgentTuiCatalogLoadSnapshot {
        skill_state: AgentTuiCatalogLoadState::Failed,
        skill_errors: vec![message.to_owned()],
        mcp_state: AgentTuiCatalogLoadState::Failed,
        ..AgentTuiCatalogLoadSnapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    #[test]
    fn catalog_loader_panic_projects_failed_snapshot() {
        let mut loader = AgentTuiCatalogLoader::spawn("axon-test-catalog-loader", || {
            panic!("fixture panic");
        });

        let snapshot = wait_for_catalog_loader(&mut loader);

        assert_eq!(snapshot.skill_state, AgentTuiCatalogLoadState::Failed);
        assert_eq!(snapshot.mcp_state, AgentTuiCatalogLoadState::Failed);
        assert_eq!(
            snapshot.skill_errors,
            vec![CATALOG_LOADER_THREAD_PANICKED.to_owned()]
        );
    }

    fn wait_for_catalog_loader(loader: &mut AgentTuiCatalogLoader) -> AgentTuiCatalogLoadSnapshot {
        for _ in 0..50 {
            if let Some(snapshot) = loader.poll() {
                return snapshot;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("catalog loader 未返回快照");
    }
}
