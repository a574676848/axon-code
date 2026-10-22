//! TUI 状态测试共享辅助。

use super::{AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiRunOverrides};

pub(super) fn launch_request(prompt: Option<&str>) -> AgentTuiLaunchRequest {
    AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: prompt.map(ToOwned::to_owned),
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog: AgentTuiCatalog::default(),
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    }
}
