//! Agent TUI 内的 MCP OAuth 授权后台执行器。
//!
//! TUI 主循环不能阻塞在浏览器授权与本地回调上：`/mcp login` 只登记请求，
//! 由本模块在线程中执行 CLI 侧登录流程，完成后回投结果并触发 catalog 刷新。

use std::sync::mpsc::{self, Receiver, TryRecvError};

/// TUI 内的 MCP OAuth 授权动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiMcpAuthAction {
    /// 登录并保存 access token。
    Login,
    /// 清除本地 token 与待完成会话。
    Logout,
}

impl AgentTuiMcpAuthAction {
    /// 中文动作标签。
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Login => "登录",
            Self::Logout => "登出",
        }
    }
}

/// MCP OAuth 后台执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiMcpAuthOutcome {
    /// 本次执行的动作。
    pub action: AgentTuiMcpAuthAction,
    /// MCP server ID。
    pub server_id: String,
    /// 成功摘要或失败原因。
    pub result: Result<String, String>,
}

/// TUI 内待执行的 MCP OAuth 授权请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiMcpAuthRequest {
    /// 本次要执行的动作。
    pub action: AgentTuiMcpAuthAction,
    /// MCP server ID。
    pub server_id: String,
}

/// MCP OAuth 后台执行器。
pub struct AgentTuiMcpAuthLoader {
    action: AgentTuiMcpAuthAction,
    server_id: String,
    rx: Receiver<AgentTuiMcpAuthOutcome>,
}

const MCP_AUTH_THREAD_PANICKED: &str = "MCP 授权后台任务异常退出";
const MCP_AUTH_THREAD_START_FAILED: &str = "MCP 授权后台任务线程启动失败";
const MCP_AUTH_CHANNEL_DISCONNECTED: &str = "MCP 授权后台任务通道已断开";

impl AgentTuiMcpAuthLoader {
    /// 在线程中执行 MCP OAuth 动作。
    #[must_use]
    pub fn spawn(
        action: AgentTuiMcpAuthAction,
        server_id: String,
        run: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let tx_task = tx.clone();
        let thread_server_id = server_id.clone();
        let spawn_result = std::thread::Builder::new()
            .name(format!("axon-tui-mcp-auth-{server_id}"))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run))
                    .unwrap_or_else(|_| Err(MCP_AUTH_THREAD_PANICKED.to_owned()));
                let _ = tx_task.try_send(AgentTuiMcpAuthOutcome {
                    action,
                    server_id: thread_server_id,
                    result,
                });
            });
        if spawn_result.is_err() {
            let _ = tx.try_send(AgentTuiMcpAuthOutcome {
                action,
                server_id: server_id.clone(),
                result: Err(MCP_AUTH_THREAD_START_FAILED.to_owned()),
            });
        }
        Self {
            action,
            server_id,
            rx,
        }
    }

    /// 非阻塞获取执行结果。
    pub(in crate::r#loop::agent_tui) fn poll(&mut self) -> Option<AgentTuiMcpAuthOutcome> {
        match self.rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(AgentTuiMcpAuthOutcome {
                action: self.action,
                server_id: self.server_id.clone(),
                result: Err(MCP_AUTH_CHANNEL_DISCONNECTED.to_owned()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const OUTCOME_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn mcp_auth_loader_reports_success_outcome() {
        let mut loader = AgentTuiMcpAuthLoader::spawn(
            AgentTuiMcpAuthAction::Login,
            "sm_cloud_platform".to_owned(),
            || Ok("已授权".to_owned()),
        );

        let outcome = wait_for_outcome(&mut loader);

        assert_eq!(outcome.action, AgentTuiMcpAuthAction::Login);
        assert_eq!(outcome.server_id, "sm_cloud_platform");
        assert_eq!(outcome.result, Ok("已授权".to_owned()));
    }

    #[test]
    fn mcp_auth_loader_projects_panic_as_failure() {
        let mut loader = AgentTuiMcpAuthLoader::spawn(
            AgentTuiMcpAuthAction::Logout,
            "sm_cloud_platform".to_owned(),
            || panic!("fixture panic"),
        );

        let outcome = wait_for_outcome(&mut loader);

        assert_eq!(outcome.action, AgentTuiMcpAuthAction::Logout);
        assert_eq!(outcome.result, Err(MCP_AUTH_THREAD_PANICKED.to_owned()));
    }

    fn wait_for_outcome(loader: &mut AgentTuiMcpAuthLoader) -> AgentTuiMcpAuthOutcome {
        loader
            .rx
            .recv_timeout(OUTCOME_WAIT_TIMEOUT)
            .expect("MCP 授权后台任务应返回结果")
    }

    #[test]
    fn poll_is_nonblocking_and_reports_result_and_disconnection() {
        let server_id = "isolated-server".to_owned();
        let (tx, rx) = mpsc::sync_channel(1);
        let mut loader = AgentTuiMcpAuthLoader {
            action: AgentTuiMcpAuthAction::Login,
            server_id: server_id.clone(),
            rx,
        };
        assert!(loader.poll().is_none());

        let outcome = AgentTuiMcpAuthOutcome {
            action: loader.action,
            server_id,
            result: Ok("已授权".to_owned()),
        };
        tx.send(outcome.clone()).expect("结果应写入通道");
        assert_eq!(loader.poll(), Some(outcome));

        drop(tx);
        let disconnected = loader.poll().expect("应返回通道断开结果");
        assert_eq!(
            disconnected.result,
            Err(MCP_AUTH_CHANNEL_DISCONNECTED.to_owned())
        );
    }
}
