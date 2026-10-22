use serde::{Deserialize, Serialize};

pub use axon_domain::RuntimeFailure as WireRuntimeError;

/// Token usage 更新。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WireUsageUpdate {
    /// 输入 token。
    pub input_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// prompt cache read 输入 token。
    pub cached_input_tokens: u64,
    /// prompt cache write 输入 token。
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    /// reasoning 输出 token。
    #[serde(default)]
    pub reasoning_output_tokens: u64,
    /// 当前上下文 token。
    #[serde(default)]
    pub current_context_tokens: u64,
    /// 上下文构成估算：注入的系统提示词与上下文 fragment。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub system_prompt_tokens: u64,
    /// 上下文构成估算：模型可见的工具定义。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tool_definition_tokens: u64,
    /// 上下文构成估算：真实对话消息（含工具回执）。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub conversation_tokens: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl WireUsageUpdate {
    /// 完整输入 token，包含非缓存输入、缓存读取和缓存写入。
    #[must_use]
    pub const fn complete_input_tokens(self) -> u64 {
        self.input_tokens
            .saturating_add(self.cached_input_tokens)
            .saturating_add(self.cache_write_input_tokens)
    }

    /// 总 token，包含完整输入、输出和推理输出。
    #[must_use]
    pub const fn total_tokens(self) -> u64 {
        self.complete_input_tokens()
            .saturating_add(self.output_tokens)
            .saturating_add(self.reasoning_output_tokens)
    }

    /// 缓存读取占完整输入的命中率，单位为 basis points。
    #[must_use]
    pub fn cache_hit_rate_basis_points(self) -> u16 {
        axon_domain::ratio_basis_points(self.cached_input_tokens, self.complete_input_tokens())
    }
}

// 删除原 WireRuntimeError struct，RuntimeFailure 是 wire/session 共用的唯一事实模型。
