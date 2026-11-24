//! Provider turn 共用的工具环境边界。

use std::sync::{Arc, RwLock};

use axon_agent_runtime::ToolRegistry;
/// 一次 Provider turn 可复用的工具环境快照。
#[derive(Clone)]
pub struct ProviderToolEnvironment {
    execution_tools: Arc<RwLock<ToolRegistry>>,
    skill_catalog_revision: String,
    mcp_schema_revision: String,
}

impl ProviderToolEnvironment {
    /// 使用已装配的注册表创建环境。
    #[must_use]
    pub fn new(
        execution_tools: Arc<RwLock<ToolRegistry>>,
        skill_catalog_revision: impl Into<String>,
        mcp_schema_revision: impl Into<String>,
    ) -> Self {
        Self {
            execution_tools,
            skill_catalog_revision: skill_catalog_revision.into(),
            mcp_schema_revision: mcp_schema_revision.into(),
        }
    }

    /// 当前 turn 使用的工具注册表。
    #[must_use]
    pub fn execution_tools(&self) -> Arc<RwLock<ToolRegistry>> {
        Arc::clone(&self.execution_tools)
    }

    /// Skill catalog 稳定 revision。
    #[must_use]
    pub fn skill_catalog_revision(&self) -> &str {
        &self.skill_catalog_revision
    }

    /// MCP schema 稳定 revision。
    #[must_use]
    pub fn mcp_schema_revision(&self) -> &str {
        &self.mcp_schema_revision
    }
}
