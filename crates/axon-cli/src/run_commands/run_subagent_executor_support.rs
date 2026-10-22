//! Provider 子代理执行器的纯支持逻辑。

use axon_agent_runtime::AgentPath;
use axon_config::{ModelConfig, ProviderConfig};

pub(crate) const SUBAGENT_SESSION_ID_SEPARATOR: &str = "--subagent--";

pub(crate) fn child_session_id(parent_session_id: &str, task_path: &AgentPath) -> String {
    let suffix = task_path.as_str().trim_start_matches('/').replace('/', "_");
    format!("{parent_session_id}{SUBAGENT_SESSION_ID_SEPARATOR}{suffix}")
}

#[derive(Debug, Clone)]
pub(super) struct ChildModelSelection {
    pub(super) provider: ProviderConfig,
    pub(super) model: ModelConfig,
    pub(super) channel_chain: Vec<(ModelConfig, ProviderConfig)>,
    pub(super) routing_key: String,
    pub(super) reasoning_effort: Option<String>,
}
