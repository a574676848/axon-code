use std::time::Duration;

use axon_provider_transport::PROVIDER_STREAM_IDLE_TIMEOUT_MS;

/// doctor 配置检查名称。
pub const DOCTOR_CHECK_CONFIG: &str = "config";
/// doctor CLI 检查名称。
pub const DOCTOR_CHECK_CLI: &str = "cli";
/// doctor 外部真实 provider 恢复验收回执检查名称。
pub const DOCTOR_CHECK_PROVIDER_RECOVERY: &str = "provider-recovery";
/// doctor vibe coding 预检检查名称。
pub const DOCTOR_CHECK_PREFLIGHT: &str = "preflight";
/// doctor provider 连通性检查名称。
pub const DOCTOR_CHECK_PROVIDER: &str = "provider";
/// doctor 工具注册表检查名称。
pub const DOCTOR_CHECK_TOOL: &str = "tool";
/// doctor MCP server 状态检查名称。
pub const DOCTOR_CHECK_MCP: &str = "mcp";
/// doctor memory store 状态检查名称。
pub const DOCTOR_CHECK_MEMORY: &str = "memory";
/// doctor LSP / diagnostics 基线检查名称。
pub const DOCTOR_CHECK_LSP: &str = "lsp";
/// doctor 本地运行目录与数据可写性检查名称。
pub const DOCTOR_CHECK_RUNTIME: &str = "runtime";
/// doctor 跨平台最小 smoke 检查名称。
pub const DOCTOR_CHECK_SMOKE: &str = "smoke";
/// provider 连通性测试超时。
pub const PROVIDER_TEST_TIMEOUT_MS: u64 = 5_000;

/// 解析 provider 流式响应空闲超时。
#[must_use]
pub(crate) fn provider_stream_idle_timeout(timeout_seconds: Option<u64>) -> Duration {
    timeout_seconds
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_millis(PROVIDER_STREAM_IDLE_TIMEOUT_MS))
}
