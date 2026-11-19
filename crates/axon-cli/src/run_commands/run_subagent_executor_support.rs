//! Provider 子代理执行器的纯支持逻辑。

use std::path::Path;

use axon_agent_runtime::AgentPath;
use axon_config::{ModelConfig, ProviderConfig};
use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore, SubagentSessionRequest};

/// 解析这个子代理的会话：同一个根会话下的同一个代理路径恒定落到同一条会话。
///
/// 会话标识是服务端 ULID，不再由「父会话 + 分隔符 + 路径」拼出来：关系落在
/// `sessions.root_session_id` 与 `sessions.agent_path` 两列上，因此这里必须查登记表。
/// 查询与登记是同一个入口，并发派发也不会长出两条子会话。
///
/// 返回这条会话的两个标识：会话身份，以及它自己的运行窗口（Thread 线程标识）。
pub(crate) fn resolve_child_session(
    config_path: &Path,
    parent_session_id: &str,
    cwd: &str,
    task_path: &AgentPath,
) -> Result<super::RunIdentities, String> {
    let root_session_id = DaemonSessionId::new(parent_session_id.to_owned())
        .map_err(|error| format!("父会话标识非法：{error:?}"))?;
    let store = super::run_agent_store::shared_runtime_store(config_path)?;
    let record = store
        .resolve_subagent_session(&SubagentSessionRequest {
            root_session_id,
            agent_path: task_path.as_str().to_owned(),
            cwd: cwd.to_owned(),
            now_ms: axon_time::unix_now_ms(),
        })
        .map_err(|error| format!("子代理会话登记失败：{error:?}"))?;
    let session = store
        .get_session(&record.session_id)
        .map_err(|error| format!("子代理会话不可读：{error:?}"))?;
    Ok(super::RunIdentities::new(
        session.session_id.as_str().to_owned(),
        session.current_run_id,
    ))
}

#[derive(Debug, Clone)]
pub(super) struct ChildModelSelection {
    pub(super) provider: ProviderConfig,
    pub(super) model: ModelConfig,
    pub(super) channel_chain: Vec<(ModelConfig, ProviderConfig)>,
    pub(super) routing_key: String,
    pub(super) reasoning_effort: Option<String>,
}
