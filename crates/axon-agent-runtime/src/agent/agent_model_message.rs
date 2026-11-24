//! 协议无关的 Agent 模型消息合同。

use axon_domain::{OutputRef, ToolCallInput};
use serde::{Deserialize, Serialize};

use crate::agent_runtime_note::AgentRuntimeNoteKind;

mod agent_model_history_snapshot;

pub use agent_model_history_snapshot::AgentModelHistorySnapshot;

const ROLE_USER: &str = "user";
pub(crate) const TOOL_RESULT_SOURCE_REF_PREFIX: &str = "tool-call://";

/// 内部模型消息类型，用于稳定识别运行时插入的控制消息。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentModelMessageKind {
    /// 普通模型消息。
    #[default]
    Regular,
    /// Axon 注入的低权威运行时 note。
    RuntimeNote(AgentRuntimeNoteKind),
    /// Axon 注入的动态上下文：环境、本机资源、话题线索、异常状态、人格动态等。
    ///
    /// 与 `Regular + Context` 的区别在语义而非协议：`Regular + Context` 是跨轮稳定、
    /// 可进缓存的前缀片段；本变体面向本轮重建、不进缓存的注入内容，
    /// 让换窗与角色转换可以按 kind 精确取舍，而不必再猜 `cache_excluded`。
    InjectedContext,
    /// 历史压缩 checkpoint。
    ConversationCheckpoint,
}

/// Context Ledger 的最小保留级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// 用户目标、显式约束和验收标准。
    P0Contract,
    /// 当前状态和可继续执行的结构化事实。
    P1State,
    /// 工具输出、命令结果和可由引用取回的证据。
    P2Evidence,
    /// 可由其他事实推导的重复或临时内容。
    P3Derivable,
}

/// 模型消息来源，用于压缩时区分初始上下文与真实会话输入。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentModelMessageSource {
    /// Axon 注入的初始上下文 fragment。
    Context,
    /// 用户、assistant 或工具产生的会话消息。
    #[default]
    Conversation,
}

/// 模型消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentModelMessage {
    /// message role。
    pub role: String,
    /// message content。
    pub content: String,
    /// Provider 补充元信息。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<AgentModelMessageProviderMetadata>,
    /// 本地图片引用，仅在 Provider 请求阶段临时读取。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_images: Vec<AgentModelLocalImageRef>,
    /// 本地文件引用，仅以安全 preview 进入 Provider 请求。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_files: Vec<AgentModelLocalFileRef>,
    /// 是否排除在 prompt cache 锚点外。
    #[serde(default, skip_serializing_if = "is_false")]
    pub cache_excluded: bool,
    /// 消息来源。
    pub source: AgentModelMessageSource,
    /// 内部消息类型。
    pub kind: AgentModelMessageKind,
    /// Context Ledger 保留级别。
    pub retention_class: RetentionClass,
    /// 可选的稳定事实引用，不等同于归档引用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

#[derive(Deserialize)]
struct PersistedAgentModelMessage {
    role: String,
    content: String,
    #[serde(default)]
    provider_metadata: Option<AgentModelMessageProviderMetadata>,
    #[serde(default)]
    local_images: Vec<AgentModelLocalImageRef>,
    #[serde(default)]
    local_files: Vec<AgentModelLocalFileRef>,
    #[serde(default)]
    cache_excluded: bool,
    #[serde(default)]
    source: AgentModelMessageSource,
    #[serde(default)]
    kind: AgentModelMessageKind,
    #[serde(default)]
    retention_class: Option<RetentionClass>,
    #[serde(default)]
    source_ref: Option<String>,
}

impl<'de> Deserialize<'de> for AgentModelMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let persisted = PersistedAgentModelMessage::deserialize(deserializer)?;
        let retention_class = persisted
            .retention_class
            .unwrap_or_else(|| retention_class_for_role(&persisted.role));

        Ok(Self {
            role: persisted.role,
            content: persisted.content,
            provider_metadata: persisted.provider_metadata,
            local_images: persisted.local_images,
            local_files: persisted.local_files,
            cache_excluded: persisted.cache_excluded,
            source: persisted.source,
            kind: persisted.kind,
            retention_class,
            source_ref: persisted.source_ref,
        })
    }
}

/// Provider 可见的本地图片引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelLocalImageRef {
    /// 本地图片路径。
    pub path: String,
    /// MIME 类型。
    pub mime_type: String,
    /// 原始内容字节数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_len: Option<u64>,
    /// 原始内容 SHA-256。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// 可恢复的归档引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<String>,
    /// 来源追踪引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
}

/// Provider 无原生文件 block 时使用的协议无关文件引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelLocalFileRef {
    /// 文件显示名。
    pub name: String,
    /// 磁盘上的绝对路径；附件由本地文件登记时给出，模型据此可以自己读取原文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// MIME 类型。
    pub mime_type: String,
    /// 有界安全预览。
    pub preview: String,
    /// 原始字节数。
    pub byte_len: Option<u64>,
    /// 原始内容 SHA-256。
    pub sha256: Option<String>,
    /// 受控归档引用。
    pub archive_ref: Option<String>,
    /// 来源追踪引用。
    pub source_ref: Option<String>,
}

/// Provider 可见文件预览的最大 UTF-8 字节数。
pub const MAX_PROVIDER_FILE_PREVIEW_BYTES: usize = 32 * 1024;

impl AgentModelLocalImageRef {
    /// 创建只有本地路径的媒体引用；Provider 请求阶段再解析实际内容。
    #[must_use]
    pub fn local(path: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            mime_type: mime_type.into(),
            byte_len: None,
            sha256: None,
            archive_ref: None,
            source_ref: None,
        }
    }
}

/// 模型消息的 Provider 扩展元信息。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelMessageProviderMetadata {
    /// OpenAI-compatible reasoning_content / reasoning_details。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openai_reasoning_content: Option<String>,
    /// Provider 多轮工具环中的 assistant tool call 列表。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<AgentModelToolCallMetadata>>,
    /// Provider 多轮工具环中的单条 tool result。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<AgentModelToolResultMetadata>,
    /// Anthropic assistant thinking / redacted_thinking 块。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anthropic_thinking_blocks: Option<Vec<AnthropicThinkingBlockMetadata>>,
}

/// Anthropic assistant thinking 元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicThinkingBlockMetadata {
    /// block 类型：thinking 或 redacted_thinking。
    pub block_type: String,
    /// thinking 明文，仅真实 Provider 返回且具备签名时使用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// redacted_thinking 加密数据。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    /// Anthropic thinking 签名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// 协议无关的 assistant tool call 元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelToolCallMetadata {
    /// Provider tool call 稳定 ID。
    pub id: String,
    /// Provider 可见工具名。
    pub name: String,
    /// 协议无关的工具输入。
    pub input: ToolCallInput,
}

/// 协议无关的 tool result 元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentModelToolResultMetadata {
    /// 对应的 Provider tool call 稳定 ID。
    pub call_id: String,
    /// Provider 可见的工具输出正文。
    pub output: String,
    /// 工具是否成功完成。
    pub success: bool,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

impl AgentModelMessage {
    /// 判断当前消息是否是可作为历史操作边界的真实用户 turn。
    #[must_use]
    pub fn is_real_user_turn(&self) -> bool {
        self.role == ROLE_USER
            && self.source == AgentModelMessageSource::Conversation
            && self.kind == AgentModelMessageKind::Regular
    }

    /// 构造普通文本 message。
    #[must_use]
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        let role = role.into();
        let retention_class = retention_class_for_role(&role);
        Self {
            role,
            retention_class,
            content: content.into(),
            provider_metadata: None,
            local_images: Vec::new(),
            local_files: Vec::new(),
            cache_excluded: false,
            source: AgentModelMessageSource::Conversation,
            kind: AgentModelMessageKind::Regular,
            source_ref: None,
        }
    }

    /// 构造 Axon 注入的初始上下文消息。
    #[must_use]
    pub fn context(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            retention_class: RetentionClass::P1State,
            source: AgentModelMessageSource::Context,
            ..Self::text(role, content)
        }
    }

    /// 构造不参与 cache 的临时 message。
    #[must_use]
    pub fn cache_excluded(role: impl Into<String>, content: impl Into<String>) -> Self {
        let mut message = Self::text(role, content);
        message.cache_excluded = true;
        message.retention_class = RetentionClass::P3Derivable;
        message
    }

    /// 构造运行时注入的动态上下文消息。
    ///
    /// 面向本轮重建、不进缓存的内容：环境信息、本机资源、话题线索、异常状态、
    /// 人格动态、子代理提示等。与 `cache_excluded()` 的差别只在于显式标出
    /// `kind = InjectedContext`，让下游可以按 kind 判断「这是注入，不是用户发言」。
    #[must_use]
    pub fn injected_context(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            cache_excluded: true,
            retention_class: RetentionClass::P3Derivable,
            kind: AgentModelMessageKind::InjectedContext,
            ..Self::context(role, content)
        }
    }

    /// 附加本地图片引用。
    #[must_use]
    pub fn with_local_images(mut self, local_images: Vec<AgentModelLocalImageRef>) -> Self {
        self.local_images = local_images;
        self
    }

    /// 附加本地文件引用。
    #[must_use]
    pub fn with_local_files(mut self, local_files: Vec<AgentModelLocalFileRef>) -> Self {
        self.local_files = local_files;
        self
    }

    /// 附加 OpenAI-compatible reasoning_content。
    #[must_use]
    pub fn with_openai_reasoning_content(mut self, reasoning_content: impl Into<String>) -> Self {
        let mut metadata = self.provider_metadata.unwrap_or_default();
        metadata.openai_reasoning_content = Some(reasoning_content.into());
        self.provider_metadata = Some(metadata);
        self
    }

    /// 附加协议无关的 assistant tool call 列表。
    #[must_use]
    pub fn with_tool_calls(mut self, tool_calls: Vec<AgentModelToolCallMetadata>) -> Self {
        let mut metadata = self.provider_metadata.unwrap_or_default();
        metadata.tool_calls = Some(tool_calls);
        self.provider_metadata = Some(metadata);
        self
    }

    /// 构造协议无关的 tool result 消息。
    #[must_use]
    pub fn tool_result(
        call_id: impl Into<String>,
        output: impl Into<String>,
        success: bool,
    ) -> Self {
        let call_id = call_id.into();
        let mut message = Self::text("tool", "");
        message.retention_class = RetentionClass::P2Evidence;
        message.source_ref = Some(tool_result_source_ref(&call_id));
        message.provider_metadata = Some(AgentModelMessageProviderMetadata {
            tool_result: Some(AgentModelToolResultMetadata {
                call_id,
                output: output.into(),
                success,
            }),
            ..AgentModelMessageProviderMetadata::default()
        });
        message
    }

    /// 附加 Anthropic assistant thinking / redacted_thinking 块。
    #[must_use]
    pub fn with_anthropic_thinking_blocks(
        mut self,
        thinking_blocks: Vec<AnthropicThinkingBlockMetadata>,
    ) -> Self {
        let mut metadata = self.provider_metadata.unwrap_or_default();
        metadata.anthropic_thinking_blocks = Some(thinking_blocks);
        self.provider_metadata = Some(metadata);
        self
    }

    /// 附加稳定事实引用。
    #[must_use]
    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        let source_ref = source_ref.into();
        self.source_ref = (!source_ref.trim().is_empty()).then_some(source_ref);
        self
    }

    /// 将完整工具输出引用关联到消息的稳定事实引用。
    #[must_use]
    pub fn with_output_ref(self, output_ref: &OutputRef) -> Self {
        self.with_source_ref(output_ref.source_trace.clone())
    }

    /// 在存在完整工具输出引用时关联它。
    #[must_use]
    pub fn with_output_ref_if_present(self, output_ref: Option<&OutputRef>) -> Self {
        match output_ref {
            Some(output_ref) => self.with_output_ref(output_ref),
            None => self,
        }
    }
}

pub(crate) fn tool_result_source_ref(call_id: &str) -> String {
    format!("{TOOL_RESULT_SOURCE_REF_PREFIX}{call_id}")
}

/// 将文件预览限制在 Provider 传输边界内，避免历史消息重复放大。
#[must_use]
pub fn bound_provider_file_preview(preview: &str) -> String {
    truncate_utf8_bytes(preview, MAX_PROVIDER_FILE_PREVIEW_BYTES).to_owned()
}

/// 按 UTF-8 字符边界把文本截断到 `max_bytes` 以内。
#[must_use]
pub fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn retention_class_for_role(role: &str) -> RetentionClass {
    if role.eq_ignore_ascii_case("user") {
        RetentionClass::P0Contract
    } else if role.eq_ignore_ascii_case("tool") {
        RetentionClass::P2Evidence
    } else {
        RetentionClass::P1State
    }
}

#[cfg(test)]
#[path = "agent_model_message_tests.rs"]
mod tests;
