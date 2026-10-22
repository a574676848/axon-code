//! Token 计数与估算抽象层。
//!
//! Provider 返回的 usage 始终是真值来源；本模块只服务本地预算、压缩阈值和预估展示。
//! OpenAI 系列模型使用 tiktoken BPE 精确计数，非 OpenAI 官方离线 tokenizer 不可用时必须标记为估算。

use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tiktoken_rs::{cl100k_base_singleton, o200k_base_singleton};

const HEURISTIC_CHARS_PER_TOKEN: u64 = 4;
const HEURISTIC_ENCODING_NAME: &str = "unicode_char_div_4";
const OPENAI_CL100K_BASE: &str = "cl100k_base";
const OPENAI_O200K_BASE: &str = "o200k_base";
const ANTHROPIC_ESTIMATE_ENCODING: &str = "anthropic_local_estimate";

/// Token 估算结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount {
    /// Token 数量。
    pub count: u64,
    /// 是否使用了精确 tokenizer（false 表示本地估算）。
    pub is_exact: bool,
}

impl TokenCount {
    /// 创建精确的 token 计数。
    #[must_use]
    pub const fn exact(count: u64) -> Self {
        Self {
            count,
            is_exact: true,
        }
    }

    /// 创建启发式的 token 计数。
    #[must_use]
    pub const fn heuristic(count: u64) -> Self {
        Self {
            count,
            is_exact: false,
        }
    }
}

/// Tokenizer 提供者类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenizerProvider {
    /// OpenAI/tiktoken 编码器。
    OpenAI,
    /// Anthropic Claude 编码器。
    Anthropic,
    /// 启发式回退。
    Heuristic,
}

/// Tokenizer 编码器标识。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenizerEncoding {
    /// 提供者。
    pub provider: TokenizerProvider,
    /// 编码器名称（如 "cl100k_base", "claude-3"）。
    pub encoding_name: String,
}

impl TokenizerEncoding {
    /// 创建 OpenAI 编码器。
    #[must_use]
    pub fn openai(encoding_name: impl Into<String>) -> Self {
        Self {
            provider: TokenizerProvider::OpenAI,
            encoding_name: encoding_name.into(),
        }
    }

    /// 创建 Anthropic 编码器。
    #[must_use]
    pub fn anthropic(encoding_name: impl Into<String>) -> Self {
        Self {
            provider: TokenizerProvider::Anthropic,
            encoding_name: encoding_name.into(),
        }
    }

    /// 创建启发式编码器。
    #[must_use]
    pub fn heuristic() -> Self {
        Self {
            provider: TokenizerProvider::Heuristic,
            encoding_name: HEURISTIC_ENCODING_NAME.to_owned(),
        }
    }
}

/// Tokenizer trait。
pub trait Tokenizer: Send + Sync + std::fmt::Debug {
    /// 计算文本的 token 数量。
    fn count_tokens(&self, text: &str) -> TokenCount;

    /// 获取编码器信息。
    fn encoding(&self) -> TokenizerEncoding;
}

/// 启发式 tokenizer（char/4 回退）。
#[derive(Debug, Clone, Copy)]
pub struct HeuristicTokenizer {
    /// 每个 token 的平均字符数。
    chars_per_token: u64,
}

impl Default for HeuristicTokenizer {
    fn default() -> Self {
        Self::new(HEURISTIC_CHARS_PER_TOKEN)
    }
}

impl HeuristicTokenizer {
    /// 创建启发式 tokenizer。
    #[must_use]
    pub const fn new(chars_per_token: u64) -> Self {
        Self { chars_per_token }
    }
}

impl Tokenizer for HeuristicTokenizer {
    fn count_tokens(&self, text: &str) -> TokenCount {
        let chars = text.chars().count() as u64;
        if chars == 0 {
            return TokenCount::heuristic(0);
        }
        let divisor = self.chars_per_token.max(1);
        let tokens = chars.div_ceil(divisor);
        TokenCount::heuristic(tokens)
    }

    fn encoding(&self) -> TokenizerEncoding {
        TokenizerEncoding::heuristic()
    }
}

/// OpenAI tiktoken tokenizer。
#[derive(Debug, Clone)]
pub struct OpenAITokenizer {
    encoding_name: String,
}

impl OpenAITokenizer {
    /// 创建 OpenAI tokenizer。
    #[must_use]
    pub fn new(encoding_name: impl Into<String>) -> Self {
        Self {
            encoding_name: encoding_name.into(),
        }
    }

    /// 创建 GPT-4/GPT-3.5 兼容 tokenizer。
    #[must_use]
    pub fn cl100k_base() -> Self {
        Self::new(OPENAI_CL100K_BASE)
    }

    /// 创建 GPT-4o/GPT-5/o-series/codex 兼容 tokenizer。
    #[must_use]
    pub fn o200k_base() -> Self {
        Self::new(OPENAI_O200K_BASE)
    }

    /// 创建 gpt-4 默认 tokenizer。
    #[must_use]
    pub fn gpt4() -> Self {
        Self::cl100k_base()
    }
}

impl Tokenizer for OpenAITokenizer {
    fn count_tokens(&self, text: &str) -> TokenCount {
        let tokens = match self.encoding_name.as_str() {
            OPENAI_O200K_BASE => o200k_base_singleton()
                .encode_with_special_tokens(text)
                .len(),
            OPENAI_CL100K_BASE => cl100k_base_singleton()
                .encode_with_special_tokens(text)
                .len(),
            _ => cl100k_base_singleton()
                .encode_with_special_tokens(text)
                .len(),
        };
        TokenCount::exact(tokens as u64)
    }

    fn encoding(&self) -> TokenizerEncoding {
        TokenizerEncoding::openai(&self.encoding_name)
    }
}

/// Anthropic Claude 本地 token 估算器。
///
/// Anthropic 精确 token 计数需使用 provider count-tokens API；离线估算不能标记为 exact。
#[derive(Debug, Clone)]
pub struct AnthropicTokenizer {
    model_name: String,
}

impl AnthropicTokenizer {
    /// 创建 Anthropic tokenizer。
    #[must_use]
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
        }
    }

    /// 创建 claude-3-5-sonnet 默认 tokenizer。
    #[must_use]
    pub fn claude_3_5_sonnet() -> Self {
        Self::new("claude-3-5-sonnet-20241022")
    }
}

impl Tokenizer for AnthropicTokenizer {
    fn count_tokens(&self, text: &str) -> TokenCount {
        HeuristicTokenizer::default().count_tokens(text)
    }

    fn encoding(&self) -> TokenizerEncoding {
        TokenizerEncoding::anthropic(format!("{ANTHROPIC_ESTIMATE_ENCODING}:{}", self.model_name))
    }
}

/// 智能 tokenizer 路由器。
///
/// 根据模型名称自动选择合适的 tokenizer。
#[derive(Debug, Clone)]
pub struct TokenizerRouter {
    /// 默认 tokenizer。
    default: Arc<dyn Tokenizer>,
}

impl Default for TokenizerRouter {
    fn default() -> Self {
        Self::new(Arc::new(HeuristicTokenizer::default()))
    }
}

impl TokenizerRouter {
    /// 创建路由器。
    #[must_use]
    pub fn new(default: Arc<dyn Tokenizer>) -> Self {
        Self { default }
    }

    /// 根据模型 ID 选择 tokenizer。
    #[must_use]
    pub fn select_for_model(&self, model_id: &str) -> Arc<dyn Tokenizer> {
        let model_lower = model_id.to_lowercase();

        if uses_o200k_base(&model_lower) {
            return Arc::new(OpenAITokenizer::o200k_base());
        }

        if uses_cl100k_base(&model_lower) {
            return Arc::new(OpenAITokenizer::cl100k_base());
        }

        if model_lower.contains("claude") || model_lower.contains("anthropic") {
            return Arc::new(AnthropicTokenizer::new(model_id));
        }

        Arc::clone(&self.default)
    }

    /// 计算文本 token 数（使用默认 tokenizer）。
    #[must_use]
    pub fn count_tokens(&self, text: &str) -> TokenCount {
        self.default.count_tokens(text)
    }

    /// 计算文本 token 数（使用指定模型的 tokenizer）。
    #[must_use]
    pub fn count_tokens_for_model(&self, model_id: &str, text: &str) -> TokenCount {
        let tokenizer = self.select_for_model(model_id);
        tokenizer.count_tokens(text)
    }
}

fn uses_o200k_base(model_lower: &str) -> bool {
    model_lower.contains("gpt-4o")
        || model_lower.contains("gpt-4.1")
        || model_lower.contains("gpt-4.5")
        || model_lower.contains("gpt-5")
        || model_lower.contains("o1")
        || model_lower.contains("o3")
        || model_lower.contains("o4")
        || model_lower.contains("codex")
        || model_lower.contains("gpt-oss")
}

fn uses_cl100k_base(model_lower: &str) -> bool {
    model_lower.contains("gpt-4")
        || model_lower.contains("gpt-3.5")
        || model_lower.contains("text-embedding-ada-002")
        || model_lower.contains("text-embedding-3")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heuristic_tokenizer_counts_chars() {
        let tokenizer = HeuristicTokenizer::new(4);
        let count = tokenizer.count_tokens("hello world");
        assert_eq!(count.count, 3); // 11 chars / 4 = 2.75 -> 3
        assert!(!count.is_exact);
    }

    #[test]
    fn openai_tokenizer_counts_with_tiktoken() {
        let tokenizer = OpenAITokenizer::cl100k_base();
        let count = tokenizer.count_tokens("hello world");
        assert_eq!(count.count, 2);
        assert!(count.is_exact);
    }

    #[test]
    fn anthropic_tokenizer_is_local_estimate_not_exact() {
        let tokenizer = AnthropicTokenizer::claude_3_5_sonnet();
        let count = tokenizer.count_tokens("hello world");
        assert_eq!(count.count, 3);
        assert!(!count.is_exact);
    }

    #[test]
    fn router_selects_appropriate_tokenizer_per_model() {
        let router = TokenizerRouter::default();
        let o200k = router.select_for_model("gpt-4o").encoding();
        assert_eq!(o200k.provider, TokenizerProvider::OpenAI);
        assert_eq!(o200k.encoding_name, OPENAI_O200K_BASE);

        let cl100k = router.select_for_model("gpt-4").encoding();
        assert_eq!(cl100k.provider, TokenizerProvider::OpenAI);
        assert_eq!(cl100k.encoding_name, OPENAI_CL100K_BASE);

        let fallback = router.select_for_model("gemini-pro").encoding();
        assert_eq!(fallback.provider, TokenizerProvider::Heuristic);
    }
}
