use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::AxonConfig;

/// Setup 请求的来源，仅用于审计，不改变候选配置语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetupSource {
    /// 来自 CLI 请求。
    Cli,
    /// 来自 Agent 生成的 setup 意图。
    Agent,
    /// 来自交互式 Axon TUI 请求。
    Interactive,
    /// 来自本地配置文件初始化。
    Bootstrap,
}

/// 最小稳定的用户配置意图。
///
/// Provider/model 技术元数据不在这里暴露；它们由目录和运行时推导。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupIntent {
    /// 调用方观察到的配置修订号。
    pub base_revision: Option<String>,
    /// 主 Axon 模型选择。
    pub default_model: ExplicitEntry<String>,
    /// Provider 凭据对应的环境变量名。
    pub credential_env: ExplicitEntry<String>,
    /// 显式 reset 操作。
    pub actions: SetupActions,
    /// 请求来源。
    pub source: Option<SetupSource>,
    /// 用于幂等处理的稳定命令 ID。
    pub command_id: Option<String>,
}

/// 一个 setup 字段相对于当前配置的显式操作。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExplicitEntry<T> {
    /// 保持当前值不变。
    #[default]
    Unchanged,
    /// 将字段设置为指定值。
    Set(T),
    /// 清除字段值或恢复其默认语义。
    Clear,
}

impl<T> ExplicitEntry<T> {
    /// 判断该字段是否包含实际变更。
    #[must_use]
    pub fn is_changed(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    /// 获取设置操作携带的值。
    #[must_use]
    pub fn as_set(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            _ => None,
        }
    }
}

/// Setup 的 reset 语义。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupActions {
    /// 要移除的 Provider 名称；为空表示不重置 Provider。
    pub reset_provider: Option<String>,
    /// 是否重置 Agent 模型槽位。
    pub reset_agent_models: bool,
}

/// CLI、Agent JSON 和交互 adapter 合并后的显式字段集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupExplicitEntries {
    /// Provider 名称。
    pub provider_name: ExplicitEntry<String>,
    /// Provider 协议。
    pub protocol: ExplicitEntry<String>,
    /// Provider 基础 URL。
    pub base_url: ExplicitEntry<String>,
    /// Provider wire transport。
    pub wire_transport: ExplicitEntry<String>,
    /// Provider WebSocket URL。
    pub websocket_url: ExplicitEntry<String>,
    /// Provider 是否启用。
    pub provider_enabled: ExplicitEntry<bool>,
    /// Provider 优先级。
    pub provider_priority: ExplicitEntry<i32>,
    /// Provider 权重。
    pub provider_weight: ExplicitEntry<u32>,
    /// 模型 ID。
    pub model_id: ExplicitEntry<String>,
    /// 模型展示名称。
    pub model_name: ExplicitEntry<String>,
    /// 模型所属 Provider。
    pub model_provider: ExplicitEntry<String>,
    /// Agent 推理强度。
    pub agent_reasoning_effort: ExplicitEntry<String>,
    /// 模型上下文窗口大小。
    pub context_window: ExplicitEntry<u64>,
    /// 模型最大输出 token 数。
    pub max_output_tokens: ExplicitEntry<u64>,
    /// 模型是否支持视觉输入。
    pub supports_vision: ExplicitEntry<bool>,
    /// 模型是否支持 embedding。
    pub supports_embedding: ExplicitEntry<bool>,
    /// embedding 维度。
    pub embedding_dimension: ExplicitEntry<usize>,
    /// Provider API key 环境变量名。
    pub api_key_env: ExplicitEntry<String>,
    /// Provider API key 明文，仅在受控写入链路中使用。
    pub api_key: ExplicitEntry<String>,
    /// 额外配置键值。
    pub config_values: BTreeMap<String, ExplicitEntry<String>>,
}

impl SetupExplicitEntries {
    /// 判断显式字段或 reset 操作是否会改变配置。
    #[must_use]
    pub fn has_mutation(&self, actions: &SetupActions) -> bool {
        self.provider_name.is_changed()
            || self.protocol.is_changed()
            || self.base_url.is_changed()
            || self.wire_transport.is_changed()
            || self.websocket_url.is_changed()
            || self.provider_enabled.is_changed()
            || self.provider_priority.is_changed()
            || self.provider_weight.is_changed()
            || self.model_id.is_changed()
            || self.model_name.is_changed()
            || self.model_provider.is_changed()
            || self.agent_reasoning_effort.is_changed()
            || self.context_window.is_changed()
            || self.max_output_tokens.is_changed()
            || self.supports_vision.is_changed()
            || self.supports_embedding.is_changed()
            || self.embedding_dimension.is_changed()
            || self.api_key_env.is_changed()
            || self.api_key.is_changed()
            || !self.config_values.is_empty()
            || actions.reset_provider.is_some()
            || actions.reset_agent_models
    }
}

/// CLI、Agent JSON 和交互 adapter 提供给纯 planner 的非用户元数据。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SetupPlannerOptions {
    /// 合并后的显式字段。
    pub entries: SetupExplicitEntries,
    /// 只补齐缺失项，不覆盖已存在项。
    pub quick: bool,
    /// 交互层当前选中的 Provider。
    pub selected_provider_name: Option<String>,
    /// 交互层当前选中的模型 ID。
    pub selected_model_id: Option<String>,
    /// 交互层当前模型所属 Provider。
    pub selected_model_provider: Option<String>,
}

impl SetupIntent {
    /// 将已合并的显式字段转换为稳定的用户配置意图。
    #[must_use]
    pub fn from_explicit_entries(
        entries: &SetupExplicitEntries,
        actions: SetupActions,
        source: SetupSource,
        current: &AxonConfig,
    ) -> Self {
        let default_model = entries
            .model_id
            .as_set()
            .cloned()
            .or_else(|| entries.model_name.as_set().cloned())
            .filter(|model_ref| {
                current
                    .models
                    .iter()
                    .any(|model| model.id == *model_ref || model.name == *model_ref)
            })
            .map(ExplicitEntry::Set)
            .unwrap_or_default();
        Self {
            default_model,
            credential_env: entries.api_key_env.clone(),
            actions,
            source: Some(source),
            ..Self::default()
        }
    }
}
